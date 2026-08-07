// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Expression lowering.

use crate::FieldAccess;
use super::{
    binop_result_type, is_type_constructor_name, lower_binop, lower_unaryop,
    operator_method_to_binop, operator_method_to_unaryop, LoopContext, LoweringError,
    MirLowerer, TypedOperand, HANDLE_NONE_SENTINEL,
};
use crate::{
    operand::MirConst, types::{EnumLayoutId, StructLayoutId},
    BlockId, FunctionRef, LocalId, MirOperand, MirRValue, MirStmt, MirStmtKind, MirTerminator,
    MirTerminatorKind, MirType,
};
use rask_ast::{
    expr::{BinOp, CallArg, Expr, ExprKind, UnaryOp},
    stmt::{Stmt, StmtKind},
    token::{FloatSuffix, IntSuffix},
};

/// Detect comparison patterns in assert conditions for smart failure messages.
///
/// Returns `Some((left_expr, right_expr, op_str, is_string))` if the condition
/// is a desugared comparison. After desugar: `a == b` → `a.eq(b)`,
/// `a != b` → `!(a.eq(b))`, `a < b` → `a.lt(b)`, etc.
fn extract_assert_comparison(condition: &Expr) -> Option<(&Expr, &Expr, &'static str, bool)> {
    match &condition.kind {
        // Desugared comparison: a.eq(b), a.lt(b), etc.
        ExprKind::MethodCall { object, method, args, .. } if args.len() == 1 => {
            let op_str = match method.as_str() {
                "eq" => "==",
                "lt" => "<",
                "gt" => ">",
                "le" => "<=",
                "ge" => ">=",
                _ => return None,
            };
            // Conservative: assume i64 unless one side is obviously a string literal
            let is_string = matches!(&object.kind, ExprKind::String(_))
                || matches!(&args[0].expr.kind, ExprKind::String(_));
            Some((object.as_ref(), &args[0].expr, op_str, is_string))
        }
        // Desugared !=: !(a.eq(b))
        ExprKind::Unary { op: UnaryOp::Not, operand } => {
            if let ExprKind::MethodCall { object, method, args, .. } = &operand.kind {
                if method == "eq" && args.len() == 1 {
                    let is_string = matches!(&object.kind, ExprKind::String(_))
                        || matches!(&args[0].expr.kind, ExprKind::String(_));
                    return Some((object.as_ref(), &args[0].expr, "!=", is_string));
                }
            }
            None
        }
        _ => None,
    }
}

/// Could evaluating this twice do something the second run would notice?
///
/// Only used to decide whether an assert operand needs parking in a temp, so it
/// leans the safe way: anything not obviously a read of something already there
/// counts as work. Reads stay as they were written, which matters — lowering
/// special-cases whole shapes like `x == none`, and swapping a side for a
/// temporary would step past them.
fn expr_may_do_work(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Ident(_)
        | ExprKind::Int(..)
        | ExprKind::Float(..)
        | ExprKind::String(_)
        | ExprKind::Bool(_)
        | ExprKind::Char(_)
        | ExprKind::None
        | ExprKind::Null => false,
        ExprKind::Field { object, .. } => expr_may_do_work(object),
        ExprKind::Index { object, index } => expr_may_do_work(object) || expr_may_do_work(index),
        ExprKind::Unary { operand, .. } => expr_may_do_work(operand),
        _ => true,
    }
}

/// Rebuild `a.eq(b)` (or `!a.eq(b)`) with the two sides swapped for expressions
/// that just read an already-computed value. The node ids are carried over, so
/// method dispatch still sees the same checker types it would have.
fn rebuild_assert_condition(condition: &Expr, left: Expr, right: Expr) -> Expr {
    let rebuild_call = |call: &Expr, left: Expr, right: Expr| -> Expr {
        let ExprKind::MethodCall { method, args, type_args, .. } = &call.kind else {
            return call.clone();
        };
        let mut args = args.clone();
        args[0].expr = right;
        Expr {
            id: call.id,
            span: call.span,
            kind: ExprKind::MethodCall {
                object: Box::new(left),
                method: method.clone(),
                args,
                type_args: type_args.clone(),
            },
        }
    };
    match &condition.kind {
        ExprKind::Unary { op: UnaryOp::Not, operand } => Expr {
            id: condition.id,
            span: condition.span,
            kind: ExprKind::Unary {
                op: UnaryOp::Not,
                operand: Box::new(rebuild_call(operand, left, right)),
            },
        },
        _ => rebuild_call(condition, left, right),
    }
}

/// Extract pattern name from an `is` pattern in an assert condition.
/// Returns the pattern name as a string for the failure message.
fn extract_assert_is_pattern(condition: &Expr) -> Option<String> {
    use rask_ast::expr::Pattern;
    match &condition.kind {
        ExprKind::IsPattern { pattern, .. } => {
            let name = match pattern {
                Pattern::Constructor { name, .. } => name.clone(),
                Pattern::Ident(n) => n.clone(),
                _ => return None,
            };
            Some(name)
        }
        _ => None,
    }
}

/// The width a Vec element actually occupies. Scalars live in 8-byte slots, so
/// a narrower type on the reading side sees half a value; aggregates keep their
/// own size.
fn vec_slot_type(ty: MirType) -> MirType {
    match ty {
        MirType::Struct(_) | MirType::Enum(_) => ty,
        _ if ty.size() < 8 => MirType::I64,
        _ => ty,
    }
}

/// Resolve primitive type associated constants (type.primitives/NT1):
/// `ZERO`, `ONE`, `MIN`, `MAX` on every numeric type.
fn primitive_type_constant(type_name: &str, field: &str) -> Option<TypedOperand> {
    if matches!(type_name, "f32" | "f64") {
        let val = match field {
            "ZERO" => 0.0,
            "ONE" => 1.0,
            "MIN" if type_name == "f32" => f32::MIN as f64,
            "MIN" => f64::MIN,
            "MAX" if type_name == "f32" => f32::MAX as f64,
            "MAX" => f64::MAX,
            "EPSILON" if type_name == "f32" => f32::EPSILON as f64,
            "EPSILON" => f64::EPSILON,
            "INFINITY" => f64::INFINITY,
            "NAN" => f64::NAN,
            _ => return None,
        };
        let ty = if type_name == "f32" { MirType::F32 } else { MirType::F64 };
        return Some((MirOperand::Constant(MirConst::Float(val)), ty));
    }

    // `MIN`/`MAX` per width; `ZERO`/`ONE` are the same everywhere. u64::MAX
    // rides in an i64 constant as its two's-complement bit pattern — the
    // width in `ty` is what tells codegen to read it unsigned.
    let (min, max, ty) = match type_name {
        "i8" => (i8::MIN as i64, i8::MAX as i64, MirType::I8),
        "i16" => (i16::MIN as i64, i16::MAX as i64, MirType::I16),
        "i32" => (i32::MIN as i64, i32::MAX as i64, MirType::I32),
        "i64" | "isize" => (i64::MIN, i64::MAX, MirType::I64),
        "u8" => (0, u8::MAX as i64, MirType::U8),
        "u16" => (0, u16::MAX as i64, MirType::U16),
        "u32" => (0, u32::MAX as i64, MirType::U32),
        "u64" | "usize" => (0, u64::MAX as i64, MirType::U64),
        _ => return None,
    };
    let val = match field {
        "MIN" => min,
        "MAX" => max,
        "ZERO" => 0,
        "ONE" => 1,
        _ => return None,
    };
    Some((MirOperand::Constant(MirConst::Int(val)), ty))
}

/// Disambiguation policy for method names that are shared across stdlib types
/// (e.g. `get`, `map`, `find`) or absent from the stub API (iterator terminals
/// like `collect`). Used ONLY when the type checker leaves the receiver type
/// unresolved — a resolved receiver type always takes precedence. This is a
/// deliberate policy choice (e.g. bare `.get()` means `Map`, since `Vec` uses
/// index syntax), not a lookup table for well-typed calls: adding or renaming
/// an unambiguous stdlib method needs no edit here.
fn ambiguous_method_prefix(method: &str, arg_count: usize) -> Option<&'static str> {
    Some(match method {
        "contains" | "substr" | "reverse" | "index_of"
        | "push_str" | "push_char" | "compare" | "as_ptr" => "string",
        "remove_at" | "to_vec" | "map" | "filter" | "collect"
        | "find" | "for_each" => "Vec",
        "join" if arg_count == 2 => "Vec",
        "values" | "get" | "insert" | "remove" => "Map",
        "sleep" => "time",
        "read_all" | "write_all" => "TcpConnection",
        "accept" => "TcpListener",
        "detach" => "TaskHandle",
        _ => return None,
    })
}

impl<'a> MirLowerer<'a> {
    /// Does this Vec receiver hold string elements? Drives the dispatch choice
    /// for the runtime entry points that need a real string compare.
    fn vec_elem_is_string(&self, object: &Expr) -> bool {
        Self::vec_tracking_key(object)
            .and_then(|key| self.meta(&key).and_then(|m| m.elem_type.clone())
                .or_else(|| self.ctx.shared_elem_types.borrow().get(&key).cloned()))
            .map_or(false, |ty| matches!(ty, MirType::String))
    }

    /// Wrap a plain value for a struct field declared `T?` or `T or E`.
    /// Returns the operand unchanged when no wrapping is needed — the field
    /// isn't a sum type, the value already has the sum shape, or the option
    /// uses a niche (a `Handle?` is a sentinel, not a tag plus payload).
    fn wrap_sum_field_value(
        &mut self,
        field_ty: Option<&MirType>,
        val_ty: &MirType,
        val: MirOperand,
    ) -> MirOperand {
        let Some(field_ty) = field_ty else { return val };
        let (tag_offset, payload_offset, inner) = match field_ty {
            // A `Handle?` is a sentinel, not a tag plus payload. A handle
            // stores as itself; `none` has to become the sentinel, because the
            // generic `none` lowering builds a tagged option and storing that
            // into the field left a tag where the handle belongs (#438).
            MirType::Option(inner) if matches!(**inner, MirType::Handle) => {
                if matches!(val_ty, MirType::Option(_)) {
                    return MirOperand::Constant(MirConst::Int(
                        crate::lower::HANDLE_NONE_SENTINEL,
                    ));
                }
                return val;
            }
            MirType::Option(inner) => {
                if matches!(val_ty, MirType::Option(_)) {
                    return val;
                }
                (
                    rask_mono::abi::OPTION_TAG_OFFSET,
                    rask_mono::abi::OPTION_PAYLOAD_OFFSET,
                    (**inner).clone(),
                )
            }
            MirType::Result { ok, err } => {
                // Only the ok side gets wrapped implicitly; an error value at a
                // field position isn't allowed (ER11).
                if matches!(val_ty, MirType::Result { .. }) || val_ty == &**err {
                    return val;
                }
                (
                    rask_mono::abi::RESULT_TAG_OFFSET,
                    rask_mono::abi::RESULT_PAYLOAD_OFFSET,
                    (**ok).clone(),
                )
            }
            _ => return val,
        };

        let slot = self.builder.alloc_temp(field_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: slot,
            offset: tag_offset,
            value: MirOperand::Constant(MirConst::Int(0)),
            store_size: Some(8),
        }));
        if payload_offset == rask_mono::abi::RESULT_PAYLOAD_OFFSET {
            for off in [
                rask_mono::abi::RESULT_ORIGIN_FILE_OFFSET,
                rask_mono::abi::RESULT_ORIGIN_LINE_OFFSET,
            ] {
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                    addr: slot,
                    offset: off,
                    value: MirOperand::Constant(MirConst::Int(0)),
                    store_size: Some(8),
                }));
            }
        }
        let payload_size = inner.size();
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: slot,
            offset: payload_offset,
            value: val,
            store_size: (payload_size > 8).then_some(payload_size),
        }));
        MirOperand::Local(slot)
    }

    /// Lower an absent optional. `none` and `None` are the same value written
    /// two ways, and both land here.
    ///
    /// A `Handle?` is a niche — the sentinel *is* the none, with no tag beside
    /// it. The checker's type is the primary signal for that, but at a bare
    /// `none` it's usually still unresolved, so the lowered type gets a look
    /// too; without the second check this built a tagged option for a slotless
    /// niche and stored its tag through a null address (#438).
    fn lower_none(&mut self, expr: &Expr) -> Result<TypedOperand, LoweringError> {
        let option_ty = self.lookup_expr_type(expr)
            .filter(|t| matches!(t, MirType::Option(_)))
            .unwrap_or_else(|| MirType::Option(Box::new(MirType::I64)));
        if self.option_is_niche(expr, &option_ty) {
            return Ok((MirOperand::Constant(MirConst::Int(HANDLE_NONE_SENTINEL)), MirType::Handle));
        }
        let result_local = self.builder.alloc_temp(option_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result_local,
            offset: 0,
            value: MirOperand::Constant(MirConst::Int(1)), // tag = None
            store_size: None,
        }));
        Ok((MirOperand::Local(result_local), option_ty))
    }

    /// Park an assert operand in a named local and hand back an expression that
    /// reads it. Keeps the original node id so the comparison built around it
    /// still resolves to the same type. A plain ident needs no parking — it's
    /// already a name, and re-reading it costs nothing.
    fn bind_assert_operand(&mut self, src: &Expr, op: &MirOperand, ty: &MirType) -> Expr {
        if !expr_may_do_work(src) {
            return src.clone();
        }
        let name = format!("__assert_operand_{}", self.closure_counter);
        self.closure_counter += 1;
        let local = match op {
            MirOperand::Local(id) => *id,
            other => {
                let local = self.builder.alloc_local(name.clone(), ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: local,
                    rvalue: MirRValue::Use(other.clone()),
                }));
                local
            }
        };
        self.locals.insert(name.clone(), (local, ty.clone()));
        Expr { id: src.id, span: src.span, kind: ExprKind::Ident(name) }
    }

    /// Widen a scalar operand to the width an assert-failure helper takes.
    /// Aggregates and strings pass through — they reach the helper as pointers,
    /// which is what it already expects.
    fn widen_for_assert_helper(
        &mut self,
        op: MirOperand,
        from: &MirType,
        want: &MirType,
    ) -> MirOperand {
        let needs_widening = match want {
            MirType::F64 => matches!(from, MirType::F32),
            MirType::I64 => matches!(from,
                MirType::I8 | MirType::I16 | MirType::I32
                | MirType::U8 | MirType::U16 | MirType::U32
                | MirType::Bool | MirType::Char),
            _ => false,
        };
        if !needs_widening {
            return op;
        }
        let dst = self.builder.alloc_temp(want.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst,
            rvalue: MirRValue::Cast { value: op, target_ty: want.clone() },
        }));
        MirOperand::Local(dst)
    }

    /// Resolve a MirType to its named type prefix using struct/enum layouts.
    pub(super) fn mir_type_name(&self, ty: &MirType) -> Option<String> {
        match ty {
            MirType::Struct(crate::types::StructLayoutId { id, .. }) => {
                self.ctx.struct_layouts.get(*id as usize).map(|l| l.name.clone())
            }
            MirType::Enum(crate::types::EnumLayoutId { id, .. }) => {
                self.ctx.enum_layouts.get(*id as usize).map(|l| l.name.clone())
            }
            MirType::String => Some("string".to_string()),
            MirType::F64 | MirType::F32 => Some("f64".to_string()),
            MirType::Bool => Some("bool".to_string()),
            MirType::Char => Some("char".to_string()),
            _ => None,
        }
    }

    /// Emit a TraitBox instruction: heap-allocate `value` and produce a trait object.
    /// Used for both explicit `as any Trait` casts and implicit TR5 coercions.
    fn emit_trait_box(
        &mut self,
        val: MirOperand,
        concrete_mir_ty: &MirType,
        trait_name: &str,
    ) -> (MirOperand, MirType) {
        let concrete_type = self.mir_type_name(concrete_mir_ty)
            .unwrap_or_else(|| "unknown".to_string());
        let concrete_size = self.elem_size_for_type(concrete_mir_ty) as u32;
        let vtable_name = format!(".vtable.{}__{}", concrete_type, trait_name);
        let trait_obj_ty = MirType::TraitObject { trait_name: trait_name.to_string() };
        let result_local = self.builder.alloc_temp(trait_obj_ty.clone());

        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::TraitBox {
            dst: result_local,
            value: val,
            concrete_type,
            trait_name: trait_name.to_string(),
            concrete_size,
            vtable_name,
        }));

        (MirOperand::Local(result_local), trait_obj_ty)
    }

    /// Parameter types a closure argument at position `i` should take, read off
    /// the callee's declared `func(...)` parameter. Empty when the callee is
    /// unknown or that parameter isn't a function type.
    fn expected_closure_param_tys(
        callee_params: &[Option<String>],
        i: usize,
    ) -> Vec<String> {
        callee_params
            .get(i)
            .and_then(|p| p.as_deref())
            .and_then(super::fn_type_param_strs)
            .unwrap_or_default()
    }

    /// How many element-typed parameters a collection method hands its closure.
    /// `sort_by(|a, b| …)` gets two elements, `any(|x| …)` one. `fold` isn't here
    /// — its first parameter is the accumulator, not an element.
    fn elem_closure_arity(method: &str) -> Option<usize> {
        Some(match method {
            "sort_by" | "min_by" | "max_by" => 2,
            "sort_by_key" | "min_by_key" | "max_by_key" | "any" | "all" | "find"
            | "position" | "retain" | "for_each" | "each" | "count_by" => 1,
            _ => return None,
        })
    }

    /// Parameter types for a closure whose arguments are collection elements.
    /// Without them the parameters default to `i64`, so a field read inside the
    /// body picks its index from whatever struct happens to declare that field
    /// name — `|a, b| a.priority.compare(b.priority)` on a `Vec<Ranked>`
    /// compiled to field 2 of an unrelated struct and a string comparison.
    fn elem_closure_param_tys(&self, object: &Expr, method: &str) -> Vec<String> {
        let Some(arity) = Self::elem_closure_arity(method) else { return Vec::new() };
        let Some(key) = Self::vec_tracking_key(object) else { return Vec::new() };
        let elem = self
            .meta(&key)
            .and_then(|m| m.elem_type.clone())
            .or_else(|| self.ctx.shared_elem_types.borrow().get(&key).cloned());
        let Some(name) = elem.and_then(|ty| self.mir_type_name(&ty)) else { return Vec::new() };
        vec![name; arity]
    }

    /// Derive a tracking key for Vec element type inference.
    /// Returns `"v"` for `v.push(x)` and `"self.field"` for `self.field.push(x)`.
    fn vec_tracking_key(object: &Expr) -> Option<String> {
        match &object.kind {
            ExprKind::Ident(name) => Some(name.clone()),
            ExprKind::Field { object: inner, field } => {
                if let ExprKind::Ident(name) = &inner.kind {
                    Some(format!("{}.{}", name, field))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Resolve a numeric field name on a tuple type.
    /// Returns (field_index, element_type, byte_offset, field_size).
    pub(super) fn resolve_tuple_field(
        ty: &MirType,
        field: &str,
    ) -> Option<(u32, MirType, Option<u32>, Option<u32>)> {
        let fields = match ty {
            MirType::Tuple(fields) => fields,
            _ => return None,
        };
        let idx: usize = field.parse().ok()?;
        if idx >= fields.len() {
            return None;
        }
        let elem_ty = fields[idx].clone();
        let mut offset = 0u32;
        for (i, f) in fields.iter().enumerate() {
            let align = f.align();
            offset = (offset + align - 1) & !(align - 1);
            if i == idx {
                break;
            }
            offset += f.size();
        }
        let size = elem_ty.size();
        Some((idx as u32, elem_ty, Some(offset), Some(size)))
    }

    /// #270: lower a call argument for a callee param. When `scalar_mutate` is
    /// `Some(scalar_ty)` the callee expects a by-pointer scalar `mutate` param, so
    /// pass an address instead of a value:
    ///   - a field/index projection → the place's address (write-back visible);
    ///   - a chained scalar-mutate param → its pointer, passed through;
    ///   - anything else (a whole Copy var) → the address of a spilled copy, so the
    ///     write-back is discarded (matching `modify_int(x)`).
    fn lower_call_arg(
        &mut self,
        arg: &Expr,
        scalar_mutate: Option<&MirType>,
    ) -> Result<TypedOperand, LoweringError> {
        let sty = match scalar_mutate {
            Some(t) => t.clone(),
            None => return self.lower_expr(arg),
        };
        // Chained: the arg is itself a by-pointer scalar mutate param — pass the
        // pointer straight through rather than loading + re-spilling it.
        if let ExprKind::Ident(name) = &arg.kind {
            if self.meta(name).and_then(|m| m.scalar_mutate_ptr.clone()).is_some() {
                if let Some((id, _)) = self.locals.get(name).cloned() {
                    return Ok((MirOperand::Local(id), MirType::Ptr));
                }
            }
        }
        // Field/index projection: pass the address of the place so the callee's
        // store lands in the caller's storage.
        if matches!(&arg.kind, ExprKind::Field { .. } | ExprKind::Index { .. }) {
            if let Some((base, offset, _)) = self.lower_place_chain(arg) {
                let addr = if offset == 0 {
                    MirOperand::Local(base)
                } else {
                    let tmp = self.builder.alloc_temp(MirType::Ptr);
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                        dst: tmp,
                        rvalue: MirRValue::BinaryOp {
                            op: crate::operand::BinOp::Add,
                            left: MirOperand::Local(base),
                            right: MirOperand::Constant(MirConst::Int(offset as i64)),
                        },
                    }));
                    MirOperand::Local(tmp)
                };
                return Ok((addr, MirType::Ptr));
            }
        }
        // Whole Copy var / other scalar expr: spill a copy and pass its address.
        let (val, _) = self.lower_expr(arg)?;
        let tmp = self.builder.alloc_temp(sty);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: tmp,
            rvalue: MirRValue::Use(val),
        }));
        let addr = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: addr,
            rvalue: MirRValue::Ref(tmp),
        }));
        Ok((MirOperand::Local(addr), MirType::Ptr))
    }

    pub(super) fn lower_expr(&mut self, expr: &Expr) -> Result<TypedOperand, LoweringError> {
        let (op, ty) = self.lower_expr_inner(expr)?;
        // TR5: a concrete value the checker flagged as flowing into an
        // `any Trait` position gets its vtable here — at the value, so every
        // use site is covered by one rule. Boxing at the call argument alone
        // left an annotated binding, a struct field and a collection element
        // holding a bare struct pointer that the first method call dispatched
        // through (#335, #474, #481).
        if !matches!(ty, MirType::TraitObject { .. }) {
            if let Some(trait_name) = self.ctx.trait_coercions.get(&expr.id).cloned() {
                return Ok(self.emit_trait_box(op, &ty, &trait_name));
            }
        }
        Ok((op, ty))
    }

    fn lower_expr_inner(&mut self, expr: &Expr) -> Result<TypedOperand, LoweringError> {
        self.builder.set_span(expr.span);
        match &expr.kind {
            // Literals
            ExprKind::Int(val, suffix) => {
                // Suffixed literals carry their type explicitly. Unsuffixed
                // literals follow the type checker's inference (default i32).
                let ty = match suffix {
                    Some(IntSuffix::I8) => MirType::I8,
                    Some(IntSuffix::I16) => MirType::I16,
                    Some(IntSuffix::I32) => MirType::I32,
                    Some(IntSuffix::I64) => MirType::I64,
                    Some(IntSuffix::U8) => MirType::U8,
                    Some(IntSuffix::U16) => MirType::U16,
                    Some(IntSuffix::U32) => MirType::U32,
                    Some(IntSuffix::U64) | Some(IntSuffix::U64ByMagnitude) => MirType::U64,
                    Some(IntSuffix::I128 | IntSuffix::U128 | IntSuffix::Isize | IntSuffix::Usize) => MirType::I64,
                    None => self
                        .ctx
                        .lookup_node_type(expr.id)
                        .filter(|t| matches!(t,
                            MirType::I8 | MirType::I16 | MirType::I32 | MirType::I64
                            | MirType::U8 | MirType::U16 | MirType::U32 | MirType::U64))
                        .unwrap_or(MirType::I64),
                };
                Ok((MirOperand::Constant(MirConst::Int(*val)), ty))
            }
            ExprKind::Float(val, suffix) => {
                let ty = match suffix {
                    Some(FloatSuffix::F32) => MirType::F32,
                    Some(FloatSuffix::F64) | None => MirType::F64,
                };
                Ok((MirOperand::Constant(MirConst::Float(*val)), ty))
            }
            ExprKind::String(s) => Ok((
                MirOperand::Constant(MirConst::String(s.clone())),
                MirType::String,
            )),
            ExprKind::Char(c) => Ok((MirOperand::Constant(MirConst::Char(*c)), MirType::Char)),
            ExprKind::Bool(b) => Ok((MirOperand::Constant(MirConst::Bool(*b)), MirType::Bool)),
            // StringInterp is desugared to concat chains before MIR lowering
            ExprKind::StringInterp(_) => unreachable!("StringInterp should be desugared before MIR"),
            ExprKind::Null => {
                // Null pointer literal — zero value
                Ok((MirOperand::Constant(MirConst::Int(0)), MirType::Ptr))
            }
            ExprKind::None => self.lower_none(expr),
            // Variable reference (or bare enum variant like None)
            ExprKind::Ident(name) => {
                // A module-level const with a non-literal initializer is emitted
                // on first use, not at every function's entry — everything below
                // then finds it in `locals` like any other binding.
                self.materialize_module_const(name)?;
                // #270: a scalar `mutate` param is a pointer — a bare read loads
                // the scalar through it (writes store through it; see stmt.rs).
                if let Some(sty) = self.meta(name).and_then(|m| m.scalar_mutate_ptr.clone()) {
                    if let Some((id, _)) = self.locals.get(name).cloned() {
                        let tmp = self.builder.alloc_temp(sty.clone());
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                            dst: tmp,
                            rvalue: MirRValue::Field {
                                base: MirOperand::Local(id),
                                field_index: 0,
                                byte_offset: Some(0),
                                access: FieldAccess::Sized(sty.size()),
                            },
                        }));
                        return Ok((MirOperand::Local(tmp), sty));
                    }
                }
                if let Some((id, ty)) = self.locals.get(name).cloned() {
                    // ER24/CF22: ER24 narrowing redefines the binding's type in
                    // the type checker's scope, but the MIR local keeps its
                    // declared type. When the use-site has a more specific
                    // narrowed type recorded, extract the payload from the
                    // wider Result/Option so downstream Field / method lookup
                    // operates on the narrowed type, not the wrapper.
                    let narrow_ty = self
                        .ctx
                        .lookup_node_type(expr.id)
                        .filter(|t| !matches!(t, MirType::Ptr));
                    let needs_narrow = matches!(
                        (&ty, &narrow_ty),
                        (MirType::Result { .. }, Some(n)) if !matches!(n, MirType::Result { .. })
                    ) || matches!(
                        (&ty, &narrow_ty),
                        (MirType::Option(_), Some(n)) if !matches!(n, MirType::Option(_))
                    );
                    // A niche `Handle?` keeps the handle as its whole value, so
                    // narrowing it is a rename, not an extraction. Reading
                    // field 0 loaded through the handle and crashed (#438's
                    // rule, missed on this path).
                    if needs_narrow && matches!(&ty, MirType::Option(inner) if **inner == MirType::Handle) {
                        return Ok((MirOperand::Local(id), narrow_ty.unwrap()));
                    }
                    if needs_narrow {
                        let inner_ty = narrow_ty.unwrap();
                        let inner_local = self.builder.alloc_temp(inner_ty.clone());
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                            dst: inner_local,
                            rvalue: MirRValue::Field {
                                base: MirOperand::Local(id),
                                field_index: 0,
                                byte_offset: None,
                                access: FieldAccess::Word,
                            },
                        }));
                        return Ok((MirOperand::Local(inner_local), inner_ty));
                    }
                    Ok((MirOperand::Local(id), ty))
                } else if name == "None" {
                    self.lower_none(expr)
                } else if let Some(meta) = self.ctx.comptime_globals.get(name) {
                    // Module-level comptime global reference
                    let global_local = self.builder.alloc_temp(MirType::Ptr);
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::GlobalRef {
                        dst: global_local,
                        name: name.clone(),
                    }));

                    if meta.type_prefix == "Vec" {
                        // Array global: wrap raw data into a Vec
                        let vec_local = self.builder.alloc_temp(MirType::I64);
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                            dst: Some(vec_local),
                            func: FunctionRef::internal("rask_vec_from_static".to_string()),
                            args: vec![
                                MirOperand::Local(global_local),
                                MirOperand::Constant(MirConst::Int(meta.elem_count as i64)),
                                // Comptime array globals hold i64 elements.
                                MirOperand::Constant(MirConst::Int(8)),
                            ],
                        }));
                        self.meta_mut(&name).type_prefix = Some("Vec".to_string());
                        Ok((MirOperand::Local(vec_local), MirType::I64))
                    } else {
                        // Scalar global: load value from the data pointer
                        let mir_ty = match meta.type_prefix.as_str() {
                            "bool" => MirType::Bool,
                            "i32" => MirType::I32,
                            "i64" => MirType::I64,
                            "f32" => MirType::F32,
                            "f64" => MirType::F64,
                            _ => MirType::I64,
                        };
                        let result_local = self.builder.alloc_temp(mir_ty.clone());
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                            dst: result_local,
                            rvalue: MirRValue::Deref(MirOperand::Local(global_local)),
                        }));
                        Ok((MirOperand::Local(result_local), mir_ty))
                    }
                } else {
                    Err(LoweringError::UnresolvedVariable(name.clone()))
                }
            }

            ExprKind::Binary { op, left, right } => {
                // Short-circuit `&&`/`||`: evaluate rhs only if lhs doesn't decide the result.
                if matches!(op, BinOp::And | BinOp::Or) {
                    return self.lower_short_circuit(*op, left, right);
                }

                let (left_op, left_ty) = self.lower_expr(left)?;
                let (right_op, _) = self.lower_expr(right)?;
                let mir_op = lower_binop(*op);
                let result_ty = binop_result_type(&mir_op, &left_ty);
                let result_local = self.builder.alloc_temp(result_ty.clone());

                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result_local,
                    rvalue: MirRValue::BinaryOp {
                        op: mir_op,
                        left: left_op,
                        right: right_op,
                    },
                }));

                Ok((MirOperand::Local(result_local), result_ty))
            }

            // Unary operations (only !, &, * survive desugar)
            ExprKind::Unary { op, operand } => {
                let (operand_op, operand_ty) = self.lower_expr(operand)?;
                let (result_ty, rvalue) = match op {
                    UnaryOp::Ref => {
                        let rv = match operand_op {
                            MirOperand::Local(id) => MirRValue::Ref(id),
                            _ => MirRValue::Use(operand_op),
                        };
                        (MirType::Ptr, rv)
                    }
                    UnaryOp::Deref => (operand_ty.clone(), MirRValue::Deref(operand_op)),
                    UnaryOp::Not => (MirType::Bool, MirRValue::UnaryOp {
                        op: lower_unaryop(*op),
                        operand: operand_op,
                    }),
                    _ => (operand_ty.clone(), MirRValue::UnaryOp {
                        op: lower_unaryop(*op),
                        operand: operand_op,
                    }),
                };

                let result_local = self.builder.alloc_temp(result_ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result_local,
                    rvalue,
                }));

                Ok((MirOperand::Local(result_local), result_ty))
            }

            // Function call — direct or through closure
            ExprKind::Call { func, args } => {
                // `Id(5)` on a nominal newtype is the value, not a call — there
                // is no `Id` function to dispatch to (#445).
                if let ExprKind::Ident(name) = &func.kind {
                    if args.len() == 1 {
                        if let Some((op, ty)) =
                            self.lower_newtype_wrap(name, Some(&args[0].expr))?
                        {
                            return Ok((op, ty));
                        }
                    }
                }
                // #270: peek the callee's scalar-`mutate` param classification so
                // those args are passed by address (write-back visible).
                let callee_smut: Vec<Option<MirType>> = match &func.kind {
                    ExprKind::Ident(name) => {
                        let key = self.ctx.call_rewrites.get(&expr.id).cloned()
                            .unwrap_or_else(|| name.clone());
                        self.func_sigs.get(&key)
                            .map(|s| s.scalar_mutate_params.clone())
                            .unwrap_or_default()
                    }
                    _ => Vec::new(),
                };
                // A closure handed to `spawn` outlives the frame that built it:
                // the task runs later, on another worker, and the runtime frees
                // the environment when it finishes. A scope-limited closure puts
                // that environment on the stack, so spawning one had the task
                // reading a dead frame and freeing a stack address — glibc aborted
                // with "free(): invalid pointer" right after the task ran (#463).
                let spawns_closure = matches!(&func.kind, ExprKind::Ident(n) if n == "spawn");
                let callee_params: Vec<Option<String>> = match &func.kind {
                    ExprKind::Ident(name) => {
                        let key = self.ctx.call_rewrites.get(&expr.id).cloned()
                            .unwrap_or_else(|| name.clone());
                        self.func_sigs.get(&key)
                            .map(|s| s.param_ty_strs.clone())
                            .unwrap_or_default()
                    }
                    _ => Vec::new(),
                };
                let mut arg_operands = Vec::new();
                let mut arg_mir_types = Vec::new();
                for (i, a) in args.iter().enumerate() {
                    let smut = callee_smut.get(i).and_then(|o| o.as_ref());
                    let (op, mir_ty) = if let ExprKind::Closure { params, ret_ty, body, is_own } = &a.expr.kind {
                        let expected = Self::expected_closure_param_tys(&callee_params, i);
                        self.lower_closure_expecting(
                            params, ret_ty.as_deref(), body,
                            *is_own || spawns_closure, &expected,
                        )?
                    } else {
                        self.lower_call_arg(&a.expr, smut)?
                    };
                    // TR5 boxing happens in lower_expr, at the value — doing
                    // it again here wrapped the box in another box, and the
                    // outer one had no concrete type to name its vtable after.
                    arg_operands.push(op);
                    arg_mir_types.push(mir_ty);
                }

                // Non-ident callees: field access, returned functions, etc.
                // Lower the callee expression and emit an indirect ClosureCall.
                let func_name = match &func.kind {
                    ExprKind::Ident(name) => {
                        // Check for monomorphized generic call rewrite
                        if let Some(mangled) = self.ctx.call_rewrites.get(&expr.id) {
                            mangled.clone()
                        } else {
                            name.clone()
                        }
                    }
                    _ => {
                        let (callee_op, _callee_ty) = self.lower_expr(func)?;
                        let callee_local = match callee_op {
                            MirOperand::Local(id) => id,
                            _ => {
                                let tmp = self.builder.alloc_temp(MirType::Ptr);
                                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                                    dst: tmp,
                                    rvalue: MirRValue::Use(callee_op),
                                }));
                                tmp
                            }
                        };
                        let ret_ty = self.lookup_expr_type(expr).unwrap_or(MirType::I64);
                        let result_local = self.builder.alloc_temp(ret_ty.clone());
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ClosureCall {
                            dst: Some(result_local),
                            closure: callee_local,
                            args: arg_operands,
                        }));
                        return Ok((MirOperand::Local(result_local), ret_ty));
                    }
                };

                // If the callee is a known closure variable, emit ClosureCall
                if self.closure_locals.contains(&func_name) {
                    if let Some((closure_local, _)) = self.locals.get(&func_name).cloned() {
                        let ret_ty = self.func_sigs
                            .get(&func_name)
                            .map(|s| s.ret_ty.clone())
                            .unwrap_or(MirType::I64);
                        let result_local = self.builder.alloc_temp(ret_ty.clone());
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ClosureCall {
                            dst: Some(result_local),
                            closure: closure_local,
                            args: arg_operands,
                        }));
                        return Ok((MirOperand::Local(result_local), ret_ty));
                    }
                }

                // transmute(val) — identity at MIR level (all values are i64)
                if func_name == "transmute" {
                    let val = arg_operands.into_iter().next()
                        .unwrap_or(MirOperand::Constant(MirConst::Int(0)));
                    return Ok((val, MirType::I64));
                }

                // todo()/unreachable() — desugar to panic() with descriptive message
                if func_name == "todo" || func_name == "unreachable" {
                    let prefix = if func_name == "todo" {
                        "not yet implemented"
                    } else {
                        "entered unreachable code"
                    };
                    let msg = if let Some(MirOperand::Constant(MirConst::String(s))) = arg_operands.first() {
                        format!("{}: {}", prefix, s)
                    } else {
                        prefix.to_string()
                    };
                    let msg_op = MirOperand::Constant(MirConst::String(msg));
                    let result_local = self.builder.alloc_temp(MirType::I64);
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: Some(result_local),
                        func: FunctionRef::internal("panic".to_string()),
                        args: vec![msg_op],
                    }));
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Unreachable));
                    let cont = self.builder.create_block();
                    self.builder.switch_to_block(cont);
                    return Ok((MirOperand::Local(result_local), MirType::I64));
                }

                // assert_eq(got, expected) — compare, report got/expected on failure.
                //
                // This used to hand both sides to a runtime function typed
                // (i64, i64). Two strings arrived as their addresses and never
                // matched; a float or a char didn't even fit the signature, so
                // Cranelift rejected the whole test function. Compare the same
                // way `assert a == b` does and pick the reporter by type.
                if func_name == "assert_eq" {
                    let got_op = arg_operands.first().cloned()
                        .unwrap_or(MirOperand::Constant(MirConst::Int(0)));
                    let expected_op = arg_operands.get(1).cloned()
                        .unwrap_or(MirOperand::Constant(MirConst::Int(0)));
                    let got_ty = arg_mir_types.first().cloned().unwrap_or(MirType::I64);
                    let expected_ty = arg_mir_types.get(1).cloned().unwrap_or(MirType::I64);

                    let is_string = matches!(got_ty, MirType::String)
                        || matches!(expected_ty, MirType::String);
                    let is_float = got_ty.is_float() || expected_ty.is_float();
                    let is_char = matches!(got_ty, MirType::Char)
                        && matches!(expected_ty, MirType::Char);
                    let is_bool = matches!(got_ty, MirType::Bool)
                        && matches!(expected_ty, MirType::Bool);

                    // Strings need the runtime's content compare; every other
                    // type has a MIR-level Eq that codegen already knows how
                    // to emit, aggregates included.
                    let cond_local = self.builder.alloc_temp(MirType::Bool);
                    if is_string {
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                            dst: Some(cond_local),
                            func: FunctionRef::internal("string_eq".to_string()),
                            args: vec![got_op.clone(), expected_op.clone()],
                        }));
                    } else {
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                            dst: cond_local,
                            rvalue: MirRValue::BinaryOp {
                                op: crate::operand::BinOp::Eq,
                                left: got_op.clone(),
                                right: expected_op.clone(),
                            },
                        }));
                    }

                    let ok_block = self.builder.create_block();
                    let fail_block = self.builder.create_block();
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                        cond: MirOperand::Local(cond_local),
                        then_block: ok_block,
                        else_block: fail_block,
                    }));

                    self.builder.switch_to_block(fail_block);
                    let scalar = matches!(got_ty,
                        MirType::Bool
                        | MirType::I8 | MirType::I16 | MirType::I32 | MirType::I64
                        | MirType::U8 | MirType::U16 | MirType::U32 | MirType::U64
                        | MirType::Ptr | MirType::Handle);
                    let (fail_fn, fail_args) = if is_string {
                        ("assert_eq_fail_str", vec![got_op, expected_op])
                    } else if is_float {
                        let want = MirType::F64;
                        ("assert_eq_fail_f64", vec![
                            self.widen_for_assert_helper(got_op, &got_ty, &want),
                            self.widen_for_assert_helper(expected_op, &expected_ty, &want),
                        ])
                    } else if is_bool || is_char || scalar {
                        let want = MirType::I64;
                        let name = if is_bool {
                            "assert_eq_fail_bool"
                        } else if is_char {
                            "assert_eq_fail_char"
                        } else {
                            "assert_eq_fail_i64"
                        };
                        (name, vec![
                            self.widen_for_assert_helper(got_op, &got_ty, &want),
                            self.widen_for_assert_helper(expected_op, &expected_ty, &want),
                        ])
                    } else {
                        // Structs, enums, tuples — compared correctly above,
                        // but there's no one-line rendering for the message.
                        ("assert_eq_fail", Vec::new())
                    };
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal(fail_fn.to_string()),
                        args: fail_args,
                    }));
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Unreachable));

                    self.builder.switch_to_block(ok_block);
                    return Ok((MirOperand::Constant(MirConst::Int(0)), MirType::Void));
                }

                // skip("reason") — set skip flag then unwind via rask_test_skip
                if func_name == "skip" {
                    let msg = if let Some(MirOperand::Constant(MirConst::String(s))) = arg_operands.first() {
                        s.clone()
                    } else {
                        "skipped".to_string()
                    };
                    let msg_op = MirOperand::Constant(MirConst::String(msg));
                    let result_local = self.builder.alloc_temp(MirType::I64);
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: Some(result_local),
                        func: FunctionRef::internal("rask_test_skip".to_string()),
                        args: vec![msg_op],
                    }));
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Unreachable));
                    let cont = self.builder.create_block();
                    self.builder.switch_to_block(cont);
                    return Ok((MirOperand::Local(result_local), MirType::I64));
                }

                // expect_fail() — mark test as expecting failure (returns, not noreturn)
                if func_name == "expect_fail" {
                    let result_local = self.builder.alloc_temp(MirType::I64);
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: Some(result_local),
                        func: FunctionRef::internal("rask_test_expect_fail".to_string()),
                        args: vec![],
                    }));
                    return Ok((MirOperand::Constant(MirConst::Int(0)), MirType::Void));
                }

                // Built-in variant constructors: Ok(v), Err(v), Some(v)
                match func_name.as_str() {
                    "Some" if self.is_niche_option_expr(expr) => {
                        // Niche: Some(handle) is just the handle value
                        let val = arg_operands.into_iter().next()
                            .unwrap_or(MirOperand::Constant(MirConst::Int(0)));
                        return Ok((val, MirType::Handle));
                    }
                    "Ok" | "Some" | "Err" => {
                        let tag = self.variant_tag(&func_name);
                        // Derive the result MirType from type checker info if available.
                        // Fallback uses the payload's actual type so aggregate payloads
                        // get a correctly-sized stack slot.
                        let payload_ty = arg_mir_types.first().cloned().unwrap_or(MirType::I64);
                        let fallback_ty = if func_name == "Some" {
                            MirType::Option(Box::new(payload_ty.clone()))
                        } else if func_name == "Ok" {
                            MirType::Result {
                                ok: Box::new(payload_ty.clone()),
                                err: Box::new(MirType::I64),
                            }
                        } else {
                            // Err
                            MirType::Result {
                                ok: Box::new(MirType::I64),
                                err: Box::new(payload_ty.clone()),
                            }
                        };
                        let result_ty = self.lookup_expr_type(expr)
                            .filter(|t| match t {
                                MirType::Result { .. } => true,
                                MirType::Option(_) => true,
                                _ => false,
                            })
                            .unwrap_or(fallback_ty);
                        let result_local = self.builder.alloc_temp(result_ty.clone());
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                            addr: result_local,
                            offset: 0,
                            value: MirOperand::Constant(MirConst::Int(tag)),
                            store_size: None,
                        }));
                        if let Some(payload) = arg_operands.first() {
                            let payload_offset = if matches!(result_ty, MirType::Result { .. }) {
                                crate::types::RESULT_PAYLOAD_OFFSET
                            } else {
                                8 // Option payload offset
                            };
                            // Result: zero origin fields
                            if matches!(result_ty, MirType::Result { .. }) {
                                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                                    addr: result_local,
                                    offset: crate::types::RESULT_ORIGIN_FILE_OFFSET,
                                    value: MirOperand::Constant(crate::operand::MirConst::Int(0)),
                                    store_size: None,
                                }));
                                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                                    addr: result_local,
                                    offset: crate::types::RESULT_ORIGIN_LINE_OFFSET,
                                    value: MirOperand::Constant(crate::operand::MirConst::Int(0)),
                                    store_size: None,
                                }));
                            }
                            // Set store_size for aggregate payloads (strings are 16 bytes)
                            let payload_store_size = if payload_ty.size() > 8 {
                                Some(payload_ty.size())
                            } else {
                                None
                            };
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                                addr: result_local,
                                offset: payload_offset,
                                value: payload.clone(),
                                store_size: payload_store_size,
                            }));
                        }
                        return Ok((MirOperand::Local(result_local), result_ty));
                    }
                    _ => {}
                }

                let ret_ty = self
                    .func_sigs
                    .get(&func_name)
                    .map(|s| s.ret_ty.clone())
                    .unwrap_or(MirType::I64);

                let result_local = self.builder.alloc_temp(ret_ty.clone());

                let func_ref = if self.ctx.extern_funcs.contains(&func_name) {
                    FunctionRef::extern_c(func_name)
                } else {
                    FunctionRef::internal(func_name)
                };
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: Some(result_local),
                    func: func_ref,
                    args: arg_operands,
                }));

                Ok((MirOperand::Local(result_local), ret_ty))
            }

            // If expression (spec L1)
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
                else_binding,
            } => self.lower_if(cond, then_branch, else_branch.as_deref(), else_binding.as_deref()),

            // Match expression (spec L2)
            ExprKind::Match { scrutinee, arms } => self.lower_match(scrutinee, arms),

            // Block expression
            ExprKind::Block(stmts) => self.lower_block(stmts),

            // Method call — operator methods from desugar become BinaryOp/UnaryOp,
            // type constructors become enum construction or static calls.
            ExprKind::MethodCall {
                object,
                method,
                args,
                type_args,
            } => self.lower_method_call(expr, object, method, args, type_args),

            // Field access
            ExprKind::Field { object, field } => {
                // Primitive type constants: i64.MAX, i32.MIN, etc.
                if let ExprKind::Ident(name) = &object.kind {
                    if let Some(val) = primitive_type_constant(name, field) {
                        return Ok(val);
                    }
                }

                // Unwrapping a nominal newtype is a no-op — `id.value` IS `id`.
                // It's transparent in MIR, so there's no aggregate to offset
                // into and a field load would dereference the value (#445).
                if field == "value" && self.expr_is_transparent_newtype(object) {
                    return self.lower_expr(object);
                }

                // Cross-package type access: pkg.Type → treat field as the type name.
                // Subsequent field access (pkg.DbError.NotFound) chains through
                // enum variant resolution on the resolved type.
                if let ExprKind::Ident(name) = &object.kind {
                    if self.ctx.package_modules.contains(name) {
                        // Look up the field as an enum type
                        if let Some((idx, layout)) = self.ctx.find_enum(field) {
                            let enum_ty = MirType::Enum(EnumLayoutId::new(idx, layout.size, layout.align));
                            let result_local = self.builder.alloc_temp(enum_ty.clone());
                            // Default-initialize (tag 0) — caller will likely access a variant
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                                addr: result_local,
                                offset: layout.tag_offset,
                                value: MirOperand::Constant(MirConst::Int(0)),
                                store_size: None,
                            }));
                            return Ok((MirOperand::Local(result_local), enum_ty));
                        }
                        // Look up as a struct type
                        if let Some((idx, sl)) = self.ctx.find_struct(field) {
                            let struct_ty = MirType::Struct(StructLayoutId::new(idx, sl.size, sl.align));
                            let result_local = self.builder.alloc_temp(struct_ty.clone());
                            return Ok((MirOperand::Local(result_local), struct_ty));
                        }
                        // Fallback: treat as an opaque type reference
                        let result_local = self.builder.alloc_temp(MirType::I64);
                        return Ok((MirOperand::Local(result_local), MirType::I64));
                    }
                }

                // Enum variant access: Color.Red (no parens, fieldless variant)
                if let ExprKind::Ident(name) = &object.kind {
                    if !self.locals.contains_key(name) {
                        if let Some((idx, layout)) = self.ctx.find_enum(name) {
                            if let Some(variant) = layout.variants.iter().find(|v| v.name == *field) {
                                let enum_ty = MirType::Enum(EnumLayoutId::new(idx, layout.size, layout.align));
                                let result_local = self.builder.alloc_temp(enum_ty.clone());
                                // Store discriminant tag
                                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                                    addr: result_local,
                                    offset: layout.tag_offset,
                                    value: MirOperand::Constant(MirConst::Int(variant.tag as i64)),
                                    store_size: None,
                                }));
                                return Ok((MirOperand::Local(result_local), enum_ty));
                            }
                        }
                        // Unknown enum type (built-in Error, etc.) — produce a
                        // tag-only stub so codegen can proceed.
                        if is_type_constructor_name(name) {
                            let tag = self.variant_tag(field);
                            return Ok((MirOperand::Constant(MirConst::Int(tag)), MirType::Ptr));
                        }
                    }
                }

                // `box.lock()/.read()/.write().field` — read a field of the
                // locked value: acquire → field access → release.
                if let Some((box_obj, acquire, release)) = self.sync_guard(object) {
                    let field = field.clone();
                    let ret_hint = self.ctx.lookup_raw_type(expr.id).map(|t| self.ctx.type_to_mir(t));
                    return self.lower_sync_guard_access(box_obj, acquire, release, ret_hint, move |g| Expr {
                        id: rask_ast::NodeId::DUMMY,
                        span: rask_ast::Span::new(0, 0),
                        kind: ExprKind::Field {
                            object: Box::new(g),
                            field,
                        },
                    });
                }

                let (obj_op, obj_ty) = self.lower_expr(object)?;

                // Resolve field index, type, and byte offset from struct layout.
                // byte_offset is passed to codegen so it doesn't need to re-derive
                // the offset (which would require knowing the struct type).
                let (field_index, result_ty, byte_offset, field_size) = if let MirType::Struct(StructLayoutId { id, .. }) = &obj_ty {
                    if let Some(layout) = self.ctx.struct_layouts.get(*id as usize) {
                        if let Some((idx, fl)) = layout.fields.iter().enumerate()
                            .find(|(_, f)| f.name == *field)
                        {
                            // Resolve field type from layout; if generic/unresolved,
                            // prefer the type checker's type for this expression.
                            let mut ft = self.ctx.resolve_type_str(&format!("{}", fl.ty));
                            if matches!(ft, MirType::Ptr | MirType::I64) {
                                if let Some(raw) = self.ctx.lookup_raw_type(expr.id) {
                                    let tc_ty = self.ctx.type_to_mir(raw);
                                    if !matches!(tc_ty, MirType::Ptr) {
                                        ft = tc_ty;
                                    }
                                }
                            }
                            (idx as u32, ft, Some(fl.offset), Some(fl.size))
                        } else {
                            (0, MirType::I64, None, None)
                        }
                    } else {
                        (0, MirType::I64, None, None)
                    }
                } else if let Some(resolved) = Self::resolve_tuple_field(&obj_ty, field) {
                    resolved
                } else {
                    // Object isn't MirType::Struct — try the type checker to
                    // resolve struct info (e.g. pool[h] returns Ptr but the
                    // type checker knows it's a struct).
                    let mut resolved = false;
                    let mut fi = 0u32;
                    let mut rt = MirType::I64;
                    let mut bo: Option<u32> = None;
                    let mut fs: Option<u32> = None;

                    // Strategy 1: Check type checker's node_types for the object
                    if let Some(raw_ty) = self.ctx.lookup_raw_type(object.id) {
                        let obj_mir = self.ctx.type_to_mir(raw_ty);
                        if let MirType::Struct(StructLayoutId { id: sid, .. }) = &obj_mir {
                            if let Some(layout) = self.ctx.struct_layouts.get(*sid as usize) {
                                if let Some((idx, fl)) = layout.fields.iter().enumerate()
                                    .find(|(_, f)| f.name == *field)
                                {
                                    fi = idx as u32;
                                    rt = self.ctx.resolve_type_str(&format!("{}", fl.ty));
                                    bo = Some(fl.offset);
                                    fs = Some(fl.size);
                                    resolved = true;
                                }
                            }
                        } else if let Some(tuple_resolved) = Self::resolve_tuple_field(&obj_mir, field) {
                            return {
                                let (ti, trt, tbo, tfs) = tuple_resolved;
                                let result_local = self.builder.alloc_temp(trt.clone());
                                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                                    dst: result_local,
                                    rvalue: MirRValue::Field {
                                        base: obj_op,
                                        field_index: ti,
                                        byte_offset: tbo,
                                        access: tfs.map_or(FieldAccess::Word, FieldAccess::Sized),
                                    },
                                }));
                                Ok((MirOperand::Local(result_local), trt))
                            };
                        }
                    }

                    // Strategy 2: If object is a variable, check its MIR local type
                    if !resolved {
                        if let ExprKind::Ident(var_name) = &object.kind {
                            if let Some((local_id, _)) = self.locals.get(var_name) {
                                let local_ty = self.builder.local_type(*local_id);
                                if let Some(MirType::Struct(StructLayoutId { id: sid, .. })) = local_ty {
                                    if let Some(layout) = self.ctx.struct_layouts.get(sid as usize) {
                                        if let Some((idx, fl)) = layout.fields.iter().enumerate()
                                            .find(|(_, f)| f.name == *field)
                                        {
                                            fi = idx as u32;
                                            rt = self.ctx.resolve_type_str(&format!("{}", fl.ty));
                                            bo = Some(fl.offset);
                                            fs = Some(fl.size);
                                            resolved = true;
                                        }
                                    }
                                } else if let Some(MirType::Tuple(_)) = &local_ty {
                                    if let Some(tuple_resolved) = Self::resolve_tuple_field(&local_ty.unwrap(), field) {
                                        fi = tuple_resolved.0;
                                        rt = tuple_resolved.1;
                                        bo = tuple_resolved.2;
                                        fs = tuple_resolved.3;
                                        resolved = true;
                                    }
                                }
                            }
                        }
                    }

                    // Strategy 3: Search all struct layouts for the field name
                    if !resolved {
                        for layout in self.ctx.struct_layouts.iter() {
                            if let Some((idx, fl)) = layout.fields.iter().enumerate()
                                .find(|(_, f)| f.name == *field)
                            {
                                fi = idx as u32;
                                rt = self.ctx.resolve_type_str(&format!("{}", fl.ty));
                                bo = Some(fl.offset);
                                fs = Some(fl.size);
                                resolved = true;
                                break;
                            }
                        }
                    }

                    if !resolved {
                        rt = self.ctx.lookup_node_type(expr.id)
                            .filter(|t| !matches!(t, MirType::Ptr))
                            .unwrap_or_else(|| {
                                eprintln!(
                                    "[mir] unresolved field `{}` — defaulting to I64 (should be caught by type checker)",
                                    field
                                );
                                MirType::I64
                            });
                    }

                    (fi, rt, bo, fs)
                };

                let result_local = self.builder.alloc_temp(result_ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Field {
                        base: obj_op,
                        field_index,
                        byte_offset,
                        access: field_size.map_or(FieldAccess::Word, FieldAccess::Sized),
                    },
                }));
                Ok((MirOperand::Local(result_local), result_ty))
            }

            // Dynamic field access: value.(expr) — should be resolved by comptime before MIR
            ExprKind::DynamicField { object, field_expr } => {
                let _ = (object, field_expr);
                Err(LoweringError::InvalidConstruct(
                    "dynamic field access (value.(expr)) must be resolved at comptime before MIR lowering".into()
                ))
            }

            // Index access
            ExprKind::Index { object, index } => {
                // Range index → slice operation: vec[start..end] or string[start..end]
                if let ExprKind::Range { start, end, .. } = &index.kind {
                    let (obj_op, obj_ty) = self.lower_expr(object)?;

                    // Determine if receiver is a string (MIR type, type checker, or local prefix)
                    let is_string = matches!(obj_ty, MirType::String)
                        || self.ctx.lookup_raw_type(object.id)
                            .map(|ty| matches!(ty, rask_types::Type::String))
                            .unwrap_or(false)
                        || if let ExprKind::Ident(var_name) = &object.kind {
                            self.meta(var_name)
                                .and_then(|m| m.type_prefix.as_deref())
                                .map(|p| p == "string")
                                .unwrap_or(false)
                        } else {
                            false
                        };

                    let start_op = if let Some(s) = start {
                        let (op, _) = self.lower_expr(s)?;
                        op
                    } else {
                        MirOperand::Constant(MirConst::Int(0))
                    };

                    if is_string {
                        // String slice: string_substr(s, start, end)
                        let end_op = if let Some(e) = end {
                            let (op, _) = self.lower_expr(e)?;
                            op
                        } else {
                            let len_local = self.builder.alloc_temp(MirType::I64);
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                                dst: Some(len_local),
                                func: FunctionRef::internal("string_len".to_string()),
                                args: vec![obj_op.clone()],
                            }));
                            MirOperand::Local(len_local)
                        };
                        let result_local = self.builder.alloc_temp(MirType::String);
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                            dst: Some(result_local),
                            func: FunctionRef::internal("string_substr".to_string()),
                            args: vec![obj_op, start_op, end_op],
                        }));
                        return Ok((MirOperand::Local(result_local), MirType::String));
                    }

                    // Vec slice: Vec_slice(v, start, end)
                    // end is None for open ranges (parts[2..]), use Vec_len
                    let end_op = if let Some(e) = end {
                        let (op, _) = self.lower_expr(e)?;
                        op
                    } else {
                        let len_local = self.builder.alloc_temp(MirType::I64);
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                            dst: Some(len_local),
                            func: FunctionRef::internal("Vec_len".to_string()),
                            args: vec![obj_op.clone()],
                        }));
                        MirOperand::Local(len_local)
                    };
                    let result_local = self.builder.alloc_temp(MirType::Ptr);
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: Some(result_local),
                        func: FunctionRef::internal("Vec_slice".to_string()),
                        args: vec![obj_op, start_op, end_op],
                    }));
                    return Ok((MirOperand::Local(result_local), MirType::Ptr));
                }

                let (obj_op, obj_ty) = self.lower_expr(object)?;
                let (idx_op, _) = self.lower_expr(index)?;

                // Fixed-size arrays: direct memory access (base + index * elem_size)
                if let MirType::Array { ref elem, .. } = obj_ty {
                    let elem_size = elem.size();
                    let result_ty = *elem.clone();
                    let result_local = self.builder.alloc_temp(result_ty.clone());
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                        dst: result_local,
                        rvalue: MirRValue::ArrayIndex {
                            base: obj_op,
                            index: idx_op,
                            elem_size,
                        },
                    }));
                    return Ok((MirOperand::Local(result_local), result_ty));
                }

                // Vec/Map/etc: dispatch through runtime
                // Try to determine the element type from the type checker,
                // then from tracked push/set calls, then default to I64
                let result_ty = self.ctx.lookup_node_type(expr.id)
                    .filter(|t| !matches!(t, MirType::Ptr))
                    .or_else(|| {
                        Self::vec_tracking_key(object).and_then(|key| {
                            self.meta(&key).and_then(|m| m.elem_type.clone())
                                .or_else(|| self.ctx.shared_elem_types.borrow().get(&key).cloned())
                        })
                    })
                    // Last resort: the receiver's own `Vec<T>`. Push tracking
                    // only sees Vecs built in this function, and the checker
                    // doesn't type every index node, so a Vec that arrived some
                    // other way — a field of a struct returned from a call, or of
                    // a `json.decode` result — fell through to i64 and
                    // `h.names[0]` printed a string's first bytes as a number.
                    .or_else(|| self.vec_elem_of_expr(object))
                    .unwrap_or(MirType::I64);
                let type_prefix = if let ExprKind::Ident(var_name) = &object.kind {
                        self.meta(var_name).and_then(|m| m.type_prefix.clone())
                    } else {
                        None
                    }
                    .or_else(|| {
                        self.ctx.lookup_raw_type(object.id)
                            .and_then(|ty| super::MirContext::type_prefix(ty, self.ctx.type_names))
                    });

                // Pool index: emit PoolCheckedAccess for generation checking.
                if self.index_object_is_pool(object) {
                    // If result_ty is I64 (default), try to extract the element type
                    // from the pool's generic parameter (Pool<Entity> → Entity)
                    let result_ty = if matches!(result_ty, MirType::I64) {
                        // Extract element type from the Pool<T> generic argument,
                        // whether the checker left it resolved (Generic) or not
                        // (UnresolvedGeneric).
                        self.ctx.lookup_raw_type(object.id)
                            .and_then(|ty| match ty {
                                rask_types::Type::Generic { args, .. }
                                | rask_types::Type::UnresolvedGeneric { args, .. } => {
                                    args.first().and_then(|a| match a {
                                        rask_types::GenericArg::Type(t) => Some(t.as_ref()),
                                        _ => None,
                                    })
                                }
                                _ => None,
                            })
                            .map(|elem_ty| self.ctx.type_to_mir(elem_ty))
                            .filter(|t| !matches!(t, MirType::Ptr | MirType::I64))
                            .unwrap_or(result_ty)
                    } else {
                        result_ty
                    };
                    let pool_local = match obj_op {
                        MirOperand::Local(id) => id,
                        _ => {
                            let tmp = self.builder.alloc_temp(MirType::I64);
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                                dst: tmp,
                                rvalue: MirRValue::Use(obj_op),
                            }));
                            tmp
                        }
                    };
                    let handle_local = match idx_op {
                        MirOperand::Local(id) => id,
                        _ => {
                            let tmp = self.builder.alloc_temp(MirType::I64);
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                                dst: tmp,
                                rvalue: MirRValue::Use(idx_op),
                            }));
                            tmp
                        }
                    };
                    let result_local = self.builder.alloc_temp(result_ty.clone());
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::PoolCheckedAccess {
                        dst: result_local,
                        pool: pool_local,
                        handle: handle_local,
                    }));
                    return Ok((MirOperand::Local(result_local), result_ty));
                }

                let index_name = type_prefix
                    .map(|prefix| {
                        // Strip generic parameters: "Vec<T>" → "Vec"
                        let base = prefix.split('<').next().unwrap_or(&prefix);
                        // Map indexing: `m[k]` panics on missing key — same shape
                        // as `Map_get_unwrap` (the unwrapping form of Map_get).
                        if base == "Map" {
                            "Map_get_unwrap".to_string()
                        } else {
                            format!("{}_index", base)
                        }
                    })
                    .unwrap_or_else(|| "index".to_string());
                let result_local = self.builder.alloc_temp(result_ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: Some(result_local),
                    func: FunctionRef::internal(index_name),
                    args: vec![obj_op, idx_op],
                }));
                Ok((MirOperand::Local(result_local), result_ty))
            }

            // Array literal
            ExprKind::Array(elems) => {
                // Lower elements first to determine the element type
                let mut lowered = Vec::new();
                let mut elem_ty = MirType::I32;
                for (i, elem) in elems.iter().enumerate() {
                    let (elem_op, ty) = self.lower_expr(elem)?;
                    if i == 0 {
                        elem_ty = ty;
                    }
                    lowered.push(elem_op);
                }
                let elem_size = elem_ty.size();
                let array_ty = MirType::Array {
                    elem: Box::new(elem_ty),
                    len: elems.len() as u32,
                };
                let result_local = self.builder.alloc_temp(array_ty.clone());
                for (i, elem_op) in lowered.into_iter().enumerate() {
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                        addr: result_local,
                        offset: i as u32 * elem_size,
                        value: elem_op,
                        // Deliberately unsized: an array of strings holds
                        // *pointers* to 16-byte values, not inline ones, and the
                        // whole read path expects that. Passing elem_size here
                        // makes codegen copy the value inline instead, which
                        // reads correctly right up until something indexes the
                        // array — see #414 for why the two disagree.
                        store_size: None,
                    }));
                }
                Ok((MirOperand::Local(result_local), array_ty))
            }

            // Tuple literal
            ExprKind::Tuple(elems) => {
                let mut elem_types = Vec::new();
                let mut lowered_elems = Vec::new();
                for elem in elems.iter() {
                    let (elem_op, elem_ty) = self.lower_expr(elem)?;
                    lowered_elems.push(elem_op);
                    elem_types.push(elem_ty);
                }
                let tuple_ty = MirType::Tuple(elem_types.clone());
                let result_local = self.builder.alloc_temp(tuple_ty.clone());
                let mut offset = 0u32;
                for (elem_op, elem_ty) in lowered_elems.into_iter().zip(elem_types.iter()) {
                    let elem_size = elem_ty.size();
                    let elem_align = elem_ty.align().max(1);
                    offset = (offset + elem_align - 1) & !(elem_align - 1);
                    // The element's own size, both ways. Wider than a word and
                    // the operand is a pointer to the data, not the data: a
                    // string constant lowers to the address of its 16-byte
                    // blob, and without a size that address landed in the slot,
                    // so reading `t.1` back gave garbage (#442). Narrower than
                    // a word and the store has to be narrow too — the offsets
                    // here are packed, so a full-word store of a 4-byte element
                    // runs into the next one (#548).
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                        addr: result_local,
                        offset,
                        value: elem_op,
                        store_size: Some(elem_size),
                    }));
                    offset += elem_size;
                }
                Ok((MirOperand::Local(result_local), tuple_ty))
            }

            // Struct literal
            ExprKind::StructLit { name, fields, spread } => {
                // A nominal newtype is transparent: `Id { value: 5 }` IS 5.
                // There's no layout to store into — treating it as an aggregate
                // stored through an uninitialised pointer (#445).
                if let Some((op, ty)) = self.lower_newtype_wrap(name, fields.first().map(|f| &f.value))? {
                    return Ok((op, ty));
                }
                // Check for enum variant constructor: "EnumName.VariantName { ... }"
                let (result_ty, layout, enum_variant_info) = if let Some(dot_pos) = name.find('.') {
                    let enum_name = &name[..dot_pos];
                    let variant_name = &name[dot_pos + 1..];
                    if let Some((idx, el)) = self.ctx.find_enum(enum_name) {
                        let variant_info = el.variants.iter().find(|v| v.name == variant_name)
                            .map(|v| (v.tag, v.payload_offset, v.fields.clone()));
                        (MirType::Enum(EnumLayoutId::new(idx, el.size, el.align)), None, variant_info)
                    } else if let Some((idx, sl)) = self.ctx.find_struct(name) {
                        (MirType::Struct(StructLayoutId::new(idx, sl.size, sl.align)), Some(sl), None)
                    } else {
                        (MirType::Ptr, None, None)
                    }
                } else if let Some((idx, sl)) = self.ctx.find_struct(name) {
                    (MirType::Struct(StructLayoutId::new(idx, sl.size, sl.align)), Some(sl), None)
                } else {
                    (MirType::Ptr, None, None)
                };

                let result_local = self.builder.alloc_temp(result_ty.clone());

                // For enum variants, store the tag first
                if let Some((tag, payload_offset, ref variant_fields)) = enum_variant_info {
                    // Store discriminant tag at offset 0
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                        addr: result_local,
                        offset: 0,
                        value: MirOperand::Constant(MirConst::Int(tag as i64)),
                        store_size: None,
                    }));
                    // Store fields at their offsets within the payload
                    for field in fields.iter() {
                        let (val_op, _) = self.lower_expr(&field.value)?;
                        let vf = variant_fields.iter()
                            .find(|f| f.name == field.name);
                        let offset = vf.map(|f| payload_offset + f.offset)
                            .unwrap_or(payload_offset);
                        let store_size = vf.map(|f| f.size);
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                            addr: result_local,
                            offset,
                            value: val_op,
                            store_size,
                        }));
                    }
                } else {
                for field in fields.iter() {
                    // The field's declared type is the only place a `Map.new()`
                    // initializer can learn its key/value sizes when the checker
                    // never typed this node (every stdlib body).
                    let saved_hint = self.field_type_hint.take();
                    self.field_type_hint = layout
                        .and_then(|sl| sl.fields.iter().find(|f| f.name == field.name))
                        .map(|f| format!("{}", f.ty));
                    let lowered = self.lower_expr(&field.value);
                    self.field_type_hint = saved_hint;
                    let (val_op, val_ty) = lowered?;
                    // Look up field offset and size from layout
                    let field_layout = layout
                        .and_then(|sl| sl.fields.iter().find(|f| f.name == field.name));
                    let offset = field_layout.map(|f| f.offset).unwrap_or(0);
                    let store_size = field_layout.map(|f| f.size);
                    // A `T?` or `T or E` field given a plain `T` has to be
                    // wrapped here. Stored bare, the value landed where the tag
                    // belongs — `Row { name: "bo" }` for a `string?` field put
                    // the string's first word in the tag slot and left the
                    // payload unwritten, so reading it back crashed (#376).
                    let field_mir_ty = field_layout.map(|f| {
                        // A `Handle?` field is a niche — the sentinel stands in
                        // for `none`. `type_to_mir` doesn't always keep the
                        // Handle inside the option, so read the declared type.
                        if super::is_niche_option_handle(&f.ty) {
                            MirType::Option(Box::new(MirType::Handle))
                        } else {
                            self.ctx.type_to_mir(&f.ty)
                        }
                    });
                    let val_op = self.wrap_sum_field_value(
                        field_mir_ty.as_ref(), &val_ty, val_op,
                    );
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                        addr: result_local,
                        offset,
                        value: val_op,
                        store_size,
                    }));

                    // Propagate Vec element types from source var to struct field.
                    // If v has known elem type F64 and we're constructing State { data: v },
                    // record "self.data" so methods can look it up.
                    if let ExprKind::Ident(src_var) = &field.value.kind {
                        if let Some(elem_ty) = self.meta(src_var).and_then(|m| m.elem_type.clone())
                            .or_else(|| self.ctx.shared_elem_types.borrow().get(src_var).cloned())
                        {
                            let field_key = format!("self.{}", field.name);
                            self.meta_mut(&field_key).elem_type = Some(elem_ty.clone());
                            self.ctx.record_shared_elem(field_key, elem_ty);
                        }
                    }
                }

                // Struct update syntax: `Point { x: 10, ..p }`. Every field not
                // given explicitly is copied from the spread base. Without this
                // the un-listed fields are left uninitialized and read garbage on
                // native (interp happens to zero-init).
                if let Some(spread_expr) = spread {
                    if let Some(sl) = layout {
                        let explicit: std::collections::HashSet<&str> =
                            fields.iter().map(|f| f.name.as_str()).collect();
                        // Clone the field layouts we need before lowering the
                        // spread expression (which borrows self mutably).
                        let missing: Vec<(usize, MirType, u32, u32)> = sl.fields.iter()
                            .enumerate()
                            .filter(|(_, f)| !explicit.contains(f.name.as_str()))
                            .map(|(i, f)| (i, self.ctx.type_to_mir(&f.ty), f.offset, f.size))
                            .collect();
                        let (base_op, _) = self.lower_expr(spread_expr)?;
                        for (idx, field_ty, offset, size) in missing {
                            let tmp = self.builder.alloc_temp(field_ty);
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                                dst: tmp,
                                rvalue: MirRValue::Field {
                                    base: base_op.clone(),
                                    field_index: idx as u32,
                                    byte_offset: Some(offset),
                                    access: FieldAccess::Sized(size),
                                },
                            }));
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                                addr: result_local,
                                offset,
                                value: MirOperand::Local(tmp),
                                store_size: Some(size),
                            }));
                        }
                    }
                }
                } // end else (non-enum-variant struct literal)
                Ok((MirOperand::Local(result_local), result_ty))
            }

            // If-let (if expr is Pattern { then } else { else })
            ExprKind::IfLet {
                expr,
                pattern,
                then_branch,
                else_branch,
            } => {
                let (val, val_ty) = self.lower_expr(expr)?;
                let is_niche = self.option_is_niche(expr, &val_ty);
                let tag = self.emit_option_tag(&val, is_niche);

                // Compare tag against expected variant. Use type-context
                // resolution so `if r is ErrEnum [as e]` against `T or ErrEnum`
                // routes to the err side (tag 1) instead of falling through to
                // 0 like the bare `pattern_tag` does.
                let expected = self.pattern_tag_in_type_context(pattern, &val_ty);
                let matches = self.builder.alloc_temp(MirType::Bool);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: matches,
                    rvalue: MirRValue::BinaryOp {
                        op: crate::operand::BinOp::Eq,
                        left: MirOperand::Local(tag),
                        right: MirOperand::Constant(MirConst::Int(expected)),
                    },
                }));

                let then_block = self.builder.create_block();
                let else_block = self.builder.create_block();
                let merge_block = self.builder.create_block();

                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: MirOperand::Local(matches),
                    then_block,
                    else_block,
                }));

                // Then block: bind payload, evaluate body
                self.builder.switch_to_block(then_block);
                // ER23: `Type as v` binds the matching side's payload. Which side
                // is decided by type identity (capitalization only as last resort,
                // inside pattern_is_err_side).
                let bind_ty = if let rask_ast::expr::Pattern::TypePat { ty_name, .. } = pattern {
                    let err_side = self.pattern_is_err_side(ty_name, &val_ty);
                    if let MirType::Result { ok, err } = &val_ty {
                        if err_side { *err.clone() } else { *ok.clone() }
                    } else if err_side {
                        self.extract_err_type(expr).unwrap_or(MirType::I64)
                    } else {
                        self.extract_payload_type(expr).unwrap_or(MirType::I64)
                    }
                } else {
                    self.extract_payload_type(expr).unwrap_or(MirType::I64)
                };
                self.bind_pattern_payload_niche(pattern, val, bind_ty, is_niche, &val_ty);
                let (then_val, then_ty) = self.lower_expr(then_branch)?;
                let result_local = self.builder.alloc_temp(then_ty.clone());
                if self.builder.current_block_unterminated() {
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                        dst: result_local,
                        rvalue: MirRValue::Use(then_val),
                    }));
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));
                }

                // Else block: evaluate else branch or default to zero-value
                self.builder.switch_to_block(else_block);
                if let Some(else_expr) = else_branch {
                    let (else_val, _) = self.lower_expr(else_expr)?;
                    if self.builder.current_block_unterminated() {
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                            dst: result_local,
                            rvalue: MirRValue::Use(else_val),
                        }));
                    }
                } else if self.builder.current_block_unterminated() {
                    // No else branch — initialize to default zero value
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                        dst: result_local,
                        rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
                    }));
                }
                if self.builder.current_block_unterminated() {
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));
                }

                self.builder.switch_to_block(merge_block);
                Ok((MirOperand::Local(result_local), then_ty))
            }

            // Guard pattern (const v = expr is Pattern else { diverge })
            ExprKind::GuardPattern {
                expr,
                pattern,
                else_branch,
            } => {
                let (val, val_ty) = self.lower_expr(expr)?;
                let is_niche = self.option_is_niche(expr, &val_ty);
                let tag = self.emit_option_tag(&val, is_niche);

                let expected = self.pattern_tag_in_type_context(pattern, &val_ty);
                let matches = self.builder.alloc_temp(MirType::Bool);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: matches,
                    rvalue: MirRValue::BinaryOp {
                        op: crate::operand::BinOp::Eq,
                        left: MirOperand::Local(tag),
                        right: MirOperand::Constant(MirConst::Int(expected)),
                    },
                }));

                let ok_block = self.builder.create_block();
                let else_block = self.builder.create_block();
                let merge_block = self.builder.create_block();

                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: MirOperand::Local(matches),
                    then_block: ok_block,
                    else_block,
                }));

                // Else branch diverges (return, panic, etc.)
                self.builder.switch_to_block(else_block);
                self.lower_expr(else_branch)?;
                // Only add unreachable if the else branch didn't already terminate
                // (e.g. via return or break)
                if self.builder.current_block_unterminated() {
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Unreachable));
                }

                // Ok block: bind payload and continue
                self.builder.switch_to_block(ok_block);
                let payload_ty = self.extract_payload_type(expr)
                    .unwrap_or(MirType::I64);
                self.bind_pattern_payload_niche(pattern, val.clone(), payload_ty.clone(), is_niche, &val_ty);
                // Extract the payload value for the result
                let payload = self.emit_option_payload(val, payload_ty.clone(), is_niche);
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

                self.builder.switch_to_block(merge_block);
                Ok((MirOperand::Local(payload), payload_ty))
            }

            // Pattern test (expr is Pattern) — evaluates to bool
            ExprKind::IsPattern { expr: inner, pattern } => {
                let (val, val_ty) = self.lower_expr(inner)?;
                let is_niche = self.option_is_niche(inner, &val_ty);
                let tag = self.emit_option_tag(&val, is_niche);
                let expected = self.pattern_tag_in_type_context(pattern, &val_ty);
                let result = self.builder.alloc_temp(MirType::Bool);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result,
                    rvalue: MirRValue::BinaryOp {
                        op: crate::operand::BinOp::Eq,
                        left: MirOperand::Local(tag),
                        right: MirOperand::Constant(MirConst::Int(expected)),
                    },
                }));
                Ok((MirOperand::Local(result), MirType::Bool))
            }

            // ER16: `try` propagates; ER17: a block operand propagates per `try`.
            ExprKind::Try { expr: inner } => self.lower_try(expr.id, inner),

            // ER14: `r catch <binder> => <body>`.
            ExprKind::Catch { value, ref clause } => self.lower_catch(value, clause),

            // OPT32: read the slot, write `none` back, hand back what was read.
            ExprKind::Take { place } => {
                let (val, ty) = self.lower_expr(place)?;
                let taken = self.builder.alloc_temp(ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: taken,
                    rvalue: MirRValue::Use(val),
                }));
                // The `none` carries the place's own node id so it picks up the
                // slot's option layout (niche or tagged) instead of guessing.
                let absent = Expr {
                    id: place.id,
                    kind: ExprKind::None,
                    span: expr.span,
                };
                self.lower_stmt(&rask_ast::stmt::Stmt {
                    id: expr.id,
                    kind: rask_ast::stmt::StmtKind::Assign {
                        target: (**place).clone(),
                        value: absent,
                    },
                    span: expr.span,
                })?;
                Ok((MirOperand::Local(taken), ty))
            }

            // Presence predicate (postfix ?) — evaluates to bool.
            // Some/Ok tag is 0 (present/ok); None/Err tag is 1.
            ExprKind::IsPresent { expr: inner, .. } => {
                let (val, _ty) = self.lower_expr(inner)?;
                let is_niche = self.option_is_niche(inner, &_ty);
                let tag = self.emit_option_tag(&val, is_niche);
                let result = self.builder.alloc_temp(MirType::Bool);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result,
                    rvalue: MirRValue::BinaryOp {
                        op: crate::operand::BinOp::Eq,
                        left: MirOperand::Local(tag),
                        right: MirOperand::Constant(MirConst::Int(0)),
                    },
                }));
                Ok((MirOperand::Local(result), MirType::Bool))
            }

            // Unwrap (postfix !) - panic on None/Err
            ExprKind::Unwrap { expr: inner, message: _ } => {
                let (val, _inner_ty) = self.lower_expr(inner)?;
                let is_niche = self.option_is_niche(inner, &_inner_ty);
                let tag_local = self.emit_option_tag(&val, is_niche);

                let ok_block = self.builder.create_block();
                let panic_block = self.builder.create_block();
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: MirOperand::Local(tag_local),
                    then_block: panic_block,
                    else_block: ok_block,
                }));

                self.builder.switch_to_block(panic_block);

                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: None,
                    func: FunctionRef::internal("panic_unwrap".to_string()),
                    args: vec![],
                }));
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Unreachable));

                self.builder.switch_to_block(ok_block);
                let payload_ty = self.extract_payload_type(inner)
                    .unwrap_or(MirType::I64);
                let result_local = self.emit_option_payload(val, payload_ty.clone(), is_niche);
                Ok((MirOperand::Local(result_local), payload_ty))
            }

            // Null coalescing (a ?? b)
            ExprKind::NullCoalesce { value, default } => {
                let (val, val_ty) = self.lower_expr(value)?;
                let is_niche = self.option_is_niche(value, &val_ty);
                let tag_local = self.emit_option_tag(&val, is_niche);

                let some_block = self.builder.create_block();
                let none_block = self.builder.create_block();
                let merge_block = self.builder.create_block();

                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: MirOperand::Local(tag_local),
                    then_block: none_block,
                    else_block: some_block,
                }));

                self.builder.switch_to_block(some_block);
                // The checker often leaves this node's type an unresolved var,
                // and reading the payload as an opaque pointer hands back the
                // slot's address instead of the value. The lowered receiver
                // knows its own ok type — take that when the checker has
                // nothing better.
                let payload_ty = Self::better_payload_ty(
                    self.extract_payload_type(value),
                    match &val_ty {
                        MirType::Result { ok, .. } => Some((**ok).clone()),
                        MirType::Option(inner) => Some((**inner).clone()),
                        _ => None,
                    },
                ).unwrap_or(MirType::I64);
                let result_local = self.emit_option_payload(val, payload_ty.clone(), is_niche);
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

                self.builder.switch_to_block(none_block);
                let (default_val, _) = self.lower_expr(default)?;
                // Guard against a divergent default (`?? continue` / `?? break` /
                // `?? return`): if it already terminated the block, don't emit
                // the assignment+goto into a dead block.
                if self.builder.current_block_unterminated() {
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                        dst: result_local,
                        rvalue: MirRValue::Use(default_val),
                    }));
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));
                }

                self.builder.switch_to_block(merge_block);
                Ok((MirOperand::Local(result_local), payload_ty))
            }

            // Range expression
            ExprKind::Range { start, end, inclusive } => {
                let result_ty = MirType::Ptr; // Range is an opaque struct
                let result_local = self.builder.alloc_temp(result_ty.clone());
                let mut args = Vec::new();
                if let Some(s) = start {
                    let (op, _) = self.lower_expr(s)?;
                    args.push(op);
                }
                if let Some(e) = end {
                    let (op, _) = self.lower_expr(e)?;
                    args.push(op);
                }
                let func_name = if *inclusive { "range_inclusive" } else { "range" };
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: Some(result_local),
                    func: FunctionRef::internal(func_name.to_string()),
                    args,
                }));
                Ok((MirOperand::Local(result_local), result_ty))
            }

            // Array repeat ([value; count])
            ExprKind::ArrayRepeat { value, count } => {
                // Constant count → expand to a fixed-size array (same as literal)
                if let ExprKind::Int(n, _) = &count.kind {
                    let (val, elem_ty) = self.lower_expr(value)?;
                    let len = *n as u32;
                    let elem_size = elem_ty.size();
                    let array_ty = MirType::Array { elem: Box::new(elem_ty), len };
                    let result_local = self.builder.alloc_temp(array_ty.clone());
                    for i in 0..len {
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                            addr: result_local,
                            offset: i * elem_size,
                            value: val.clone(),
                            store_size: None,
                        }));
                    }
                    return Ok((MirOperand::Local(result_local), array_ty));
                }

                // Dynamic count: keep existing Ptr-based fallback
                let (val, elem_ty) = self.lower_expr(value)?;
                let (cnt, _) = self.lower_expr(count)?;
                let result_ty = MirType::Ptr;
                let result_local = self.builder.alloc_temp(result_ty.clone());
                let elem_size = self.elem_size_for_type(&elem_ty);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: Some(result_local),
                    func: FunctionRef::internal("array_repeat".to_string()),
                    args: vec![val, cnt, MirOperand::Constant(MirConst::Int(elem_size))],
                }));
                Ok((MirOperand::Local(result_local), result_ty))
            }

            // Optional chaining (a?.b)
            ExprKind::OptionalField { object, field } => {
                // `obj?.field` lowers to:
                //   if obj is Some(t): Some(t.field)
                //   if obj is None:    None
                // The result type is Option<typeof(t.field)>, NOT the payload
                // type. Earlier lowering ignored the field name and just
                // unwrapped the Option, which collapsed the chain to garbage
                // for any non-leaf access (#271).
                let (obj, obj_opt_ty) = self.lower_expr(object)?;
                let is_niche = self.option_is_niche(object, &obj_opt_ty);
                let tag_local = self.emit_option_tag(&obj, is_niche);

                // Resolve the payload struct's layout to find the field's
                // index, type, and offset. Required for the Some-branch
                // load and to size the Option<field_ty> result.
                // Peel off any extra Option layers — the type checker's flatten
                // logic only fires when constraints have already resolved at
                // OptionalField time, so chained `?.` can store an
                // `Option<Option<T>>` raw type for the inner expression. We
                // want the bare T to look up the field on.
                let mut payload_ty = self.extract_payload_type(object)
                    .unwrap_or(MirType::I64);
                while let MirType::Option(inner) = payload_ty {
                    payload_ty = *inner;
                }
                let (field_index, field_ty, byte_offset, field_size) =
                    if let MirType::Struct(StructLayoutId { id, .. }) = &payload_ty {
                        if let Some(layout) = self.ctx.struct_layouts.get(*id as usize) {
                            if let Some((idx, fl)) = layout.fields.iter().enumerate()
                                .find(|(_, f)| f.name == *field)
                            {
                                let ft = self.ctx.resolve_type_str(&format!("{}", fl.ty));
                                (idx as u32, ft, Some(fl.offset), Some(fl.size))
                            } else {
                                (0, payload_ty.clone(), None, None)
                            }
                        } else {
                            (0, payload_ty.clone(), None, None)
                        }
                    } else {
                        // Non-struct payload (rare — e.g. tuple). Fall back to
                        // the old "just unwrap" shape so we don't regress.
                        (0, payload_ty.clone(), None, None)
                    };

                // Flatten: if the field is already T?, the result stays T?
                // (matches check_expr.rs behavior). In that case the Some
                // branch just copies the field-Option through; no extra wrap.
                let field_is_option = matches!(field_ty, MirType::Option(_));
                let result_ty = if field_is_option {
                    field_ty.clone()
                } else {
                    MirType::Option(Box::new(field_ty.clone()))
                };
                let result_local = self.builder.alloc_temp(result_ty.clone());

                let some_block = self.builder.create_block();
                let none_block = self.builder.create_block();
                let merge_block = self.builder.create_block();

                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: MirOperand::Local(tag_local),
                    then_block: none_block,
                    else_block: some_block,
                }));

                // Some branch: read field, then either copy through (flattened)
                // or wrap as Some.
                self.builder.switch_to_block(some_block);
                if byte_offset.is_some() {
                    // Read payload struct, then field from struct layout.
                    let payload_local = self.builder.alloc_temp(payload_ty.clone());
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                        dst: payload_local,
                        rvalue: MirRValue::Field {
                            base: obj.clone(),
                            field_index: 0,
                            byte_offset: None,
                            access: FieldAccess::Word,
                        },
                    }));
                    let field_local = self.builder.alloc_temp(field_ty.clone());
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                        dst: field_local,
                        rvalue: MirRValue::Field {
                            base: MirOperand::Local(payload_local),
                            field_index,
                            byte_offset,
                            access: field_size.map_or(FieldAccess::Word, FieldAccess::Sized),
                        },
                    }));
                    if field_is_option {
                        // result_local is the same Option<X> shape — just copy
                        // the bytes through. Codegen Assign(Option, Option)
                        // hits the src_option_ty needs_copy branch.
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                            dst: result_local,
                            rvalue: MirRValue::Use(MirOperand::Local(field_local)),
                        }));
                    } else {
                        // Wrap as Some: tag=0, payload at offset 8.
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                            addr: result_local,
                            offset: 0,
                            value: MirOperand::Constant(MirConst::Int(0)),
                            store_size: Some(8),
                        }));
                        let value_size = field_size.unwrap_or(8);
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                            addr: result_local,
                            offset: 8,
                            value: MirOperand::Local(field_local),
                            store_size: Some(value_size),
                        }));
                    }
                } else if is_niche {
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                        addr: result_local,
                        offset: 0,
                        value: MirOperand::Constant(MirConst::Int(0)),
                        store_size: Some(8),
                    }));
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                        addr: result_local,
                        offset: 8,
                        value: obj.clone(),
                        store_size: Some(field_ty.size()),
                    }));
                } else {
                    // Payload with no struct layout to find the field in. The
                    // access can't be performed, so this keeps the older
                    // "unwrap and pass the payload through" shape rather than
                    // inventing a field read.
                    //
                    // It used to write nothing at all — not even the tag — so
                    // the result was whatever the slot happened to hold. No
                    // program in the corpus reaches here and no reduction found
                    // one either (#367), but an empty branch under a `Some` is
                    // not something to leave sitting there.
                    let payload_local = self.builder.alloc_temp(payload_ty.clone());
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                        dst: payload_local,
                        rvalue: MirRValue::Field {
                            base: obj.clone(),
                            field_index: 0,
                            byte_offset: None,
                            access: FieldAccess::Word,
                        },
                    }));
                    if field_is_option {
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                            dst: result_local,
                            rvalue: MirRValue::Use(MirOperand::Local(payload_local)),
                        }));
                    } else {
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                            addr: result_local,
                            offset: 0,
                            value: MirOperand::Constant(MirConst::Int(0)),
                            store_size: Some(8),
                        }));
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                            addr: result_local,
                            offset: 8,
                            value: MirOperand::Local(payload_local),
                            store_size: Some(payload_ty.size().max(1)),
                        }));
                    }
                }
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

                // None branch: write None-tag (1) at offset 0.
                self.builder.switch_to_block(none_block);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                    addr: result_local,
                    offset: 0,
                    value: MirOperand::Constant(MirConst::Int(1)),
                    store_size: Some(8),
                }));
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

                self.builder.switch_to_block(merge_block);
                Ok((MirOperand::Local(result_local), result_ty))
            }

            // Closure — synthesize a separate MIR function and emit ClosureCreate
            ExprKind::Closure { params, ret_ty, body, is_own } => {
                self.lower_closure(params, ret_ty.as_deref(), body, *is_own)
            }

            // Cast
            ExprKind::Cast { expr, ty } => {
                // Trait object boxing: `value as any Trait`
                if let Some(trait_name) = ty.strip_prefix("any ") {
                    let (val, concrete_mir_ty) = self.lower_expr(expr)?;
                    return Ok(self.emit_trait_box(val, &concrete_mir_ty, trait_name));
                }

                let (val, _) = self.lower_expr(expr)?;
                let target_ty = self.ctx.resolve_type_str(ty);
                let result_local = self.builder.alloc_temp(target_ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Cast {
                        value: val,
                        target_ty: target_ty.clone(),
                    },
                }));
                Ok((MirOperand::Local(result_local), target_ty))
            }

            // Explicit lossy conversion (CV5–CV10).
            ExprKind::Convert { expr, target, kind } => {
                let (val, source_ty) = self.lower_expr(expr)?;
                let target_ty = self.ctx.resolve_type_str(target);
                let result_ty = if kind.is_optional() {
                    MirType::Option(Box::new(target_ty.clone()))
                } else {
                    target_ty.clone()
                };
                let result_local = self.builder.alloc_temp(result_ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Convert {
                        value: val,
                        source_ty,
                        target_ty,
                        kind: *kind,
                    },
                }));
                Ok((MirOperand::Local(result_local), result_ty))
            }

            // Using block — emit runtime init/shutdown for Multitasking/ThreadPool
            ExprKind::UsingBlock { name, args, body } => {
                if name == "Multitasking" || name == "MultiTasking" || name == "multitasking"
                    || name == "ThreadPool" || name == "threadpool"
                {
                    // Extract worker count from args, default to 0 (auto-detect)
                    let worker_count = if let Some(arg) = args.first() {
                        let (op, _ty) = self.lower_expr(&arg.expr)?;
                        op
                    } else {
                        MirOperand::Constant(crate::operand::MirConst::Int(0))
                    };
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal("rask_runtime_init".to_string()),
                        args: vec![worker_count],
                    }));
                    let result = self.lower_block(body);
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal("rask_runtime_shutdown".to_string()),
                        args: vec![],
                    }));
                    result
                } else {
                    self.lower_block(body)
                }
            }

            // With-as binding
            ExprKind::WithAs { bindings, body } => {
                // Detect Shared.read() / Shared.write() pattern:
                //   with shared.read() as d { body }
                // Synthesize a closure from the body and call Shared_read(handle, closure).
                if bindings.len() == 1 {
                    let binding = &bindings[0];
                    if let ExprKind::MethodCall { object, method, args: call_args, .. } = &binding.source.kind {
                        let is_shared_access = (method == "read" || method == "write") && call_args.is_empty();
                        if is_shared_access {
                            // Check if the object type is Shared
                            let obj_raw_type = self.ctx.lookup_raw_type(object.id);
                            let is_shared = obj_raw_type.map(|ty| {
                                matches!(ty,
                                    rask_types::Type::UnresolvedGeneric { name, .. }
                                    | rask_types::Type::UnresolvedNamed(name)
                                    if name == "Shared"
                                )
                            }).unwrap_or(false)
                            // Fallback: check local_meta type_prefix
                            || if let ExprKind::Ident(var_name) = &object.kind {
                                self.meta(var_name)
                                    .and_then(|m| m.type_prefix.as_deref())
                                    .map(|p| p == "Shared")
                                    .unwrap_or(false)
                            } else {
                                false
                            };
                            if is_shared {
                                return self.lower_shared_with_block(object, method, &binding.name, body);
                            }
                        }
                    }
                }

                // Detect Mutex pattern: with mutex.lock() as v { body }
                // Source is a method call `.lock()` on a Mutex expression.
                if bindings.len() == 1 {
                    let binding = &bindings[0];
                    if let ExprKind::MethodCall { object, method, args: call_args, .. } = &binding.source.kind {
                        let is_lock_call = method == "lock" && call_args.is_empty();
                        if is_lock_call {
                            let is_mutex = self.ctx.lookup_raw_type(object.id)
                                .map(|ty| matches!(ty,
                                    rask_types::Type::UnresolvedGeneric { name, .. }
                                    | rask_types::Type::UnresolvedNamed(name)
                                    if name == "Mutex"
                                ))
                                .unwrap_or(false)
                            || if let ExprKind::Ident(var_name) = &object.kind {
                                self.meta(var_name)
                                    .and_then(|m| m.type_prefix.as_deref())
                                    .map(|p| p == "Mutex")
                                    .unwrap_or(false)
                            } else {
                                false
                            };
                            if is_mutex {
                                return self.lower_mutex_with_block(object, &binding.name, body);
                            }
                        }
                    }
                }

                // Detect Mutex pattern: with mutex as v { body }
                // Source is a plain Ident referring to a Mutex variable.
                if bindings.len() == 1 {
                    let binding = &bindings[0];
                    let is_mutex = if let ExprKind::Ident(var_name) = &binding.source.kind {
                        let from_type = self.ctx.lookup_raw_type(binding.source.id)
                            .map(|ty| matches!(ty,
                                rask_types::Type::UnresolvedGeneric { name, .. }
                                | rask_types::Type::UnresolvedNamed(name)
                                if name == "Mutex"
                            ))
                            .unwrap_or(false);
                        let from_prefix = self.meta(var_name)
                            .and_then(|m| m.type_prefix.as_deref())
                            .map(|p| p == "Mutex")
                            .unwrap_or(false);
                        from_type || from_prefix
                    } else {
                        false
                    };
                    if is_mutex {
                        return self.lower_mutex_with_block(&binding.source, &binding.name, body);
                    }
                }

                // Default: simple alias binding (Pool, Cell, etc.)
                // W2a/W2b: Track pool bindings for re-resolution after pool mutators
                let mut pool_binding_keys: Vec<String> = Vec::new();
                // Vec[i] / Map[k] writeback info: (collection, index/key, item_local, setter_name).
                // Captured per binding so we can emit Vec_set / Map_set after the body runs.
                let mut coll_writebacks: Vec<(MirOperand, MirOperand, LocalId, &'static str)> = Vec::new();
                for binding in bindings {
                    // Does the binding read a pool slot? Decides aliasing below, and
                    // unlike `pool_info` it holds for `self.tasks[h]` as well as a
                    // plain `tasks[h]` — the pool's type is what matters, not whether
                    // it happens to be reachable by bare name.
                    let source_is_pool = match &binding.source.kind {
                        ExprKind::Index { object, .. } => self.index_object_is_pool(object),
                        _ => false,
                    };

                    // Before lowering, extract pool/handle info for re-resolution tracking
                    let pool_info = if let ExprKind::Index { object, index } = &binding.source.kind {
                        if let ExprKind::Ident(coll_name) = &object.kind {
                            if source_is_pool {
                                let pool_local = self.locals.get(coll_name).map(|(id, _)| *id);
                                let handle_local = if let ExprKind::Ident(h) = &index.kind {
                                    self.locals.get(h).map(|(id, _)| *id)
                                } else {
                                    None
                                };
                                pool_local.zip(handle_local).map(|(p, h)| (coll_name.clone(), p, h))
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    // Detect `with vec[i] as item` / `with map[k] as item` so we
                    // can write `item` back to the collection once the body
                    // finishes. Without this, mutations through `item` are lost.
                    // Reached through a field (`self.items[0]`) just as much as by
                    // bare name — the element is copied out either way.
                    let coll_writeback_info = if let ExprKind::Index { object, index } = &binding.source.kind {
                        let setter = match self.index_object_base(object).as_deref() {
                            Some("Vec") => Some("Vec_set"),
                            Some("Map") => Some("Map_set"),
                            _ => None,
                        };
                        if let Some(setter_name) = setter {
                            let (obj_op, _) = self.lower_expr(object)?;
                            let (idx_op, _) = self.lower_expr(index)?;
                            Some((obj_op, idx_op, setter_name))
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    let (val, val_ty) = self.lower_expr(&binding.source)?;
                    // A `with pool[h] as e` binding must alias the pool slot, not
                    // copy it: `pool[h]` yields a pointer into the arena, and a
                    // struct `Assign Use` would value-copy it, so writes through
                    // `e` would land in the copy and never reach the pool (#402).
                    // Reuse the access result local directly as the binding. Vec/
                    // Map bindings still copy + write back below (their element
                    // isn't a stable pointer).
                    let local = match (source_is_pool, &val) {
                        (true, MirOperand::Local(id)) => {
                            self.locals.insert(binding.name.clone(), (*id, val_ty.clone()));
                            *id
                        }
                        _ => {
                            let local = self.builder.alloc_local(binding.name.clone(), val_ty.clone());
                            self.locals.insert(binding.name.clone(), (local, val_ty.clone()));
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                                dst: local,
                                rvalue: MirRValue::Use(val),
                            }));
                            local
                        }
                    };

                    if let Some((obj_op, idx_op, setter_name)) = coll_writeback_info {
                        let _ = val_ty;
                        coll_writebacks.push((obj_op, idx_op, local, setter_name));
                    }

                    // Register pool binding for re-resolution
                    if let Some((pool_name, pool_local, handle_local)) = pool_info {
                        self.with_pool_bindings.entry(pool_name.clone())
                            .or_default()
                            .push((handle_local, local, pool_local));
                        pool_binding_keys.push(pool_name);
                    }
                }
                let result = self.lower_block(body);
                // Write back Vec[i] / Map[k] mutations through `with` bindings.
                for (obj_op, idx_op, item_local, setter_name) in coll_writebacks {
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal(setter_name.to_string()),
                        args: vec![obj_op, idx_op, MirOperand::Local(item_local)],
                    }));
                }
                // Clean up pool binding registrations
                for key in &pool_binding_keys {
                    if let Some(entries) = self.with_pool_bindings.get_mut(key) {
                        entries.pop();
                        if entries.is_empty() {
                            self.with_pool_bindings.remove(key);
                        }
                    }
                }
                result
            }

            // Spawn — synthesize a closure function and call rask_closure_spawn
            ExprKind::Spawn { body } => {
                self.lower_spawn(body)
            }

            // Block call (e.g., spawn_raw { ... })
            ExprKind::BlockCall { name, body } => {
                let (body_val, _) = self.lower_block(body)?;
                let ret_ty = self
                    .func_sigs
                    .get(name)
                    .map(|s| s.ret_ty.clone())
                    .unwrap_or(MirType::I64);
                let result_local = self.builder.alloc_temp(ret_ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: Some(result_local),
                    func: FunctionRef::internal(name.clone()),
                    args: vec![body_val],
                }));
                Ok((MirOperand::Local(result_local), ret_ty))
            }

            // Unsafe block
            ExprKind::Unsafe { body } => {
                self.lower_block(body)
            }

            // CF25: loop expression — allocate result slot for break-with-value
            ExprKind::Loop { body, label } => {
                let result_local = self.builder.alloc_local(
                    "__loop_result".to_string(),
                    MirType::I64,
                );
                let loop_block = self.builder.create_block();
                let exit_block = self.builder.create_block();

                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                    target: loop_block,
                }));
                self.builder.switch_to_block(loop_block);

                let ensure_depth = self.ensure_stack.len();
                self.loop_stack.push(LoopContext {
                    label: label.as_ref().map(|s| s.to_string()),
                    continue_block: loop_block,
                    exit_block,
                    result_local: Some(result_local),
                    ensure_depth,
                });

                for stmt in body {
                    self.lower_stmt(stmt)?;
                }
                // EN7: run loop-scoped ensures at iteration end
                self.emit_loop_cleanup(ensure_depth);
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                    target: loop_block,
                }));

                self.loop_stack.pop();
                self.ensure_stack.truncate(ensure_depth);
                self.builder.switch_to_block(exit_block);

                Ok((MirOperand::Local(result_local), MirType::I64))
            }

            // Comptime expression — try compile-time evaluation (CC1)
            ExprKind::Comptime { body } => {
                if let Some(ref interp_cell) = self.ctx.comptime_interp {
                    // Try evaluating the entire comptime block
                    let mut interp = interp_cell.borrow_mut();
                    if let Ok(val) = interp.eval_block_to_value(body) {
                        return Ok(match val {
                            rask_comptime::ComptimeValue::Bool(b) => {
                                (MirOperand::Constant(MirConst::Bool(b)), MirType::Bool)
                            }
                            rask_comptime::ComptimeValue::I64(n) => {
                                (MirOperand::Constant(MirConst::Int(n)), MirType::I64)
                            }
                            rask_comptime::ComptimeValue::String(s) => {
                                (MirOperand::Constant(MirConst::String(s)), MirType::String)
                            }
                            _ => {
                                // Complex value — fall through to normal lowering
                                drop(interp);
                                return self.lower_block(body);
                            }
                        });
                    }
                    drop(interp);
                }
                self.lower_block(body)
            }

            // Select (channel multiplexing)
            ExprKind::Select { arms, .. } => self.lower_select(arms),

            // Assert
            ExprKind::Assert { condition, message } => {
                // Detect comparison patterns for smart failure messages.
                // After desugaring, `a == b` → `a.eq(b)`, `a != b` → `!a.eq(b)`.
                let cmp_info = if message.is_none() {
                    extract_assert_comparison(condition)
                } else {
                    None
                };

                if let Some((left_expr, right_expr, op_str, is_string)) = cmp_info {
                    // Lower both sides first to capture their values + types
                    let (left_op, left_ty) = self.lower_expr(left_expr)?;
                    let (right_op, right_ty) = self.lower_expr(right_expr)?;

                    // Then compare the values we already have. Lowering the whole
                    // condition here instead would run both sides a second time,
                    // so `assert push(v) == 1` pushed twice.
                    let (cond_op, _) = if expr_may_do_work(left_expr) || expr_may_do_work(right_expr) {
                        let left_ref = self.bind_assert_operand(left_expr, &left_op, &left_ty);
                        let right_ref = self.bind_assert_operand(right_expr, &right_op, &right_ty);
                        let rebuilt = rebuild_assert_condition(condition, left_ref, right_ref);
                        self.lower_expr(&rebuilt)?
                    } else {
                        self.lower_expr(condition)?
                    };
                    let ok_block = self.builder.create_block();
                    let fail_block = self.builder.create_block();

                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                        cond: cond_op,
                        then_block: ok_block,
                        else_block: fail_block,
                    }));

                    self.builder.switch_to_block(fail_block);
                    let op_const = MirOperand::Constant(MirConst::String(op_str.to_string()));
                    // Pick the right fail helper for the operand types so the
                    // Cranelift call signature matches: f64 args go to a f64
                    // helper, strings to the str helper, everything else i64.
                    let is_float = matches!(left_ty, MirType::F32 | MirType::F64)
                        || matches!(right_ty, MirType::F32 | MirType::F64);
                    let is_char = matches!(left_ty, MirType::Char)
                        && matches!(right_ty, MirType::Char);
                    let fail_fn = if is_string {
                        "assert_fail_cmp_str"
                    } else if is_float {
                        "assert_fail_cmp_f64"
                    } else if is_char {
                        "assert_fail_cmp_char"
                    } else {
                        "assert_fail_cmp_i64"
                    };
                    // Each fail helper has one fixed signature, so a narrower
                    // operand has to be widened to it. An f32 or a char reached
                    // the f64/i64 helper at its own width and Cranelift
                    // rejected the call outright (#332).
                    let (left_op, right_op) = if is_string {
                        (left_op, right_op)
                    } else {
                        let want = if is_float { MirType::F64 } else { MirType::I64 };
                        (
                            self.widen_for_assert_helper(left_op, &left_ty, &want),
                            self.widen_for_assert_helper(right_op, &right_ty, &want),
                        )
                    };
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal(fail_fn.to_string()),
                        args: vec![left_op, right_op, op_const],
                    }));
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Unreachable));

                    self.builder.switch_to_block(ok_block);
                    Ok((MirOperand::Constant(MirConst::Bool(true)), MirType::Bool))
                } else {
                    // Check for `is` pattern: assert x is Some
                    let is_msg = if message.is_none() {
                        extract_assert_is_pattern(condition)
                            .map(|pat| format!("assertion failed: expected {}", pat))
                    } else {
                        None
                    };

                    // Generic path: lower condition, pass optional message
                    let (cond_op, _) = self.lower_expr(condition)?;
                    let ok_block = self.builder.create_block();
                    let fail_block = self.builder.create_block();

                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                        cond: cond_op,
                        then_block: ok_block,
                        else_block: fail_block,
                    }));

                    self.builder.switch_to_block(fail_block);
                    let mut args = Vec::new();
                    if let Some(msg) = message {
                        let (msg_op, _) = self.lower_expr(msg)?;
                        args.push(msg_op);
                    } else if let Some(is_msg) = is_msg {
                        args.push(MirOperand::Constant(MirConst::String(is_msg)));
                    }
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal("assert_fail".to_string()),
                        args,
                    }));
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Unreachable));

                    self.builder.switch_to_block(ok_block);
                    Ok((MirOperand::Constant(MirConst::Bool(true)), MirType::Bool))
                }
            }

            // Check (like assert but continues)
            ExprKind::Check { condition, message } => {
                let (cond_op, _) = self.lower_expr(condition)?;
                let ok_block = self.builder.create_block();
                let fail_block = self.builder.create_block();
                let merge_block = self.builder.create_block();
                let result_local = self.builder.alloc_temp(MirType::Bool);

                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: cond_op,
                    then_block: ok_block,
                    else_block: fail_block,
                }));

                self.builder.switch_to_block(fail_block);
                let mut args = Vec::new();
                if let Some(msg) = message {
                    let (msg_op, _) = self.lower_expr(msg)?;
                    args.push(msg_op);
                }
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: None,
                    func: FunctionRef::internal("check_fail".to_string()),
                    args,
                }));
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Bool(false))),
                }));
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

                self.builder.switch_to_block(ok_block);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Bool(true))),
                }));
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

                self.builder.switch_to_block(merge_block);
                Ok((MirOperand::Local(result_local), MirType::Bool))
            }
        }
    }

    // =================================================================
    // Control flow lowering
    // =================================================================

    /// If expression lowering (spec L1).
    ///
    /// ```text
    /// [current]  cond → branch then_block / else_block
    /// [then]     result = then_val; goto merge
    /// [else]     result = else_val; goto merge
    /// [merge]    continue with result
    /// ```
    /// Lower `object.method(args)`. An ordered chain of dispatch attempts;
    /// the order is the dispatch precedence and is load-bearing.
    fn lower_method_call(
        &mut self,
        expr: &Expr,
        object: &Expr,
        method: &str,
        args: &[CallArg],
        type_args: &Option<Vec<String>>,
    ) -> Result<TypedOperand, LoweringError> {
        let method = method.to_string();
        let method = &method;
        if let Some(r) = self.try_lower_c_namespace_call(expr, object, method, args)? {
            return Ok(r);
        }

        // Iterator terminal methods: .collect(), .fold(), .any(), .all(), etc.
        // Try to recognize an iterator chain on the receiver and fuse it inline.
        if let Some(result) = self.try_lower_iter_terminal(expr, object, method, args)? {
            return Ok(result);
        }

        if let Some(r) = self.try_lower_origin(object, method, args)? {
            return Ok(r);
        }

        if let Some(r) = self.try_lower_discriminant(object, method, args)? {
            return Ok(r);
        }

        if let Some(r) = self.try_lower_module_type_method(object, method, args)? {
            return Ok(r);
        }

        if let Some(r) = self.try_lower_type_name_call(expr, object, method, args, type_args)? {
            return Ok(r);
        }

        // A bare `box.lock()` / `.read()` / `.write()` whose value is used
        // directly, with nothing chained onto it. `sync_guard` only fires when
        // the guard is the *object* of a trailing field or method access, so
        // this form fell through to plain dispatch and mangled `Mutex_lock` —
        // the closure-taking runtime entry point — with no closure to give it,
        // which failed the Cranelift verifier on argument count (#479).
        //
        // Same acquire / use / release shape as the chained form, with the
        // guard itself as the value.
        if let Some((box_obj, acquire, release)) = self.sync_guard(expr) {
            let ret_hint = self.ctx.lookup_raw_type(expr.id).map(|t| self.ctx.type_to_mir(t));
            return self.lower_sync_guard_access(box_obj, acquire, release, ret_hint, |g| g);
        }

        // `box.lock()/.read()/.write().method(args)` — the guard result is a
        // scoped lock, so run the trailing call between acquire and release
        // (lock → call → unlock) instead of lowering the guard to a bare 1-arg
        // call the closure-based runtime can't service directly.
        if let Some((box_obj, acquire, release)) = self.sync_guard(object) {
            let method = method.to_string();
            let args = args.to_vec();
            let ret_hint = self.ctx.lookup_raw_type(expr.id).map(|t| self.ctx.type_to_mir(t));
            return self.lower_sync_guard_access(box_obj, acquire, release, ret_hint, move |g| Expr {
                id: rask_ast::NodeId::DUMMY,
                span: rask_ast::Span::new(0, 0),
                kind: ExprKind::MethodCall {
                    object: Box::new(g),
                    method,
                    type_args: None,
                    args,
                },
            });
        }

        let (obj_op, obj_ty) = self.lower_expr(object)?;

        if let Some(r) = self.try_lower_raw_ptr_method(object, method, args, &obj_op, &obj_ty)? {
            return Ok(r);
        }

        if let Some(r) = self.try_lower_option_none_cmp(object, method, args, &obj_op)? {
            return Ok(r);
        }

        if let Some(r) = self.try_lower_operator_method(object, method, args, &obj_op, &obj_ty)? {
            return Ok(r);
        }

        if let Some(r) = self.try_lower_primitive_compare(method, args, &obj_op, &obj_ty)? {
            return Ok(r);
        }

        if let Some(r) = self.try_lower_string_compare(object, method, args, &obj_op, &obj_ty)? {
            return Ok(r);
        }

        if let Some(r) = self.try_lower_string_concat(method, args, &obj_op, &obj_ty)? {
            return Ok(r);
        }

        if let Some(r) = self.try_lower_fmt(expr, object, method, args, &obj_op, &obj_ty)? {
            return Ok(r);
        }

        if let Some(r) = self.try_lower_to_string(object, method, args, &obj_op, &obj_ty)? {
            return Ok(r);
        }

        if let Some(r) = self.try_lower_map_err(method, args, &obj_op, &obj_ty)? {
            return Ok(r);
        }

        if let Some(r) = self.try_lower_ok_to_option(expr, object, method, args, &obj_op, &obj_ty)? {
            return Ok(r);
        }

        if let Some(r) = self.try_lower_unwrap(object, method, args, &obj_op, &obj_ty)? {
            return Ok(r);
        }

        // .clone(): dispatch to type-specific clone (Vec_clone, string_clone, etc.)
        // Value types (integers, bools) fall through to generic rask_clone.
        // Heap types (Vec, Map, string) need deep copy via their runtime functions.
        if let Some(r) = self.try_lower_array_len(method, args, &obj_ty)? {
            return Ok(r);
        }

        if let Some(r) = self.try_lower_trait_object(expr, method, args, &obj_op, &obj_ty)? {
            return Ok(r);
        }

        self.lower_regular_method_call(expr, object, method, args, type_args, obj_op, obj_ty)
    }

    /// Calls where the receiver is a type or module name, not a value:
    /// `Vec.new()`, `pkg.func()`, `Shape.Circle(r)`, `json.encode(x)`, enum
    /// variants, etc. Returns None when the receiver is a local variable or
    /// the form isn't a recognized type-name call.
    fn try_lower_type_name_call(
        &mut self,
        expr: &Expr,
        object: &Expr,
        method: &String,
        args: &[CallArg],
        type_args: &Option<Vec<String>>,
    ) -> Result<Option<TypedOperand>, LoweringError> {
        if let ExprKind::Ident(name) = &object.kind {
            if !self.locals.contains_key(name) {
                        // Cross-package call: pkg.func() → direct call to func
                        // Skip builtin stdlib modules — they use prefixed names
                        // (e.g. net.tcp_listen → net_tcp_listen) handled by
                        // the is_known_type path below.
                        if self.ctx.package_modules.contains(name)
                            && !super::is_type_constructor_name(name)
                        {
                            let func_name = method.clone();
                            let mut arg_operands = Vec::new();
                            for arg in args {
                                let (op, _) = self.lower_expr(&arg.expr)?;
                                arg_operands.push(op);
                            }
                            let ret_ty = self
                                .func_sigs
                                .get(&func_name)
                                .map(|s| s.ret_ty.clone())
                                .unwrap_or_else(|| super::stdlib_return_mir_type(&func_name));
                            let result_local = self.builder.alloc_temp(ret_ty.clone());
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                                dst: Some(result_local),
                                func: FunctionRef::internal(func_name),
                                args: arg_operands,
                            }));
                            return Ok(Some((MirOperand::Local(result_local), ret_ty)));
                        }

                        // Comptime global: TABLE.get(0) → GlobalRef + Vec_get
                        if let Some(meta) = self.ctx.comptime_globals.get(name) {
                            let type_prefix = meta.type_prefix.clone();
                            let elem_count = meta.elem_count;

                            // Load the comptime global data pointer
                            let global_local = self.builder.alloc_temp(MirType::Ptr);
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::GlobalRef {
                                dst: global_local,
                                name: name.clone(),
                            }));

                            // Wrap raw data into a Vec: rask_vec_from_static(ptr, count, elem_size)
                            let vec_local = self.builder.alloc_temp(MirType::I64);
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                                dst: Some(vec_local),
                                func: FunctionRef::internal("rask_vec_from_static".to_string()),
                                args: vec![
                                    MirOperand::Local(global_local),
                                    MirOperand::Constant(MirConst::Int(elem_count as i64)),
                                    // Comptime array globals hold i64 elements.
                                    MirOperand::Constant(MirConst::Int(8)),
                                ],
                            }));

                            // Dispatch method using the type prefix
                            let func_name = format!("{}_{}", type_prefix, method);
                            let mut arg_operands = vec![MirOperand::Local(vec_local)];
                            for arg in args {
                                let (op, _) = self.lower_expr(&arg.expr)?;
                                arg_operands.push(op);
                            }
                            let ret_ty = self
                                .func_sigs
                                .get(&func_name)
                                .map(|s| s.ret_ty.clone())
                                .unwrap_or_else(|| super::stdlib_return_mir_type(&func_name));
                            let result_local = self.builder.alloc_temp(ret_ty.clone());
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                                dst: Some(result_local),
                                func: FunctionRef::internal(func_name),
                                args: arg_operands,
                            }));
                            return Ok(Some((MirOperand::Local(result_local), ret_ty)));
                        }

                        // Enum variant constructor: Shape.Circle(r)
                        // Extract layout data before mutable borrows in lower_expr
                        let enum_variant = self.ctx.find_enum(name).and_then(|(idx, layout)| {
                            let variant = layout.variants.iter().find(|v| v.name == *method)?;
                            Some((
                                idx,
                                layout.size,
                                layout.align,
                                layout.tag_offset,
                                variant.tag,
                                variant.payload_offset,
                                variant.fields.clone(),
                            ))
                        });

                        if let Some((idx, enum_size, enum_align, tag_offset, tag_val, payload_offset, fields)) =
                            enum_variant
                        {
                            let enum_ty = MirType::Enum(EnumLayoutId::new(idx, enum_size, enum_align));
                            let result_local = self.builder.alloc_temp(enum_ty.clone());

                            // Store discriminant tag
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                                addr: result_local,
                                offset: tag_offset,
                                value: MirOperand::Constant(MirConst::Int(tag_val as i64)),
                                store_size: None,
                            }));

                            // Store payload fields
                            for (i, arg) in args.iter().enumerate() {
                                let (val, _) = self.lower_expr(&arg.expr)?;
                                let (offset, field_size) = if i < fields.len() {
                                    (payload_offset + fields[i].offset, fields[i].size)
                                } else {
                                    (payload_offset + (i as u32 * 8), 8)
                                };
                                // Aggregate payloads (string = 16 bytes, embedded
                                // structs) must copy the full value. Without store_size
                                // a string constant stores only its 8-byte data pointer
                                // and the length word is left uninitialized (#387).
                                let store_size = if field_size > 8 { Some(field_size) } else { None };
                                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                                    addr: result_local,
                                    offset,
                                    value: val,
                                    store_size,
                                }));
                            }

                            return Ok(Some((MirOperand::Local(result_local), enum_ty)));
                        }

                        // .variants() on enum types: build a Vec of tag values
                        if method == "variants" && args.is_empty() {
                            if let Some((_idx, layout)) = self.ctx.find_enum(name) {
                                // Create a new Vec
                                let vec_local = self.builder.alloc_temp(MirType::I64);
                                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                                    dst: Some(vec_local),
                                    func: FunctionRef::internal("Vec_new".to_string()),
                                    args: vec![],
                                }));
                                // Push each variant's tag value
                                for variant in &layout.variants {
                                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                                        dst: None,
                                        func: FunctionRef::internal("Vec_push".to_string()),
                                        args: vec![
                                            MirOperand::Local(vec_local),
                                            MirOperand::Constant(MirConst::Int(variant.tag as i64)),
                                        ],
                                    }));
                                }
                                return Ok(Some((MirOperand::Local(vec_local), MirType::I64)));
                            }
                        }

                        // json.encode — expand struct/vec/primitive serialization at MIR level
                        if name == "json" && method == "encode" && args.len() == 1 {
                            let (arg_op, arg_ty) = self.lower_expr(&args[0].expr)?;
                            if let MirType::Struct(StructLayoutId { id, .. }) = &arg_ty {
                                if let Some(layout) = self.ctx.struct_layouts.get(*id as usize) {
                                    return self.lower_json_encode_struct(arg_op, layout.clone()).map(Some);
                                }
                            }

                            // Vec<T>: generate loop that encodes each element.
                            // Detection: check type checker first, fall back to local_meta type_prefix.
                            let raw_ty = self.ctx.lookup_raw_type(args[0].expr.id);
                            // A resolved `Type::Generic { base }` counts too: a
                            // call returning `Vec<T>` comes back that way, and
                            // missing it sent `json.encode(views())` through
                            // json_encode_i64 — the binding held a pointer.
                            let is_vec_from_type = raw_ty.map_or(false, |ty| {
                                matches!(ty,
                                    rask_types::Type::UnresolvedGeneric { name, .. } if name == "Vec"
                                ) || matches!(ty, rask_types::Type::UnresolvedNamed(n) if n == "Vec")
                                    || self.vec_elem_of_checker_type(ty).is_some()
                            });
                            let is_vec_from_prefix = if !is_vec_from_type {
                                if let ExprKind::Ident(var_name) = &args[0].expr.kind {
                                    self.meta(var_name)
                                        .and_then(|m| m.type_prefix.as_deref())
                                        .map(|p| p == "Vec")
                                        .unwrap_or(false)
                                } else {
                                    false
                                }
                            } else {
                                false
                            };
                            // Last resort: the callee's declared return type. A
                            // call returning `Vec<T>` doesn't always carry a type
                            // on the argument node, and missing it sent
                            // `json.encode(views())` through json_encode_i64.
                            let elem_from_sig = if is_vec_from_type || is_vec_from_prefix {
                                None
                            } else {
                                self.vec_elem_of_expr(&args[0].expr)
                            };
                            if is_vec_from_type || is_vec_from_prefix || elem_from_sig.is_some() {
                                // Extract element type from generic args when available
                                let elem_ty = raw_ty.and_then(|ty| match ty {
                                    rask_types::Type::UnresolvedGeneric { args: ga, .. }
                                    | rask_types::Type::Generic { args: ga, .. } => {
                                        ga.first().and_then(|a| match a {
                                            rask_types::GenericArg::Type(t) => Some(t.as_ref().clone()),
                                            _ => None,
                                        })
                                    }
                                    _ => None,
                                });
                                // The checker leaves a `Vec.new()` filled by
                                // `push` with an inference variable, so fall back
                                // to the element type lowering tracked itself.
                                let elem_mir = elem_ty
                                    .as_ref()
                                    .map(|t| self.ctx.type_to_mir(t))
                                    .or(elem_from_sig)
                                    .or_else(|| self.vec_elem_of_expr(&args[0].expr));
                                return self.lower_json_encode_vec(arg_op, elem_ty, elem_mir).map(Some);
                            }

                            // Non-struct: string or integer
                            let helper = if matches!(arg_ty, MirType::String) {
                                "json_encode_string"
                            } else {
                                "json_encode_i64"
                            };
                            let result_local = self.builder.alloc_temp(MirType::I64);
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                                dst: Some(result_local),
                                func: FunctionRef::internal(helper.to_string()),
                                args: vec![arg_op],
                            }));
                            return Ok(Some((MirOperand::Local(result_local), MirType::I64)));
                        }

                        // json.decode<T> — describe T to the runtime, decode into it
                        if name == "json" && method == "decode" && args.len() == 1 {
                            let (str_op, _) = self.lower_expr(&args[0].expr)?;
                            let target = self.json_decode_target(expr, type_args)?;
                            return self.lower_json_decode(str_op, &target).map(Some);
                        }

                        // Vec.from([...]) → stack array + rask_vec_from_static(ptr, count)
                        // Map.from([("k", "v"), ...]) → Map.new() + Map.insert() per pair
                        {
                            let base = name.split('<').next().unwrap_or(name);
                            if base == "Vec" && method == "from" && args.len() == 1 {
                                if let ExprKind::Array(elems) = &args[0].expr.kind {
                                    return self.lower_vec_from_array(elems).map(Some);
                                }
                            }
                            if base == "Map" && method == "from" && args.len() == 1 {
                                if let ExprKind::Array(elems) = &args[0].expr.kind {
                                    return self.lower_map_from_pairs(elems).map(Some);
                                }
                            }
                        }

                        // Static method on a type: Vec.new(), string.new().
                        // A name that holds a value isn't a type, whatever its
                        // capitalisation — `is_type_constructor_name` says yes to
                        // anything starting with a capital, so a SCREAMING_CASE
                        // module const was read as a type and `CT.to_string()`
                        // compiled to a call to a function named `CT_to_string`
                        // that doesn't exist (#403).
                        let is_known_type = !self.name_holds_a_value(name)
                            && (self.ctx.find_struct(name).is_some()
                                || self.ctx.find_enum(name).is_some()
                                || is_type_constructor_name(name));

                        // CH3: char.from_u32(n) → char?. Reuse the Convert→Option
                        // codegen path (same as `try convert`) with a Char target.
                        if name == "char" && method == "from_u32" {
                            let (n, _) = self.lower_expr(&args[0].expr)?;
                            let result_ty = MirType::Option(Box::new(MirType::Char));
                            let result_local = self.builder.alloc_temp(result_ty.clone());
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                                dst: result_local,
                                rvalue: MirRValue::Convert {
                                    value: n,
                                    source_ty: MirType::U32,
                                    target_ty: MirType::Char,
                                    kind: rask_ast::expr::ConvertKind::TryConvert,
                                },
                            }));
                            return Ok(Some((MirOperand::Local(result_local), result_ty)));
                        }

                        if is_known_type {
                            // Strip generic parameters: "Channel<i64>" → "Channel"
                            let base_name = name.split('<').next().unwrap_or(name);
                            let func_name = format!("{}_{}", base_name, method);
                            let callee_params: Vec<Option<String>> = self
                                .func_sigs
                                .get(&func_name)
                                .map(|s| s.param_ty_strs.clone())
                                .unwrap_or_default();
                            let mut arg_operands = Vec::new();
                            for (i, arg) in args.iter().enumerate() {
                                // An unannotated closure parameter takes its type
                                // from the callee's declared `func(...)` parameter;
                                // `|req| req.method` on a `func(Request) -> Response`
                                // otherwise defaulted to i64 (#463).
                                let (op, _) = if let ExprKind::Closure { params, ret_ty, body, is_own } = &arg.expr.kind {
                                    let expected = Self::expected_closure_param_tys(&callee_params, i);
                                    self.lower_closure_expecting(
                                        params, ret_ty.as_deref(), body, *is_own, &expected,
                                    )?
                                } else {
                                    self.lower_expr(&arg.expr)?
                                };
                                arg_operands.push(op);
                            }

                            // Inject elem_size/data_size for generic constructors.
                            // The C runtime needs actual sizes for struct types;
                            // the dispatch table expects these as extra arguments.
                            // The element type is read from the call's resolved
                            // result type (`Channel<T>` returns a Sender/Receiver
                            // tuple; generic_arg_slot_size unwraps that).
                            if (base_name == "Channel" && (method == "buffered" || method == "unbuffered"))
                                || ((base_name == "Shared" || base_name == "Mutex") && method == "new")
                            {
                                let elem_size = self.generic_arg_slot_size(expr.id, 0);
                                let size_op = MirOperand::Constant(MirConst::Int(elem_size));
                                if base_name == "Channel" {
                                    // Channel: elem_size goes first → (elem_size, capacity)
                                    arg_operands.insert(0, size_op);
                                } else {
                                    // Shared: data_size goes last → (data_ptr, data_size)
                                    arg_operands.push(size_op);
                                }
                            }
                            // Pool.new() / Pool.with_capacity(n): inject elem_size
                            // so the pool allocates correctly-sized slots for struct
                            // elements. with_capacity keeps its `n` after elem_size.
                            if base_name == "Pool" && (method == "new" || method == "with_capacity") {
                                let elem_size = self.generic_arg_slot_size(expr.id, 0);
                                let size_op = MirOperand::Constant(MirConst::Int(elem_size));
                                arg_operands.insert(0, size_op);
                            }

                            // Vec.new() / Vec.with_capacity(n): inject elem_size so
                            // the runtime allocates correct slots.
                            if base_name == "Vec" && (method == "new" || method == "with_capacity") {
                                let elem_size = self.generic_arg_slot_size(expr.id, 0);
                                let size_op = MirOperand::Constant(MirConst::Int(elem_size));
                                arg_operands.insert(0, size_op);
                            }
                            // Map.new(): inject key_size, val_size
                            if (base_name == "Map") && method == "new" {
                                let key_size = self.generic_arg_slot_size(expr.id, 0);
                                let val_size = self.generic_arg_slot_size(expr.id, 1);
                                arg_operands.insert(0, MirOperand::Constant(MirConst::Int(key_size)));
                                arg_operands.insert(1, MirOperand::Constant(MirConst::Int(val_size)));
                            }

                            // Map.new() with string keys → use string hash/eq.
                            // Inspect the first generic arg of the Map type for
                            // any string-flavored shape (resolved or unresolved),
                            // OR fall back to the syntactic type name when the
                            // user wrote `Map<string, _>.new()` explicitly.
                            let func_name = if func_name == "Map_new" {
                                fn arg_is_string(arg: &rask_types::GenericArg) -> bool {
                                    if let rask_types::GenericArg::Type(t) = arg {
                                        match t.as_ref() {
                                            rask_types::Type::String => true,
                                            rask_types::Type::UnresolvedNamed(n) => n == "string",
                                            _ => false,
                                        }
                                    } else {
                                        false
                                    }
                                }
                                let has_string_keys = self.ctx.lookup_raw_type(expr.id)
                                    .map(|ty| match ty {
                                        rask_types::Type::Generic { args, .. }
                                        | rask_types::Type::UnresolvedGeneric { args, .. } => {
                                            args.first().map_or(false, arg_is_string)
                                        }
                                        _ => false,
                                    })
                                    .unwrap_or(false)
                                    || (name.starts_with("Map<") && name.contains("string"));
                                if has_string_keys {
                                    "Map_new_string_keys".to_string()
                                } else {
                                    func_name
                                }
                            } else {
                                func_name
                            };

                            let ret_ty = self
                                .func_sigs
                                .get(&func_name)
                                .map(|s| s.ret_ty.clone())
                                .unwrap_or_else(|| super::stdlib_return_mir_type(&func_name));
                            // Channel.buffered()/unbuffered() C runtime returns a
                            // single i64 (raw channel pair pointer), not a tuple.
                            // Override the Tuple return type from stubs to I64 so the
                            // codegen allocates a register, not a stack slot. The
                            // tuple destructure emits channel_tx/channel_rx calls.
                            let ret_ty = if base_name == "Channel"
                                && (method == "buffered" || method == "unbuffered")
                            {
                                MirType::I64
                            } else {
                                ret_ty
                            };
                            let result_local = self.builder.alloc_temp(ret_ty.clone());
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                                dst: Some(result_local),
                                func: FunctionRef::internal(func_name),
                                args: arg_operands,
                            }));

                            return Ok(Some((MirOperand::Local(result_local), ret_ty)));
                        }
            }
        }
        Ok(None)
    }

    /// Ordinary method dispatch: mangle the method to `{Type}_{method}` from
    /// the resolved receiver type, then emit the call. Also carries the inline
    /// Result/Option handling, struct/enum clone, and collection element
    /// tracking the plain call path needs.
    fn lower_regular_method_call(
        &mut self,
        expr: &Expr,
        object: &Expr,
        method: &String,
        args: &[CallArg],
        type_args: &Option<Vec<String>>,
        obj_op: MirOperand,
        obj_ty: MirType,
    ) -> Result<TypedOperand, LoweringError> {
        // Generic method: append type arg to name (e.g. parse<i32> → parse_i32)
        let method = if let Some(ta) = type_args {
            if let Some(ty_name) = ta.first() {
                format!("{}_{}", method, ty_name)
            } else {
                method.clone()
            }
        } else {
            method.clone()
        };

        // `const x: f64 = "3.5".parse()` infers the target from context, so
        // there's no type argument to mangle and the name stays plain `parse`
        // — which dispatches to the integer runtime and fails on "3.5" (#480).
        // Recover the target from the checker's type for the call.
        let method = match (method.as_str(), self.ctx.lookup_node_type(expr.id)) {
            ("parse", Some(MirType::Result { ok, .. })) => match *ok {
                MirType::F32 => "parse_f32".to_string(),
                MirType::F64 => "parse_f64".to_string(),
                _ => method,
            },
            _ => method,
        };

        // Regular method call. Clone the receiver operand so obj_op stays
        // available for the inline Result/Option dispatch below (which
        // must reuse it rather than re-lower `object` — see #349).
        let mut all_args = vec![obj_op.clone()];
        let mut arg_types = Vec::new();
        // The qualified name isn't resolved until after the arguments are
        // lowered, but a closure argument needs its parameter types up front.
        // A module-style receiver (`http.listen_and_serve(…)`) mangles
        // predictably, so try that key for the callee's signature.
        let tentative_params: Vec<Option<String>> = match &object.kind {
            ExprKind::Ident(recv) => self
                .func_sigs
                .get(&format!("{}_{}", recv, method))
                .map(|s| s.param_ty_strs.clone())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let elem_params = self.elem_closure_param_tys(object, &method);
        for (i, arg) in args.iter().enumerate() {
            let (op, ty) = if let ExprKind::Closure { params, ret_ty, body, is_own } = &arg.expr.kind {
                let mut expected = Self::expected_closure_param_tys(&tentative_params, i);
                if expected.is_empty() {
                    expected = elem_params.clone();
                }
                self.lower_closure_expecting(params, ret_ty.as_deref(), body, *is_own, &expected)?
            } else {
                self.lower_expr(&arg.expr)?
            };
            all_args.push(op);
            arg_types.push(ty);
        }

        // Qualify method name with receiver type to avoid dispatch
        // ambiguity (e.g. Vec.get vs Map.get vs Pool.get).
        // Priority: user-defined struct/enum from type checker first
        // (`extend E { func get(self) }` would otherwise be shadowed by
        // the hardcoded Map.get fallback below). Skip stdlib types
        // (Option, Result, ...) so their methods stay on the existing
        // dispatch path.
        let user_type_prefix = self.ctx.lookup_raw_type(object.id)
            .filter(|ty| super::MirContext::stdlib_type_prefix(ty).is_none())
            .and_then(|ty| super::MirContext::type_prefix(ty, self.ctx.type_names))
            .filter(|prefix| {
                let base = prefix.split('<').next().unwrap_or(prefix);
                self.ctx.find_struct(base).is_some()
                    || self.ctx.find_enum(base).is_some()
                    // A nominal newtype has no layout of its own, but its
                    // `extend` methods are registered under its own name. Left
                    // out, `Label("hey").shout()` on a `type Label = string`
                    // mangled to `string_shout`, which doesn't exist (#445).
                    || (self.is_transparent_newtype(base)
                        && self.func_sigs.contains_key(&format!("{}_{}", base, method)))
            });

        // CALL6: what dispatch actually resolved to, when MIR can confirm the
        // type exists here. The confirmation matters — the record is written
        // before monomorphization, so a receiver typed as a bare type parameter
        // (`T`) would otherwise mangle to `T_method`. Anything unconfirmed falls
        // through to the guessing chain below, so this can only add precision.
        let recorded_prefix = self.ctx.recorded_prefix(expr.id).filter(|prefix| {
            // The one thing the record can't be trusted for: it's written
            // before monomorphization, so a receiver still typed as a bare
            // type parameter would mangle to `T_method`. Single uppercase
            // letters are type parameters by rule (type.gradual/PC3), which
            // is exactly the case to drop.
            //
            // Nothing else needs confirming. Requiring the type to also
            // declare the method in a stub sounded safer and wasn't — it
            // rejected `string.push_str`, which codegen has and no stub
            // declares, sending a call the checker had already resolved back
            // to the guessing chain.
            let base = prefix.split('<').next().unwrap_or(prefix).trim();
            let mut chars = base.chars();
            !matches!((chars.next(), chars.next()),
                      (Some(c), None) if c.is_ascii_uppercase())
        });
        // Inline Result/Option methods that have no runtime impl —
        // `.map(f)`, `.ok()`, `.filter(f)`. These were dispatching
        // to Vec_map et al. as a fallback and silently
        // mis-computing on aggregate or non-i64 values.
        if let Some(handled) = self.try_lower_result_option_method(
            expr, object, method.as_str(), args, &obj_op, &obj_ty,
        )? {
            return Ok(handled);
        }

        // Resolve the receiver's stdlib type prefix, then mangle to
        // `{Type}_{method}`. Dispatch is driven by the resolved receiver
        // type and the stub-derived metadata — not a hand-maintained
        // method-name table. Priority:
        //   0. the receiver dispatch resolved to (CALL6), when MIR confirms it
        //   1. user struct/enum from the type checker
        //   2. tracked local/field type (LocalMeta, struct layout)
        //   3. resolved receiver type, when that stdlib type actually
        //      declares the method (validated against the stub API)
        //   4. the method's sole defining stdlib type, when unambiguous
        //   5. disambiguation policy for shared/absent names on
        //      receivers the checker left unresolved
        //   6. resolved type / MIR type as a last resort
        let type_prefix_of_receiver = || {
            self.ctx.lookup_raw_type(object.id)
                .and_then(|ty| super::MirContext::type_prefix(ty, self.ctx.type_names))
        };
        let qualified_name = recorded_prefix
            .or(user_type_prefix)
            .or_else(|| if let ExprKind::Ident(var_name) = &object.kind {
                self.meta(var_name).and_then(|m| m.type_prefix.clone())
            } else {
                None
            })
            // Field access on struct: resolve field type from struct layout
            .or_else(|| {
                if let ExprKind::Field { object: inner_obj, field: field_name } = &object.kind {
                    if let ExprKind::Ident(var_name) = &inner_obj.kind {
                        if let Some((local_id, _)) = self.locals.get(var_name) {
                            let local_ty = self.builder.local_type(*local_id);
                            if let Some(MirType::Struct(StructLayoutId { id, .. })) = local_ty {
                                if let Some(layout) = self.ctx.struct_layouts.get(id as usize) {
                                    if let Some(fl) = layout.fields.iter().find(|f| f.name == *field_name) {
                                        return super::MirContext::type_prefix(&fl.ty, self.ctx.type_names);
                                    }
                                }
                            }
                        }
                    }
                    None
                } else {
                    None
                }
            })
            // Resolved receiver type is authoritative when that stdlib
            // type declares the method (checked against the stub API).
            .or_else(|| type_prefix_of_receiver().filter(|p| {
                let base = p.split('<').next().unwrap_or(p).trim();
                rask_stdlib::mir_metadata::type_has_method(base, &method)
            }))
            // Unambiguous stub method → its sole defining type.
            .or_else(|| rask_stdlib::mir_metadata::unique_method_prefix(&method)
                .map(|s| s.to_string()))
            // Disambiguation policy for method names shared across (or
            // absent from) stub types, used only when the receiver type
            // is unresolved. A resolved receiver above always wins.
            .or_else(|| ambiguous_method_prefix(&method, all_args.len())
                .map(|s| s.to_string()))
            // Last resort: resolved type even if the stub doesn't list
            // the method (user types, monomorphized aggregates), then
            // the MIR type (catches F64, String, etc.).
            .or_else(type_prefix_of_receiver)
            .or_else(|| super::mir_type_method_prefix(&obj_ty).map(|s| s.to_string()))
            // A Struct/Enum MIR type carries a layout whose name is the type —
            // resolve it directly. Catches receivers the checker left untyped
            // but MIR typed concretely: a pool-element `with` binding, a Handle
            // deref, a `self`-typed receiver.
            .or_else(|| self.mir_aggregate_prefix(&obj_ty))
            // parse<T> always belongs to string (structural, not type-prefix related)
            .or_else(|| if method.starts_with("parse_") { Some("string".to_string()) } else { None })
            .map(|prefix| {
                // Strip generic params from the prefix before mangling:
                // "Vec<T>" → "Vec", "Map<K, V>" → "Map". Otherwise the
                // call name is `Vec<T>_len` which has no codegen entry.
                let base = prefix.split('<').next().unwrap_or(&prefix).trim();
                format!("{}_{}", base, method)
            });
        let qualified_name = match qualified_name {
            Some(name) => name,
            // Type-driven dispatch failed for a call the checker accepted
            // — an internal invariant violation, surfaced through the
            // MIR-lowering error path rather than a stray print.
            None => return Err(LoweringError::InvalidConstruct(format!(
                "method `{}` on receiver of unresolved type — dispatch could \
                 not determine a stdlib type prefix",
                method
            ))),
        };

        // A method with its own type parameters was monomorphized per call, so
        // dispatch has to name the copy rather than the generic body.
        let qualified_name = self.ctx.call_rewrites
            .get(&expr.id)
            .cloned()
            .unwrap_or(qualified_name);

        // Track collection element types from push/insert so get returns the right type.
        // Handles both `v.push(x)` and `self.field.push(x)`.
        // Writes to both per-function and shared cross-function maps.
        if matches!(qualified_name.as_str(), "Vec_push" | "Vec_set" | "Pool_insert") {
            if let Some(arg_ty) = arg_types.first() {
                if !matches!(arg_ty, MirType::I64) {
                    if let Some(key) = Self::vec_tracking_key(object) {
                        self.meta_mut(&key).elem_type = Some(arg_ty.clone());
                        self.ctx.record_shared_elem(key, arg_ty.clone());
                    }
                }
            }
        }

        // Channel recv with struct elements: switch to struct variant
        // and inject elem_size so the builder can allocate the right buffer.
        let qualified_name = if qualified_name == "Receiver_receive" {
            let elem_size = self.channel_elem_size(object);
            if elem_size > 8 {
                all_args.push(MirOperand::Constant(MirConst::Int(elem_size)));
                "Receiver_receive_struct".to_string()
            } else {
                qualified_name
            }
        } else if qualified_name == "Receiver_try_receive" {
            // try_receive recvs into a buffer of the element's real size and
            // maps status→Result in codegen. Pass elem_size for the buffer.
            let elem_size = self.channel_elem_size(object);
            all_args.push(MirOperand::Constant(MirConst::Int(elem_size)));
            qualified_name
        } else {
            qualified_name
        };

        // Use tracked element type for Vec_get/index return instead of default I64.
        // Checks per-function map first, then shared cross-function map.
        let tracked_elem = if matches!(qualified_name.as_str(), "Vec_get" | "Vec_index") {
            Self::vec_tracking_key(object).and_then(|key| {
                self.meta(&key).and_then(|m| m.elem_type.clone())
                    .or_else(|| self.ctx.shared_elem_types.borrow().get(&key).cloned())
            })
        } else {
            None
        };
        let ret_ty = if qualified_name == "Vec_get" {
            // `.get()` returns T? (Option, none on OOB per V3). The call is
            // renamed to Vec_get_opt below so codegen uses the NULL-encoding
            // runtime + DerefOption adapter. The element (Option payload)
            // type sizes the result slot, so it must be right even when the
            // Vec wasn't push-tracked in this function (e.g. returned from a
            // callee): take the checker's `T?` payload first, then tracking.
            let elem = self.extract_payload_type(expr)
                .or(tracked_elem)
                .unwrap_or(MirType::I64);
            Some(MirType::Option(Box::new(elem)))
        } else if matches!(qualified_name.as_str(), "Vec_first" | "Vec_last") {
            // Same reasoning as Vec_get: these answer `T?`, and the payload type
            // sizes the result slot. From the stub metadata the result came back
            // as a bare `T`, so a `Vec<i64?>` got a 16-byte slot for a 24-byte
            // answer and the tag was read off the wrong bytes.
            let tracked = Self::vec_tracking_key(object).and_then(|key| {
                self.meta(&key).and_then(|m| m.elem_type.clone())
                    .or_else(|| self.ctx.shared_elem_types.borrow().get(&key).cloned())
            });
            // In a generic body the checker's payload is still the unresolved
            // type parameter; the receiver's tracked element type is the
            // concrete one after monomorphization.
            let elem = Self::better_payload_ty(self.extract_payload_type(expr), tracked)
                .unwrap_or(MirType::I64);
            Some(MirType::Option(Box::new(elem)))
        } else if qualified_name == "Map_get" {
            // Same reasoning as Vec_get: `Map.get` returns `V?`, and the payload
            // type sizes the result slot. The DerefOption adapter copies
            // `slot_size - tag` bytes out of the map's storage, so a bare
            // `i64?` copied only the value's first word — `self.users.get(id)`
            // handed back eight bytes of a `User` and reading a field off it
            // dereferenced the id.
            let payload = self.extract_payload_type(expr)
                .or_else(|| self.map_value_mir(object))
                .unwrap_or(MirType::I64);
            Some(MirType::Option(Box::new(payload)))
        } else if qualified_name == "Vec_index" {
            // Indexing (`v[i]`) panics on OOB and yields the raw element.
            //
            // Push tracking only sees Vecs built in this function, so a Vec that
            // arrived some other way — a struct field off a returned value, or a
            // `json.decode` result — fell through to i64 and `h.names[0]` printed
            // the string's first bytes as a number. The checker knows what the
            // index expression is; ask it when tracking has nothing.
            tracked_elem
        } else if qualified_name == "Vec_remove" {
            // `.remove(i)` hands back the element itself. The stub says `-> T`,
            // which the metadata fallback reads as a bare i64 — so a
            // `Vec<string>` element came back through an 8-byte out-param and
            // `positional.remove(0)` produced half a string's bytes as a number
            // (#203's grep_clone entry).
            //
            // Tracking first, like `v[i]`; the checker's type second, because a
            // `test` body doesn't carry the push tracking. Either way the width
            // is rounded up to a word: a Vec keeps scalars in 8-byte slots, and
            // an i32-typed destination read back only half of what the
            // out-param wrote.
            tracked_elem
                .or_else(|| self.ctx.lookup_node_type(expr.id))
                .map(vec_slot_type)
        } else if qualified_name == "Vec_pop" {
            // Same, one level in: `.pop()` is `T?`, and the payload type sizes
            // the slot the DerefOption adapter copies into. A bare `i64?` slot
            // took 8 of a string's 16 bytes and reading it segfaulted.
            tracked_elem
                .or_else(|| self.extract_payload_type(expr))
                .map(|elem| MirType::Option(Box::new(vec_slot_type(elem))))
        } else if qualified_name == "Pool_get" || qualified_name == "Pool_remove" {
            // Both return T? — extract T from the tracked element type. Without
            // this `remove` answered `i64?` regardless of what the pool held,
            // so reading a struct back out of it dereferenced a field offset
            // into a scalar (#356).
            let elem_ty = Self::vec_tracking_key(object)
                .and_then(|key| {
                    self.meta(&key).and_then(|m| m.elem_type.clone())
                        .or_else(|| self.ctx.shared_elem_types.borrow().get(&key).cloned())
                })
                // Fallback: extract from Pool<T> generic parameter
                .or_else(|| {
                    self.ctx.lookup_raw_type(object.id)
                        .and_then(|ty| match ty {
                            rask_types::Type::UnresolvedGeneric { args, .. } => {
                                args.first().and_then(|a| match a {
                                    rask_types::GenericArg::Type(t) => Some(t.as_ref()),
                                    _ => None,
                                })
                            }
                            _ => None,
                        })
                        .map(|elem_ty| self.ctx.type_to_mir(elem_ty))
                        .filter(|t| !matches!(t, MirType::Ptr))
                })
                .unwrap_or(MirType::I64);
            Some(MirType::Option(Box::new(elem_ty)))
        } else if qualified_name == "Cell_get" {
            // What the cell holds. The stub's return type is a bare word, so a
            // `Cell<string>` read came back as an i64 and printed as a pointer.
            self.ctx.lookup_node_type(expr.id).filter(|t| !matches!(t, MirType::Ptr))
        } else if qualified_name == "Receiver_receive_struct" {
            // Renamed from Receiver_receive above for struct elements. Only the
            // original name is in the stub metadata, so the fallback typed the
            // result a bare i64 — then `r?` read a tag off a local that never got
            // a Result slot and every receive looked like a failure (#463).
            self.ctx.lookup_node_type(expr.id)
                .filter(|t| matches!(t, MirType::Result { .. } | MirType::Option(_)))
                .or_else(|| self.func_sigs.get("Receiver_receive").map(|s| s.ret_ty.clone()))
                .or_else(|| Some(super::stdlib_return_mir_type("Receiver_receive")))
        } else if qualified_name == "string_parse"
            || qualified_name.strip_prefix("string_parse_")
                .is_some_and(super::is_parse_target_type_name)
        {
            // `parse<T>` yields `T or ParseError`. The stub's signature still
            // says the literal `T`, which maps to i64, and a mangled
            // `string_parse_<T>` isn't in the stub metadata at all — either way
            // the fallback below lands on a bare i64. The local then gets no
            // Result slot while the caller still reads a tag and payload off it
            // — garbage, and a segfault on the `??`. Prefer the checker's type;
            // rebuild it from the mangled type argument when node types aren't
            // available (instantiated bodies).
            let target = qualified_name.strip_prefix("string_parse_").unwrap_or("i64");
            Some(self.ctx.lookup_node_type(expr.id)
                .filter(|t| matches!(t, MirType::Result { ok, .. } if !matches!(**ok, MirType::Ptr)))
                .unwrap_or_else(|| MirType::Result {
                    ok: Box::new(self.ctx.resolve_type_str(target)),
                    err: Box::new(self.ctx.resolve_type_str("ParseError")),
                }))
        } else {
            None
        }.unwrap_or_else(|| self
            .func_sigs
            .get(&method)
            .or_else(|| self.func_sigs.get(&qualified_name))
            .map(|s| s.ret_ty.clone())
            .unwrap_or_else(|| super::stdlib_return_mir_type(&qualified_name)));

        // A method on a generic type is lowered once, so its signature says `T`
        // — which reaches MIR as a bare `Ptr`. The call site knows what `T`
        // became: `Box<string>.get()` returning `Ptr` meant the caller printed
        // the string's address as a number (#272).
        let ret_ty = if matches!(ret_ty, MirType::Ptr) {
            self.ctx.lookup_node_type(expr.id)
                .filter(|t| !matches!(t, MirType::Ptr | MirType::Void))
                .unwrap_or(ret_ty)
        } else {
            ret_ty
        };

        // Struct clone: inline field-by-field copy with deep clone for
        // heap fields (string, Vec, Map). Avoids needing a generated
        // runtime clone function for every user struct.
        if method == "clone" {
            if let MirType::Struct(StructLayoutId { id, .. }) = &obj_ty {
                if let Some(layout) = self.ctx.struct_layouts.get(*id as usize).cloned() {
                    let result_local = self.builder.alloc_temp(obj_ty.clone());
                    let src = all_args[0].clone();
                    for (idx, field) in layout.fields.iter().enumerate() {
                        // `field_index` is an index — codegen resolves the offset
                        // from the layout. Passing the byte offset here indexed past
                        // the end of the field list for every field but the first,
                        // and the out-of-range fallback read offset 0, so a
                        // `port: u16` at offset 16 cloned as whatever the first
                        // field's bytes happened to say (11824 for a host string).
                        //
                        // A field wider than a word comes back as a pointer into the
                        // source struct. Its clone needs a destination of the field's
                        // real type so `string_clone` has the 16-byte slot it copies
                        // into, and the store has to write all of it — as an i64 temp
                        // a `string` field was truncated to its first 8 bytes (#463).
                        // A struct/enum field also comes back as a pointer, even
                        // when it fits in a word — reading one gives the address
                        // of the field inside the source. Typed as an i64 temp
                        // that address got stored where the value belongs, so a
                        // `priority: Priority` field cloned to a stack address
                        // and every later read of it saw an out-of-range tag.
                        let field_mir_ty = self.ctx.type_to_mir(&field.ty);
                        let wide = field.size > 8 || super::mir_ty_is_aggregate(&field_mir_ty);
                        let field_val = self.builder.alloc_temp(if wide {
                            field_mir_ty.clone()
                        } else {
                            MirType::I64
                        });
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                            dst: field_val,
                            rvalue: MirRValue::Field {
                                base: src.clone(),
                                field_index: idx as u32,
                                byte_offset: None,
                                access: FieldAccess::Word,
                            },
                        }));
                        // Deep clone heap types
                        let clone_fn = Self::clone_fn_for_type(&field.ty);
                        let store_val = if let Some(cfn) = clone_fn {
                            let cloned_ty = if wide {
                                self.ctx.type_to_mir(&field.ty)
                            } else {
                                MirType::I64
                            };
                            let cloned = self.builder.alloc_temp(cloned_ty);
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                                dst: Some(cloned),
                                func: FunctionRef::internal(cfn.to_string()),
                                args: vec![MirOperand::Local(field_val)],
                            }));
                            MirOperand::Local(cloned)
                        } else {
                            MirOperand::Local(field_val)
                        };
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                            addr: result_local,
                            offset: field.offset,
                            value: store_val,
                            store_size: if wide { Some(field.size) } else { None },
                        }));
                    }
                    return Ok((MirOperand::Local(result_local), obj_ty));
                }
            }
            // Enum clone: copy tag, then switch on tag to deep-clone
            // heap fields per variant.
            if let MirType::Enum(EnumLayoutId { id, .. }) = &obj_ty {
                if let Some(layout) = self.ctx.enum_layouts.get(*id as usize).cloned() {
                    return self.lower_enum_clone(&layout, &all_args[0], obj_ty);
                }
            }
        }

        // Pool.alloc(value) → Pool_insert(pool, elem_ptr)
        // Pool_alloc takes no element arg; codegen Pool_insert appends elem_size
        let (final_name, final_args) = if qualified_name == "Pool_alloc" && all_args.len() == 2 {
            ("Pool_insert".to_string(), all_args)
        } else if qualified_name == "Vec_get" {
            // Safe `.get()` → Option-returning runtime (none on OOB, no panic).
            ("Vec_get_opt".to_string(), all_args)
        } else if qualified_name == "Vec_join" {
            // Vec_join assumes Vec<string>; use Vec_join_i64 for non-string elements
            if self.vec_elem_is_string(object) {
                (qualified_name.clone(), all_args)
            } else {
                ("Vec_join_i64".to_string(), all_args)
            }
        } else if qualified_name == "Vec_contains" && self.vec_elem_is_string(object) {
            // The byte-compare runtime can't match two equal heap strings —
            // they hold different pointers. Route strings to a real compare.
            ("Vec_contains_str".to_string(), all_args)
        } else {
            (qualified_name.clone(), all_args)
        };

        let result_local = self.builder.alloc_temp(ret_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(result_local),
            func: FunctionRef::internal(final_name.clone()),
            args: final_args,
        }));

        // W2a/W2b: Re-resolve pool bindings after pool mutators inside `with` blocks
        if matches!(final_name.as_str(),
            "Pool_insert" | "Pool_remove" | "Pool_clear" | "Pool_drain" | "Pool_alloc"
        ) {
            if let ExprKind::Ident(pool_var) = &object.kind {
                if let Some(bindings) = self.with_pool_bindings.get(pool_var) {
                    for &(handle_local, binding_local, pool_local) in bindings {
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::PoolCheckedAccess {
                            dst: binding_local,
                            pool: pool_local,
                            handle: handle_local,
                        }));
                    }
                }
            }
        }

        Ok((MirOperand::Local(result_local), ret_ty))
    }

    /// `c.func(args)` where `c` is the C namespace → extern "C" call.
    fn try_lower_c_namespace_call(
        &mut self,
        expr: &Expr,
        object: &Expr,
        method: &String,
        args: &[CallArg],
    ) -> Result<Option<TypedOperand>, LoweringError> {
        // C namespace call: c.func_name(args...) → extern "C" call
        if self.ctx.extern_funcs.contains(method) {
            if let ExprKind::Ident(ns) = &object.kind {
                if !self.locals.contains_key(ns) {
                    let mut arg_operands = Vec::new();
                    for arg in args {
                        let (op, _) = self.lower_expr(&arg.expr)?;
                        arg_operands.push(op);
                    }
                    let ret_ty = self.lookup_expr_type(expr).unwrap_or(MirType::I64);
                    let result_local = self.builder.alloc_temp(ret_ty.clone());
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: Some(result_local),
                        func: crate::FunctionRef::extern_c(method.clone()),
                        args: arg_operands,
                    }));
                    return Ok(Some((MirOperand::Local(result_local), ret_ty)));
                }
            }
        }
        Ok(None)
    }

    /// `.origin()` on a Result → formatted origin string.
    fn try_lower_origin(
        &mut self,
        object: &Expr,
        method: &String,
        args: &[CallArg],
    ) -> Result<Option<TypedOperand>, LoweringError> {
        // ER16: .origin() on Result — read origin fields and format as string
        if method == "origin" && args.is_empty() {
            let (obj_op, obj_ty) = self.lower_expr(object)?;
            if matches!(obj_ty, MirType::Result { .. }) {
                let result_local = self.builder.alloc_temp(MirType::String);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: Some(result_local),
                    func: crate::FunctionRef::internal("rask_result_origin".to_string()),
                    args: vec![obj_op],
                }));
                return Ok(Some((MirOperand::Local(result_local), MirType::String)));
            }
            // Non-Result: return "<no origin>"
            return Ok(Some((
                MirOperand::Constant(crate::operand::MirConst::String("<no origin>".to_string())),
                MirType::String,
            )));
        }
        Ok(None)
    }

    /// `.discriminant()` on an enum value → tag via EnumTag.
    fn try_lower_discriminant(
        &mut self,
        object: &Expr,
        method: &String,
        args: &[CallArg],
    ) -> Result<Option<TypedOperand>, LoweringError> {
        // E9: .discriminant() on enum values — extract tag via EnumTag
        if method == "discriminant" && args.is_empty() {
            let (obj_op, obj_ty) = self.lower_expr(object)?;
            if matches!(obj_ty, MirType::Enum(_)) {
                let result_local = self.builder.alloc_temp(MirType::U16);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result_local,
                    rvalue: MirRValue::EnumTag { value: obj_op },
                }));
                return Ok(Some((MirOperand::Local(result_local), MirType::U16)));
            }
        }
        Ok(None)
    }

    /// `module.Type.method()` → flattened `Type_method` qualified call.
    fn try_lower_module_type_method(
        &mut self,
        object: &Expr,
        method: &String,
        args: &[CallArg],
    ) -> Result<Option<TypedOperand>, LoweringError> {
        // Module.Type.method() pattern: time.Instant.now() → Instant_now
        // Detect field access on a module name and flatten to a qualified call.
        if let ExprKind::Field { object: inner_obj, field: type_name } = &object.kind {
            if let ExprKind::Ident(module_name) = &inner_obj.kind {
                // `Level.Low.label()` looks like the same shape but isn't:
                // `Level.Low` is an enum *value*, so flattening it to
                // `Low_label()` threw the receiver away and mangled the call
                // under the variant name instead of the enum's (#400).
                let is_enum_variant = self
                    .ctx
                    .find_enum(module_name)
                    .is_some_and(|(_, layout)| layout.variants.iter().any(|v| v.name == *type_name));
                if !self.locals.contains_key(module_name)
                    && !is_enum_variant
                    && is_type_constructor_name(module_name)
                {
                    let func_name = format!("{}_{}", type_name, method);
                    let mut arg_operands = Vec::new();
                    for arg in args {
                        let (op, _) = self.lower_expr(&arg.expr)?;
                        arg_operands.push(op);
                    }
                    let ret_ty = self
                        .func_sigs
                        .get(&func_name)
                        .map(|s| s.ret_ty.clone())
                        .unwrap_or_else(|| super::stdlib_return_mir_type(&func_name));
                    let result_local = self.builder.alloc_temp(ret_ty.clone());
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: Some(result_local),
                        func: FunctionRef::internal(func_name),
                        args: arg_operands,
                    }));
                    return Ok(Some((MirOperand::Local(result_local), ret_ty)));
                }
            }
        }
        Ok(None)
    }

    /// Raw-pointer methods (`.read()`, `.write()`, `.add()`, `.cast()`, ...)
    /// dispatched to `RawPtr_*` C functions. Skips smart-pointer types.
    fn try_lower_raw_ptr_method(
        &mut self,
        object: &Expr,
        method: &String,
        args: &[CallArg],
        obj_op: &MirOperand,
        obj_ty: &MirType,
    ) -> Result<Option<TypedOperand>, LoweringError> {
        // Raw pointer methods: dispatch directly to RawPtr_* C functions.
        // Skip for smart pointer types (Shared, Channel, etc.) that also use MirType::Ptr.
        let is_smart_ptr = self.ctx.lookup_raw_type(object.id)
            .and_then(|ty| super::MirContext::stdlib_type_prefix(ty))
            .map(|prefix| matches!(prefix, "Shared" | "Mutex" | "Channel" | "Sender" | "Receiver"))
            .unwrap_or(false)
            || if let ExprKind::Ident(var_name) = &object.kind {
                self.meta(var_name)
                    .and_then(|m| m.type_prefix.as_deref())
                    .map(|p| matches!(p, "Shared" | "Mutex" | "Channel" | "Sender" | "Receiver"))
                    .unwrap_or(false)
            } else {
                false
            };
        if matches!(obj_ty, MirType::Ptr) && !is_smart_ptr {
            let ptr_method = match method.as_str() {
                "read" | "write" | "add" | "sub" | "offset"
                | "is_null" | "is_aligned" | "is_aligned_to" | "align_offset" => {
                    Some(format!("RawPtr_{}", method))
                }
                "cast" => None, // cast is type-only, no runtime call
                _ => None,
            };
            if method == "cast" {
                // Cast is a no-op at runtime — pointer value unchanged
                return Ok(Some((obj_op.clone(), MirType::Ptr)));
            }
            if let Some(func_name) = ptr_method {
                // Determine element size from the pointer's type (*u8 → 1, *i64 → 8)
                let elem_size: i64 = self.ctx.lookup_raw_type(object.id)
                    .and_then(|ty| match ty {
                        rask_types::Type::RawPtr(inner) => Some(match inner.as_ref() {
                            rask_types::Type::U8 | rask_types::Type::I8 | rask_types::Type::Bool => 1,
                            rask_types::Type::U16 | rask_types::Type::I16 => 2,
                            rask_types::Type::U32 | rask_types::Type::I32 | rask_types::Type::F32 => 4,
                            _ => 8,
                        }),
                        _ => None,
                    })
                    .unwrap_or(8);

                let mut all_args = vec![obj_op.clone()];
                for arg in args {
                    let (op, _) = self.lower_expr(&arg.expr)?;
                    all_args.push(op);
                }
                // Inject element size for read/write/add/sub/offset
                if matches!(method.as_str(), "read" | "write" | "add" | "sub" | "offset") {
                    all_args.push(MirOperand::Constant(crate::operand::MirConst::Int(elem_size)));
                }
                let ret_ty = match method.as_str() {
                    "read" => MirType::I64,
                    "write" => MirType::Void,
                    "add" | "sub" | "offset" => MirType::Ptr,
                    "is_null" | "is_aligned" | "is_aligned_to" => MirType::Bool,
                    "align_offset" => MirType::I64,
                    _ => MirType::I64,
                };
                let result_local = self.builder.alloc_temp(ret_ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: Some(result_local),
                    func: FunctionRef::internal(func_name),
                    args: all_args,
                }));
                return Ok(Some((MirOperand::Local(result_local), ret_ty)));
            }
        }
        Ok(None)
    }

    /// `x == none` / `x != none` → option-tag comparison.
    fn try_lower_option_none_cmp(
        &mut self,
        object: &Expr,
        method: &String,
        args: &[CallArg],
        obj_op: &MirOperand,
    ) -> Result<Option<TypedOperand>, LoweringError> {
        // `x == none` / `x != none`: desugared to x.eq(none) / !(x.eq(none)).
        // Lower as a tag comparison — emit the option tag and compare to 1 (None).
        let is_option_none_cmp = (method == "eq" || method == "ne")
            && args.len() == 1
            && matches!(args[0].expr.kind, ExprKind::None)
            && self.ctx.lookup_raw_type(object.id)
                .map_or(false, |ty| ty.is_option());
        if is_option_none_cmp {
            let is_niche = self.option_operand_is_niche(object, obj_op);
            let tag_local = self.emit_option_tag(obj_op, is_niche);
            let result = self.builder.alloc_temp(MirType::Bool);
            // tag == 1 means None; tag == 0 means Some.
            // eq(none) → true when None (tag == 1)
            // ne(none) → true when Some (tag == 0), i.e. tag != 1
            let cmp_op = if method == "eq" {
                crate::operand::BinOp::Eq
            } else {
                crate::operand::BinOp::Ne
            };
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: result,
                rvalue: MirRValue::BinaryOp {
                    op: cmp_op,
                    left: MirOperand::Local(tag_local),
                    right: MirOperand::Constant(MirConst::Int(1)),
                },
            }));
            return Ok(Some((MirOperand::Local(result), MirType::Bool)));
        }
        Ok(None)
    }

    /// Desugared operator methods (`a + b` -> `a.add(b)`, `-a` -> `a.neg()`).
    /// Emits a native BinaryOp/UnaryOp unless the receiver needs a runtime
    /// call (strings, SIMD) or has a user operator overload.
    fn try_lower_operator_method(
        &mut self,
        object: &Expr,
        method: &String,
        args: &[CallArg],
        obj_op: &MirOperand,
        obj_ty: &MirType,
    ) -> Result<Option<TypedOperand>, LoweringError> {
        // Skip native binop for types that need C runtime calls (strings,
        // SIMD vectors) or special method dispatch (raw pointers:
        // ptr.add != arithmetic add).
        // When obj_ty is Ptr (type info lost), check the type checker to
        // see if the actual type is numeric — if so, use native binop.
        let raw_type_is_numeric = self.ctx.lookup_raw_type(object.id)
            .map(|ty| matches!(ty,
                rask_types::Type::I8 | rask_types::Type::I16 | rask_types::Type::I32 | rask_types::Type::I64
                | rask_types::Type::U8 | rask_types::Type::U16 | rask_types::Type::U32 | rask_types::Type::U64
                | rask_types::Type::F32 | rask_types::Type::F64 | rask_types::Type::Bool
            ))
            .unwrap_or(false);
        let skip_binop = if raw_type_is_numeric {
            false
        } else {
            matches!(obj_ty, MirType::String)
            || if let ExprKind::Ident(var_name) = &object.kind {
                self.meta(var_name)
                    .and_then(|m| m.type_prefix.as_deref())
                    .map(|p| matches!(p, "string" | "f32x4" | "f32x8" | "f64x2" | "f64x4" | "i32x4" | "i32x8" | "Ptr"))
                    .unwrap_or(false)
            } else {
                // Unknown type from complex expression — default to native
                // binop. The common case is numeric field access chains
                // (e.g. self.entries.len() / 2) where Ptr means lost type info.
                false
            }
        };

        // Operator overload: `a + b` desugars to `a.add(b)`. A native
        // BinaryOp only makes sense for primitive operands — on a Struct/Enum
        // receiver it would `sadd` two aggregate pointers as integers and hand
        // back garbage (#386). So dispatch any aggregate-receiver operator
        // method to the real `{Type}_{method}` instead. This is driven by the
        // MIR type, not the checker's node type, so it also covers receivers
        // the checker left untyped (e.g. a synthesized lock guard).
        //
        // Only when the type actually declares that operator, though. Without an
        // overload there is nothing to call, and `==`/`!=` on an aggregate is
        // meant to reach codegen's structural comparison (tag then payload for
        // enums, field by field for structs) as a BinaryOp. Routing every
        // aggregate operator to `{Type}_{method}` sent derived comparisons to a
        // function that was never emitted — `Status_eq` not found (#399/#463).
        let aggregate_receiver = matches!(obj_ty, MirType::Struct(_) | MirType::Enum(_));
        let has_operator_overload = aggregate_receiver
            && self.mir_type_name(obj_ty)
                .map(|ty_name| format!("{}_{}", ty_name, method))
                .is_some_and(|qualified| self.func_sigs.contains_key(&qualified));
        let skip_binop = skip_binop || has_operator_overload;

        // std.bits B1 on an integer receiver. These aren't operator methods —
        // they're named calls — but they lower the same way, to a single
        // machine instruction on the receiver's own width, so they belong here
        // rather than in the `{Type}_{method}` dispatch chain (which had no
        // `i64_count_ones` to find, #397).
        if matches!(obj_ty, MirType::I8 | MirType::I16 | MirType::I32 | MirType::I64
                          | MirType::U8 | MirType::U16 | MirType::U32 | MirType::U64)
        {
            if let Some(handled) = self.lower_int_bit_method(method, args, &obj_op, obj_ty)? {
                return Ok(Some(handled));
            }
        }

        // Detect binary operator methods (desugared from a + b → a.add(b))
        // Skip for SIMD types and raw pointers — they use method dispatch.
        if !skip_binop {
        if let Some(mir_binop) = operator_method_to_binop(method) {
            if args.len() == 1 {
                let (rhs, _) = self.lower_expr(&args[0].expr)?;
                let result_ty = binop_result_type(&mir_binop, obj_ty);
                let result_local = self.builder.alloc_temp(result_ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result_local,
                    rvalue: MirRValue::BinaryOp {
                        op: mir_binop,
                        left: obj_op.clone(),
                        right: rhs,
                    },
                }));
                return Ok(Some((MirOperand::Local(result_local), result_ty)));
            }
        }

        // Detect unary operator methods (desugared from -a → a.neg())
        if let Some(mir_unop) = operator_method_to_unaryop(method) {
            if args.is_empty() {
                let result_local = self.builder.alloc_temp(obj_ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result_local,
                    rvalue: MirRValue::UnaryOp {
                        op: mir_unop,
                        operand: obj_op.clone(),
                    },
                }));
                return Ok(Some((MirOperand::Local(result_local), obj_ty.clone())));
            }
        }
        } // end if !skip_binop
        Ok(None)
    }

    /// std.bits B1 bit methods on an integer receiver.
    ///
    /// The "ones" counts are the "zeros" counts of the complement, and
    /// `count_zeros` is `count_ones` of the complement, so those three compose
    /// from a BitNot rather than carrying MIR ops of their own. Every result
    /// keeps the receiver's type, matching what the checker unified.
    fn lower_int_bit_method(
        &mut self,
        method: &str,
        args: &[CallArg],
        obj_op: &MirOperand,
        obj_ty: &MirType,
    ) -> Result<Option<super::TypedOperand>, LoweringError> {
        use crate::operand::UnaryOp as MirUnaryOp;

        // Rotations take an amount; the rest are nullary.
        if let Some(rot) = match method {
            "rotate_left" => Some(crate::operand::BinOp::RotateLeft),
            "rotate_right" => Some(crate::operand::BinOp::RotateRight),
            _ => None,
        } {
            if args.len() != 1 {
                return Ok(None);
            }
            let (amount, _) = self.lower_expr(&args[0].expr)?;
            let out = self.builder.alloc_temp(obj_ty.clone());
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: out,
                rvalue: MirRValue::BinaryOp { op: rot, left: obj_op.clone(), right: amount },
            }));
            return Ok(Some((MirOperand::Local(out), obj_ty.clone())));
        }

        if !args.is_empty() {
            return Ok(None);
        }
        let (op, complement) = match method {
            "count_ones" => (MirUnaryOp::CountOnes, false),
            "count_zeros" => (MirUnaryOp::CountOnes, true),
            "leading_zeros" => (MirUnaryOp::LeadingZeros, false),
            "leading_ones" => (MirUnaryOp::LeadingZeros, true),
            "trailing_zeros" => (MirUnaryOp::TrailingZeros, false),
            "trailing_ones" => (MirUnaryOp::TrailingZeros, true),
            "reverse_bits" => (MirUnaryOp::ReverseBits, false),
            // Little-endian hosts: to_be swaps, to_le is already in order.
            "swap_bytes" | "to_be" => (MirUnaryOp::SwapBytes, false),
            "to_le" => return Ok(Some((obj_op.clone(), obj_ty.clone()))),
            _ => return Ok(None),
        };

        let input = if complement {
            let flipped = self.builder.alloc_temp(obj_ty.clone());
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: flipped,
                rvalue: MirRValue::UnaryOp { op: MirUnaryOp::BitNot, operand: obj_op.clone() },
            }));
            MirOperand::Local(flipped)
        } else {
            obj_op.clone()
        };

        let out = self.builder.alloc_temp(obj_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: out,
            rvalue: MirRValue::UnaryOp { op, operand: input },
        }));
        Ok(Some((MirOperand::Local(out), obj_ty.clone())))
    }

    /// String comparison operators → `string_lt`, `string_ge`, etc.
    /// Read an enum's variant tag into a fresh local.
    fn emit_enum_tag(&mut self, value: MirOperand) -> crate::LocalId {
        let tag = self.builder.alloc_temp(MirType::U16);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: tag,
            rvalue: MirRValue::EnumTag { value },
        }));
        tag
    }

    /// `a.compare(b)` on a number, char, bool, or fieldless enum → inline
    /// three-way compare (-1 / 0 / 1), the same shape the auto-derived struct
    /// `compare` emits.
    ///
    /// Neither has a `compare` anywhere: primitives have no runtime one, and the
    /// derive pass only writes bodies for structs. The fallback for an
    /// unqualified `compare` is `string_compare`, so comparing two integers read
    /// their values as string pointers and dereferenced them. For an enum,
    /// declaration order is the ordering (`type.enums`/CO1), so comparing tags is
    /// the whole operation.
    fn try_lower_primitive_compare(
        &mut self,
        method: &str,
        args: &[CallArg],
        obj_op: &MirOperand,
        obj_ty: &MirType,
    ) -> Result<Option<TypedOperand>, LoweringError> {
        if method != "compare" || args.len() != 1 {
            return Ok(None);
        }
        let scalar = matches!(
            obj_ty,
            MirType::I8 | MirType::I16 | MirType::I32 | MirType::I64
                | MirType::U8 | MirType::U16 | MirType::U32 | MirType::U64
                | MirType::F32 | MirType::F64
                | MirType::Char | MirType::Bool
        );
        // Only fieldless enums: a payload-carrying variant needs the payloads
        // compared too, which is the derive pass's job, not a tag compare.
        let fieldless_enum = match obj_ty {
            MirType::Enum(EnumLayoutId { id, .. }) => self
                .ctx
                .enum_layouts
                .get(*id as usize)
                .is_some_and(|l| l.variants.iter().all(|v| v.fields.is_empty())),
            _ => false,
        };
        if !scalar && !fieldless_enum {
            return Ok(None);
        }
        let (rhs, _) = self.lower_expr(&args[0].expr)?;
        let (obj_op, rhs) = if fieldless_enum {
            (
                MirOperand::Local(self.emit_enum_tag(obj_op.clone())),
                MirOperand::Local(self.emit_enum_tag(rhs)),
            )
        } else {
            (obj_op.clone(), rhs)
        };
        let obj_op = &obj_op;
        let result = self.builder.alloc_temp(MirType::I64);

        let lt_cond = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: lt_cond,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Lt,
                left: obj_op.clone(),
                right: rhs.clone(),
            },
        }));

        let less_block = self.builder.create_block();
        let not_less_block = self.builder.create_block();
        let greater_block = self.builder.create_block();
        let equal_block = self.builder.create_block();
        let done_block = self.builder.create_block();

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(lt_cond),
            then_block: less_block,
            else_block: not_less_block,
        }));

        self.builder.switch_to_block(less_block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: result,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(rask_stdlib::ORDERING_LESS))),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: done_block }));

        self.builder.switch_to_block(not_less_block);
        let gt_cond = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: gt_cond,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Gt,
                left: obj_op.clone(),
                right: rhs,
            },
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(gt_cond),
            then_block: greater_block,
            else_block: equal_block,
        }));

        self.builder.switch_to_block(greater_block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: result,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(rask_stdlib::ORDERING_GREATER))),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: done_block }));

        self.builder.switch_to_block(equal_block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: result,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(rask_stdlib::ORDERING_EQUAL))),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: done_block }));

        self.builder.switch_to_block(done_block);
        Ok(Some((MirOperand::Local(result), MirType::I64)))
    }

    fn try_lower_string_compare(
        &mut self,
        object: &Expr,
        method: &String,
        args: &[CallArg],
        obj_op: &MirOperand,
        obj_ty: &MirType,
    ) -> Result<Option<TypedOperand>, LoweringError> {
        // String comparison operators: route to string_lt, string_ge, etc.
        let is_string_obj = matches!(obj_ty, MirType::String) || self.ctx.lookup_raw_type(object.id)
            .map(|ty| matches!(ty, rask_types::Type::String))
            .unwrap_or(false);
        // `compare` answers with an Ordering, not the C runtime's -1/0/1
        // (ORD1). The tags run Less, Equal, Greater, so the shift is +1.
        if is_string_obj && args.len() == 1 && method == "compare" {
            let (rhs, _) = self.lower_expr(&args[0].expr)?;
            let raw = self.builder.alloc_temp(MirType::I64);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(raw),
                func: FunctionRef::internal("string_compare".to_string()),
                args: vec![obj_op.clone(), rhs],
            }));
            let tag = self.builder.alloc_temp(MirType::I64);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: tag,
                rvalue: MirRValue::BinaryOp {
                    op: crate::operand::BinOp::Add,
                    left: MirOperand::Local(raw),
                    right: MirOperand::Constant(MirConst::Int(rask_stdlib::ORDERING_EQUAL)),
                },
            }));
            return Ok(Some((MirOperand::Local(tag), MirType::I64)));
        }
        if is_string_obj && args.len() == 1 {
            let string_cmp_fn = match method.as_str() {
                "eq" => Some("string_eq"),
                "lt" => Some("string_lt"),
                "gt" => Some("string_gt"),
                "le" => Some("string_le"),
                "ge" => Some("string_ge"),
                _ => None,
            };
            if let Some(func_name) = string_cmp_fn {
                let (rhs, _) = self.lower_expr(&args[0].expr)?;
                let result_local = self.builder.alloc_temp(MirType::Bool);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: Some(result_local),
                    func: FunctionRef::internal(func_name.to_string()),
                    args: vec![obj_op.clone(), rhs],
                }));
                return Ok(Some((MirOperand::Local(result_local), MirType::Bool)));
            }
        }
        Ok(None)
    }

    /// `concat()` string concatenation from interpolation.
    fn try_lower_string_concat(
        &mut self,
        method: &String,
        args: &[CallArg],
        obj_op: &MirOperand,
        obj_ty: &MirType,
    ) -> Result<Option<TypedOperand>, LoweringError> {
        // concat(): string concatenation from interpolation
        if method == "concat" && args.len() == 1 && matches!(obj_ty, MirType::String) {
            let (arg_op, _) = self.lower_expr(&args[0].expr)?;
            let result_local = self.builder.alloc_temp(MirType::String);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(result_local),
                func: FunctionRef::internal("concat".to_string()),
                args: vec![obj_op.clone(), arg_op],
            }));
            return Ok(Some((MirOperand::Local(result_local), MirType::String)));
        }
        Ok(None)
    }

    /// `x.__fmt(type, width, precision, align, fill)` — what desugaring makes
    /// of `{x:spec}`. The spec's already parsed, so this picks the base
    /// conversion by receiver type and then pads (std.fmt/CM5).
    fn try_lower_fmt(
        &mut self,
        expr: &Expr,
        object: &Expr,
        method: &String,
        args: &[CallArg],
        obj_op: &MirOperand,
        obj_ty: &MirType,
    ) -> Result<Option<TypedOperand>, LoweringError> {
        if method != "__fmt" || args.len() != 5 {
            return Ok(None);
        }

        // Every argument is a literal the desugar pass put there.
        let int_arg = |i: usize| match &args[i].expr.kind {
            ExprKind::Int(n, _) => *n,
            _ => 0,
        };
        let fill = match &args[4].expr.kind {
            ExprKind::Char(c) => *c,
            _ => ' ',
        };
        let spec = rask_ast::fmt_spec::FormatSpec::decode(
            int_arg(0), int_arg(1), int_arg(2), int_arg(3), fill,
        );

        let is_unsigned = matches!(
            obj_ty,
            MirType::U64 | MirType::U32 | MirType::U16 | MirType::U8
        );
        let is_int = is_unsigned
            || matches!(obj_ty, MirType::I64 | MirType::I32 | MirType::I16 | MirType::I8);
        let is_float = matches!(obj_ty, MirType::F64 | MirType::F32);
        let numeric = is_int || is_float;

        // Stage 1: render the value. An unsupported pairing (a hex spec on a
        // string, say) falls back to the plain rendering rather than failing —
        // same as the interpreter.
        let call = |this: &mut Self, name: &str, args: Vec<MirOperand>| {
            let dst = this.builder.alloc_temp(MirType::String);
            this.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(dst),
                func: FunctionRef::internal(name.to_string()),
                args,
            }));
            MirOperand::Local(dst)
        };
        let int_const = |n: i64| MirOperand::Constant(MirConst::Int(n));

        use rask_ast::fmt_spec::SpecType;
        let base: MirOperand = match spec.ty {
            SpecType::Hex { upper } if is_int => {
                let name = if is_unsigned { "u64_to_base" } else { "i64_to_base" };
                call(self, name, vec![obj_op.clone(), int_const(16), int_const(upper as i64)])
            }
            SpecType::Binary if is_int => {
                let name = if is_unsigned { "u64_to_base" } else { "i64_to_base" };
                call(self, name, vec![obj_op.clone(), int_const(2), int_const(0)])
            }
            SpecType::Octal if is_int => {
                let name = if is_unsigned { "u64_to_base" } else { "i64_to_base" };
                call(self, name, vec![obj_op.clone(), int_const(8), int_const(0)])
            }
            SpecType::Exp if is_float => call(self, "f64_to_exp", vec![obj_op.clone()]),
            SpecType::Debug if matches!(obj_ty, MirType::String) => {
                call(self, "string_debug", vec![obj_op.clone()])
            }
            SpecType::Debug if matches!(obj_ty, MirType::Char) => {
                call(self, "char_debug", vec![obj_op.clone()])
            }
            SpecType::Display if is_float && spec.precision.is_some() => {
                let prec = spec.precision.unwrap() as i64;
                call(self, "f64_to_precision", vec![obj_op.clone(), int_const(prec)])
            }
            SpecType::Display if matches!(obj_ty, MirType::String) && spec.precision.is_some() => {
                let prec = spec.precision.unwrap() as i64;
                call(self, "string_truncate_chars", vec![obj_op.clone(), int_const(prec)])
            }
            // Everything else renders the ordinary way — including `debug` on
            // a struct or enum, which goes to the type's own to_string.
            _ => {
                // A struct or enum renders through its own body. Which one
                // that is — `to_string` or a `message` bridged to it — was
                // settled during reachability; without that name the receiver
                // would be handed to `string_pad` as if the pointer were text.
                match self.ctx.call_rewrites.get(&expr.id).cloned() {
                    Some(renderer) => call(self, &renderer, vec![obj_op.clone()]),
                    None => {
                        let (op, _) = self
                            .try_lower_to_string(object, &"to_string".to_string(), &[], obj_op, obj_ty)?
                            .unwrap_or_else(|| (obj_op.clone(), MirType::String));
                        op
                    }
                }
            }
        };

        // Stage 2: pad. Text reads left, numbers read right (S2).
        if spec.width == 0 {
            return Ok(Some((base, MirType::String)));
        }
        let align = spec.effective_align(numeric).as_code();
        let padded = call(
            self,
            "string_pad",
            vec![
                base,
                int_const(spec.width as i64),
                int_const(align),
                MirOperand::Constant(MirConst::Char(spec.fill)),
            ],
        );
        Ok(Some((padded, MirType::String)))
    }

    /// `.to_string()` on a primitive → type-specific runtime call. Types with
    /// their own to_string fall through to normal dispatch.
    fn try_lower_to_string(
        &mut self,
        object: &Expr,
        method: &String,
        args: &[CallArg],
        obj_op: &MirOperand,
        obj_ty: &MirType,
    ) -> Result<Option<TypedOperand>, LoweringError> {
        // to_string(): route to type-specific runtime function.
        // Types with their own to_string in stdlib dispatch (Path, etc.)
        // fall through to normal method dispatch.
        if method == "to_string" && args.is_empty() {
            // Check if the type checker knows this is a type with its own to_string
            let has_own_to_string = self.ctx.lookup_raw_type(object.id)
                .and_then(|ty| super::MirContext::type_prefix(ty, self.ctx.type_names))
                .map(|prefix| {
                    let qualified = format!("{}_to_string", prefix);
                    rask_stdlib::mir_metadata::lookup(&qualified).is_some()
                })
                .unwrap_or(false);

            // A struct or enum receiver has a layout name to dispatch on, so
            // its own `to_string` wins — including a user `Displayable` impl,
            // which the stdlib stub check above can't see. Without this the
            // catch-all below reached `i64_to_string` and printed the
            // receiver's address as a decimal number (#471).
            let is_user_aggregate = self.mir_aggregate_prefix(obj_ty).is_some();

            if !has_own_to_string && !is_user_aggregate {
                let func_name = match obj_ty {
                    MirType::String => {
                        return Ok(Some((obj_op.clone(), MirType::String)));
                    }
                    MirType::I64 | MirType::I32 | MirType::I16 | MirType::I8 => "i64_to_string",
                    // Unsigned values print unsigned. Shared with the signed
                    // helper, `u8` 200 came out as -56 (#326).
                    MirType::U64 | MirType::U32 | MirType::U16 | MirType::U8 => "u64_to_string",
                    MirType::F64 => "f64_to_string",
                    MirType::F32 => "f32_to_string",
                    MirType::Bool => "bool_to_string",
                    MirType::Char => "char_to_string",
                    _ => "i64_to_string",
                };
                let result_local = self.builder.alloc_temp(MirType::String);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: Some(result_local),
                    func: FunctionRef::internal(func_name.to_string()),
                    args: vec![obj_op.clone()],
                }));
                return Ok(Some((MirOperand::Local(result_local), MirType::String)));
            }
        }
        Ok(None)
    }

    /// `.map_err(f)` / `.map_err(Variant)` — inline error-payload transform.
    fn try_lower_map_err(
        &mut self,
        method: &String,
        args: &[CallArg],
        obj_op: &MirOperand,
        obj_ty: &MirType,
    ) -> Result<Option<TypedOperand>, LoweringError> {
        // map_err: inline expansion — branch on tag, transform error payload
        if method == "map_err" && args.len() == 1 {
            if matches!(&args[0].expr.kind, ExprKind::Closure { params, .. } if params.len() == 1) {
                return self.lower_map_err(obj_op.clone(), obj_ty, &args[0].expr).map(Some);
            }
            // Variant constructor: result.map_err(MyError) or
            // result.map_err(ConfigError.Io)
            if let ExprKind::Ident(name) = &args[0].expr.kind {
                return self.lower_map_err_constructor(obj_op.clone(), obj_ty, name).map(Some);
            }
            // Qualified variant: EnumName.Variant
            if let ExprKind::Field { object, field } = &args[0].expr.kind {
                if matches!(&object.kind, ExprKind::Ident(_)) {
                    return self.lower_map_err_constructor(obj_op.clone(), obj_ty, field).map(Some);
                }
            }
        }
        Ok(None)
    }

    /// `.ok()` / `.to_option()`: Result -> Option.
    fn try_lower_ok_to_option(
        &mut self,
        expr: &Expr,
        object: &Expr,
        method: &String,
        args: &[CallArg],
        obj_op: &MirOperand,
        obj_ty: &MirType,
    ) -> Result<Option<TypedOperand>, LoweringError> {
        // .ok() / .to_option(): Result<T,E> → Option<T>.
        // Try the inline lowering (try_lower_result_option_method)
        // first so payload offsets are recomputed. The legacy
        // pass-through here was lying about the layout — Result's
        // origin fields between tag and payload don't exist in
        // Option, so subsequent `.0` reads landed on the wrong
        // bytes and `opt == none` checks compared stale pointers.
        if (method == "ok" || method == "to_option") && args.is_empty() {
            if let Some(handled) = self.try_lower_result_option_method(
                expr, object, method.as_str(), args, obj_op, obj_ty,
            )? {
                return Ok(Some(handled));
            }
            // Fallback for cases the inline lowerer couldn't handle
            // (no resolved receiver type, etc.) — pass the already-lowered
            // receiver through. Re-lowering here would double any side
            // effect in `object` (e.g. `tx.send(x).ok()` sending twice).
            return Ok(Some((obj_op.clone(), obj_ty.clone())));
        }
        Ok(None)
    }

    /// `.unwrap()` on Option/Result — panic on None/Err. Includes the
    /// `.get(i).unwrap()` collection special-cases.
    fn try_lower_unwrap(
        &mut self,
        object: &Expr,
        method: &String,
        args: &[CallArg],
        obj_op: &MirOperand,
        obj_ty: &MirType,
    ) -> Result<Option<TypedOperand>, LoweringError> {
        // .unwrap(): Option<T>/Result<T,E> → T — panic on None/Err
        // Special case: .get(i).unwrap() on collections.
        // Vec_get panics on OOB → unwrap is a no-op.
        // Map_get returns NULL on missing key → rewrite to Map_get_unwrap.
        if method == "unwrap" && args.is_empty() {
            if let ExprKind::MethodCall { method: inner_method, object: inner_obj, .. } = &object.kind {
                if inner_method == "get" {
                    // Only rewrite Map_get → Map_get_unwrap, not Pool_get
                    let is_map = if let ExprKind::Ident(name) = &inner_obj.kind {
                        self.meta(name.as_str())
                            .and_then(|m| m.type_prefix.as_deref())
                            .map_or(false, |p| p == "Map")
                    } else { false };
                    if is_map {
                        self.builder.rewrite_last_call("Map_get", "Map_get_unwrap");
                        return Ok(Some((obj_op.clone(), obj_ty.clone())));
                    }
                }
            }
        }
        if method == "unwrap" && args.is_empty() {
            let is_niche = self.option_is_niche(object, &obj_ty);
            let tag_local = self.emit_option_tag(obj_op, is_niche);

            let ok_block = self.builder.create_block();
            let panic_block = self.builder.create_block();
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                cond: MirOperand::Local(tag_local),
                then_block: panic_block,
                else_block: ok_block,
            }));

            self.builder.switch_to_block(panic_block);

            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::internal("panic_unwrap".to_string()),
                args: vec![],
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Unreachable));

            self.builder.switch_to_block(ok_block);
            let payload_ty = self.extract_payload_type(object)
                .unwrap_or(MirType::I64);
            let result_local = self.emit_option_payload(obj_op.clone(), payload_ty.clone(), is_niche);
            return Ok(Some((MirOperand::Local(result_local), payload_ty)));
        }
        Ok(None)
    }

    /// `Array.len()` -> compile-time constant.
    fn try_lower_array_len(
        &mut self,
        method: &String,
        args: &[CallArg],
        obj_ty: &MirType,
    ) -> Result<Option<TypedOperand>, LoweringError> {
        // Array.len() → compile-time constant (no runtime call)
        if method == "len" && args.is_empty() {
            if let MirType::Array { len, .. } = obj_ty {
                return Ok(Some((
                    MirOperand::Constant(MirConst::Int(*len as i64)),
                    MirType::I64,
                )));
            }
        }
        Ok(None)
    }

    /// Method call on `any Trait` -> vtable dispatch.
    fn try_lower_trait_object(
        &mut self,
        expr: &Expr,
        method: &String,
        args: &[CallArg],
        obj_op: &MirOperand,
        obj_ty: &MirType,
    ) -> Result<Option<TypedOperand>, LoweringError> {
        // Trait object dispatch: method call on `any Trait`
        if let MirType::TraitObject { ref trait_name } = obj_ty {
            if let Some(methods) = self.ctx.trait_methods.get(trait_name) {
                if let Some(idx) = methods.iter().position(|m| m == method) {
                    let vtable_offset = 24 + (idx as u32) * 8;
                    let mut arg_operands = Vec::new();
                    for arg in args {
                        let (op, _) = self.lower_expr(&arg.expr)?;
                        arg_operands.push(op);
                    }
                    // Resolve return type from type checker or fall back to i64
                    let ret_ty = self.ctx.lookup_raw_type(expr.id)
                        .map(|t| self.ctx.type_to_mir(t))
                        .unwrap_or(MirType::I64);
                    let result_local = self.builder.alloc_temp(ret_ty.clone());
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::TraitCall {
                        dst: Some(result_local),
                        trait_object: match obj_op {
                            MirOperand::Local(id) => *id,
                            _ => return Err(LoweringError::InvalidConstruct(
                                "trait object must be a local variable".to_string()
                            )),
                        },
                        method_name: method.clone(),
                        vtable_offset,
                        args: arg_operands,
                    }));
                    return Ok(Some((MirOperand::Local(result_local), ret_ty)));
                }
            }
        }
        Ok(None)
    }

    fn lower_if(
        &mut self,
        cond: &Expr,
        then_branch: &Expr,
        else_branch: Option<&Expr>,
        else_binding: Option<&str>,
    ) -> Result<TypedOperand, LoweringError> {
        // OPT19/OPT20 + ER19/ER20/ER21/ER22: `if x?` / `if x? as v` /
        // `... else as e` evaluate the scrutinee once and rebind the
        // payload as a local in the matching branch.
        if let ExprKind::IsPresent { expr: inner, binding } = &cond.kind {
            let then_name = match (binding.as_deref(), &inner.kind) {
                (Some(v), _) => Some(v.to_string()),
                (None, ExprKind::Ident(n)) => Some(n.clone()),
                _ => None,
            };
            let else_name = else_binding.map(|s| s.to_string()).or_else(|| then_name.clone());
            if then_name.is_some() || else_name.is_some() {
                return self.lower_if_present(
                    inner,
                    then_branch,
                    else_branch,
                    then_name,
                    else_name,
                );
            }
        }

        let (cond_op, _) = self.lower_expr(cond)?;

        let then_block = self.builder.create_block();
        let else_block = self.builder.create_block();
        let merge_block = self.builder.create_block();

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: cond_op,
            then_block,
            else_block,
        }));

        // Then branch
        self.builder.switch_to_block(then_block);
        let (then_val, then_ty) = self.lower_expr(then_branch)?;
        let result_local = self.builder.alloc_temp(then_ty.clone());
        // Only add merge-goto if the branch didn't already terminate (e.g. return)
        if self.builder.current_block_unterminated() {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: result_local,
                rvalue: MirRValue::Use(then_val),
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                target: merge_block,
            }));
        }

        // Else branch
        self.builder.switch_to_block(else_block);
        if let Some(else_expr) = else_branch {
            let (else_val, _) = self.lower_expr(else_expr)?;
            if self.builder.current_block_unterminated() {
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Use(else_val),
                }));
            }
        }
        if self.builder.current_block_unterminated() {
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                target: merge_block,
            }));
        }

        self.builder.switch_to_block(merge_block);

        Ok((MirOperand::Local(result_local), then_ty))
    }

    /// Lower `&&` / `||` with short-circuit semantics.
    ///
    /// For `lhs && rhs`: if lhs is false, skip rhs and yield false.
    /// For `lhs || rhs`: if lhs is true, skip rhs and yield true.
    fn lower_short_circuit(
        &mut self,
        op: BinOp,
        left: &Expr,
        right: &Expr,
    ) -> Result<TypedOperand, LoweringError> {
        let (left_op, _) = if matches!(op, BinOp::And) {
            self.lower_and_operand(left)?
        } else {
            self.lower_expr(left)?
        };

        let rhs_block = self.builder.create_block();
        let short_block = self.builder.create_block();
        let merge_block = self.builder.create_block();

        // `&&`: take rhs branch when lhs is true. `||`: take rhs branch when lhs is false.
        let (then_block, else_block) = match op {
            BinOp::And => (rhs_block, short_block),
            BinOp::Or => (short_block, rhs_block),
            _ => unreachable!(),
        };
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: left_op,
            then_block,
            else_block,
        }));

        let result_local = self.builder.alloc_temp(MirType::Bool);

        // Short-circuit branch: yield the lhs value (false for &&, true for ||).
        self.builder.switch_to_block(short_block);
        let short_val = match op {
            BinOp::And => MirOperand::Constant(MirConst::Int(0)),
            BinOp::Or => MirOperand::Constant(MirConst::Int(1)),
            _ => unreachable!(),
        };
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: result_local,
            rvalue: MirRValue::Use(short_val),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: merge_block,
        }));

        // Long branch: evaluate rhs, yield rhs.
        self.builder.switch_to_block(rhs_block);
        let (right_op, _) = if matches!(op, BinOp::And) {
            self.lower_and_operand(right)?
        } else {
            self.lower_expr(right)?
        };
        if self.builder.current_block_unterminated() {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: result_local,
                rvalue: MirRValue::Use(right_op),
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                target: merge_block,
            }));
        }

        self.builder.switch_to_block(merge_block);

        Ok((MirOperand::Local(result_local), MirType::Bool))
    }

    /// Lower one operand of an `&&` chain. An `is` pattern anywhere in the
    /// chain has to bind its payload, because everything to its right — and the
    /// branch the chain guards — only runs when it matched (#256).
    fn lower_and_operand(&mut self, expr: &Expr) -> Result<TypedOperand, LoweringError> {
        match &expr.kind {
            ExprKind::IsPattern { expr: scrutinee, pattern } => {
                self.lower_is_pattern_binding(scrutinee, pattern)
            }
            _ => self.lower_expr(expr),
        }
    }

    /// Lower `scrutinee is Pattern` as a bool, binding the payload on the
    /// matched path. The bare `IsPattern` lowering is just a tag comparison and
    /// drops the bindings; extracting the payload unconditionally isn't an
    /// option either, since a wrong-variant read can produce a bogus string.
    fn lower_is_pattern_binding(
        &mut self,
        scrutinee: &Expr,
        pattern: &rask_ast::expr::Pattern,
    ) -> Result<TypedOperand, LoweringError> {
        let (val, val_ty) = self.lower_expr(scrutinee)?;
        let is_niche = self.option_is_niche(scrutinee, &val_ty);
        let tag = self.emit_option_tag(&val, is_niche);
        let expected = self.pattern_tag_in_type_context(pattern, &val_ty);

        let matches = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: matches,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Eq,
                left: MirOperand::Local(tag),
                right: MirOperand::Constant(MirConst::Int(expected)),
            },
        }));

        let bind_block = self.builder.create_block();
        let short_block = self.builder.create_block();
        let merge_block = self.builder.create_block();

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(matches),
            then_block: bind_block,
            else_block: short_block,
        }));

        let result_local = self.builder.alloc_temp(MirType::Bool);

        // No-match path: short-circuit to false.
        self.builder.switch_to_block(short_block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: result_local,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: merge_block,
        }));

        // Match path: bind the payload, yield true.
        self.builder.switch_to_block(bind_block);
        let payload_ty = self
            .extract_payload_type(scrutinee)
            .unwrap_or(MirType::I64);
        self.bind_pattern_payload_niche(pattern, val, payload_ty, is_niche, &val_ty);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: result_local,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(1))),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: merge_block,
        }));

        self.builder.switch_to_block(merge_block);
        Ok((MirOperand::Local(result_local), MirType::Bool))
    }

    /// Lower `if expr? [as v] { then } [else [as e] { else_br }]` — present-check
    /// with payload narrowing. Mirrors the interpreter path in
    /// `rask-interp/src/interp/eval_expr.rs::ExprKind::If(IsPresent ..)`.
    fn lower_if_present(
        &mut self,
        inner: &Expr,
        then_branch: &Expr,
        else_branch: Option<&Expr>,
        then_name: Option<String>,
        else_name: Option<String>,
    ) -> Result<TypedOperand, LoweringError> {
        let (val, scrutinee_ty) = self.lower_expr(inner)?;
        let is_niche = self.option_is_niche(inner, &scrutinee_ty);
        let tag = self.emit_option_tag(&val, is_niche);

        // Branch on tag: 0 = present (Some/Ok), nonzero = absent (None/Err).
        let is_present = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: is_present,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Eq,
                left: MirOperand::Local(tag),
                right: MirOperand::Constant(MirConst::Int(0)),
            },
        }));

        let then_block = self.builder.create_block();
        let else_block = self.builder.create_block();
        let merge_block = self.builder.create_block();

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(is_present),
            then_block,
            else_block,
        }));

        // Then: bind the present payload as the narrow name, lower body.
        self.builder.switch_to_block(then_block);
        let payload_ty = self.extract_payload_type(inner)
            .or_else(|| Self::payload_of_mir(&scrutinee_ty))
            .unwrap_or(MirType::I64);
        if let Some(name) = then_name.as_ref() {
            let local = self.builder.alloc_local(name.clone(), payload_ty.clone());
            let rvalue = if is_niche {
                MirRValue::Use(val.clone())
            } else {
                // Scalar payloads need the explicit offset so codegen loads the
                // value at RESULT_PAYLOAD_OFFSET. Without it, a `T or E` whose
                // err side is an aggregate makes codegen guess "aggregate" and
                // return the slot address instead of the ok scalar (#389).
                MirRValue::Field {
                    base: val.clone(),
                    field_index: 0,
                    byte_offset: self.payload_byte_offset(&payload_ty),
                    access: FieldAccess::Word,
                }
            };
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign { dst: local, rvalue }));
            self.locals.insert(name.clone(), (local, payload_ty.clone()));
            if let Some(prefix) = self.mir_type_name(&payload_ty) {
                self.meta_mut(name).type_prefix = Some(prefix);
            }
        }
        let (then_val, then_ty) = self.lower_expr(then_branch)?;
        let result_local = self.builder.alloc_temp(then_ty.clone());
        if self.builder.current_block_unterminated() {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: result_local,
                rvalue: MirRValue::Use(then_val),
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                target: merge_block,
            }));
        }

        // Else: for Result, bind the err payload (field 0) as the else name.
        // For Option, None has no payload so skip the bind.
        self.builder.switch_to_block(else_block);
        let else_err_ty = self.extract_err_type(inner)
            .or_else(|| match &scrutinee_ty {
                MirType::Result { err, .. } => Some((**err).clone()),
                _ => None,
            });
        if let (Some(name), Some(err_ty)) = (else_name.as_ref(), else_err_ty) {
            let local = self.builder.alloc_local(name.clone(), err_ty.clone());
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: local,
                rvalue: MirRValue::Field {
                    base: val.clone(),
                    field_index: 0,
                    byte_offset: self.payload_byte_offset(&err_ty),
                    access: FieldAccess::Word,
                },
            }));
            self.locals.insert(name.clone(), (local, err_ty.clone()));
            if let Some(prefix) = self.mir_type_name(&err_ty) {
                self.meta_mut(name).type_prefix = Some(prefix);
            }
        }
        if let Some(else_expr) = else_branch {
            let (else_val, _) = self.lower_expr(else_expr)?;
            if self.builder.current_block_unterminated() {
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Use(else_val),
                }));
            }
        }
        if self.builder.current_block_unterminated() {
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                target: merge_block,
            }));
        }

        self.builder.switch_to_block(merge_block);
        Ok((MirOperand::Local(result_local), then_ty))
    }

    /// Block expression: lower each statement, last expression is the value.
    pub(super) fn lower_block(&mut self, stmts: &[Stmt]) -> Result<TypedOperand, LoweringError> {
        let mut last_val = MirOperand::Constant(MirConst::Int(0));
        let mut last_ty = MirType::Void;
        // Block scope: any const/mut declared inside the braces shadows but
        // doesn't leak out. Snapshot the locals map and restore after.
        let saved_locals = self.locals.clone();
        for (i, stmt) in stmts.iter().enumerate() {
            if i == stmts.len() - 1 {
                if let StmtKind::Expr(e) = &stmt.kind {
                    let (val, ty) = self.lower_expr(e)?;
                    last_val = val;
                    last_ty = ty;
                    continue;
                }
            }
            self.lower_stmt(stmt)?;
        }
        self.locals = saved_locals;
        Ok((last_val, last_ty))
    }
} // end impl MirLowerer
