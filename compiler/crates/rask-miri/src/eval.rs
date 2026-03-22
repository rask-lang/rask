// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! MIR evaluation loop — execute statements, follow terminators.

use rask_mir::{
    BlockId, MirBlock, MirConst, MirFunction, MirOperand, MirRValue, MirStmt, MirStmtKind,
    MirTerminator, MirTerminatorKind,
};

use crate::intrinsics;
use crate::memory::StackFrame;
use crate::{MiriEngine, MiriError, MiriValue};

impl MiriEngine {
    /// Execute a function by name with the given arguments.
    pub fn call_function(
        &mut self,
        func_name: &str,
        args: Vec<MiriValue>,
    ) -> Result<MiriValue, MiriError> {
        let func = self
            .functions
            .get(func_name)
            .cloned()
            .ok_or_else(|| {
                // Not a user-defined function — try stdlib
                MiriError::UnsupportedOperation(format!("unknown function: {func_name}"))
            })?;

        self.eval_function(&func, args)
    }

    /// Execute a MIR function with the given arguments.
    fn eval_function(
        &mut self,
        func: &MirFunction,
        args: Vec<MiriValue>,
    ) -> Result<MiriValue, MiriError> {
        let mut frame = StackFrame::new(func);

        // Initialize parameters
        for (param, arg) in func.params.iter().zip(args) {
            frame.set(param.id, arg);
        }

        self.stack.push(frame)?;
        let result = self.eval_body(func);
        self.stack.pop();
        result
    }

    /// Execute the body of a function (block-by-block loop).
    fn eval_body(&mut self, func: &MirFunction) -> Result<MiriValue, MiriError> {
        let mut current_block_id = func.entry_block;

        loop {
            let block = self.find_block(func, current_block_id)?;

            // Execute all statements
            for stmt in &block.statements {
                self.eval_stmt(stmt)?;
            }

            // Follow the terminator
            match &block.terminator.kind {
                MirTerminatorKind::Return { value } => {
                    let result = match value {
                        Some(op) => self.resolve_operand(op)?,
                        None => MiriValue::Unit,
                    };
                    return Ok(result);
                }

                MirTerminatorKind::Goto { target } => {
                    self.count_branch_if_backwards(*target, current_block_id)?;
                    current_block_id = *target;
                }

                MirTerminatorKind::Branch {
                    cond,
                    then_block,
                    else_block,
                } => {
                    let cond_val = self.resolve_operand(cond)?;
                    let target = if cond_val.as_bool()? {
                        *then_block
                    } else {
                        *else_block
                    };
                    self.count_branch_if_backwards(target, current_block_id)?;
                    current_block_id = target;
                }

                MirTerminatorKind::Switch {
                    value,
                    cases,
                    default,
                } => {
                    let val = self.resolve_operand(value)?;
                    let tag = val.to_u64().ok_or_else(|| {
                        MiriError::UnsupportedOperation("switch on non-integer value".to_string())
                    })?;
                    let target = cases
                        .iter()
                        .find(|(case_val, _)| *case_val == tag)
                        .map(|(_, block)| *block)
                        .unwrap_or(*default);
                    self.count_branch_if_backwards(target, current_block_id)?;
                    current_block_id = target;
                }

                MirTerminatorKind::Unreachable => {
                    return Err(MiriError::Unreachable);
                }

                MirTerminatorKind::CleanupReturn { value, cleanup_chain } => {
                    // Execute cleanup blocks in order
                    for &cleanup_block_id in cleanup_chain {
                        let cleanup_block = self.find_block(func, cleanup_block_id)?;
                        for stmt in &cleanup_block.statements {
                            self.eval_stmt(stmt)?;
                        }
                    }
                    let result = match value {
                        Some(op) => self.resolve_operand(op)?,
                        None => MiriValue::Unit,
                    };
                    return Ok(result);
                }
            }
        }
    }

    /// Find a block by ID within a function.
    fn find_block<'f>(&self, func: &'f MirFunction, id: BlockId) -> Result<&'f MirBlock, MiriError> {
        func.blocks
            .iter()
            .find(|b| b.id == id)
            .ok_or_else(|| {
                MiriError::UnsupportedOperation(format!("block {:?} not found", id))
            })
    }

    /// Count a backwards branch (for loop detection / quota enforcement).
    fn count_branch_if_backwards(
        &mut self,
        target: BlockId,
        current: BlockId,
    ) -> Result<(), MiriError> {
        if target.0 <= current.0 {
            self.branch_count += 1;
            if self.branch_count > self.branch_limit {
                return Err(MiriError::BranchLimitExceeded(self.branch_limit));
            }
        }
        Ok(())
    }

    /// Execute a single MIR statement.
    fn eval_stmt(&mut self, stmt: &MirStmt) -> Result<(), MiriError> {
        match &stmt.kind {
            MirStmtKind::Assign { dst, rvalue } => {
                let value = self.eval_rvalue(rvalue)?;
                self.stack.current_mut()?.set(*dst, value);
            }

            MirStmtKind::Call { dst, func, args } => {
                let arg_values: Vec<MiriValue> = args
                    .iter()
                    .map(|a| self.resolve_operand(a))
                    .collect::<Result<_, _>>()?;

                let result = if let Some(mir_func) = self.functions.get(&func.name).cloned() {
                    self.eval_function(&mir_func, arg_values)?
                } else {
                    // Try stdlib
                    match self.stdlib.call(&func.name, &arg_values)? {
                        Some(v) => v,
                        None => MiriValue::Unit,
                    }
                };

                if let Some(dst) = dst {
                    self.stack.current_mut()?.set(*dst, result);
                }
            }

            MirStmtKind::Store {
                addr,
                offset,
                value,
                ..
            } => {
                let val = self.resolve_operand(value)?;
                let base = self.stack.current()?.get(*addr)?.clone();

                // Store into struct field by byte offset
                if let MiriValue::Struct { layout_id, mut fields } = base {
                    // Find field index from byte offset using layout
                    let field_idx = self.field_index_from_offset(layout_id, *offset);
                    if let Some(idx) = field_idx {
                        if idx < fields.len() {
                            fields[idx] = val;
                            self.stack.current_mut()?.set(
                                *addr,
                                MiriValue::Struct { layout_id, fields },
                            );
                            return Ok(());
                        }
                    }
                    // Fallback: store back unchanged (offset didn't match)
                    self.stack.current_mut()?.set(
                        *addr,
                        MiriValue::Struct { layout_id, fields },
                    );
                } else {
                    return Err(MiriError::UnsupportedOperation(
                        format!("Store into non-struct value: {base:?}"),
                    ));
                }
            }

            MirStmtKind::ArrayStore {
                base,
                index,
                value,
                ..
            } => {
                let idx_val = self.resolve_operand(index)?;
                let idx = idx_val.to_u64().ok_or_else(|| {
                    MiriError::UnsupportedOperation("array index must be integer".to_string())
                })? as usize;
                let val = self.resolve_operand(value)?;

                let mut arr = self.stack.current()?.get(*base)?.clone();
                if let MiriValue::Array(ref mut elems) = arr {
                    if idx >= elems.len() {
                        return Err(MiriError::UnsupportedOperation(
                            format!("array index {idx} out of bounds (len {})", elems.len()),
                        ));
                    }
                    elems[idx] = val;
                    self.stack.current_mut()?.set(*base, arr);
                } else {
                    return Err(MiriError::UnsupportedOperation(
                        "ArrayStore on non-array value".to_string(),
                    ));
                }
            }

            MirStmtKind::GlobalRef { dst, name } => {
                // Look up comptime globals
                if let Some(value) = self.comptime_globals.get(name).cloned() {
                    self.stack.current_mut()?.set(*dst, value);
                } else {
                    return Err(MiriError::UnsupportedOperation(
                        format!("global '{name}' not found at compile time"),
                    ));
                }
            }

            MirStmtKind::EnsurePush { cleanup_block } => {
                self.stack.current_mut()?.cleanup_stack.push(*cleanup_block);
            }

            MirStmtKind::EnsurePop => {
                self.stack.current_mut()?.cleanup_stack.pop();
            }

            // Forbidden at comptime
            MirStmtKind::ResourceRegister { .. }
            | MirStmtKind::ResourceConsume { .. }
            | MirStmtKind::ResourceScopeCheck { .. } => {
                return Err(MiriError::UnsupportedOperation(
                    "resource types are not available at compile time".to_string(),
                ));
            }

            MirStmtKind::PoolCheckedAccess { .. } => {
                return Err(MiriError::UnsupportedOperation(
                    "pool access is not available at compile time".to_string(),
                ));
            }

            // Closures — not in initial scope
            MirStmtKind::ClosureCreate { .. }
            | MirStmtKind::ClosureCall { .. }
            | MirStmtKind::LoadCapture { .. }
            | MirStmtKind::ClosureDrop { .. } => {
                return Err(MiriError::UnsupportedOperation(
                    "closures are not yet supported in compile-time evaluation".to_string(),
                ));
            }

            // Trait objects — not in initial scope
            MirStmtKind::TraitBox { .. }
            | MirStmtKind::TraitCall { .. }
            | MirStmtKind::TraitDrop { .. } => {
                return Err(MiriError::UnsupportedOperation(
                    "trait objects are not yet supported in compile-time evaluation".to_string(),
                ));
            }

            MirStmtKind::Phi { .. } => {
                panic!("Phi nodes must be lowered by de-SSA before interpretation");
            }

            // RC ops are no-ops at comptime — strings are GC'd by the interpreter.
            MirStmtKind::RcInc { .. } | MirStmtKind::RcDec { .. } => {}
        }
        Ok(())
    }

    /// Evaluate an rvalue.
    fn eval_rvalue(&mut self, rvalue: &MirRValue) -> Result<MiriValue, MiriError> {
        match rvalue {
            MirRValue::Use(op) => self.resolve_operand(op),

            MirRValue::BinaryOp { op, left, right } => {
                let l = self.resolve_operand(left)?;
                let r = self.resolve_operand(right)?;
                intrinsics::eval_binop(*op, &l, &r)
            }

            MirRValue::UnaryOp { op, operand } => {
                let v = self.resolve_operand(operand)?;
                intrinsics::eval_unaryop(*op, &v)
            }

            MirRValue::Cast { value, target_ty } => {
                let v = self.resolve_operand(value)?;
                intrinsics::eval_cast(&v, target_ty)
            }

            MirRValue::Field {
                base, field_index, ..
            } => {
                let base_val = self.resolve_operand(base)?;
                match base_val {
                    MiriValue::Struct { fields, .. } => {
                        let idx = *field_index as usize;
                        fields.into_iter().nth(idx).ok_or_else(|| {
                            MiriError::UnsupportedOperation(
                                format!("field index {idx} out of bounds"),
                            )
                        })
                    }
                    MiriValue::Tuple(fields) => {
                        let idx = *field_index as usize;
                        fields.into_iter().nth(idx).ok_or_else(|| {
                            MiriError::UnsupportedOperation(
                                format!("tuple field index {idx} out of bounds"),
                            )
                        })
                    }
                    // Option/Result — tag is field 0, payload is field 1
                    MiriValue::Enum { tag, payload, .. } => {
                        match *field_index {
                            0 => Ok(MiriValue::U64(tag)),
                            1 => payload.map(|p| *p).ok_or_else(|| {
                                MiriError::UnsupportedOperation(
                                    "enum has no payload".to_string(),
                                )
                            }),
                            _ => Err(MiriError::UnsupportedOperation(
                                format!("invalid enum field index: {field_index}"),
                            )),
                        }
                    }
                    _ => Err(MiriError::UnsupportedOperation(
                        format!("field access on non-struct value: {base_val:?}"),
                    )),
                }
            }

            MirRValue::EnumTag { value } => {
                let v = self.resolve_operand(value)?;
                match v {
                    MiriValue::Enum { tag, .. } => Ok(MiriValue::U64(tag)),
                    _ => Err(MiriError::UnsupportedOperation(
                        format!("EnumTag on non-enum value: {v:?}"),
                    )),
                }
            }

            MirRValue::ArrayIndex {
                base, index, ..
            } => {
                let base_val = self.resolve_operand(base)?;
                let idx_val = self.resolve_operand(index)?;
                let idx = idx_val.to_u64().ok_or_else(|| {
                    MiriError::UnsupportedOperation("array index must be integer".to_string())
                })? as usize;

                match base_val {
                    MiriValue::Array(elems) => {
                        elems.into_iter().nth(idx).ok_or_else(|| {
                            MiriError::UnsupportedOperation(
                                format!("array index {idx} out of bounds"),
                            )
                        })
                    }
                    _ => Err(MiriError::UnsupportedOperation(
                        format!("ArrayIndex on non-array value: {base_val:?}"),
                    )),
                }
            }

            MirRValue::Ref(_) => {
                Err(MiriError::UnsupportedOperation(
                    "references are not supported in compile-time evaluation".to_string(),
                ))
            }

            MirRValue::Deref(_) => {
                Err(MiriError::UnsupportedOperation(
                    "dereferencing is not supported in compile-time evaluation".to_string(),
                ))
            }
        }
    }

    /// Resolve an operand to a value.
    pub(crate) fn resolve_operand(&self, op: &MirOperand) -> Result<MiriValue, MiriError> {
        match op {
            MirOperand::Local(id) => self.stack.current()?.get(*id).cloned(),
            MirOperand::Constant(c) => Ok(const_to_value(c)),
        }
    }

    /// Find field index from byte offset using struct layout.
    fn field_index_from_offset(
        &self,
        layout_id: rask_mir::StructLayoutId,
        offset: u32,
    ) -> Option<usize> {
        let layout = self.struct_layouts.get(layout_id.id as usize)?;
        layout
            .fields
            .iter()
            .position(|f| f.offset == offset)
    }
}

/// Convert a MIR constant to a MiriValue.
fn const_to_value(c: &MirConst) -> MiriValue {
    match c {
        MirConst::Int(v) => MiriValue::I64(*v),
        MirConst::Float(v) => MiriValue::F64(*v),
        MirConst::Bool(v) => MiriValue::Bool(*v),
        MirConst::Char(v) => MiriValue::Char(*v),
        MirConst::String(v) => MiriValue::String(v.clone()),
    }
}
