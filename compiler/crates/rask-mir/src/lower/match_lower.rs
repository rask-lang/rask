// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Match expression lowering: enum/tagged dispatch, string match, tuple match.

use crate::FieldAccess;
use super::{is_variant_name, LoweringError, MirLowerer, TypedOperand};
use crate::{
    operand::MirConst, BlockId, FunctionRef, MirOperand, MirRValue, MirStmt,
    MirStmtKind, MirTerminator, MirTerminatorKind, MirType,
};
use rask_ast::expr::{Expr, ExprKind};

/// Walk a pattern to see if it contains a range pattern anywhere.
fn contains_range_pattern(pattern: &rask_ast::expr::Pattern) -> bool {
    use rask_ast::expr::Pattern;
    match pattern {
        Pattern::Range { .. } => true,
        Pattern::Or(pats) => pats.iter().any(contains_range_pattern),
        _ => false,
    }
}

/// Flatten an Or pattern into its alternatives. Non-Or patterns return themselves.
fn flatten_pattern_alternatives(pattern: &rask_ast::expr::Pattern) -> Vec<&rask_ast::expr::Pattern> {
    use rask_ast::expr::Pattern;
    match pattern {
        Pattern::Or(pats) => pats.iter().collect(),
        other => vec![other],
    }
}

impl<'a> MirLowerer<'a> {

    /// A value that can fail *and* be absent — `T? or E` — carries an error
    /// tag around an option tag. Nothing else in MIR nests two wrappers.
    fn is_flat_two_layer(ty: &MirType) -> bool {
        match ty {
            MirType::Result { ok, err } => {
                !matches!(**err, MirType::Void)
                    && matches!(**ok, MirType::Option(_))
            }
            _ => false,
        }
    }

    /// `match` on a flat `T? or E`. The three leaves (`T`, `none`, `E`) sit
    /// behind two tags, so this computes one discriminant for them — 0 for the
    /// payload, 1 for absent, 2 for the error — and switches on that. Reading a
    /// single tag would collapse `none` and `T` into the same arm (OPT30).
    fn lower_flat_match(
        &mut self,
        scrutinee: &Expr,
        scrutinee_op: MirOperand,
        scrutinee_ty: MirType,
        arms: &[rask_ast::expr::MatchArm],
    ) -> Result<TypedOperand, LoweringError> {
        use rask_ast::expr::Pattern;

        let (inner_opt_ty, err_ty) = match &scrutinee_ty {
            MirType::Result { ok, err } => ((**ok).clone(), (**err).clone()),
            _ => unreachable!("checked by is_flat_two_layer"),
        };
        let payload_ty = match &inner_opt_ty {
            MirType::Option(inner) => (**inner).clone(),
            _ => MirType::I64,
        };

        // The inner optional, lifted out of the result's payload slot. It's a
        // tagged aggregate living inline, so what's wanted is its address —
        // loading a word here would hand back the tag and the next read would
        // dereference it.
        let inner_local = self.builder.alloc_temp(inner_opt_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: inner_local,
            rvalue: MirRValue::Field {
                base: scrutinee_op.clone(),
                field_index: 0,
                byte_offset: None,
                access: FieldAccess::Word,
            },
        }));

        // leaf = 2 on the error side, otherwise the inner option's own tag.
        let leaf = self.builder.alloc_temp(MirType::U8);
        let outer_tag = self.emit_option_tag(&scrutinee_op, false);
        let err_blk = self.builder.create_block();
        let ok_blk = self.builder.create_block();
        let disc_blk = self.builder.create_block();
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(outer_tag),
            then_block: err_blk,
            else_block: ok_blk,
        }));
        self.builder.switch_to_block(err_blk);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: leaf,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(2))),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: disc_blk }));
        self.builder.switch_to_block(ok_blk);
        let inner_tag = self.emit_option_tag(&MirOperand::Local(inner_local), false);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: leaf,
            rvalue: MirRValue::Use(MirOperand::Local(inner_tag)),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: disc_blk }));
        self.builder.switch_to_block(disc_blk);

        let merge_block = self.builder.create_block();
        let arm_blocks: Vec<BlockId> = arms.iter().map(|_| self.builder.create_block()).collect();
        let mut cases: Vec<(u64, BlockId)> = Vec::new();
        let mut default_block = merge_block;

        // Which leaf each arm names. `none` is the absent one; anything the
        // error side answers to is the error; the rest is the payload.
        let leaf_of = |lowerer: &Self, name: &str| -> u64 {
            if name == "none" {
                1
            } else if lowerer.pattern_is_err_side(name, &scrutinee_ty) {
                2
            } else {
                0
            }
        };

        for (i, arm) in arms.iter().enumerate() {
            let name = match &arm.pattern {
                Pattern::Wildcard => {
                    default_block = arm_blocks[i];
                    continue;
                }
                Pattern::TypePat { ty_name, .. } => ty_name.clone(),
                Pattern::Ident(n) => n.clone(),
                Pattern::Constructor { name, .. } => name.clone(),
                _ => {
                    default_block = arm_blocks[i];
                    continue;
                }
            };
            cases.push((leaf_of(self, &name), arm_blocks[i]));
        }

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Switch {
            value: MirOperand::Local(leaf),
            cases,
            default: default_block,
        }));

        let mut result_ty = MirType::Void;
        let result_local = self.builder.alloc_temp(MirType::I64);
        for (i, arm) in arms.iter().enumerate() {
            self.builder.switch_to_block(arm_blocks[i]);

            if let Pattern::TypePat { ty_name, binding: Some(binding) } = &arm.pattern {
                // The payload comes from the layer the arm named: the inner
                // option for `T`, the outer result for `E`.
                // The payload comes from the layer the arm named. The error
                // reads out of the result the way any `T or E` arm does; the
                // success value reads out of the inner option, whose slot
                // holds a word unless the payload is a real aggregate.
                let (bind_ty, base, byte_offset) = if leaf_of(self, ty_name) == 2 {
                    let off = self.payload_byte_offset(&err_ty);
                    (err_ty.clone(), scrutinee_op.clone(), off)
                } else {
                    let off = if matches!(
                        payload_ty,
                        MirType::Struct(_) | MirType::Enum(_) | MirType::Tuple(_)
                    ) {
                        None
                    } else {
                        Some(crate::types::RESULT_PAYLOAD_OFFSET)
                    };
                    (payload_ty.clone(), MirOperand::Local(inner_local), off)
                };
                let local = self.builder.alloc_local(binding.clone(), bind_ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: local,
                    rvalue: MirRValue::Field {
                        base,
                        field_index: 0,
                        byte_offset,
                        access: FieldAccess::Word,
                    },
                }));
                if let Some(p) = self.mir_type_name(&bind_ty) {
                    self.meta_mut(binding).type_prefix = Some(p);
                }
                self.locals.insert(binding.clone(), (local, bind_ty));
            }

            let (body_val, arm_ty) = self.lower_expr(&arm.body)?;
            if i == 0 {
                result_ty = arm_ty;
            }
            if self.builder.current_block_unterminated() {
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Use(body_val),
                }));
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                    target: merge_block,
                }));
            }
        }

        let _ = scrutinee;
        self.builder.switch_to_block(merge_block);
        Ok((MirOperand::Local(result_local), result_ty))
    }

    /// Match expression lowering (spec L2).
    pub(super) fn lower_match(
        &mut self,
        scrutinee: &Expr,
        arms: &[rask_ast::expr::MatchArm],
    ) -> Result<TypedOperand, LoweringError> {
        use rask_ast::expr::Pattern;

        // Tuple pattern matching
        let has_tuple_patterns = arms.iter().any(|a| matches!(&a.pattern, Pattern::Tuple(_)));
        if has_tuple_patterns {
            return self.lower_tuple_match(scrutinee, arms);
        }

        // Range patterns can't be a switch case — fall back to an if-chain.
        let has_range = arms.iter().any(|a| contains_range_pattern(&a.pattern));
        if has_range {
            let (scrutinee_op, scrutinee_ty) = self.lower_expr(scrutinee)?;
            return self.lower_scalar_chain_match(scrutinee_op, scrutinee_ty, arms);
        }

        let is_niche = self.is_niche_option_expr(scrutinee);
        let (scrutinee_op, scrutinee_ty) = self.lower_expr(scrutinee)?;

        // String match
        let is_string_match = matches!(scrutinee_ty, MirType::String)
            || self.ctx.lookup_raw_type(scrutinee.id)
                .map_or(false, |ty| matches!(ty, rask_types::Type::String));
        if is_string_match {
            return self.lower_string_match(scrutinee_op, arms);
        }

        // A flat `T? or E` wears two tags, and the arms name leaves across
        // both layers. Flatten first, then it's an ordinary switch.
        if Self::is_flat_two_layer(&scrutinee_ty) {
            return self.lower_flat_match(scrutinee, scrutinee_op, scrutinee_ty, arms);
        }

        let is_enum = matches!(scrutinee_ty, MirType::Enum(_));

        let is_result_or_option = if !is_enum {
            self.ctx.lookup_raw_type(scrutinee.id).map_or(false, |ty| {
                matches!(ty, rask_types::Type::Result { .. })
            })
        } else {
            false
        };

        let patterns_imply_enum = if !is_enum && !is_result_or_option {
            arms.iter().any(|arm| match &arm.pattern {
                Pattern::Constructor { name, .. } => is_variant_name(name),
                Pattern::Struct { name, .. } => {
                    self.resolve_pattern_tag(name).is_some()
                }
                Pattern::Ident(name) => {
                    self.resolve_pattern_tag(name).is_some()
                        || matches!(name.as_str(), "Ok" | "Err" | "Some" | "None")
                }
                _ => false,
            })
        } else {
            false
        };
        // An `Ordering` has no boxed form: `compare` hands back the tag itself,
        // and nothing ever allocates one. Reading a tag out of that would
        // dereference the value 0, 1 or 2 as an address (#496).
        let is_ordering_match = arms.iter().any(|arm| {
            pattern_name(&arm.pattern).is_some_and(|n| n.starts_with("Ordering."))
        });
        let has_tag =
            (is_enum || is_result_or_option || patterns_imply_enum || is_niche) && !is_ordering_match;

        let ok_payload_ty = self.extract_payload_type(scrutinee)
            .or_else(|| match &scrutinee_ty {
                MirType::Result { ok, .. } => Some(ok.as_ref().clone()),
                MirType::Option(inner) => Some(inner.as_ref().clone()),
                _ => None,
            })
            .unwrap_or(MirType::I64);
        let err_payload_ty = self.extract_err_type(scrutinee)
            .or_else(|| match &scrutinee_ty {
                MirType::Result { err, .. } => Some(err.as_ref().clone()),
                _ => None,
            })
            .unwrap_or(MirType::I64);

        let switch_val = if has_tag {
            let tag_local = self.emit_option_tag(&scrutinee_op, is_niche);
            MirOperand::Local(tag_local)
        } else {
            scrutinee_op.clone()
        };

        let merge_block = self.builder.create_block();
        let arm_blocks: Vec<BlockId> = arms.iter().map(|_| self.builder.create_block()).collect();

        let mut cases: Vec<(u64, BlockId)> = Vec::new();
        let mut default_block = merge_block;

        for (i, arm) in arms.iter().enumerate() {
            match &arm.pattern {
                Pattern::Wildcard => {
                    default_block = arm_blocks[i];
                }
                Pattern::Ident(name) => {
                    if let Some(tag) = self.resolve_pattern_tag(name) {
                        cases.push((tag, arm_blocks[i]));
                    } else if has_tag && is_result_or_option {
                        // Result match: ok arm = tag 0, err arm = tag 1, decided
                        // by the scrutinee's real ok/err type identities.
                        let tag = self.pattern_is_err_side(name, &scrutinee_ty) as u64;
                        cases.push((tag, arm_blocks[i]));
                    } else if has_tag && is_variant_name(name) {
                        cases.push((self.variant_tag(name) as u64, arm_blocks[i]));
                    } else {
                        default_block = arm_blocks[i];
                    }
                }
                Pattern::Constructor { name, .. } => {
                    if let Some(tag) = self.resolve_pattern_tag(name) {
                        cases.push((tag, arm_blocks[i]));
                    } else if has_tag {
                        cases.push((self.variant_tag(name) as u64, arm_blocks[i]));
                    } else {
                        cases.push((i as u64, arm_blocks[i]));
                    }
                }
                Pattern::Literal(lit_expr) => {
                    if let ExprKind::Int(v, _) = &lit_expr.kind {
                        cases.push((*v as u64, arm_blocks[i]));
                    } else if let ExprKind::Bool(b) = &lit_expr.kind {
                        cases.push((if *b { 1 } else { 0 }, arm_blocks[i]));
                    } else {
                        cases.push((i as u64, arm_blocks[i]));
                    }
                }
                Pattern::Struct { name, .. } => {
                    if let Some(tag) = self.resolve_pattern_tag(name) {
                        cases.push((tag, arm_blocks[i]));
                    } else {
                        cases.push((i as u64, arm_blocks[i]));
                    }
                }
                Pattern::TypePat { ty_name, .. } => {
                    if is_result_or_option {
                        // Result/Option match: ok arm = tag 0, err arm = tag 1,
                        // decided by the real ok/err type identities.
                        let tag = self.pattern_is_err_side(ty_name, &scrutinee_ty) as u64;
                        cases.push((tag, arm_blocks[i]));
                    } else {
                        cases.push((i as u64, arm_blocks[i]));
                    }
                }
                _ => {
                    cases.push((i as u64, arm_blocks[i]));
                }
            }
        }

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Switch {
            value: switch_val,
            cases,
            default: default_block,
        }));

        let mut result_ty = MirType::Void;
        let result_local = self.builder.alloc_temp(MirType::I64);
        for (i, arm) in arms.iter().enumerate() {
            self.builder.switch_to_block(arm_blocks[i]);

            if has_tag {
                if let Pattern::Constructor { name, fields } = &arm.pattern {
                    // Qualified variant names like "Tagged.With" — strip the
                    // enum prefix so the layout lookup matches the variant's
                    // bare name (same as the Pattern::Struct path below).
                    let variant_name = name.rsplit('.').next().unwrap_or(name);
                    // (mir type, absolute byte offset within the enum, field size)
                    let variant_fields: Option<Vec<(MirType, u32, u32)>> =
                        if let MirType::Enum(crate::types::EnumLayoutId { id: idx, .. }) = &scrutinee_ty {
                            self.ctx.enum_layouts.get(*idx as usize).and_then(|layout| {
                                layout.variants.iter().find(|v| v.name == variant_name).map(|v| {
                                    v.fields.iter().map(|f| {
                                        (self.ctx.type_to_mir(&f.ty), v.payload_offset + f.offset, f.size)
                                    }).collect()
                                })
                            })
                        } else {
                            None
                        };

                    for (j, field_pat) in fields.iter().enumerate() {
                        if let Pattern::Ident(binding) = field_pat {
                            // For a user enum, pass the exact payload offset + size so codegen
                            // doesn't guess the variant (mixed variants share field indices).
                            let (field_ty, field_loc) = if let Some(ref vf) = variant_fields {
                                vf.get(j)
                                    .map(|(ty, off, sz)| (ty.clone(), Some((*off, *sz))))
                                    .unwrap_or((MirType::I64, None))
                            } else {
                                let ty = match name.as_str() {
                                    "Err" => err_payload_ty.clone(),
                                    _ => ok_payload_ty.clone(),
                                };
                                (ty, None)
                            };
                            let payload_local = self.builder.alloc_local(
                                binding.clone(), field_ty.clone(),
                            );
                            let rvalue = if is_niche {
                                MirRValue::Use(scrutinee_op.clone())
                            } else {
                                MirRValue::Field {
                                    base: scrutinee_op.clone(),
                                    field_index: j as u32,
                                    byte_offset: field_loc.map(|(off, _)| off),
                                    access: field_loc.map_or(FieldAccess::Word, |(_, sz)| {
                                        FieldAccess::for_field(&field_ty, sz)
                                    }),
                                }
                            };
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                                dst: payload_local,
                                rvalue,
                            }));
                            let prefix = self.mir_type_name(&field_ty)
                                .or_else(|| {
                                    if let MirType::Enum(crate::types::EnumLayoutId { id: idx, .. }) = &scrutinee_ty {
                                        self.ctx.enum_layouts.get(*idx as usize).and_then(|layout| {
                                            layout.variants.iter().find(|v| v.name == variant_name).and_then(|v| {
                                                v.fields.get(j).and_then(|f| {
                                                    super::MirContext::type_prefix(&f.ty, self.ctx.type_names)
                                                })
                                            })
                                        })
                                    } else {
                                        None
                                    }
                                })
                                .or_else(|| {
                                    let payload_mir = match (&scrutinee_ty, name.as_str()) {
                                        (MirType::Result { err, .. }, "Err") => Some(err.as_ref()),
                                        (MirType::Result { ok, .. }, _) => Some(ok.as_ref()),
                                        (MirType::Option(inner), _) => Some(inner.as_ref()),
                                        _ => None,
                                    };
                                    payload_mir.and_then(|t| self.mir_type_name(t))
                                });
                            if let Some(p) = prefix {
                                self.meta_mut(binding).type_prefix = Some(p);
                            }
                            self.locals.insert(binding.clone(), (payload_local, field_ty));
                        }
                    }
                } else if let Pattern::Struct { name, fields, .. } = &arm.pattern {
                    let variant_name = name.rsplit('.').next().unwrap_or(name);
                    if let MirType::Enum(crate::types::EnumLayoutId { id: idx, .. }) = &scrutinee_ty {
                        if let Some(layout) = self.ctx.enum_layouts.get(*idx as usize) {
                            if let Some(variant) = layout.variants.iter().find(|v| v.name == variant_name) {
                                for (field_name, field_pat) in fields {
                                    if let Pattern::Ident(binding) = field_pat {
                                        if let Some((field_idx, field_layout)) = variant.fields.iter()
                                            .enumerate()
                                            .find(|(_, f)| f.name == *field_name)
                                        {
                                            let field_ty = self.ctx.type_to_mir(&field_layout.ty);
                                            let payload_local = self.builder.alloc_local(
                                                binding.clone(), field_ty.clone(),
                                            );
                                            // Exact offset/size so codegen doesn't guess the variant.
                                            let rvalue = MirRValue::Field {
                                                base: scrutinee_op.clone(),
                                                field_index: field_idx as u32,
                                                byte_offset: Some(variant.payload_offset + field_layout.offset),
                                                access: FieldAccess::for_field(&field_ty, field_layout.size),
                                            };
                                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                                                dst: payload_local,
                                                rvalue,
                                            }));
                                            if let Some(p) = self.mir_type_name(&field_ty)
                                                .or_else(|| super::MirContext::type_prefix(&field_layout.ty, self.ctx.type_names))
                                            {
                                                self.meta_mut(binding).type_prefix = Some(p);
                                            }
                                            self.locals.insert(binding.clone(), (payload_local, field_ty));
                                        }
                                    }
                                }
                            }
                        }
                    }
                // TypePat { ty_name, binding } — `T as name` in a Result/Option match.
                // The switch case routing is already correct (arm index → tag).
                // Here we emit the payload extraction for the binding.
                } else if let Pattern::TypePat { ty_name, binding } = &arm.pattern {
                    if let Some(binding_name) = binding {
                        if is_result_or_option {
                            // Bind the matching side's payload — the side is
                            // decided by type identity, same as the tag routing.
                            let payload_ty = if self.pattern_is_err_side(ty_name, &scrutinee_ty) {
                                err_payload_ty.clone()
                            } else {
                                ok_payload_ty.clone()
                            };
                            let payload_local = self.builder.alloc_local(
                                binding_name.clone(), payload_ty.clone(),
                            );
                            // Scalar payloads: provide byte_offset to bypass the codegen's
                            // "return pointer if either ok or err is aggregate" check, which
                            // would wrongly return a pointer when ok=i32 but err=SomeEnum.
                            // Aggregate payloads: let field_index=0 trigger the pointer return.
                            let is_aggregate_payload = matches!(
                                payload_ty,
                                MirType::Struct(_) | MirType::Enum(_) | MirType::Tuple(_) | MirType::String
                            );
                            let rvalue = MirRValue::Field {
                                base: scrutinee_op.clone(),
                                field_index: 0,
                                byte_offset: if !is_aggregate_payload {
                                    Some(crate::types::RESULT_PAYLOAD_OFFSET)
                                } else {
                                    None
                                },
                                access: FieldAccess::Word,
                            };
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                                dst: payload_local,
                                rvalue,
                            }));
                            if let Some(p) = self.mir_type_name(&payload_ty) {
                                self.meta_mut(binding_name).type_prefix = Some(p);
                            }
                            self.locals.insert(binding_name.clone(), (payload_local, payload_ty));
                        }
                    }
                }
            }

            if let Some(guard_expr) = &arm.guard {
                let (guard_val, _) = self.lower_expr(guard_expr)?;
                let guard_fail_block = if i + 1 < arm_blocks.len() {
                    arm_blocks[i + 1]
                } else {
                    default_block
                };
                let guard_pass_block = self.builder.create_block();
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: guard_val,
                    then_block: guard_pass_block,
                    else_block: guard_fail_block,
                }));
                self.builder.switch_to_block(guard_pass_block);
            }

            let (body_val, arm_ty) = self.lower_expr(&arm.body)?;
            if i == 0 {
                result_ty = arm_ty;
            }

            if self.builder.current_block_unterminated() {
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Use(body_val),
                }));
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                    target: merge_block,
                }));
            }
        }

        self.builder.switch_to_block(merge_block);
        Ok((MirOperand::Local(result_local), result_ty))
    }

    /// Lower match on strings: emit chain of string_eq comparisons.
    pub(super) fn lower_string_match(
        &mut self,
        scrutinee_op: MirOperand,
        arms: &[rask_ast::expr::MatchArm],
    ) -> Result<TypedOperand, LoweringError> {
        use rask_ast::expr::Pattern;

        let merge_block = self.builder.create_block();
        let arm_blocks: Vec<BlockId> = arms.iter().map(|_| self.builder.create_block()).collect();
        let result_local = self.builder.alloc_temp(MirType::I64);
        let mut result_ty = MirType::Void;

        let default_idx = arms.iter().position(|a| {
            matches!(&a.pattern, Pattern::Wildcard)
                || matches!(&a.pattern, Pattern::Ident(n) if !n.starts_with('"'))
        });

        let mut string_arms: Vec<(usize, Vec<String>)> = Vec::new();
        for (i, arm) in arms.iter().enumerate() {
            match &arm.pattern {
                Pattern::Literal(lit) => {
                    if let ExprKind::String(s) = &lit.kind {
                        string_arms.push((i, vec![s.clone()]));
                    }
                }
                Pattern::Or(pats) => {
                    let strs: Vec<String> = pats.iter().filter_map(|p| {
                        if let Pattern::Literal(lit) = p {
                            if let ExprKind::String(s) = &lit.kind {
                                return Some(s.clone());
                            }
                        }
                        None
                    }).collect();
                    if !strs.is_empty() {
                        string_arms.push((i, strs));
                    }
                }
                Pattern::Wildcard | Pattern::Ident(_) => {}
                _ => {}
            }
        }

        let default_block = default_idx.map(|i| arm_blocks[i]).unwrap_or(merge_block);

        for (arm_idx, literals) in &string_arms {
            for (j, lit) in literals.iter().enumerate() {
                let eq_result = self.builder.alloc_temp(MirType::Bool);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: Some(eq_result),
                    func: FunctionRef::internal("string_eq".to_string()),
                    args: vec![
                        scrutinee_op.clone(),
                        MirOperand::Constant(MirConst::String(lit.clone())),
                    ],
                }));
                let next_test = self.builder.create_block();
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: MirOperand::Local(eq_result),
                    then_block: arm_blocks[*arm_idx],
                    else_block: next_test,
                }));
                self.builder.switch_to_block(next_test);
                let _ = j;
            }
        }
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: default_block }));

        for (i, arm) in arms.iter().enumerate() {
            self.builder.switch_to_block(arm_blocks[i]);

            if let Pattern::Ident(name) = &arm.pattern {
                let bind_local = self.builder.alloc_local(name.clone(), MirType::String);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: bind_local,
                    rvalue: MirRValue::Use(scrutinee_op.clone()),
                }));
                self.locals.insert(name.clone(), (bind_local, MirType::String));
            }

            let (body_val, arm_ty) = self.lower_expr(&arm.body)?;
            if i == 0 || result_ty == MirType::Void {
                result_ty = arm_ty;
            }
            if self.builder.current_block_unterminated() {
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Use(body_val),
                }));
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));
            }
        }

        self.builder.switch_to_block(merge_block);
        Ok((MirOperand::Local(result_local), result_ty))
    }

    /// Lower match with tuple patterns.
    pub(super) fn lower_tuple_match(
        &mut self,
        scrutinee: &Expr,
        arms: &[rask_ast::expr::MatchArm],
    ) -> Result<TypedOperand, LoweringError> {
        use rask_ast::expr::Pattern;

        let tuple_elems: Vec<(MirOperand, MirType)> = if let ExprKind::Tuple(elems) = &scrutinee.kind {
            let mut result = Vec::new();
            for elem in elems {
                result.push(self.lower_expr(elem)?);
            }
            result
        } else {
            // Non-literal scrutinee — if its type is a tuple, project each
            // field; otherwise treat it as a single-element vec.
            let (op, ty) = self.lower_expr(scrutinee)?;
            if let MirType::Tuple(field_tys) = &ty {
                let mut result = Vec::with_capacity(field_tys.len());
                for (i, field_ty) in field_tys.iter().enumerate() {
                    let field_local = self.builder.alloc_temp(field_ty.clone());
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                        dst: field_local,
                        rvalue: MirRValue::Field {
                            base: op.clone(),
                            field_index: i as u32,
                            byte_offset: None,
                            access: FieldAccess::Word,
                        },
                    }));
                    result.push((MirOperand::Local(field_local), field_ty.clone()));
                }
                result
            } else {
                vec![(op, ty)]
            }
        };

        let merge_block = self.builder.create_block();
        let result_local = self.builder.alloc_temp(MirType::I64);
        let mut result_ty = MirType::Void;

        let arm_test_blocks: Vec<BlockId> = arms.iter().map(|_| self.builder.create_block()).collect();
        let fallthrough = merge_block;

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: arm_test_blocks[0] }));

        for (i, arm) in arms.iter().enumerate() {
            self.builder.switch_to_block(arm_test_blocks[i]);
            let next_arm = if i + 1 < arm_test_blocks.len() {
                arm_test_blocks[i + 1]
            } else {
                fallthrough
            };

            let sub_patterns = match &arm.pattern {
                Pattern::Tuple(pats) => pats.clone(),
                Pattern::Wildcard => {
                    vec![]
                }
                _ => vec![arm.pattern.clone()],
            };

            let body_block = self.builder.create_block();
            let _current_pass = body_block;

            // A tuple element naming an enum variant is a *test*, not a binding.
            // Only literals were collected here, so `(Method.Get, "/tasks")`
            // checked the path and ignored the method: a POST to /tasks matched
            // the GET arm and answered with the task list.
            let mut variant_checks: Vec<(usize, u64)> = Vec::new();
            let mut checks: Vec<(usize, Pattern)> = Vec::new();
            for (j, pat) in sub_patterns.iter().enumerate() {
                match pat {
                    Pattern::Literal(_) => checks.push((j, pat.clone())),
                    Pattern::Ident(name) => {
                        let elem_ty = tuple_elems.get(j).map(|(_, t)| t);
                        if let Some(tag) = self.tuple_variant_tag(name, elem_ty) {
                            variant_checks.push((j, tag));
                        }
                    }
                    Pattern::Wildcard => {}
                    _ => {}
                }
            }

            // Emit the tag tests before anything else in the arm.
            for (j, want_tag) in &variant_checks {
                let (ref elem_op, _) = tuple_elems[*j];
                let tag_local = self.builder.alloc_temp(MirType::U16);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: tag_local,
                    rvalue: MirRValue::EnumTag { value: elem_op.clone() },
                }));
                let cmp_local = self.builder.alloc_temp(MirType::Bool);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: cmp_local,
                    rvalue: MirRValue::BinaryOp {
                        op: crate::BinOp::Eq,
                        left: MirOperand::Local(tag_local),
                        right: MirOperand::Constant(crate::operand::MirConst::Int(*want_tag as i64)),
                    },
                }));
                let pass_block = self.builder.create_block();
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: MirOperand::Local(cmp_local),
                    then_block: pass_block,
                    else_block: next_arm,
                }));
                self.builder.switch_to_block(pass_block);
            }
            let variant_tested: std::collections::HashSet<usize> =
                variant_checks.iter().map(|(j, _)| *j).collect();

            if checks.is_empty() && !matches!(&arm.pattern, Pattern::Wildcard) {
                for (j, pat) in sub_patterns.iter().enumerate() {
                    if let Pattern::Ident(name) = pat {
                        if j < tuple_elems.len() && !variant_tested.contains(&j) {
                            let (ref elem_op, ref elem_ty) = tuple_elems[j];
                            let local_id = self.builder.alloc_local(name.clone(), elem_ty.clone());
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                                dst: local_id,
                                rvalue: MirRValue::Use(elem_op.clone()),
                            }));
                            if let Some(prefix) = self.mir_type_name(elem_ty) {
                                self.meta_mut(name).type_prefix = Some(prefix);
                            }
                            self.locals.insert(name.clone(), (local_id, elem_ty.clone()));
                        }
                    }
                }
            } else if matches!(&arm.pattern, Pattern::Wildcard) {
                // No checks needed
            } else {
                let mut first_check = true;
                for (j, pat) in &checks {
                    if let Pattern::Literal(lit_expr) = pat {
                        let (ref elem_op, _) = tuple_elems[*j];
                        let (lit_op, _) = self.lower_expr(lit_expr)?;

                        let cmp_local = self.builder.alloc_temp(MirType::I64);
                        if matches!(&lit_expr.kind, ExprKind::String(_)) {
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                                dst: Some(cmp_local),
                                func: FunctionRef::internal("string_eq".to_string()),
                                args: vec![elem_op.clone(), lit_op],
                            }));
                        } else {
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                                dst: cmp_local,
                                rvalue: MirRValue::BinaryOp {
                                    op: crate::BinOp::Eq,
                                    left: elem_op.clone(),
                                    right: lit_op,
                                },
                            }));
                        }

                        let pass_block = if first_check {
                            first_check = false;
                            self.builder.create_block()
                        } else {
                            self.builder.create_block()
                        };
                        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                            cond: MirOperand::Local(cmp_local),
                            then_block: pass_block,
                            else_block: next_arm,
                        }));
                        self.builder.switch_to_block(pass_block);
                    }
                }

                for (j, pat) in sub_patterns.iter().enumerate() {
                    if let Pattern::Ident(name) = pat {
                        if j < tuple_elems.len() && !variant_tested.contains(&j) {
                            let (ref elem_op, ref elem_ty) = tuple_elems[j];
                            let local_id = self.builder.alloc_local(name.clone(), elem_ty.clone());
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                                dst: local_id,
                                rvalue: MirRValue::Use(elem_op.clone()),
                            }));
                            if let Some(prefix) = self.mir_type_name(elem_ty) {
                                self.meta_mut(name).type_prefix = Some(prefix);
                            }
                            self.locals.insert(name.clone(), (local_id, elem_ty.clone()));
                        }
                    }
                }
            }

            if let Some(guard_expr) = &arm.guard {
                let (guard_val, _) = self.lower_expr(guard_expr)?;
                let guard_pass = self.builder.create_block();
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: guard_val,
                    then_block: guard_pass,
                    else_block: next_arm,
                }));
                self.builder.switch_to_block(guard_pass);
            }

            if !matches!(&arm.pattern, Pattern::Wildcard) && checks.is_empty() {
                // Already in body block (bindings-only case)
            }

            let (body_val, arm_ty) = self.lower_expr(&arm.body)?;
            if i == 0 { result_ty = arm_ty; }

            if self.builder.current_block_unterminated() {
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Use(body_val),
                }));
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));
            }
        }

        self.builder.switch_to_block(merge_block);
        Ok((MirOperand::Local(result_local), result_ty))
    }

    /// Chain lowering for scalar (int/char) matches that include range patterns.
    /// Each arm becomes a boolean test branching to the body or the next arm.
    pub(super) fn lower_scalar_chain_match(
        &mut self,
        scrutinee_op: MirOperand,
        scrutinee_ty: MirType,
        arms: &[rask_ast::expr::MatchArm],
    ) -> Result<TypedOperand, LoweringError> {
        use rask_ast::expr::Pattern;

        let merge_block = self.builder.create_block();
        let arm_body_blocks: Vec<BlockId> = arms.iter().map(|_| self.builder.create_block()).collect();
        let result_local = self.builder.alloc_temp(MirType::I64);
        let mut result_ty = MirType::Void;

        for (i, arm) in arms.iter().enumerate() {
            let next_arm = if i + 1 < arms.len() {
                self.builder.create_block()
            } else {
                merge_block
            };

            // Catch-all patterns jump straight to the body.
            let is_catch_all = matches!(
                &arm.pattern,
                Pattern::Wildcard
                    | Pattern::Ident(_)
            );

            if !is_catch_all {
                // Build the condition by OR-ing the condition of each alternative.
                let alts = flatten_pattern_alternatives(&arm.pattern);
                let pass = arm_body_blocks[i];

                let n = alts.len();
                for (j, alt) in alts.into_iter().enumerate() {
                    let last = j + 1 == n;
                    let on_fail = if last { next_arm } else { self.builder.create_block() };
                    self.emit_pattern_test(&scrutinee_op, alt, pass, on_fail)?;
                    if !last {
                        self.builder.switch_to_block(on_fail);
                    }
                }
            } else {
                // Unconditional pass.
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                    target: arm_body_blocks[i],
                }));
            }

            // Body of this arm.
            self.builder.switch_to_block(arm_body_blocks[i]);

            // Bind the scrutinee to a catch-all identifier if present.
            if let Pattern::Ident(name) = &arm.pattern {
                // Scalar matches don't involve enums — just bind the value.
                let bind_local = self.builder.alloc_local(name.clone(), scrutinee_ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: bind_local,
                    rvalue: MirRValue::Use(scrutinee_op.clone()),
                }));
                self.locals.insert(name.clone(), (bind_local, scrutinee_ty.clone()));
            }

            if let Some(guard_expr) = &arm.guard {
                let (guard_val, _) = self.lower_expr(guard_expr)?;
                let guard_pass = self.builder.create_block();
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: guard_val,
                    then_block: guard_pass,
                    else_block: next_arm,
                }));
                self.builder.switch_to_block(guard_pass);
            }

            let (body_val, arm_ty) = self.lower_expr(&arm.body)?;
            if i == 0 { result_ty = arm_ty; }

            if self.builder.current_block_unterminated() {
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Use(body_val),
                }));
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                    target: merge_block,
                }));
            }

            if next_arm != merge_block {
                self.builder.switch_to_block(next_arm);
            }
        }

        self.builder.switch_to_block(merge_block);
        Ok((MirOperand::Local(result_local), result_ty))
    }

    /// Emit a boolean test for a non-Or pattern and branch accordingly.
    fn emit_pattern_test(
        &mut self,
        scrutinee_op: &MirOperand,
        pattern: &rask_ast::expr::Pattern,
        pass_block: BlockId,
        fail_block: BlockId,
    ) -> Result<(), LoweringError> {
        use rask_ast::expr::Pattern;

        let cond_local = self.builder.alloc_temp(MirType::Bool);
        match pattern {
            Pattern::Literal(lit_expr) => {
                let (lit_op, _) = self.lower_expr(lit_expr)?;
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: cond_local,
                    rvalue: MirRValue::BinaryOp {
                        op: crate::BinOp::Eq,
                        left: scrutinee_op.clone(),
                        right: lit_op,
                    },
                }));
            }
            Pattern::Range { start, end } => {
                let (start_op, _) = self.lower_expr(start)?;
                let (end_op, _) = self.lower_expr(end)?;

                // scrutinee >= start
                let lo_local = self.builder.alloc_temp(MirType::Bool);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: lo_local,
                    rvalue: MirRValue::BinaryOp {
                        op: crate::BinOp::Ge,
                        left: scrutinee_op.clone(),
                        right: start_op,
                    },
                }));

                // Short-circuit: if lo is false, skip the hi check.
                let hi_block = self.builder.create_block();
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: MirOperand::Local(lo_local),
                    then_block: hi_block,
                    else_block: fail_block,
                }));
                self.builder.switch_to_block(hi_block);

                // scrutinee <= end
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: cond_local,
                    rvalue: MirRValue::BinaryOp {
                        op: crate::BinOp::Le,
                        left: scrutinee_op.clone(),
                        right: end_op,
                    },
                }));
            }
            Pattern::Wildcard | Pattern::Ident(_) => {
                // Unconditional pass — caller should have handled this.
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                    target: pass_block,
                }));
                return Ok(());
            }
            _ => {
                // Unsupported scalar sub-pattern: always fail to stay safe.
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                    target: fail_block,
                }));
                return Ok(());
            }
        }

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(cond_local),
            then_block: pass_block,
            else_block: fail_block,
        }));
        Ok(())
    }

    /// Resolve enum variant name to its tag value from the layout.
    /// Tag for a tuple-element pattern that names an enum variant, or None when
    /// it's an ordinary binding.
    ///
    /// Accepts both the qualified form (`Method.Get`) and a bare variant name,
    /// resolved against the element's own enum layout — which is also what proves
    /// the name is a variant and not a variable.
    fn tuple_variant_tag(&self, name: &str, elem_ty: Option<&MirType>) -> Option<u64> {
        if let Some(tag) = self.resolve_pattern_tag(name) {
            return Some(tag);
        }
        let MirType::Enum(crate::types::EnumLayoutId { id, .. }) = elem_ty? else {
            return None;
        };
        let layout = self.ctx.enum_layouts.get(*id as usize)?;
        let bare = name.rsplit('.').next().unwrap_or(name);
        layout.variants.iter().find(|v| v.name == bare).map(|v| v.tag)
    }

    pub(super) fn resolve_pattern_tag(&self, name: &str) -> Option<u64> {
        let parts: Vec<&str> = name.splitn(2, '.').collect();
        let (enum_name, variant_name) = if parts.len() == 2 {
            (parts[0], parts[1])
        } else {
            return None;
        };
        match self.ctx.find_enum(enum_name) {
            Some((_, layout)) => layout
                .variants
                .iter()
                .find(|v| v.name == variant_name)
                .map(|v| v.tag),
            // `Ordering` is compiler-registered and has no layout. Without
            // this, `match x.compare(y)` built a switch with no cases at all
            // and every comparison took the last arm (#496).
            None if enum_name == "Ordering" => {
                rask_stdlib::ordering_tag(variant_name).map(|t| t as u64)
            }
            None => None,
        }
    }
}

/// The type-or-variant name a pattern matches on, if it names one.
fn pattern_name(pattern: &rask_ast::expr::Pattern) -> Option<&str> {
    use rask_ast::expr::Pattern;
    match pattern {
        Pattern::Ident(name)
        | Pattern::Constructor { name, .. }
        | Pattern::Struct { name, .. } => Some(name),
        Pattern::TypePat { ty_name, .. } => Some(ty_name),
        _ => None,
    }
}
