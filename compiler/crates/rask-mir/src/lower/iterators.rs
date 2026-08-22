// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Iterator chain recognition and fused loop lowering.
//!
//! Recognizes patterns like `vec.iter().filter(|x| p(x)).map(|x| f(x)).collect()`
//! and fuses them into a single index-based loop at MIR level.

use super::{LoweringError, MirLowerer, TypedOperand};
use crate::{
    operand::MirConst, BlockId, FieldAccess, FunctionRef, LocalId, MirOperand, MirRValue, MirStmt,
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
            // A field access (`t.deps`, `pool[h].field`) resolves through the
            // checker's `HasField` constraint to the *registered* form, not the
            // unresolved one above — without this, `.filter()`/`.map()` on any
            // Vec-typed field fell through to the raw `Vec_filter`/`Vec_map`
            // builtins, which don't pass the closure's env pointer and crash.
            // `type_names` stores the declaration name with its parameter list
            // attached ("Vec<T>"), same as `collection_elem_type` in the checker.
            Some(Type::Generic { base, .. })
                if self
                    .ctx
                    .type_names
                    .get(base)
                    .is_some_and(|n| n.split('<').next() == Some("Vec")) =>
            {
                return true;
            }
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
            "values" | "keys" | "lines" | "chars" | "bytes"
            | "split_whitespace" | "take_all" => {
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
            // SEQ29: same fused loop, `Map_insert` per pair instead of a push.
            // The checker has already rejected a non-pair element type.
            "to_map" if args.is_empty() => {
                if let Some(chain) = self.try_parse_iter_chain(object) {
                    let result = self.lower_iter_to_map(&chain)?;
                    return Ok(Some(result));
                }
            }
            // SEQ30: materialize the chain, then the existing string join. The
            // separator is the one argument.
            //
            // Only for a real chain. `try_parse_iter_chain` also succeeds on a
            // bare Vec, and `Vec.join` already picks between the string and i64
            // runtime by element type — taking this path for `numbers.join(", ")`
            // on a `Vec<i64>` sent it to the string one and printed nothing.
            "join" if args.len() == 1 => {
                if let Some(chain) = self.try_parse_iter_chain(object)
                    .filter(|c| !c.adapters.is_empty() || self.source_is_a_sequence(c.source))
                {
                    let (vec_op, _) = self.lower_iter_collect(&chain)?;
                    let (sep_op, _) = self.lower_expr(&args[0].expr)?;
                    let dst = self.builder.alloc_temp(MirType::String);
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: Some(dst),
                        func: FunctionRef::internal("Vec_join".to_string()),
                        args: vec![vec_op, sep_op],
                    }));
                    return Ok(Some((MirOperand::Local(dst), MirType::String)));
                }
            }
            // `min`/`max` — the same fused loop, keeping the running extreme.
            // They were the only iterator terminals without a lowering, so
            // `v.iter().min()` reached codegen as a call to `Vec_iter`, which
            // doesn't exist: "Function not found: Vec_iter".
            "min" | "max" if args.is_empty() => {
                if let Some(chain) = self.try_parse_iter_chain(object) {
                    let result = self.lower_iter_extreme(&chain, method == "max")?;
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
            // `v.enumerate()` on its own is `v.iter().enumerate().to_vec()`.
            // Same argument as `map`/`filter` above, and the adapter already
            // existed — only the standalone spelling was missing, so it reached
            // codegen as `Vec_enumerate`, which nothing emits (#886).
            "enumerate" if args.is_empty() => {
                if let Some(mut chain) = self.try_parse_iter_chain(object) {
                    chain.adapters.push(super::IterAdapter::Enumerate);
                    let result = self.lower_iter_collect(&chain)?;
                    return Ok(Some(result));
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
            // Same loop as `find`, keeping the count of yielded elements
            // instead of the element. Without it `v.position(p)` reached
            // codegen as a call to `Vec_position`, which doesn't exist (#842).
            "position" if args.len() == 1 => {
                if let Some(chain) = self.try_parse_iter_chain(object) {
                    if matches!(&args[0].expr.kind, ExprKind::Closure { .. }) {
                        let result = self.lower_iter_position(&chain, &args[0].expr)?;
                        return Ok(Some(result));
                    }
                }
            }
            // `v.read(i, f)` and `v.modify(i, f)`. Not an iterator chain — an
            // index and one call — but this is where a collection method with a
            // closure gets its chance, and both reached codegen as "Function not
            // found: Vec_read" while the interpreter ran them (#842).
            "read" | "modify" if args.len() == 2 => {
                // Only for a Vec. A Map is a pointer too, and taking this path
                // for `m.modify(key, f)` ran `Vec_len`/`Vec_get` over a Map — no
                // crash, just `none` for a key that was there. The checker's
                // recorded receiver is the one thing that tells them apart.
                let on_vec = Self::prefix_is(&self.ctx.recorded_prefix(_full_expr.id), "Vec");
                if on_vec && matches!(&args[1].expr.kind, ExprKind::Closure { .. }) {
                    if let Some(result) =
                        self.lower_vec_element_closure(method, object, args)?
                    {
                        return Ok(Some(result));
                    }
                }
                let on_map = Self::prefix_is(&self.ctx.recorded_prefix(_full_expr.id), "Map");
                if on_map && matches!(&args[1].expr.kind, ExprKind::Closure { .. }) {
                    if let Some(result) =
                        self.lower_map_value_closure(method, object, args)?
                    {
                        return Ok(Some(result));
                    }
                }
            }
            // The entry API: insert the default when the key is missing, then
            // modify whatever is there. Answers `R`, not `R?` — after the
            // insert there is always a value.
            "modify_with_default" if args.len() == 3 => {
                let on_map = Self::prefix_is(&self.ctx.recorded_prefix(_full_expr.id), "Map");
                let both_closures = matches!(&args[1].expr.kind, ExprKind::Closure { .. })
                    && matches!(&args[2].expr.kind, ExprKind::Closure { .. });
                if on_map && both_closures {
                    if let Some(result) = self.lower_map_entry_modify(object, args)? {
                        return Ok(Some(result));
                    }
                }
            }
            // `reduce` is `fold` with the first element as the seed, so it has
            // no value for an empty source and answers `T?`.
            "reduce" if args.len() == 1 => {
                if let Some(chain) = self.try_parse_iter_chain(object) {
                    if matches!(&args[0].expr.kind, ExprKind::Closure { .. }) {
                        let result = self.lower_iter_reduce(&chain, &args[0].expr)?;
                        return Ok(Some(result));
                    }
                }
            }
            // `v.flat_map(f)` on its own is the same "implicit .collect()"
            // reasoning as `map`/`filter` above, except each pushed value is one
            // of `f(elem)`'s own elements, not `f(elem)` itself. Without this it
            // reached codegen as a call to `Vec_flat_map`, which nothing emits
            // (#842).
            "flat_map" if args.len() == 1 => {
                if matches!(&args[0].expr.kind, ExprKind::Closure { .. }) {
                    if let Some(chain) = self.try_parse_iter_chain(object) {
                        let result = self.lower_iter_flat_map(&chain, &args[0].expr)?;
                        return Ok(Some(result));
                    }
                }
            }
            // `v.sort_by_key(f)` — in place, answers unit. Not an iterator
            // chain (it mutates rather than produces a value) but it's a Vec
            // method with a closure the same way `read`/`modify` are, and it
            // reached codegen as a call to `Vec_sort_by_key`, which nothing
            // emits (#842).
            "sort_by_key" if args.len() == 1 => {
                if matches!(&args[0].expr.kind, ExprKind::Closure { .. }) {
                    let result = self.lower_iter_sort_by_key(object, &args[0].expr)?;
                    return Ok(Some(result));
                }
            }
            _ => {}
        }
        Ok(None)
    }

    /// The base name of a recorded receiver prefix, which can carry its generic
    /// arguments — `Vec<i64>` is still a Vec.
    /// Is this chain source already a sequence — an `Iterator<T>` — rather than a
    /// collection?
    ///
    /// It decides who answers `join` on a chain with no adapters. A `Vec` answers
    /// it itself and picks the right runtime by element type, so
    /// `numbers.join(", ")` on a `Vec<i64>` has to keep going there. A sequence
    /// has nothing to answer with: `s.split(", ").join("|")` reached codegen as a
    /// call to `Iterator_join`, which nothing emits, while
    /// `s.split(", ").to_vec().join("|")` two characters longer worked (#878).
    ///
    /// Asked positively, off the checker's own type. Asking the other way round —
    /// "is this *not* a Vec" — answers yes whenever the prefix simply wasn't
    /// recorded, which sent `Vec<i64>.join` down the materializing path and
    /// printed nothing.
    fn source_is_a_sequence(&self, source: &Expr) -> bool {
        let name = match self.ctx.lookup_raw_type(source.id) {
            Some(rask_types::Type::UnresolvedGeneric { name, .. }) => name.clone(),
            Some(rask_types::Type::Generic { base, .. }) => {
                match self.ctx.type_names.get(base) {
                    Some(n) => n.clone(),
                    None => return false,
                }
            }
            _ => return false,
        };
        name.split('<').next().unwrap_or(&name).trim() == "Iterator"
    }

    fn prefix_is(prefix: &Option<String>, want: &str) -> bool {
        prefix
            .as_deref()
            .is_some_and(|p| p.split('<').next().unwrap_or(p).trim() == want)
    }

    /// `m.modify_with_default(k, || default, f)`: insert the default when `k`
    /// is missing, then hand the value to `f` and keep what it leaves.
    ///
    /// Answers `R` rather than `R?` — after the insert there is always a value,
    /// which is the whole point of the entry API (one lookup, no absent case for
    /// the caller to handle).
    fn lower_map_entry_modify(
        &mut self,
        object: &Expr,
        args: &[rask_ast::expr::CallArg],
    ) -> Result<Option<TypedOperand>, LoweringError> {
        let (obj_op, obj_ty) = self.lower_expr(object)?;
        let value_ty = self
            .collection_elem_of_expr(object)
            .unwrap_or_else(|| crate::fallback::i64_fallback("lower/iterators:map_entry_modify"));

        let map = self.builder.alloc_temp(obj_ty);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: map,
            rvalue: MirRValue::Use(obj_op),
        }));
        let (key_op, key_ty) = self.lower_expr(&args[0].expr)?;
        let key = self.builder.alloc_temp(key_ty);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: key,
            rvalue: MirRValue::Use(key_op),
        }));

        let present = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(present),
            func: FunctionRef::internal("Map_contains_key".to_string()),
            args: vec![MirOperand::Local(map), MirOperand::Local(key)],
        }));

        let insert_block = self.builder.create_block();
        let have_block = self.builder.create_block();
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(present),
            then_block: have_block,
            else_block: insert_block,
        }));

        // Missing: run the factory and put its value in, so the modify path
        // below has something to read either way.
        self.builder.switch_to_block(insert_block);
        let (default_op, _) = self.inline_closure_no_arg(&args[1].expr)?;
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::internal("Map_insert".to_string()),
            args: vec![MirOperand::Local(map), MirOperand::Local(key), default_op],
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: have_block,
        }));

        self.builder.switch_to_block(have_block);
        let value = self.builder.alloc_temp(value_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(value),
            func: FunctionRef::internal("Map_get_unwrap".to_string()),
            args: vec![MirOperand::Local(map), MirOperand::Local(key)],
        }));
        let ((body_op, body_ty), param_local) = self.inline_closure_keeping_param(
            &args[2].expr,
            MirOperand::Local(value),
            value_ty,
        )?;
        if let Some(param) = param_local {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::internal("Map_insert".to_string()),
                args: vec![
                    MirOperand::Local(map),
                    MirOperand::Local(key),
                    MirOperand::Local(param),
                ],
            }));
        }
        Ok(Some((body_op, body_ty)))
    }

    /// Inline a closure that takes nothing — `|| default`. The same machinery as
    /// `inline_closure_keeping_param` without a parameter to bind.
    fn inline_closure_no_arg(
        &mut self,
        closure: &Expr,
    ) -> Result<TypedOperand, LoweringError> {
        let ExprKind::Closure { body, .. } = &closure.kind else {
            return Err(LoweringError::InvalidConstruct("expected closure".to_string()));
        };
        let result_local = self.builder.alloc_temp(MirType::I64);
        let cont_block = self.builder.create_block();
        let saved_return_target = self.inline_return_target.take();
        let saved_return_taken = self.inline_return_taken.take();
        self.inline_return_target = Some((result_local, cont_block));

        let (body_op, body_ty) = self.lower_expr(body)?;

        let returned_ty = self.inline_return_taken.take();
        self.inline_return_target = saved_return_target;
        self.inline_return_taken = saved_return_taken;
        if returned_ty.is_none() {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: result_local,
                rvalue: MirRValue::Use(body_op),
            }));
        }
        let body_ty = returned_ty.unwrap_or(body_ty);
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: cont_block,
        }));
        self.builder.switch_to_block(cont_block);
        self.builder.set_local_type(result_local, body_ty.clone());
        Ok((MirOperand::Local(result_local), body_ty))
    }

    /// `read(k, f)` and `modify(k, f)` on a Map: hand the value at `k` to the
    /// closure, answer `R?`, and for `modify` put back whatever the closure left
    /// in its parameter.
    ///
    /// `Map_contains_key` decides the branch rather than `Map_get`, so the
    /// present path can use `Map_get_unwrap` and get the value itself instead of
    /// an optional to unwrap.
    fn lower_map_value_closure(
        &mut self,
        method: &str,
        object: &Expr,
        args: &[rask_ast::expr::CallArg],
    ) -> Result<Option<TypedOperand>, LoweringError> {
        let (obj_op, obj_ty) = self.lower_expr(object)?;
        let value_ty = self
            .collection_elem_of_expr(object)
            .unwrap_or_else(|| crate::fallback::i64_fallback("lower/iterators:map_value_closure"));

        let map = self.builder.alloc_temp(obj_ty);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: map,
            rvalue: MirRValue::Use(obj_op),
        }));
        let (key_op, key_ty) = self.lower_expr(&args[0].expr)?;
        let key = self.builder.alloc_temp(key_ty);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: key,
            rvalue: MirRValue::Use(key_op),
        }));

        let present = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(present),
            func: FunctionRef::internal("Map_contains_key".to_string()),
            args: vec![MirOperand::Local(map), MirOperand::Local(key)],
        }));

        let result = self.builder.alloc_temp(MirType::Option(Box::new(MirType::I64)));
        let present_block = self.builder.create_block();
        let absent_block = self.builder.create_block();
        let merge_block = self.builder.create_block();
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(present),
            then_block: present_block,
            else_block: absent_block,
        }));

        self.builder.switch_to_block(present_block);
        let value = self.builder.alloc_temp(value_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(value),
            func: FunctionRef::internal("Map_get_unwrap".to_string()),
            args: vec![MirOperand::Local(map), MirOperand::Local(key)],
        }));
        let ((body_op, body_ty), param_local) = self.inline_closure_keeping_param(
            &args[1].expr,
            MirOperand::Local(value),
            value_ty,
        )?;
        if method == "modify" {
            if let Some(param) = param_local {
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: None,
                    func: FunctionRef::internal("Map_insert".to_string()),
                    args: vec![
                        MirOperand::Local(map),
                        MirOperand::Local(key),
                        MirOperand::Local(param),
                    ],
                }));
            }
        }
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result,
            offset: 0,
            value: MirOperand::Constant(MirConst::Int(0)),
            store_size: None,
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result,
            offset: 8,
            value: body_op,
            store_size: Some(body_ty.size().max(1)),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: merge_block,
        }));

        self.builder.switch_to_block(absent_block);
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
        let result_ty = MirType::Option(Box::new(body_ty));
        self.builder.set_local_type(result, result_ty.clone());
        Ok(Some((MirOperand::Local(result), result_ty)))
    }

    /// `read(i, f)` and `modify(i, f)` on a Vec: hand element `i` to the
    /// closure, answer `R?`, and for `modify` keep whatever the closure left in
    /// its parameter.
    ///
    /// Out of range is `none` and touches nothing — the bounds check is here
    /// rather than in the runtime because the answer is a `T?`, and the runtime
    /// can't build one.
    ///
    /// The result slot is allocated before its payload type is known and
    /// retyped once the body has been lowered, the same way
    /// `inline_closure_keeping_param` handles its own result local: `R` is
    /// whatever the closure returns, and there's no way to know that without
    /// lowering it.
    fn lower_vec_element_closure(
        &mut self,
        method: &str,
        object: &Expr,
        args: &[rask_ast::expr::CallArg],
    ) -> Result<Option<TypedOperand>, LoweringError> {
        let (obj_op, obj_ty) = self.lower_expr(object)?;
        if !matches!(obj_ty, MirType::Ptr) {
            return Ok(None);
        }
        let elem_ty = self
            .collection_elem_of_expr(object)
            .unwrap_or_else(|| crate::fallback::i64_fallback("lower/iterators:element_closure"));

        let collection = self.builder.alloc_temp(obj_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: collection,
            rvalue: MirRValue::Use(obj_op),
        }));

        let (index_op, _) = self.lower_expr(&args[0].expr)?;
        let idx = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: idx,
            rvalue: MirRValue::Use(index_op),
        }));

        let len = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(len),
            func: FunctionRef::internal("Vec_len".to_string()),
            args: vec![MirOperand::Local(collection)],
        }));

        let non_negative = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: non_negative,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Ge,
                left: MirOperand::Local(idx),
                right: MirOperand::Constant(MirConst::Int(0)),
            },
        }));
        let below_len = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: below_len,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Lt,
                left: MirOperand::Local(idx),
                right: MirOperand::Local(len),
            },
        }));
        let in_range = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: in_range,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::And,
                left: MirOperand::Local(non_negative),
                right: MirOperand::Local(below_len),
            },
        }));

        let result = self.builder.alloc_temp(MirType::Option(Box::new(MirType::I64)));
        let present_block = self.builder.create_block();
        let absent_block = self.builder.create_block();
        let merge_block = self.builder.create_block();
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(in_range),
            then_block: present_block,
            else_block: absent_block,
        }));

        self.builder.switch_to_block(present_block);
        let elem = self.builder.alloc_temp(elem_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(elem),
            func: FunctionRef::internal("Vec_get".to_string()),
            args: vec![MirOperand::Local(collection), MirOperand::Local(idx)],
        }));
        let ((body_op, body_ty), param_local) = self.inline_closure_keeping_param(
            &args[1].expr,
            MirOperand::Local(elem),
            elem_ty,
        )?;
        if method == "modify" {
            if let Some(param) = param_local {
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: None,
                    func: FunctionRef::internal("Vec_set".to_string()),
                    args: vec![
                        MirOperand::Local(collection),
                        MirOperand::Local(idx),
                        MirOperand::Local(param),
                    ],
                }));
            }
        }
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result,
            offset: 0,
            value: MirOperand::Constant(MirConst::Int(0)),
            store_size: None,
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result,
            offset: 8,
            value: body_op,
            store_size: Some(body_ty.size().max(1)),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: merge_block,
        }));

        self.builder.switch_to_block(absent_block);
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
        let result_ty = MirType::Option(Box::new(body_ty));
        self.builder.set_local_type(result, result_ty.clone());
        Ok(Some((MirOperand::Local(result), result_ty)))
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
        let (result, _) = self.inline_closure_keeping_param(closure, arg_op, arg_ty)?;
        Ok(result)
    }

    /// The same, handing back the local the closure's parameter was bound to.
    ///
    /// `modify` needs it: the closure is given mutable access to the element and
    /// whatever it leaves there is the new element. Inlining is what makes that
    /// cheap — the parameter *is* a local, so a body that assigns to it has
    /// already written the value, and the write-back is one store of this local
    /// into the slot.
    pub(super) fn inline_closure_keeping_param(
        &mut self,
        closure: &Expr,
        arg_op: MirOperand,
        arg_ty: MirType,
    ) -> Result<(TypedOperand, Option<LocalId>), LoweringError> {
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
                return Ok((
                    (MirOperand::Local(result_local), body_ty),
                    Some(param_local),
                ));
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

        // When the source holds functions, an adapter's closure parameter holds
        // one too, and calling it needs the same registration a `for` binding
        // gets (#869). Without it `fs.map(|f| { return f(3) })` lowered `f(3)` as
        // a call to a function named `f`, found no signature for it, and gave up
        // on the return type (#870).
        //
        // Registered across the whole chain rather than per adapter: a closure
        // parameter's name is scoped to its own closure anyway, and the names
        // come back out at the end.
        let callable_params = self.register_callable_adapter_params(chain);

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

        for name in &callable_params {
            self.closure_locals.remove(name);
            self.func_sigs.remove(name);
        }
        Ok((current_op, current_ty))
    }

    /// Register every adapter closure's parameters as callable, when the source
    /// holds functions. Returns the names so they can be taken back out.
    ///
    /// Only up to the first `map`: after one, the element is whatever that
    /// closure returned, so a later adapter's parameter isn't a function any
    /// more.
    fn register_callable_adapter_params(&mut self, chain: &super::IterChain<'_>) -> Vec<String> {
        let Some(ret) = self.source_elem_fn_ret(chain.source) else {
            return Vec::new();
        };
        let mut names = Vec::new();
        for adapter in &chain.adapters {
            let closure = match adapter {
                super::IterAdapter::Map { closure } | super::IterAdapter::Filter { closure } => {
                    closure
                }
                _ => continue,
            };
            if let ExprKind::Closure { params, .. } = &closure.kind {
                for p in params {
                    if self.closure_locals.insert(p.name.clone()) {
                        names.push(p.name.clone());
                    }
                    self.func_sigs.insert(
                        p.name.clone(),
                        super::FuncSig {
                            ret_ty: ret.clone(),
                            scalar_mutate_params: Vec::new(),
                            aggregate_mutate_params: Vec::new(),
                            ret_vec_elem: None,
                            param_ty_strs: Vec::new(),
                        },
                    );
                }
            }
            if matches!(adapter, super::IterAdapter::Map { .. }) {
                break;
            }
        }
        names
    }

    /// The MIR return type of the functions a source holds, when it holds
    /// functions. `Vec<func(i64) -> i64>` answers `i64`; anything else answers
    /// nothing.
    fn source_elem_fn_ret(&self, source: &Expr) -> Option<MirType> {
        let ty = self.ctx.lookup_raw_type(source.id)?;
        let (name, args) = self.generic_head(ty)?;
        if !matches!(name.as_str(), "Vec" | "Iterator") {
            return None;
        }
        let rask_types::GenericArg::Type(elem) = args.first()? else { return None };
        match &**elem {
            rask_types::Type::Fn { ret, .. } => Some(self.ctx.type_to_mir(ret.as_ref())),
            _ => None,
        }
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

    /// `.to_map()` — the collect loop, inserting each pair instead of pushing.
    ///
    /// Later keys overwrite earlier ones, which is what repeated `insert` does
    /// anyway (SEQ29). The element is a 2-tuple, so the key and value come out
    /// of its two slots.
    pub(super) fn lower_iter_to_map(
        &mut self,
        chain: &super::IterChain<'_>,
    ) -> Result<TypedOperand, LoweringError> {
        let result_map = self.builder.alloc_temp(MirType::I64);
        let map_new_pos = self.builder.next_stmt_pos();
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(result_map),
            func: FunctionRef::internal("Map_new".to_string()),
            args: vec![],
        }));

        let setup = self.setup_iter_chain_loop(chain)?;
        let (final_op, final_ty) = self.apply_iter_adapters(
            chain, MirOperand::Local(setup.elem_local), setup.elem_ty,
            setup.inc_block, setup.idx,
        )?;

        let (key_ty, val_ty) = match &final_ty {
            MirType::Tuple(parts) if parts.len() == 2 => (parts[0].clone(), parts[1].clone()),
            other => {
                return Err(LoweringError::InvalidConstruct(format!(
                    "to_map needs a sequence of pairs, got {:?}",
                    other
                )))
            }
        };
        // `Map_new` sizes its key and value slots the way `Vec_new` sizes its
        // element — from the type the loop actually produces, filled in here
        // because the adapters decide it.
        self.builder.set_call_args(
            map_new_pos.0,
            map_new_pos.1,
            "Map_new",
            vec![
                MirOperand::Constant(MirConst::Int(Self::mir_slot_size(&key_ty))),
                MirOperand::Constant(MirConst::Int(Self::mir_slot_size(&val_ty))),
            ],
        );

        let key = self.builder.alloc_temp(key_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: key,
            rvalue: MirRValue::Field {
                base: final_op.clone(),
                field_index: 0,
                byte_offset: Some(0),
                access: FieldAccess::Word,
            },
        }));
        let val = self.builder.alloc_temp(val_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: val,
            rvalue: MirRValue::Field {
                base: final_op,
                field_index: 1,
                byte_offset: Some(Self::mir_slot_size(&key_ty) as u32),
                access: FieldAccess::Word,
            },
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::internal("Map_insert".to_string()),
            args: vec![
                MirOperand::Local(result_map),
                MirOperand::Local(key),
                MirOperand::Local(val),
            ],
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: setup.inc_block }));

        self.emit_iter_increment(setup.idx, setup.inc_block, setup.check_block);
        self.builder.switch_to_block(setup.exit_block);
        Ok((MirOperand::Local(result_map), MirType::I64))
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

    /// `.min()` / `.max()` — fused loop keeping the running extreme, `none` for
    /// an empty sequence.
    ///
    /// The comparison is on the element as the loop produces it, so an adapter
    /// ahead of the terminal is already applied — `.map(f).max()` is the max of
    /// the mapped values, not of the sources.
    pub(super) fn lower_iter_extreme(
        &mut self,
        chain: &super::IterChain<'_>,
        want_max: bool,
    ) -> Result<TypedOperand, LoweringError> {
        let result = self.builder.alloc_temp(MirType::Option(Box::new(MirType::I64)));
        // none until the first element lands (tag 1).
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result,
            offset: 0,
            value: MirOperand::Constant(MirConst::Int(1)),
            store_size: None,
        }));

        let setup = self.setup_iter_chain_loop(chain)?;
        let (final_op, _) = self.apply_iter_adapters(
            chain, MirOperand::Local(setup.elem_local), setup.elem_ty,
            setup.inc_block, setup.idx,
        )?;

        // Take this element when the slot is still empty, or when it beats what's
        // there. Reading the tag is how "still empty" is asked.
        let tag = self.builder.alloc_temp(MirType::U8);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: tag,
            rvalue: MirRValue::EnumTag { value: MirOperand::Local(result) },
        }));
        let is_empty = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: is_empty,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Eq,
                left: MirOperand::Local(tag),
                right: MirOperand::Constant(MirConst::Int(1)),
            },
        }));
        let current = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: current,
            rvalue: MirRValue::Field {
                base: MirOperand::Local(result),
                field_index: 0,
                byte_offset: Some(8),
                access: FieldAccess::Word,
            },
        }));
        let beats = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: beats,
            rvalue: MirRValue::BinaryOp {
                op: if want_max {
                    crate::operand::BinOp::Gt
                } else {
                    crate::operand::BinOp::Lt
                },
                left: final_op.clone(),
                right: MirOperand::Local(current),
            },
        }));
        let take = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: take,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Or,
                left: MirOperand::Local(is_empty),
                right: MirOperand::Local(beats),
            },
        }));

        let store_block = self.builder.create_block();
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(take),
            then_block: store_block,
            else_block: setup.inc_block,
        }));

        self.builder.switch_to_block(store_block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result,
            offset: 0,
            value: MirOperand::Constant(MirConst::Int(0)), // present
            store_size: None,
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result,
            offset: 8,
            value: final_op,
            store_size: None,
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: setup.inc_block }));

        self.emit_iter_increment(setup.idx, setup.inc_block, setup.check_block);
        self.builder.switch_to_block(setup.exit_block);
        Ok((MirOperand::Local(result), MirType::Option(Box::new(MirType::I64))))
    }

    /// .find(|x| pred) — fused loop, return Some on first match, None otherwise.
    /// SEQ: `position(p)` — the index of the first yielded element the
    /// predicate accepts, counted over what the chain *yields*, so a `filter`
    /// in front of it doesn't leave gaps in the numbering.
    pub(super) fn lower_iter_position(
        &mut self,
        chain: &super::IterChain<'_>,
        predicate: &Expr,
    ) -> Result<TypedOperand, LoweringError> {
        let opt_ty = MirType::Option(Box::new(MirType::I64));
        let result = self.builder.alloc_temp(opt_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result,
            offset: 0,
            value: MirOperand::Constant(MirConst::Int(1)), // None
            store_size: None,
        }));
        let pos = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: pos,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
        }));

        let setup = self.setup_iter_chain_loop(chain)?;
        let (final_op, final_ty) = self.apply_iter_adapters(
            chain, MirOperand::Local(setup.elem_local), setup.elem_ty,
            setup.inc_block, setup.idx,
        )?;

        // This element's own position, before the counter moves on.
        let here = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: here,
            rvalue: MirRValue::Use(MirOperand::Local(pos)),
        }));
        let next = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: next,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Add,
                left: MirOperand::Local(pos),
                right: MirOperand::Constant(MirConst::Int(1)),
            },
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: pos,
            rvalue: MirRValue::Use(MirOperand::Local(next)),
        }));

        let (pred_op, _) = self.inline_closure_body(predicate, final_op, final_ty)?;
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
            value: MirOperand::Constant(MirConst::Int(0)), // Some
            store_size: None,
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result,
            offset: 8,
            value: MirOperand::Local(here),
            store_size: None,
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: setup.exit_block }));

        self.emit_iter_increment(setup.idx, setup.inc_block, setup.check_block);
        self.builder.switch_to_block(setup.exit_block);
        Ok((MirOperand::Local(result), opt_ty))
    }

    /// SEQ: `reduce(f)` — `fold` seeded with the first yielded element. The
    /// answer is `T?` because an empty source has no seed.
    pub(super) fn lower_iter_reduce(
        &mut self,
        chain: &super::IterChain<'_>,
        closure: &Expr,
    ) -> Result<TypedOperand, LoweringError> {
        let ExprKind::Closure { params, body, .. } = &closure.kind else {
            return Err(LoweringError::InvalidConstruct("reduce takes a closure".to_string()));
        };
        if params.len() != 2 {
            return Err(LoweringError::InvalidConstruct(
                "reduce's closure takes two parameters".to_string(),
            ));
        }

        let setup = self.setup_iter_chain_loop(chain)?;
        let (final_op, final_ty) = self.apply_iter_adapters(
            chain, MirOperand::Local(setup.elem_local), setup.elem_ty,
            setup.inc_block, setup.idx,
        )?;

        let opt_ty = MirType::Option(Box::new(final_ty.clone()));
        let acc = self.builder.alloc_temp(final_ty.clone());
        let have = self.builder.alloc_temp(MirType::Bool);

        // `have` and `acc` are declared outside the loop, but the loop body is
        // the only writer, so the initial store has to happen before the
        // header — which `setup_iter_chain_loop` has already emitted. Seed
        // them at the top of the function's current block instead by branching
        // on `have` each iteration: false takes the element as the seed.
        let seed_block = self.builder.create_block();
        let combine_block = self.builder.create_block();
        let after_block = self.builder.create_block();
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(have),
            then_block: combine_block,
            else_block: seed_block,
        }));

        self.builder.switch_to_block(seed_block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: acc,
            rvalue: MirRValue::Use(final_op.clone()),
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: have,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Bool(true))),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: after_block }));

        self.builder.switch_to_block(combine_block);
        let acc_name = &params[0].name;
        let elem_name = &params[1].name;
        let saved_acc = self.locals.remove(acc_name);
        let saved_elem = self.locals.remove(elem_name);

        let acc_param = self.builder.alloc_local(acc_name.clone(), final_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: acc_param,
            rvalue: MirRValue::Use(MirOperand::Local(acc)),
        }));
        self.locals.insert(acc_name.clone(), (acc_param, final_ty.clone()));
        let elem_param = self.builder.alloc_local(elem_name.clone(), final_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: elem_param,
            rvalue: MirRValue::Use(final_op),
        }));
        self.locals.insert(elem_name.clone(), (elem_param, final_ty.clone()));

        let saved_return_target = self.inline_return_target.take();
        let saved_return_taken = self.inline_return_taken.take();
        self.inline_return_target = Some((acc, after_block));
        let (result_op, _) = self.lower_expr(body)?;
        let returned = self.inline_return_taken.take().is_some();
        self.inline_return_target = saved_return_target;
        self.inline_return_taken = saved_return_taken;
        if !returned {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: acc,
                rvalue: MirRValue::Use(result_op),
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: after_block }));
        }

        self.locals.remove(acc_name);
        self.locals.remove(elem_name);
        if let Some(prev) = saved_acc { self.locals.insert(acc_name.clone(), prev); }
        if let Some(prev) = saved_elem { self.locals.insert(elem_name.clone(), prev); }

        self.builder.switch_to_block(after_block);
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: setup.inc_block }));

        self.emit_iter_increment(setup.idx, setup.inc_block, setup.check_block);
        self.builder.switch_to_block(setup.exit_block);

        // Build the `T?`: present only if the loop ran at least once.
        let result = self.builder.alloc_temp(opt_ty.clone());
        let some_block = self.builder.create_block();
        let none_block = self.builder.create_block();
        let done_block = self.builder.create_block();
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(have),
            then_block: some_block,
            else_block: none_block,
        }));
        self.builder.switch_to_block(some_block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result, offset: 0,
            value: MirOperand::Constant(MirConst::Int(0)),
            store_size: None,
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result, offset: 8,
            value: MirOperand::Local(acc),
            store_size: None,
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: done_block }));
        self.builder.switch_to_block(none_block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result, offset: 0,
            value: MirOperand::Constant(MirConst::Int(1)),
            store_size: None,
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: done_block }));
        self.builder.switch_to_block(done_block);
        Ok((MirOperand::Local(result), opt_ty))
    }

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

    /// `v.flat_map(f)` — for each element, run `f` and push every element of
    /// what it returns. Same fused loop as `.collect()`, with an inner loop
    /// over `f(elem)`'s own elements instead of a single push.
    ///
    /// `f` returns `Vec<U>`, but a Vec's own MIR type is opaque (`Ptr`/`I64`
    /// — it never carries what it holds), so `U`'s real size can't come from
    /// lowering the closure body the way `map`'s result type does. It has to
    /// come from the checker instead: find `f`'s return expression(s) and ask
    /// what Vec they resolve to.
    pub(super) fn lower_iter_flat_map(
        &mut self,
        chain: &super::IterChain<'_>,
        closure: &Expr,
    ) -> Result<TypedOperand, LoweringError> {
        let sub_elem_ty = Self::closure_return_exprs(closure)
            .into_iter()
            .find_map(|e| self.collection_elem_of_expr(e))
            .unwrap_or_else(|| crate::fallback::i64_fallback("lower/iterators:flat_map_elem"));

        let result_vec = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(result_vec),
            func: FunctionRef::internal("Vec_new".to_string()),
            args: vec![MirOperand::Constant(MirConst::Int(Self::mir_slot_size(&sub_elem_ty)))],
        }));

        let setup = self.setup_iter_chain_loop(chain)?;
        let (elem_op, elem_ty) = self.apply_iter_adapters(
            chain, MirOperand::Local(setup.elem_local), setup.elem_ty,
            setup.inc_block, setup.idx,
        )?;

        // The Vec the closure hands back is not freed, so one allocation per
        // element leaks (#943). It can't simply be freed here: the closure
        // doesn't have to have allocated it — `|k| lookup()` can hand back a
        // Vec that outlives the call, and freeing that is a use-after-free
        // rather than a leak fix. Doing better needs to know whether the result
        // is owned, which this lowering has no way to ask today.
        let (sub_op, _) = self.inline_closure_body(closure, elem_op, elem_ty)?;
        let sub_vec = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: sub_vec,
            rvalue: MirRValue::Use(sub_op),
        }));
        let sub_len = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(sub_len),
            func: FunctionRef::internal("Vec_len".to_string()),
            args: vec![MirOperand::Local(sub_vec)],
        }));
        let sub_idx = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: sub_idx,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
        }));

        let sub_check = self.builder.create_block();
        let sub_body = self.builder.create_block();
        let sub_inc = self.builder.create_block();
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: sub_check }));

        self.builder.switch_to_block(sub_check);
        let sub_cond = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: sub_cond,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Lt,
                left: MirOperand::Local(sub_idx),
                right: MirOperand::Local(sub_len),
            },
        }));
        // Falls through to the outer loop's own increment once every element
        // of this call's sub-vec has been pushed.
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(sub_cond),
            then_block: sub_body,
            else_block: setup.inc_block,
        }));

        self.builder.switch_to_block(sub_body);
        let sub_elem = self.builder.alloc_temp(sub_elem_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(sub_elem),
            func: FunctionRef::internal("Vec_get".to_string()),
            args: vec![MirOperand::Local(sub_vec), MirOperand::Local(sub_idx)],
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::internal("Vec_push".to_string()),
            args: vec![MirOperand::Local(result_vec), MirOperand::Local(sub_elem)],
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: sub_inc }));

        self.builder.switch_to_block(sub_inc);
        let sub_next = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: sub_next,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Add,
                left: MirOperand::Local(sub_idx),
                right: MirOperand::Constant(MirConst::Int(1)),
            },
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: sub_idx,
            rvalue: MirRValue::Use(MirOperand::Local(sub_next)),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: sub_check }));

        self.emit_iter_increment(setup.idx, setup.inc_block, setup.check_block);
        self.builder.switch_to_block(setup.exit_block);
        self.collected_elem_types.insert(result_vec, sub_elem_ty);
        Ok((MirOperand::Local(result_vec), MirType::I64))
    }

    /// Every return-position expression reachable from a closure body — used
    /// to read off whichever one the checker resolved a concrete type for.
    /// Doesn't walk into loops or nested closures: a `flat_map` transform is a
    /// straight-line body (an expression, or a couple of branches), not
    /// control flow deep enough to need more.
    fn closure_return_exprs(closure: &Expr) -> Vec<&Expr> {
        let ExprKind::Closure { body, .. } = &closure.kind else { return Vec::new() };
        let mut out = Vec::new();
        Self::collect_returns(body, &mut out);
        Self::collect_tail_values(body, &mut out);
        out
    }

    /// Every explicit `return e`, however deeply nested in branches.
    fn collect_returns<'e>(expr: &'e Expr, out: &mut Vec<&'e Expr>) {
        match &expr.kind {
            ExprKind::Block(stmts) => {
                for s in stmts {
                    match &s.kind {
                        rask_ast::stmt::StmtKind::Return(Some(e)) => out.push(e),
                        rask_ast::stmt::StmtKind::Expr(e) => Self::collect_returns(e, out),
                        _ => {}
                    }
                }
            }
            ExprKind::If { then_branch, else_branch, .. } => {
                Self::collect_returns(then_branch, out);
                if let Some(e) = else_branch {
                    Self::collect_returns(e, out);
                }
            }
            ExprKind::Match { arms, .. } => {
                for arm in arms {
                    Self::collect_returns(&arm.body, out);
                }
            }
            _ => {}
        }
    }

    /// The expression(s) a body evaluates *to* without saying `return`.
    ///
    /// A closure body doesn't have to be a block: `|x| pair_of(x)` is stored as
    /// the bare call, and `|x| { …; r }` ends in a trailing expression. Looking
    /// only inside `Block`/`If`/`Match` for `return` found neither, so
    /// `flat_map`'s element type had nothing to resolve from and lowering gave
    /// up on a form the interpreter runs fine.
    ///
    /// Only the *last* statement of a block is a value — an earlier expression
    /// statement is evaluated and discarded, so offering it as a candidate
    /// would let an unrelated Vec answer for the element type.
    fn collect_tail_values<'e>(expr: &'e Expr, out: &mut Vec<&'e Expr>) {
        match &expr.kind {
            ExprKind::Block(stmts) => {
                if let Some(rask_ast::stmt::StmtKind::Expr(e)) = stmts.last().map(|s| &s.kind) {
                    Self::collect_tail_values(e, out);
                }
            }
            ExprKind::If { then_branch, else_branch, .. } => {
                Self::collect_tail_values(then_branch, out);
                if let Some(e) = else_branch {
                    Self::collect_tail_values(e, out);
                }
            }
            ExprKind::Match { arms, .. } => {
                for arm in arms {
                    Self::collect_tail_values(&arm.body, out);
                }
            }
            _ => out.push(expr),
        }
    }

    /// `key_j < min_key`, choosing how by the key's MIR type.
    ///
    /// A raw `BinOp::Lt` already does the right thing for numbers (Cranelift
    /// icmp/fcmp) and for structs/enums/tuples/arrays (codegen's own
    /// field-by-field ordering, `is_structural_ord_type`) — but `String` isn't
    /// in that structural set, so a bare `Lt` on two strings compares their
    /// 16-byte representation as if it were a number, the same bug
    /// `rask_vec_sort` needed `rask_vec_sort_str` to avoid. `string_lt` is the
    /// runtime's own lexicographic compare, so route there instead.
    fn emit_key_less(&mut self, key_ty: &MirType, left: LocalId, right: LocalId) -> LocalId {
        let less = self.builder.alloc_temp(MirType::Bool);
        if matches!(key_ty, MirType::String) {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(less),
                func: FunctionRef::internal("string_lt".to_string()),
                args: vec![MirOperand::Local(left), MirOperand::Local(right)],
            }));
        } else {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: less,
                rvalue: MirRValue::BinaryOp {
                    op: crate::operand::BinOp::Lt,
                    left: MirOperand::Local(left),
                    right: MirOperand::Local(right),
                },
            }));
        }
        less
    }

    /// `v.sort_by_key(f)` — in place. Not stable: the spec allows `sort` and
    /// `sort_by_key` to use the platform sort since two elements with equal
    /// keys but different other fields are indistinguishable from the
    /// guarantee's point of view (only `sort_by` promises stability).
    ///
    /// Extracts each element's key once into a parallel `keys` Vec, then
    /// selection-sorts on `keys`, swapping the same pair in both `vec` and
    /// `keys` whenever a new minimum is found — so the closure runs exactly
    /// once per element and the sort itself only ever compares already-
    /// computed keys.
    pub(super) fn lower_iter_sort_by_key(
        &mut self,
        object: &Expr,
        closure: &Expr,
    ) -> Result<TypedOperand, LoweringError> {
        let (obj_op, obj_ty) = self.lower_expr(object)?;
        let vec_local = self.builder.alloc_temp(obj_ty);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: vec_local,
            rvalue: MirRValue::Use(obj_op),
        }));
        let elem_ty = self.collection_elem_of_expr(object)
            .unwrap_or_else(|| crate::fallback::i64_fallback("lower/iterators:sort_by_key_elem"));

        let n = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(n),
            func: FunctionRef::internal("Vec_len".to_string()),
            args: vec![MirOperand::Local(vec_local)],
        }));

        // keys[i] = f(vec[i]), extracted once up front.
        let keys = self.builder.alloc_temp(MirType::I64);
        let keys_new_pos = self.builder.next_stmt_pos();
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(keys),
            func: FunctionRef::internal("Vec_new".to_string()),
            args: vec![],
        }));

        let ex_idx = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: ex_idx,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
        }));
        let ex_check = self.builder.create_block();
        let ex_body = self.builder.create_block();
        let ex_inc = self.builder.create_block();
        let ex_done = self.builder.create_block();
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: ex_check }));

        self.builder.switch_to_block(ex_check);
        let ex_cond = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: ex_cond,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Lt,
                left: MirOperand::Local(ex_idx),
                right: MirOperand::Local(n),
            },
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(ex_cond),
            then_block: ex_body,
            else_block: ex_done,
        }));

        self.builder.switch_to_block(ex_body);
        let elem = self.builder.alloc_temp(elem_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(elem),
            func: FunctionRef::internal("Vec_get".to_string()),
            args: vec![MirOperand::Local(vec_local), MirOperand::Local(ex_idx)],
        }));
        let (key_op, key_ty) = self.inline_closure_body(closure, MirOperand::Local(elem), elem_ty)?;
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::internal("Vec_push".to_string()),
            args: vec![MirOperand::Local(keys), key_op],
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: ex_inc }));

        self.builder.switch_to_block(ex_inc);
        let ex_next = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: ex_next,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Add,
                left: MirOperand::Local(ex_idx),
                right: MirOperand::Constant(MirConst::Int(1)),
            },
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: ex_idx,
            rvalue: MirRValue::Use(MirOperand::Local(ex_next)),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: ex_check }));

        self.builder.switch_to_block(ex_done);
        self.builder.set_call_args(
            keys_new_pos.0,
            keys_new_pos.1,
            "Vec_new",
            vec![MirOperand::Constant(MirConst::Int(Self::mir_slot_size(&key_ty)))],
        );

        // Insertion sort over [1, n), keeping `vec` and `keys` in lockstep.
        //
        // Stable, which is what `sort_by_key` has to be: the interpreter sorts
        // with Rust's stable sort, so anything else here makes the two backends
        // disagree on tied keys. A selection sort doesn't qualify — lifting the
        // minimum out of the tail reorders the equal elements it jumps over.
        // Insertion sort only ever swaps a pair the comparison calls *strictly*
        // less, so equal keys never cross.
        //
        // O(n²) in the worst case, where `sort`/`sort_by` hand off to the
        // runtime's qsort/merge sort. Those take the comparison as a C function
        // pointer or a closure, and this comparison is neither — it's emitted
        // MIR, which is what lets an arbitrary key type (a struct, an enum, a
        // tuple) be compared field-by-field by codegen at all. Reusing the
        // runtime's sort means giving it a callable comparator over keys; #942
        // has the plan.
        let i = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: i,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(1))),
        }));

        let i_check = self.builder.create_block();
        let i_body = self.builder.create_block();
        let i_inc = self.builder.create_block();
        let i_done = self.builder.create_block();
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: i_check }));

        self.builder.switch_to_block(i_check);
        let i_cond = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: i_cond,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Lt,
                left: MirOperand::Local(i),
                right: MirOperand::Local(n),
            },
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(i_cond),
            then_block: i_body,
            else_block: i_done,
        }));

        // Walk element `i` down past every predecessor with a bigger key.
        let j = self.builder.alloc_temp(MirType::I64);
        self.builder.switch_to_block(i_body);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: j,
            rvalue: MirRValue::Use(MirOperand::Local(i)),
        }));

        let j_check = self.builder.create_block();
        let j_body = self.builder.create_block();
        let j_shift = self.builder.create_block();
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: j_check }));

        self.builder.switch_to_block(j_check);
        let j_positive = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: j_positive,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Gt,
                left: MirOperand::Local(j),
                right: MirOperand::Constant(MirConst::Int(0)),
            },
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(j_positive),
            then_block: j_body,
            else_block: i_inc,
        }));

        self.builder.switch_to_block(j_body);
        let j_prev = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: j_prev,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Sub,
                left: MirOperand::Local(j),
                right: MirOperand::Constant(MirConst::Int(1)),
            },
        }));
        let key_j = self.builder.alloc_temp(key_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(key_j),
            func: FunctionRef::internal("Vec_get".to_string()),
            args: vec![MirOperand::Local(keys), MirOperand::Local(j)],
        }));
        let key_prev = self.builder.alloc_temp(key_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(key_prev),
            func: FunctionRef::internal("Vec_get".to_string()),
            args: vec![MirOperand::Local(keys), MirOperand::Local(j_prev)],
        }));
        let less = self.emit_key_less(&key_ty, key_j, key_prev);
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(less),
            then_block: j_shift,
            else_block: i_inc,
        }));

        self.builder.switch_to_block(j_shift);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::internal("Vec_swap".to_string()),
            args: vec![MirOperand::Local(vec_local), MirOperand::Local(j), MirOperand::Local(j_prev)],
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::internal("Vec_swap".to_string()),
            args: vec![MirOperand::Local(keys), MirOperand::Local(j), MirOperand::Local(j_prev)],
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: j,
            rvalue: MirRValue::Use(MirOperand::Local(j_prev)),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: j_check }));

        self.builder.switch_to_block(i_inc);
        let i_next = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: i_next,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Add,
                left: MirOperand::Local(i),
                right: MirOperand::Constant(MirConst::Int(1)),
            },
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: i,
            rvalue: MirRValue::Use(MirOperand::Local(i_next)),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: i_check }));

        self.builder.switch_to_block(i_done);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::internal("Vec_free".to_string()),
            args: vec![MirOperand::Local(keys)],
        }));

        Ok((MirOperand::Constant(MirConst::Int(0)), MirType::Void))
    }
}
