// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Expression lowering.

use crate::FieldAccess;
use super::{
    binop_result_type, concurrency::BoxWithSyms, is_type_constructor_name, lower_binop,
    lower_unaryop, operator_method_to_binop, operator_method_to_unaryop, LoopContext,
    LoweringError, MirLowerer, TypedOperand,
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
/// Returns `Some((left_expr, right_expr, op_str))` if the condition is a
/// desugared comparison. After desugar: `a == b` → `a.eq(b)`,
/// `a != b` → `!(a.eq(b))`, `a < b` → `a.lt(b)`, etc.
///
/// Which fail helper the operands need is the caller's decision, made from the
/// lowered types. This used to answer "is it a string?" here from the source
/// shape — true only when one side was written as a literal — so `assert a == b`
/// on two string variables took the i64 helper and printed the two `RaskStr`
/// slot addresses (#897).
fn extract_assert_comparison(condition: &Expr) -> Option<(&Expr, &Expr, &'static str)> {
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
            Some((object.as_ref(), &args[0].expr, op_str))
        }
        // Desugared !=: !(a.eq(b))
        ExprKind::Unary { op: UnaryOp::Not, operand } => {
            if let ExprKind::MethodCall { object, method, args, .. } = &operand.kind {
                if method == "eq" && args.len() == 1 {
                    return Some((object.as_ref(), &args[0].expr, "!="));
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

/// An annotation attachment's value text as a MIR constant
/// (type.annotations/AN1, AN6). The declared type decides how the text reads —
/// `3` is an integer for `weight: i64` and a float for `scale: f64` — so this
/// takes the checker's answer rather than guessing from the digits.
fn annotation_const(value: &str, ty: &MirType) -> Option<MirConst> {
    use rask_ast::decl::field_attrs;
    let value = value.trim();
    match ty {
        MirType::Bool => match value {
            "true" => Some(MirConst::Bool(true)),
            "false" => Some(MirConst::Bool(false)),
            _ => None,
        },
        MirType::F32 | MirType::F64 => value.parse::<f64>().ok().map(MirConst::Float),
        MirType::String => field_attrs::string_literal(value).map(MirConst::String),
        _ if ty.is_int_like() => value.parse::<i64>().ok().map(MirConst::Int),
        _ => None,
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
        "i64" => (i64::MIN, i64::MAX, MirType::I64),
        "isize" => (i64::MIN, i64::MAX, MirType::isize_ty()),
        "u8" => (0, u8::MAX as i64, MirType::U8),
        "u16" => (0, u16::MAX as i64, MirType::U16),
        "u32" => (0, u32::MAX as i64, MirType::U32),
        "u64" => (0, u64::MAX as i64, MirType::U64),
        "usize" => (0, u64::MAX as i64, MirType::usize_ty()),
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


impl<'a> MirLowerer<'a> {
    /// A scalar whose layout is known here and now. Deliberately not `Ptr` or
    /// any aggregate: `Ptr` is what an unsubstituted generic parameter looks like
    /// by the time it reaches MIR, and taking it for a real layout gave a
    /// monomorphized tuple a slot shaped for neither instantiation.
    ///
    /// Floats are in the set even though float widening isn't implicit yet. The
    /// restriction that mattered was always the `Ptr` exclusion; keying it to
    /// integers was incidental, and left `(f64, f32)` primed to reproduce the
    /// tuple-literal layout bug the moment #624 makes `f32` → `f64` implicit —
    /// 4-and-8 element offsets against a declared 0-and-8 shape, which reads
    /// back as a plausible wrong number rather than a crash (#660).
    /// Whether an array element of this type lives *inline* in its slot.
    ///
    /// By-value aggregates do: the slots are `i * size` apart and the reader
    /// takes the slot address. A `string` doesn't — an array of strings holds
    /// pointers, and the read path depends on that (#414). Scalars need no size
    /// on the store either way.
    fn is_sized_scalar(ty: &MirType) -> bool {
        matches!(
            ty,
            MirType::I8
                | MirType::I16
                | MirType::I32
                | MirType::I64
                | MirType::U8
                | MirType::U16
                | MirType::U32
                | MirType::U64
                | MirType::F32
                | MirType::F64
                | MirType::Bool
                | MirType::Char
        )
    }

    /// A declared return type, unless the declaration is a stdlib stub saying `T`.
    ///
    /// `func_sigs` is seeded from the stub metadata, and a `-> T` was baked into
    /// it as `i64` — so the signature lookup answered with a width the stub
    /// never knew, in front of everything that could have done better. That is
    /// what each per-call-site arm in this file was reaching around. Refusing
    /// the entry here lets the chain fall through to the checker instead
    /// (#1020).
    fn sig_ret_ty(&self, name: &str) -> Option<MirType> {
        let stub_says_t = rask_stdlib::mir_metadata::lookup(name)
            .is_some_and(|m| m.ret_category.names_a_type_param());
        if stub_says_t {
            return None;
        }
        self.func_sigs.get(name).map(|s| s.ret_ty.clone())
    }

    /// The return type of a call once the declared signatures have had their
    /// say — the stdlib's stub metadata, then the checker's own type for the
    /// call expression.
    ///
    /// The checker comes after the stubs because a declared return beats an
    /// inference variable. But it has to be asked at all, and it wasn't: a
    /// method no stub declares ran off the end of the chain into a bare `i64`.
    /// `s.clone()` is exactly that method. Nothing declares `string_clone`, so
    /// the destination was an 8-byte slot, the 16-byte string was half-copied
    /// with no `rc_inc`, and `println` printed the address as a number (#1020).
    ///
    /// When nobody knows, that is a lowering failure naming the site rather
    /// than a slot sized by hope.
    fn call_ret_ty(&self, qualified: &str, node: rask_ast::NodeId) -> MirType {
        super::stdlib_return_mir_type_known(qualified, Some(self.ctx))
            .or_else(|| self.ctx.lookup_node_type(node))
            .unwrap_or_else(|| crate::fallback::i64_fallback("lower/expr:call-return"))
    }

    /// The element type push tracking recorded for this receiver.
    ///
    /// Two maps, always consulted in this order: what this function's own pushes
    /// said, then the cross-function record. They were written out longhand at
    /// eight call sites, which is how arms that answer the same question came to
    /// consult different subsets of it.
    fn tracked_elem_of(&self, object: &Expr) -> Option<MirType> {
        self.tracked_elem_for_key(&Self::vec_tracking_key(object)?)
    }

    /// Same, for a caller that already has the key.
    fn tracked_elem_for_key(&self, key: &str) -> Option<MirType> {
        self.meta(key)
            .and_then(|m| m.elem_type.clone())
            .or_else(|| self.ctx.shared_elem_types.borrow().get(key).cloned())
    }

    /// Does this Vec receiver hold string elements? Drives the dispatch choice
    /// for the runtime entry points that need a real string compare.
    fn vec_elem_is_string(&self, object: &Expr) -> bool {
        self.tracked_elem_of(object)
            // Push tracking only sees a Vec built in this function. A Vec that
            // came back from a call has none, so `p.components().join(",")`
            // took the integer join and printed the bytes of each component as
            // numbers — `97,98,99` for `a,b,c` (#852). The checker knows the
            // return type; ask it when tracking has nothing.
            .or_else(|| self.collection_elem_of_expr(object))
            .map_or(false, |ty| matches!(ty, MirType::String))
    }

    /// The `compare` function for this Vec's element type, when it has one.
    ///
    /// `sort()` is `T: Comparable` (std.collections/SO3), so an element type
    /// that defines or derives `compare` has to be sorted by it. Only
    /// aggregates ask this question — a scalar or a string is sorted by the
    /// type-specific runtime comparator, which is the same order its `compare`
    /// would give and cheaper to reach.
    fn vec_elem_compare_fn(&self, object: &Expr) -> Option<String> {
        let elem = self.tracked_elem_of(object)
            .or_else(|| self.collection_elem_of_expr(object))?;
        if !matches!(elem, MirType::Struct(_) | MirType::Enum(_)) {
            return None;
        }
        let name = format!("{}_compare", self.mir_type_name(&elem)?);
        self.func_sigs.contains_key(&name).then_some(name)
    }

    /// Does this Vec receiver hold floats? Picks the sort that uses the float
    /// total order rather than an integer compare over the bit patterns.
    fn vec_elem_is_float(&self, object: &Expr) -> bool {
        self.tracked_elem_of(object)
            .or_else(|| self.collection_elem_of_expr(object))
            .map_or(false, |ty| matches!(ty, MirType::F64 | MirType::F32))
    }

    /// Wrap a plain value for a struct field declared `T?` or `T or E`.
    /// Returns the operand unchanged when no wrapping is needed — the field
    /// isn't a sum type, the value already has the sum shape, or the option
    /// uses a niche (a `Handle?` is a sentinel, not a tag plus payload).
    ///
    /// The layers themselves come from `coerce_into_wrapper`, shared with
    /// `return` and the annotated bindings. Only the niche `Handle?` is special
    /// here: a bare `none` at that field has to become the sentinel, because the
    /// generic `none` lowering builds a tagged option and storing that into the
    /// field left a tag where the handle belongs (#438).
    pub(super) fn wrap_sum_field_value(
        &mut self,
        field_ty: Option<&MirType>,
        field_niche: Option<i64>,
        val_ty: &MirType,
        val: MirOperand,
    ) -> MirOperand {
        let Some(field_ty) = field_ty else { return val };
        // The declared type is the better witness — it says which niche this is
        // even when `T` has no layout to hang a `Link` on — but the lowered type
        // still answers for the fields no declaration reached.
        let niche = field_niche.or_else(|| crate::lower::mir_niche_none(field_ty));
        if let Some(sentinel) = niche {
            // A source already carrying `Option(Handle)` is already
            // niche-encoded — the same sentinel scheme the field uses — so its
            // operand IS the value to store, real handle or sentinel alike.
            // Only a `none` that slipped through the generic tagged-Option
            // path (its inner type isn't Handle, so the niche check missed it
            // before this field's type was known) needs converting to the
            // sentinel here; storing a real `Handle?` value used to be
            // overwritten by this same branch and silently became `none` (#733).
            if matches!(val_ty, MirType::Option(inner) if !inner.is_niche_payload()) {
                return MirOperand::Constant(MirConst::Int(sentinel));
            }
            return val;
        }
        self.coerce_into_wrapper(
            rask_ast::coercion::CoercionSite::StructField,
            val, val_ty, field_ty,
        )
    }

    /// A collection literal's element, wrapped into the slot it fills.
    ///
    /// Same job `wrap_sum_field_value` does for a struct field — a `T` going
    /// into a `T?` slot acquires the tag — with the niche `Handle<T>?` carved out
    /// the same way, because there the handle *is* the value.
    pub(super) fn wrap_collection_element(
        &mut self,
        elem_ty: &MirType,
        val_ty: &MirType,
        val: MirOperand,
    ) -> MirOperand {
        if let Some(sentinel) = crate::lower::mir_niche_none(elem_ty) {
            // Same carve-out, plus the `none` case: a bare `none` whose type
            // the checker never settled lowers as a tagged option, and a niche
            // slot wants that type's sentinel word instead of its address.
            if matches!(val_ty, MirType::Option(inner) if !inner.is_niche_payload()) {
                return MirOperand::Constant(MirConst::Int(sentinel));
            }
            return val;
        }
        self.coerce_into_wrapper(
            rask_ast::coercion::CoercionSite::CollectionElement,
            val, val_ty, elem_ty,
        )
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
        if let Some(sentinel) = self.option_niche(expr, &option_ty) {
            // The type this operand carries has to be the one `type_to_mir`
            // would give, or the next coercion sees a layer missing and wraps
            // the sentinel: `v.push(none)` into a `Vec<Link<T>?>` built
            // `Some(0)` in a 16-byte slot instead of writing the one word.
            //
            // A link keeps the option spelling; a handle collapses to bare
            // `Handle`, which is what `type_to_mir` still does for it.
            let repr = match &option_ty {
                MirType::Option(inner) if matches!(**inner, MirType::Link(_)) => option_ty.clone(),
                _ => MirType::Handle,
            };
            return Ok((MirOperand::Constant(MirConst::Int(sentinel)), repr));
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

    /// `x.to_int()` on a float and `n.to_float()` on an integer — a single
    /// convert instruction, not a runtime call. Dispatching them by name sent
    /// codegen looking for `f64_to_int`/`i64_to_float`, which no backend has:
    /// the float side died with "Function not found" and the integer side never
    /// even resolved a type prefix.
    fn try_lower_numeric_conversion(
        &mut self,
        method: &str,
        args: &[CallArg],
        obj_op: &MirOperand,
        obj_ty: &MirType,
    ) -> Option<TypedOperand> {
        if !args.is_empty() {
            return None;
        }
        let target = match method {
            "to_int" if matches!(obj_ty, MirType::F32 | MirType::F64) => MirType::I64,
            "to_float" if matches!(
                obj_ty,
                MirType::I8 | MirType::I16 | MirType::I32 | MirType::I64
                | MirType::U8 | MirType::U16 | MirType::U32 | MirType::U64
            ) => MirType::F64,
            _ => return None,
        };
        let dst = self.builder.alloc_temp(target.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst,
            rvalue: MirRValue::Cast { value: obj_op.clone(), target_ty: target.clone() },
        }));
        Some((MirOperand::Local(dst), target))
    }

    /// Resolve a MirType to its named type prefix using struct/enum layouts.
    /// Whether a `!` operand is a `T or E` rather than a `T?`. Decides which
    /// panic message the runtime prints: an error branch that got thrown away
    /// reads nothing like an absent value, and both used to say "None".
    ///
    /// A type it can't read counts as an optional — that's the far commoner
    /// form, and it is what both backends said before either distinguished.
    pub(super) fn forced_operand_was_result(&self, operand: &Expr) -> bool {
        // A `T?` reaches the checker as a `Result` whose error side is `none`,
        // so the error side is what tells the two apart.
        matches!(
            self.ctx.lookup_raw_type(operand.id),
            Some(crate::lower::Type::Result { err, .. }) if **err != crate::lower::Type::None
        )
    }

    /// Emit `panic_forced_error(err.message())` in the panic block of a `r!`.
    /// `false` when there's nothing to call, and the caller falls back to the
    /// message that only says which kind of `!` failed.
    fn lower_forced_error_panic(
        &mut self,
        outer: &Expr,
        inner: &Expr,
        val: &MirOperand,
    ) -> Result<bool, LoweringError> {
        let Some(msg_fn) = self.ctx.call_rewrites.get(&outer.id).cloned() else {
            return Ok(false);
        };
        let Some(crate::lower::Type::Result { err, .. }) = self.ctx.lookup_raw_type(inner.id)
        else {
            return Ok(false);
        };
        let err_ty = self.ctx.type_to_mir(err);
        let size = err_ty.size();
        let payload = self.builder.alloc_temp(err_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: payload,
            rvalue: MirRValue::Field {
                base: val.clone(),
                field_index: 0,
                byte_offset: self.payload_byte_offset(&err_ty),
                access: FieldAccess::for_field(&err_ty, size),
            },
        }));

        // The rewrite carries every body reachability queued for this `!`,
        // joined by `|`. One name is a concrete error type; several is a
        // union, where which member is present isn't known until run time.
        let queued: Vec<&str> = msg_fn.split('|').collect();
        let text = self.builder.alloc_temp(MirType::String);
        if let MirType::Union(members) = err_ty.clone() {
            let Some(arms) = self.union_message_arms(&members, &queued) else {
                return Ok(false);
            };
            // A union discriminates by the member index it carries at offset
            // 0, not by a one-byte tag — the same read `match` does.
            let idx = self.builder.alloc_temp(MirType::I64);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: idx,
                rvalue: MirRValue::Field {
                    base: MirOperand::Local(payload),
                    field_index: 0,
                    byte_offset: Some(crate::types::UNION_MEMBER_OFFSET),
                    access: FieldAccess::Sized(8),
                },
            }));
            let merge = self.builder.create_block();
            let blocks: Vec<crate::BlockId> =
                arms.iter().map(|_| self.builder.create_block()).collect();
            let cases: Vec<(u64, crate::BlockId)> = blocks
                .iter()
                .enumerate()
                .map(|(i, b)| (i as u64, *b))
                .collect();
            let default = blocks.first().copied().unwrap_or(merge);
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Switch {
                value: MirOperand::Local(idx),
                cases,
                default,
            }));
            for (i, (member_ty, fn_name)) in arms.into_iter().enumerate() {
                self.builder.switch_to_block(blocks[i]);
                let member_size = member_ty.size();
                let member = self.builder.alloc_temp(member_ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: member,
                    rvalue: MirRValue::Field {
                        base: MirOperand::Local(payload),
                        field_index: 0,
                        byte_offset: Some(crate::types::UNION_PAYLOAD_OFFSET),
                        access: FieldAccess::for_field(&member_ty, member_size),
                    },
                }));
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: Some(text),
                    func: FunctionRef::internal(fn_name),
                    args: vec![MirOperand::Local(member)],
                }));
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                    target: merge,
                }));
            }
            self.builder.switch_to_block(merge);
        } else {
            let [only] = queued[..] else { return Ok(false) };
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(text),
                func: FunctionRef::internal(only.to_string()),
                args: vec![MirOperand::Local(payload)],
            }));
        }

        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::internal("panic_forced_error".to_string()),
            args: vec![MirOperand::Local(text)],
        }));
        Ok(true)
    }

    /// `(member type, message fn)` per union member, in member-index order.
    /// `None` when any member's `{Name}_message` isn't among the bodies
    /// reachability queued — a switch missing an arm would call a function
    /// nothing emits, which is worse than the generic message it replaces.
    fn union_message_arms(
        &self,
        members: &[MirType],
        queued: &[&str],
    ) -> Option<Vec<(MirType, String)>> {
        members
            .iter()
            .map(|m| {
                let name = format!("{}_message", self.mir_type_name(m)?);
                queued.contains(&name.as_str()).then(|| (m.clone(), name))
            })
            .collect()
    }

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
    pub(super) fn emit_trait_box(
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
        let elem = self.tracked_elem_for_key(&key);
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
            // A slice carries its source's element type, so look through to
            // the container. Without this `parts[2..]` had no tracked element
            // type and `.join()` fell back to the integer runtime, which
            // printed each 16-byte string as the number its bytes spell.
            ExprKind::Index { object: inner, .. } => Self::vec_tracking_key(inner),
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
    /// Does `<receiver>.<method>()` resolve to something that writes through
    /// `self`? Uses the receiver's recorded type to build the qualified name, the
    /// same way dispatch does below.
    fn receiver_method_mutates(&self, object: &Expr, method: &str) -> bool {
        let Some(prefix) = self
            .ctx
            .lookup_raw_type(object.id)
            .and_then(|ty| super::MirContext::type_prefix(ty, self.ctx.type_names))
        else {
            return false;
        };
        let base = prefix.split('<').next().unwrap_or(&prefix);
        self.mutate_self_methods
            .contains(&format!("{}_{}", base, method))
    }

    /// Address of a field/index place, as `base` or `base + offset`. `None` when
    /// the chain isn't a place this can take the address of.
    fn place_address(&mut self, place: &Expr) -> Option<MirOperand> {
        let (base, offset, _, _) = self.lower_place_chain(place)?;
        if offset == 0 {
            return Some(MirOperand::Local(base));
        }
        let tmp = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: tmp,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Add,
                left: MirOperand::Local(base),
                right: MirOperand::Constant(MirConst::Int(offset as i64)),
            },
        }));
        Some(MirOperand::Local(tmp))
    }

    /// Borrow `v[i]` as a pointer into the buffer, for an element about to be
    /// handed to something that writes through it.
    ///
    /// Lowering `v[i]` the ordinary way produces a *copy*: `Vec_index` returns a
    /// pointer into the buffer, but codegen copies those bytes into the
    /// destination's own slot — it has to, because the same lowering serves
    /// `let e = v[i]`, which is a copy by definition. A callee handed that copy
    /// writes into it and the collection never sees it.
    ///
    /// So a `mutate` use borrows the real element instead. The runtime counts
    /// the borrow and panics if anything would move the buffer out from under
    /// it, which is what keeps the pointer from going stale.
    ///
    /// `None` when this isn't a Vec element, or the element type isn't known;
    /// the caller then lowers it the ordinary way.
    fn lower_elem_for_mutate(&mut self, place: &Expr) -> Option<TypedOperand> {
        self.lower_elem_for_mutate_inner(place, true)
    }

    /// The same borrow for a *scalar* element. The aggregate rule doesn't apply
    /// here: a scalar `mutate` parameter is itself a pointer, so the borrowed
    /// address is exactly what the callee wants.
    ///
    /// Without it a scalar element fell through to the spill-a-copy path, and
    /// `bump(mutate arr[0])` left the element alone natively while the
    /// interpreter wrote it back — the same call on a struct field wrote back on
    /// both (#879).
    fn lower_elem_for_mutate_scalar(&mut self, place: &Expr) -> Option<TypedOperand> {
        self.lower_elem_for_mutate_inner(place, false)
    }

    fn lower_elem_for_mutate_inner(
        &mut self,
        place: &Expr,
        require_aggregate: bool,
    ) -> Option<TypedOperand> {
        let ExprKind::Index { object, index } = &place.kind else {
            return None;
        };
        let (borrow, release) = if self.is_vec_expr(object) {
            ("Vec_borrow_elem", "Vec_release_elem")
        } else if self.is_map_expr(object) {
            ("Map_borrow_elem", "Map_release_elem")
        } else {
            return None;
        };
        let elem_ty = self
            .ctx
            .lookup_raw_type(place.id)
            .map(|t| self.ctx.type_to_mir(t))
            .or_else(|| self.collection_elem_of_expr(object))?;
        // On the aggregate path, only aggregates: a scalar element is passed by
        // value there, and handing over its address would have the callee read
        // the pointer as the value.
        if require_aggregate && !crate::lower::stmt::mutate_param_by_pointer(&elem_ty) {
            return None;
        }
        let (coll_op, _) = self.lower_expr(object).ok()?;
        let (idx_op, _) = self.lower_expr(index).ok()?;
        // Ptr, not the element type: an aggregate-typed destination is what
        // makes codegen copy the bytes into a slot, which is the copy being
        // avoided. The callee wants an address either way.
        let ptr = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(ptr),
            func: FunctionRef::internal(borrow.to_string()),
            args: vec![coll_op.clone(), idx_op],
        }));
        self.elem_writebacks.push(super::ElemWriteback::ReleaseBorrow {
            collection: coll_op,
            release,
        });
        Some((MirOperand::Local(ptr), elem_ty))
    }

    /// Settle every `mutate` argument opened since `mark`. Called right after
    /// the call statement, so a borrow covers exactly the call that writes
    /// through it and a spilled scalar is read back before anything else runs.
    fn flush_elem_writebacks(&mut self, mark: usize) {
        for wb in self.elem_writebacks.split_off(mark) {
            match wb {
                super::ElemWriteback::ReleaseBorrow { collection, release } => {
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal(release.to_string()),
                        args: vec![collection],
                    }));
                }
                super::ElemWriteback::ScalarCopyBack { dst, addr } => {
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                        dst,
                        rvalue: MirRValue::Deref(MirOperand::Local(addr)),
                    }));
                }
            }
        }
    }

    fn lower_call_arg(
        &mut self,
        arg: &Expr,
        scalar_mutate: Option<&MirType>,
        aggregate_mutate: bool,
    ) -> Result<TypedOperand, LoweringError> {
        let sty = match scalar_mutate {
            Some(t) => t.clone(),
            None => {
                // An aggregate `mutate` param is supposed to get the caller's
                // storage, but lowering a place as an expression hands over a
                // copy. #702 fixed that for method receivers and left the
                // free-function form — `bump(mutate h.c)` — still writing to a
                // copy nobody reads.
                if aggregate_mutate {
                    // A Vec element has no storage to point at: it was copied
                    // out of the buffer, so it gets a write-back instead.
                    if let Some(r) = self.lower_elem_for_mutate(arg) {
                        return Ok(r);
                    }
                    // A field does have storage — point at it.
                    if matches!(&arg.kind, ExprKind::Field { .. }) {
                        if let Some(addr) = self.place_address(arg) {
                            let ty = self
                                .ctx
                                .lookup_raw_type(arg.id)
                                .map(|t| self.ctx.type_to_mir(t))
                                .unwrap_or(MirType::Ptr);
                            return Ok((addr, ty));
                        }
                    }
                }
                return self.lower_expr(arg);
            }
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
        // A Vec or Map element has no base+offset place to point at — it lives in
        // the collection's own buffer — so borrow it for the length of the call.
        if let Some(borrowed) = self.lower_elem_for_mutate_scalar(arg) {
            return Ok(borrowed);
        }
        // Field/index projection: pass the address of the place so the callee's
        // store lands in the caller's storage.
        if matches!(&arg.kind, ExprKind::Field { .. } | ExprKind::Index { .. }) {
            if let Some((base, offset, _, _)) = self.lower_place_chain(arg) {
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
        // A named variable is the caller's storage and PM2 says the caller sees
        // the write. The spill above is unavoidable — a scalar lives in a
        // register, so there is no address to hand over — so read the slot back
        // into the variable once the call returns. Without it `bump(mutate n)`
        // incremented a copy nobody looked at again (#899).
        if let ExprKind::Ident(name) = &arg.kind {
            if let Some((id, _)) = self.locals.get(name).cloned() {
                self.elem_writebacks.push(super::ElemWriteback::ScalarCopyBack {
                    dst: id,
                    addr,
                });
            }
        }
        Ok((MirOperand::Local(addr), MirType::Ptr))
    }

    pub(super) fn lower_expr(&mut self, expr: &Expr) -> Result<TypedOperand, LoweringError> {
        // ER16a: this is the chain step an enclosing `try` attached to. Lower it,
        // then branch right here — the rest of the chain then works on the
        // payload, which is what `(try read_file(p)).len()` means.
        if let Some((try_id, step)) = self.pending_try_step {
            if step == expr.id {
                self.pending_try_step = None;
                let (op, ty) = self.lower_expr_inner(expr)?;
                return self.emit_try_branch(try_id, expr, op, ty);
            }
        }
        let (op, ty) = self.lower_expr_inner(expr)?;
        // Lowering works each expression's type out as it goes, and lands on
        // `Ptr` — "some address, contents unknown" — whenever it can't. The
        // checker already answered the question; ask it here, once, instead of
        // at each of the sites that would otherwise guess downstream (#725).
        //
        // Only `Ptr` defers. Anything lowering actually determined stays, because
        // lowering knows things the checker doesn't — niche layouts, and the
        // concrete shape a generic took after monomorphization.
        let ty = if matches!(ty, MirType::Ptr) {
            self.ctx.lookup_node_type(expr.id)
                .filter(|t| !matches!(t, MirType::Ptr | MirType::Void))
                .unwrap_or(ty)
        } else {
            ty
        };
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
        // Which expression form we're walking, for the type-coverage report.
        // A `node_types` miss records this, so `RASK_TRACE_TYPE_COVERAGE=1`
        // says which forms the checker doesn't record rather than only how
        // many (#725). No-op unless that variable is set.
        crate::fallback::set_current_kind(rask_ast::expr::expr_kind_name(&expr.kind));
        crate::fallback::set_current_detail(
            &self.parent_name,
            match &expr.kind {
                ExprKind::Ident(name) => name.as_str(),
                ExprKind::Field { field, .. } => field.as_str(),
                ExprKind::MethodCall { method, .. } => method.as_str(),
                _ => "",
            },
        );
        // Cleared when this expression's lowering ends, however it ends, so a
        // lookup from outside any expression is reported as `<outside>` rather
        // than inheriting the last one walked.
        let _kind_scope = crate::fallback::KindScope;
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
                    Some(IntSuffix::Isize) => MirType::isize_ty(),
                    Some(IntSuffix::Usize) => MirType::usize_ty(),
                    Some(IntSuffix::I128) | Some(IntSuffix::I128ByMagnitude) => MirType::I128,
                    Some(IntSuffix::U128) | Some(IntSuffix::U128ByMagnitude) => MirType::U128,
                    // An unsuffixed literal the checker didn't pin down takes the
                    // language's own default rather than counting as a failure to
                    // resolve — type.primitives/L1 says an integer literal
                    // defaults, and i64 holds every value i32 does.
                    None => {
                        // `let x: f64 = 1` — the checker settles the literal as a
                        // float, and the integer filter below dropped that answer
                        // and fell back to i64. An `Int` constant then went into
                        // an f64 slot and Cranelift's verifier hit `unreachable`,
                        // so a three-line program crashed the *compiler*. Take the
                        // checker's answer: the literal is a float, so is the
                        // constant.
                        let settled = self.ctx.lookup_node_type(expr.id);
                        if let Some(float_ty @ (MirType::F32 | MirType::F64)) = settled {
                            return Ok((
                                MirOperand::Constant(MirConst::Float(*val as f64)),
                                float_ty,
                            ));
                        }
                        settled
                            .filter(|t| matches!(t,
                                MirType::I8 | MirType::I16 | MirType::I32 | MirType::I64
                                | MirType::U8 | MirType::U16 | MirType::U32 | MirType::U64
                                | MirType::I128 | MirType::U128))
                            .unwrap_or(MirType::I64)
                    }
                };
                // A literal too big for `i64` gets `U64ByMagnitude` from the
                // lexer — that's about how it was *written*, not what it is. If
                // the checker settled the node at 128 bits, take that: otherwise
                // `let u: u128 = 18446744073709551615` came out a `u64` (#762).
                let ty = if matches!(
                    suffix,
                    None | Some(IntSuffix::U64ByMagnitude) | Some(IntSuffix::I128ByMagnitude)
                ) {
                    match self.ctx.lookup_node_type(expr.id) {
                        Some(wide @ (MirType::I128 | MirType::U128)) => wide,
                        _ => ty,
                    }
                } else {
                    ty
                };
                // The token already holds the literal's own value at 128 bits,
                // so a wide slot just takes it. `MirConst::Int128` is a bit
                // pattern either way, which is what the one range `i128` can't
                // represent — a `u128` above `i128::MAX` — needs (#800).
                let konst = match ty {
                    MirType::I128 | MirType::U128 => MirConst::Int128(*val),
                    // Anything that doesn't fit a narrower slot was already
                    // reported out of range by the checker.
                    _ => MirConst::Int(*val as i64),
                };
                Ok((MirOperand::Constant(konst), ty))
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
                    let narrow_ty = self.ctx.lookup_node_type(expr.id);
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
                } else if let Some((key, meta)) = self.comptime_global_for(name) {
                    // A folded comptime value: a module-level const, or a local in
                    // this function (which is keyed by the function too — see
                    // `comptime_local_key`).
                    let global_local = self.builder.alloc_temp(MirType::Ptr);
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::GlobalRef {
                        dst: global_local,
                        name: key,
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
                        let mir_ty = Self::comptime_global_mir_type(&meta.type_prefix)
                            .unwrap_or(MirType::I64);
                        let result_local = self.builder.alloc_temp(mir_ty.clone());
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                            dst: result_local,
                            rvalue: MirRValue::Deref(MirOperand::Local(global_local)),
                        }));
                        Ok((MirOperand::Local(result_local), mir_ty))
                    }
                } else if let Some(fnval) = self.lower_fn_as_value(name) {
                    // Not a variable — a function's name used as a value.
                    Ok(fnval)
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
                let (right_op, right_ty) = self.lower_expr(right)?;
                let mir_op = lower_binop(*op);
                // `x == v` with a `T?` on one side and a bare `T` on the other:
                // wrap the bare side, which is what every other position does
                // and what the interpreter answers (#834).
                let (left_op, right_op) = if matches!(mir_op, crate::operand::BinOp::Eq | crate::operand::BinOp::Ne) {
                    self.align_optional_compare(left_op, &left_ty, right_op, &right_ty)
                } else {
                    (left_op, right_op)
                };
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
                    // `own expr` heap-allocates (mem.owned) at the point of
                    // evaluation — not just when the value happens to land in a
                    // struct field or enum payload declared `Owned<T>` (#739).
                    // A scalar already fits an `Owned<T>` slot in place (OW7),
                    // so `box_into_owned` leaves it alone; only an aggregate
                    // actually moves to the heap.
                    UnaryOp::Heap => {
                        let boxed = self.box_into_owned(operand_op, &operand_ty);
                        let result_ty = if operand_ty.passed_by_address() {
                            MirType::Ptr
                        } else {
                            operand_ty.clone()
                        };
                        return Ok((boxed, result_ty));
                    }
                    // `*p` on a raw pointer reads exactly the pointee's width.
                    // Plain MIR Deref always took a full word, so `*p` on a
                    // `*u8` handed back four bytes of whatever followed —
                    // "hello" read as 1869376613 instead of the byte 101 —
                    // while `p.read()` next to it was right, because only the
                    // method path passed the pointee size (#696). Both go
                    // through the same call now. Floats and struct pointees
                    // keep the old path: RawPtr_read hands back an integer.
                    UnaryOp::Deref if self.integral_pointee_size(operand).is_some() => {
                        let elem_size = self.integral_pointee_size(operand).unwrap();
                        let result_local = self.builder.alloc_temp(MirType::I64);
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                            dst: Some(result_local),
                            func: FunctionRef::internal(
                                rask_stdlib::ptr_methods::mir_name("read"),
                            ),
                            args: vec![
                                operand_op,
                                MirOperand::Constant(crate::operand::MirConst::Int(elem_size)),
                            ],
                        }));
                        return Ok((MirOperand::Local(result_local), MirType::I64));
                    }
                    // A raw pointer needs the load. An `Owned<T>` doesn't —
                    // it's transparent (OW5), so the checker's type for the
                    // pointee is `T` itself, not a `RawPtr`. Once `own` boxes an
                    // aggregate (#739) its MIR type is `Ptr` too, same as a raw
                    // pointer — the checker's type is what tells them apart. For
                    // an aggregate pointee, the pointer's value already *is* the
                    // address its fields live at (every aggregate is addressed
                    // this way), so deref is a relabel, not a load; a real load
                    // here read whatever address the value's first word happened
                    // to look like (segfault on `(*owned_point).x`), #737.
                    UnaryOp::Deref if matches!(operand_ty, MirType::Ptr) => {
                        let pointee_mir_ty = self.ctx.lookup_raw_type(operand.id)
                            .map(|t| self.ctx.type_to_mir(t));
                        match pointee_mir_ty {
                            Some(ty) if ty.passed_by_address() => (ty, MirRValue::Use(operand_op)),
                            _ => (operand_ty.clone(), MirRValue::Deref(operand_op)),
                        }
                    }
                    UnaryOp::Deref => (operand_ty.clone(), MirRValue::Use(operand_op)),
                    // mem.owned/OW3: `own e` moves the value to the heap and the
                    // pointer is its representation from here on. A scalar needs
                    // no box — it already fits the slot — so `box_into_owned`
                    // hands it straight back, and `drop` on one frees nothing.
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
                let callee_agg_mutate: Vec<bool> = match &func.kind {
                    ExprKind::Ident(name) => {
                        let key = self.ctx.call_rewrites.get(&expr.id).cloned()
                            .unwrap_or_else(|| name.clone());
                        self.func_sigs.get(&key)
                            .map(|s| s.aggregate_mutate_params.clone())
                            .unwrap_or_default()
                    }
                    _ => Vec::new(),
                };
                let wb_mark = self.elem_writebacks.len();
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
                let mut spawn_boxes_result = false;
                for (i, a) in args.iter().enumerate() {
                    let smut = callee_smut.get(i).and_then(|o| o.as_ref());
                    let (op, mir_ty) = if let ExprKind::Closure { params, ret_ty, body, is_own } = &a.expr.kind {
                        let expected = Self::expected_closure_param_tys(&callee_params, i);
                        let lowered = self.lower_closure_expecting(
                            params, ret_ty.as_deref(), body,
                            *is_own || spawns_closure, &expected, Some(a.expr.id),
                            spawns_closure,
                        )?;
                        if spawns_closure {
                            // Tell the runtime whether the word this task hands
                            // back is a box it owns and must free when nobody
                            // joins. The closure lowering just decided; this is
                            // the call that carries it (#963).
                            spawn_boxes_result = self.spawn_result_boxed;
                        }
                        lowered
                    } else {
                        let agg_mut = callee_agg_mutate.get(i).copied().unwrap_or(false);
                        let (op, mir_ty) = self.lower_call_arg(&a.expr, smut, agg_mut)?;
                        // A parameter declared `T?` or `T or E` given a bare `T`
                        // is the same coercion as an annotated binding, so it
                        // takes the same path. Left to codegen it only ever
                        // gained Option layers, and typed the payload one layer
                        // too shallow — an `f32??` parameter arrived as 0 (#637).
                        let declared = callee_params
                            .get(i)
                            .and_then(|o| o.as_ref())
                            .map(|s| self.ctx.resolve_type_str(s));
                        match declared {
                            Some(dst_ty) => {
                                let op = self.coerce_into_wrapper(
                                    rask_ast::coercion::CoercionSite::Argument,
                                    op, &mir_ty, &dst_ty,
                                );
                                (op, mir_ty)
                            }
                            None => (op, mir_ty),
                        }
                    };
                    // TR5 boxing happens in lower_expr, at the value — doing
                    // it again here wrapped the box in another box, and the
                    // outer one had no concrete type to name its vtable after.
                    arg_operands.push(op);
                    arg_mir_types.push(mir_ty);
                }
                if spawns_closure {
                    arg_operands.push(MirOperand::Constant(
                        crate::operand::MirConst::Int(i64::from(spawn_boxes_result)),
                    ));
                    arg_mir_types.push(MirType::I64);
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
                        let ret_ty = self.lookup_expr_type(expr).unwrap_or_else(|| crate::fallback::i64_fallback("lower/expr:885"));
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
                            .unwrap_or_else(|| crate::fallback::i64_fallback("lower/expr:902"));
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

                // mem.owned/OW3: `drop(p)` frees the box `own` allocated. Exactly
                // one box — a type that owns further boxes frees them itself, per
                // OW1/OW2's "consumed exactly once by the program".
                //
                // A scalar `Owned` was never boxed (it fits the slot already), so
                // there is nothing to free and this is where that ends. Anything
                // else lowering can't see as a box is left alone too: freeing a
                // stack address aborts the process, and refusing here would reject
                // shapes linearity should be judging instead.
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
                    let got_ty = arg_mir_types.first().cloned().unwrap_or_else(|| crate::fallback::i64_fallback("lower/expr:957"));
                    let expected_ty = arg_mir_types.get(1).cloned().unwrap_or_else(|| crate::fallback::i64_fallback("lower/expr:958"));

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
                        // f32 reports at its own width — widening to double
                        // round-trips against the wrong one and spells out the
                        // f32's exact binary value instead of `1.1`.
                        let both_f32 = matches!(got_ty, MirType::F32)
                            && matches!(expected_ty, MirType::F32);
                        let want = if both_f32 { MirType::F32 } else { MirType::F64 };
                        let name = if both_f32 {
                            "assert_eq_fail_f32"
                        } else {
                            "assert_eq_fail_f64"
                        };
                        (name, vec![
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

                // drop(ptr) — consume an `Owned<T>` (mem.owned). Whether there's
                // anything to free depends on whether `own` actually boxed: a
                // scalar `T` fits an `Owned<T>` slot in place (OW7) and was never
                // heap-allocated, so freeing it would hand `rask_free` a value
                // that was never a pointer. Only a genuinely-boxed aggregate
                // (MIR type `Ptr`) has a block to release.
                if func_name == "drop" {
                    if matches!(arg_mir_types.first(), Some(MirType::Ptr)) {
                        let arg_op = arg_operands.into_iter().next().unwrap();
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                            dst: None,
                            func: FunctionRef::internal("rask_free".to_string()),
                            args: vec![arg_op],
                        }));
                    }
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
                        let payload_ty = arg_mir_types.first().cloned().unwrap_or_else(|| crate::fallback::i64_fallback("lower/expr:1084"));
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
                    .unwrap_or_else(|| crate::fallback::i64_fallback("lower/expr:1156"));

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
                self.flush_elem_writebacks(wb_mark);

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
                // CT49: inside an unrolled `comptime for field in reflect.fields<T>()`
                // body, `field` isn't a runtime value — it never got a local — so
                // `field.name`/`field.serial_name`/... splice the loop's current
                // FieldInfo directly instead of going through object lowering.
                if let ExprKind::Ident(name) = &object.kind {
                    if let Some(op) = self.comptime_field_const(name, field) {
                        return Ok(op);
                    }
                }

                // AN6/AN8: `field.get<A>().weight` — the attachment's value for
                // one field, spliced. The annotation itself is never built.
                if let Some(op) = self.comptime_annotation_const(expr, object, field)? {
                    return Ok(op);
                }

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

                // Enum variant access: Color.Red (no parens, fieldless variant).
                // `find_enum_written` so `Holder<i64>.Empty` resolves too — the
                // parser folds the written type arguments into the name (#782).
                if let ExprKind::Ident(name) = &object.kind {
                    if !self.locals.contains_key(name) {
                        if let Some((idx, layout)) = self.ctx.find_enum_written(name) {
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
                    // Same access, so the same node — anything the checker
                    // recorded about it stays reachable.
                    let (access_id, access_span) = (expr.id, expr.span);
                    return self.lower_sync_guard_access(box_obj, acquire, release, ret_hint, move |g| Expr {
                        id: access_id,
                        span: access_span,
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

                // A field declared `Owned<T>` holds the *pointer* to its value
                // (#705 put it there), so the read has to load that word. Every
                // other aggregate field lives inline, where base+offset is the
                // answer — and taking the address here handed back the address of
                // the slot instead of what it points to, so `h.inner.v` read the
                // pointer's own bits as the first field (#739).
                //
                // A field's *size* is enough to tell the two apart everywhere
                // except one place: a generic type's shared layout gives every
                // type parameter one word, so a field declared `T` reports 8
                // bytes and an integer type no matter what T is. Fill it with a
                // struct that happens to fit — `Wrap<Wrap<i64>>` — and the write
                // copied the 8 bytes in while the read loaded them as a pointer
                // and dereferenced it. The type at the read site is the one that
                // knows (#871).
                //
                // Struct, enum and tuple only. A `Handle<T>?` field is 8 bytes
                // because it's a niche — the handle *is* the value, `none` is the
                // all-ones sentinel — so its word is the answer, not its address.
                let access = if self.owned_field_is_boxed(object, field) {
                    FieldAccess::Sized(8)
                } else {
                    field_size.map_or(FieldAccess::Word, |size| {
                        let lives_inline = matches!(
                            result_ty,
                            MirType::Struct(_) | MirType::Enum(_) | MirType::Tuple(_)
                        );
                        if size <= 8 && lives_inline {
                            FieldAccess::InPlace(size)
                        } else if size > 8 && !result_ty.passed_by_address() {
                            // A scalar wider than a word — a 128-bit integer is
                            // the only one. It rides in a register pair, so it
                            // comes back loaded; `Sized` reads a size over a
                            // word as "aggregate" and handed back the field's
                            // address, so `ledger.balance` printed a stack
                            // address (#933).
                            FieldAccess::InRegister(size)
                        } else {
                            FieldAccess::Sized(size)
                        }
                    })
                };
                let result_local = self.builder.alloc_temp(result_ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Field {
                        base: obj_op,
                        field_index,
                        byte_offset,
                        access,
                    },
                }));
                Ok((MirOperand::Local(result_local), result_ty))
            }

            // Dynamic field access: value.(expr) — CT49 resolves this to a
            // direct field access once `expr` is comptime-known. See
            // `comptime_field_name` for what counts as known.
            ExprKind::DynamicField { object, field_expr } => {
                let Some(name) = self.comptime_field_name(field_expr)? else {
                    return Err(LoweringError::InvalidConstruct(
                        "the field name in `value.(expr)` has to be known at compile time — \
                         write a string literal, a `comptime { … }` block, or a `comptime for` \
                         binding's `.name`".into()
                    ));
                };
                // Ordinary field lowering answers "field 0" for a name it can't
                // find, which is right nowhere and only safe because the checker
                // rejects `p.nope` long before. A name that arrived through the
                // unroller was never checked against anything, so a typo in
                // `p.(f.name)` would read the wrong field and say nothing. Check
                // it here, where the struct is known.
                self.check_dynamic_field_exists(object, &name)?;
                let synthetic = Expr {
                    id: expr.id,
                    span: expr.span,
                    kind: ExprKind::Field { object: object.clone(), field: name },
                };
                self.lower_expr(&synthetic)
            }

            // Index access
            ExprKind::Index { object, index } => {
                // Range index → slice operation: vec[start..end] or string[start..end]
                if let ExprKind::Range { start, end, inclusive } = &index.kind {
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
                            // `..=` includes its last index, and the runtime
                            // takes a half-open pair. Dropping the flag here
                            // made `s[0..=4]` four bytes on native and five on
                            // the interpreter — the same `Range { .., .. }`
                            // slip that made the E0324 message quote `s[0..4]`
                            // for code that said `s[0..=4]` (#694).
                            self.bump_inclusive_end(op, *inclusive)
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
                        self.bump_inclusive_end(op, *inclusive)
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
                    .or_else(|| self.tracked_elem_of(object))
                    // Last resort: the receiver's own `Vec<T>`. Push tracking
                    // only sees Vecs built in this function, and the checker
                    // doesn't type every index node, so a Vec that arrived some
                    // other way — a field of a struct returned from a call, or of
                    // a `json.decode` result — fell through to i64 and
                    // `h.names[0]` printed a string's first bytes as a number.
                    .or_else(|| self.collection_elem_of_expr(object))
                    .unwrap_or_else(|| crate::fallback::i64_fallback("lower/expr:vec_index_elem"));
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
                    let pool_local = self.as_local(obj_op);
                    let handle_local = self.as_local(idx_op);
                    // `PoolCheckedAccess` hands back the slot's address, always —
                    // that's what the write path needs (`pool[h].field = v`
                    // projects a store onto it) and what an aggregate read wants
                    // anyway, since an aggregate local *is* an address.
                    //
                    // A scalar read needs the value, and it says so here with a
                    // load rather than leaving codegen to work out which of the
                    // two a given destination meant. It used to declare the
                    // destination with the element's own type and let codegen
                    // store an address into it, which is a Cranelift panic
                    // outright: "declared type of variable var10 doesn't match
                    // type of value v13" (#719).
                    if result_ty.passed_by_address() {
                        let slot = self.pool_slot_addr(
                            pool_local,
                            handle_local,
                            result_ty.clone(),
                        );
                        return Ok((MirOperand::Local(slot), result_ty));
                    }
                    let slot_addr =
                        self.pool_slot_addr(pool_local, handle_local, MirType::Ptr);
                    let value_local = self.builder.alloc_temp(result_ty.clone());
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                        dst: value_local,
                        rvalue: MirRValue::Deref(MirOperand::Local(slot_addr)),
                    }));
                    return Ok((MirOperand::Local(value_local), result_ty));
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
                // std.collections: `[1, 2, 3]` *is* a Vec value; it's a fixed
                // array only where the slot it fills says so. The checker records
                // which of the two this literal is, and reading that is the whole
                // rule — built as a stack array regardless, `let xs: Vec<i64> =
                // [1, 2, 3]` handed `xs.len()` a length nobody wrote (#771).
                if let Some(node_ty) = self.ctx.node_types.get(&expr.id).cloned() {
                    if self.generic_head(&node_ty).is_some_and(|(n, _)| n == "Vec") {
                        let elem_hint = self.collection_elem_of_checker_type(&node_ty);
                        return self.lower_vec_from_array_with(elems, elem_hint);
                    }
                }
                // The element type is the checker's, not the first element's.
                // CV1a makes it the type every element fits, so `[small_u8,
                // big_u64]` is a `[u64; 2]` — taking the first element's type
                // laid the array out at one byte per slot and stored the u64
                // truncated (#649).
                let checked_elem = match self.ctx.node_types.get(&expr.id).cloned() {
                    Some(rask_types::Type::Array { elem, .. }) => {
                        Some(self.ctx.type_to_mir(&elem))
                    }
                    _ => None,
                };
                let mut lowered = Vec::new();
                let mut elem_ty = MirType::I32;
                for (i, elem) in elems.iter().enumerate() {
                    let (elem_op, ty) = self.lower_expr(elem)?;
                    if i == 0 {
                        elem_ty = ty.clone();
                    }
                    lowered.push((elem_op, ty));
                }
                if let Some(ty) = checked_elem {
                    elem_ty = ty;
                }
                // A bare `T` filling a `T?` slot gets its layers here, the same
                // way a struct field's does. `[1, none, 3]` in a `[i32?; 3]` used
                // to store the bare 1 where the tag belongs, so the second read
                // came back as the first element's value (#783).
                let lowered: Vec<MirOperand> = lowered
                    .into_iter()
                    .map(|(op, val_ty)| {
                        self.wrap_collection_element(&elem_ty, &val_ty, op)
                    })
                    .collect();
                let elem_size = elem_ty.size();
                let elem_ty_for_store = elem_ty.clone();
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
                        // Two separate questions, and reusing one answer for both
                        // is what broke `[f32; 3]`. `stored_inline_in_array` says
                        // the element occupies its slot and is copied in whole; a
                        // `string` or a niche `Handle<T>?` is a pointer and keeps
                        // the word store. *Width* is the other question: an
                        // element narrower than a word needs a store that narrow,
                        // or each write spills over the elements after it. The
                        // integer cases got away with it because writing in
                        // ascending order overwrites the spill with the right
                        // bytes — an f32 promoted to f64 does not, so `[1.5, 2.5,
                        // 3.5]` read back as zeroes (#902).
                        store_size: if elem_ty_for_store.stored_inline_in_array()
                            || elem_size < 8
                        {
                            Some(elem_size)
                        } else {
                            None
                        },
                    }));
                }
                Ok((MirOperand::Local(result_local), array_ty))
            }

            // Tuple literal
            ExprKind::Tuple(elems) if elems.is_empty() => {
                // `()` is the unit value, and MIR spells unit `Void`. Lowered as
                // an empty tuple it got a local of its own, which every "does
                // this operand carry a value?" test downstream answered yes to:
                // `return ()` out of a `void or E` function with an `ensure`
                // handed the cleanup path a zero and the caller read a Result
                // tag out of address 0.
                let local = self.builder.alloc_temp(MirType::Void);
                Ok((MirOperand::Local(local), MirType::Void))
            }

            ExprKind::Tuple(elems) => {
                let mut elem_types = Vec::new();
                let mut lowered_elems = Vec::new();
                for elem in elems.iter() {
                    let (elem_op, elem_ty) = self.lower_expr(elem)?;
                    lowered_elems.push(elem_op);
                    elem_types.push(elem_ty);
                }
                // CV1a: the tuple is built at the *destination's* layout, not the
                // elements'. `let t: (i64, i32) = (u32_val, u16_val)` type-checks
                // by widening each element, so the slot has to be the annotated
                // shape — built from the element types it packed a u32 at offset 0
                // and a u16 at offset 4, then copied those bytes into an
                // `(i64, i32)` slot whose fields live at 0 and 8. Reading `t.0`
                // took eight bytes spanning both, and printed
                // `60000 << 32 | 3000000000`.
                // Only an integer element is overridden, and only by another
                // integer type. In a generic body the checker's type is the
                // *unsubstituted* parameter — `-> (A, B)` — which reaches MIR as a
                // pointer, and taking that as the layout gave the monomorphized
                // copy of `both<i64, string>` a slot shaped for neither.
                let target_elems = match self.ctx.lookup_node_type(expr.id) {
                    Some(MirType::Tuple(target)) if target.len() == elem_types.len() => Some(target),
                    _ => None,
                };
                let elem_types: Vec<MirType> = match target_elems {
                    None => elem_types,
                    Some(target) => elem_types
                        .into_iter()
                        .zip(target.into_iter())
                        .map(|(got, want)| {
                            if Self::is_sized_scalar(&got) && Self::is_sized_scalar(&want) {
                                want
                            } else {
                                got
                            }
                        })
                        .collect(),
                };
                let tuple_ty = MirType::Tuple(elem_types.clone());
                let result_local = self.builder.alloc_temp(tuple_ty.clone());
                // Widen each element into the slot's type first. Going through an
                // Assign rather than extending in the store keeps one place that
                // knows unsigned widening zero-extends (#326) instead of two.
                let lowered_elems: Vec<MirOperand> = lowered_elems
                    .into_iter()
                    .zip(elem_types.iter())
                    .map(|(op, target_ty)| {
                        let src_ty = match &op {
                            MirOperand::Local(id) => self.builder.local_type(*id),
                            MirOperand::Constant(_) => None,
                        };
                        let coerces = src_ty.as_ref().is_some_and(|s| {
                            s != target_ty && s.size() <= 8 && target_ty.size() <= 8
                        });
                        if !coerces {
                            return op;
                        }
                        let widened = self.builder.alloc_temp(target_ty.clone());
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                            dst: widened,
                            rvalue: MirRValue::Use(op),
                        }));
                        MirOperand::Local(widened)
                    })
                    .collect();
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
                    } else if let Some((idx, sl)) = self.ctx.find_struct_written(name) {
                        (MirType::Struct(StructLayoutId::new(idx, sl.size, sl.align)), Some(sl), None)
                    } else {
                        (MirType::Ptr, None, None)
                    }
                } else if let Some((idx, sl)) = self.ctx.find_struct_written(name) {
                    (MirType::Struct(StructLayoutId::new(idx, sl.size, sl.align)), Some(sl), None)
                } else {
                    (MirType::Ptr, None, None)
                };

                // A generic struct with an inline aggregate type argument has a
                // layout of its own, and the written name doesn't identify it —
                // `One { only: Big { … } }` says only "One". The checker's type for
                // this node carries the instantiation, so that picks the layout
                // (#781).
                let (result_ty, layout) = match self
                    .ctx
                    .generic_instance_struct(self.ctx.lookup_raw_type(expr.id))
                {
                    Some((idx, sl)) => (
                        MirType::Struct(StructLayoutId::new(idx, sl.size, sl.align)),
                        Some(sl),
                    ),
                    None => (result_ty, layout),
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
                    // A generic type gets one layout for every instantiation, laid
                    // out with a word-sized placeholder per type parameter. An
                    // aggregate type argument is bigger than that, so the store
                    // runs past its own slot and the read comes back garbage —
                    // `One { only: Big { … } }` with a 24-byte Big segfaulted,
                    // silently (#781). Refuse instead, at the field whose two
                    // sizes disagree.
                    //
                    // Only when the value is stored *inline*. A field that holds a
                    // reference has a word-sized slot on purpose and its value's
                    // own size says nothing about it:
                    //
                    //   `Owned<T>` heap-allocates just below, so any size fits.
                    //   `Handle<T>?` is a niche — the handle *is* the value and
                    //   `none` is the all-ones sentinel — so it occupies 8 bytes
                    //   even though `Option(Handle).size()` reports 16.
                    //   `Link<T>?` is the same niche, and its `none` is often
                    //   still an untyped tagged option at this point — the
                    //   field's own declared type is what settles it.
                    let stores_a_reference = field_layout
                        .is_some_and(|f| self.owned_payload(&f.ty).is_some())
                        || field_layout.is_some_and(|f| super::is_niche_option_handle(&f.ty))
                        || matches!(&val_ty, MirType::Option(inner) if inner.is_niche_payload())
                        || val_ty.is_niche_payload();
                    if let Some(fl) = field_layout {
                        let value_size = val_ty.size();
                        if !stores_a_reference
                            && val_ty.passed_by_address()
                            && !matches!(val_ty, MirType::String)
                            && value_size > fl.size
                        {
                            // Only a field whose slot came from a substituted
                            // type parameter has the generic explanation. Saying
                            // it either way sent anyone reading it looking for a
                            // generic that isn't there — a plain `[i32; 4]`
                            // field the layout pass couldn't size reported
                            // itself as a generic instantiation problem (#895).
                            let cause = if fl.is_type_param {
                                " — this instantiation is using the shared layout of a \
                                 generic type, which gives every type parameter one word, \
                                 and an aggregate that size doesn't fit. A settled \
                                 instantiation gets a layout of its own; this one wasn't \
                                 settled at the point the layout was picked. Hold the \
                                 aggregate behind a field of its own, or use a concrete \
                                 type here (#781, #814)"
                            } else {
                                ". The layout gave this field a slot too small for the \
                                 value being stored, so the store would run past it into \
                                 the next field. That is a compiler bug, not something \
                                 the program can be rewritten around — please report it \
                                 with this declaration"
                            };
                            return Err(LoweringError::InvalidConstruct(format!(
                                "field `{}` holds {} bytes but its slot is {}{}",
                                field.name, value_size, fl.size, cause
                            )));
                        }
                    }
                    // A `T?` or `T or E` field given a plain `T` has to be
                    // wrapped here. Stored bare, the value landed where the tag
                    // belongs — `Row { name: "bo" }` for a `string?` field put
                    // the string's first word in the tag slot and left the
                    // payload unwritten, so reading it back crashed (#376).
                    let field_mir_ty = field_layout.map(|f| {
                        // A niche field is a sentinel where a tag would be.
                        // `type_to_mir` doesn't always keep the payload inside
                        // the option, so read the declared type — and keep
                        // which niche it is, since a handle's `none` and a
                        // link's `none` are different words.
                        self.ctx.niche_option_mir_type(&f.ty)
                            .unwrap_or_else(|| self.ctx.type_to_mir(&f.ty))
                    });
                    let field_niche = field_layout
                        .and_then(|f| self.ctx.niche_option_sentinel(&f.ty));
                    let val_op = self.wrap_sum_field_value(
                        field_mir_ty.as_ref(), field_niche, &val_ty, val_op,
                    );
                    // A field declared `Owned<T>` given a `T` goes on the heap —
                    // same boundary as an enum payload declared `Owned<T>` (#705).
                    let val_op = match field_layout
                        .and_then(|f| self.owned_payload(&f.ty))
                    {
                        Some(_) => self.box_into_owned_slot(&field.value, val_op, &val_ty),
                        None => val_op,
                    };
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
                else_branch, else_binding } => {
                let (val, val_ty) = self.lower_expr(expr)?;
                let niche = self.option_niche(expr, &val_ty);
                let is_niche = niche.is_some();
                let tag = self.emit_option_tag(&val, niche);

                // Type-context resolution, so `if r is ErrEnum [as e]` against
                // `T or ErrEnum` routes to the err side (tag 1) instead of
                // falling through to 0 like the bare `pattern_tag` does — and a
                // variant of that error enum tests both layers.
                let matches = self.emit_two_layer_pattern_test(&val, &val_ty, tag, is_niche, pattern);

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
                        Some(if err_side { *err.clone() } else { *ok.clone() })
                    } else if err_side {
                        self.err_type_of(expr, &val_ty)
                    } else {
                        self.payload_type_of_niche(expr, &val_ty, is_niche)
                    }
                } else {
                    self.payload_type_of_niche(expr, &val_ty, is_niche)
                };
                self.bind_pattern_payload_niche(pattern, val.clone(), bind_ty, is_niche, &val_ty);
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
                    // ER22: `else as e` binds the branch the test ruled out —
                    // the other side of the same two-branch value.
                    let mut shadowed = None;
                    if let Some(name) = else_binding {
                        let other_ty = match &val_ty {
                            MirType::Result { ok, err } => {
                                let err_side = match pattern {
                                    rask_ast::expr::Pattern::TypePat { ty_name, .. } => {
                                        self.pattern_is_err_side(ty_name, &val_ty)
                                    }
                                    _ => false,
                                };
                                if err_side { (**ok).clone() } else { (**err).clone() }
                            }
                            _ => MirType::I64,
                        };
                        let local = self.builder.alloc_local(name.clone(), other_ty.clone());
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                            dst: local,
                            rvalue: MirRValue::Field {
                                base: val.clone(),
                                field_index: 0,
                                byte_offset: self.payload_byte_offset(&other_ty),
                                access: FieldAccess::Word,
                            },
                        }));
                        if let Some(prefix) = self.mir_type_name(&other_ty) {
                            self.meta_mut(name).type_prefix = Some(prefix);
                        }
                        shadowed = Some((name.clone(), self.locals.insert(name.clone(), (local, other_ty))));
                    }
                    let (else_val, _) = self.lower_expr(else_expr)?;
                    if let Some((name, prev)) = shadowed {
                        match prev {
                            Some(p) => { self.locals.insert(name, p); }
                            None => { self.locals.remove(&name); }
                        }
                    }
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
                let niche = self.option_niche(expr, &val_ty);
                let is_niche = niche.is_some();
                let tag = self.emit_option_tag(&val, niche);

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
                // Demanded, not optional: `emit_option_payload` below extracts the
                // value and needs its real width.
                let payload_ty = self.payload_type_of_niche(expr, &val_ty, is_niche)
                    .unwrap_or_else(|| crate::fallback::i64_fallback("lower/expr:try_else_payload"));
                self.bind_pattern_payload_niche(
                    pattern, val.clone(), Some(payload_ty.clone()), is_niche, &val_ty);
                // Extract the payload value for the result
                let payload = self.emit_option_payload(val, payload_ty.clone(), is_niche);
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

                self.builder.switch_to_block(merge_block);
                Ok((MirOperand::Local(payload), payload_ty))
            }

            // Pattern test (expr is Pattern) — evaluates to bool
            ExprKind::IsPattern { expr: inner, pattern } => {
                let (val, val_ty) = self.lower_expr(inner)?;
                let niche = self.option_niche(inner, &val_ty);
                let is_niche = niche.is_some();
                let tag = self.emit_option_tag(&val, niche);

                let result = self.emit_two_layer_pattern_test(&val, &val_ty, tag, is_niche, pattern);
                Ok((MirOperand::Local(result), MirType::Bool))
            }

            // ER16: `try` propagates; ER17: a block operand propagates per `try`.
            ExprKind::Try { expr: inner } => self.lower_try(expr.id, inner),

            // ER14: `r catch <binder> => <body>`.
            ExprKind::Catch { value, ref clause } => self.lower_catch(expr.id, value, clause),

            // OPT32: hand back what the slot held, and leave `none` in it.
            //
            // Rebuilt branch by branch rather than copied whole: a copy of the
            // option's own storage aliases the slot, and the write-back then
            // overwrites the value that was just read.
            ExprKind::Take { place } => {
                let (val, ty) = self.lower_expr(place)?;
                let niche = self.option_niche(place, &ty);
                let is_niche = niche.is_some();
                let payload_ty = match &ty {
                    MirType::Option(inner) => (**inner).clone(),
                    MirType::Result { ok, .. } => (**ok).clone(),
                    _ => MirType::I64,
                };
                let name = format!("__taken_{}", self.closure_counter);
                self.closure_counter += 1;
                let taken = self.builder.alloc_local(name, ty.clone());

                let tag = self.emit_option_tag(&val, niche);
                let present_block = self.builder.create_block();
                let absent_block = self.builder.create_block();
                let merge_block = self.builder.create_block();
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: MirOperand::Local(tag),
                    then_block: absent_block,
                    else_block: present_block,
                }));

                // Present: assigning the payload into an option-typed local is
                // the wrap, the same way `let x: T? = value` builds one.
                self.builder.switch_to_block(present_block);
                let payload = self.emit_option_payload(val.clone(), payload_ty, is_niche);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: taken,
                    rvalue: MirRValue::Use(MirOperand::Local(payload)),
                }));
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                    target: merge_block,
                }));

                self.builder.switch_to_block(absent_block);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                    addr: taken,
                    offset: 0,
                    value: MirOperand::Constant(MirConst::Int(1)),
                    store_size: None,
                }));
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                    target: merge_block,
                }));

                self.builder.switch_to_block(merge_block);
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
                        op: None,
                    },
                    span: expr.span,
                })?;
                Ok((MirOperand::Local(taken), ty))
            }

            // Presence predicate (postfix ?) — evaluates to bool.
            // Some/Ok tag is 0 (present/ok); None/Err tag is 1.
            ExprKind::IsPresent { expr: inner, .. } => {
                let (val, _ty) = self.lower_expr(inner)?;
                let niche = self.option_niche(inner, &_ty);
                let tag = self.emit_option_tag(&val, niche);
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
                let niche = self.option_niche(inner, &_inner_ty);
                let is_niche = niche.is_some();
                let tag_local = self.emit_option_tag(&val, niche);

                let ok_block = self.builder.create_block();
                let panic_block = self.builder.create_block();
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: MirOperand::Local(tag_local),
                    then_block: panic_block,
                    else_block: ok_block,
                }));

                self.builder.switch_to_block(panic_block);

                // ER15: `!` panics *using* the error's `message()`, and
                // ctrl.panic/F3 wants the message to be a function of the
                // failing operation's operands. The operand here is the error,
                // it's in the slot we're standing on, and every error type has
                // a `message()` — so "was an error" alone threw away the one
                // thing the reader wanted (#1009).
                //
                // Reachability picked the method and queued its body;
                // naming it here instead is how `json.encode` once reached
                // codegen as a function nothing emits.
                if !self.lower_forced_error_panic(expr, inner, &val)? {
                    // No message to reach for: an absent `T?`, or an error
                    // type whose `message()` has no body to instantiate.
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal("panic_unwrap".to_string()),
                        args: vec![MirOperand::Constant(crate::operand::MirConst::Int(
                            self.forced_operand_was_result(inner) as i64,
                        ))],
                    }));
                }
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Unreachable));

                self.builder.switch_to_block(ok_block);
                let payload_ty = self.extract_payload_type(inner)
                    .unwrap_or_else(|| crate::fallback::i64_fallback("lower/expr:2073"));
                let result_local = self.emit_option_payload(val, payload_ty.clone(), is_niche);
                Ok((MirOperand::Local(result_local), payload_ty))
            }

            // Null coalescing (a ?? b)
            ExprKind::NullCoalesce { value, default } => {
                let (val, val_ty) = self.lower_expr(value)?;
                // Nothing to fall back from. `if x? { … x ?? -1 … }` rebinds `x`
                // to the payload inside the block, so by the time `??` sees it
                // the value is a bare `i32` — and reading a tag out of one and
                // dereferencing at that offset segfaulted. The interpreter hands
                // the value straight back, so that is the answer.
                let is_two_branch = matches!(
                    val_ty,
                    MirType::Option(_) | MirType::Result { .. }
                ) || self.option_is_niche(value, &val_ty);
                if !is_two_branch {
                    return Ok((val, val_ty));
                }
                let niche = self.option_niche(value, &val_ty);
                let is_niche = niche.is_some();
                let tag_local = self.emit_option_tag(&val, niche);

                let some_block = self.builder.create_block();
                let none_block = self.builder.create_block();
                let merge_block = self.builder.create_block();

                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: MirOperand::Local(tag_local),
                    then_block: none_block,
                    else_block: some_block,
                }));

                self.builder.switch_to_block(some_block);
                // ER14a: a still-wrapped right side means the chain carries the
                // layer onward, so a present left operand goes back untouched.
                // Only a collapsing `??` reads the payload out.
                let keeps_shape = self.ctx.fallback_keeps_shape.contains(&expr.id);
                // The checker often leaves this node's type an unresolved var,
                // and reading the payload as an opaque pointer hands back the
                // slot's address instead of the value. The lowered receiver
                // knows its own ok type — take that when the checker has
                // nothing better.
                let payload_ty = if keeps_shape {
                    val_ty.clone()
                } else {
                    Self::better_payload_ty(
                        self.extract_payload_type(value),
                        match &val_ty {
                            MirType::Result { ok, .. } => Some((**ok).clone()),
                            MirType::Option(inner) => Some((**inner).clone()),
                            _ => None,
                        },
                    ).unwrap_or_else(|| crate::fallback::i64_fallback("lower/expr:coalesce_payload"))
                };
                let result_local = if keeps_shape {
                    let slot = self.builder.alloc_temp(payload_ty.clone());
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                        dst: slot,
                        rvalue: MirRValue::Use(val),
                    }));
                    slot
                } else {
                    self.emit_option_payload(val, payload_ty.clone(), is_niche)
                };
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
                let niche = self.option_niche(object, &obj_opt_ty);
                let is_niche = niche.is_some();
                let tag_local = self.emit_option_tag(&obj, niche);

                // Resolve the payload struct's layout to find the field's
                // index, type, and offset. Required for the Some-branch
                // load and to size the Option<field_ty> result.
                // Peel off any extra Option layers — the type checker's flatten
                // logic only fires when constraints have already resolved at
                // OptionalField time, so chained `?.` can store an
                // `Option<Option<T>>` raw type for the inner expression. We
                // want the bare T to look up the field on.
                let mut payload_ty = self.extract_payload_type(object)
                    .unwrap_or_else(|| crate::fallback::i64_fallback("lower/expr:2206"));
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
                self.lower_closure(params, ret_ty.as_deref(), body, *is_own, Some(expr.id))
            }

            // Cast
            ExprKind::Cast { expr, ty } => {
                // Trait object boxing: `value as any Trait`
                if let Some(trait_name) = rask_ast::traits::trait_object_name(ty) {
                    let trait_name = trait_name.to_string();
                    let (val, concrete_mir_ty) = self.lower_expr(expr)?;
                    return Ok(self.emit_trait_box(val, &concrete_mir_ty, &trait_name));
                }

                let (val, source_ty) = self.lower_expr(expr)?;
                let target_ty = self.ctx.resolve_type_str(ty);

                // E18: `e as i64` on a fieldless enum extracts the discriminant.
                // An enum value is passed by address, so casting it directly
                // handed back the *address* — 140726462192184 where the tag was
                // wanted, and a different number every run. `& 0xFF` in a wire
                // encoder masks that into a plausible wrong byte, so a packed
                // instruction came out with an arbitrary opcode and nothing
                // crashed (#796). Read the tag the way `.discriminant()` does,
                // then cast that.
                let val = if matches!(source_ty, MirType::Enum(_)) && target_ty.is_int_like() {
                    let tag_local = self.builder.alloc_temp(MirType::U16);
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                        dst: tag_local,
                        rvalue: MirRValue::EnumTag { value: val },
                    }));
                    MirOperand::Local(tag_local)
                } else {
                    val
                };

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
                } else if kind.yields_result(target_ty.is_int_like()) {
                    // CV11/CV14–CV16: anything that can fail yields a result,
                    // so `!`, `try` and `catch` all work on it without the
                    // conversion inventing an error vocabulary of its own.
                    MirType::Result {
                        ok: Box::new(target_ty.clone()),
                        err: Box::new(self.ctx.resolve_type_str("ConvertError")),
                    }
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

            // Using block — bracket the body with the context's install/teardown.
            // The two runtime contexts are independent (conc.async): Multitasking
            // starts the green scheduler, ThreadPool starts a bounded worker
            // pool. They used to emit the same call, so `using ThreadPool` spun
            // up a green scheduler that ThreadPool.spawn never looked at and the
            // `workers:` count was accepted and ignored (#686).
            ExprKind::UsingBlock { name, args, body } => {
                let ctx_fns = match name.as_str() {
                    "Multitasking" | "MultiTasking" | "multitasking" =>
                        Some(("rask_runtime_init", "rask_runtime_shutdown")),
                    "ThreadPool" | "threadpool" =>
                        Some(("rask_threadpool_init", "rask_threadpool_shutdown")),
                    _ => None,
                };
                if let Some((init_fn, shutdown_fn)) = ctx_fns {
                    // Worker count, or 0 for "one per core"
                    let worker_count = if let Some(arg) = args.first() {
                        let (op, _ty) = self.lower_expr(&arg.expr)?;
                        op
                    } else {
                        MirOperand::Constant(crate::operand::MirConst::Int(0))
                    };
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal(init_fn.to_string()),
                        args: vec![worker_count],
                    }));
                    let result = self.lower_block(body);
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal(shutdown_fn.to_string()),
                        args: vec![],
                    }));
                    result
                } else {
                    self.lower_block(body)
                }
            }

            // With-as binding
            ExprKind::WithAs { bindings, body } => {
                // `with shared.read() as d { body }` / `.write()`. Takes the same
                // in-frame acquire/body/release path as Mutex and Cell below.
                // It used to build a closure and hand it to Shared_read, and the
                // closure's return type was hardcoded i64: a block ending in a
                // string printed the pointer as a number, and a block ending in a
                // struct returned the address of a slot in the closure's own
                // frame — dead by the time the caller read it, so
                // `with db.write() as d { Out { id: d.next_id, name: "hi" } }`
                // came back with the id intact and the name gone.
                if bindings.len() == 1 {
                    let binding = &bindings[0];
                    if let ExprKind::MethodCall { object, method, args: call_args, .. } = &binding.source.kind {
                        let is_shared_access =
                            matches!(method.as_str(), "read" | "write" | "staged")
                            && call_args.is_empty();
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
                                // Which lock the block takes is the strategy's
                                // business (SH5) — the verb only says read or
                                // write. A `Local` box takes none. `staged` takes
                                // the exclusive lock either way and binds a copy.
                                let strategy = self.shared_strategy(object);
                                let syms = if method == "staged" {
                                    strategy.staged_syms()
                                } else {
                                    strategy.with_syms(method == "write")
                                };
                                return self.lower_box_with_block(object, &binding.name, body, &syms);
                            }
                        }
                    }
                }

                // Mutex and Cell both bind the payload by address, so both take
                // the acquire/body/write-back path — `with m.lock() as v`,
                // `with m as v`, and `with c as v` alike. Cell's payload used to
                // fall through to the alias binding below, which handed the body
                // the *box* instead of what it holds: `with c as v { v }` on a
                // `Cell<i64>` printed the pointer (#558). CE4 says the binding is
                // the value, and the interpreter always agreed.
                //
                // `is_sync_box_expr` resolves the box from the receiver's type, so
                // a field receiver (`with self.state as s`, straight out of
                // mem.cell) works as well as a bare name. The checks here used to
                // pattern-match `Ident` and silently miss it — the same name-only
                // restriction that hid two pool bugs in #567.
                if bindings.len() == 1 {
                    let binding = &bindings[0];
                    let locked = match &binding.source.kind {
                        ExprKind::MethodCall { object, method, args: call_args, .. }
                            if method == "lock" && call_args.is_empty() => Some(object.as_ref()),
                        _ => None,
                    };
                    // `.lock()` only makes sense on a Mutex; a bare receiver can
                    // be either box.
                    let target = match locked {
                        Some(object) => self
                            .is_sync_box_expr(object, "Mutex")
                            .then_some((object, &BoxWithSyms::MUTEX)),
                        None => {
                            let source = &binding.source;
                            if self.is_sync_box_expr(source, "Mutex") {
                                Some((source, &BoxWithSyms::MUTEX))
                            } else if self.is_sync_box_expr(source, "Cell") {
                                Some((source, &BoxWithSyms::CELL))
                            } else {
                                None
                            }
                        }
                    };
                    if let Some((object, syms)) = target {
                        return self.lower_box_with_block(object, &binding.name, body, syms);
                    }
                }

                // Default: simple alias binding (Pool, Vec/Map element, ...)
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
                    .unwrap_or_else(|| crate::fallback::i64_fallback("lower/expr:2653"));
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
                // The slot holds whatever `break v` puts in it, so its type is
                // the loop's own. Hardcoding I64 meant `break s` on a string
                // stored the pointer and read it back as a number —
                // `let w = loop { break s }` then printed 140728353807216 and
                // compared unequal to every string but itself.
                let result_ty = self
                    .ctx
                    .lookup_node_type(expr.id)
                    .unwrap_or(MirType::I64);
                let result_local = self.builder.alloc_local(
                    "__loop_result".to_string(),
                    result_ty.clone(),
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

                Ok((MirOperand::Local(result_local), result_ty))
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
            ExprKind::Select { arms, is_priority } => self.lower_select(arms, *is_priority),

            // Assert
            ExprKind::Assert { condition, message } => {
                // Detect comparison patterns for smart failure messages.
                // After desugaring, `a == b` → `a.eq(b)`, `a != b` → `!a.eq(b)`.
                let cmp_info = if message.is_none() {
                    extract_assert_comparison(condition)
                } else {
                    None
                };

                if let Some((left_expr, right_expr, op_str)) = cmp_info {
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
                    // Keyed off the lowered types, same as every classifier
                    // below. Guessing from the source shape meant only a written
                    // literal counted as a string, so two string variables went
                    // to the i64 helper and reported their slot addresses (#897).
                    let is_string = matches!(left_ty, MirType::String)
                        || matches!(right_ty, MirType::String);
                    let is_float = matches!(left_ty, MirType::F32 | MirType::F64)
                        || matches!(right_ty, MirType::F32 | MirType::F64);
                    // Both sides f32 keeps the operands at that width. The
                    // shortest decimal that reads back as the same value depends
                    // on the width you check against, so an f32 widened to
                    // double reports 1.100000023841858 for 1.1 — a number no
                    // `println` of the same value would ever print.
                    let is_f32 = matches!(left_ty, MirType::F32)
                        && matches!(right_ty, MirType::F32);
                    let is_char = matches!(left_ty, MirType::Char)
                        && matches!(right_ty, MirType::Char);
                    // A 128-bit comparison gets its own helper: narrowing the
                    // operands to report them would print the wrong numbers,
                    // since the values worth asserting about at that width are
                    // the ones i64 can't hold (#762).
                    let is_i128 = matches!(left_ty, MirType::I128)
                        || matches!(right_ty, MirType::I128);
                    let is_u128 = matches!(left_ty, MirType::U128)
                        || matches!(right_ty, MirType::U128);
                    // Every cmp helper takes a raw scalar, and an optional is a
                    // slot with a present flag next to the payload. Passing the
                    // slot where a number is expected reinterprets its address,
                    // so an optional operand reports without the values (#834).
                    let is_optional = matches!(left_ty, MirType::Option(_))
                        || matches!(right_ty, MirType::Option(_));
                    let fail_fn = if is_string {
                        "assert_fail_cmp_str"
                    } else if is_f32 {
                        "assert_fail_cmp_f32"
                    } else if is_float {
                        "assert_fail_cmp_f64"
                    } else if is_char {
                        "assert_fail_cmp_char"
                    } else if is_u128 {
                        "assert_fail_cmp_u128"
                    } else if is_i128 {
                        "assert_fail_cmp_i128"
                    } else {
                        "assert_fail_cmp_i64"
                    };
                    // Each fail helper has one fixed signature, so a narrower
                    // operand has to be widened to it. An f32 or a char reached
                    // the f64/i64 helper at its own width and Cranelift
                    // rejected the call outright (#332).
                    let (left_op, right_op) = if is_string || is_optional {
                        (left_op, right_op)
                    } else {
                        let want = if is_f32 {
                            MirType::F32
                        } else if is_float {
                            MirType::F64
                        } else if is_u128 {
                            MirType::U128
                        } else if is_i128 {
                            MirType::I128
                        } else {
                            MirType::I64
                        };
                        (
                            self.widen_for_assert_helper(left_op, &left_ty, &want),
                            self.widen_for_assert_helper(right_op, &right_ty, &want),
                        )
                    };
                    if is_optional {
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                            dst: None,
                            func: FunctionRef::internal("assert_fail".to_string()),
                            args: vec![],
                        }));
                    } else {
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                            dst: None,
                            func: FunctionRef::internal(fail_fn.to_string()),
                            args: vec![left_op, right_op, op_const],
                        }));
                    }
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
    // comptime for (CT48–CT54)
    // =================================================================

    /// `object.field` where `object` is an active `comptime for` binding —
    /// splice the loop's current FieldInfo member as a constant. Returns
    /// `None` for anything else (not that binding, or not a FieldInfo member),
    /// so the caller falls through to ordinary field-access lowering.
    fn comptime_field_const(&mut self, object_name: &str, field: &str) -> Option<TypedOperand> {
        let fc = self
            .comptime_for_bindings
            .iter()
            .rev()
            .find(|(name, _)| name == object_name)?
            .1
            .clone();
        Some(match field {
            "name" => (MirOperand::Constant(MirConst::String(fc.name)), MirType::String),
            "type_name" => (MirOperand::Constant(MirConst::String(fc.type_name)), MirType::String),
            "serial_name" => (MirOperand::Constant(MirConst::String(fc.serial_name)), MirType::String),
            "offset" => (MirOperand::Constant(MirConst::Int(fc.offset as i64)), MirType::U64),
            "size" => (MirOperand::Constant(MirConst::Int(fc.size as i64)), MirType::U64),
            "is_public" => (MirOperand::Constant(MirConst::Bool(fc.is_public)), MirType::Bool),
            "is_skipped" => (MirOperand::Constant(MirConst::Bool(fc.is_skipped)), MirType::Bool),
            "has_default" => (MirOperand::Constant(MirConst::Bool(fc.has_default)), MirType::Bool),
            _ => return None,
        })
    }

    /// `binding.has<A>()` where `binding` is an active `comptime for` binding —
    /// answered at compile time from the field's attachments
    /// (type.annotations/AN6). Returns `None` for anything else so the caller
    /// falls through to ordinary method dispatch.
    pub(super) fn comptime_field_method_const(
        &mut self,
        object: &Expr,
        method: &str,
        type_args: &Option<Vec<String>>,
    ) -> Option<TypedOperand> {
        let ExprKind::Ident(object_name) = &object.kind else { return None };
        if method != "has" {
            return None;
        }
        let annotation = type_args.as_ref()?.first()?;
        let fc = &self
            .comptime_for_bindings
            .iter()
            .rev()
            .find(|(name, _)| name == object_name)?
            .1;
        let has = fc.attrs.iter().any(|attr| {
            rask_ast::decl::field_attrs::attachment_name(attr) == annotation
        });
        Some((MirOperand::Constant(MirConst::Bool(has)), MirType::Bool))
    }

    /// `binding.get<A>().field` where `binding` is an active `comptime for`
    /// binding — the attached annotation's value for `field`, spliced as a
    /// constant (type.annotations/AN6). Nothing of the annotation reaches
    /// MIR: there is no annotation value to build (AN8), only this projection.
    ///
    /// Attachment text arrived complete — desugar filled the declared defaults
    /// (`annotation_defaults`) — so this reads what's written and never looks
    /// the declaration up. `Ok(None)` means "not this form", so the caller
    /// falls through to ordinary field-access lowering; `Err` means it *was*
    /// this form and something about it was wrong.
    fn comptime_annotation_const(
        &mut self,
        expr: &Expr,
        object: &Expr,
        field: &str,
    ) -> Result<Option<TypedOperand>, LoweringError> {
        use rask_ast::decl::field_attrs;

        let ExprKind::MethodCall { object: recv, method, type_args, .. } = &object.kind else {
            return Ok(None);
        };
        if method != "get" {
            return Ok(None);
        }
        let ExprKind::Ident(binding) = &recv.kind else { return Ok(None) };
        let Some(annotation) = type_args.as_ref().and_then(|ta| ta.first()) else {
            return Ok(None);
        };
        let Some((_, fc)) = self.comptime_for_bindings.iter().rev().find(|(n, _)| n == binding)
        else {
            return Ok(None);
        };

        let Some(attr) = fc
            .attrs
            .iter()
            .find(|a| field_attrs::attachment_name(a) == annotation.as_str())
        else {
            return Err(LoweringError::InvalidConstruct(format!(
                "`{}` has no `@{}` to read `{}` from — guard the read with `comptime if {}.has<{}>()`",
                fc.name, annotation, field, binding, annotation
            )));
        };
        let Some((_, value)) = field_attrs::attachment_args(attr)
            .into_iter()
            .find(|(name, _)| *name == field)
        else {
            return Err(LoweringError::InvalidConstruct(format!(
                "`@{}` on `{}` has no field `{}`",
                annotation, fc.name, field
            )));
        };
        // The attachment text says `3`, not whether that's an i64, a u8 or an
        // f64 — the declaration does. Not the checker's type for this node:
        // inference has nothing to pin `get<A>()`'s result to when the read
        // feeds an interpolation, and it stayed an open type variable there.
        let ty = self
            .ctx
            .annotation_field_type(annotation, field)
            .ok_or_else(|| LoweringError::InvalidConstruct(format!(
                "`@{}` declares no field `{}`", annotation, field
            )))?;
        annotation_const(value, &ty)
            .map(|c| Some((MirOperand::Constant(c), ty)))
            .ok_or_else(|| LoweringError::InvalidConstruct(format!(
                "`@{}({}: {})` is not a constant this backend can splice",
                annotation, field, value
            )))
    }

    /// Reject `value.(name)` where `name` isn't a field of `value`, when the
    /// struct is knowable here. Silent otherwise — an object whose type this
    /// pass can't pin down is lowered the way it always was.
    fn check_dynamic_field_exists(&self, object: &Expr, name: &str) -> Result<(), LoweringError> {
        let Some(raw) = self.ctx.lookup_raw_type(object.id) else { return Ok(()) };
        let MirType::Struct(StructLayoutId { id, .. }) = self.ctx.type_to_mir(raw) else {
            return Ok(());
        };
        let Some(layout) = self.ctx.struct_layouts.get(id as usize) else { return Ok(()) };
        if layout.fields.iter().any(|f| f.name == name) {
            return Ok(());
        }
        let known = layout
            .fields
            .iter()
            .map(|f| f.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        Err(LoweringError::InvalidConstruct(format!(
            "`{}` has no field `{}` — it has: {}",
            layout.name, name, known
        )))
    }

    /// CT53: the expression in `value.(expr)` must be comptime-known. Every
    /// spelling SYNTAX.md gives the feature folds through here — a bare literal
    /// `p.("x")`, a `comptime { … }` block, a binding whose initializer was one
    /// of those, and a `comptime for` loop binding's string-valued FieldInfo
    /// members (`field.name`, `.serial_name`, `.type_name`).
    ///
    /// `Ok(None)` means "not one of these forms", which the caller turns into
    /// the user-facing "has to be known at compile time" error. `Err` means it
    /// *was* a `comptime` block and evaluating it went wrong — telling someone
    /// who already wrote one to write one is no help, so that error carries the
    /// reason instead. Only the loop-binding arm used to exist, so the literal
    /// form — the one you write to try the feature out, needing neither
    /// `reflect` nor a loop — reached MIR unresolved and failed the build (#930).
    pub(super) fn comptime_field_name(
        &self,
        expr: &Expr,
    ) -> Result<Option<String>, LoweringError> {
        Ok(match &expr.kind {
            ExprKind::String(s) => Some(s.clone()),

            // A name bound earlier in this body to a comptime-known string.
            ExprKind::Ident(name) => self.comptime_strings.get(name).cloned(),

            ExprKind::Comptime { body } => {
                let Some(interp_cell) = self.ctx.comptime_interp.as_ref() else {
                    return Ok(None);
                };
                let mut interp = interp_cell.borrow_mut();
                match interp.eval_block_to_value(body) {
                    Ok(rask_comptime::ComptimeValue::String(s)) => Some(s),
                    // Evaluated fine, just not to a name.
                    Ok(other) => {
                        return Err(LoweringError::InvalidConstruct(format!(
                            "a `comptime` block naming a field has to produce a string — \
                             this one produced {}",
                            other.type_name()
                        )))
                    }
                    // Quota, overflow, a divide by zero — say which.
                    Err(e) => {
                        return Err(LoweringError::InvalidConstruct(format!(
                            "the `comptime` block naming a field didn't finish: {}",
                            e
                        )))
                    }
                }
            }

            // `field.name` inside an unrolled `comptime for` (CT49).
            ExprKind::Field { object, field } => {
                let ExprKind::Ident(name) = &object.kind else { return Ok(None) };
                let Some((_, fc)) =
                    self.comptime_for_bindings.iter().rev().find(|(n, _)| n == name)
                else {
                    return Ok(None);
                };
                match field.as_str() {
                    "name" => Some(fc.name.clone()),
                    "serial_name" => Some(fc.serial_name.clone()),
                    "type_name" => Some(fc.type_name.clone()),
                    _ => None,
                }
            }

            _ => None,
        })
    }

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
        // AN6: `field.has<A>()` on a comptime-for binding is a constant.
        if let Some(r) = self.comptime_field_method_const(object, method, type_args) {
            return Ok(r);
        }
        // `v.freeze()` ends a `comptime` block to say the collection it built
        // is the constant's value. The comptime engine has already evaluated
        // the block by the time this runs, so there is nothing to do but pass
        // the receiver along — and the const's folded bytes are what a reader
        // of the constant actually gets.
        //
        // It's declared `comptime func` with an empty body, so without this it
        // reached codegen as a call to a `Vec_freeze` nothing emits, and every
        // `const X = comptime { … v.freeze() }` failed to compile (#1069).
        if method == "freeze" && args.is_empty() {
            let on_collection = self
                .ctx
                .lookup_raw_type(object.id)
                .and_then(|t| self.generic_head(t))
                .map(|(name, _)| name == "Vec" || name == "Map")
                .unwrap_or(false);
            if on_collection {
                return self.lower_expr(object);
            }
        }
        if let Some(r) = self.try_lower_try_push(expr, object, method, args)? {
            return Ok(r);
        }
        if let Some(r) = self.try_lower_c_namespace_call(expr, object, method, args)? {
            return Ok(r);
        }

        // Iterator terminal methods: .collect(), .fold(), .any(), .all(), etc.
        // Try to recognize an iterator chain on the receiver and fuse it inline.
        if let Some(result) = self.try_lower_iter_terminal(expr, object, method, args)? {
            return Ok(result);
        }

        if let Some(r) = self.try_lower_reflect_call(object, method, type_args)? {
            return Ok(r);
        }

        if let Some(r) = self.try_lower_enum_from_value(object, method, args)? {
            return Ok(r);
        }

        if let Some(r) = self.try_lower_origin(object, method, args)? {
            return Ok(r);
        }

        if let Some(r) = self.try_lower_discriminant(object, method, args)? {
            return Ok(r);
        }

        if let Some(r) = self.try_lower_module_type_method(expr, object, method, args)? {
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
            // The rebuilt call is the same call — same method, same arguments,
            // the guard standing in for the receiver — so it keeps the original
            // node's id and span. A `DUMMY` id here threw away the checker's
            // recorded dispatch target, and `store.lock().create_task(…)` fell
            // back to guessing the receiver's type from the variable name
            // (#425).
            let (call_id, call_span) = (expr.id, expr.span);
            return self.lower_sync_guard_access(box_obj, acquire, release, ret_hint, move |g| Expr {
                id: call_id,
                span: call_span,
                kind: ExprKind::MethodCall {
                    object: Box::new(g),
                    method,
                    type_args: None,
                    args,
                },
            });
        }

        let wb_mark = self.elem_writebacks.len();
        // `v[i].bump()` has the same problem as `bump(mutate v[i])`: the receiver
        // is a copy of the element, so the write never reaches the collection.
        // `place_address` below can't help — a Vec element has no base+offset to
        // point at — so take the copy and write it back after the call.
        let elem_receiver = if self.receiver_method_mutates(object, method) {
            self.lower_elem_for_mutate(object)
        } else {
            None
        };
        let (obj_op, obj_ty) = match elem_receiver {
            Some(r) => r,
            None => self.lower_expr(object)?,
        };

        // A receiver reached through a field or index is a *place*, and lowering
        // it as an expression loads a copy of the aggregate:
        //
        //   _3 = _2.0                 // copy of o.inner
        //   _4 = Inner_bump(_3)       // mutates the copy
        //   _5 = _2.0                 // reloads the original — unchanged
        //
        // So `o.inner.bump()` and `self.lexer.next_token()` mutated something
        // nobody could see afterwards (#702). Point the receiver at the field's
        // real storage instead. An aggregate operand is already a pointer to its
        // bytes, so this doesn't change the representation — only which bytes.
        // Safe for read-only methods too, and it saves the copy.
        // Only for methods that actually write through `self`. Doing it for every
        // aggregate receiver also rewrote read-only calls like
        // `.map(|r| r.view.clone())`, and clone's lowering wants the value it was
        // handed, not an address into someone else's storage.
        let obj_op = match (&object.kind, &obj_ty) {
            (
                ExprKind::Field { .. } | ExprKind::Index { .. },
                MirType::Struct(_) | MirType::Enum(_) | MirType::Tuple(_) | MirType::Array { .. },
            ) if self.receiver_method_mutates(object, method) => {
                self.place_address(object).unwrap_or(obj_op)
            }
            _ => obj_op,
        };

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

        if let Some(r) = self.try_lower_array_intrinsic(method, args, &obj_op, &obj_ty)? {
            return Ok(r);
        }

        if let Some(r) = self.try_lower_trait_object(expr, method, args, &obj_op, &obj_ty)? {
            return Ok(r);
        }

        // Anything left on an array goes to the shared `Vec` lowering, which
        // reads a `RaskVec` header the array doesn't have. Give it a real one.
        let (obj_op, obj_ty) = match self.array_receiver_as_vec(&obj_op, &obj_ty) {
            Some(v) => v,
            None => (obj_op, obj_ty),
        };

        self.lower_regular_method_call(expr, object, method, args, type_args, obj_op, obj_ty, wb_mark)
    }

    /// A `Vec` view over a `[T; N]` receiver, for the methods an array borrows
    /// from `Vec`.
    ///
    /// An array local *is* its buffer — no header, no length word — so handing
    /// it to `Vec_join` or `Vec_contains` made those read the first element as
    /// a `RaskVec` and walk off whatever it spelled (#1021, and #946 before it
    /// for `as_ptr`). Copy the elements into a real vector instead; the
    /// container-drop pass frees it, since `rask_vec_from_static` is one of the
    /// constructors it tracks.
    fn array_receiver_as_vec(
        &mut self,
        obj_op: &MirOperand,
        obj_ty: &MirType,
    ) -> Option<(MirOperand, MirType)> {
        let MirType::Array { elem, len } = obj_ty else {
            return None;
        };
        // A Vec keeps scalars in 8-byte slots — `Vec.new()` declares elem_size
        // 8 and every Vec method reads a whole word per element. An array packs
        // them at their natural stride, so handing the buffer over as-is built a
        // Vec whose stride and readers disagreed: `[1i32, 2i32, 3i32].join(",")`
        // answered `8589934593,12884901890,3`, where 8589934593 is `(2<<32)|1` —
        // two elements read as one. Copy narrow scalars up to the slot width
        // first; `lower_vec_from_array_with` widens for the same reason.
        let (buffer, slot_size) = self.array_buffer_at_vec_stride(obj_op, elem, *len);
        let vec_local = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(vec_local),
            func: FunctionRef::internal("rask_vec_from_static".to_string()),
            args: vec![
                buffer,
                MirOperand::Constant(MirConst::Int(*len as i64)),
                MirOperand::Constant(MirConst::Int(slot_size as i64)),
                MirOperand::Constant(MirConst::Int(crate::elem_strs::tag_of(Some(elem)))),
            ],
        }));
        Some((MirOperand::Local(vec_local), MirType::I64))
    }

    /// The array's elements laid out the way a Vec expects them, and the stride
    /// that describes it.
    ///
    /// Anything already a word or wider is handed over as-is — the strides
    /// agree, so there is nothing to copy. A narrower scalar is read out
    /// element by element into a fresh array of word-sized slots; the length is
    /// a compile-time constant, so this is a fixed sequence rather than a loop.
    fn array_buffer_at_vec_stride(
        &mut self,
        obj_op: &MirOperand,
        elem: &MirType,
        len: u32,
    ) -> (MirOperand, u32) {
        let elem_size = elem.size();
        if elem_size >= 8 || matches!(elem, MirType::Struct(_) | MirType::Enum(_)) {
            return (obj_op.clone(), elem_size);
        }
        // Word-sized slots, so the buffer is `8 * len` bytes and the stores
        // below land where the Vec's readers look.
        let wide_ty = MirType::Array { elem: Box::new(MirType::I64), len };
        let wide = self.builder.alloc_temp(wide_ty);
        for i in 0..len {
            let slot = self.builder.alloc_temp(elem.clone());
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: slot,
                rvalue: MirRValue::ArrayIndex {
                    base: obj_op.clone(),
                    index: MirOperand::Constant(MirConst::Int(i as i64)),
                    elem_size,
                },
            }));
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: wide,
                offset: i * 8,
                value: MirOperand::Local(slot),
                store_size: None,
            }));
        }
        (MirOperand::Local(wide), 8)
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
            // IM3: a transparent alias is the target type, so every prefix this
            // function mangles has to be the target's name. `import time.Duration
            // as Span` reached codegen as `Span_from_millis`, which no function
            // is called — while `d.as_millis()` right after it mangled to
            // `Duration_as_millis`, because an instance call takes its prefix
            // from the receiver's *type* rather than its spelling (#923).
            //
            // Done once here rather than at each mangling site below: the alias
            // is a property of the name, not of which call shape found it.
            let aliased = self
                .ctx
                .type_defs
                .alias_target(name)
                .map(str::to_string);
            let name = match &aliased {
                Some(target) if !self.locals.contains_key(name) => target,
                _ => name,
            };
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
                                .sig_ret_ty(&func_name)
                                .unwrap_or_else(|| self.call_ret_ty(&func_name, expr.id));
                            let result_local = self.builder.alloc_temp(ret_ty.clone());
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                                dst: Some(result_local),
                                func: FunctionRef::internal(func_name),
                                args: arg_operands,
                            }));
                            return Ok(Some((MirOperand::Local(result_local), ret_ty)));
                        }

                        // A folded comptime const is *not* handled here. It
                        // used to be — this arm built the receiver itself and
                        // dispatched `{type_prefix}_{method}` — and that second
                        // implementation of receiver lowering got everything wrong
                        // that the ordinary path gets right: it wrapped scalars in
                        // a Vec, spelled the prefix with Miri's Rust variant names,
                        // and skipped the arithmetic/bit/hash lowering entirely, so
                        // `N + 1` on a comptime const emitted a call to `i64_add`
                        // (#824). Declining leaves the receiver to `lower_expr`,
                        // whose `Ident` arm knows how to read a folded const, and
                        // the rest of the method chain then treats it like any
                        // other value of its type.

                        // Enum variant constructor: Shape.Circle(r)
                        // Extract layout data before mutable borrows in lower_expr.
                        // A generic enum whose type argument is an inline aggregate
                        // has its own layout, and the written name doesn't identify
                        // it — `Holder.Full(Big { … })` says only "Holder". The
                        // checker's type for the call carries the instantiation
                        // (#781).
                        let enum_variant = self
                            .ctx
                            .generic_instance_enum(self.ctx.lookup_raw_type(expr.id))
                            .or_else(|| self.ctx.find_enum_written(name))
                            .and_then(|(idx, layout)| {
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
                                let (val, val_ty) = self.lower_expr(&arg.expr)?;
                                let (offset, field_size) = if i < fields.len() {
                                    (payload_offset + fields[i].offset, fields[i].size)
                                } else {
                                    (payload_offset + (i as u32 * 8), 8)
                                };
                                // A payload declared `Owned<T>` is a pointer slot,
                                // and the value arriving is a `T` — OW5 lets a `T`
                                // stand where `Owned<T>` is asked for. Put it on the
                                // heap and store the pointer. Storing the value
                                // instead wrote 16 bytes of enum into an 8-byte slot,
                                // clobbering the next payload, and the recursive read
                                // came back as a tag used for an address (#705).
                                let val = match fields.get(i).and_then(|f| self.owned_payload(&f.ty)) {
                                    Some(_) => {
                                        self.box_into_owned_slot(&arg.expr, val, &val_ty)
                                    }
                                    None => val,
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
                        // `encode_pretty` differs only in which Rask body
                        // reachability names for a JsonValue; the struct and Vec
                        // paths below are shared.
                        if name == "json"
                            && matches!(method.as_str(), "encode" | "encode_pretty")
                            && args.len() == 1
                        {
                            // The struct and Vec encoders below write compact
                            // text, not a value tree, so the pretty variant came
                            // back identical to `encode` while the interpreter —
                            // which routes a JsonValue through the Rask pretty
                            // printer — indented. Indent the text afterwards
                            // (#847). The JsonValue path is already right: it
                            // picks the pretty body by name.
                            let pretty = method == "encode_pretty";
                            let (arg_op, arg_ty) = self.lower_expr(&args[0].expr)?;
                            if let MirType::Struct(StructLayoutId { id, .. }) = &arg_ty {
                                if let Some(layout) = self.ctx.struct_layouts.get(*id as usize) {
                                    let encoded = self.lower_json_encode_struct(arg_op, layout.clone())?;
                                    return Ok(Some(self.maybe_json_pretty(encoded, pretty)));
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
                                let encoded = self.lower_json_encode_vec(arg_op, elem_ty, elem_mir)?;
                                return Ok(Some(self.maybe_json_pretty(encoded, pretty)));
                            }

                            // A JsonValue has a Rask encoder — `stringify_value`
                            // in stdlib/json.rk, reached through
                            // `JsonValue.to_string`. Falling through to
                            // `json_encode_i64` printed the enum's own address
                            // (#689).
                            //
                            // Which body that is, is reachability's call, not
                            // ours: it recorded the name here and compiled the
                            // body because of the record. Naming it here instead
                            // meant codegen looked up a `JsonValue_to_string`
                            // nobody had queued.
                            if self.mir_type_name(&arg_ty).as_deref() == Some("JsonValue") {
                                let body = self
                                    .ctx
                                    .call_rewrites
                                    .get(&expr.id)
                                    .cloned()
                                    .ok_or_else(|| {
                                        LoweringError::InvalidConstruct(
                                            "json.encode on a JsonValue, but reachability \
                                             queued no encoder for it"
                                                .to_string(),
                                        )
                                    })?;
                                let dst = self.builder.alloc_temp(MirType::String);
                                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                                    dst: Some(dst),
                                    func: FunctionRef::internal(body),
                                    args: vec![arg_op],
                                }));
                                return Ok(Some((MirOperand::Local(dst), MirType::String)));
                            }

                            // A bare enum value, not nested in a struct field
                            // (and not a JsonValue — that's handled above,
                            // through its own Rask encoder). This used to fall
                            // all the way to "Non-struct: string or integer"
                            // below, which has no enum case and silently sent
                            // it through json_encode_i64 — `json.encode(Color.Green)`
                            // printed the address of the enum's stack slot as
                            // a number (std.encoding/E22-E25).
                            if let crate::types::MirType::Enum(crate::types::EnumLayoutId { id, .. }) = &arg_ty {
                                if let Some(layout) = self.ctx.enum_layouts.get(*id as usize).cloned() {
                                    let encoded = self.lower_json_encode_enum(arg_op, &layout)?;
                                    return Ok(Some(self.maybe_json_pretty(encoded, pretty)));
                                }
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
                            let encoded = (MirOperand::Local(result_local), MirType::I64);
                            return Ok(Some(self.maybe_json_pretty(encoded, pretty)));
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
                                    kind: rask_ast::expr::ConvertKind::CheckedOption,
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
                            // Same escape as bare `spawn` (#463): the body runs
                            // after this frame is gone, and the runtime frees the
                            // environment once it finishes. A scope-limited closure
                            // puts that environment on the stack, so the task read a
                            // dead frame and then handed a stack address to free() —
                            // glibc aborted with "free(): invalid size" (#589). The
                            // #463 fix keyed off an `Ident("spawn")` callee, which
                            // `Thread.spawn` never is; it arrives here instead.
                            let spawns_closure = method == "spawn"
                                && (base_name == "Thread" || base_name == "ThreadPool");
                            let mut method_spawn_boxes = false;
                            for (i, arg) in args.iter().enumerate() {
                                // An unannotated closure parameter takes its type
                                // from the callee's declared `func(...)` parameter;
                                // `|req| req.method` on a `func(Request) -> Response`
                                // otherwise defaulted to i64 (#463).
                                let (op, _) = if let ExprKind::Closure { params, ret_ty, body, is_own } = &arg.expr.kind {
                                    let expected = Self::expected_closure_param_tys(&callee_params, i);
                                    let lowered = self.lower_closure_expecting(
                                        params, ret_ty.as_deref(), body,
                                        *is_own || spawns_closure, &expected,
                                        Some(arg.expr.id),
                                        spawns_closure,
                                    )?;
                                    if spawns_closure {
                                        method_spawn_boxes = self.spawn_result_boxed;
                                    }
                                    lowered
                                } else {
                                    self.lower_expr(&arg.expr)?
                                };
                                arg_operands.push(op);
                            }
                            if spawns_closure {
                                // Same handover as the free-function `spawn`:
                                // the runtime frees the box when no join comes.
                                arg_operands.push(MirOperand::Constant(
                                    crate::operand::MirConst::Int(i64::from(method_spawn_boxes)),
                                ));
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
                            if base_name == "Vec"
                                && (method == "new" || method == "with_capacity" || method == "fixed")
                            {
                                let elem_size = self.generic_arg_slot_size(expr.id, 0);
                                let size_op = MirOperand::Constant(MirConst::Int(elem_size));
                                arg_operands.insert(0, size_op);
                                // What the elements are, settled here and kept
                                // by the container for the rest of its life —
                                // see `elem_strs`.
                                let tag = crate::elem_strs::tag_of(
                                    self.container_elem_mir_type(expr.id, 0).as_ref(),
                                );
                                arg_operands.push(MirOperand::Constant(MirConst::Int(tag)));
                            }
                            // Map.new(): inject key_size, val_size
                            if (base_name == "Map") && method == "new" {
                                let key_size = self.generic_arg_slot_size(expr.id, 0);
                                let val_size = self.generic_arg_slot_size(expr.id, 1);
                                arg_operands.insert(0, MirOperand::Constant(MirConst::Int(key_size)));
                                arg_operands.insert(1, MirOperand::Constant(MirConst::Int(val_size)));
                                let key_tag = crate::elem_strs::tag_of(
                                    self.container_elem_mir_type(expr.id, 0).as_ref(),
                                );
                                let val_tag = crate::elem_strs::tag_of(
                                    self.container_elem_mir_type(expr.id, 1).as_ref(),
                                );
                                arg_operands.push(MirOperand::Constant(MirConst::Int(key_tag)));
                                arg_operands.push(MirOperand::Constant(MirConst::Int(val_tag)));
                            }

                            // Map.new() with string keys → use string hash/eq.
                            //
                            // Only the *key* decides. The old test asked whether the
                            // receiver's spelling contained "string" anywhere, so
                            // `Map<Key, string>.new()` — struct key, string value —
                            // picked the string-keyed constructor and the runtime
                            // then read the key's 8 bytes as a char pointer. Lookups
                            // hashed whatever that address held, so an insert and a
                            // later get disagreed at random (#812).
                            let func_name = if func_name == "Map_new" {
                                let from_checker = self.container_elem_mir_type(expr.id, 0);
                                // The spelling still answers when the checker has
                                // nothing for this node: `Map<string, _>.new()` inside
                                // a stdlib body has no recorded type.
                                let from_spelling = super::generic_args_of_str(name)
                                    .and_then(|args| args.first().copied())
                                    .map(|arg| self.ctx.resolve_type_str(arg));
                                let has_string_keys = matches!(from_checker, Some(MirType::String))
                                    || matches!(from_spelling, Some(MirType::String));
                                if has_string_keys {
                                    "Map_new_string_keys".to_string()
                                } else {
                                    func_name
                                }
                            } else {
                                func_name
                            };

                            // A static method on a generic type gets one copy per
                            // instantiation, and monomorphization named the copy —
                            // `Box_new$string`. This path built the callee name from
                            // the source spelling and never asked, so the call went
                            // to a `Box_new` nobody emits (#820).
                            let func_name = self
                                .ctx
                                .call_rewrites
                                .get(&expr.id)
                                .cloned()
                                .unwrap_or(func_name);
                            // `Shared.new/mutex/local` settle the strategy, so
                            // the constructor call resolves to that strategy's
                            // runtime family (conc.sync/SH2).
                            let func_name =
                                self.resolve_shared_strategy_call(&func_name, object);

                            let ret_ty = self
                                .sig_ret_ty(&func_name)
                                .unwrap_or_else(|| self.call_ret_ty(&func_name, expr.id));
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
        // Where the caller's element write-backs start — the receiver may
        // already have queued one before this was reached.
        wb_mark: usize,
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
        // A module-style receiver (`http.serve(…)`) mangles
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
        // #270 write-back applies to methods too. A `mutate` param that isn't
        // passed by pointer already (a Copy scalar, or a stdlib handle like
        // StringBuilder) is registered in the callee as a `ptr`, so the caller
        // has to hand over an address. Only the plain-call path did that, so
        // every `mutate` argument to a *method* passed its value into a
        // parameter typed as a pointer, and the callee dereferenced it:
        //
        //   caller:  Word_render(_3, _1)     // _1 is the 8-byte handle
        //   callee:  Word_render(self, b: ptr) { _2 = _1.0 }
        //
        // which read the handle as the address of one. Natively that was a
        // segfault as soon as any trait rendered into a builder (#693).
        //
        // The qualified name isn't resolved until after the arguments are
        // lowered, so rebuild the candidate keys in the same priority order the
        // resolution below uses and take the first one with a signature.
        let callee_sig = {
            let mut keys: Vec<String> = Vec::new();
            if let Some(prefix) = self.ctx.recorded_prefix(expr.id) {
                keys.push(format!("{}_{}", prefix, method));
            }
            if let Some(prefix) = self
                .ctx
                .lookup_raw_type(object.id)
                .filter(|ty| super::MirContext::stdlib_type_prefix(ty).is_none())
                .and_then(|ty| super::MirContext::type_prefix(ty, self.ctx.type_names))
            {
                keys.push(format!("{}_{}", prefix, method));
            }
            if let ExprKind::Ident(recv) = &object.kind {
                keys.push(format!("{}_{}", recv, method));
            }
            keys.iter().find_map(|k| self.func_sigs.get(k)).cloned()
        };
        // Three things are read off that one signature. They used to be three
        // separate key-building lookups of the same table, in the same order,
        // for the same entry.
        let callee_smut: Vec<Option<MirType>> = callee_sig
            .as_ref()
            .map(|s| s.scalar_mutate_params.clone())
            .unwrap_or_default();
        let callee_agg_mutate: Vec<bool> = callee_sig
            .as_ref()
            .map(|s| s.aggregate_mutate_params.clone())
            .unwrap_or_default();
        // A method parameter declared `T?` or `T or E` given a bare `T` is the
        // same coercion as a free function's, and takes the same path. It didn't
        // used to: only the plain-call path wrapped, so `w.deep(7)` into an
        // `i64??` parameter got codegen's one-layer net and arrived with the
        // inner layer absent, printing -2 where the interpreter printed 7 (#701).
        let callee_params: Vec<Option<String>> = callee_sig
            .as_ref()
            .map(|s| s.param_ty_strs.clone())
            .unwrap_or_default();
        for (i, arg) in args.iter().enumerate() {
            // all_args[0] is the receiver, so callee param i+1 is this argument.
            let smut = callee_smut.get(i + 1).and_then(|o| o.as_ref());
            let agg_mut = callee_agg_mutate.get(i + 1).copied().unwrap_or(false);
            let (op, ty) = if let ExprKind::Closure { params, ret_ty, body, is_own } = &arg.expr.kind {
                let mut expected = Self::expected_closure_param_tys(&tentative_params, i);
                if expected.is_empty() {
                    expected = elem_params.clone();
                }
                self.lower_closure_expecting(params, ret_ty.as_deref(), body, *is_own, &expected, Some(arg.expr.id), false)?
            } else {
                let (op, mir_ty) = self.lower_call_arg(&arg.expr, smut, agg_mut)?;
                let declared = callee_params
                    .get(i + 1)
                    .and_then(|o| o.as_ref())
                    .map(|s| self.ctx.resolve_type_str(s));
                match declared {
                    Some(dst_ty) => {
                        let op = self.coerce_into_wrapper(
                            rask_ast::coercion::CoercionSite::Argument,
                            op,
                            &mir_ty,
                            &dst_ty,
                        );
                        (op, mir_ty)
                    }
                    None => (op, mir_ty),
                }
            };
            all_args.push(op);
            arg_types.push(ty);
        }

        // A union receiver has no single target — the member it holds decides.
        // Switch on the member index and call that member's own method.
        if let MirType::Union(_) = &obj_ty {
            if let Some(handled) =
                self.lower_union_method(expr, &obj_op, &obj_ty, method.as_str(), &all_args)?
            {
                return Ok(handled);
            }
        }

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

        // `to_int` / `to_float` are one machine instruction, so lower them to a
        // Cast instead of hunting for a `f64_to_int` symbol that never existed.
        if let Some(handled) =
            self.try_lower_numeric_conversion(method.as_str(), args, &obj_op, &obj_ty)
        {
            return Ok(handled);
        }

        // Resolve the receiver's stdlib type prefix, then mangle to
        // `{Type}_{method}`. Dispatch is driven by the resolved receiver
        // type and the stub-derived metadata — not a hand-maintained
        // method-name table. Priority:
        // Two sources, both authoritative:
        //
        //   0. what dispatch resolved to (CALL6) — the checker's own answer,
        //      which covers everything the checker saw.
        //   1. a MIR-synthesized local's recorded type. A `store.lock().put(x)`
        //      guard is invented *here*, during lowering — the checker never saw
        //      that node, so there can be no record of it. Lowering writes the
        //      guard's type down where it creates the local and reads it back.
        //
        // Neither guesses from the method's name, which is what the seven
        // deleted steps did. `RASK_TRACE_DISPATCH=1` tallies which one answered:
        // over 914 method calls across the examples, suite, fixtures and
        // compile-error cases, 912 come from the checker and 2 are lock guards.
        let mut prefix_of: Option<String> = None;
        let mut answered_by: &'static str = "9_unresolved";
        macro_rules! step {
            ($name:expr, $e:expr) => {
                if prefix_of.is_none() {
                    let candidate: Option<String> = $e;
                    if candidate.is_some() {
                        answered_by = $name;
                        prefix_of = candidate;
                    }
                }
            };
        }

        step!("0_checker_recorded", recorded_prefix);
        // Nothing else. Eight more steps used to follow: a variable's tracked
        // prefix, the checker's node type for the receiver, a struct field's
        // declared type read off the layout, the receiver type when a stub
        // declared the method, the method's sole defining stdlib type, a
        // name-to-type policy table, the MIR type, and a struct/enum layout name.
        // All eight are gone — each went dead once the real gap behind it closed,
        // and the tally above is how that was established and how it stays true
        // (#425).
        //
        // The last one out was the variable's tracked prefix, and it was holding
        // up four separate gaps in the *checker*, each of which left a binding
        // with no type at all:
        //
        //   - a binding from an `is` test inside a condition (`m is Msg.Text(t)
        //     && t.len() > 1`) — the bindings were computed and discarded
        //   - a tuple `for` binding (`for (k, v) in m`) — each name got a fresh
        //     unconstrained variable rather than its slot in the element tuple
        //   - a struct-shaped enum variant pattern (`Outer.Named { code, kind }`)
        //     — the name isn't a type, so the struct lookup missed and every
        //     field got a fresh variable
        //   - `box.lock().method(…)` — lowering rebuilds the call and used to
        //     stamp it `NodeId::DUMMY`, discarding the record
        //
        // A receiver that resolves to nothing now fails lowering with the method
        // named, which is the same trade `fallback::i64_fallback` makes: a
        // missing answer reported beats a wrong one emitted.

        crate::dispatch_trace::record(answered_by, &method);

        let qualified_name = prefix_of
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

        // `Shared<T, S>` is one type over three runtime families (conc.sync/SH2).
        // The strategy is a type argument, so which family a call lands in is
        // settled here and nothing about the choice survives into the emitted
        // code — a `Local` box calls the no-lock runtime directly.
        let qualified_name = self.resolve_shared_strategy_call(&qualified_name, object);

        // A value going into a container's element slot is an argument position,
        // so it gains wrapper layers the same way any other one does. Nothing
        // did that here: a stdlib method has no `param_ty_strs` to coerce
        // against, so `v.push(1)` on a `Vec<i32?>` stored a bare 1 into a
        // 16-byte `[tag][payload]` slot. Reading it back said absent natively
        // and handed the interpreter a bare i64.
        if let Some((arg_index, type_arg_index)) = match qualified_name.as_str() {
            "Vec_push" | "Vec_remove_item" => Some((0usize, 0usize)),
            "Vec_set" | "Vec_insert" => Some((1, 0)),
            "Map_insert" => Some((1, 1)),
            _ => None,
        } {
            // all_args[0] is the receiver.
            if let (Some(elem_ty), Some(src_ty)) = (
                self.container_elem_mir_type(object.id, type_arg_index),
                arg_types.get(arg_index).cloned(),
            ) {
                if let Some(slot) = all_args.get(arg_index + 1).cloned() {
                    all_args[arg_index + 1] = self.coerce_into_wrapper(
                        rask_ast::coercion::CoercionSite::Argument,
                        slot,
                        &src_ty,
                        &elem_ty,
                    );
                }
            }
        }

        // Track collection element types from push/insert so get returns the right type.
        // Handles both `v.push(x)` and `self.field.push(x)`.
        // Writes to both per-function and shared cross-function maps.
        //
        // Tracking is last-write-wins across every push in the function, so the
        // *declared* element type gets first refusal: `Vec<i32?>` fed `push(1)`,
        // `push(none)`, `push(3)` recorded `i32` because the last bare literal
        // overwrote the option, and reading an element then took a tag out of a
        // bare slot. It crashed on three elements and not two, decided entirely
        // by which push came last.
        if matches!(qualified_name.as_str(), "Vec_push" | "Vec_set" | "Pool_insert") {
            let declared = self.container_elem_mir_type(object.id, 0);
            if let Some(arg_ty) = declared.as_ref().or_else(|| arg_types.first()) {
                if !matches!(arg_ty, MirType::I64) {
                    let arg_ty = arg_ty.clone();
                    if let Some(key) = Self::vec_tracking_key(object) {
                        self.meta_mut(&key).elem_type = Some(arg_ty.clone());
                        self.ctx.record_shared_elem(key, arg_ty);
                    }
                }
            }
        }

        // Both receives take the value through an out-param buffer of the
        // element's real size and hand back the channel's status, which codegen
        // turns into the `T or E` the signature promises. Pass the size.
        //
        // `receive` used to return the payload itself and panic on a closed
        // channel, with a separate `_struct` spelling for elements too big to
        // fit in the return value. One shape covers both, and the error branch
        // works (#1067).
        if matches!(qualified_name.as_str(), "Receiver_receive" | "Receiver_try_receive") {
            let elem_size = self.channel_elem_size(object);
            all_args.push(MirOperand::Constant(MirConst::Int(elem_size)));
        }

        // Use tracked element type for Vec_get/index return instead of default I64.
        // Checks per-function map first, then shared cross-function map.
        //
        // The receiver's declared element type comes first, because tracking is
        // last-write-wins over every push in the function and a `Vec<i32?>` fed
        // `push(1)`, `push(none)`, `push(3)` ended up recorded as `i32`: the
        // final bare literal overwrote the option. Reading an element then took
        // a tag out of a bare i32 slot and the loop segfaulted — on three
        // elements but not two, purely by which push happened to be last.
        let tracked_elem = if matches!(qualified_name.as_str(), "Vec_get" | "Vec_index") {
            self.container_elem_mir_type(object.id, 0)
                .or_else(|| self.tracked_elem_of(object))
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
                .unwrap_or_else(|| crate::fallback::i64_fallback("lower/expr:3750"));
            Some(super::option_of(elem))
        } else if matches!(qualified_name.as_str(), "Vec_first" | "Vec_last") {
            // Same reasoning as Vec_get: these answer `T?`, and the payload type
            // sizes the result slot. From the stub metadata the result came back
            // as a bare `T`, so a `Vec<i64?>` got a 16-byte slot for a 24-byte
            // answer and the tag was read off the wrong bytes.
            let tracked = self.tracked_elem_of(object);
            // In a generic body the checker's payload is still the unresolved
            // type parameter; the receiver's tracked element type is the
            // concrete one after monomorphization.
            let elem = Self::better_payload_ty(self.extract_payload_type(expr), tracked)
                .unwrap_or_else(|| crate::fallback::i64_fallback("lower/expr:3765"));
            Some(super::option_of(elem))
        } else if qualified_name == "Random_choice" {
            // `choice(v)` answers `T?`, and the payload type sizes the slot the
            // DerefOption adapter copies into. From the stub metadata it came
            // back as a bare `T` → i64, so a `Vec<string>` handed back the
            // first eight bytes of the string as a number (#857). The element
            // type is the *argument's*, not the receiver's — the receiver is
            // the generator.
            let elem = args
                .first()
                .and_then(|a| self.collection_elem_of_expr(&a.expr))
                .unwrap_or_else(|| crate::fallback::i64_fallback("lower/expr:random_choice"));
            Some(super::option_of(vec_slot_type(elem)))
        } else if matches!(qualified_name.as_str(),
            "Map_get" | "Map_remove" | "Map_insert")
        {
            // Same reasoning as Vec_get: all three answer `V?`, and the payload
            // type sizes the result slot. The DerefOption adapter copies
            // `slot_size - tag` bytes out of the map's storage, so a bare
            // `i64?` copied only the value's first word — `self.users.get(id)`
            // handed back eight bytes of a `User` and reading a field off it
            // dereferenced the id.
            //
            // `remove` and `insert` were left off this arm, so on a
            // `Map<string, string>` both came back as `i64?`: the payload read
            // took the string's first eight bytes — its inline SSO characters —
            // and codegen dereferenced them as a `RaskStr *` (#903).
            let payload = self.extract_payload_type(expr)
                .or_else(|| self.map_value_mir(object))
                .or_else(|| self.collection_elem_of_expr(object))
                .unwrap_or_else(|| crate::fallback::i64_fallback("lower/expr:map_get_value"));
            Some(super::option_of(payload))
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
            // The receiver's own element type first — `self.items` is a
            // `Vec<string>` once the struct is monomorphized and the layout
            // knows it — then the checker. A `Stack<T>` whose `pop` did
            // `self.items.remove(last)` came out as `let _44: i64` at
            // `T = string`: eight bytes of a string, and no `rc_inc` on the
            // payload either, since those follow the slot's type. It printed
            // fine and segfaulted on the next comparison.
            //
            // This chain used to start with `tracked_elem`, which is only
            // computed for `Vec_get` and `Vec_index` and so was always `None`
            // here — the receiver's layout was doing all the work.
            //
            // Either way the width is rounded up to a word: a Vec keeps scalars
            // in 8-byte slots, and an i32-typed destination read back only half
            // of what the out-param wrote.
            self.collection_elem_of_expr(object)
                .or_else(|| self.ctx.lookup_node_type(expr.id))
                .map(vec_slot_type)
        } else if qualified_name == "Vec_pop" {
            // Same, one level in: `.pop()` is `T?`, and the payload type sizes
            // the slot the DerefOption adapter copies into. A bare `i64?` slot
            // took 8 of a string's 16 bytes and reading it segfaulted.
            //
            // `tracked_elem` was in front of this too, and `None` here for the
            // same reason.
            self.extract_payload_type(expr)
                .map(|elem| super::option_of(vec_slot_type(elem)))
        } else if qualified_name == "Pool_try_insert" {
            // `try_insert` answers `Handle<T>?`, not `T?` — a niche, one word
            // with the all-ones handle for `none`. Without this the local came
            // back as a tagged `i64?` while the checker knew it was a niche, and
            // `r is none` was lowered from whichever of the two the reader
            // consulted (#959-adjacent).
            Some(MirType::Option(Box::new(MirType::Handle)))
        } else if qualified_name == "Pool_get" || qualified_name == "Pool_remove" {
            // Both return T? — extract T from the tracked element type. Without
            // this `remove` answered `i64?` regardless of what the pool held,
            // so reading a struct back out of it dereferenced a field offset
            // into a scalar (#356).
            let elem_ty = self.tracked_elem_of(object)
                // Then whatever the receiver's own type says it holds — a
                // `Vec<T>`/`Pool<T>` element or a `Map<K, V>` value, resolved or
                // not. This used to read the first generic arg of an
                // `UnresolvedGeneric` only, so a resolved type or a Map missed.
                .or_else(|| self.collection_elem_of_expr(object))
                .unwrap_or_else(|| crate::fallback::i64_fallback("lower/expr:vec_get_elem"));
            Some(super::option_of(elem_ty))
        } else if matches!(qualified_name.as_str(), "Rack_insert" | "Rack_corresponding") {
            // Both hand back a link. The stub says `Link<T>`, which reaches MIR
            // without `T`'s layout attached — and a link without its layout
            // can't project a field, so `rack.insert(n).health` had nothing to
            // offset from. The call site knows what `T` became.
            // `insert` answers a bare `Link<T>`; `corresponding` answers
            // `Link<T>?`, which is the same word with the null address for
            // `none`. Accept both spellings — filtering to the bare one dropped
            // the return type for `corresponding` entirely, and its `?` test
            // then read a tag that isn't there.
            self.ctx.lookup_node_type(expr.id).filter(|t| t.is_link_slot())
        } else if matches!(qualified_name.as_str(),
            "Receiver_receive" | "Receiver_try_receive")
        {
            // The stub's `T or E` doesn't survive to MIR — neither side has a
            // name by the time it gets here, so the fallback typed the result a
            // bare i64. `r?` then read a tag off a local that never got a Result
            // slot and every receive looked like a failure (#463); for
            // `try_receive` the ok/err routing ran out of type identities and
            // fell through to a capitalization guess that sent the success case
            // to the error tag, so the ok branch ran on an empty channel and
            // read a payload nothing had written (#631).
            let stub = qualified_name.as_str();
            // The checker's answer has to be a wrapper — a `Result` or an
            // `Option`. The nested `!matches!(**ok, MirType::Ptr)` this used to
            // carry was the same guess at "the ok side is still `T`" the rest of
            // the file made; `lookup_node_type` refuses a real type parameter
            // and keeps a channel of `Vec`s, which is legitimately `Ptr`
            // inside.
            self.ctx.lookup_node_type(expr.id)
                .filter(|t| matches!(t, MirType::Result { .. } | MirType::Option(_)))
                .or_else(|| self.func_sigs.get(stub).map(|s| s.ret_ty.clone()))
                .or_else(|| Some(super::stdlib_return_mir_type_in(stub, Some(self.ctx))))
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
            // Qualified first. `Type_method` names exactly one function;
            // the bare method name is whatever else in the program shares it,
            // so consulting it first let an unrelated `join` answer for
            // `ThreadHandle_join`.
            .sig_ret_ty(&qualified_name)
            .or_else(|| self.sig_ret_ty(&method))
            .unwrap_or_else(|| self.call_ret_ty(&qualified_name, expr.id)));

        // A method on a generic type is lowered once, so its signature says `T`
        // — which reaches MIR as a bare `Ptr`. The call site knows what `T`
        // became: `Box<string>.get()` returning `Ptr` meant the caller printed
        // the string's address as a number (#272). That substitution now happens
        // for every expression kind on the way out of `lower_expr`, not just here.

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

        // Built before the chain below: emitting the wrapper mutates the
        // builder, and the arms are an expression.
        let sort_comparator = if qualified_name == "Vec_sort"
            && !self.vec_elem_is_string(object)
            && !self.vec_elem_is_float(object)
        {
            self.vec_elem_compare_fn(object)
                .and_then(|name| self.lower_compare_as_comparator(&name))
                .map(|(op, _)| op)
        } else {
            None
        };

        // Pool.alloc(value) → Pool_insert(pool, elem_ptr)
        // Pool_alloc takes no element arg; codegen Pool_insert appends elem_size
        let (final_name, final_args) = if qualified_name.starts_with("string_parse")
            && Self::parse_variant_for_slot(&ret_ty).is_some()
        {
            // The parse variant has to agree with the slot it writes into. The
            // name comes from the turbofish and the slot from inference, and
            // those can disagree: `"2.25".parse<f32>() ?? -1.0` with no
            // annotation is an `f64 or E` slot (the fallback literal is f64),
            // so `string_parse_f32` wrote four bytes where eight were read.
            // The slot's own payload type decides.
            //
            // Integers get the same treatment, and it isn't only about width:
            // each narrow variant range-checks against its own type, so
            // `let a: u8 = "300".parse()` is `ParseError.OutOfRange` instead of
            // 44 (native) or 300 in a u8 (interp) (#919). Without this the
            // inferred call kept the bare 64-bit parse.
            let by_slot = Self::parse_variant_for_slot(&ret_ty)
                .expect("checked just above");
            (by_slot.to_string(), all_args)
        } else if qualified_name == "Pool_alloc" && all_args.len() == 2 {
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
        } else if qualified_name == "Vec_sort" && self.vec_elem_is_float(object) {
            // The default sort compares elements as integers, which puts
            // -1.5 before -2.5 and a NaN wherever its sign bit lands.
            ("Vec_sort_f64".to_string(), all_args)
        } else if qualified_name == "Vec_sort" && self.vec_elem_is_string(object) {
            // Same problem, different type: as integers a string compares by
            // its inline bytes or its heap pointer, so `["pear", "apple"]`
            // came back in whatever order the allocator produced.
            ("Vec_sort_str".to_string(), all_args)
        } else if qualified_name == "Vec_sort" && sort_comparator.is_some() {
            // An aggregate with a `compare`: sort by it. The default runtime
            // comparator reads the first eight bytes, which for a struct is
            // whichever field landed there — `sort()` on a `{name, rank}`
            // ordered by name however the type's own `compare` was written.
            //
            // Going through `sort_by` also picks up its stable merge sort,
            // which is what SO1 asks for and what matters here: a comparator
            // that reads one field of several has ties, and ties are where
            // stability is observable.
            let mut args = all_args;
            args.push(sort_comparator.expect("checked above"));
            ("Vec_sort_by".to_string(), args)
        } else if qualified_name == "Vec_contains" && self.vec_elem_is_string(object) {
            // The byte-compare runtime can't match two equal heap strings —
            // they hold different pointers. Route strings to a real compare.
            ("Vec_contains_str".to_string(), all_args)
        } else {
            (qualified_name.clone(), all_args)
        };

        let result_local = self.builder.alloc_temp(ret_ty.clone());
        let container_edge = self.container_edge_call(&final_name, &final_args);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(result_local),
            func: FunctionRef::internal(final_name.clone()),
            args: final_args,
        }));
        // A link put into a container is an edge like any other, so the target
        // has to learn about it (mem.racks/RK3). The record names the container
        // rather than a position — a push or a rehash moves entries around, and
        // a record naming an index would be wrong by the next call.
        if let Some((func, args)) = container_edge {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::internal(func.to_string()),
                args,
            }));
        }
        self.flush_elem_writebacks(wb_mark);

        // An `f32` receiver dispatches to the `f64` symbol — there's one `sqrt`
        // in libm and one entry per method in the table — so the argument is
        // promoted on the way in and the answer comes back at double width.
        // Handing that straight back left `(2.0 as f32).sqrt()` as
        // 1.4142135623730951 while the interpreter, which computes at the
        // receiver's own width, said 1.4142135. Round the answer back to the
        // width the method was called at (#844).
        if matches!(obj_ty, MirType::F32)
            && matches!(ret_ty, MirType::F64)
            && final_name.starts_with("f64_")
        {
            let narrowed = self.builder.alloc_temp(MirType::F32);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: narrowed,
                rvalue: MirRValue::Cast {
                    value: MirOperand::Local(result_local),
                    target_ty: MirType::F32,
                },
            }));
            return Ok((MirOperand::Local(narrowed), MirType::F32));
        }

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

    /// Rewrite a `Shared_*` call to the runtime family its strategy names.
    ///
    /// Constructors pick the strategy by their own name — `Shared.new` is
    /// `Shared.mutex` and `Shared.local` say which lock they want; `new` takes
    /// the default.
    /// Everything else reads it off the receiver's type.
    fn resolve_shared_strategy_call(&self, qualified: &str, object: &Expr) -> String {
        use super::concurrency::SharedStrategy;
        let Some(rest) = qualified.strip_prefix("Shared_") else {
            return qualified.to_string();
        };
        let strategy = match rest {
            "new" => SharedStrategy::Readers,
            "mutex" => SharedStrategy::Mutex,
            "local" => SharedStrategy::Local,
            _ => self.shared_strategy(object),
        };
        if matches!(rest, "new" | "mutex" | "local") {
            return format!("{}_new", strategy.prefix());
        }
        let name = match (strategy, rest) {
            // A `Local` box takes nothing, so both verbs are the same call: the
            // slot's address. `read` versus `write` is intent the reader can
            // see, not a different operation here.
            (SharedStrategy::Local, "read" | "write" | "try_read" | "try_write") => "Cell_acquire",
            (SharedStrategy::Local, "get") => "Cell_get",
            (SharedStrategy::Local, "set") => "Cell_set",
            (SharedStrategy::Local, "replace") => "Cell_replace",
            (SharedStrategy::Local, "into_inner") => "Cell_into_inner",
            // A plain lock has one mode, so a `read()` under it takes the
            // exclusive lock — slower than `Readers` would be there, never wrong
            // (SH5).
            (SharedStrategy::Mutex, "read" | "write") => "Mutex_lock",
            (SharedStrategy::Mutex, "try_read" | "try_write") => "Mutex_try_lock",
            (SharedStrategy::Mutex, "clone") => "Mutex_clone",
            // `get`/`set`/`replace` exist under every strategy (CE6), not just
            // the one that needs no lock. Left unmapped, `Shared.new(5).get()`
            // type-checked and then failed to link on `Shared_get`.
            (SharedStrategy::Mutex, "get") => "Mutex_get",
            (SharedStrategy::Mutex, "set") => "Mutex_set",
            (SharedStrategy::Mutex, "replace" | "into_inner") => "Mutex_replace",
            (SharedStrategy::Readers, "get") => "Shared_get",
            (SharedStrategy::Readers, "set") => "Shared_set",
            (SharedStrategy::Readers, "replace" | "into_inner") => "Shared_replace",
            _ => return qualified.to_string(),
        };
        name.to_string()
    }

    /// The edge-registration call a container mutator needs, if its value is a
    /// link. `None` for everything else, which is almost every call.
    fn container_edge_call(
        &self,
        name: &str,
        args: &[MirOperand],
    ) -> Option<(&'static str, Vec<MirOperand>)> {
        let (value_index, func) = match name {
            "Vec_push" => (1, "Link_register_element"),
            "Vec_set" | "Vec_insert_at" => (2, "Link_register_element"),
            "Map_insert" => (2, "Link_register_entry"),
            _ => return None,
        };
        let value = args.get(value_index)?;
        let container = args.first()?;
        let is_link = match value {
            MirOperand::Local(id) => {
                matches!(self.builder.local_type(*id), Some(MirType::Link(_)))
            }
            _ => false,
        };
        is_link.then(|| (func, vec![container.clone(), value.clone()]))
    }

    /// `v.try_push(x)` — lowered here rather than called, because the element
    /// can't cross a function boundary generically.
    ///
    /// The stdlib body reads `if self.is_full() { return GrowError.Full(value) }`
    /// then `self.push(value)`, and that is exactly what this emits. Compiling
    /// it as an ordinary function instead gives one `Vec_try_push` shared by
    /// every element type, so `value: T` has to be a pointer — and a `Vec<i32>`
    /// then pushed the address of a register while a `Vec<string>` pushed a
    /// pointer to a pointer. Stdlib generic bodies aren't monomorphized per
    /// element type; until they are, the call site is the only place that knows
    /// what `T` is. Same shape on the interpreter, which keeps its own builtin.
    fn try_lower_try_push(
        &mut self,
        expr: &Expr,
        object: &Expr,
        method: &String,
        args: &[CallArg],
    ) -> Result<Option<TypedOperand>, LoweringError> {
        if method != "try_push" || args.len() != 1 {
            return Ok(None);
        }
        let (recv, recv_ty) = self.lower_expr(object)?;
        // A Vec is a runtime pointer; anything else with a `try_push` isn't ours.
        if !matches!(recv_ty, MirType::Ptr) {
            return Ok(None);
        }
        // `void or GrowError<T>` — the err side names the enum whose `Full`
        // variant carries the rejected element.
        let Some(result_ty @ MirType::Result { .. }) = self.ctx.lookup_node_type(expr.id) else {
            return Ok(None);
        };
        let MirType::Result { err, .. } = &result_ty else { return Ok(None) };
        let MirType::Enum(crate::types::EnumLayoutId { id: enum_id, .. }) = err.as_ref() else {
            return Ok(None);
        };
        let Some(layout) = self.ctx.enum_layouts.get(*enum_id as usize) else {
            return Ok(None);
        };
        let Some(full) = layout.variants.iter().find(|v| v.name == "Full") else {
            return Ok(None);
        };
        let (full_tag, payload_offset) = (full.tag, full.payload_offset);
        let err_size = err.size();
        let tag_offset = layout.tag_offset as u32;

        let (value, value_ty) = self.lower_expr(&args[0].expr)?;
        let value_size = value_ty.size();

        let full_local = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(full_local),
            func: FunctionRef::internal("Vec_is_full".to_string()),
            args: vec![recv.clone()],
        }));

        let result_local = self.builder.alloc_temp(result_ty.clone());
        let full_block = self.builder.create_block();
        let push_block = self.builder.create_block();
        let merge_block = self.builder.create_block();
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(full_local),
            then_block: full_block,
            else_block: push_block,
        }));

        // At the bound: build GrowError.Full(value) and hand it back as the
        // error branch. Nothing is pushed, so the caller still owns the value.
        self.builder.switch_to_block(full_block);
        let err_local = self.builder.alloc_temp((**err).clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: err_local,
            offset: tag_offset,
            value: MirOperand::Constant(MirConst::Int(full_tag as i64)),
            store_size: None,
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: err_local,
            offset: payload_offset,
            value: value.clone(),
            store_size: if value_size > 8 { Some(value_size) } else { None },
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result_local,
            offset: crate::types::RESULT_TAG_OFFSET,
            value: MirOperand::Constant(MirConst::Int(1)),
            store_size: None,
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result_local,
            offset: crate::types::RESULT_PAYLOAD_OFFSET,
            value: MirOperand::Local(err_local),
            store_size: if err_size > 8 { Some(err_size) } else { None },
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: merge_block,
        }));

        // Room left: an ordinary push, and the ok branch carries nothing.
        self.builder.switch_to_block(push_block);
        let push_ret = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(push_ret),
            func: FunctionRef::internal("Vec_push".to_string()),
            args: vec![recv, value],
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result_local,
            offset: crate::types::RESULT_TAG_OFFSET,
            value: MirOperand::Constant(MirConst::Int(0)),
            store_size: None,
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: merge_block,
        }));

        self.builder.switch_to_block(merge_block);
        Ok(Some((MirOperand::Local(result_local), result_ty)))
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
                    let ret_ty = self.lookup_expr_type(expr).unwrap_or_else(|| crate::fallback::i64_fallback("lower/expr:4037"));
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

    /// `Enum.from_value(n)` on a fieldless enum → `Enum?`.
    ///
    /// E18: auto-generated for every fieldless enum, `none` when the number
    /// isn't a discriminant. The checker implements it and the interpreter runs
    /// it; native had no lowering, so a program using it type-checked, ran on
    /// one backend, and failed codegen on the other with "Function not found:
    /// Colour_from_value" (#795).
    ///
    /// A fieldless enum value *is* its discriminant, so the construction is:
    /// test `n` against each variant's tag, and on a hit store `n` into the
    /// option's payload. No per-variant construction needed.
    fn try_lower_enum_from_value(
        &mut self,
        object: &Expr,
        method: &str,
        args: &[CallArg],
    ) -> Result<Option<TypedOperand>, LoweringError> {
        if method != "from_value" || args.len() != 1 {
            return Ok(None);
        }
        let ExprKind::Ident(enum_name) = &object.kind else {
            return Ok(None);
        };
        if self.locals.contains_key(enum_name) {
            return Ok(None);
        }
        let Some((layout_id, layout)) = self.ctx.find_enum(enum_name) else {
            return Ok(None);
        };
        if !layout.variants.iter().all(|v| v.fields.is_empty()) {
            return Ok(None);
        }
        let tags: Vec<u64> = layout.variants.iter().map(|v| v.tag).collect();
        let enum_ty = MirType::Enum(crate::types::EnumLayoutId {
            id: layout_id,
            byte_size: layout.size,
            align: layout.align,
        });
        let opt_ty = MirType::Option(Box::new(enum_ty));

        let (n_op, _) = self.lower_expr(&args[0].expr)?;

        let slot = self.builder.alloc_temp(opt_ty.clone());
        let some_block = self.builder.create_block();
        let none_block = self.builder.create_block();
        let done_block = self.builder.create_block();

        // One case per discriminant. A switch rather than a chain of compares
        // so the cost doesn't grow with the enum — raido's Opcode has 42.
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Switch {
            value: n_op.clone(),
            cases: tags.iter().map(|t| (*t, some_block)).collect(),
            default: none_block,
        }));

        self.builder.switch_to_block(some_block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: slot,
            offset: rask_mono::abi::OPTION_TAG_OFFSET,
            value: MirOperand::Constant(crate::operand::MirConst::Int(0)),
            store_size: Some(8),
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: slot,
            offset: rask_mono::abi::OPTION_PAYLOAD_OFFSET,
            value: n_op,
            store_size: Some(8),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: done_block,
        }));

        self.builder.switch_to_block(none_block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: slot,
            offset: rask_mono::abi::OPTION_TAG_OFFSET,
            value: MirOperand::Constant(crate::operand::MirConst::Int(1)),
            store_size: Some(8),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: done_block,
        }));

        self.builder.switch_to_block(done_block);
        Ok(Some((MirOperand::Local(slot), opt_ty)))
    }

    /// `a.mod(b)` — the Euclidean remainder (type.operators/AR3).
    ///
    /// `%` takes the dividend's sign (AR2), so `-1 % 10` is `-1` and every ring
    /// buffer and calendar calculation writes `((a % n) + n) % n` by hand. This
    /// is that expression with a name, and with each operand evaluated once —
    /// the hand-written form evaluates both twice, so `i.mod(next())` would
    /// call `next()` twice if this were a desugar.
    ///
    /// Lowered as `r = a % b` then a branch, rather than a new MIR operator:
    /// `Mod` and `Add` already exist and codegen already knows both widths and
    /// both signednesses for them.
    fn lower_int_mod(
        &mut self,
        lhs: &MirOperand,
        rhs: MirOperand,
        ty: &MirType,
    ) -> Result<TypedOperand, LoweringError> {
        let result = self.builder.alloc_temp(ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: result,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Mod,
                left: lhs.clone(),
                right: rhs.clone(),
            },
        }));

        // An unsigned remainder is already in range, so `mod` and `%` coincide
        // and there is nothing to correct.
        if ty.is_unsigned() {
            return Ok((MirOperand::Local(result), ty.clone()));
        }

        // Signed: a negative remainder is one divisor away from the answer,
        // and which way depends on the divisor's sign — `r + b` for a positive
        // divisor, `r - b` for a negative one. Both land non-negative, which is
        // the mathematical definition the name promises: `(-1).mod(10)` and
        // `(-1).mod(-10)` are both 9.
        //
        // Two branches and no new MIR operator: `Mod`, `Lt`, `Add` and `Sub`
        // all exist and codegen already handles every width for them.
        let is_neg = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: is_neg,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Lt,
                left: MirOperand::Local(result),
                right: MirOperand::Constant(crate::operand::MirConst::Int(0)),
            },
        }));

        let fix_block = self.builder.create_block();
        let done_block = self.builder.create_block();
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(is_neg),
            then_block: fix_block,
            else_block: done_block,
        }));

        self.builder.switch_to_block(fix_block);
        let divisor_neg = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: divisor_neg,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Lt,
                left: rhs.clone(),
                right: MirOperand::Constant(crate::operand::MirConst::Int(0)),
            },
        }));
        let sub_block = self.builder.create_block();
        let add_block = self.builder.create_block();
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(divisor_neg),
            then_block: sub_block,
            else_block: add_block,
        }));

        self.builder.switch_to_block(sub_block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: result,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Sub,
                left: MirOperand::Local(result),
                right: rhs.clone(),
            },
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: done_block,
        }));

        self.builder.switch_to_block(add_block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: result,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Add,
                left: MirOperand::Local(result),
                right: rhs,
            },
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: done_block,
        }));

        self.builder.switch_to_block(done_block);
        Ok((MirOperand::Local(result), ty.clone()))
    }

    /// `reflect.<method><T>()` → the constant it answers.
    ///
    /// Every reflect method is compile-time known once mono has picked `T`
    /// (std.reflect/R5), so there is nothing to call — the answer becomes a
    /// literal here. Only `reflect.fields()` was handled before, and only as the
    /// iterable of a `comptime for`; everywhere else the `reflect` name was left
    /// for ordinary local lookup to trip over, and the failure came out as
    /// "unresolved variable `reflect`" — a diagnosis pointing at name
    /// resolution, which was fine (#775).
    ///
    /// The answers come from `rask_types::reflect` so the interpreter folds the
    /// same ones. Where neither backend can answer — anything needing layout —
    /// it says so instead of picking a number.
    fn try_lower_reflect_call(
        &mut self,
        object: &Expr,
        method: &str,
        type_args: &Option<Vec<String>>,
    ) -> Result<Option<TypedOperand>, LoweringError> {
        use rask_types::reflect::{self, ReflectAnswer};

        if !matches!(&object.kind, ExprKind::Ident(n) if n == "reflect") {
            return Ok(None);
        }
        // `fields` as the iterable of a `comptime for` is unrolled in stmt.rs
        // and never reaches here. Reaching here means it was used as an
        // ordinary value — `let v = reflect.fields<T>()` — which the
        // declaration promises and the interpreter delivers. Native used to
        // refuse it outright (#997); it builds the vector instead now, out of
        // the same constants the unroller splices.
        if method == "fields" {
            let Some(type_name) = type_args.as_ref().and_then(|ta| ta.first()) else {
                return Err(LoweringError::InvalidConstruct(
                    "reflect.fields() needs the type it's asking about: \
                     write `reflect.fields<T>()`"
                        .into(),
                ));
            };
            let type_name = type_name.clone();
            return self.lower_reflect_fields_value(&type_name).map(Some);
        }

        let Some(type_name) = type_args.as_ref().and_then(|ta| ta.first()) else {
            return Err(LoweringError::InvalidConstruct(format!(
                "reflect.{method}() needs the type it's asking about: \
                 write `reflect.{method}<T>()`"
            )));
        };

        // The layouts answer "is this declared"; the checker's type table answers
        // the rest. A layout has dropped `@resource` and substituted its field
        // types by the time it exists, and those are exactly what `is_resource`
        // and the flatness walk are asking about (#791). The interpreter reads the
        // same declarations off its AST maps, so the classifier gets the same
        // input from both sides.
        struct MirDecls<'a, 'b>(&'a super::MirContext<'b>);
        impl MirDecls<'_, '_> {
            fn def(&self, name: &str) -> Option<&rask_types::TypeDef> {
                let bare = name.split('<').next().unwrap_or(name).trim();
                self.0.type_defs.get(self.0.type_defs.get_type_id(bare)?)
            }
        }
        impl reflect::ReflectDecls for MirDecls<'_, '_> {
            fn declares_struct(&self, name: &str) -> bool {
                self.0.find_struct(name).is_some()
            }
            fn declares_enum(&self, name: &str) -> bool {
                self.0.find_enum(name).is_some()
            }
            fn is_resource(&self, name: &str) -> bool {
                // `File` is the compiler's own resource — no declaration carries
                // the annotation for it, and the runtime tracks it as one.
                name == "File"
                    || matches!(self.def(name), Some(rask_types::TypeDef::Struct { is_resource: true, .. }))
            }
            fn member_type_names(&self, name: &str) -> Option<Vec<String>> {
                let spell = |t: &rask_types::Type| {
                    format!("{}", self.0.type_defs.resolve_type_names(t))
                };
                match self.def(name)? {
                    rask_types::TypeDef::Struct { fields, .. } => {
                        Some(fields.iter().map(|(_, t)| spell(t)).collect())
                    }
                    rask_types::TypeDef::Enum { variants, .. } => Some(
                        variants.iter().flat_map(|(_, payload)| payload.iter().map(&spell)).collect(),
                    ),
                    rask_types::TypeDef::Union { fields, .. } => {
                        Some(fields.iter().map(|(_, t)| spell(t)).collect())
                    }
                    rask_types::TypeDef::NominalAlias { underlying, .. } => Some(vec![spell(underlying)]),
                    rask_types::TypeDef::Trait { .. } => None,
                }
            }
            fn type_params(&self, name: &str) -> Vec<String> {
                match self.def(name) {
                    Some(rask_types::TypeDef::Struct { type_params, .. })
                    | Some(rask_types::TypeDef::Enum { type_params, .. }) => type_params.clone(),
                    _ => Vec::new(),
                }
            }
        }

        let answer = reflect::answer(method, type_name, &MirDecls(self.ctx));
        Ok(Some(match answer {
            ReflectAnswer::Bool(b) => (
                MirOperand::Constant(crate::operand::MirConst::Bool(b)),
                MirType::Bool,
            ),
            ReflectAnswer::Int(n) => (
                MirOperand::Constant(crate::operand::MirConst::Int(n as i64)),
                MirType::U64,
            ),
            ReflectAnswer::Str(s) => (
                MirOperand::Constant(crate::operand::MirConst::String(s)),
                MirType::String,
            ),
            ReflectAnswer::Unsupported(why) => {
                return Err(LoweringError::InvalidConstruct(format!(
                    "reflect.{method}<{type_name}>() isn't implemented on either \
                     backend — {why} (#791)"
                )));
            }
            ReflectAnswer::NoSuchMethod => {
                return Err(LoweringError::InvalidConstruct(format!(
                    "no reflect method `{method}` — see specs/stdlib/reflect.md \
                     for the surface"
                )));
            }
        }))
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
        expr: &Expr,
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
                // The qualifier is a module when it isn't a local and the
                // *second* name is a type. `is_type_constructor_name` alone
                // asks whether the qualifier is a known stdlib module, and it
                // only knows the ones that have an `extend <module>` block in
                // the stubs — `stdlib/path.rk` has only `extend Path`, so
                // `path.Path.from("/a")` fell through to ordinary variable
                // lookup and failed with "unresolved variable `path`" while
                // the interpreter ran it fine (#851).
                let names_a_type = self.ctx.find_struct(type_name).is_some()
                    || self.ctx.find_enum(type_name).is_some()
                    || rask_stdlib::mir_metadata::stdlib_type_names().contains(type_name);
                if !self.locals.contains_key(module_name)
                    && !is_enum_variant
                    && (is_type_constructor_name(module_name) || names_a_type)
                {
                    let func_name = format!("{}_{}", type_name, method);
                    let mut arg_operands = Vec::new();
                    for arg in args {
                        let (op, _) = self.lower_expr(&arg.expr)?;
                        arg_operands.push(op);
                    }
                    let ret_ty = self
                        .sig_ret_ty(&func_name)
                        .unwrap_or_else(|| self.call_ret_ty(&func_name, expr.id));
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

    /// Byte size of what `expr`'s pointer points at — `*u8` → 1, `*i64` → 8.
    /// `None` when `expr` isn't a raw pointer, or its pointee isn't a scalar
    /// this can size (a struct pointee has no single-word read).
    fn pointee_size(&self, expr: &Expr) -> Option<i64> {
        match self.ctx.lookup_raw_type(expr.id)? {
            rask_types::Type::RawPtr(inner) => match inner.as_ref() {
                rask_types::Type::U8 | rask_types::Type::I8 | rask_types::Type::Bool => Some(1),
                rask_types::Type::U16 | rask_types::Type::I16 => Some(2),
                rask_types::Type::U32 | rask_types::Type::I32 | rask_types::Type::F32 => Some(4),
                rask_types::Type::U64 | rask_types::Type::I64 | rask_types::Type::F64
                | rask_types::Type::U128 | rask_types::Type::I128
                | rask_types::Type::Char => Some(8),
                _ => None,
            },
            _ => None,
        }
    }

    /// Like `pointee_size`, but only for pointees that come back as a plain
    /// integer — `RawPtr_read` returns an i64, so a float or a struct behind
    /// the pointer would arrive as its bit pattern.
    fn integral_pointee_size(&self, expr: &Expr) -> Option<i64> {
        match self.ctx.lookup_raw_type(expr.id)? {
            rask_types::Type::RawPtr(inner) if matches!(
                inner.as_ref(),
                rask_types::Type::U8 | rask_types::Type::I8 | rask_types::Type::Bool
                    | rask_types::Type::U16 | rask_types::Type::I16
                    | rask_types::Type::U32 | rask_types::Type::I32
                    | rask_types::Type::U64 | rask_types::Type::I64
            ) => self.pointee_size(expr),
            _ => None,
        }
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
            let entry = rask_stdlib::ptr_methods::lookup(method.as_str());
            if method == "cast" {
                // Cast is a no-op at runtime — pointer value unchanged
                return Ok(Some((obj_op.clone(), MirType::Ptr)));
            }
            let ptr_method = entry
                .filter(|e| e.c_symbol.is_some())
                .map(|e| rask_stdlib::ptr_methods::mir_name(e.name));
            if let Some(func_name) = ptr_method {
                let elem_size = self.pointee_size(object).unwrap_or(8);

                let mut all_args = vec![obj_op.clone()];
                for arg in args {
                    let (op, _) = self.lower_expr(&arg.expr)?;
                    all_args.push(op);
                }
                // read/write/add/sub/offset step by whole elements, so the
                // runtime needs the pointee's size.
                if entry.map(|e| e.scales_by_elem).unwrap_or(false) {
                    all_args.push(MirOperand::Constant(crate::operand::MirConst::Int(elem_size)));
                }
                let ret_ty = match entry.map(|e| e.sig) {
                    Some(rask_stdlib::PtrSig::Write) => MirType::Void,
                    Some(rask_stdlib::PtrSig::Arith) => MirType::Ptr,
                    Some(rask_stdlib::PtrSig::Predicate)
                    | Some(rask_stdlib::PtrSig::PredicateInt)
                    | Some(rask_stdlib::PtrSig::Comparison) => MirType::Bool,
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
            let niche = self.option_operand_niche(object, obj_op);
            let tag_local = self.emit_option_tag(obj_op, niche);
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
        // `Path / "component"` desugars to `.div(...)` same as numeric `/` —
        // Path's own MIR type looks like a plain string (no aggregate layout
        // to distinguish it), so without this check it fell into the numeric
        // fallback below and got compiled as a raw pointer division instead
        // of a call to `Path_div`.
        let is_path_receiver = self.ctx.lookup_raw_type(object.id)
            .and_then(|ty| super::MirContext::type_prefix(&ty, self.ctx.type_names))
            .as_deref() == Some("Path");
        let skip_binop = if raw_type_is_numeric {
            false
        } else if is_path_receiver {
            true
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
        // `mir_type_name` reads the struct/enum layout's own `name`, which for
        // a generic type is the bare declared name ("Wrapping"), never mono's
        // mangled one ("Wrapping$u32") — `compute_struct_layout` never adds
        // the type-argument suffix to `.name`. The registered function is
        // keyed the other way around, method mangled *after* the base name
        // (`Wrapping_mul$u32`, from `mangle_name("Wrapping_mul", [u32])`). An
        // exact-match lookup on `"{ty_name}_{method}"` only ever finds a
        // non-generic overload; on a generic one it silently missed, so
        // `a.mul(b)` on `Wrapping<u32>` fell through to a raw struct-address
        // multiply instead of calling `Wrapping_mul$u32` (#838). A generic
        // instantiation's key always starts with the unmangled prefix plus
        // `$`, so match on that instead of requiring an exact hit.
        // The fallback scan below is a linear pass over every registered
        // function, so it only runs for a generic receiver — the one case an
        // exact-match lookup can't ever find. A non-generic receiver's exact
        // key either exists or the method just isn't an overload, and the
        // first `contains_key` already answers both (#937 review).
        let receiver_is_generic = matches!(
            self.ctx.lookup_raw_type(object.id),
            Some(rask_types::Type::Generic { .. } | rask_types::Type::UnresolvedGeneric { .. })
        );
        let has_operator_overload = aggregate_receiver
            && self.mir_type_name(obj_ty)
                .map(|ty_name| format!("{}_{}", ty_name, method))
                .is_some_and(|qualified| {
                    self.func_sigs.contains_key(&qualified)
                        || (receiver_is_generic
                            && self.func_sigs.keys().any(|k| k.starts_with(&format!("{}$", qualified))))
                });
        let skip_binop = skip_binop || has_operator_overload;

        // std.bits B1 on an integer receiver. These aren't operator methods —
        // they're named calls — but they lower the same way, to a single
        // machine instruction on the receiver's own width, so they belong here
        // rather than in the `{Type}_{method}` dispatch chain (which had no
        // `i64_count_ones` to find, #397).
        // `x.hash()` on any of the scalar Hashable types (HA1). Not an operator
        // method, but like the bit methods it's a single call on the receiver's own
        // width rather than a `{Type}_{method}` body, and there was no `u64_hash`
        // for the dispatch chain to find (#813).
        if method == "hash" && args.is_empty() {
            if let Some(handled) = self.lower_scalar_hash(&obj_op, obj_ty) {
                return Ok(Some(handled));
            }
        }

        if matches!(obj_ty, MirType::I8 | MirType::I16 | MirType::I32 | MirType::I64
                          | MirType::U8 | MirType::U16 | MirType::U32 | MirType::U64
                          | MirType::I128 | MirType::U128)
        {
            if let Some(handled) = self.lower_int_bit_method(method, args, &obj_op, obj_ty)? {
                return Ok(Some(handled));
            }
            if method == "mod" && args.len() == 1 {
                let (rhs, _) = self.lower_expr(&args[0].expr)?;
                return Ok(Some(self.lower_int_mod(&obj_op, rhs, obj_ty)?));
            }
        }

        // Detect binary operator methods (desugared from a + b → a.add(b))
        // Skip for SIMD types and raw pointers — they use method dispatch.
        if !skip_binop {
        if let Some(mir_binop) = operator_method_to_binop(method) {
            if args.len() == 1 {
                let (rhs, rhs_ty) = self.lower_expr(&args[0].expr)?;
                // `a == v` is `a.eq(v)` after desugaring, so the optional/bare
                // mismatch arrives here rather than at the `Binary` arm (#834).
                let (lhs, rhs) = if matches!(mir_binop, crate::operand::BinOp::Eq | crate::operand::BinOp::Ne) {
                    self.align_optional_compare(obj_op.clone(), obj_ty, rhs, &rhs_ty)
                } else {
                    (obj_op.clone(), rhs)
                };
                let result_ty = binop_result_type(&mir_binop, obj_ty);
                let result_local = self.builder.alloc_temp(result_ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result_local,
                    rvalue: MirRValue::BinaryOp {
                        op: mir_binop,
                        left: lhs,
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

    /// `x.hash()` on an integer, a bool or a char — FNV-1a over the value's
    /// little-endian bytes, which is what an int-keyed Map buckets with, so a
    /// value and the same value used as a key agree (HA1, #813).
    ///
    /// The runtime takes the bytes as two words plus a width rather than an
    /// address: a 128-bit value lives in a register pair and has no address to
    /// spell here.
    fn lower_scalar_hash(
        &mut self,
        obj_op: &MirOperand,
        obj_ty: &MirType,
    ) -> Option<super::TypedOperand> {
        let width = match obj_ty {
            MirType::Bool | MirType::I8 | MirType::U8 => 1,
            MirType::I16 | MirType::U16 => 2,
            MirType::I32 | MirType::U32 | MirType::Char => 4,
            MirType::I64 | MirType::U64 => 8,
            MirType::I128 | MirType::U128 => 16,
            _ => return None,
        };
        // Widened to a word for the call. Sign or zero extension makes no
        // difference: only the low `width` bytes are read.
        let (lo, hi) = if width == 16 {
            let low = self.builder.alloc_temp(MirType::U64);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: low,
                rvalue: MirRValue::Cast { value: obj_op.clone(), target_ty: MirType::U64 },
            }));
            let shifted = self.builder.alloc_temp(obj_ty.clone());
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: shifted,
                rvalue: MirRValue::BinaryOp {
                    op: crate::operand::BinOp::Shr,
                    left: obj_op.clone(),
                    right: MirOperand::Constant(MirConst::Int(64)),
                },
            }));
            let high = self.builder.alloc_temp(MirType::U64);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: high,
                rvalue: MirRValue::Cast {
                    value: MirOperand::Local(shifted),
                    target_ty: MirType::U64,
                },
            }));
            (MirOperand::Local(low), MirOperand::Local(high))
        } else {
            (obj_op.clone(), MirOperand::Constant(MirConst::Int(0)))
        };
        let out = self.builder.alloc_temp(MirType::U64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(out),
            func: FunctionRef::internal("int_hash".to_string()),
            args: vec![lo, hi, MirOperand::Constant(MirConst::Int(width))],
        }));
        Some((MirOperand::Local(out), MirType::U64))
    }

    /// std.bits B1 bit methods on an integer receiver.
    ///
    /// The "ones" counts are the "zeros" counts of the complement, and
    /// `count_zeros` is `count_ones` of the complement, so those three compose
    /// from a BitNot rather than carrying MIR ops of their own. Every result
    /// keeps the receiver's type, matching what the checker unified.
    /// OV5's fallible forms: `checked_add`/`sub`/`mul`/`div` answering `T?`, and
    /// `overflowing_add`/`sub`/`mul` answering `(T, bool)`.
    ///
    /// These can't be a `BinOp` the way the wrapping and saturating forms are —
    /// a `BinOp` produces one scalar and these produce an aggregate. So the
    /// pieces come from two ops that do fit: `Wrapping*` for the number and
    /// `Overflow*` for the flag, assembled into the slot the checker typed the
    /// call as.
    ///
    /// `checked_div` is the odd one out. There's no "wrapping div" to compute
    /// unconditionally — dividing by zero traps — so the division happens only
    /// in the branch where the flag already said it's safe, and the guards
    /// codegen puts around `Div` are provably dead there.
    fn lower_fallible_arith(
        &mut self,
        method: &str,
        args: &[CallArg],
        obj_op: &MirOperand,
        obj_ty: &MirType,
    ) -> Result<Option<super::TypedOperand>, LoweringError> {
        use crate::operand::BinOp;

        let (flag_op, value_op, wraps) = match method {
            "checked_add" => (BinOp::OverflowAdd, BinOp::WrappingAdd, false),
            "checked_sub" => (BinOp::OverflowSub, BinOp::WrappingSub, false),
            "checked_mul" => (BinOp::OverflowMul, BinOp::WrappingMul, false),
            "checked_div" => (BinOp::OverflowDiv, BinOp::Div, false),
            "overflowing_add" => (BinOp::OverflowAdd, BinOp::WrappingAdd, true),
            "overflowing_sub" => (BinOp::OverflowSub, BinOp::WrappingSub, true),
            "overflowing_mul" => (BinOp::OverflowMul, BinOp::WrappingMul, true),
            _ => return Ok(None),
        };
        if args.len() != 1 {
            return Ok(None);
        }
        let (rhs, _) = self.lower_expr(&args[0].expr)?;

        let flag = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: flag,
            rvalue: MirRValue::BinaryOp {
                op: flag_op,
                left: obj_op.clone(),
                right: rhs.clone(),
            },
        }));

        // `overflowing_*` reports the wrap instead of refusing it, so both
        // halves are always computed and there's no branch.
        if wraps {
            let wrapped = self.builder.alloc_temp(obj_ty.clone());
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: wrapped,
                rvalue: MirRValue::BinaryOp {
                    op: value_op,
                    left: obj_op.clone(),
                    right: rhs,
                },
            }));
            let pair_ty = MirType::Tuple(vec![obj_ty.clone(), MirType::Bool]);
            let pair = self.builder.alloc_temp(pair_ty.clone());
            let value_size = obj_ty.size();
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: pair,
                offset: 0,
                value: MirOperand::Local(wrapped),
                store_size: Some(value_size),
            }));
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: pair,
                offset: value_size,
                value: MirOperand::Local(flag),
                store_size: Some(MirType::Bool.size()),
            }));
            return Ok(Some((MirOperand::Local(pair), pair_ty)));
        }

        // `T?`: tag 0 with the answer at +8, or tag 1 and nothing.
        let opt_ty = MirType::Option(Box::new(obj_ty.clone()));
        let result = self.builder.alloc_temp(opt_ty.clone());
        let none_block = self.builder.create_block();
        let some_block = self.builder.create_block();
        let merge_block = self.builder.create_block();
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(flag),
            then_block: none_block,
            else_block: some_block,
        }));

        self.builder.switch_to_block(some_block);
        let value = self.builder.alloc_temp(obj_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: value,
            rvalue: MirRValue::BinaryOp {
                op: value_op,
                left: obj_op.clone(),
                right: rhs,
            },
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result,
            offset: 0,
            value: MirOperand::Constant(MirConst::Int(0)),
            store_size: None,
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result,
            offset: 8,
            value: MirOperand::Local(value),
            store_size: Some(obj_ty.size()),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: merge_block,
        }));

        self.builder.switch_to_block(none_block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result,
            offset: 0,
            value: MirOperand::Constant(MirConst::Int(1)),
            store_size: None,
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: merge_block,
        }));

        self.builder.switch_to_block(merge_block);
        Ok(Some((MirOperand::Local(result), opt_ty)))
    }

    fn lower_int_bit_method(
        &mut self,
        method: &str,
        args: &[CallArg],
        obj_op: &MirOperand,
        obj_ty: &MirType,
    ) -> Result<Option<super::TypedOperand>, LoweringError> {
        use crate::operand::UnaryOp as MirUnaryOp;

        // `checked_*` and `overflowing_*` answer with an aggregate, so they're
        // built rather than emitted as one op.
        if let Some(result) = self.lower_fallible_arith(method, args, obj_op, obj_ty)? {
            return Ok(Some(result));
        }

        // Rotations take an amount; the rest are nullary. The overflow escape
        // hatches (OV5/SH2) ride along — same shape, one operand in, the
        // receiver's type out.
        if let Some(rot) = match method {
            "rotate_left" => Some(crate::operand::BinOp::RotateLeft),
            "rotate_right" => Some(crate::operand::BinOp::RotateRight),
            // UN1: `.unchecked_*()` has no defined behavior on overflow, but
            // there's no separate "don't check" instruction to emit — a plain
            // add/sub/mul *is* the unchecked op, which is exactly what the
            // wrapping variant already lowers to (no overflow branch, just
            // the raw instruction). Reusing it costs nothing: whenever the
            // overflow the caller promised not to hit doesn't happen, wrapping
            // and unchecked compute the identical bits.
            "wrapping_add" | "unchecked_add" => Some(crate::operand::BinOp::WrappingAdd),
            "wrapping_sub" | "unchecked_sub" => Some(crate::operand::BinOp::WrappingSub),
            "wrapping_mul" | "unchecked_mul" => Some(crate::operand::BinOp::WrappingMul),
            "wrapping_shl" => Some(crate::operand::BinOp::WrappingShl),
            "wrapping_shr" => Some(crate::operand::BinOp::WrappingShr),
            "saturating_add" => Some(crate::operand::BinOp::SaturatingAdd),
            "saturating_sub" => Some(crate::operand::BinOp::SaturatingSub),
            "saturating_mul" => Some(crate::operand::BinOp::SaturatingMul),
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

    /// The half-open end index for a range's written end.
    ///
    /// `a..=b` includes `b`, while `string_substr` and `Vec_slice` both take a
    /// half-open pair — so an inclusive range ends one past its last index.
    fn bump_inclusive_end(&mut self, end: MirOperand, inclusive: bool) -> MirOperand {
        if !inclusive {
            return end;
        }
        let bumped = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: bumped,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Add,
                left: end,
                right: MirOperand::Constant(MirConst::Int(1)),
            },
        }));
        MirOperand::Local(bumped)
    }

    /// True when this MIR type is the `Ordering` enum.
    pub(super) fn is_ordering_ty(&self, ty: &MirType) -> bool {
        let MirType::Enum(EnumLayoutId { id, .. }) = ty else { return false };
        self.ctx
            .enum_layouts
            .get(*id as usize)
            .is_some_and(|l| l.name == "Ordering")
    }

    /// An `Ordering`'s tag, widened to `i64`.
    ///
    /// For the boundaries that still want a number rather than the value: the
    /// C comparator ABI, and the assert-failure helpers.
    pub(super) fn emit_ordering_tag_i64(&mut self, value: MirOperand) -> crate::LocalId {
        let tag = self.emit_enum_tag(value);
        let wide = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: wide,
            rvalue: MirRValue::Cast {
                value: MirOperand::Local(tag),
                target_ty: MirType::I64,
            },
        }));
        wide
    }

    /// Wrap a computed Ordering tag into an actual `Ordering` value.
    ///
    /// `compare` used to hand the tag back as a bare `i64`. Matching still
    /// worked — the match lowering special-cased `Ordering` against a raw tag —
    /// but nothing downstream knew the value was an enum, so `{a.compare(b)}`
    /// formatted it as the integer it claimed to be and printed `0` for Less,
    /// and a user's `extend Ordering with Displayable` was never consulted
    /// (#729). Storing the tag into a properly laid out slot makes it the same
    /// shape as any other fieldless enum value.
    fn wrap_ordering(&mut self, tag: MirOperand) -> TypedOperand {
        let Some((id, layout)) = self.ctx.find_enum("Ordering") else {
            // No layout registered — a bare context in a unit test. The raw tag
            // is what this used to produce, so fall back rather than fail.
            return (tag, MirType::I64);
        };
        let ty = MirType::Enum(EnumLayoutId::new(id, layout.size, layout.align));
        let slot = self.builder.alloc_temp(ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: slot,
            offset: 0,
            value: tag,
            // `None` — the value's natural width, which is what the enum
            // literal path stores. A narrow store leaves the rest of the slot
            // undefined, and structural `==` compares the whole slot: three
            // asserts in a row passed in `main` and the third failed in a
            // `test` block, purely on what the stack happened to hold.
            store_size: None,
        }));
        (MirOperand::Local(slot), ty)
    }

    /// String comparison operators → `string_lt`, `string_ge`, etc.
    /// Read an enum's variant tag into a fresh local.
    /// `value is <pattern>` as a bool local.
    ///
    /// A pattern naming a variant of the error enum needs *two* tags: the
    /// value's own tag says ok vs err, and the variant tag lives one layer down
    /// in the payload. Comparing the variant tag against the outer tag put
    /// `MyErr.Bad`'s 0 up against the ok tag 0, so the test answered about the
    /// wrong layer — the same two-layer mixup `match` had in #677.
    fn emit_two_layer_pattern_test(
        &mut self,
        val: &MirOperand,
        val_ty: &MirType,
        tag: crate::LocalId,
        is_niche: bool,
        pattern: &rask_ast::expr::Pattern,
    ) -> crate::LocalId {
        if let Some((err_ty, variant_tag)) = self.err_variant_of_result(pattern, val_ty) {
            let payload = self.emit_option_payload(val.clone(), err_ty, is_niche);
            let inner_tag = self.emit_enum_tag(MirOperand::Local(payload));
            let is_err = self.emit_eq_const(tag, 1);
            let is_variant = self.emit_eq_const(inner_tag, variant_tag);
            let result = self.builder.alloc_temp(MirType::Bool);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: result,
                rvalue: MirRValue::BinaryOp {
                    op: crate::operand::BinOp::And,
                    left: MirOperand::Local(is_err),
                    right: MirOperand::Local(is_variant),
                },
            }));
            return result;
        }
        // A union err side: "is it the err side" is only half the question. Both
        // members answer yes to that, so the member index decides which (#776).
        if let Some((union_ty, member_index)) = self.union_member_of_result(pattern, val_ty) {
            let payload = self.emit_option_payload(val.clone(), union_ty, is_niche);
            let member = self.builder.alloc_temp(MirType::I64);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: member,
                rvalue: MirRValue::Field {
                    base: MirOperand::Local(payload),
                    field_index: 0,
                    byte_offset: Some(crate::types::UNION_MEMBER_OFFSET),
                    access: FieldAccess::Sized(8),
                },
            }));
            let is_err = self.emit_eq_const(tag, 1);
            let is_member = self.emit_eq_const(member, member_index);
            let result = self.builder.alloc_temp(MirType::Bool);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: result,
                rvalue: MirRValue::BinaryOp {
                    op: crate::operand::BinOp::And,
                    left: MirOperand::Local(is_err),
                    right: MirOperand::Local(is_member),
                },
            }));
            return result;
        }
        let expected = self.pattern_tag_in_type_context(pattern, val_ty);
        self.emit_eq_const(tag, expected)
    }

    /// `e.message()` where `e` is a union — dispatch by member index.
    ///
    /// Every member satisfies the same obligation (ER4: an error type provides
    /// `message`), so there is one method per member and they agree on the return
    /// type. Which one to call is a runtime fact, so this is a switch on the
    /// member index with one arm per member, each calling `{Member}_{method}` on
    /// the member's own bytes.
    ///
    /// `None` when any member has no nominal name to mangle — better to fall
    /// through to the ordinary dispatch error, which names the method, than to
    /// emit a call to a symbol that doesn't exist.
    fn lower_union_method(
        &mut self,
        expr: &Expr,
        obj_op: &MirOperand,
        obj_ty: &MirType,
        method: &str,
        all_args: &[MirOperand],
    ) -> Result<Option<TypedOperand>, LoweringError> {
        let MirType::Union(members) = obj_ty else {
            return Ok(None);
        };
        let names: Option<Vec<String>> =
            members.iter().map(|m| self.mir_type_name(m)).collect();
        let Some(names) = names else {
            return Ok(None);
        };
        if names.is_empty() {
            return Ok(None);
        }

        // The member's bytes sit past the index.
        let payload = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: payload,
            rvalue: MirRValue::Field {
                base: obj_op.clone(),
                field_index: 0,
                byte_offset: Some(crate::types::UNION_PAYLOAD_OFFSET),
                access: FieldAccess::InPlace(8),
            },
        }));
        let member = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: member,
            rvalue: MirRValue::Field {
                base: obj_op.clone(),
                field_index: 0,
                byte_offset: Some(crate::types::UNION_MEMBER_OFFSET),
                access: FieldAccess::Sized(8),
            },
        }));

        // The checker typed the call, so the result slot is known before any arm
        // is built — the arms only have to agree with it, which ER4 guarantees.
        let ret_ty = self
            .ctx
            .lookup_node_type(expr.id)
            .unwrap_or(MirType::String);
        let result = self.builder.alloc_temp(ret_ty.clone());

        let merge_block = self.builder.create_block();
        let arm_blocks: Vec<crate::BlockId> =
            names.iter().map(|_| self.builder.create_block()).collect();
        let cases: Vec<(u64, crate::BlockId)> = arm_blocks
            .iter()
            .enumerate()
            .map(|(i, b)| (i as u64, *b))
            .collect();
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Switch {
            value: MirOperand::Local(member),
            cases,
            // Member 0's arm, not `unreachable`: the index is written by the
            // compiler at every production site, so an out-of-range one is a
            // compiler bug, and trapping on it would turn that into a SIGILL
            // with nothing to read.
            default: arm_blocks[0],
        }));

        for (i, name) in names.iter().enumerate() {
            self.builder.switch_to_block(arm_blocks[i]);
            let mut args = vec![MirOperand::Local(payload)];
            args.extend(all_args.iter().skip(1).cloned());
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(result),
                func: crate::operand::FunctionRef::internal(format!("{}_{}", name, method)),
                args,
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                target: merge_block,
            }));
        }

        self.builder.switch_to_block(merge_block);
        Ok(Some((MirOperand::Local(result), ret_ty)))
    }

    /// The union type and member index when `pattern` names a member of a
    /// Result's error union, rather than one of the Result's own two sides.
    fn union_member_of_result(
        &self,
        pattern: &rask_ast::expr::Pattern,
        val_ty: &MirType,
    ) -> Option<(MirType, i64)> {
        let MirType::Result { err, .. } = val_ty else {
            return None;
        };
        if !matches!(err.as_ref(), MirType::Union(_)) {
            return None;
        }
        let name = super::match_lower::pattern_name(pattern)?;
        let bare = name.rsplit('.').next().unwrap_or(name);
        let index = self.union_member_index_by_name(err.as_ref(), bare)?;
        Some((err.as_ref().clone(), index as i64))
    }

    /// `local == k` as a fresh bool temp.
    fn emit_eq_const(&mut self, local: crate::LocalId, k: i64) -> crate::LocalId {
        let result = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: result,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Eq,
                left: MirOperand::Local(local),
                right: MirOperand::Constant(MirConst::Int(k)),
            },
        }));
        result
    }

    /// The err type and variant tag when `pattern` names a variant of a
    /// Result's error enum, rather than one of the Result's own two sides.
    fn err_variant_of_result(
        &self,
        pattern: &rask_ast::expr::Pattern,
        val_ty: &MirType,
    ) -> Option<(MirType, i64)> {
        let err_ty = match val_ty {
            MirType::Result { err, .. } => err.as_ref().clone(),
            _ => return None,
        };
        let MirType::Enum(crate::types::EnumLayoutId { id, .. }) = &err_ty else {
            return None;
        };
        let name = super::match_lower::pattern_name(pattern)?;
        let bare = name.rsplit('.').next().unwrap_or(name);
        let layout = self.ctx.enum_layouts.get(*id as usize)?;
        let tag = layout.variants.iter().find(|v| v.name == bare)?.tag as i64;
        Some((err_ty, tag))
    }

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
        // Floats can't use the `<`/`>` chain below. Those operators are IEEE, so
        // NaN answers false to both and lands on Equal, and -0 vs +0 compares
        // equal — but `compare` is the *total* order (type.operators/ORD3),
        // where -0 < +0 and NaN sorts to an end. One call to the runtime's
        // total-order comparator instead.
        if matches!(obj_ty, MirType::F32 | MirType::F64) {
            let (rhs, _) = self.lower_expr(&args[0].expr)?;
            let result = self.builder.alloc_temp(MirType::I64);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(result),
                func: FunctionRef::internal("f64_compare".to_string()),
                args: vec![obj_op.clone(), rhs],
            }));
            // `rask_f64_compare_total` already answers in tag values (0/1/2),
            // unlike `string_compare`'s -1/0/1 — no shift here.
            return Ok(Some(self.wrap_ordering(MirOperand::Local(result))));
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
        Ok(Some(self.wrap_ordering(MirOperand::Local(result))))
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
            return Ok(Some(self.wrap_ordering(MirOperand::Local(tag))));
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
        if method == "__concat" && args.len() == 1 && matches!(obj_ty, MirType::String) {
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

    /// Derived `Debug` for a struct or enum: the layout, rendered.
    ///
    /// `Point { x: 1, y: 2 }` for a struct, `Shape.Circle(1.5)` for an enum
    /// variant with a payload, `Shape.Empty` for one without. Strings and chars
    /// come out quoted, which is the whole reason `{:debug}` exists next to
    /// `{}` (std.fmt/G2, G4).
    ///
    /// Compile-time expansion, not a runtime call. Nothing has to be emitted
    /// per type and nothing has to walk a descriptor at runtime — the field
    /// names are constants and each field's renderer is picked by its own type.
    ///
    /// `depth` stops a struct that contains itself through a box from expanding
    /// forever; past the limit the nested value renders as `…`.
    fn lower_derived_debug(
        &mut self,
        obj_op: &MirOperand,
        obj_ty: &MirType,
        depth: u32,
    ) -> Result<Option<MirOperand>, LoweringError> {
        const MAX_DEPTH: u32 = 4;

        let lit = |this: &mut Self, text: &str| {
            let _ = this;
            MirOperand::Constant(MirConst::String(text.to_string()))
        };

        match obj_ty {
            MirType::Struct(id) => {
                let Some(layout) = self.ctx.struct_layouts.get(id.id as usize).cloned() else {
                    return Ok(None);
                };
                let mut parts: Vec<MirOperand> = Vec::new();
                if layout.fields.is_empty() {
                    parts.push(lit(self, &format!("{} {{}}", layout.name)));
                } else {
                    parts.push(lit(self, &format!("{} {{ ", layout.name)));
                    for (i, f) in layout.fields.iter().enumerate() {
                        if i > 0 {
                            parts.push(lit(self, ", "));
                        }
                        parts.push(lit(self, &format!("{}: ", f.name)));
                        let field_ty = self.ctx.type_to_mir(&f.ty);
                        let field = self.builder.alloc_temp(field_ty.clone());
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                            dst: field,
                            rvalue: MirRValue::Field {
                                base: obj_op.clone(),
                                field_index: i as u32,
                                byte_offset: Some(f.offset),
                                access: FieldAccess::for_field(&field_ty, f.size),
                            },
                        }));
                        parts.push(self.debug_render_value(
                            &MirOperand::Local(field), &field_ty, Some(&f.ty), depth,
                        )?);
                    }
                    parts.push(lit(self, " }"));
                }
                Ok(Some(self.concat_all(parts)))
            }
            MirType::Enum(id) => {
                let Some(layout) = self.ctx.enum_layouts.get(id.id as usize).cloned() else {
                    return Ok(None);
                };
                // One block per variant, each building its own string, joined by
                // a phi-free write into a shared slot — the same shape the union
                // method dispatch uses.
                let tag = self.builder.alloc_temp(MirType::I64);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: tag,
                    rvalue: MirRValue::Field {
                        base: obj_op.clone(),
                        field_index: 0,
                        byte_offset: Some(layout.tag_offset),
                        access: FieldAccess::Sized(8),
                    },
                }));
                let result = self.builder.alloc_temp(MirType::String);
                let merge = self.builder.create_block();
                let arms: Vec<crate::BlockId> =
                    layout.variants.iter().map(|_| self.builder.create_block()).collect();
                let cases: Vec<(u64, crate::BlockId)> = layout
                    .variants
                    .iter()
                    .zip(&arms)
                    .map(|(v, b)| (v.tag, *b))
                    .collect();
                let default = arms.first().copied().unwrap_or(merge);
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Switch {
                    value: MirOperand::Local(tag),
                    cases,
                    default,
                }));

                for (vi, variant) in layout.variants.iter().enumerate() {
                    self.builder.switch_to_block(arms[vi]);
                    let mut parts: Vec<MirOperand> = Vec::new();
                    if variant.fields.is_empty() {
                        parts.push(lit(self, &format!("{}.{}", layout.name, variant.name)));
                    } else {
                        parts.push(lit(
                            self,
                            &format!("{}.{}(", layout.name, variant.name),
                        ));
                        for (fi, f) in variant.fields.iter().enumerate() {
                            if fi > 0 {
                                parts.push(lit(self, ", "));
                            }
                            let field_ty = self.ctx.type_to_mir(&f.ty);
                            let field = self.builder.alloc_temp(field_ty.clone());
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                                dst: field,
                                rvalue: MirRValue::Field {
                                    base: obj_op.clone(),
                                    field_index: fi as u32,
                                    byte_offset: Some(variant.payload_offset + f.offset),
                                    access: FieldAccess::for_field(&field_ty, f.size),
                                },
                            }));
                            parts.push(self.debug_render_value(
                                &MirOperand::Local(field), &field_ty, Some(&f.ty), depth,
                            )?);
                        }
                        parts.push(lit(self, ")"));
                    }
                    let text = self.concat_all(parts);
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                        dst: result,
                        rvalue: MirRValue::Use(text),
                    }));
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                        target: merge,
                    }));
                }

                self.builder.switch_to_block(merge);
                Ok(Some(MirOperand::Local(result)))
            }
            _ => Ok(None),
        }
    }

    /// One value inside a derived Debug: quoted for a string or char, recursive
    /// for a nested struct, enum or tuple, elementwise for a Vec, and `…` for
    /// anything the renderer can't reach.
    ///
    /// `decl` is the checked type when the caller has it. A Vec is `Ptr` by the
    /// time MIR sees it, so the element type only survives here — without it a
    /// Vec field printed the *address* of its buffer as an integer, which is
    /// how `Holder { items: 609538720 }` came out where the interpreter says
    /// `Holder { items: [1, 2] }`.
    fn debug_render_value(
        &mut self,
        op: &MirOperand,
        ty: &MirType,
        decl: Option<&rask_types::Type>,
        depth: u32,
    ) -> Result<MirOperand, LoweringError> {
        const MAX_DEPTH: u32 = 4;
        let call = |this: &mut Self, name: &str, args: Vec<MirOperand>| {
            let dst = this.builder.alloc_temp(MirType::String);
            this.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(dst),
                func: FunctionRef::internal(name.to_string()),
                args,
            }));
            MirOperand::Local(dst)
        };
        let elided = |_: &mut Self| MirOperand::Constant(MirConst::String("…".to_string()));

        match ty {
            MirType::String => Ok(call(self, "string_debug", vec![op.clone()])),
            MirType::Char => Ok(call(self, "char_debug", vec![op.clone()])),
            MirType::Struct(_) | MirType::Enum(_) if depth + 1 < MAX_DEPTH => {
                Ok(self
                    .lower_derived_debug(op, ty, depth + 1)?
                    .unwrap_or_else(|| MirOperand::Constant(MirConst::String("…".to_string()))))
            }
            MirType::Struct(_) | MirType::Enum(_) => Ok(elided(self)),
            MirType::Tuple(fields) if depth + 1 < MAX_DEPTH => {
                self.debug_render_tuple(op, fields, decl, depth + 1)
            }
            MirType::Tuple(_) => Ok(elided(self)),
            // An array's length is part of its type, so the elements unroll at
            // compile time — no runtime helper, no loop, and it works for an
            // element the helper can't read (a tuple, a struct). `[1, 2, 3]`
            // used to render as a bare `…` on native and `[1, 2, 3]` on the
            // interpreter.
            MirType::Array { elem, len } if depth + 1 < MAX_DEPTH => {
                self.debug_render_array(op, elem, *len, decl, depth + 1)
            }
            MirType::Array { .. } => {
                Ok(MirOperand::Constant(MirConst::String("[…]".to_string())))
            }
            // A Vec, or anything else that reached MIR as a bare pointer. The
            // element kind decides how the runtime reads a slot, and when the
            // element isn't one of those kinds there is nothing honest to print
            // element by element — `[…]` says that, where the old fallthrough
            // said `609538720`.
            MirType::Ptr => match decl.and_then(|d| self.debug_vec_elem_kind(d)) {
                Some(kind) => Ok(call(
                    self,
                    "vec_debug",
                    vec![op.clone(), MirOperand::Constant(MirConst::Int(kind))],
                )),
                None => {
                    // Shape without contents, when the shape is at least known.
                    // A Vec of structs or of Vecs needs per-element recursion
                    // the runtime helper can't do, and a Map needs its entries
                    // walked; both read better as an elided container than as a
                    // bare ellipsis.
                    let shape = match decl.and_then(|d| self.generic_head(d)) {
                        Some((name, _)) if name == "Vec" => Some("[…]"),
                        Some((name, _)) if name == "Map" => Some("{…}"),
                        _ => None,
                    };
                    match shape {
                        Some(text) => Ok(MirOperand::Constant(MirConst::String(text.to_string()))),
                        None => Ok(elided(self)),
                    }
                }
            },
            MirType::I64 | MirType::I32 | MirType::I16 | MirType::I8 => {
                Ok(call(self, "i64_to_string", vec![op.clone()]))
            }
            MirType::U64 | MirType::U32 | MirType::U16 | MirType::U8 => {
                Ok(call(self, "u64_to_string", vec![op.clone()]))
            }
            MirType::I128 => Ok(call(self, "i128_to_string", vec![op.clone()])),
            MirType::U128 => Ok(call(self, "u128_to_string", vec![op.clone()])),
            MirType::F64 => Ok(call(self, "f64_to_string", vec![op.clone()])),
            MirType::F32 => Ok(call(self, "f32_to_string", vec![op.clone()])),
            MirType::Bool => Ok(call(self, "bool_to_string", vec![op.clone()])),
            // Handles, links, slices, trait objects, function pointers, SIMD
            // lanes. Each is a machine word or a fat pointer with no rendering
            // of its own; printing the word was the bug, so say nothing instead.
            _ => Ok(elided(self)),
        }
    }

    /// `(1, "x", true)` — positional, so no field names.
    fn debug_render_tuple(
        &mut self,
        op: &MirOperand,
        fields: &[MirType],
        decl: Option<&rask_types::Type>,
        depth: u32,
    ) -> Result<MirOperand, LoweringError> {
        let decl_fields = match decl {
            Some(rask_types::Type::Tuple(ts)) if ts.len() == fields.len() => Some(ts),
            _ => None,
        };
        let mut parts: Vec<MirOperand> =
            vec![MirOperand::Constant(MirConst::String("(".to_string()))];
        let mut offset: u32 = 0;
        for (i, fty) in fields.iter().enumerate() {
            if i > 0 {
                parts.push(MirOperand::Constant(MirConst::String(", ".to_string())));
            }
            let size = fty.size();
            let slot = self.builder.alloc_temp(fty.clone());
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: slot,
                rvalue: MirRValue::Field {
                    base: op.clone(),
                    field_index: i as u32,
                    byte_offset: Some(offset),
                    access: FieldAccess::for_field(fty, size),
                },
            }));
            let fdecl = decl_fields.and_then(|ts| ts.get(i));
            parts.push(self.debug_render_value(
                &MirOperand::Local(slot),
                fty,
                fdecl,
                depth,
            )?);
            offset += size;
        }
        parts.push(MirOperand::Constant(MirConst::String(")".to_string())));
        Ok(self.concat_all(parts))
    }

    /// `[1, 2, 3]` — every element rendered, up to a cap.
    ///
    /// A long array would otherwise unroll into as many render calls as it has
    /// elements, so past `MAX_SHOWN` the rest is one ellipsis. That's the same
    /// bargain a debugger makes.
    fn debug_render_array(
        &mut self,
        op: &MirOperand,
        elem: &MirType,
        len: u32,
        decl: Option<&rask_types::Type>,
        depth: u32,
    ) -> Result<MirOperand, LoweringError> {
        const MAX_SHOWN: u32 = 32;
        let elem_decl = match decl {
            Some(rask_types::Type::Array { elem, .. }) => Some(&**elem),
            _ => None,
        };
        let shown = len.min(MAX_SHOWN);
        let size = elem.size();
        let mut parts: Vec<MirOperand> =
            vec![MirOperand::Constant(MirConst::String("[".to_string()))];
        for i in 0..shown {
            if i > 0 {
                parts.push(MirOperand::Constant(MirConst::String(", ".to_string())));
            }
            let slot = self.builder.alloc_temp(elem.clone());
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: slot,
                rvalue: MirRValue::Field {
                    base: op.clone(),
                    field_index: i,
                    byte_offset: Some(i * size),
                    access: FieldAccess::for_field(elem, size),
                },
            }));
            parts.push(self.debug_render_value(
                &MirOperand::Local(slot),
                elem,
                elem_decl,
                depth,
            )?);
        }
        if len > shown {
            parts.push(MirOperand::Constant(MirConst::String(", …".to_string())));
        }
        parts.push(MirOperand::Constant(MirConst::String("]".to_string())));
        Ok(self.concat_all(parts))
    }

    /// The `RASK_DEBUG_ELEM_*` code for a `Vec<T>` whose element the runtime
    /// can read on its own, or `None` when it can't — a Vec of structs, of
    /// Vecs, of tuples. Kept next to the C `switch` it feeds.
    fn debug_vec_elem_kind(&self, decl: &rask_types::Type) -> Option<i64> {
        Some(match self.vec_elem_of_checker_type(decl)? {
            MirType::I64 | MirType::I32 | MirType::I16 | MirType::I8 | MirType::I128 => 0,
            MirType::U64 | MirType::U32 | MirType::U16 | MirType::U8 | MirType::U128 => 1,
            MirType::F64 | MirType::F32 => 2,
            MirType::Bool => 3,
            MirType::String => 4,
            MirType::Char => 5,
            _ => return None,
        })
    }

    /// Concatenate a list of string operands left to right.
    fn concat_all(&mut self, parts: Vec<MirOperand>) -> MirOperand {
        let mut iter = parts.into_iter();
        let Some(mut acc) = iter.next() else {
            return MirOperand::Constant(MirConst::String(String::new()));
        };
        for part in iter {
            let dst = self.builder.alloc_temp(MirType::String);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(dst),
                func: FunctionRef::internal("concat".to_string()),
                args: vec![acc, part],
            }));
            acc = MirOperand::Local(dst);
        }
        acc
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
            ExprKind::Int(n, _) => *n as i64,
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
            // std.fmt/G2: every type derives Debug. A struct or enum has no
            // `to_string` unless it opted into Displayable, and falling through
            // to one it doesn't have is what made the spec's own example fail —
            // `{:debug}` was checked against Displayable and rejected (#1032).
            //
            // Built here rather than as a runtime call: lowering knows the
            // layout, so `Point { x: 1, y: 2 }` is a concat chain of constants
            // and field renders, decided at compile time. Same shape the rest
            // of formatting already has.
            //
            // One arm for every type, because the check is now unconditional:
            // routing the leftovers to `to_string` instead handed the receiver
            // to the padder as though the pointer were text, so `{v:debug}` on
            // a Vec printed its address.
            SpecType::Debug => {
                let decl = self.ctx.node_types.get(&object.id).cloned();
                self.debug_render_value(obj_op, obj_ty, decl.as_ref(), 0)?
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
                    // 128-bit values need their own renderers: the 64-bit ones
                    // take the low half and print a different number (#762).
                    MirType::I128 => "i128_to_string",
                    MirType::U128 => "u128_to_string",
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
            let niche = self.option_niche(object, &obj_ty);
            let is_niche = niche.is_some();
            let tag_local = self.emit_option_tag(obj_op, niche);

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
                args: vec![MirOperand::Constant(crate::operand::MirConst::Int(
                    self.forced_operand_was_result(object) as i64,
                ))],
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Unreachable));

            self.builder.switch_to_block(ok_block);
            let payload_ty = self.extract_payload_type(object)
                .unwrap_or_else(|| crate::fallback::i64_fallback("lower/expr:4941"));
            let result_local = self.emit_option_payload(obj_op.clone(), payload_ty.clone(), is_niche);
            return Ok(Some((MirOperand::Local(result_local), payload_ty)));
        }
        Ok(None)
    }

    /// `reflect.fields<T>()` as a value: a real `Vec<FieldInfo>`, built from
    /// the same compile-time constants the `comptime for` unroller splices.
    ///
    /// Everything in a `FieldInfo` is known once mono has picked `T`, so this
    /// is a stack array of literals handed to `rask_vec_from_static` — the same
    /// shape `Vec.from([…])` takes. The container-drop pass frees it.
    fn lower_reflect_fields_value(
        &mut self,
        type_name: &str,
    ) -> Result<TypedOperand, LoweringError> {
        let consts = self.reflect_field_consts(type_name)?;

        let Some((idx, layout)) = self.ctx.find_struct("FieldInfo") else {
            return Err(LoweringError::InvalidConstruct(
                "reflect.fields<T>() as a value needs `FieldInfo`, which this program \
                 never names — add `import std.reflect`"
                    .into(),
            ));
        };
        // Offset and MIR type of each FieldInfo field, in declaration order,
        // so the stores below don't depend on the layout's field order.
        let field_at = |name: &str| -> Option<(u32, MirType)> {
            layout
                .fields
                .iter()
                .find(|f| f.name == name)
                .map(|f| (f.offset, self.ctx.type_to_mir(&f.ty)))
        };
        let slots: Vec<(&'static str, u32, MirType)> = [
            "name", "type_name", "offset", "size", "is_public", "serial_name",
            "is_skipped", "has_default",
        ]
        .into_iter()
        .filter_map(|n| field_at(n).map(|(off, ty)| (n, off, ty)))
        .collect();
        let elem_ty = MirType::Struct(crate::types::StructLayoutId::new(
            idx, layout.size, layout.align,
        ));
        let elem_size = elem_ty.size();

        let array_ty = MirType::Array {
            elem: Box::new(elem_ty.clone()),
            len: consts.len() as u32,
        };
        let arr = self.builder.alloc_temp(array_ty);
        for (i, fc) in consts.iter().enumerate() {
            let base = i as u32 * elem_size;
            for (name, off, ty) in &slots {
                let value = match *name {
                    "name" => MirConst::String(fc.name.clone()),
                    "type_name" => MirConst::String(fc.type_name.clone()),
                    "offset" => MirConst::Int(fc.offset as i64),
                    "size" => MirConst::Int(fc.size as i64),
                    "is_public" => MirConst::Bool(fc.is_public),
                    "serial_name" => MirConst::String(fc.serial_name.clone()),
                    "is_skipped" => MirConst::Bool(fc.is_skipped),
                    _ => MirConst::Bool(fc.has_default),
                };
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                    addr: arr,
                    offset: base + off,
                    value: MirOperand::Constant(value),
                    // A string is 16 bytes of value and the operand is their
                    // address; a bool is one byte and a wider store would run
                    // over the field beside it.
                    store_size: Some(ty.size()),
                }));
            }
        }

        let vec_local = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(vec_local),
            func: FunctionRef::internal("rask_vec_from_static".to_string()),
            args: vec![
                MirOperand::Local(arr),
                MirOperand::Constant(MirConst::Int(consts.len() as i64)),
                MirOperand::Constant(MirConst::Int(elem_size as i64)),
                MirOperand::Constant(MirConst::Int(crate::elem_strs::tag_of(Some(&elem_ty)))),
            ],
        }));
        self.collected_elem_types.insert(vec_local, elem_ty);
        Ok((MirOperand::Local(vec_local), MirType::I64))
    }

    /// Which `string_parse_*` runtime writes into this result slot, by the
    /// slot's own payload type. `None` when the payload isn't a number the
    /// runtime has a variant for — a still-open type, or a generic body that
    /// hasn't been instantiated, both of which keep the 64-bit parse.
    fn parse_variant_for_slot(ret_ty: &MirType) -> Option<&'static str> {
        let MirType::Result { ok, .. } = ret_ty else {
            return None;
        };
        Some(match **ok {
            MirType::F32 => "string_parse_f32",
            MirType::F64 => "string_parse_f64",
            MirType::I8 => "string_parse_i8",
            MirType::I16 => "string_parse_i16",
            MirType::I32 => "string_parse_i32",
            MirType::I64 => "string_parse_i64",
            MirType::U8 => "string_parse_u8",
            MirType::U16 => "string_parse_u16",
            MirType::U32 => "string_parse_u32",
            MirType::U64 => "string_parse_u64",
            _ => return None,
        })
    }

    /// The `[T; N]` methods that need no call — the answer is in the type or is
    /// the array itself.
    ///
    /// Everything not caught here falls through to the shared `Vec` lowering,
    /// which is right for the read-only surface and wrong for anything that
    /// reads a `RaskVec` header: an array local *is* its buffer, with no header
    /// in front of it, so `Vec_as_ptr` handed back whatever the first element
    /// spelled and dereferencing it segfaulted (#946).
    fn try_lower_array_intrinsic(
        &mut self,
        method: &String,
        args: &[CallArg],
        obj_op: &MirOperand,
        obj_ty: &MirType,
    ) -> Result<Option<TypedOperand>, LoweringError> {
        let MirType::Array { len, .. } = obj_ty else {
            return Ok(None);
        };
        if !args.is_empty() {
            return Ok(None);
        }
        match method.as_str() {
            // Length is in the type — a compile-time constant, no runtime call.
            "len" => Ok(Some((
                MirOperand::Constant(MirConst::Int(*len as i64)),
                MirType::I64,
            ))),
            "is_empty" => Ok(Some((
                MirOperand::Constant(MirConst::Bool(*len == 0)),
                MirType::Bool,
            ))),
            // The array's own address is the pointer to its first element.
            "as_ptr" | "as_mut_ptr" => Ok(Some((
                obj_op.clone(),
                MirType::Ptr,
            ))),
            _ => Ok(None),
        }
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
                        .unwrap_or_else(|| crate::fallback::i64_fallback("lower/expr:4989"));
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
        let niche = self.option_niche(scrutinee, &val_ty);
        let is_niche = niche.is_some();
        let tag = self.emit_option_tag(&val, niche);
        let matches = self.emit_two_layer_pattern_test(&val, &val_ty, tag, is_niche, pattern);

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
        let payload_ty = self.payload_type_of_niche(scrutinee, &val_ty, is_niche);
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
    /// The payload type behind an `x?` scrutinee.
    pub(crate) fn presence_payload_type(&mut self, inner: &Expr, scrutinee_ty: &MirType) -> MirType {
        self.extract_payload_type(inner)
            .or_else(|| Self::payload_of_mir(scrutinee_ty))
            .unwrap_or_else(|| crate::fallback::i64_fallback("lower/expr:presence_payload"))
    }

    /// Bind an `x? as v` payload as a local in the current block. Shared by the
    /// `if` form and the `while` form, so a loop reads the payload exactly the
    /// way the branch does.
    pub(crate) fn bind_presence_payload(
        &mut self,
        name: &str,
        val: &MirOperand,
        payload_ty: &MirType,
        is_niche: bool,
    ) {
        let local = self.builder.alloc_local(name.to_string(), payload_ty.clone());
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
                byte_offset: self.payload_byte_offset(payload_ty),
                access: FieldAccess::Word,
            }
        };
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign { dst: local, rvalue }));
        self.locals.insert(name.to_string(), (local, payload_ty.clone()));
        if let Some(prefix) = self.mir_type_name(payload_ty) {
            self.meta_mut(name).type_prefix = Some(prefix);
        }
    }

    fn lower_if_present(
        &mut self,
        inner: &Expr,
        then_branch: &Expr,
        else_branch: Option<&Expr>,
        then_name: Option<String>,
        else_name: Option<String>,
    ) -> Result<TypedOperand, LoweringError> {
        let (val, scrutinee_ty) = self.lower_expr(inner)?;
        let niche = self.option_niche(inner, &scrutinee_ty);
        let is_niche = niche.is_some();
        let tag = self.emit_option_tag(&val, niche);

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
        //
        // The bind is scoped to the branch. `lower_block` snapshots `locals`,
        // but it does that *after* this bind, so the narrowed name used to
        // outlive its block — a second `if x?` further down then read a tag out
        // of the bare payload and segfaulted, where the interpreter answered.
        self.builder.switch_to_block(then_block);
        let payload_ty = self.presence_payload_type(inner, &scrutinee_ty);
        let outer_locals = self.locals.clone();
        if let Some(name) = then_name.as_ref() {
            self.bind_presence_payload(name, &val, &payload_ty, is_niche);
        }
        let (then_val, then_ty) = self.lower_expr(then_branch)?;
        self.locals = outer_locals;
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
        let outer_locals = self.locals.clone();
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

        self.locals = outer_locals;
        self.builder.switch_to_block(merge_block);
        Ok((MirOperand::Local(result_local), then_ty))
    }

    /// Block expression: lower each statement, last expression is the value.
    pub(super) fn lower_block(&mut self, stmts: &[Stmt]) -> Result<TypedOperand, LoweringError> {
        let mut last_val = MirOperand::Constant(MirConst::Int(0));
        let mut last_ty = MirType::Void;
        // Block scope: any const/mut declared inside the braces shadows but
        // doesn't leak out. Snapshot the locals map and restore after.
        //
        // Comptime-known strings are scoped the same way, and for the same
        // reason. A `let w = "limit"` inside the braces shadowing an outer
        // `let w = "spent"` used to overwrite the entry and keep it after the
        // block ended, so a later `b.(w)` read the inner name — the interpreter
        // said `spent`, native said `limit`, and nothing complained.
        let saved_locals = self.locals.clone();
        let saved_comptime_strings = self.comptime_strings.clone();
        // ctrl.ensure/EN1: an `ensure` runs when its *enclosing block* exits,
        // not when the function does. Loop bodies did this already (they lower
        // their statements directly and call `close_loop_body`), so a bare
        // block, an `if`/`else` body and a match arm — everything that reaches
        // MIR as a block expression — left their ensures on the stack for the
        // function's exit to drain: `{ ensure push(1); push(0) } push(2)` gave
        // 0,2,1 instead of 0,1,2, and a file opened in a block to bound its
        // lifetime stayed open for the whole function (#929).
        let ensure_depth = self.ensure_stack.len();
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
        // An exit that already terminated ran its own chain — `return` emits a
        // CleanupReturn covering everything still on the stack, `break` and
        // `continue` drain back to the loop's depth. Only the fall-through
        // exit is left to us.
        if self.builder.current_block_unterminated() {
            self.emit_loop_cleanup(ensure_depth);
        }
        self.ensure_stack.truncate(ensure_depth);
        self.locals = saved_locals;
        self.comptime_strings = saved_comptime_strings;
        Ok((last_val, last_ty))
    }
} // end impl MirLowerer
