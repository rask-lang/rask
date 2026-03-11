// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Iterator chain recognition and fused loop lowering.
//!
//! Recognizes patterns like `vec.iter().filter(|x| p(x)).map(|x| f(x)).collect()`
//! and fuses them into a single index-based loop at MIR level.

use super::{LoweringError, MirLowerer, TypedOperand};
use crate::{
    operand::MirConst, BlockId, FunctionRef, LocalId, MirOperand, MirRValue, MirStmt,
    MirTerminator, MirType,
};
use rask_ast::expr::{Expr, ExprKind};

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
                        _ => return None,
                    }
                }
                _ => return None,
            }
        }
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
            "collect" if args.is_empty() => {
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
                self.builder.push_stmt(MirStmt::Assign {
                    dst: param_local,
                    rvalue: MirRValue::Use(arg_op),
                });
                self.locals.insert(param_name.clone(), (param_local, arg_ty));
                let result = self.lower_expr(body)?;
                self.locals.remove(param_name);
                if let Some(prev) = saved {
                    self.locals.insert(param_name.clone(), prev);
                }
                return Ok(result);
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
        self.builder.push_stmt(MirStmt::Assign {
            dst: collection,
            rvalue: MirRValue::Use(source_op),
        });

        // len
        let len_local = self.builder.alloc_temp(MirType::I64);
        if let Some(arr_len) = array_len {
            self.builder.push_stmt(MirStmt::Assign {
                dst: len_local,
                rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(arr_len as i64))),
            });
        } else {
            self.builder.push_stmt(MirStmt::Call {
                dst: Some(len_local),
                func: FunctionRef::internal("Vec_len".to_string()),
                args: vec![MirOperand::Local(collection)],
            });
        }

        // Process Skip/Take adapters to adjust start/end bounds
        let mut start_val: Option<MirOperand> = None;
        let mut end_op = MirOperand::Local(len_local);

        for adapter in &chain.adapters {
            match adapter {
                super::IterAdapter::Skip { count } => {
                    let (skip_op, _) = self.lower_expr(count)?;
                    start_val = Some(skip_op);
                }
                super::IterAdapter::Take { count } => {
                    let (take_op, _) = self.lower_expr(count)?;
                    if let Some(ref start) = start_val {
                        let adjusted = self.builder.alloc_temp(MirType::I64);
                        self.builder.push_stmt(MirStmt::Assign {
                            dst: adjusted,
                            rvalue: MirRValue::BinaryOp {
                                op: crate::operand::BinOp::Add,
                                left: start.clone(),
                                right: take_op,
                            },
                        });
                        end_op = MirOperand::Local(adjusted);
                    } else {
                        end_op = take_op;
                    }
                }
                _ => {} // Filter/Map/Enumerate handled inside the loop body
            }
        }

        let idx = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::Assign {
            dst: idx,
            rvalue: MirRValue::Use(start_val.unwrap_or(MirOperand::Constant(MirConst::Int(0)))),
        });

        let check_block = self.builder.create_block();
        let body_block = self.builder.create_block();
        let inc_block = self.builder.create_block();
        let exit_block = self.builder.create_block();

        self.builder.terminate(MirTerminator::Goto { target: check_block });

        // check: idx < end
        self.builder.switch_to_block(check_block);
        let cond = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::Assign {
            dst: cond,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Lt,
                left: MirOperand::Local(idx),
                right: end_op,
            },
        });
        self.builder.terminate(MirTerminator::Branch {
            cond: MirOperand::Local(cond),
            then_block: body_block,
            else_block: exit_block,
        });

        // body: load element
        self.builder.switch_to_block(body_block);
        let elem_ty = self.extract_iterator_elem_type(chain.source)
            .unwrap_or(MirType::I64);
        let elem_local = self.builder.alloc_temp(elem_ty.clone());
        if is_array {
            self.builder.push_stmt(MirStmt::Assign {
                dst: elem_local,
                rvalue: MirRValue::ArrayIndex {
                    base: MirOperand::Local(collection),
                    index: MirOperand::Local(idx),
                    elem_size: array_elem_size.unwrap_or(8),
                },
            });
        } else {
            self.builder.push_stmt(MirStmt::Call {
                dst: Some(elem_local),
                func: FunctionRef::internal("Vec_get".to_string()),
                args: vec![MirOperand::Local(collection), MirOperand::Local(idx)],
            });
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
                    self.builder.terminate(MirTerminator::Branch {
                        cond: pred_op,
                        then_block: pass_block,
                        else_block: inc_block,
                    });
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
                    self.builder.push_stmt(MirStmt::Store {
                        addr: tuple_local,
                        offset: 0,
                        value: MirOperand::Local(idx),
                        store_size: None,
                    });
                    self.builder.push_stmt(MirStmt::Store {
                        addr: tuple_local,
                        offset: 8,
                        value: current_op,
                        store_size: None,
                    });
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
        self.builder.push_stmt(MirStmt::Assign {
            dst: incremented,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Add,
                left: MirOperand::Local(idx),
                right: MirOperand::Constant(MirConst::Int(1)),
            },
        });
        self.builder.push_stmt(MirStmt::Assign {
            dst: idx,
            rvalue: MirRValue::Use(MirOperand::Local(incremented)),
        });
        self.builder.terminate(MirTerminator::Goto { target: check_block });
    }

    /// .collect() — fused loop that pushes each result into a new Vec.
    pub(super) fn lower_iter_collect(
        &mut self,
        chain: &super::IterChain<'_>,
    ) -> Result<TypedOperand, LoweringError> {
        let result_vec = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::Call {
            dst: Some(result_vec),
            func: FunctionRef::internal("Vec_new".to_string()),
            args: vec![],
        });

        let setup = self.setup_iter_chain_loop(chain)?;
        let (final_op, _) = self.apply_iter_adapters(
            chain, MirOperand::Local(setup.elem_local), setup.elem_ty,
            setup.inc_block, setup.idx,
        )?;

        self.builder.push_stmt(MirStmt::Call {
            dst: None,
            func: FunctionRef::internal("Vec_push".to_string()),
            args: vec![MirOperand::Local(result_vec), final_op],
        });
        self.builder.terminate(MirTerminator::Goto { target: setup.inc_block });

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
        self.builder.push_stmt(MirStmt::Assign {
            dst: acc,
            rvalue: MirRValue::Use(init_op),
        });

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
                self.builder.push_stmt(MirStmt::Assign {
                    dst: acc_param,
                    rvalue: MirRValue::Use(MirOperand::Local(acc)),
                });
                self.locals.insert(acc_name.clone(), (acc_param, init_ty.clone()));

                let elem_param = self.builder.alloc_local(elem_name.clone(), final_ty.clone());
                self.builder.push_stmt(MirStmt::Assign {
                    dst: elem_param,
                    rvalue: MirRValue::Use(final_op),
                });
                self.locals.insert(elem_name.clone(), (elem_param, final_ty));

                let after_body = self.builder.create_block();

                let saved_return_target = self.inline_return_target.take();
                self.inline_return_target = Some((acc, setup.inc_block));

                let (result_op, _) = self.lower_expr(body)?;

                self.inline_return_target = saved_return_target;

                self.builder.push_stmt(MirStmt::Assign {
                    dst: acc,
                    rvalue: MirRValue::Use(result_op),
                });
                self.builder.terminate(MirTerminator::Goto { target: after_body });

                self.builder.switch_to_block(after_body);

                self.locals.remove(acc_name);
                self.locals.remove(elem_name);
                if let Some(prev) = saved_acc { self.locals.insert(acc_name.clone(), prev); }
                if let Some(prev) = saved_elem { self.locals.insert(elem_name.clone(), prev); }
            }
        }

        self.builder.terminate(MirTerminator::Goto { target: setup.inc_block });
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
        self.builder.push_stmt(MirStmt::Assign {
            dst: result,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Bool(false))),
        });

        let setup = self.setup_iter_chain_loop(chain)?;
        let (final_op, final_ty) = self.apply_iter_adapters(
            chain, MirOperand::Local(setup.elem_local), setup.elem_ty,
            setup.inc_block, setup.idx,
        )?;

        let (pred_op, _) = self.inline_closure_body(predicate, final_op, final_ty)?;
        let found_block = self.builder.create_block();
        self.builder.terminate(MirTerminator::Branch {
            cond: pred_op,
            then_block: found_block,
            else_block: setup.inc_block,
        });

        self.builder.switch_to_block(found_block);
        self.builder.push_stmt(MirStmt::Assign {
            dst: result,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Bool(true))),
        });
        self.builder.terminate(MirTerminator::Goto { target: setup.exit_block });

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
        self.builder.push_stmt(MirStmt::Assign {
            dst: result,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Bool(true))),
        });

        let setup = self.setup_iter_chain_loop(chain)?;
        let (final_op, final_ty) = self.apply_iter_adapters(
            chain, MirOperand::Local(setup.elem_local), setup.elem_ty,
            setup.inc_block, setup.idx,
        )?;

        let (pred_op, _) = self.inline_closure_body(predicate, final_op, final_ty)?;
        let fail_block = self.builder.create_block();
        self.builder.terminate(MirTerminator::Branch {
            cond: pred_op,
            then_block: setup.inc_block,
            else_block: fail_block,
        });

        self.builder.switch_to_block(fail_block);
        self.builder.push_stmt(MirStmt::Assign {
            dst: result,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Bool(false))),
        });
        self.builder.terminate(MirTerminator::Goto { target: setup.exit_block });

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
        self.builder.push_stmt(MirStmt::Assign {
            dst: counter,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
        });

        let setup = self.setup_iter_chain_loop(chain)?;
        let _ = self.apply_iter_adapters(
            chain, MirOperand::Local(setup.elem_local), setup.elem_ty,
            setup.inc_block, setup.idx,
        )?;

        let incremented = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::Assign {
            dst: incremented,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Add,
                left: MirOperand::Local(counter),
                right: MirOperand::Constant(MirConst::Int(1)),
            },
        });
        self.builder.push_stmt(MirStmt::Assign {
            dst: counter,
            rvalue: MirRValue::Use(MirOperand::Local(incremented)),
        });

        self.builder.terminate(MirTerminator::Goto { target: setup.inc_block });
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
        self.builder.push_stmt(MirStmt::Assign {
            dst: acc,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
        });

        let setup = self.setup_iter_chain_loop(chain)?;
        let (final_op, _) = self.apply_iter_adapters(
            chain, MirOperand::Local(setup.elem_local), setup.elem_ty,
            setup.inc_block, setup.idx,
        )?;

        let sum = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::Assign {
            dst: sum,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Add,
                left: MirOperand::Local(acc),
                right: final_op,
            },
        });
        self.builder.push_stmt(MirStmt::Assign {
            dst: acc,
            rvalue: MirRValue::Use(MirOperand::Local(sum)),
        });

        self.builder.terminate(MirTerminator::Goto { target: setup.inc_block });
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
        self.builder.push_stmt(MirStmt::Store {
            addr: result,
            offset: 0,
            value: MirOperand::Constant(MirConst::Int(1)),
            store_size: None,
        });

        let setup = self.setup_iter_chain_loop(chain)?;
        let (final_op, final_ty) = self.apply_iter_adapters(
            chain, MirOperand::Local(setup.elem_local), setup.elem_ty,
            setup.inc_block, setup.idx,
        )?;

        let (pred_op, _) = self.inline_closure_body(predicate, final_op.clone(), final_ty)?;
        let found_block = self.builder.create_block();
        self.builder.terminate(MirTerminator::Branch {
            cond: pred_op,
            then_block: found_block,
            else_block: setup.inc_block,
        });

        self.builder.switch_to_block(found_block);
        self.builder.push_stmt(MirStmt::Store {
            addr: result,
            offset: 0,
            value: MirOperand::Constant(MirConst::Int(0)), // Some tag
            store_size: None,
        });
        self.builder.push_stmt(MirStmt::Store {
            addr: result,
            offset: 8,
            value: final_op,
            store_size: None,
        });
        self.builder.terminate(MirTerminator::Goto { target: setup.exit_block });

        self.emit_iter_increment(setup.idx, setup.inc_block, setup.check_block);
        self.builder.switch_to_block(setup.exit_block);
        Ok((MirOperand::Local(result), MirType::Option(Box::new(MirType::I64))))
    }
}
