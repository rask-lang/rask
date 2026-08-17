// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Iterator chain recognition and fused loop lowering.
//!
//! Recognizes patterns like `vec.iter().filter(|x| p(x)).map(|x| f(x)).collect()`
//! and fuses them into a single index-based loop at MIR level.

use super::{LoweringError, MirLowerer, TypedOperand};
use crate::{
    operand::MirConst, BlockId, FunctionRef, LocalId, MirOperand, MirRValue, MirStmt,
    MirStmtKind, MirTerminator, MirTerminatorKind, MirType,
};
use rask_ast::expr::{Expr, ExprKind};
use rask_types::Type;

/// Internal state for iterator chain loop setup.
pub(super) struct IterLoopSetup {
    pub(super) idx: LocalId,
    pub(super) elem_local: LocalId,
    pub(super) elem_ty: MirType,
    pub(super) inc_block: BlockId,
    pub(super) exit_block: BlockId,
    pub(super) check_block: BlockId,
}

impl<'a> MirLowerer<'a> {
    /// Walk a method call chain backward to find .iter() and collect adapters.
    ///
    /// vec.iter().filter(|x| p(x)).map(|x| f(x))
    ///                                 ↑ start here, walk left
    ///
    /// Returns None if the chain doesn't end in .iter() or uses unsupported adapters.
    pub(super) fn try_parse_iter_chain<'e>(&self, expr: &'e Expr) -> Option<super::IterChain<'e>> {
        let mut adapters = Vec::new();
        let mut current = expr;

        loop {
            match &current.kind {
                ExprKind::MethodCall { object, method, args, .. } => {
                    match method.as_str() {
                        "iter" if args.is_empty() => {
                            adapters.reverse();
                            return Some(super::IterChain {
                                source: object,
                                adapters,
                            });
                        }
                        "filter" if args.len() == 1 => {
                            if matches!(&args[0].expr.kind, ExprKind::Closure { .. }) {
                                adapters.push(super::IterAdapter::Filter { closure: &args[0].expr });
                                current = object;
                            } else {
                                return None;
                            }
                        }
                        "map" if args.len() == 1 => {
                            if matches!(&args[0].expr.kind, ExprKind::Closure { .. }) {
                                adapters.push(super::IterAdapter::Map { closure: &args[0].expr });
                                current = object;
                            } else {
                                return None;
                            }
                        }
                        "take" if args.len() == 1 => {
                            adapters.push(super::IterAdapter::Take { count: &args[0].expr });
                            current = object;
                        }
                        "skip" if args.len() == 1 => {
                            adapters.push(super::IterAdapter::Skip { count: &args[0].expr });
                            current = object;
                        }
                        "enumerate" if args.is_empty() => {
                            adapters.push(super::IterAdapter::Enumerate);
                            current = object;
                        }
                        // Not an adapter. If the receiver is itself a
                        // collection, it's the source — see below.
                        _ => {
                            if self.is_iterable_source(current) {
                                adapters.reverse();
                                return Some(super::IterChain { source: current, adapters });
                            }
                            return None;
                        }
                    }
                }
                // `v.fold(...)` with no `.iter()` in between. A collection is
                // its own iteration source, so this is the same chain as
                // `v.iter().fold(...)` — without this the terminal fell through
                // to `{Type}_{method}` dispatch looking for a `Vec_fold` that
                // was never implemented (#462).
                _ => {
                    if self.is_iterable_source(current) {
                        adapters.reverse();
                        return Some(super::IterChain { source: current, adapters });
                    }
                    return None;
                }
            }
        }
    }

    /// Whether an expression can be iterated directly — a Vec, array or slice.
    ///
    /// Deliberately narrow: `setup_iter_chain_loop` indexes the source with
    /// `Vec_len` plus element loads, so anything that isn't laid out that way
    /// must keep falling through to normal method dispatch.
    fn is_iterable_source(&self, expr: &Expr) -> bool {
        match self.ctx.lookup_raw_type(expr.id) {
            Some(Type::Array { .. }) | Some(Type::Slice(_)) => return true,
            Some(Type::UnresolvedGeneric { name, .. }) if name == "Vec" => return true,
            Some(Type::UnresolvedNamed(name)) if name == "Vec" => return true,
            _ => {}
        }
        // The checker doesn't type every Ident node — an annotated
        // `const v: Vec<i64> = Vec.new()` leaves the later uses of `v`
        // untyped — so fall back to what lowering tracked for the local.
        if let ExprKind::Ident(name) = &expr.kind {
            if let Some(meta) = self.meta(name) {
                if meta.type_prefix.as_deref() == Some("Vec") {
                    return true;
                }
                if meta.full_type.as_deref().is_some_and(|t| t.trim_start().starts_with("Vec")) {
                    return true;
                }
            }
            if let Some((local_id, _)) = self.locals.get(name) {
                if matches!(self.builder.local_type(*local_id), Some(MirType::Array { .. })) {
                    return true;
                }
            }
        }
        // Everything the checker types `Iterator<T>` that native can actually
        // build is a Vec: `map.values()`, `s.split(",")`, `s.lines()` and the
        // rest all lower to a runtime call returning a `RaskVec *`. That's
        // exactly what the fused loop indexes, so they're sources — without
        // this `s.split_whitespace().collect()` fell through to
        // `Iterator_collect`, which native has no implementation of (#656).
        //
        // The test is on the *producer*, not just the type, because an adapter
        // is typed `Iterator<T>` too and `x.map(f)` is not a Vec. Adapters are
        // recognised as adapters before this runs; naming the producers keeps
        // one that isn't (a `map` with a named function rather than a closure)
        // from being mistaken for a source.
        if self.is_vec_backed_iterator(expr) {
            return true;
        }
        // The same value moved through a `let`. `map.values()` is
        // expression-scoped (std.collections), so a local holding one only
        // makes sense inside the source's scope — but native's failure was a
        // codegen error naming `Iterator_map`, not a diagnostic saying so
        // (#676). The local holds the Vec the producer returned, so the chain
        // fuses from here just as well as from the call.
        if let ExprKind::Ident(name) = &expr.kind {
            let typed_iterator = matches!(
                self.ctx.lookup_raw_type(expr.id),
                Some(Type::UnresolvedGeneric { ref name, .. }) if name == "Iterator"
            );
            if typed_iterator {
                return true;
            }
            if self.meta(name).and_then(|m| m.full_type.as_deref())
                .is_some_and(|t| t.trim_start().starts_with("Iterator"))
            {
                return true;
            }
        }
        false
    }

    /// A stdlib call that is typed `Iterator<T>` and returns a `RaskVec *`.
    ///
    /// `Map.iter()` is deliberately absent: `iter` is consumed as the chain's
    /// end marker before this runs, and the receiver becomes the source.
    fn is_vec_backed_iterator(&self, expr: &Expr) -> bool {
        let ExprKind::MethodCall { method, args, .. } = &expr.kind else {
            return false;
        };
        let materializes = match method.as_str() {
            "values" | "keys" | "lines" | "chars" | "split_whitespace" | "take_all" => {
                args.is_empty()
            }
            "split" => args.len() == 1,
            _ => false,
        };
        materializes
            && matches!(
                self.ctx.lookup_raw_type(expr.id),
                Some(Type::UnresolvedGeneric { ref name, .. }) if name == "Iterator"
            )
    }

    /// Try to handle an iterator terminal method (.collect, .fold, .any, etc.)
    /// by recognizing the chain and emitting a fused loop.
    ///
    /// Returns Some if handled, None to fall through to regular method call.
    pub(super) fn try_lower_iter_terminal(
        &mut self,
        _full_expr: &Expr,
        object: &Expr,
        method: &str,
        args: &[rask_ast::expr::CallArg],
    ) -> Result<Option<TypedOperand>, LoweringError> {
        match method {
            "to_vec" if args.is_empty() => {
                if let Some(chain) = self.try_parse_iter_chain(object) {
                    let result = self.lower_iter_collect(&chain)?;
                    return Ok(Some(result));
                }
            }
            "fold" if args.len() == 2 => {
                if let Some(chain) = self.try_parse_iter_chain(object) {
                    let result = self.lower_iter_fold(&chain, &args[0].expr, &args[1].expr)?;
                    return Ok(Some(result));
                }
            }
            // `v.map(f)` / `v.filter(f)` standing on their own produce a Vec, so
            // they're a chain with an implicit `.collect()`. Reaching here at
            // all means this adapter is the outermost call — when it's part of
            // a longer chain the terminal consumes it and this never runs.
            //
            // Without it these fell through to `Vec_map`, a runtime function
            // that ignores the closure-object/env ABI and segfaulted (#441).
            "map" | "filter" if args.len() == 1 => {
                if matches!(&args[0].expr.kind, ExprKind::Closure { .. }) {
                    if let Some(mut chain) = self.try_parse_iter_chain(object) {
                        chain.adapters.push(if method == "map" {
                            super::IterAdapter::Map { closure: &args[0].expr }
                        } else {
                            super::IterAdapter::Filter { closure: &args[0].expr }
                        });
                        let result = self.lower_iter_collect(&chain)?;
                        return Ok(Some(result));
                    }
                }
            }
            "any" if args.len() == 1 => {
                if let Some(chain) = self.try_parse_iter_chain(object) {
                    if matches!(&args[0].expr.kind, ExprKind::Closure { .. }) {
                        let result = self.lower_iter_any(&chain, &args[0].expr)?;
                        return Ok(Some(result));
                    }
                }
            }
            "all" if args.len() == 1 => {
                if let Some(chain) = self.try_parse_iter_chain(object) {
                    if matches!(&args[0].expr.kind, ExprKind::Closure { .. }) {
                        let result = self.lower_iter_all(&chain, &args[0].expr)?;
                        return Ok(Some(result));
                    }
                }
            }
            "count" if args.is_empty() => {
                if let Some(chain) = self.try_parse_iter_chain(object) {
                    let result = self.lower_iter_count(&chain)?;
                    return Ok(Some(result));
                }
            }
            "sum" if args.is_empty() => {
                if let Some(chain) = self.try_parse_iter_chain(object) {
                    let result = self.lower_iter_sum(&chain)?;
                    return Ok(Some(result));
                }
            }
            "find" if args.len() == 1 => {
                if let Some(chain) = self.try_parse_iter_chain(object) {
                    if matches!(&args[0].expr.kind, ExprKind::Closure { .. }) {
                        let result = self.lower_iter_find(&chain, &args[0].expr)?;
                        return Ok(Some(result));
                    }
                }
            }
            _ => {}
        }
        Ok(None)
    }

    /// Inline a closure body: substitute the closure parameter with a value
    /// and lower the body expression. Used to fuse iterator adapters.
    ///
    /// |x| x * 2  +  arg_op  →  lower(x * 2) with x bound to arg_op
    pub(super) fn inline_closure_body(
        &mut self,
        closure: &Expr,
        arg_op: MirOperand,
        arg_ty: MirType,
    ) -> Result<TypedOperand, LoweringError> {
        if let ExprKind::Closure { params, body, .. } = &closure.kind {
            if let Some(param) = params.first() {
                let param_name = &param.name;
                let saved = self.locals.remove(param_name);
                let param_local = self.builder.alloc_local(param_name.clone(), arg_ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: param_local,
                    rvalue: MirRValue::Use(arg_op),
                }));
                self.locals.insert(param_name.clone(), (param_local, arg_ty));

                // `|x| { return x % 2 == 0 }` is the ordinary way to write this
                // — Rask functions return explicitly. Lowering it with no
                // return target emitted a real Return terminator, which the
                // caller then overwrote with its own branch/goto. The computed
                // value was left dangling and the caller used the block's
                // fall-off value instead, so a predicate always read as its
                // placeholder (#462: the filter branched on a constant 0 and
                // every element was skipped).
                //
                // Give the body somewhere to return *to*: a result local and a
                // continuation block. The local's type isn't known until the
                // body is lowered, so it's allocated as a placeholder and
                // retyped afterwards.
                let result_local = self.builder.alloc_temp(MirType::I64);
                let cont_block = self.builder.create_block();

                let saved_return_target = self.inline_return_target.take();
                let saved_return_taken = self.inline_return_taken.take();
                self.inline_return_target = Some((result_local, cont_block));

                let (body_op, body_ty) = self.lower_expr(body)?;

                let returned_ty = self.inline_return_taken.take();
                self.inline_return_target = saved_return_target;
                self.inline_return_taken = saved_return_taken;

                // A body that fell off its end never used the target; keep its
                // value and route it through the same local so both shapes
                // leave the caller in the continuation block.
                if returned_ty.is_none() {
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                        dst: result_local,
                        rvalue: MirRValue::Use(body_op),
                    }));
                }
                // The returned expression's type is the real one; a body that
                // ends in `return` leaves lower_expr reporting its fall-off
                // placeholder instead.
                let body_ty = returned_ty.unwrap_or(body_ty);
                self.builder.terminate(MirTerminator::dummy(
                    MirTerminatorKind::Goto { target: cont_block },
                ));
                self.builder.switch_to_block(cont_block);
                self.builder.set_local_type(result_local, body_ty.clone());

                self.locals.remove(param_name);
                if let Some(prev) = saved {
                    self.locals.insert(param_name.clone(), prev);
                }
                return Ok((MirOperand::Local(result_local), body_ty));
            }
        }
        Err(LoweringError::InvalidConstruct("expected closure".to_string()))
    }

    /// Set up the index loop infrastructure for an iterator chain.
    pub(super) fn setup_iter_chain_loop(
        &mut self,
        chain: &super::IterChain<'_>,
    ) -> Result<IterLoopSetup, LoweringError> {
        let (source_op, source_ty) = self.lower_expr(chain.source)?;
        let is_array = matches!(&source_ty, MirType::Array { .. });
        let (array_len, array_elem_size) = match &source_ty {
            MirType::Array { elem, len } => (Some(*len), Some(elem.size())),
            _ => (None, None),
        };

        let collection = self.builder.alloc_temp(source_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: collection,
            rvalue: MirRValue::Use(source_op),
        }));

        // len
        let len_local = self.builder.alloc_temp(MirType::I64);
        if let Some(arr_len) = array_len {
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

        // Process Skip/Take adapters to adjust start/end bounds
        let mut start_val: Option<MirOperand> = None;
        let mut end_op = MirOperand::Local(len_local);
        let mut took = false;

        for adapter in &chain.adapters {
            match adapter {
                super::IterAdapter::Skip { count } => {
                    let (skip_op, _) = self.lower_expr(count)?;
                    start_val = Some(skip_op);
                }
                super::IterAdapter::Take { count } => {
                    let (take_op, _) = self.lower_expr(count)?;
                    took = true;
                    if let Some(ref start) = start_val {
                        let adjusted = self.builder.alloc_temp(MirType::I64);
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                            dst: adjusted,
                            rvalue: MirRValue::BinaryOp {
                                op: crate::operand::BinOp::Add,
                                left: start.clone(),
                                right: take_op,
                            },
                        }));
                        end_op = MirOperand::Local(adjusted);
                    } else {
                        end_op = take_op;
                    }
                }
                _ => {} // Filter/Map/Enumerate handled inside the loop body
            }
        }

        // `take(n)` asks for at most n, not exactly n — clamp the end to the
        // source length. Unclamped, `.skip(0).take(50)` over three elements ran
        // the loop to index 50 and panicked on the first read past the end.
        if took {
            let end_local = self.builder.alloc_temp(MirType::I64);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: end_local,
                rvalue: MirRValue::Use(end_op),
            }));
            let over = self.builder.alloc_temp(MirType::Bool);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: over,
                rvalue: MirRValue::BinaryOp {
                    op: crate::operand::BinOp::Gt,
                    left: MirOperand::Local(end_local),
                    right: MirOperand::Local(len_local),
                },
            }));
            let clamp_block = self.builder.create_block();
            let after_block = self.builder.create_block();
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                cond: MirOperand::Local(over),
                then_block: clamp_block,
                else_block: after_block,
            }));
            self.builder.switch_to_block(clamp_block);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: end_local,
                rvalue: MirRValue::Use(MirOperand::Local(len_local)),
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                target: after_block,
            }));
            self.builder.switch_to_block(after_block);
            end_op = MirOperand::Local(end_local);
        }

        let idx = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: idx,
            rvalue: MirRValue::Use(start_val.unwrap_or(MirOperand::Constant(MirConst::Int(0)))),
        }));

        let check_block = self.builder.create_block();
        let body_block = self.builder.create_block();
        let inc_block = self.builder.create_block();
        let exit_block = self.builder.create_block();

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: check_block }));

        // check: idx < end
        self.builder.switch_to_block(check_block);
        let cond = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: cond,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Lt,
                left: MirOperand::Local(idx),
                right: end_op,
            },
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(cond),
            then_block: body_block,
            else_block: exit_block,
        }));

        // body: load element
        self.builder.switch_to_block(body_block);
        let elem_ty = self.extract_iterator_elem_type(chain.source)
            .or_else(|| self.collection_elem_of_expr(chain.source))
            .unwrap_or_else(|| crate::fallback::i64_fallback("lower/iterators:chain_elem"));
        let elem_local = self.builder.alloc_temp(elem_ty.clone());
        if is_array {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: elem_local,
                rvalue: MirRValue::ArrayIndex {
                    base: MirOperand::Local(collection),
                    index: MirOperand::Local(idx),
                    elem_size: array_elem_size.unwrap_or(8),
                },
            }));
        } else {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(elem_local),
                func: FunctionRef::internal("Vec_get".to_string()),
                args: vec![MirOperand::Local(collection), MirOperand::Local(idx)],
            }));
        }

        Ok(IterLoopSetup {
            idx,
            elem_local,
            elem_ty,
            inc_block,
            exit_block,
            check_block,
        })
    }

    /// Apply filter/map/enumerate adapters inside a loop body.
    /// Returns the final (operand, type) after all adapters.
    /// For filter adapters, emits a branch that skips to inc_block on false.
    pub(super) fn apply_iter_adapters(
        &mut self,
        chain: &super::IterChain<'_>,
        elem_op: MirOperand,
        elem_ty: MirType,
        inc_block: BlockId,
        idx: LocalId,
    ) -> Result<TypedOperand, LoweringError> {
        let mut current_op = elem_op;
        let mut current_ty = elem_ty;

        for adapter in &chain.adapters {
            match adapter {
                super::IterAdapter::Filter { closure } => {
                    let (pred_op, _) = self.inline_closure_body(closure, current_op.clone(), current_ty.clone())?;
                    let pass_block = self.builder.create_block();
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                        cond: pred_op,
                        then_block: pass_block,
                        else_block: inc_block,
                    }));
                    self.builder.switch_to_block(pass_block);
                }
                super::IterAdapter::Map { closure } => {
                    let (mapped_op, mapped_ty) = self.inline_closure_body(closure, current_op, current_ty)?;
                    current_op = mapped_op;
                    current_ty = mapped_ty;
                }
                super::IterAdapter::Enumerate => {
                    let tuple_ty = MirType::Tuple(vec![MirType::I64, current_ty.clone()]);
                    let tuple_local = self.builder.alloc_temp(tuple_ty.clone());
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                        addr: tuple_local,
                        offset: 0,
                        value: MirOperand::Local(idx),
                        store_size: None,
                    }));
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                        addr: tuple_local,
                        offset: 8,
                        value: current_op,
                        store_size: None,
                    }));
                    current_op = MirOperand::Local(tuple_local);
                    current_ty = tuple_ty;
                }
                super::IterAdapter::Skip { .. } | super::IterAdapter::Take { .. } => {
                    // Already handled in setup (start/end bounds)
                }
            }
        }

        Ok((current_op, current_ty))
    }

    /// Emit the increment block: idx += 1, goto check
    pub(super) fn emit_iter_increment(
        &mut self,
        idx: LocalId,
        inc_block: BlockId,
        check_block: BlockId,
    ) {
        self.builder.switch_to_block(inc_block);
        let incremented = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: incremented,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Add,
                left: MirOperand::Local(idx),
                right: MirOperand::Constant(MirConst::Int(1)),
            },
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: idx,
            rvalue: MirRValue::Use(MirOperand::Local(incremented)),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: check_block }));
    }

    /// .collect() — fused loop that pushes each result into a new Vec.
    pub(super) fn lower_iter_collect(
        &mut self,
        chain: &super::IterChain<'_>,
    ) -> Result<TypedOperand, LoweringError> {
        let result_vec = self.builder.alloc_temp(MirType::I64);
        // The element size isn't known until the adapters have been lowered —
        // `.map(|r| r.view.clone())` collects whatever the closure returns. Note
        // where the call lands and fill the size in below; a bare `Vec_new()`
        // defaulted to an 8-byte stride, so collecting structs overlapped them.
        let vec_new_pos = self.builder.next_stmt_pos();
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(result_vec),
            func: FunctionRef::internal("Vec_new".to_string()),
            args: vec![],
        }));

        let setup = self.setup_iter_chain_loop(chain)?;
        let (final_op, final_ty) = self.apply_iter_adapters(
            chain, MirOperand::Local(setup.elem_local), setup.elem_ty,
            setup.inc_block, setup.idx,
        )?;
        let elem_size = Self::mir_slot_size(&final_ty);
        if elem_size > 0 {
            self.builder.set_call_args(
                vec_new_pos.0,
                vec_new_pos.1,
                "Vec_new",
                vec![MirOperand::Constant(MirConst::Int(elem_size))],
            );
            self.collected_elem_types.insert(result_vec, final_ty);
        }

        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::internal("Vec_push".to_string()),
            args: vec![MirOperand::Local(result_vec), final_op],
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: setup.inc_block }));

        self.emit_iter_increment(setup.idx, setup.inc_block, setup.check_block);
        self.builder.switch_to_block(setup.exit_block);
        Ok((MirOperand::Local(result_vec), MirType::I64))
    }

    /// .fold(init, |acc, x| body) — fused loop with accumulator.
    pub(super) fn lower_iter_fold(
        &mut self,
        chain: &super::IterChain<'_>,
        init: &Expr,
        closure: &Expr,
    ) -> Result<TypedOperand, LoweringError> {
        let (init_op, init_ty) = self.lower_expr(init)?;
        let acc = self.builder.alloc_temp(init_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: acc,
            rvalue: MirRValue::Use(init_op),
        }));

        let setup = self.setup_iter_chain_loop(chain)?;
        let (final_op, final_ty) = self.apply_iter_adapters(
            chain, MirOperand::Local(setup.elem_local), setup.elem_ty,
            setup.inc_block, setup.idx,
        )?;

        // Inline the fold closure with two args: (acc, elem)
        if let ExprKind::Closure { params, body, .. } = &closure.kind {
            if params.len() == 2 {
                let acc_name = &params[0].name;
                let elem_name = &params[1].name;

                let saved_acc = self.locals.remove(acc_name);
                let saved_elem = self.locals.remove(elem_name);

                let acc_param = self.builder.alloc_local(acc_name.clone(), init_ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: acc_param,
                    rvalue: MirRValue::Use(MirOperand::Local(acc)),
                }));
                self.locals.insert(acc_name.clone(), (acc_param, init_ty.clone()));

                let elem_param = self.builder.alloc_local(elem_name.clone(), final_ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: elem_param,
                    rvalue: MirRValue::Use(final_op),
                }));
                self.locals.insert(elem_name.clone(), (elem_param, final_ty));

                let after_body = self.builder.create_block();

                let saved_return_target = self.inline_return_target.take();
                let saved_return_taken = self.inline_return_taken.take();
                self.inline_return_target = Some((acc, setup.inc_block));

                let (result_op, _) = self.lower_expr(body)?;

                let returned = self.inline_return_taken.take().is_some();
                self.inline_return_target = saved_return_target;
                self.inline_return_taken = saved_return_taken;

                // `|acc, x| { return acc + x }` already stored the result and
                // jumped; the body has no fall-off value to take. Assigning one
                // anyway overwrote the accumulator with a placeholder on every
                // iteration, so the fold produced its init value (#462).
                if !returned {
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                        dst: acc,
                        rvalue: MirRValue::Use(result_op),
                    }));
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: after_body }));
                }

                self.builder.switch_to_block(after_body);

                self.locals.remove(acc_name);
                self.locals.remove(elem_name);
                if let Some(prev) = saved_acc { self.locals.insert(acc_name.clone(), prev); }
                if let Some(prev) = saved_elem { self.locals.insert(elem_name.clone(), prev); }
            }
        }

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: setup.inc_block }));
        self.emit_iter_increment(setup.idx, setup.inc_block, setup.check_block);
        self.builder.switch_to_block(setup.exit_block);
        Ok((MirOperand::Local(acc), init_ty))
    }

    /// .any(|x| pred) — fused loop, short-circuit on first true.
    pub(super) fn lower_iter_any(
        &mut self,
        chain: &super::IterChain<'_>,
        predicate: &Expr,
    ) -> Result<TypedOperand, LoweringError> {
        let result = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: result,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Bool(false))),
        }));

        let setup = self.setup_iter_chain_loop(chain)?;
        let (final_op, final_ty) = self.apply_iter_adapters(
            chain, MirOperand::Local(setup.elem_local), setup.elem_ty,
            setup.inc_block, setup.idx,
        )?;

        let (pred_op, _) = self.inline_closure_body(predicate, final_op, final_ty)?;
        let found_block = self.builder.create_block();
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: pred_op,
            then_block: found_block,
            else_block: setup.inc_block,
        }));

        self.builder.switch_to_block(found_block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: result,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Bool(true))),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: setup.exit_block }));

        self.emit_iter_increment(setup.idx, setup.inc_block, setup.check_block);
        self.builder.switch_to_block(setup.exit_block);
        Ok((MirOperand::Local(result), MirType::Bool))
    }

    /// .all(|x| pred) — fused loop, short-circuit on first false.
    pub(super) fn lower_iter_all(
        &mut self,
        chain: &super::IterChain<'_>,
        predicate: &Expr,
    ) -> Result<TypedOperand, LoweringError> {
        let result = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: result,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Bool(true))),
        }));

        let setup = self.setup_iter_chain_loop(chain)?;
        let (final_op, final_ty) = self.apply_iter_adapters(
            chain, MirOperand::Local(setup.elem_local), setup.elem_ty,
            setup.inc_block, setup.idx,
        )?;

        let (pred_op, _) = self.inline_closure_body(predicate, final_op, final_ty)?;
        let fail_block = self.builder.create_block();
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: pred_op,
            then_block: setup.inc_block,
            else_block: fail_block,
        }));

        self.builder.switch_to_block(fail_block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: result,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Bool(false))),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: setup.exit_block }));

        self.emit_iter_increment(setup.idx, setup.inc_block, setup.check_block);
        self.builder.switch_to_block(setup.exit_block);
        Ok((MirOperand::Local(result), MirType::Bool))
    }

    /// .count() — fused loop counting elements that pass filters.
    pub(super) fn lower_iter_count(
        &mut self,
        chain: &super::IterChain<'_>,
    ) -> Result<TypedOperand, LoweringError> {
        let counter = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: counter,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
        }));

        let setup = self.setup_iter_chain_loop(chain)?;
        let _ = self.apply_iter_adapters(
            chain, MirOperand::Local(setup.elem_local), setup.elem_ty,
            setup.inc_block, setup.idx,
        )?;

        let incremented = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: incremented,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Add,
                left: MirOperand::Local(counter),
                right: MirOperand::Constant(MirConst::Int(1)),
            },
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: counter,
            rvalue: MirRValue::Use(MirOperand::Local(incremented)),
        }));

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: setup.inc_block }));
        self.emit_iter_increment(setup.idx, setup.inc_block, setup.check_block);
        self.builder.switch_to_block(setup.exit_block);
        Ok((MirOperand::Local(counter), MirType::I64))
    }

    /// .sum() — fused loop accumulating with Add.
    pub(super) fn lower_iter_sum(
        &mut self,
        chain: &super::IterChain<'_>,
    ) -> Result<TypedOperand, LoweringError> {
        let acc = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: acc,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
        }));

        let setup = self.setup_iter_chain_loop(chain)?;
        let (final_op, _) = self.apply_iter_adapters(
            chain, MirOperand::Local(setup.elem_local), setup.elem_ty,
            setup.inc_block, setup.idx,
        )?;

        let sum = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: sum,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Add,
                left: MirOperand::Local(acc),
                right: final_op,
            },
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: acc,
            rvalue: MirRValue::Use(MirOperand::Local(sum)),
        }));

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: setup.inc_block }));
        self.emit_iter_increment(setup.idx, setup.inc_block, setup.check_block);
        self.builder.switch_to_block(setup.exit_block);
        Ok((MirOperand::Local(acc), MirType::I64))
    }

    /// .find(|x| pred) — fused loop, return Some on first match, None otherwise.
    pub(super) fn lower_iter_find(
        &mut self,
        chain: &super::IterChain<'_>,
        predicate: &Expr,
    ) -> Result<TypedOperand, LoweringError> {
        let result = self.builder.alloc_temp(MirType::Option(Box::new(MirType::I64)));
        // Start as None (tag = 1)
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result,
            offset: 0,
            value: MirOperand::Constant(MirConst::Int(1)),
            store_size: None,
        }));

        let setup = self.setup_iter_chain_loop(chain)?;
        let (final_op, final_ty) = self.apply_iter_adapters(
            chain, MirOperand::Local(setup.elem_local), setup.elem_ty,
            setup.inc_block, setup.idx,
        )?;

        let (pred_op, _) = self.inline_closure_body(predicate, final_op.clone(), final_ty)?;
        let found_block = self.builder.create_block();
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: pred_op,
            then_block: found_block,
            else_block: setup.inc_block,
        }));

        self.builder.switch_to_block(found_block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result,
            offset: 0,
            value: MirOperand::Constant(MirConst::Int(0)), // Some tag
            store_size: None,
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result,
            offset: 8,
            value: final_op,
            store_size: None,
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: setup.exit_block }));

        self.emit_iter_increment(setup.idx, setup.inc_block, setup.check_block);
        self.builder.switch_to_block(setup.exit_block);
        Ok((MirOperand::Local(result), MirType::Option(Box::new(MirType::I64))))
    }
}
