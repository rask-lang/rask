// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Statement lowering.

use crate::FieldAccess;
use super::{LoopContext, LoweringError, MirLowerer};
use crate::{
    operand::{BinOp, MirConst},
    types::StructLayoutId,
    FunctionRef, MirOperand, MirRValue, MirStmt, MirStmtKind, MirTerminator, MirTerminatorKind,
    MirType,
};
use rask_ast::{
    expr::{Expr, ExprKind, Pattern, UnaryOp},
    stmt::{ForBinding, Stmt, StmtKind, TuplePat},
};

/// A bare single-letter name (`T`, `E`, ...) — Rask's convention for an
/// uninstantiated generic type parameter, same heuristic reachability uses
/// to tell "still a placeholder" from "a real single-letter type name" (there
/// are none of the latter in practice).
fn is_bare_type_param(name: &str) -> bool {
    let mut chars = name.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if c.is_ascii_uppercase())
}

/// A range with its ctrl.ranges adapters already peeled off.
struct AdaptedRange<'e> {
    start: Option<&'e Expr>,
    end: Option<&'e Expr>,
    inclusive: bool,
    /// `None` means stride 1.
    step: Option<&'e Expr>,
    rev: bool,
}

impl<'a> MirLowerer<'a> {
    /// A `Vec<(A, B)>`'s element as a MIR tuple, for `for (a, b) in v`.
    fn vec_tuple_elem_type(&self, iter_expr: &Expr) -> Option<MirType> {
        use rask_types::{GenericArg, Type};
        let ty = self.ctx.lookup_raw_type(iter_expr.id)?;
        let args = match ty {
            Type::Generic { args, .. } | Type::UnresolvedGeneric { args, .. } => args,
            _ => return None,
        };
        let GenericArg::Type(elem) = args.first()? else { return None };
        let Type::Tuple(parts) = elem.as_ref() else { return None };
        Some(MirType::Tuple(
            parts.iter().map(|p| self.ctx.type_to_mir(p)).collect(),
        ))
    }

    /// The element type of `m.keys()` / `m.values()` — the map's key or value
    /// type. Nothing downstream of the call knows it otherwise.
    fn map_projection_elem_type(&self, iter_expr: &Expr) -> Option<MirType> {
        let ExprKind::MethodCall { object, method, .. } = &iter_expr.kind else {
            return None;
        };
        let index = match method.as_str() {
            "keys" => 0,
            "values" => 1,
            _ => return None,
        };
        let pair = self.map_entry_pair_types(object);
        pair.get(index).cloned().filter(|t| !matches!(t, MirType::I64))
    }

    /// The MIR types of a map's (key, value) pair, for iterating its entries.
    /// Falls back to two words when the checker didn't resolve `Map<K, V>`.
    fn map_entry_pair_types(&self, iter_expr: &Expr) -> Vec<MirType> {
        use rask_types::{GenericArg, Type};
        let pair = self.ctx.lookup_raw_type(iter_expr.id).and_then(|ty| {
            let args = match ty {
                Type::Generic { args, .. } | Type::UnresolvedGeneric { args, .. } => args,
                _ => return None,
            };
            let mut out = Vec::new();
            for a in args.iter().take(2) {
                let GenericArg::Type(t) = a else { return None };
                out.push(self.ctx.type_to_mir(t));
            }
            (out.len() == 2).then_some(out)
        });
        pair.unwrap_or_else(|| vec![MirType::I64, MirType::I64])
    }

    /// OPT6/#380: wrap a bare `T` operand into `Some(T)` when it's stored into an
    /// `Option<T>` place. Same two-store construction the return-path auto-wrap
    /// uses (tag 0 at offset 0, payload at offset 8). Returns the value unchanged
    /// when no wrap applies.
    fn wrap_for_option_place(
        &mut self,
        val_op: MirOperand,
        val_ty: MirType,
        place_ty: &MirType,
    ) -> (MirOperand, MirType) {
        // The checker already accepted this assignment, so an `Option<T>` place
        // with a non-`Option` value is a widen. Don't require the inner MIR type
        // to match `val_ty` — handles carry an inconsistent repr (`handle` vs the
        // raw `i64` an insert returns), which would spuriously skip the wrap.
        if let MirType::Option(inner) = place_ty {
            // A niche option is niche-optimized in the layout: the value *is*
            // `Some`, the reserved word is `None`. So a bare handle or link
            // stored straight in is already the `Some` repr — a tag + payload
            // wrap would be both wrong and 8 bytes too big for the slot.
            let niche = inner.is_niche_payload();
            if !niche && !matches!(val_ty, MirType::Option(_)) {
                let wrap_local = self.builder.alloc_temp(place_ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                    addr: wrap_local,
                    offset: 0,
                    value: MirOperand::Constant(MirConst::Int(0)),
                    store_size: Some(8),
                }));
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                    addr: wrap_local,
                    offset: 8,
                    value: val_op,
                    store_size: Some(val_ty.size()),
                }));
                return (MirOperand::Local(wrap_local), place_ty.clone());
            }
        }
        (val_op, val_ty)
    }

    /// Project a nested place (`base.f.g`, tuples included) to a base local plus
    /// a byte offset, so an assignment stores straight into the base's storage
    /// rather than materializing an intermediate field as a value copy — the
    /// copy is what dropped `ln.a.x = v` on native (#411). Returns `None` for
    /// anything not rooted at an aggregate local (pool index, Vec element,
    /// function result), which the caller lowers separately.
    /// Root local, byte offset, type at the end of the chain, and the layout's
    /// byte size for that field (`None` at the root, which is the whole local).
    pub(crate) fn lower_place_chain(
        &self,
        expr: &Expr,
    ) -> Option<(crate::LocalId, u32, MirType, Option<u32>)> {
        match &expr.kind {
            ExprKind::Ident(name) => {
                let (id, ty) = self.locals.get(name).cloned()?;
                match ty {
                    // A link local holds the node's address, which is the same
                    // thing an aggregate local holds — so it roots a place chain
                    // the same way, and `n.field = v` is one store rather than a
                    // write into a copy.
                    MirType::Struct(_) | MirType::Tuple(_) | MirType::Link(_) => Some((id, 0, ty, None)),
                    _ => None,
                }
            }
            ExprKind::Field { object, field } => {
                let (base, off, oty, _) = self.lower_place_chain(object)?;
                let (foff, fty, fsize) = self.field_offset_ty_size(&oty, field)?;
                Some((base, off + foff, fty, fsize))
            }
            // A deref through an `Owned` is a borrow, and `Owned<T>` is
            // transparent — `(*p).x` is the same place as `p.x` (mem.owned/OW3).
            // A raw pointer keeps its deref; that load is real.
            ExprKind::Unary { op: rask_ast::expr::UnaryOp::Deref, .. } => {
                let inner = self.peel_owned_deref(expr);
                if std::ptr::eq(inner, expr) {
                    None
                } else {
                    self.lower_place_chain(inner)
                }
            }
            _ => None,
        }
    }

    /// Strip derefs that are borrows through a transparent `Owned`, leaving a
    /// raw-pointer deref alone. `Owned<T>` is `T` to the checker, so `*p` names
    /// the same storage as `p`; treating it as a place of its own left the
    /// store matching no arm at all and silently dropped it (#737).
    pub(crate) fn peel_owned_deref<'e>(&self, expr: &'e Expr) -> &'e Expr {
        let mut cur = expr;
        while let ExprKind::Unary {
            op: rask_ast::expr::UnaryOp::Deref, operand,
        } = &cur.kind {
            let operand_is_raw = matches!(
                self.ctx.lookup_raw_type(operand.id).map(|t| self.ctx.type_to_mir(t)),
                Some(MirType::Ptr)
            );
            if operand_is_raw {
                return cur;
            }
            cur = operand;
        }
        cur
    }

    /// True when `expr` refers to a `Vec` collection (by local metadata or the
    /// type checker's recorded type).
    pub(crate) fn is_vec_expr(&self, expr: &Expr) -> bool {
        if let ExprKind::Ident(name) = &expr.kind {
            if self.meta(name).and_then(|m| m.type_prefix.as_deref()) == Some("Vec") {
                return true;
            }
        }
        // The checker records an instantiated Vec as a generic, and `type_prefix`
        // renders that as `Vec<T>` — so comparing the whole string only ever
        // matched the bare-`Vec` spelling the `meta` path above supplies. Every
        // Vec reached through a field, an index, or a binding read as "not a Vec"
        // and lost its element write-back. Compare the base name, the way
        // `receiver_method_mutates` already does.
        self.ctx
            .lookup_raw_type(expr.id)
            .and_then(|ty| super::MirContext::type_prefix(ty, self.ctx.type_names))
            .is_some_and(|p| p.split('<').next() == Some("Vec"))
    }

    /// True when `expr` refers to a `Map`. Same two sources as `is_vec_expr`,
    /// and the same base-name comparison — the checker spells an instantiated
    /// one `Map<K, V>`.
    pub(crate) fn is_map_expr(&self, expr: &Expr) -> bool {
        if let ExprKind::Ident(name) = &expr.kind {
            if self.meta(name).and_then(|m| m.type_prefix.as_deref()) == Some("Map") {
                return true;
            }
        }
        self.ctx
            .lookup_raw_type(expr.id)
            .and_then(|ty| super::MirContext::type_prefix(ty, self.ctx.type_names))
            .is_some_and(|p| p.split('<').next() == Some("Map"))
    }

    /// Peel a `.f.g.h` chain off a place, returning what it's rooted at and the
    /// field names in outermost-last order.
    fn peel_field_path(expr: &Expr) -> (&Expr, Vec<&str>) {
        let mut fields = Vec::new();
        let mut base = expr;
        while let ExprKind::Field { object, field } = &base.kind {
            fields.push(field.as_str());
            base = object;
        }
        fields.reverse();
        (base, fields)
    }

    /// Summed byte offset of a whole field path within `base_ty`.
    /// Byte offset of a field path, and how many bytes sit at the end of it.
    /// The width is what tells a store how much to copy — without it a 16-byte
    /// string field got an 8-byte pointer store.
    ///
    /// The size comes from the layout, not from the field type's nominal size.
    /// A niche-packed `Handle?` is 8 bytes where its type reports 16, and a
    /// 16-byte copy from an integer sentinel dereferences it.
    fn field_path_offset_ty(&self, base_ty: &MirType, fields: &[&str]) -> Option<(u32, u32)> {
        let mut offset = 0;
        let mut ty = base_ty.clone();
        let mut size = None;
        for field in fields {
            let (off, fty, fsize) = self.field_offset_ty_size(&ty, field)?;
            offset += off;
            ty = fty;
            size = fsize;
        }
        Some((offset, size?))
    }

    /// Byte offset + MIR type of `field` within an aggregate MIR type.
    fn field_offset_ty(&self, oty: &MirType, field: &str) -> Option<(u32, MirType)> {
        self.field_offset_ty_size(oty, field).map(|(off, ty, _)| (off, ty))
    }

    /// Same, plus the layout's recorded byte size for the field when there is
    /// one. Tuple fields carry their size in the layout too.
    fn field_offset_ty_size(
        &self,
        oty: &MirType,
        field: &str,
    ) -> Option<(u32, MirType, Option<u32>)> {
        // A link is the node's address, so a field through it projects exactly
        // as it would through an aggregate local — same base, same offsets.
        let oty = match oty {
            MirType::Link(sid) => &MirType::Struct(sid.clone()),
            other => other,
        };
        if let MirType::Struct(StructLayoutId { id, .. }) = oty {
            let layout = self.ctx.struct_layouts.get(*id as usize)?;
            let fl = layout.fields.iter().find(|f| f.name == *field)?;
            let ty = self.ctx.resolve_type_str(&format!("{}", fl.ty));
            return Some((fl.offset, ty, Some(fl.size)));
        }
        if let Some((_, ety, Some(off), fsize)) = Self::resolve_tuple_field(oty, field) {
            return Some((off, ety, fsize));
        }
        None
    }

    /// Record the edges a struct's own fields carry, against the storage it
    /// just landed in.
    ///
    /// An assignment writes one edge and can register it as it goes. A *literal*
    /// can't: `Cursor { at: victim }` fills the field before the value has an
    /// address, so nothing knew which slot to record. This runs once the
    /// destination is settled, which is the first moment there is a slot.
    ///
    /// Not emitted for the temporary that feeds `rack.insert(...)` — that one is
    /// stack scratch about to be copied into a node, and registering it would
    /// leave the rack holding the address of a dead frame. `insert` records the
    /// node's own copy from the same descriptor.
    pub(crate) fn emit_struct_link_registration(&mut self, dst: crate::LocalId, ty: &MirType) {
        if !self.ctx.struct_carries_links(ty) {
            return;
        }
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::internal("Link_register_struct".to_string()),
            args: vec![MirOperand::Local(dst)],
        }));
    }

    /// Write a link into a slot through the rack, so the target learns who
    /// points at it. `base + offset` is the slot's address; the runtime writes
    /// it, unregisters whatever edge it held, and registers the new one.
    fn emit_link_store(&mut self, base: crate::LocalId, offset: u32, value: MirOperand) {
        // A node's own link field keeps its edge record inline in the node
        // header, so the runtime reaches it by arithmetic instead of scanning
        // the old target's incoming list. Only the base tells them apart: a
        // link base means the holder is a node, anything else is foreign
        // storage that still needs the scanned record.
        if matches!(self.builder.local_type(base), Some(MirType::Link(_))) {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::internal("Link_set_node".to_string()),
                args: vec![
                    MirOperand::Local(base),
                    MirOperand::Constant(MirConst::Int(offset as i64)),
                    value,
                ],
            }));
            return;
        }
        let slot = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: slot,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Add,
                left: MirOperand::Local(base),
                right: MirOperand::Constant(MirConst::Int(offset as i64)),
            },
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::internal("Link_set".to_string()),
            args: vec![MirOperand::Local(slot), value],
        }));
    }

    pub(super) fn lower_stmt(&mut self, stmt: &Stmt) -> Result<(), LoweringError> {
        self.builder.set_span(stmt.span);
        match &stmt.kind {
            StmtKind::Expr(e) => {
                self.lower_expr(e)?;
                // C1/C2: if this is a consuming method call on an ensure receiver,
                // emit ResourceConsume so the ensure is cancelled at cleanup time.
                self.check_resource_consume(e);
                Ok(())
            }

            StmtKind::Mut { name, ty, init, .. } => {
                // A `mut` can be reassigned, so whatever this name meant to
                // `value.(name)` before, it doesn't now.
                self.comptime_strings.remove(name);
                self.lower_binding(name, ty.as_deref(), init)
            }

            StmtKind::Let { name, ty, init, .. } => {
                // A `let` bound to a compile-time string can name a field:
                // `let which = comptime { "y" }` then `p.(which)` (#930).
                // Recorded here because lowering the initializer turns it into
                // an ordinary runtime value and the fact is gone.
                // Errors are not this statement's to report: a `comptime`
                // block that fails here is still lowered as an ordinary
                // initializer below, and gets to fail on its own terms. Only a
                // name that folded to a string is worth remembering.
                match self.comptime_field_name(init) {
                    Ok(Some(s)) => { self.comptime_strings.insert(name.clone(), s); }
                    _ => { self.comptime_strings.remove(name); }
                }
                // If this const was evaluated at compile time, emit a global reference
                if let Some((key, meta)) = self.comptime_global_for(name) {
                    if meta.type_prefix == "Vec" {
                        // Array: store pointer for later Vec wrapping
                        let mir_ty = if let Some(ty_str) = ty.as_deref() {
                            self.ctx.resolve_type_str(ty_str)
                        } else {
                            MirType::Ptr
                        };
                        let local_id = self.builder.alloc_local(name.to_string(), mir_ty.clone());
                        self.locals.insert(name.to_string(), (local_id, mir_ty));
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::GlobalRef {
                            dst: local_id,
                            name: key.clone(),
                        }));
                    } else {
                        // Scalar: load the data pointer, then deref to get the value
                        //
                        // The folded value's own width decides this, not the
                        // binding's annotation — an unannotated `let v = f()` on a
                        // comptime `u128` used to default to `i64` and the deref
                        // read 8 of the 16 bytes, so the answer came back mod 2^64
                        // (#824).
                        let mir_ty = Self::comptime_global_mir_type(&meta.type_prefix)
                            .unwrap_or_else(|| match ty.as_deref() {
                                Some(ty_str) => self.ctx.resolve_type_str(ty_str),
                                None => MirType::I64,
                            });
                        let ptr_local = self.builder.alloc_temp(MirType::Ptr);
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::GlobalRef {
                            dst: ptr_local,
                            name: key.clone(),
                        }));
                        let local_id = self.builder.alloc_local(name.to_string(), mir_ty.clone());
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                            dst: local_id,
                            rvalue: MirRValue::Deref(MirOperand::Local(ptr_local)),
                        }));
                        self.locals.insert(name.to_string(), (local_id, mir_ty));
                    }
                    // The tracked prefix is what later method calls on this
                    // binding dispatch through, and a scalar's dispatch prefix
                    // isn't its type name — narrow widths ride the 64-bit symbols.
                    let prefix = super::builtin_method_prefix_for_name(&meta.type_prefix)
                        .map(str::to_string)
                        .unwrap_or_else(|| meta.type_prefix.clone());
                    self.meta_mut(name).type_prefix = Some(prefix);
                    return Ok(());
                }
                self.lower_binding(name, ty.as_deref(), init)
            }

            StmtKind::Return(opt_expr) => {
                let mut returned_ty = None;
                let value = if let Some(e) = opt_expr {
                    let (op, op_ty) = self.lower_expr(e)?;
                    returned_ty = Some(op_ty.clone());
                    // Wrap a bare value into whatever the return type asks for:
                    // `func -> User? { return User { ... } }` needs one layer,
                    // `func -> T? or E { return t }` needs two (ER9). Returned
                    // unwrapped, the caller reads the value's first word as the
                    // tag (#274, #383).
                    let ret_ty = self.builder.ret_ty().clone();
                    let final_op = self.coerce_into_wrapper(
                        rask_ast::coercion::CoercionSite::Return,
                        op, &op_ty, &ret_ty,
                    );
                    Some(final_op)
                } else {
                    None
                };
                // Inside an inlined closure (e.g. fold callback), redirect
                // return to an assignment + goto instead of a real return.
                if let Some((dst_local, cont_block)) = self.inline_return_target {
                    if let Some(val) = value {
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                            dst: dst_local,
                            rvalue: MirRValue::Use(val),
                        }));
                    }
                    self.inline_return_taken =
                        Some(returned_ty.unwrap_or(MirType::Void));
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: cont_block }));
                } else {
                    self.terminate_return(value);
                }
                Ok(())
            }

            StmtKind::Assign { target, value, .. } => {
                // mem.owned/OW3: `*p = v` writes through a borrow of a
                // transparent `Owned`, so it names the same place as `p = v`.
                // Left as a Deref target it matched no arm below and the store
                // was dropped on the floor — native printed the old value back
                // with no error at all (#737).
                let target = self.peel_owned_deref(target);
                let (val_op, val_ty) = self.lower_expr(value)?;
                // OPT6/#380: widen a bare `T` into `Some(T)` when the lvalue is an
                // `Option<T>` place (reassignment or index/field store). The checker
                // accepts the widening; without the wrap the bare value lands in the
                // slot and a later `?` read misses the `Some` (or corrupts the tag).
                let place_ty = self
                    .ctx
                    .lookup_raw_type(target.id)
                    .cloned()
                    .map(|t| self.ctx.type_to_mir(&t));
                let (val_op, val_ty) = match &place_ty {
                    Some(pt) => self.wrap_for_option_place(val_op, val_ty, pt),
                    None => (val_op, val_ty),
                };
                // A container of links that arrived whole — a `filter` result,
                // say — carries edges nothing recorded. Register them here; the
                // records dedupe per (container, target), so this is free where
                // the pushes already did it.
                if let Some(func) = self
                    .ctx
                    .lookup_raw_type(value.id)
                    .cloned()
                    .and_then(|t| self.ctx.container_link_registration(&t))
                {
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal(func.to_string()),
                        args: vec![val_op.clone()],
                    }));
                }
                // A whole struct written over a name carries whatever edges its
                // fields hold, and a literal's fields were filled before there
                // was a slot to record — same gap the binding path closes.
                let struct_assigned = matches!(&target.kind, ExprKind::Ident(_))
                    && matches!(&value.kind, ExprKind::StructLit { .. });
                match &target.kind {
                    ExprKind::Ident(name) => {
                        let (local_id, dst_ty) = self
                            .locals
                            .get(name)
                            .cloned()
                            .ok_or_else(|| LoweringError::UnresolvedVariable(name.clone()))?;
                        // mem.borrowing/M-rules: `p = expr` on an aggregate `mutate`
                        // param copies bytes through the caller's pointer. Aggregates
                        // are passed by pointer, so lower as a Store — DCE keeps it and
                        // codegen emits the through-pointer copy so the caller sees the
                        // reassignment. Scalar (Copy) params are passed by value, not
                        // by pointer: storing through them would dereference the value
                        // as an address (segfault), so they fall through to a plain
                        // local Assign — the Copy is mutated in place with no writeback,
                        // matching `modify_int(x)` in the spec.
                        let is_mutate_param = self.meta(name)
                            .map(|m| m.is_mutate_param)
                            .unwrap_or(false);
                        // #270: a scalar `mutate` param's local is a pointer — the
                        // store must use the *scalar* size, not the pointer's, or it
                        // clobbers the adjacent field (e.g. `swap_fields(p.x, p.y)`).
                        let scalar_mutate = self.meta(name).and_then(|m| m.scalar_mutate_ptr.clone());
                        let store_through_ptr = is_mutate_param
                            && (scalar_mutate.is_some() || mutate_param_by_pointer(&dst_ty));
                        if store_through_ptr {
                            let store_size = match (&scalar_mutate, &dst_ty) {
                                (Some(sty), _) => Some(sty.size()),
                                (None, MirType::Struct(layout)) => Some(layout.byte_size),
                                (None, MirType::Enum(layout)) => Some(layout.byte_size),
                                (None, _) => Some(dst_ty.size()),
                            };
                            let _ = val_ty;
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                                addr: local_id,
                                offset: 0,
                                value: val_op,
                                store_size,
                            }));
                        } else {
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                                dst: local_id,
                                rvalue: MirRValue::Use(val_op),
                            }));
                        }
                        // After the write, not before: the slots have to hold the
                        // links before there is anything to record.
                        if struct_assigned {
                            let dst_ty = dst_ty.clone();
                            self.emit_struct_link_registration(local_id, &dst_ty);
                        }
                    }
                    // Field assignment: obj.field = value → Store at field offset
                    ExprKind::Field { object, field } => {
                        // #411: a place rooted at an aggregate local (`p.x`,
                        // `ln.a.x`, tuple fields) projects straight to base+offset
                        // as one store. This avoids loading an intermediate field
                        // as a value copy and losing the write on native.
                        if let Some((base, offset, fty, fsize)) = self.lower_place_chain(target) {
                            // An edge write is not a plain store: the rack
                            // records who points at whom, so `delete` can find
                            // this slot later and null it (mem.racks/RK3). The
                            // runtime does the recording and the store together,
                            // which is also what makes re-pointing an edge forget
                            // its old target.
                            if fty.is_link_slot() {
                                // A bare `none` on the right lowers as a tagged
                                // option whenever the checker hasn't settled its
                                // type, and the field is a niche — it wants the
                                // sentinel word. Without this, `n.peer = none`
                                // stored the *address* of the tagged local and
                                // the rack recorded an edge to the stack.
                                let val_op = self.wrap_sum_field_value(
                                    Some(&fty), fty.niche_none(), &val_ty, val_op,
                                );
                                self.emit_link_store(base, offset, val_op);
                                return Ok(());
                            }
                            // The field's own width, not None. Codegen only
                            // copies the bytes when the size says the value is
                            // wider than a pointer; with no size it stored the
                            // 8-byte pointer instead, so `s.text = "lit"` left
                            // the address of the constant in a 16-byte string
                            // field and the field read back as garbage.
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                                addr: base,
                                offset,
                                value: val_op,
                                store_size: fsize,
                            }));
                            return Ok(());
                        }
                        // A place rooted at an index, with any number of fields
                        // after it. #411 fixed the one-field forms; two dropped
                        // the write again, silently, on native only:
                        // `pool[h].health.current = 42` read back 100, and
                        // `v[i].a.x = 88` read back the original. The old code
                        // matched on `object` being the index, which only ever
                        // sees the last field, and everything deeper fell through
                        // to the generic path — which loads the intermediate
                        // aggregate as a value copy and stores into the copy.
                        let (index_base, path) = Self::peel_field_path(target);
                        if let ExprKind::Index { object: coll, index: idx } = &index_base.kind {
                            // A pool index is an address into the arena, so the
                            // whole path projects to one store. The generation
                            // check comes along, which is what a write through a
                            // stale handle should hit.
                            if self.index_object_is_pool(coll) {
                                // Ask for the slot's address rather than lowering
                                // `pool[h]` as an expression: for a scalar element
                                // that reads the *value*, and storing through 42
                                // is a segfault. Reading and writing want
                                // different things from the same syntax, so the
                                // write says which.
                                let elem_ty = self
                                    .ctx
                                    .lookup_raw_type(index_base.id)
                                    .map(|t| self.ctx.type_to_mir(t))
                                    .or_else(|| self.collection_elem_of_expr(coll));
                                let (coll_op, _) = self.lower_expr(coll)?;
                                let (idx_op, _) = self.lower_expr(idx)?;
                                let pool_local = self.as_local(coll_op);
                                let handle_local = self.as_local(idx_op);
                                let slot_addr = self.pool_slot_addr(pool_local, handle_local, MirType::Ptr);
                                // An empty path is the whole element — offset 0,
                                // its own width. `field_path_offset_ty` answers
                                // for a field path and had nothing to say here, so
                                // a scalar `pool[h] = v` fell out of this branch
                                // entirely and landed on `Vec_set(pool, handle, v)`
                                // — a handle used as an index, which is
                                // `index:32 | generation:32` and lands far out of
                                // range.
                                let store = elem_ty.as_ref().and_then(|ty| {
                                    if path.is_empty() {
                                        Some((0u32, ty.size() as u32))
                                    } else {
                                        self.field_path_offset_ty(ty, &path)
                                    }
                                });
                                if let Some((offset, fsize)) = store {
                                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                                        addr: slot_addr,
                                        offset,
                                        value: val_op,
                                        store_size: Some(fsize),
                                    }));
                                    return Ok(());
                                }
                            } else if self.is_vec_expr(coll) {
                                // Reading `v[i]` copies the element (value
                                // semantics for `let p = v[i]`), so a store into
                                // that copy is lost. Read-modify-writeback, the
                                // same path a `with vec[i] as item` binding uses.
                                let elem_ty = self
                                    .ctx
                                    .lookup_raw_type(index_base.id)
                                    .map(|t| self.ctx.type_to_mir(t))
                                    .or_else(|| self.collection_elem_of_expr(coll));
                                if let Some(elem_ty) = elem_ty {
                                    if let Some((offset, fsize)) = self.field_path_offset_ty(&elem_ty, &path) {
                                        let (coll_op, _) = self.lower_expr(coll)?;
                                        let (idx_op, _) = self.lower_expr(idx)?;
                                        let tmp = self.builder.alloc_temp(elem_ty);
                                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                                            dst: Some(tmp),
                                            func: FunctionRef::internal("Vec_index".to_string()),
                                            args: vec![coll_op.clone(), idx_op.clone()],
                                        }));
                                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                                            addr: tmp,
                                            offset,
                                            value: val_op,
                                            store_size: Some(fsize),
                                        }));
                                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                                            dst: None,
                                            func: FunctionRef::internal("Vec_set".to_string()),
                                            args: vec![coll_op, idx_op, MirOperand::Local(tmp)],
                                        }));
                                        return Ok(());
                                    }
                                }
                            }
                        }
                        let (obj_op, obj_ty) = self.lower_expr(object)?;
                        let offset = if let MirType::Struct(StructLayoutId { id, .. }) = &obj_ty {
                            if let Some(layout) = self.ctx.struct_layouts.get(*id as usize) {
                                layout.fields.iter()
                                    .find(|f| f.name == *field)
                                    .map(|f| f.offset)
                                    .unwrap_or(0)
                            } else { 0 }
                        } else if let Some((_, _, Some(bo), _)) = Self::resolve_tuple_field(&obj_ty, field) {
                            bo
                        } else {
                            // Base is a raw pointer (I64/Ptr) — field offset unknown.
                            // With correct element type tracking, pool[h] and pool.get(h)
                            // return Struct-typed results so this path shouldn't fire
                            // for pool operations.
                            0
                        };
                        let base_local = match obj_op {
                            MirOperand::Local(id) => id,
                            _ => {
                                let tmp = self.builder.alloc_temp(obj_ty);
                                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                                    dst: tmp,
                                    rvalue: MirRValue::Use(obj_op),
                                }));
                                tmp
                            }
                        };
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                            addr: base_local,
                            offset,
                            value: val_op,
                            store_size: None,
                        }));
                    }
                    // Index assignment: a[i] = val
                    ExprKind::Index { object, index } => {
                        // A pool index is a handle, not a position. Falling through
                        // to `Vec_set(pool, handle, v)` below used it as one, and a
                        // handle is `index:32 | generation:32` — so the store landed
                        // far outside the arena and `pool[h] = 7` segfaulted for any
                        // element type with no field to write (#719). The field-path
                        // case above already routed pools correctly; a bare element
                        // never reached it.
                        if self.index_object_is_pool(object) {
                            let elem_ty = self
                                .ctx
                                .lookup_raw_type(target.id)
                                .map(|t| self.ctx.type_to_mir(t))
                                .or_else(|| self.collection_elem_of_expr(object));
                            if let Some(elem_ty) = elem_ty {
                                let (pool_op, _) = self.lower_expr(object)?;
                                let (handle_op, _) = self.lower_expr(index)?;
                                let pool_local = self.as_local(pool_op);
                                let handle_local = self.as_local(handle_op);
                                let slot_addr =
                                    self.pool_slot_addr(pool_local, handle_local, MirType::Ptr);
                                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                                    addr: slot_addr,
                                    offset: 0,
                                    value: val_op,
                                    store_size: Some(elem_ty.size() as u32),
                                }));
                                return Ok(());
                            }
                        }

                        let (obj_op, obj_ty) = self.lower_expr(object)?;
                        let (idx_op, _) = self.lower_expr(index)?;

                        if let MirType::Array { ref elem, .. } = obj_ty {
                            // Fixed-size array: direct store at base + index * elem_size
                            if let MirOperand::Local(base_id) = obj_op {
                                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ArrayStore {
                                    base: base_id,
                                    index: idx_op,
                                    elem_size: elem.size(),
                                    value: val_op,
                                }));
                            }
                        } else {
                            // A map key is not a position. `Vec_set` on a map
                            // reached the native runtime as `rask_vec_set`,
                            // which takes arg 1 as an integer index — so
                            // `m["a"] = 1` used the key's address as the index
                            // and panicked with `index is 140728023345216 but
                            // length is 8`. The interpreter accepts a map
                            // receiver for `Vec_set`, which is why only native
                            // failed.
                            let setter = if self.is_map_expr(object) { "Map_set" } else { "Vec_set" };
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                                dst: None,
                                func: FunctionRef::internal(setter.to_string()),
                                args: vec![obj_op, idx_op, val_op],
                            }));
                        }
                    }
                    // Deref assignment: *ptr = value → Store through pointer
                    ExprKind::Unary { op: UnaryOp::Deref, operand } => {
                        let (addr_op, _) = self.lower_expr(operand)?;
                        let addr_local = match addr_op {
                            MirOperand::Local(id) => id,
                            _ => {
                                let tmp = self.builder.alloc_temp(MirType::Ptr);
                                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                                    dst: tmp,
                                    rvalue: MirRValue::Use(addr_op),
                                }));
                                tmp
                            }
                        };
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                            addr: addr_local,
                            offset: 0,
                            value: val_op,
                            store_size: None,
                        }));
                    }
                    // CT49 is a read — `value.("x")` resolves to a field access
                    // in expression position, and nothing says what writing
                    // through one means. Both backends reject it; say so in words
                    // rather than printing the AST at the user.
                    ExprKind::DynamicField { .. } => {
                        return Err(LoweringError::InvalidConstruct(
                            "can't assign through `value.(name)` — comptime field access reads a \
                             field, it doesn't name one to write to. Write the field directly."
                                .into(),
                        ));
                    }
                    _ => {
                        return Err(LoweringError::InvalidConstruct(format!(
                            "can't assign to a {} — an assignment target has to be a variable, \
                             a field, or an index",
                            rask_ast::expr::expr_kind_name(&target.kind)
                        )));
                    }
                }
                Ok(())
            }

            // While loop (spec L5)
            StmtKind::While { label, cond, body } => self.lower_while(label.as_deref(), cond, body),

            // For loop - desugar to while with iterator
            StmtKind::For {
                label,
                binding,
                mutate,
                iter,
                body,
            } => self.lower_for(label.as_deref(), binding, *mutate, iter, body),

            // Infinite loop
            StmtKind::Loop { label, body } => self.lower_loop(label.as_deref(), body),

            // Break
            StmtKind::Break { label, value } => self.lower_break(label.as_deref(), value.as_ref()),

            // Continue
            StmtKind::Continue(label) => self.lower_continue(label.as_deref()),

            // Tuple destructuring
            StmtKind::MutTuple { patterns, init }
            | StmtKind::LetTuple { patterns, init } => {
                // Flattening the pattern to a name list loses the shape: a
                // nested `((a, b), c)` then read a, b and c off the *outer*
                // tuple at flat positions 0, 1, 2, and a wildcard shifted
                // everything after it up one (#442). Only a pattern that's
                // already flat can use the name-list path — which is also where
                // channel handles and type-prefix tracking live.
                if patterns.iter().all(|p| matches!(p, TuplePat::Name(_))) {
                    let names: Vec<String> = rask_ast::stmt::tuple_pats_flat_names(patterns)
                        .into_iter().map(|s| s.to_string()).collect();
                    self.lower_tuple_destructure(&names, init)
                } else {
                    let (init_op, init_ty) = self.lower_expr(init)?;
                    self.destructure_tuple_pattern(patterns, &init_op, &init_ty)
                }
            }

            // Struct destructuring binding: `let Point { x, .. } = p`.
            StmtKind::LetStruct { pattern, init, .. } => {
                self.lower_struct_destructure(pattern, init)
            }

            // While-let pattern loop
            StmtKind::WhileLet { label, pattern, expr, body } => {
                let check_block = self.builder.create_block();
                let body_block = self.builder.create_block();
                let exit_block = self.builder.create_block();

                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: check_block }));

                self.builder.switch_to_block(check_block);
                let (val, val_ty) = self.lower_expr(expr)?;
                let tag = self.builder.alloc_temp(MirType::U8);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: tag,
                    rvalue: MirRValue::EnumTag { value: val.clone() },
                }));
                // Compare tag against expected variant. Use type-context resolution
                // so `while c() is Reading as r` against `Reading or RecvErr` routes
                // to the ok side (tag 0) instead of the bare `pattern_tag`'s
                // capitalization guess (uppercase ⇒ tag 1, which is the err side).
                let expected = self.pattern_tag_in_type_context(pattern, &val_ty);
                let matches = self.builder.alloc_temp(MirType::Bool);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: matches,
                    rvalue: MirRValue::BinaryOp {
                        op: crate::operand::BinOp::Eq,
                        left: MirOperand::Local(tag),
                        right: MirOperand::Constant(crate::operand::MirConst::Int(expected)),
                    },
                }));
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: MirOperand::Local(matches),
                    then_block: body_block,
                    else_block: exit_block,
                }));

                self.builder.switch_to_block(body_block);
                // Bind payload variables from the pattern
                // Optional: a Constructor pattern on a user enum takes each
                // field's type from the enum layout and never looks at this.
                let payload_ty = self.payload_type_of(expr, &val_ty);
                self.bind_pattern_payload(pattern, val, payload_ty, &val_ty);
                let ensure_depth = self.ensure_stack.len();
                self.loop_stack.push(LoopContext {
                    label: label.clone(),
                    continue_block: check_block,
                    exit_block,
                    result_local: None,
                    ensure_depth,
                });
                for s in body {
                    self.lower_stmt(s)?;
                }
                self.close_loop_body(ensure_depth, check_block);
                self.loop_stack.pop();
                self.ensure_stack.truncate(ensure_depth);

                self.builder.switch_to_block(exit_block);
                Ok(())
            }

            // Ensure (EN1–EN7): schedule cleanup to run at scope exit.
            // Body is lowered into a cleanup block; CleanupReturn terminators
            // at return/try sites chain through these blocks.
            StmtKind::Ensure { body, else_handler } => {
                let cleanup_block = self.builder.create_block();
                let continue_block = self.builder.create_block();

                // Marker for MIRI/analysis
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::EnsurePush { cleanup_block }));

                // C1/C2: extract receiver variable from body (e.g. `ensure tx.rollback()` → "tx").
                // Register a resource_id so consumption can be tracked at runtime.
                let receiver_name = Self::extract_ensure_receiver(body);
                if let Some(ref name) = receiver_name {
                    if let Some((local_id, _)) = self.locals.get(name) {
                        let resource_id = self.builder.alloc_local(
                            format!("__ensure_res_{}", cleanup_block.0),
                            MirType::I64,
                        );
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ResourceRegister {
                            dst: resource_id,
                            type_name: name.clone(),
                            scope_depth: 0,
                        }));
                        self.ensure_receivers.insert(cleanup_block, (name.clone(), resource_id));
                        // Store resource_id in local_meta so method calls on this
                        // receiver can find it for ResourceConsume.
                        self.meta_mut(name).resource_id = Some(resource_id);
                    }
                }
                self.ensure_stack.push(cleanup_block);

                // Reify a runtime hook so the body also runs if the scope unwinds
                // on a native panic (ctrl.panic/U1). Registered in the main flow at
                // schedule time; popped by the inline cleanup on a normal exit, so
                // only a panic reaches it via rask_ensure_run_all.
                let hook_resource = self.ensure_receivers.get(&cleanup_block).map(|(_, id)| *id);
                let hook = self.try_reify_ensure_hook(body, else_handler, hook_resource);
                if let Some((thunk, captures)) = &hook {
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::EnsureHookRegister {
                        thunk: thunk.clone(),
                        captures: captures.clone(),
                    }));
                }

                // Main flow skips to continue block (body runs at scope exit)
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: continue_block }));

                // Lower ensure body into cleanup block.
                // C1/C2: if receiver has a resource_id, check consumption first.
                self.builder.switch_to_block(cleanup_block);
                // Normal exit runs the inline cleanup — deregister the panic hook
                // first so it can't double-run.
                if hook.is_some() {
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::EnsureHookPop));
                }
                if let Some((_, resource_id)) = receiver_name
                    .as_ref()
                    .and_then(|name| self.ensure_receivers.get(&cleanup_block).cloned())
                {
                    // Check if resource was consumed → skip cleanup
                    let consumed = self.builder.alloc_temp(MirType::I64);
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: Some(consumed),
                        func: crate::FunctionRef::internal("rask_resource_is_consumed".to_string()),
                        args: vec![MirOperand::Local(resource_id)],
                    }));
                    let body_block = self.builder.create_block();
                    let skip_block = self.builder.create_block();
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                        cond: MirOperand::Local(consumed),
                        then_block: skip_block,
                        else_block: body_block,
                    }));
                    // skip_block: sentinel (consumed → skip cleanup)
                    self.builder.switch_to_block(skip_block);
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Unreachable));
                    // body_block: run the actual cleanup
                    self.builder.switch_to_block(body_block);
                }

                for s in body {
                    self.lower_stmt(s)?;
                }

                if let Some((param_name, handler_body)) = else_handler {
                    self.lower_ensure_else_handler(param_name, handler_body)?;
                }
                // Sentinel for the end of the cleanup sub-CFG: control never
                // falls out of a cleanup block, it is spliced in at each exit.
                if self.builder.current_block_unterminated() {
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Unreachable));
                }

                self.builder.switch_to_block(continue_block);
                Ok(())
            }

            // Discard (D1): value dropped, binding invalidated.
            // At MIR level this is a no-op — the value becomes dead.
            // Ownership checker handles use-after-discard errors.
            StmtKind::Discard { name, .. } => {
                // If the local exists, remove it from scope so later
                // references fail during lowering rather than silently.
                self.locals.remove(name);
                Ok(())
            }

            // Comptime (compile-time evaluated)
            StmtKind::Comptime(stmts) => {
                // A condition the comptime interpreter can't see, because the
                // fact lives in lowering: `field.has<A>()` inside an unrolled
                // `comptime for` (type.annotations/AN6). Asked first — the
                // interpreter would only fail on it.
                if let Some(taken) = self.try_eval_comptime_if_locally(stmts) {
                    for s in taken {
                        self.lower_stmt(s)?;
                    }
                    return Ok(());
                }
                // Try to evaluate comptime if at compile time (CC1)
                if let Some(ref interp_cell) = self.ctx.comptime_interp {
                    if let Some(taken) = self.try_eval_comptime_if(stmts, interp_cell)? {
                        for s in taken {
                            self.lower_stmt(s)?;
                        }
                        return Ok(());
                    }
                }
                for s in stmts {
                    self.lower_stmt(s)?;
                }
                Ok(())
            }

            // CT48/CT50/CT63: unrolled right here. By the time MIR sees a
            // monomorphized function, mono has already substituted T to a
            // concrete type on `reflect.fields<T>()` — that's what makes this
            // "per instantiation" without a separate mono-side pass.
            StmtKind::ComptimeFor { binding, iter, body } => {
                self.lower_comptime_for(binding, iter, body)
            }
        }
    }

    /// Lower a nested body's statements with the bindings it introduces scoped
    /// to it.
    ///
    /// `lower_block` snapshots `locals` for a braced block, but a loop body is
    /// lowered statement-by-statement and never goes through it. That's
    /// invisible for `locals` — the checker settled every name long before —
    /// and not for comptime-known strings, where a `let w = "limit"` shadowing
    /// an outer `let w = "spent"` outlived its loop and changed which field a
    /// later `value.(w)` read. Restores on the error path too, so a body that
    /// fails to lower doesn't leave its names behind.
    ///
    /// Every loop body goes through here rather than each writing the restore
    /// out, so the next one added gets it without anyone remembering to.
    fn lower_body_scoped(&mut self, body: &[Stmt]) -> Result<(), LoweringError> {
        let saved = self.comptime_strings.clone();
        let mut result = Ok(());
        for stmt in body {
            result = self.lower_stmt(stmt);
            if result.is_err() {
                break;
            }
        }
        self.comptime_strings = saved;
        result
    }

    /// CT48: fully unroll a `comptime for` — one copy of `body` per field,
    /// with the loop binding tracked in `comptime_for_bindings` so `field.xxx`
    /// and `value.(field.xxx)` inside the body splice as compile-time
    /// constants (CT49) instead of going through normal local lookup.
    fn lower_comptime_for(
        &mut self,
        binding: &ForBinding,
        iter: &Expr,
        body: &[Stmt],
    ) -> Result<(), LoweringError> {
        let name = match binding {
            ForBinding::Single(name) => name.clone(),
            ForBinding::Tuple(_) => {
                return Err(LoweringError::InvalidConstruct(
                    "comptime for requires a single binding — the iterable is a list of fields, not tuples".into(),
                ));
            }
        };

        let fields = self.reflect_fields_for(iter)?;

        for field in fields {
            self.comptime_for_bindings.push((name.clone(), field));
            self.lower_body_scoped(body)?;
            self.comptime_for_bindings.pop();
        }
        Ok(())
    }

    /// CT51: the iterable must be comptime-known. The only form implemented
    /// so far — matching the interpreter (rask-interp/src/stdlib/reflect.rs)
    /// — is `reflect.fields<T>()`, T already concrete after mono substitution.
    fn reflect_fields_for(
        &self,
        iter: &Expr,
    ) -> Result<Vec<super::ReflectFieldConst>, LoweringError> {
        use rask_ast::decl::field_attrs;

        let unsupported = || {
            LoweringError::InvalidConstruct(
                "comptime for requires a comptime-known iterable, e.g. reflect.fields<T>()".into(),
            )
        };

        let ExprKind::MethodCall { object, method, type_args, .. } = &iter.kind else {
            return Err(unsupported());
        };
        let is_reflect_fields = method == "fields"
            && matches!(&object.kind, ExprKind::Ident(n) if n == "reflect");
        if !is_reflect_fields {
            return Err(unsupported());
        }
        let type_name = type_args
            .as_ref()
            .and_then(|ta| ta.first())
            .ok_or_else(unsupported)?;

        let Some((_, layout)) = self.ctx.find_struct(type_name) else {
            // The uninstantiated generic template gets lowered too (alongside
            // every real instantiation), with its type params still literal
            // ("T"). It's never actually run — type.generics/G2 treats an
            // uninstantiated generic body as a template, not code that has to
            // fully resolve — so a bare single-letter placeholder unrolls to
            // nothing instead of hard-erroring the whole compile. A real typo
            // or a non-struct type still errors.
            if is_bare_type_param(type_name) {
                return Ok(Vec::new());
            }
            return Err(LoweringError::InvalidConstruct(format!(
                "reflect.fields<{}>(): not a struct type",
                type_name
            )));
        };

        Ok(layout
            .fields
            .iter()
            .map(|fl| {
                let type_name = match &fl.ty {
                    rask_types::Type::Named(id) => self
                        .ctx
                        .type_names
                        .get(id)
                        .cloned()
                        .unwrap_or_else(|| format!("{}", fl.ty)),
                    other => format!("{}", other),
                };
                super::ReflectFieldConst {
                    name: fl.name.clone(),
                    type_name,
                    offset: fl.offset,
                    size: fl.size,
                    is_public: fl.is_public,
                    serial_name: field_attrs::serial_name(&fl.attrs, &fl.name),
                    is_skipped: field_attrs::is_skipped(&fl.attrs),
                    has_default: fl.has_declared_default
                        || field_attrs::default_literal(&fl.attrs).is_some(),
                    attrs: fl.attrs.clone(),
                }
            })
            .collect())
    }

    /// A `comptime if` whose condition only lowering can answer.
    ///
    /// `field.has<A>()` is a fact about the loop iteration being unrolled, and
    /// that state lives here, not in the comptime interpreter — so the
    /// interpreter reports "unknown method" and the branch used to survive as a
    /// runtime `if` on a constant. Cranelift folded the test, but both branches
    /// were still lowered, and lowering the untaken one is not harmless:
    /// `field.get<A>().weight` in the body has no annotation to read on a field
    /// that doesn't carry it (type.annotations/AN6). Folding here is what makes
    /// `comptime if` actually remove the branch.
    ///
    /// Returns the taken branch's statements, or `None` when the condition
    /// isn't one of these — the caller then tries the comptime interpreter.
    fn try_eval_comptime_if_locally<'b>(&mut self, stmts: &'b [Stmt]) -> Option<&'b [Stmt]> {
        let (cond, then_branch, else_branch) = Self::as_comptime_if(stmts)?;
        let taken = self.eval_local_comptime_cond(cond)?;
        Self::comptime_branch(taken, then_branch, else_branch)
    }

    /// `comptime { if cond { … } else { … } }` — the only shape either
    /// evaluator handles.
    fn as_comptime_if(stmts: &[Stmt]) -> Option<(&Expr, &Expr, &Option<Box<Expr>>)> {
        if stmts.len() != 1 {
            return None;
        }
        let StmtKind::Expr(inner) = &stmts[0].kind else { return None };
        let ExprKind::If { cond, then_branch, else_branch, .. } = &inner.kind else {
            return None;
        };
        Some((cond, then_branch, else_branch))
    }

    /// The statements of whichever branch a decided condition selects. An
    /// undecidable branch shape gives `None`, and a false condition with no
    /// `else` gives an empty slice — the branch is gone, not un-lowered.
    fn comptime_branch<'b>(
        taken: bool,
        then_branch: &'b Expr,
        else_branch: &'b Option<Box<Expr>>,
    ) -> Option<&'b [Stmt]> {
        let chosen = if taken {
            then_branch
        } else {
            match else_branch {
                Some(e) => e,
                None => return Some(&[]),
            }
        };
        match &chosen.kind {
            ExprKind::Block(block_stmts) => Some(block_stmts),
            _ => None,
        }
    }

    /// A condition lowering can decide on its own: `binding.has<A>()`, and `!`
    /// / `&&` / `||` over those. Anything else is `None`.
    fn eval_local_comptime_cond(&mut self, cond: &Expr) -> Option<bool> {
        match &cond.kind {
            ExprKind::MethodCall { object, method, type_args, .. } => {
                let (op, _) = self.comptime_field_method_const(object, method, type_args)?;
                match op {
                    MirOperand::Constant(MirConst::Bool(b)) => Some(b),
                    _ => None,
                }
            }
            ExprKind::Unary { op: rask_ast::expr::UnaryOp::Not, operand } => {
                Some(!self.eval_local_comptime_cond(operand)?)
            }
            ExprKind::Binary { op, left, right } => {
                let (l, r) = (
                    self.eval_local_comptime_cond(left)?,
                    self.eval_local_comptime_cond(right)?,
                );
                match op {
                    rask_ast::expr::BinOp::And => Some(l && r),
                    rask_ast::expr::BinOp::Or => Some(l || r),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Try to evaluate a `comptime if` block at compile time.
    ///
    /// Returns `Some(stmts)` with the taken branch's statements if the condition
    /// evaluates successfully, or `None` if the block isn't a comptime if pattern.
    fn try_eval_comptime_if<'b>(
        &self,
        stmts: &'b [Stmt],
        interp_cell: &std::cell::RefCell<rask_comptime::ComptimeInterpreter>,
    ) -> Result<Option<&'b [Stmt]>, LoweringError> {
        // Pattern: comptime { if cond { then } else { else } }
        if stmts.len() != 1 {
            return Ok(None);
        }
        let inner = match &stmts[0].kind {
            StmtKind::Expr(e) => e,
            _ => return Ok(None),
        };
        let (cond, then_branch, else_branch) = match &inner.kind {
            ExprKind::If { cond, then_branch, else_branch, .. } => (cond, then_branch, else_branch),
            _ => return Ok(None),
        };

        let mut interp = interp_cell.borrow_mut();
        match interp.eval_expr(cond) {
            Ok(val) => {
                let taken = val.as_bool().unwrap_or(false);
                if taken {
                    // Lower the then branch — it's a Block(stmts) expression
                    if let ExprKind::Block(block_stmts) = &then_branch.kind {
                        Ok(Some(block_stmts))
                    } else {
                        Ok(None)
                    }
                } else if let Some(else_br) = else_branch {
                    if let ExprKind::Block(block_stmts) = &else_br.kind {
                        Ok(Some(block_stmts))
                    } else {
                        Ok(None)
                    }
                } else {
                    // No else branch, condition is false — emit nothing
                    Ok(Some(&[]))
                }
            }
            Err(_) => {
                // Condition not evaluable — fall through to normal lowering
                Ok(None)
            }
        }
    }

    /// Lower a let/const binding: evaluate init, assign to a new local.
    fn lower_binding(&mut self, name: &str, ty: Option<&str>, init: &Expr) -> Result<(), LoweringError> {
        let is_closure = matches!(&init.kind, ExprKind::Closure { .. });
        // Ask before lowering: `own` is gone from the operand by then, and the
        // type says nothing (OW5 erases `Owned<T>` to `T`), so this is the only
        // point where "the value in this local is a heap box" is knowable (#739).
        let init_may_be_box = self.expr_yields_owned_box(init);
        let (init_op, inferred_ty) = self.lower_expr(init)?;

        // `let p = own Big { … }` takes over the box rather than copying out of
        // it. A struct-typed destination copies its bytes on assignment, which is
        // right for every other aggregate and wrong here: it left `p` naming a
        // stack copy and orphaned the heap value, so `drop(p)` had a stack address
        // to free and a field storing `p` held an address that dangled at scope
        // exit (#739). The binding aliases the pointer instead.
        //
        // The pointer is the test, not the `own`: a scalar `Owned` was never boxed
        // — it fits the slot — so `let ptr: Owned<i32> = own 42` is an ordinary
        // binding holding 42, and marking it a box would have `drop` free the
        // address 42.
        if init_may_be_box {
            if let MirOperand::Local(src) = init_op {
                if matches!(self.builder.local_type(src), Some(MirType::Ptr)) {
                    let var_ty = ty
                        .map(|s| self.ctx.resolve_type_str(s))
                        .unwrap_or(inferred_ty);
                    self.builder.name_local(src, name.to_string());
                    self.locals.insert(name.to_string(), (src, var_ty));
                    self.meta_mut(name).is_owned_box = true;
                    return Ok(());
                }
            }
        }
        let var_ty = ty.map(|s| self.ctx.resolve_type_str(s)).unwrap_or(inferred_ty.clone());
        let local_id = self.builder.alloc_local(name.to_string(), var_ty.clone());
        self.locals.insert(name.to_string(), (local_id, var_ty.clone()));
        // An annotated binding is a coercion site like any other: `let b: T?? = t`
        // needs both layers built here rather than left for codegen to infer from
        // the depth mismatch, which only ever added Option layers and typed the
        // payload one layer too shallow (#637).
        let init_op = self.coerce_into_wrapper(
            rask_ast::coercion::CoercionSite::AnnotatedBinding,
            init_op, &inferred_ty, &var_ty,
        );
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: local_id,
            rvalue: MirRValue::Use(init_op.clone()),
        }));
        // The struct may carry edges nothing has recorded — a literal writes its
        // fields before the value has anywhere to live, so there was no slot to
        // register (mem.racks/RK3). Now there is.
        self.emit_struct_link_registration(local_id, &var_ty);
        // A fused `collect()` records its element type against the local it
        // built; carry it onto the binding so `for v in page` iterates the right
        // stride and dispatches methods on the right type.
        if let MirOperand::Local(src) = &init_op {
            if let Some(elem) = self.collected_elem_types.get(src).cloned() {
                self.collected_elem_types.insert(local_id, elem.clone());
                self.meta_mut(name).elem_type = Some(elem);
            }
        }

        // Track collection element types for for-in iteration heuristics
        if let ExprKind::MethodCall { object, method, .. } = &init.kind {
            if let ExprKind::Ident(obj_name) = &object.kind {
                match (obj_name.as_str(), method.as_str()) {
                    ("cli", "args") | ("fs", "read_lines") => {
                        self.meta_mut(name).elem_type = Some(MirType::String);
                    }
                    ("fs", "read_bytes") => {
                        self.meta_mut(name).elem_type = Some(MirType::U8);
                    }
                    _ => {}
                }
            }
            // String methods that always return Vec<string>
            match method.as_str() {
                "lines" | "split" | "split_whitespace" | "graphemes" => {
                    self.meta_mut(name).elem_type = Some(MirType::String);
                }
                _ => {}
            }
        }

        // Track stdlib type prefix for variables assigned from type constructors,
        // known module functions, or method calls on tracked variables,
        // so later method calls dispatch correctly.
        // Unwrap try/unwrap wrappers to see the underlying expression.
        let init_inner = match &init.kind {
            ExprKind::Try { expr, .. } => expr.as_ref(),
            ExprKind::Unwrap { expr, .. } => expr.as_ref(),
            _ => init,
        };
        if let ExprKind::MethodCall { object, method, .. } = &init_inner.kind {
            if let ExprKind::Ident(obj_name) = &object.kind {
                if super::is_type_constructor_name(obj_name) {
                    // Type.method() → prefix is the type name.
                    // Covers stdlib (Vec, Map, string) and user types (Person, Document).
                    // Strip generic args: Map<string, JsonValue> → Map
                    let base_name = obj_name.split('<').next().unwrap_or(obj_name);
                    let is_module = rask_stdlib::mir_metadata::stdlib_module_names()
                        .contains(base_name);
                    if !is_module && (super::MirContext::stdlib_type_prefix(
                        &rask_types::Type::UnresolvedNamed(base_name.to_string())
                    ).is_some()
                        || base_name.chars().next().map_or(false, |c| c.is_uppercase()))
                    {
                        self.meta_mut(name).type_prefix = Some(base_name.to_string());
                    } else {
                        // Module function (fs.open) → check return type prefix
                        let func_name = format!("{}_{}", obj_name, method);
                        if let Some(prefix) = super::func_return_type_prefix(&func_name) {
                            self.meta_mut(name).type_prefix = Some(prefix.to_string());
                        }
                    }
                } else if let Some(obj_prefix) = self.meta(obj_name).and_then(|m| m.type_prefix.clone()) {
                    // Instance method on tracked variable (file.lines() → File_lines)
                    let func_name = format!("{}_{}", obj_prefix, method);
                    if let Some(prefix) = super::func_return_type_prefix(&func_name) {
                        self.meta_mut(name).type_prefix = Some(prefix.to_string());
                    }
                    // Propagate full generic type through clone (Shared, Sender, Receiver)
                    if method == "clone" {
                        if let Some(full_ty) = self.meta(obj_name).and_then(|m| m.full_type.clone()) {
                            self.meta_mut(name).full_type = Some(full_ty);
                        }
                    }
                }
            }
            // Module.Type.method() pattern: http.HttpServer.listen() → prefix "HttpServer"
            if let ExprKind::Field { object: inner_obj, field: type_name } = &object.kind {
                if let ExprKind::Ident(module_name) = &inner_obj.kind {
                    if !self.locals.contains_key(module_name)
                        && super::is_type_constructor_name(module_name)
                    {
                        self.meta_mut(name).type_prefix = Some(type_name.clone());
                    }
                }
            }
        }
        // Track full generic type for Shared.new(data) calls:
        // infer inner type from the constructor argument.
        if let ExprKind::MethodCall { object, method, args: call_args, .. } = &init.kind {
            if let ExprKind::Ident(obj_name) = &object.kind {
                if obj_name == "Shared" && method == "new" && !call_args.is_empty() {
                    // Infer inner type from the first arg
                    let inner_name = match &call_args[0].expr.kind {
                        ExprKind::StructLit { name: sn, .. } => Some(sn.clone()),
                        ExprKind::MethodCall { object: inner_obj, .. } => {
                            if let ExprKind::Ident(tn) = &inner_obj.kind {
                                if tn.chars().next().map_or(false, |c| c.is_uppercase()) {
                                    Some(tn.clone())
                                } else { None }
                            } else { None }
                        }
                        ExprKind::Ident(vn) => {
                            // Look up variable type from local_meta
                            self.meta(vn).and_then(|m| m.type_prefix.clone())
                        }
                        _ => None,
                    };
                    if let Some(inner) = inner_name {
                        self.meta_mut(name).full_type = Some(
                            format!("Shared<{}>", inner),
                        );
                    }
                }
            }
        }
        // Track element type for Pool<T>.new() constructors:
        // let pool = Pool<Node>.new() → collection_elem_types["pool"] = Struct(Node)
        if let ExprKind::MethodCall { object, method, .. } = &init.kind {
            if let ExprKind::Ident(obj_name) = &object.kind {
                let base_name = obj_name.split('<').next().unwrap_or(obj_name);
                if base_name == "Pool" && method == "new" {
                    if let Some(inner) = obj_name.split('<').nth(1).and_then(|s| s.strip_suffix('>')) {
                        let elem_mir = self.ctx.resolve_type_str(inner);
                        if !matches!(elem_mir, MirType::Ptr | MirType::I64) {
                            self.meta_mut(name).elem_type = Some(elem_mir);
                        }
                    }
                }
            }
        }
        // Iterator terminal .collect() returns a Vec
        if let ExprKind::MethodCall { method, .. } = &init.kind {
            if method == "to_vec" {
                self.meta_mut(name).type_prefix = Some("Vec".to_string());
            }
        }
        // Also track for simple function calls (e.g. cli.args())
        if let ExprKind::Call { func, .. } = &init.kind {
            if let ExprKind::Ident(func_name) = &func.kind {
                if let Some(prefix) = super::func_return_type_prefix(func_name) {
                    self.meta_mut(name).type_prefix = Some(prefix.to_string());
                }
            }
        }
        // Index expression: args[1] → if args has known element type, propagate it
        if let ExprKind::Index { object, .. } = &init.kind {
            if let ExprKind::Ident(coll_name) = &object.kind {
                if let Some(elem_ty) = self.meta(coll_name).and_then(|m| m.elem_type.clone()) {
                    if let Some(prefix) = self.mir_type_name(&elem_ty) {
                        self.meta_mut(name).type_prefix = Some(prefix);
                    }
                }
            }
        }

        // Aliasing a variable (`let players = __ctx_players`) carries its type
        // metadata forward, so pool/collection indexing on the alias still
        // resolves. Used by the hidden-param pass's SIG2 named-context alias,
        // and correct for any `const b = a` where `a` is a tracked container.
        if let ExprKind::Ident(src) = &init.kind {
            let carried = self.meta(src).map(|m| {
                (m.type_prefix.clone(), m.full_type.clone(), m.elem_type.clone())
            });
            if let Some((prefix, full, elem)) = carried {
                if let Some(p) = prefix {
                    self.meta_mut(name).type_prefix = Some(p);
                }
                if let Some(f) = full {
                    self.meta_mut(name).full_type = Some(f);
                }
                if let Some(e) = elem {
                    self.meta_mut(name).elem_type = Some(e);
                }
            }
        }

        // Fallback: derive prefix from the MIR type (catches String, Struct, Enum)
        // or from the type annotation string (catches Ptr types like Vec<T>, Map<K,V>)
        if self.meta(name).and_then(|m| m.type_prefix.as_ref()).is_none() {
            if let Some(prefix) = self.mir_type_name(&var_ty) {
                self.meta_mut(name).type_prefix = Some(prefix);
            } else if let Some(ty_str) = ty {
                if let Some(prefix) = super::type_prefix_from_str(ty_str) {
                    self.meta_mut(name).type_prefix = Some(prefix);
                }
            }
        }

        // Track collection element types from type annotations (Vec<u8>, Pool<T>)
        if self.meta(name).and_then(|m| m.elem_type.as_ref()).is_none() {
            if let Some(ty_str) = ty {
                if let Some(elem_str) = ty_str.strip_prefix("Vec<").and_then(|s| s.strip_suffix('>')) {
                    let elem_mir = self.ctx.resolve_type_str(elem_str);
                    self.meta_mut(name).elem_type = Some(elem_mir);
                } else if let Some(elem_str) = ty_str.strip_prefix("Pool<").and_then(|s| s.strip_suffix('>')) {
                    let elem_mir = self.ctx.resolve_type_str(elem_str);
                    self.meta_mut(name).elem_type = Some(elem_mir);
                }
            }
        }

        // Unannotated binding from a call: take the element type from the
        // initializer's own type, then from the callee's declared return type.
        //
        // Without this `const rows = build()` where `build() -> Vec<Ranked>`
        // left the element type unknown, so `for r in rows` typed the loop
        // variable i64 and Vec_get's scalar deref read the element's first
        // 8 bytes as a pointer (#478).
        if self.meta(name).and_then(|m| m.elem_type.as_ref()).is_none() {
            let from_init = self
                .ctx
                .lookup_raw_type(init.id)
                .and_then(|t| self.vec_elem_of_checker_type(t));
            let from_callee = || match &init.kind {
                ExprKind::Call { func, .. } => match &func.kind {
                    ExprKind::Ident(callee) => {
                        let key = self.ctx.call_rewrites.get(&init.id).cloned()
                            .unwrap_or_else(|| callee.clone());
                        self.func_sigs.get(&key).and_then(|s| s.ret_vec_elem.clone())
                    }
                    _ => None,
                },
                ExprKind::MethodCall { object, method, args, .. } => {
                    // `Vec.from([1, 2, 3])` — the receiver is the type name, not
                    // a tracked local, so the prefix lookup below finds nothing.
                    // Take the element type off the argument instead. Left
                    // unknown, the loop element defaulted to a pointer and
                    // `|x| { return x + 1 }` compiled to pointer arithmetic:
                    // x + 8 instead of x + 1.
                    if matches!(&object.kind, ExprKind::Ident(n) if n == "Vec")
                        && method == "from"
                    {
                        if let Some(arg) = args.first() {
                            if let Some(elem) = self.iterable_elem_of(&arg.expr) {
                                return Some(elem);
                            }
                        }
                    }
                    let prefix = match &object.kind {
                        ExprKind::Ident(n) => self.meta(n).and_then(|m| m.type_prefix.clone()),
                        _ => None,
                    }?;
                    let base = prefix.split('<').next().unwrap_or(&prefix).trim();
                    self.func_sigs
                        .get(&format!("{}_{}", base, method))
                        .and_then(|s| s.ret_vec_elem.clone())
                }
                _ => None,
            };
            if let Some(elem) = from_init.or_else(from_callee) {
                self.meta_mut(name).elem_type = Some(elem);
            }
        }

        // Track closure bindings and alias the func_sig so callers can
        // look up the return type by variable name.
        if is_closure {
            self.closure_locals.insert(name.to_string());
            let closure_fn = format!("{}__closure_{}", self.parent_name, self.closure_counter - 1);
            if let Some(sig) = self.func_sigs.get(&closure_fn).cloned() {
                self.func_sigs.insert(name.to_string(), sig);
            }
        }

        // Track locals assigned from calls that return closure types.
        // e.g., `const add_5 = make_adder(5)` where make_adder returns |i32| -> i32.
        // Check via type checker's node_types: if init expr has Type::Fn, it's a closure.
        if !is_closure {
            // A `Sequence<T>` counts: `let chain = src.filter(p)` binds
            // something callable, and calling it is how the next adapter drives
            // it (type.sequence/SEQ1 makes it nominal, so the `Type::Fn` test
            // alone doesn't see it).
            let bound_ret = self.ctx.node_types.get(&init.id)
                .and_then(|ty| self.ctx.callable_ret_ty(ty, self.ctx.type_names));
            if let Some(ret_mir) = bound_ret {
                self.closure_locals.insert(name.to_string());
                self.func_sigs.insert(name.to_string(), super::FuncSig { ret_ty: ret_mir, scalar_mutate_params: Vec::new(), aggregate_mutate_params: Vec::new(), ret_vec_elem: None, param_ty_strs: Vec::new() });
            }
        }

        // Propagate Vec element types from "self.field" to "<name>.field"
        // so struct field access like `state.data.get(i)` finds the right type.
        if let ExprKind::StructLit { fields, .. } = &init.kind {
            let shared = self.ctx.shared_elem_types.borrow();
            let mut to_add = Vec::new();
            for field in fields {
                let self_key = format!("self.{}", field.name);
                if let Some(elem_ty) = shared.get(&self_key) {
                    let var_key = format!("{}.{}", name, field.name);
                    to_add.push((var_key, elem_ty.clone()));
                }
                // Also check if the source variable directly has an element type
                if let ExprKind::Ident(src_var) = &field.value.kind {
                    if let Some(elem_ty) = self.meta(src_var).and_then(|m| m.elem_type.as_ref())
                        .or_else(|| shared.get(src_var))
                    {
                        let var_key = format!("{}.{}", name, field.name);
                        to_add.push((var_key, elem_ty.clone()));
                    }
                }
            }
            drop(shared);
            for (key, ty) in to_add {
                self.meta_mut(&key).elem_type = Some(ty.clone());
                self.ctx.record_shared_elem(key, ty);
            }
        }

        Ok(())
    }

    /// Lower tuple destructuring: evaluate init, extract each element by field index.
    /// Bind a tuple pattern against `base`, following the pattern's shape.
    /// A nested pattern reads its own sub-tuple out first, so element indices
    /// always match the tuple they're read from.
    pub(super) fn destructure_tuple_pattern(
        &mut self,
        pats: &[TuplePat],
        base: &MirOperand,
        base_ty: &MirType,
    ) -> Result<(), LoweringError> {
        let elem_types = match base_ty {
            MirType::Tuple(fields) => Some(fields.clone()),
            _ => None,
        };
        for (i, pat) in pats.iter().enumerate() {
            if matches!(pat, TuplePat::Wildcard) {
                continue;
            }
            let elem_ty = elem_types.as_ref()
                .and_then(|f| f.get(i).cloned())
                .unwrap_or_else(|| crate::fallback::i64_fallback("lower/stmt:1220"));
            let dst = match pat {
                TuplePat::Name(name) => {
                    let local_id = self.builder.alloc_local(name.clone(), elem_ty.clone());
                    self.locals.insert(name.clone(), (local_id, elem_ty.clone()));
                    local_id
                }
                _ => self.builder.alloc_temp(elem_ty.clone()),
            };
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst,
                rvalue: MirRValue::Field {
                    base: base.clone(),
                    field_index: i as u32,
                    byte_offset: None,
                    access: FieldAccess::Word,
                },
            }));
            if let TuplePat::Nested(inner) = pat {
                self.destructure_tuple_pattern(inner, &MirOperand::Local(dst), &elem_ty)?;
            }
        }
        Ok(())
    }

    /// `let Point { x, y } = p` — read each named field into its binding.
    ///
    /// One read per field the pattern names, which is what the source says: the
    /// alternative, binding the whole struct and projecting later, would make the
    /// bindings views into `p` rather than values of their own. Nested patterns
    /// don't reach here — a destructuring *binding* can't fail, so the parser only
    /// accepts names and `..` inside one.
    fn lower_struct_destructure(
        &mut self,
        pattern: &Pattern,
        init: &Expr,
    ) -> Result<(), LoweringError> {
        let Pattern::Struct { fields, .. } = pattern else {
            return Err(LoweringError::InvalidConstruct(
                "destructuring binding needs a struct pattern".into(),
            ));
        };
        let (src_op, src_ty) = self.lower_expr(init)?;
        for (field_name, field_pat) in fields {
            let Pattern::Ident(binding) = field_pat else {
                return Err(LoweringError::InvalidConstruct(format!(
                    "field `{}` in a destructuring binding must bind a name",
                    field_name
                )));
            };
            let Some((field_idx, field_layout)) = self.struct_field(&src_ty, field_name) else {
                return Err(LoweringError::InvalidConstruct(format!(
                    "no field `{}` to bind",
                    field_name
                )));
            };
            let field_ty = self.ctx.type_to_mir(&field_layout.ty);
            let (offset, size) = (field_layout.offset, field_layout.size);
            let local = self.builder.alloc_local(binding.clone(), field_ty.clone());
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: local,
                rvalue: MirRValue::Field {
                    base: src_op.clone(),
                    field_index: field_idx as u32,
                    byte_offset: Some(offset),
                    access: FieldAccess::for_field(&field_ty, size),
                },
            }));
            self.locals.insert(binding.clone(), (local, field_ty));
        }
        Ok(())
    }

    fn lower_tuple_destructure(&mut self, names: &[String], init: &Expr) -> Result<(), LoweringError> {
        // Channel.buffered()/unbuffered() returns a raw channel pointer in
        // codegen, not a (Sender, Receiver) tuple. Emit channel_tx/channel_rx
        // calls to extract the handles instead of field extraction.
        let is_channel_create = match &init.kind {
            ExprKind::MethodCall { object, method, .. } => {
                if let ExprKind::Ident(type_name) = &object.kind {
                    let base = type_name.split('<').next().unwrap_or(type_name);
                    base == "Channel" && (method == "buffered" || method == "unbuffered")
                } else { false }
            }
            _ => false,
        };

        let (init_op, init_mir_ty) = self.lower_expr(init)?;
        // Extract tuple element types from type checker for type prefix tracking.
        // e.g. Channel<T>.buffered() → (Sender<T>, Receiver<T>)
        let tuple_elems: Option<Vec<rask_types::Type>> =
            self.ctx.lookup_raw_type(init.id).and_then(|ty| {
                if let rask_types::Type::Tuple(elems) = ty {
                    Some(elems.clone())
                } else {
                    None
                }
            });

        // Extract per-element MIR types from the tuple type.
        let mir_elem_types: Option<Vec<MirType>> = match &init_mir_ty {
            MirType::Tuple(fields) => Some(fields.clone()),
            _ => None,
        };

        for (i, name) in names.iter().enumerate() {
            let elem_ty = if is_channel_create {
                // Channel tx/rx handles are opaque i64 pointers
                MirType::I64
            } else {
                mir_elem_types.as_ref()
                    .and_then(|elems| elems.get(i).cloned())
                    .or_else(|| self.lookup_expr_type(init))
                    .unwrap_or_else(|| crate::fallback::i64_fallback("lower/stmt:1285"))
            };
            let local_id = self.builder.alloc_local(name.clone(), elem_ty.clone());
            self.locals.insert(name.clone(), (local_id, elem_ty));

            if is_channel_create && names.len() == 2 {
                // Extract tx (index 0) or rx (index 1) from the raw channel ptr.
                let extract_fn = if i == 0 { "channel_tx" } else { "channel_rx" };
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: Some(local_id),
                    func: FunctionRef::internal(extract_fn.to_string()),
                    args: vec![init_op.clone()],
                }));
            } else {
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: local_id,
                    rvalue: MirRValue::Field {
                        base: init_op.clone(),
                        field_index: i as u32,
                        byte_offset: None,
                        access: FieldAccess::Word,
                    },
                }));
            }

            // Track type prefix so method calls get qualified names.
            // First try type-checker info (works when types are fully resolved).
            let mut found_prefix = false;
            if let Some(ref elems) = tuple_elems {
                if let Some(elem_type) = elems.get(i) {
                    if let Some(prefix) = super::MirContext::type_prefix(elem_type, self.ctx.type_names) {
                        self.meta_mut(name).type_prefix = Some(prefix);
                        found_prefix = true;
                    }
                }
            }
            // Channel<T>.buffered/unbuffered returns (Sender<T>, Receiver<T>).
            // The element size has to be recorded whether or not the checker
            // resolved the prefixes: `rx.receive()` picks the struct-aware recv
            // off it, and without it a 24-byte element was received into an
            // 8-byte buffer and smashed the stack (#463).
            if let ExprKind::MethodCall { object, method, .. } = &init.kind {
                if let ExprKind::Ident(type_name) = &object.kind {
                    let base = type_name.split('<').next().unwrap_or(type_name);
                    if base == "Channel" && (method == "buffered" || method == "unbuffered") {
                        // Same source the constructor uses for its elem_size arg,
                        // falling back to the annotation's inner type name.
                        let mut elem_size = self.generic_arg_slot_size(init.id, 0);
                        if elem_size <= 8 {
                            if let Some(tn) = type_name.split('<').nth(1)
                                .and_then(|s| s.strip_suffix('>'))
                            {
                                if let Some((_, l)) = self.ctx.find_struct(tn) {
                                    elem_size = l.size as i64;
                                }
                            }
                        }
                        self.meta_mut(name).channel_elem_size = Some(elem_size);
                        if !found_prefix {
                            let prefix = match i {
                                0 => "Sender",
                                1 => "Receiver",
                                _ => continue,
                            };
                            self.meta_mut(name).type_prefix = Some(prefix.to_string());
                        }
                    }
                }
            }
        }
        Ok(())
    }

    // =================================================================
    // Loop lowering
    // =================================================================

    /// While loop (spec L5).
    fn lower_while(&mut self, label: Option<&str>, cond: &Expr, body: &[Stmt]) -> Result<(), LoweringError> {
        let check_block = self.builder.create_block();
        let body_block = self.builder.create_block();
        let exit_block = self.builder.create_block();

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: check_block,
        }));

        // OPT19 on a loop: `while <expr>? as v` reads the scrutinee once per
        // iteration in the check block and rebinds the payload at the top of the
        // body — the same two-part shape `while <expr> is <T> as v` lowers to.
        // A bare `while <expr>?` with no binder needs none of this; the plain
        // path already lowers the test to a bool.
        let presence = match &cond.kind {
            ExprKind::IsPresent { expr: inner, binding: Some(name) } => Some((inner, name.clone())),
            _ => None,
        };

        self.builder.switch_to_block(check_block);
        let bind_in_body = match presence {
            Some((inner, name)) => {
                let (val, scrutinee_ty) = self.lower_expr(inner)?;
                let niche = self.option_niche(inner, &scrutinee_ty);
                let is_niche = niche.is_some();
                let tag = self.emit_option_tag(&val, niche);
                // Tag 0 is present (Some/Ok); anything else ends the loop.
                let is_present = self.builder.alloc_temp(MirType::Bool);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: is_present,
                    rvalue: MirRValue::BinaryOp {
                        op: BinOp::Eq,
                        left: MirOperand::Local(tag),
                        right: MirOperand::Constant(MirConst::Int(0)),
                    },
                }));
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: MirOperand::Local(is_present),
                    then_block: body_block,
                    else_block: exit_block,
                }));
                let payload_ty = self.presence_payload_type(inner, &scrutinee_ty);
                Some((name, val, payload_ty, is_niche))
            }
            None => {
                let (cond_op, _) = self.lower_expr(cond)?;
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: cond_op,
                    then_block: body_block,
                    else_block: exit_block,
                }));
                None
            }
        };

        self.builder.switch_to_block(body_block);
        if let Some((name, val, payload_ty, is_niche)) = bind_in_body {
            self.bind_presence_payload(&name, &val, &payload_ty, is_niche);
        }
        let ensure_depth = self.ensure_stack.len();
        self.loop_stack.push(LoopContext {
            label: label.map(|s| s.to_string()),
            continue_block: check_block,
            exit_block,
            result_local: None,
            ensure_depth,
        });

        self.lower_body_scoped(body)?;
        self.close_loop_body(ensure_depth, check_block);

        self.loop_stack.pop();
        self.ensure_stack.truncate(ensure_depth);
        self.builder.switch_to_block(exit_block);
        Ok(())
    }

    /// For loop: counter-based while for ranges, iterator protocol otherwise.
    fn lower_for(
        &mut self,
        label: Option<&str>,
        binding: &ForBinding,
        mutate: bool,
        iter_expr: &Expr,
        body: &[Stmt],
    ) -> Result<(), LoweringError> {
        // Extract single name for range/iter-chain delegation (tuple not supported there)
        let single_name = match binding {
            ForBinding::Single(name) => name.as_str(),
            ForBinding::Tuple(names) => names.first().map_or("_", |n| n.as_str()),
        };

        // Range expressions desugar to a simple counter loop
        if let ExprKind::Range { start, end, inclusive } = &iter_expr.kind {
            return self.lower_for_range(label, single_name, start.as_deref(), end.as_deref(), *inclusive, body);
        }

        // `(0..10).step(2)` / `(0..5).rev()` — peel the adapters off and run a
        // count-driven loop instead of the plain counter one.
        if let Some(adapted) = Self::peel_range_adapters(iter_expr) {
            return self.lower_for_adapted_range(label, single_name, &adapted, body);
        }

        // Iterator chain: for x in vec.iter().filter(...).map(...) { ... }
        // Fuse into index loop with inlined adapter closures.
        //
        // Not for `for mutate`: the fused loop has no writeback, so the body's
        // changes went into a local copy and the collection never saw them
        // (LP11-LP12). A bare `for mutate x in v` parses as a trivial chain, so
        // it was landing here.
        // A bare `.iter()` with nothing chained onto it iterates exactly what
        // its receiver does, so unwrap it and let the checks below see the
        // collection. The fused-chain path assumes a Vec-shaped source: on a
        // Map it ran `Vec_len` and `Vec_get` against the map pointer, and
        // reading a field off what came back gave "unresolved field `1`" and
        // then a segfault (#398).
        let iter_expr = match &iter_expr.kind {
            ExprKind::MethodCall { object, method, args, .. }
                if method == "iter" && args.is_empty() =>
            {
                object.as_ref()
            }
            _ => iter_expr,
        };

        if !mutate {
            if let Some(chain) = self.try_parse_iter_chain(iter_expr) {
                return self.lower_for_iter_chain(label, single_name, &chain, body, binding);
            }
        }

        // type.sequence/SEQ6: a Sequence is a function taking a yield, so
        // iterating one is calling it. Ahead of the index loop below, which
        // would ask a closure for its `len()`.
        if let Some(elem) = self.sequence_elem_ty(iter_expr) {
            return self.lower_for_sequence(label, binding, iter_expr, body, elem);
        }

        // pool.entries(): for (h, val) in pool.entries() { ... }
        // Desugars to: handles = Pool_handles(pool); for i in 0..len { h = handles[i]; val = Pool_get(pool, h); body }
        if let ForBinding::Tuple(names) = binding {
            if let ExprKind::MethodCall { object, method, .. } = &iter_expr.kind {
                if method == "entries" {
                    let obj_is_pool = self.ctx.lookup_raw_type(object.id).map_or(false, |ty| {
                        matches!(ty, rask_types::Type::UnresolvedNamed(n) if n == "Pool")
                            || matches!(ty, rask_types::Type::UnresolvedGeneric { name, .. } if name == "Pool")
                    });
                    if obj_is_pool {
                        return self.lower_for_pool_entries(label, names, object, body, mutate);
                    }
                }
            }
        }

        // Pool iteration: `for h in pool` desugars to snapshot handle iteration.
        // Calls Pool_handles(pool) → Vec<Handle>, then iterates the Vec.
        let is_pool = self.ctx.lookup_raw_type(iter_expr.id).map_or(false, |ty| {
            matches!(
                ty,
                rask_types::Type::UnresolvedNamed(n) if n == "Pool"
            ) || matches!(
                ty,
                rask_types::Type::UnresolvedGeneric { name, .. } if name == "Pool"
            ) || super::MirContext::type_prefix(ty, self.ctx.type_names)
                .is_some_and(|p| p.split('<').next() == Some("Pool"))
        });

        // LP13: Detect Map iteration for correct writeback target.
        // The name has to come from `type_prefix`, not a match on the
        // Unresolved shapes alone: once the checker resolves the receiver it's
        // a `Type::Generic { base: TypeId, .. }` and the literal-name test
        // missed it entirely.
        let is_map = self.ctx.lookup_raw_type(iter_expr.id).map_or(false, |ty| {
            super::MirContext::type_prefix(ty, self.ctx.type_names)
                .is_some_and(|p| p.split('<').next() == Some("Map"))
        });

        // Index-based iteration: for item in collection { ... }
        // Desugars to: _i = 0; _len = collection.len(); while _i < _len { item = collection[_i]; ...; _i += 1 }
        let (iter_op, iter_ty) = self.lower_expr(iter_expr)?;

        // LP13: the map itself, kept aside because the loop goes on to walk a
        // snapshot of its entries. `for mutate` writes back into the map, not
        // into the snapshot, so `Map_set` needs this one and not `collection`.
        let mut map_local = None;

        // For pools: convert pool → Vec<Handle> via Pool_handles snapshot
        let (iter_op, iter_ty) = if is_pool {
            let pool_tmp = self.builder.alloc_temp(iter_ty.clone());
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: pool_tmp,
                rvalue: MirRValue::Use(iter_op),
            }));
            let handles_vec = self.builder.alloc_temp(MirType::I64);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(handles_vec),
                func: FunctionRef::internal("Pool_handles".to_string()),
                args: vec![MirOperand::Local(pool_tmp)],
            }));
            (MirOperand::Local(handles_vec), MirType::I64)
        } else if is_map {
            // LP13: a map iterates over its entries. Without this the loop ran
            // `Vec_len`/`Vec_get` straight on the map pointer, and reading a
            // field off whatever came back segfaulted. `Map_entries` snapshots
            // it as a Vec of 16-byte (key, value) pairs, which the tuple
            // destructuring below already knows how to read.
            let map_tmp = self.builder.alloc_temp(iter_ty.clone());
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: map_tmp,
                rvalue: MirRValue::Use(iter_op),
            }));
            map_local = Some(map_tmp);
            let entries_vec = self.builder.alloc_temp(MirType::I64);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(entries_vec),
                func: FunctionRef::internal("Map_entries".to_string()),
                args: vec![MirOperand::Local(map_tmp)],
            }));
            (MirOperand::Local(entries_vec), MirType::I64)
        } else {
            (iter_op, iter_ty)
        };

        let is_array = matches!(&iter_ty, MirType::Array { .. });
        let (array_len, array_elem_size) = match &iter_ty {
            MirType::Array { elem, len } => (Some(*len), Some(elem.size())),
            _ => (None, None),
        };

        let collection = self.builder.alloc_temp(iter_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: collection,
            rvalue: MirRValue::Use(iter_op),
        }));

        // _len = collection.len()
        let len_local = self.builder.alloc_temp(MirType::I64);
        if let Some(arr_len) = array_len {
            // Fixed-size array: compile-time constant length
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: len_local,
                rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(arr_len as i64))),
            }));
        } else {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(len_local),
                func: FunctionRef::internal("Vec_len".to_string()),
                args: vec![MirOperand::Local(collection)],
            }));
        }

        // _i = 0
        let idx = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: idx,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
        }));

        let check_block = self.builder.create_block();
        let body_block = self.builder.create_block();
        let inc_block = self.builder.create_block();
        let exit_block = self.builder.create_block();

        // For `for mutate`, create writeback blocks that call Vec_set
        // before continuing or breaking out of the loop.
        let (wb_block, break_wb_block) = if mutate && !is_array {
            let wb = self.builder.create_block();
            let break_wb = self.builder.create_block();
            (Some(wb), Some(break_wb))
        } else {
            (None, None)
        };

        // continue target: writeback block (if mutate), otherwise inc_block
        let continue_target = wb_block.unwrap_or(inc_block);
        // break target: break-writeback block (if mutate), otherwise exit_block
        let break_target = break_wb_block.unwrap_or(exit_block);

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: check_block }));

        // check: _i < _len
        self.builder.switch_to_block(check_block);
        let cond = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: cond,
            rvalue: MirRValue::BinaryOp {
                op: BinOp::Lt,
                left: MirOperand::Local(idx),
                right: MirOperand::Local(len_local),
            },
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(cond),
            then_block: body_block,
            else_block: exit_block,
        }));

        // body: item = collection[_i]
        self.builder.switch_to_block(body_block);
        let elem_ty = if is_map {
            // A map entry is a (key, value) pair, laid out like a tuple. Typed
            // as a bare i64 the element load came back as just the key, and
            // reading field 1 off it dereferenced that value as a pointer.
            MirType::Tuple(self.map_entry_pair_types(iter_expr))
        } else if is_pool {
            // mem.pools/PF1-PF4: `for h in pool` iterates the handle snapshot,
            // so the binding is a `Handle`, not the pool's value type. Asking
            // `iter_expr` — still the pool — gave the value type, so the loop
            // read each 8-byte handle as if it were an element struct and
            // `pool[h]` then panicked with "invalid handle".
            MirType::Handle
        } else {
            self.extract_iterator_elem_type(iter_expr)
                // `m.keys()` / `m.values()` hand back a Vec of the map's own key
                // or value type; nothing else in the chain says what that is.
                .or_else(|| self.map_projection_elem_type(iter_expr))
                .filter(|t| !matches!(binding, ForBinding::Tuple(_))
                    || matches!(t, MirType::Tuple(_)))
                // Destructuring needs the element's real shape: a `Vec<(string,
                // string)>` element is 32 bytes, and typed as a word the second
                // field was read 8 bytes in instead of 16.
                .or_else(|| matches!(binding, ForBinding::Tuple(_))
                    .then(|| self.vec_tuple_elem_type(iter_expr))
                    .flatten())
                // Last: the source collection's own declared element type.
                .or_else(|| self.collection_elem_of_expr(iter_expr))
                .unwrap_or_else(|| crate::fallback::i64_fallback("lower/stmt:for_loop_elem"))
        };
        let (pair_tys, binding_ty, binding_local, elem_slot) =
            self.alloc_destructure_slots(&elem_ty, binding, single_name);
        self.note_closure_binding(single_name, iter_expr);
        if let Some(prefix) = self.mir_type_name(&elem_ty) {
            self.meta_mut(single_name).type_prefix = Some(prefix);
        } else if is_pool {
            // Same prefix `for h in pool.handles()` gets, so indexing on the
            // binding resolves the same way.
            self.meta_mut(single_name).type_prefix = Some("Handle".to_string());
        } else {
            // MirType is Ptr — try to derive element prefix from iterable context.
            // Method calls like .chunks() return Vec elements, .handles() returns Handle elements.
            if let ExprKind::MethodCall { method, .. } = &iter_expr.kind {
                match method.as_str() {
                    "chunks" => {
                        self.meta_mut(single_name).type_prefix = Some("Vec".to_string());
                    }
                    "handles" | "cursor" => {
                        self.meta_mut(single_name).type_prefix = Some("Handle".to_string());
                    }
                    _ => {}
                }
            }
        }
        if is_array {
            // Fixed-size array: direct memory load
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: elem_slot,
                rvalue: MirRValue::ArrayIndex {
                    base: MirOperand::Local(collection),
                    index: MirOperand::Local(idx),
                    elem_size: array_elem_size.unwrap_or(8),
                },
            }));
        } else {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(elem_slot),
                func: FunctionRef::internal("Vec_get".to_string()),
                args: vec![MirOperand::Local(collection), MirOperand::Local(idx)],
            }));
        }

        // Tuple destructuring: for (a, b) in collection { ... }
        // Extract fields from the loaded element into each binding.
        // LP13: Track value local for Map writeback (key=field0, value=field1).
        let mut map_value_local = None;
        if let ForBinding::Tuple(names) = binding {
            if let Some(prefix) = self.mir_type_name(&binding_ty) {
                self.meta_mut(single_name).type_prefix = Some(prefix);
            }
            let second = self.split_destructured_element(names, &pair_tys, elem_slot, binding_local);
            if is_map {
                map_value_local = second;
            }
        }

        let ensure_depth = self.ensure_stack.len();
        self.loop_stack.push(LoopContext {
            label: label.map(|s| s.to_string()),
            continue_block: continue_target,
            exit_block: break_target,
            result_local: None,
            ensure_depth,
        });

        // The writeback the body owes its collection. `continue` and `break` pay it
        // through the blocks below; a `return` or a `try` that propagates pays it
        // from here, because neither passes through a block (#650).
        // A map writes back into itself; everything else into the thing being
        // walked. Handing `Map_set` the entries snapshot stored a key/value pair
        // through a Vec pointer and segfaulted on the first iteration (#738).
        let writeback = super::MutateWriteback::new(
            map_local.unwrap_or(collection),
            idx,
            binding_local,
            map_value_local,
        );
        if wb_block.is_some() {
            self.mutate_writebacks.push(writeback);
        }
        self.lower_body_scoped(body)?;
        if wb_block.is_some() {
            self.mutate_writebacks.pop();
        }
        self.close_loop_body(ensure_depth, continue_target);

        // Writeback blocks for `for mutate`
        if let Some(wb) = wb_block {
            self.builder.switch_to_block(wb);
            self.emit_one_mutate_writeback(&writeback);
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: inc_block }));
        }
        if let Some(break_wb) = break_wb_block {
            self.builder.switch_to_block(break_wb);
            self.emit_one_mutate_writeback(&writeback);
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: exit_block }));
        }

        // inc: _i = _i + 1
        self.builder.switch_to_block(inc_block);
        let incremented = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: incremented,
            rvalue: MirRValue::BinaryOp {
                op: BinOp::Add,
                left: MirOperand::Local(idx),
                right: MirOperand::Constant(MirConst::Int(1)),
            },
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: idx,
            rvalue: MirRValue::Use(MirOperand::Local(incremented)),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: check_block }));

        self.loop_stack.pop();
        self.ensure_stack.truncate(ensure_depth);
        self.builder.switch_to_block(exit_block);
        Ok(())
    }

    /// A `for` over a collection of functions binds a closure value, so record
    /// it the way `let`/`mut` already do.
    ///
    /// Without this the call site had no reason to emit a `ClosureCall`: it
    /// lowered `f(5)` as a call to a *function named `f`*, found no signature
    /// for it, and gave up on the return type — "couldn't work out a type here"
    /// out of MIR lowering, while `let g = fs[0]` two lines away was fine
    /// (#869). The interpreter has no such split and ran both.
    fn note_closure_binding(&mut self, name: &str, source: &Expr) {
        let Some(source_ty) = self.ctx.lookup_raw_type(source.id) else { return };
        let Some(elem) = self.checker_elem_of(source_ty) else { return };
        if let rask_types::Type::Fn { ret, .. } = elem {
            self.closure_locals.insert(name.to_string());
            let ret_mir = self.ctx.type_to_mir(&ret);
            self.func_sigs.insert(
                name.to_string(),
                super::FuncSig {
                    ret_ty: ret_mir,
                    scalar_mutate_params: Vec::new(),
                    aggregate_mutate_params: Vec::new(),
                    ret_vec_elem: None,
                    param_ty_strs: Vec::new(),
                },
            );
        }
    }

    /// The checker's element type of a `Vec`/`Iterator` source, as a checker
    /// type rather than a MIR one — a function type has no MIR shape beyond
    /// "pointer", so this has to look before the conversion.
    fn checker_elem_of(&self, ty: &rask_types::Type) -> Option<rask_types::Type> {
        let (name, args) = self.generic_head(ty)?;
        if !matches!(name.as_str(), "Vec" | "Iterator") {
            return None;
        }
        match args.first()? {
            rask_types::GenericArg::Type(inner) => Some((**inner).clone()),
            _ => None,
        }
    }

    /// Pool entries iteration: `for (h, val) in pool.entries()`
    /// Desugars to snapshot handle iteration with Pool_get for each handle.
    /// Work out where a `for` loop's element and its destructured pieces live.
    ///
    /// Destructuring reads the whole element, then splits it. The binding named
    /// first carries the *first field's* type, so it can't double as the slot
    /// holding the pair — codegen reads a local's declared type, and a `string`
    /// key binding would make the field-1 read offset into a RaskStr instead of
    /// the pair. So a destructuring loop gets a separate slot for the element.
    ///
    /// Returns the field types (empty when there's nothing to destructure), the
    /// first binding's local, and the slot the element itself goes in.
    fn alloc_destructure_slots(
        &mut self,
        elem_ty: &MirType,
        binding: &ForBinding,
        first_name: &str,
    ) -> (Vec<MirType>, MirType, crate::LocalId, crate::LocalId) {
        let pair_tys: Vec<MirType> = match (elem_ty, binding) {
            (MirType::Tuple(tys), ForBinding::Tuple(_)) => tys.clone(),
            _ => Vec::new(),
        };
        let binding_ty = pair_tys.first().cloned().unwrap_or_else(|| elem_ty.clone());
        let binding_local = self.builder.alloc_local(first_name.to_string(), binding_ty.clone());
        let elem_slot = if pair_tys.is_empty() {
            binding_local
        } else {
            self.builder.alloc_temp(elem_ty.clone())
        };
        if let Some(prefix) = self.mir_type_name(&binding_ty) {
            self.meta_mut(first_name).type_prefix = Some(prefix);
        }
        self.locals.insert(first_name.to_string(), (binding_local, binding_ty.clone()));
        (pair_tys, binding_ty, binding_local, elem_slot)
    }

    /// Copy each field of the element in `elem_slot` into its own binding.
    /// Field 0 goes to the binding named first, which already has a local.
    /// Returns the local field 1 landed in — Map iteration writes back through it.
    fn split_destructured_element(
        &mut self,
        names: &[String],
        pair_tys: &[MirType],
        elem_slot: crate::LocalId,
        binding_local: crate::LocalId,
    ) -> Option<crate::LocalId> {
        let mut second = None;
        for (i, name) in names.iter().enumerate() {
            if i == 0 { continue; }
            let field_ty = pair_tys.get(i).cloned().unwrap_or_else(|| crate::fallback::i64_fallback("lower/stmt:1828"));
            let field_local = self.builder.alloc_local(name.clone(), field_ty.clone());
            self.locals.insert(name.clone(), (field_local, field_ty.clone()));
            if let Some(prefix) = self.mir_type_name(&field_ty) {
                self.meta_mut(name).type_prefix = Some(prefix);
            }
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: field_local,
                rvalue: MirRValue::Field {
                    base: MirOperand::Local(elem_slot),
                    field_index: i as u32,
                    byte_offset: None,
                    access: FieldAccess::Word,
                },
            }));
            if i == 1 {
                second = Some(field_local);
            }
        }
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: binding_local,
            rvalue: MirRValue::Field {
                base: MirOperand::Local(elem_slot),
                field_index: 0,
                byte_offset: None,
                access: FieldAccess::Word,
            },
        }));
        second
    }

    /// LP11-LP13: `for mutate` adds Pool_set writeback.
    fn lower_for_pool_entries(
        &mut self,
        label: Option<&str>,
        names: &[String],
        pool_expr: &Expr,
        body: &[Stmt],
        mutate: bool,
    ) -> Result<(), LoweringError> {
        let (pool_op, _) = self.lower_expr(pool_expr)?;
        let pool_local = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: pool_local,
            rvalue: MirRValue::Use(pool_op),
        }));

        // handles_vec = Pool_handles(pool)
        let handles_vec = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(handles_vec),
            func: FunctionRef::internal("Pool_handles".to_string()),
            args: vec![MirOperand::Local(pool_local)],
        }));

        // _len = Vec_len(handles_vec)
        let len_local = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(len_local),
            func: FunctionRef::internal("Vec_len".to_string()),
            args: vec![MirOperand::Local(handles_vec)],
        }));

        // _i = 0
        let idx = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: idx,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
        }));

        let check_block = self.builder.create_block();
        let body_block = self.builder.create_block();
        let inc_block = self.builder.create_block();
        let exit_block = self.builder.create_block();

        // LP11-LP13: for mutate writeback blocks for Pool_set
        let (wb_block, break_wb_block) = if mutate && names.len() > 1 {
            let wb = self.builder.create_block();
            let break_wb = self.builder.create_block();
            (Some(wb), Some(break_wb))
        } else {
            (None, None)
        };
        let continue_target = wb_block.unwrap_or(inc_block);
        let break_target = break_wb_block.unwrap_or(exit_block);

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: check_block }));

        // check: _i < _len
        self.builder.switch_to_block(check_block);
        let cond = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: cond,
            rvalue: MirRValue::BinaryOp {
                op: BinOp::Lt,
                left: MirOperand::Local(idx),
                right: MirOperand::Local(len_local),
            },
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(cond),
            then_block: body_block,
            else_block: exit_block,
        }));

        // body: h = handles_vec[_i]; val = Pool_get(pool, h)
        self.builder.switch_to_block(body_block);

        // Bind handle (first name)
        let handle_name = names.first().map_or("_h", |n| n.as_str());
        let handle_local = self.builder.alloc_local(handle_name.to_string(), MirType::I64);
        self.locals.insert(handle_name.to_string(), (handle_local, MirType::I64));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(handle_local),
            func: FunctionRef::internal("Vec_get".to_string()),
            args: vec![MirOperand::Local(handles_vec), MirOperand::Local(idx)],
        }));

        // Bind value (second name) via Pool_get
        let val_local = if names.len() > 1 {
            let val_name = &names[1];
            let val_local = self.builder.alloc_local(val_name.clone(), MirType::I64);
            self.locals.insert(val_name.clone(), (val_local, MirType::I64));
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(val_local),
                func: FunctionRef::internal("Pool_get".to_string()),
                args: vec![MirOperand::Local(pool_local), MirOperand::Local(handle_local)],
            }));
            Some(val_local)
        } else {
            None
        };

        let ensure_depth = self.ensure_stack.len();
        self.loop_stack.push(LoopContext {
            label: label.map(|s| s.to_string()),
            continue_block: continue_target,
            exit_block: break_target,
            result_local: None,
            ensure_depth,
        });

        self.lower_body_scoped(body)?;
        self.close_loop_body(ensure_depth, continue_target);

        // LP13: Pool_set writeback blocks for `for mutate`
        if let (Some(wb), Some(vl)) = (wb_block, val_local) {
            self.builder.switch_to_block(wb);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::internal("Pool_set".to_string()),
                args: vec![
                    MirOperand::Local(pool_local),
                    MirOperand::Local(handle_local),
                    MirOperand::Local(vl),
                ],
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: inc_block }));
        }
        if let (Some(break_wb), Some(vl)) = (break_wb_block, val_local) {
            self.builder.switch_to_block(break_wb);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::internal("Pool_set".to_string()),
                args: vec![
                    MirOperand::Local(pool_local),
                    MirOperand::Local(handle_local),
                    MirOperand::Local(vl),
                ],
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: exit_block }));
        }

        // inc: _i = _i + 1
        self.builder.switch_to_block(inc_block);
        let incremented = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: incremented,
            rvalue: MirRValue::BinaryOp {
                op: BinOp::Add,
                left: MirOperand::Local(idx),
                right: MirOperand::Constant(MirConst::Int(1)),
            },
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: idx,
            rvalue: MirRValue::Use(MirOperand::Local(incremented)),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: check_block }));

        self.loop_stack.pop();
        self.ensure_stack.truncate(ensure_depth);
        self.builder.switch_to_block(exit_block);
        Ok(())
    }

    /// Range for-loop: `for i in start..end` desugars to a counter-based while.
    fn lower_for_range(
        &mut self,
        label: Option<&str>,
        binding: &str,
        start: Option<&Expr>,
        end: Option<&Expr>,
        inclusive: bool,
        body: &[Stmt],
    ) -> Result<(), LoweringError> {
        let (start_op, start_ty) = if let Some(s) = start {
            self.lower_expr(s)?
        } else {
            (MirOperand::Constant(MirConst::Int(0)), MirType::I64)
        };
        let (end_op, _) = if let Some(e) = end {
            self.lower_expr(e)?
        } else {
            return Err(LoweringError::InvalidConstruct("Unbounded range in for loop".to_string()));
        };

        // Mutable counter initialized to start
        let counter = self.builder.alloc_local(binding.to_string(), start_ty.clone());
        self.locals.insert(binding.to_string(), (counter, start_ty.clone()));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: counter,
            rvalue: MirRValue::Use(start_op),
        }));

        // Evaluate end once
        let end_local = self.builder.alloc_temp(start_ty);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: end_local,
            rvalue: MirRValue::Use(end_op),
        }));

        let check_block = self.builder.create_block();
        let body_block = self.builder.create_block();
        let inc_block = self.builder.create_block();
        let exit_block = self.builder.create_block();

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: check_block }));
        self.builder.switch_to_block(check_block);

        // counter < end (or <= for inclusive)
        let cmp_op = if inclusive { BinOp::Le } else { BinOp::Lt };
        let cond = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: cond,
            rvalue: MirRValue::BinaryOp {
                op: cmp_op,
                left: MirOperand::Local(counter),
                right: MirOperand::Local(end_local),
            },
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(cond),
            then_block: body_block,
            else_block: exit_block,
        }));

        self.builder.switch_to_block(body_block);
        let ensure_depth = self.ensure_stack.len();
        self.loop_stack.push(LoopContext {
            label: label.map(|s| s.to_string()),
            continue_block: inc_block,
            exit_block,
            result_local: None,
            ensure_depth,
        });

        self.lower_body_scoped(body)?;
        self.close_loop_body(ensure_depth, inc_block);

        // counter = counter + 1
        self.builder.switch_to_block(inc_block);
        let incremented = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: incremented,
            rvalue: MirRValue::BinaryOp {
                op: BinOp::Add,
                left: MirOperand::Local(counter),
                right: MirOperand::Constant(MirConst::Int(1)),
            },
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: counter,
            rvalue: MirRValue::Use(MirOperand::Local(incremented)),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: check_block }));

        self.loop_stack.pop();
        self.ensure_stack.truncate(ensure_depth);
        self.builder.switch_to_block(exit_block);
        Ok(())
    }

    /// Strip `.rev()` / `.step(n)` off a range expression, innermost first.
    /// Returns `None` for anything that isn't a range under those adapters, so
    /// the caller falls through to normal iteration.
    fn peel_range_adapters(expr: &Expr) -> Option<AdaptedRange<'_>> {
        let mut rev = false;
        let mut step: Option<&Expr> = None;
        let mut cursor = expr;
        let mut saw_adapter = false;
        loop {
            match &cursor.kind {
                ExprKind::MethodCall { object, method, args, .. } if method == "rev" && args.is_empty() => {
                    rev = !rev;
                    saw_adapter = true;
                    cursor = object;
                }
                ExprKind::MethodCall { object, method, args, .. } if method == "step" && args.len() == 1 => {
                    // Outermost `.step()` wins if somebody writes two.
                    if step.is_none() {
                        step = Some(&args[0].expr);
                    }
                    saw_adapter = true;
                    cursor = object;
                }
                ExprKind::Range { start, end, inclusive } if saw_adapter => {
                    return Some(AdaptedRange {
                        start: start.as_deref(),
                        end: end.as_deref(),
                        inclusive: *inclusive,
                        step,
                        rev,
                    });
                }
                _ => return None,
            }
        }
    }

    /// `for i in (a..b).step(s)` / `.rev()`.
    ///
    /// Counts the elements up front and then walks an index, rather than
    /// stepping a value and testing it against the end. The count is what makes
    /// `.rev()` possible at all — you can't start from the last element without
    /// knowing how many there are — and it makes the wrong-direction cases fall
    /// out for free: `(0..10).step(-1)` counts -10, so the loop never runs,
    /// which is the empty range SP1/SP2 ask for.
    fn lower_for_adapted_range(
        &mut self,
        label: Option<&str>,
        binding: &str,
        range: &AdaptedRange<'_>,
        body: &[Stmt],
    ) -> Result<(), LoweringError> {
        let (start_op, start_ty) = if let Some(s) = range.start {
            self.lower_expr(s)?
        } else {
            (MirOperand::Constant(MirConst::Int(0)), MirType::I64)
        };
        let (end_op, _) = match range.end {
            Some(e) => self.lower_expr(e)?,
            None => {
                return Err(LoweringError::InvalidConstruct(
                    "range adapter on an unbounded range".to_string(),
                ))
            }
        };
        let (step_op, _) = match range.step {
            Some(s) => self.lower_expr(s)?,
            None => (MirOperand::Constant(MirConst::Int(1)), start_ty.clone()),
        };

        let mut define = |this: &mut Self, ty: &MirType, rvalue: MirRValue| {
            let local = this.builder.alloc_temp(ty.clone());
            this.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign { dst: local, rvalue }));
            local
        };
        let start_l = define(self, &start_ty, MirRValue::Use(start_op));
        let end_l = define(self, &start_ty, MirRValue::Use(end_op));
        let step_l = define(self, &start_ty, MirRValue::Use(step_op));

        let bin = |op: BinOp, left: MirOperand, right: MirOperand| MirRValue::BinaryOp { op, left, right };
        let diff = define(self, &start_ty, bin(
            BinOp::Sub, MirOperand::Local(end_l), MirOperand::Local(start_l),
        ));
        let q = define(self, &start_ty, bin(
            BinOp::Div, MirOperand::Local(diff), MirOperand::Local(step_l),
        ));

        // Element count. Inclusive is q+1 flat; exclusive is q, rounded up when
        // the span doesn't divide evenly ((0..10).step(3) → 0,3,6,9, not 3).
        let count = self.builder.alloc_temp(start_ty.clone());
        if range.inclusive {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: count,
                rvalue: bin(BinOp::Add, MirOperand::Local(q), MirOperand::Constant(MirConst::Int(1))),
            }));
        } else {
            let prod = define(self, &start_ty, bin(
                BinOp::Mul, MirOperand::Local(q), MirOperand::Local(step_l),
            ));
            let exact = define(self, &MirType::Bool, bin(
                BinOp::Eq, MirOperand::Local(prod), MirOperand::Local(diff),
            ));
            let exact_bb = self.builder.create_block();
            let round_bb = self.builder.create_block();
            let joined = self.builder.create_block();
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                cond: MirOperand::Local(exact),
                then_block: exact_bb,
                else_block: round_bb,
            }));
            self.builder.switch_to_block(exact_bb);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: count,
                rvalue: MirRValue::Use(MirOperand::Local(q)),
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: joined }));
            self.builder.switch_to_block(round_bb);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: count,
                rvalue: bin(BinOp::Add, MirOperand::Local(q), MirOperand::Constant(MirConst::Int(1))),
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: joined }));
            self.builder.switch_to_block(joined);
        }

        // A negative count needs no clamp: `k < count` is false from the start.
        let k = self.builder.alloc_temp(start_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: k,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
        }));
        let value = self.builder.alloc_local(binding.to_string(), start_ty.clone());
        self.locals.insert(binding.to_string(), (value, start_ty.clone()));

        let check_block = self.builder.create_block();
        let body_block = self.builder.create_block();
        let inc_block = self.builder.create_block();
        let exit_block = self.builder.create_block();

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: check_block }));
        self.builder.switch_to_block(check_block);
        let cond = define(self, &MirType::Bool, bin(
            BinOp::Lt, MirOperand::Local(k), MirOperand::Local(count),
        ));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(cond),
            then_block: body_block,
            else_block: exit_block,
        }));

        self.builder.switch_to_block(body_block);
        // `.rev()` walks the same elements from the far end: index count-1-k.
        let index = if range.rev {
            let last = define(self, &start_ty, bin(
                BinOp::Sub, MirOperand::Local(count), MirOperand::Constant(MirConst::Int(1)),
            ));
            define(self, &start_ty, bin(
                BinOp::Sub, MirOperand::Local(last), MirOperand::Local(k),
            ))
        } else {
            k
        };
        let offset = define(self, &start_ty, bin(
            BinOp::Mul, MirOperand::Local(index), MirOperand::Local(step_l),
        ));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: value,
            rvalue: bin(BinOp::Add, MirOperand::Local(start_l), MirOperand::Local(offset)),
        }));

        let ensure_depth = self.ensure_stack.len();
        self.loop_stack.push(LoopContext {
            label: label.map(|s| s.to_string()),
            continue_block: inc_block,
            exit_block,
            result_local: None,
            ensure_depth,
        });
        self.lower_body_scoped(body)?;
        self.close_loop_body(ensure_depth, inc_block);

        self.builder.switch_to_block(inc_block);
        let next = self.builder.alloc_temp(start_ty);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: next,
            rvalue: bin(BinOp::Add, MirOperand::Local(k), MirOperand::Constant(MirConst::Int(1))),
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: k,
            rvalue: MirRValue::Use(MirOperand::Local(next)),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: check_block }));

        self.loop_stack.pop();
        self.ensure_stack.truncate(ensure_depth);
        self.builder.switch_to_block(exit_block);
        Ok(())
    }

    /// Infinite loop.
    pub(super) fn lower_loop(&mut self, label: Option<&str>, body: &[Stmt]) -> Result<(), LoweringError> {
        let loop_block = self.builder.create_block();
        let exit_block = self.builder.create_block();

        // CF25: allocate result slot so break-with-value can store to it
        let result_local = self.builder.alloc_local(
            "__loop_result".to_string(),
            MirType::I64,
        );

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: loop_block,
        }));

        self.builder.switch_to_block(loop_block);

        let ensure_depth = self.ensure_stack.len();
        self.loop_stack.push(LoopContext {
            label: label.map(|s| s.to_string()),
            continue_block: loop_block,
            exit_block,
            result_local: Some(result_local),
            ensure_depth,
        });

        self.lower_body_scoped(body)?;
        self.close_loop_body(ensure_depth, loop_block);

        self.loop_stack.pop();
        self.ensure_stack.truncate(ensure_depth);
        self.builder.switch_to_block(exit_block);
        Ok(())
    }

    /// Break statement - jump to enclosing loop's exit block.
    /// EX4: runs loop-scoped ensures before exiting.
    fn lower_break(
        &mut self,
        label: Option<&str>,
        value: Option<&Expr>,
    ) -> Result<(), LoweringError> {
        let ctx = self.find_loop(label)?;
        let exit_block = ctx.exit_block;
        let result_local = ctx.result_local;
        let ensure_depth = ctx.ensure_depth;

        if let Some(val_expr) = value {
            let (val_op, _) = self.lower_expr(val_expr)?;
            if let Some(result) = result_local {
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result,
                    rvalue: MirRValue::Use(val_op),
                }));
            }
        }

        self.emit_loop_cleanup(ensure_depth);
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: exit_block,
        }));

        let dead_block = self.builder.create_block();
        self.builder.switch_to_block(dead_block);

        Ok(())
    }

    /// Continue statement - jump to enclosing loop's check block.
    /// EX4: runs loop-scoped ensures before continuing.
    fn lower_continue(&mut self, label: Option<&str>) -> Result<(), LoweringError> {
        let ctx = self.find_loop(label)?;
        let continue_block = ctx.continue_block;
        let ensure_depth = ctx.ensure_depth;

        self.emit_loop_cleanup(ensure_depth);
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: continue_block,
        }));

        let dead_block = self.builder.create_block();
        self.builder.switch_to_block(dead_block);

        Ok(())
    }

    /// Find the loop context for a break/continue, optionally by label.
    fn find_loop(&self, label: Option<&str>) -> Result<&LoopContext, LoweringError> {
        match label {
            None => self.loop_stack.last().ok_or_else(|| {
                LoweringError::InvalidConstruct("break/continue outside of loop".to_string())
            }),
            Some(lbl) => self
                .loop_stack
                .iter()
                .rev()
                .find(|ctx| ctx.label.as_deref() == Some(lbl))
                .ok_or_else(|| {
                    LoweringError::InvalidConstruct(format!("No loop with label '{}'", lbl))
                }),
        }
    }

    /// For-in over an iterator chain: fused index loop with inlined adapters.
    fn lower_for_iter_chain(
        &mut self,
        label: Option<&str>,
        binding_name: &str,
        chain: &super::IterChain<'_>,
        body: &[Stmt],
        for_binding: &ForBinding,
    ) -> Result<(), LoweringError> {
        let setup = self.setup_iter_chain_loop(chain)?;
        let (final_op, final_ty) = self.apply_iter_adapters(
            chain, MirOperand::Local(setup.elem_local), setup.elem_ty.clone(),
            setup.inc_block, setup.idx,
        )?;

        // Destructuring needs the element's real shape. A `Vec<(string, i64)>`
        // element is 24 bytes; typed as a word, field 1 was read 8 bytes in and
        // the string binding had no type to dispatch `.len()` on (#535).
        let final_ty = match (for_binding, &final_ty) {
            (ForBinding::Tuple(_), MirType::Tuple(_)) => final_ty,
            (ForBinding::Tuple(_), _) => self
                .vec_tuple_elem_type(chain.source)
                .unwrap_or(final_ty),
            _ => final_ty,
        };
        let (pair_tys, _binding_ty, binding_local, elem_slot) =
            self.alloc_destructure_slots(&final_ty, for_binding, binding_name);
        // Adapters can change the element type, so ask about the chain's own
        // source only when nothing was applied. Otherwise `for f in fs.iter()`
        // over a Vec of functions still binds a callable, but
        // `for n in fs.map(|f| f(1))` binds an i64.
        if chain.adapters.is_empty() {
            self.note_closure_binding(binding_name, chain.source);
        }
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: elem_slot,
            rvalue: MirRValue::Use(final_op),
        }));

        if let ForBinding::Tuple(names) = for_binding {
            self.split_destructured_element(names, &pair_tys, elem_slot, binding_local);
        }

        let ensure_depth = self.ensure_stack.len();
        self.loop_stack.push(super::LoopContext {
            label: label.map(|s| s.to_string()),
            continue_block: setup.inc_block,
            exit_block: setup.exit_block,
            result_local: None,
            ensure_depth,
        });

        self.lower_body_scoped(body)?;

        self.close_loop_body(ensure_depth, setup.inc_block);
        self.loop_stack.pop();
        self.ensure_stack.truncate(ensure_depth);

        self.emit_iter_increment(setup.idx, setup.inc_block, setup.check_block);
        self.builder.switch_to_block(setup.exit_block);
        Ok(())
    }

    /// ER2: route an error out of an `ensure` body into its `else |e|` handler.
    ///
    /// Assumes the body has just been lowered into the current block, so the
    /// last `Call` in it is the operation whose `T or E` decides the branch.
    /// Leaves the builder on the merge block, unterminated — the caller decides
    /// how a cleanup that ran to completion continues (the inline path splices
    /// a sentinel, the panic thunk returns).
    ///
    /// Shared by both so the handler's shape can't drift between the exit a
    /// normal return takes and the one a panic takes.
    pub(super) fn lower_ensure_else_handler(
        &mut self,
        param_name: &str,
        handler_body: &[Stmt],
    ) -> Result<(), LoweringError> {
        let handler_block = self.builder.create_block();
        let done_block = self.builder.create_block();

        let Some(call_dst) = self.builder.last_call_dst() else {
            // No call in the body — nothing can fail, so the handler never fires.
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: done_block }));
            self.builder.switch_to_block(done_block);
            return Ok(());
        };

        // Result tag: 0 = ok, 1 = err.
        let tag = self.builder.alloc_temp(MirType::U8);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: tag,
            rvalue: MirRValue::EnumTag { value: MirOperand::Local(call_dst) },
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(tag),
            then_block: handler_block,
            else_block: done_block,
        }));

        // Handler binds the error payload; its type comes from the call's own
        // return type.
        let err_ty = self.builder.local_type(call_dst)
            .and_then(|t| match t {
                MirType::Result { err, .. } => Some(*err),
                _ => None,
            })
            .unwrap_or_else(|| crate::fallback::i64_fallback("lower/stmt:ensure_else"));
        self.builder.switch_to_block(handler_block);
        let err_local = self.builder.alloc_local(param_name.to_string(), err_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: err_local,
            rvalue: MirRValue::Field {
                base: MirOperand::Local(call_dst),
                field_index: 0,
                byte_offset: None,
                access: FieldAccess::Word,
            },
        }));
        self.locals.insert(param_name.to_string(), (err_local, err_ty));
        for s in handler_body {
            self.lower_stmt(s)?;
        }
        if self.builder.current_block_unterminated() {
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: done_block }));
        }

        self.builder.switch_to_block(done_block);
        Ok(())
    }

}

/// A `mutate` param is passed by pointer only for aggregate types (structs,
/// enums, tuples, and other by-reference layouts). Reassigning such a param
/// stores bytes through the caller's pointer so the change is visible. Scalar
/// Copy types are passed by value — reassignment stays local (no writeback).
pub(crate) fn mutate_param_by_pointer(ty: &MirType) -> bool {
    !matches!(
        ty,
        MirType::Void
            | MirType::Bool
            | MirType::I8 | MirType::I16 | MirType::I32 | MirType::I64
            | MirType::U8 | MirType::U16 | MirType::U32 | MirType::U64
            | MirType::F32 | MirType::F64
            | MirType::Char
            | MirType::Handle
            | MirType::FuncPtr(_)
    )
}
