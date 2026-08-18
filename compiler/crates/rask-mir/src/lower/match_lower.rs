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

        // A `match` on a plain struct: the arms name fields, not variants, so
        // it's an ordered if-chain over field tests rather than a tag switch.
        // structs.md and SYNTAX.md both document it and neither backend had it —
        // the parser only accepted the qualified `Enum.Variant { … }` form, so it
        // never reached lowering at all (#307).
        if matches!(scrutinee_ty, MirType::Struct(_))
            && arms.iter().any(|a| matches!(&a.pattern, Pattern::Struct { .. }))
        {
            return self.lower_struct_match(scrutinee_op, scrutinee_ty, arms);
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
        // `Ordering` used to be exempt here: it had no layout, `compare` handed
        // back the bare tag, and reading a tag out of that would have
        // dereferenced 0, 1 or 2 as an address (#496). It has a layout now and
        // `compare` allocates a real value, so it takes the ordinary enum path
        // like anything else (#729).
        let has_tag = is_enum || is_result_or_option || patterns_imply_enum || is_niche;

        // Left as Options. A match on a plain enum or an integer has no ok/err
        // payload at all, so resolving one here would ask for a type that
        // doesn't exist and report every such match as unknown. The two places
        // that need these are both on the Result/Option path, and they demand
        // the type there — where not knowing it really is a gap.
        let ok_payload_ty = self.extract_payload_type(scrutinee)
            .or_else(|| match &scrutinee_ty {
                MirType::Result { ok, .. } => Some(ok.as_ref().clone()),
                MirType::Option(inner) => Some(inner.as_ref().clone()),
                _ => None,
            });
        let err_payload_ty = self.extract_err_type(scrutinee)
            .or_else(|| match &scrutinee_ty {
                MirType::Result { err, .. } => Some(err.as_ref().clone()),
                _ => None,
            });

        // Arms that name a variant of the error enum, by arm index. A `T or E`
        // match keys its switch on Ok vs Err, so a variant tag can't share that
        // switch: in `match r { i64 as v => …, MyErr.Bad(m) => …, MyErr.Worse => … }`
        // `Bad`'s variant tag 0 collided with the Ok arm's tag 0 and `Worse`'s
        // tag 1 collided with Err, so the error always ran whichever arm the
        // jump table kept (#677). These get a second switch inside the Err
        // branch instead.
        let err_variant_tags: Vec<Option<u64>> = if is_result_or_option {
            let err_layout_id = err_payload_ty.as_ref().and_then(|t| match t {
                MirType::Enum(crate::types::EnumLayoutId { id, .. }) => Some(*id),
                _ => None,
            });
            arms.iter()
                .map(|arm| {
                    let id = err_layout_id?;
                    // `MyErr.Bad` and a bare `Bad` name the same variant.
                    let name = pattern_name(&arm.pattern)?;
                    let bare = name.rsplit('.').next().unwrap_or(name);
                    let layout = self.ctx.enum_layouts.get(id as usize)?;
                    layout
                        .variants
                        .iter()
                        .find(|v| v.name == bare)
                        .map(|v| v.tag as u64)
                })
                .collect()
        } else {
            vec![None; arms.len()]
        };
        let two_level = err_variant_tags.iter().any(|t| t.is_some());

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
        // Arms are tried in order, so the *first* catch-all owns the switch
        // default. Letting a later one overwrite it skipped every catch-all
        // before it — which is what happens the moment a guard is involved:
        //
        //   match c {
        //       '+' => …
        //       _ if c.is_digit() => read_number()
        //       _ => error(c)
        //   }
        //
        // Both wildcards set the default, the second won, and the guarded arm
        // became unreachable. Every digit took the error arm (#675).
        // A guarded arm that fails falls through to the next arm below, so
        // ordering still works out once the first one wins.
        let mut default_claimed = false;

        for (i, arm) in arms.iter().enumerate() {
            // Error-variant arms belong to the inner switch, built below.
            if two_level && err_variant_tags[i].is_some() {
                continue;
            }
            match &arm.pattern {
                Pattern::Wildcard => {
                    if !default_claimed {
                        default_block = arm_blocks[i];
                        default_claimed = true;
                    }
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
                        // Unqualified: the scrutinee's own enum decides, not
                        // whichever layout happens to declare the name first.
                        let tag = self
                            .variant_tag_in_scrutinee(name, &scrutinee_ty)
                            .unwrap_or_else(|| self.variant_tag(name));
                        cases.push((tag as u64, arm_blocks[i]));
                    } else if !default_claimed {
                        // A plain binding pattern is a catch-all too.
                        default_block = arm_blocks[i];
                        default_claimed = true;
                    }
                }
                Pattern::Constructor { name, .. } => {
                    if let Some(tag) = self.resolve_pattern_tag(name) {
                        cases.push((tag, arm_blocks[i]));
                    } else if has_tag {
                        let tag = self
                            .variant_tag_in_scrutinee(name, &scrutinee_ty)
                            .unwrap_or_else(|| self.variant_tag(name));
                        cases.push((tag as u64, arm_blocks[i]));
                    } else {
                        cases.push((i as u64, arm_blocks[i]));
                    }
                }
                Pattern::Literal(lit_expr) => {
                    if let ExprKind::Int(v, _) = &lit_expr.kind {
                        cases.push((*v as u64, arm_blocks[i]));
                    } else if let ExprKind::Bool(b) = &lit_expr.kind {
                        cases.push((if *b { 1 } else { 0 }, arm_blocks[i]));
                    } else if let ExprKind::Char(c) = &lit_expr.kind {
                        // A char switches on its code point, same as an int.
                        // Without this the arm fell through to the index case
                        // below and `match c { '&' => … }` compared the code
                        // point against 0, 1, 2 — so no arm ever matched and
                        // every char took the wildcard. `c == '&'` was fine,
                        // which is why this hid for so long (#693).
                        cases.push((u32::from(*c) as u64, arm_blocks[i]));
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

        // Where an error-variant arm reads its payload from: the address of the
        // error enum sitting in the Result's payload slot.
        let mut err_value_local: Option<crate::LocalId> = None;

        if two_level {
            // Outer switch: Ok goes to the one arm that names the success side,
            // Err goes to a block that switches again on the variant tag.
            let mut ok_target = None;
            let mut err_catchall = None;
            for (i, arm) in arms.iter().enumerate() {
                if err_variant_tags[i].is_some() {
                    continue;
                }
                let name = match &arm.pattern {
                    Pattern::TypePat { ty_name, .. } => ty_name.as_str(),
                    Pattern::Ident(n) => n.as_str(),
                    _ => continue,
                };
                // An arm naming the error type itself (`MyErr as e`) catches
                // every variant the inner switch doesn't list.
                if self.pattern_is_err_side(name, &scrutinee_ty) {
                    err_catchall.get_or_insert(arm_blocks[i]);
                } else {
                    ok_target.get_or_insert(arm_blocks[i]);
                }
            }
            let err_dispatch = self.builder.create_block();
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Switch {
                value: switch_val,
                cases: vec![(0, ok_target.unwrap_or(default_block)), (1, err_dispatch)],
                default: default_block,
            }));

            self.builder.switch_to_block(err_dispatch);
            let err_ty = err_payload_ty
                .clone()
                .unwrap_or_else(|| crate::fallback::i64_fallback("lower/match_lower:err_enum"));
            let err_local = self.builder.alloc_temp(err_ty.clone());
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: err_local,
                rvalue: MirRValue::Field {
                    base: scrutinee_op.clone(),
                    field_index: 0,
                    // An enum payload is an aggregate, so this hands back its
                    // address rather than loading a word.
                    byte_offset: None,
                    access: FieldAccess::Word,
                },
            }));
            err_value_local = Some(err_local);
            let inner_tag = self.emit_option_tag(&MirOperand::Local(err_local), false);
            let inner_cases: Vec<(u64, BlockId)> = err_variant_tags
                .iter()
                .enumerate()
                .filter_map(|(i, tag)| tag.map(|t| (t, arm_blocks[i])))
                .collect();
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Switch {
                value: MirOperand::Local(inner_tag),
                cases: inner_cases,
                default: err_catchall.unwrap_or(default_block),
            }));
        } else {
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Switch {
                value: switch_val,
                cases,
                default: default_block,
            }));
        }

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
                    // An error-variant arm reads out of the error enum, whose
                    // address err_dispatch computed; everything else reads out
                    // of the scrutinee.
                    let enum_layout_id = if err_variant_tags[i].is_some() {
                        match &err_payload_ty {
                            Some(MirType::Enum(crate::types::EnumLayoutId { id, .. })) => Some(*id),
                            _ => None,
                        }
                    } else if let MirType::Enum(crate::types::EnumLayoutId { id, .. }) = &scrutinee_ty {
                        Some(*id)
                    } else {
                        None
                    };
                    let payload_base = match (err_variant_tags[i], err_value_local) {
                        (Some(_), Some(l)) => MirOperand::Local(l),
                        _ => scrutinee_op.clone(),
                    };
                    // (mir type, absolute byte offset within the enum, field size)
                    let variant_fields: Option<Vec<(MirType, u32, u32)>> =
                        enum_layout_id.and_then(|idx| {
                            self.ctx.enum_layouts.get(idx as usize).and_then(|layout| {
                                layout.variants.iter().find(|v| v.name == variant_name).map(|v| {
                                    v.fields.iter().map(|f| {
                                        (self.ctx.type_to_mir(&f.ty), v.payload_offset + f.offset, f.size)
                                    }).collect()
                                })
                            })
                        });

                    for (j, field_pat) in fields.iter().enumerate() {
                        if let Pattern::Ident(binding) = field_pat {
                            // For a user enum, pass the exact payload offset + size so codegen
                            // doesn't guess the variant (mixed variants share field indices).
                            let (field_ty, field_loc) = if let Some(ref vf) = variant_fields {
                                vf.get(j)
                                    .map(|(ty, off, sz)| (ty.clone(), Some((*off, *sz))))
                                    .unwrap_or_else(|| (
                                        crate::fallback::i64_fallback(
                                            "lower/match_lower:enum_variant_field"),
                                        None,
                                    ))
                            } else {
                                let ty = match name.as_str() {
                                    "Err" => err_payload_ty.clone(),
                                    _ => ok_payload_ty.clone(),
                                }
                                .unwrap_or_else(|| crate::fallback::i64_fallback(
                                    "lower/match_lower:ok_err_binding"));
                                (ty, None)
                            };
                            let payload_local = self.builder.alloc_local(
                                binding.clone(), field_ty.clone(),
                            );
                            let rvalue = if is_niche {
                                MirRValue::Use(scrutinee_op.clone())
                            } else {
                                MirRValue::Field {
                                    base: payload_base.clone(),
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
                                    enum_layout_id.and_then(|idx| {
                                        self.ctx.enum_layouts.get(idx as usize).and_then(|layout| {
                                            layout.variants.iter().find(|v| v.name == variant_name).and_then(|v| {
                                                v.fields.get(j).and_then(|f| {
                                                    super::MirContext::type_prefix(&f.ty, self.ctx.type_names)
                                                })
                                            })
                                        })
                                    })
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
                    // Same split as the tuple-variant arm above: an error
                    // variant's fields live in the error enum, reached through
                    // the address err_dispatch computed.
                    let enum_layout_id = if err_variant_tags[i].is_some() {
                        match &err_payload_ty {
                            Some(MirType::Enum(crate::types::EnumLayoutId { id, .. })) => Some(*id),
                            _ => None,
                        }
                    } else if let MirType::Enum(crate::types::EnumLayoutId { id, .. }) = &scrutinee_ty {
                        Some(*id)
                    } else {
                        None
                    };
                    let payload_base = match (err_variant_tags[i], err_value_local) {
                        (Some(_), Some(l)) => MirOperand::Local(l),
                        _ => scrutinee_op.clone(),
                    };
                    if let Some(idx) = enum_layout_id {
                        if let Some(layout) = self.ctx.enum_layouts.get(idx as usize) {
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
                                                base: payload_base.clone(),
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
                            }
                            .unwrap_or_else(|| crate::fallback::i64_fallback(
                                "lower/match_lower:typepat_payload"));
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
                } else if default_block == arm_blocks[i] {
                    // Last arm, and it's the catch-all that the switch already
                    // defaults to. Falling back to `default_block` here would
                    // branch this block at itself and spin.
                    merge_block
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
            // A tuple element that names a variant *and* binds its payload —
            // `(Value.Int(x), Value.Int(y))`. Same tag test as a bare variant
            // name, plus the payload reads. This arm fell into the catch-all
            // below, so no test ran and `x` was never defined: MIR lowering
            // failed with "unresolved variable `x`" while the interpreter
            // answered fine (#793).
            let mut payload_binds: Vec<(usize, u64, String, Vec<Pattern>)> = Vec::new();
            for (j, pat) in sub_patterns.iter().enumerate() {
                match pat {
                    Pattern::Literal(_) => checks.push((j, pat.clone())),
                    Pattern::Ident(name) => {
                        let elem_ty = tuple_elems.get(j).map(|(_, t)| t);
                        if let Some(tag) = self.tuple_variant_tag(name, elem_ty) {
                            variant_checks.push((j, tag));
                        }
                    }
                    Pattern::Constructor { name, fields } => {
                        let elem_ty = tuple_elems.get(j).map(|(_, t)| t);
                        if let Some(tag) = self.tuple_variant_tag(name, elem_ty) {
                            payload_binds.push((j, tag, name.clone(), fields.clone()));
                        }
                    }
                    Pattern::Wildcard => {}
                    _ => {}
                }
            }
            // The tag test is the same shape for both, so they share the loop.
            variant_checks.extend(payload_binds.iter().map(|(j, tag, _, _)| (*j, *tag)));

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

            // Every tag test has passed by now, so the payloads are the right
            // variant's. Read each bound field out of the element at its exact
            // offset — mixed variants share field indices, so codegen must not
            // be left to guess which variant a field index belongs to.
            for (j, _, variant_path, fields) in &payload_binds {
                let (elem_op, elem_ty) = tuple_elems[*j].clone();
                self.bind_tuple_element_payload(&elem_op, &elem_ty, variant_path, fields)?;
            }

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
    /// `match p { Point { x: 0, y } => …, Point { x, y } => … }`.
    ///
    /// Arms are tried in order. An arm matches when every field pattern that
    /// tests a value agrees; field patterns that only bind always agree, and are
    /// what the body reads. Modelled on `lower_scalar_chain_match` — the shape is
    /// the same, the condition is per-field instead of on the scrutinee (#307).
    pub(super) fn lower_struct_match(
        &mut self,
        scrutinee_op: MirOperand,
        scrutinee_ty: MirType,
        arms: &[rask_ast::expr::MatchArm],
    ) -> Result<TypedOperand, LoweringError> {
        use rask_ast::expr::Pattern;

        let merge_block = self.builder.create_block();
        let arm_body_blocks: Vec<BlockId> =
            arms.iter().map(|_| self.builder.create_block()).collect();
        let result_local = self.builder.alloc_temp(MirType::I64);
        let mut result_ty = MirType::Void;

        for (i, arm) in arms.iter().enumerate() {
            let next_arm = if i + 1 < arms.len() {
                self.builder.create_block()
            } else {
                merge_block
            };

            // Every field the pattern names is read once, whether it's tested or
            // bound — a test needs the value and so does the body.
            let mut reads: Vec<(String, crate::LocalId, MirType, &Pattern)> = Vec::new();
            if let Pattern::Struct { fields, .. } = &arm.pattern {
                for (field_name, field_pat) in fields {
                    let Some((field_idx, field_layout)) =
                        self.struct_field(&scrutinee_ty, field_name)
                    else {
                        continue;
                    };
                    let field_ty = self.ctx.type_to_mir(&field_layout.ty);
                    // Named for the binding when there is one, so debug info and
                    // the local's name agree.
                    let local_name = match field_pat {
                        Pattern::Ident(binding) => binding.clone(),
                        _ => format!("__match_{}", field_name),
                    };
                    let local = self.builder.alloc_local(local_name, field_ty.clone());
                    let rvalue = MirRValue::Field {
                        base: scrutinee_op.clone(),
                        field_index: field_idx as u32,
                        byte_offset: Some(field_layout.offset),
                        access: FieldAccess::for_field(&field_ty, field_layout.size),
                    };
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                        dst: local,
                        rvalue,
                    }));
                    reads.push((field_name.clone(), local, field_ty, field_pat));
                }
            }

            // A pattern that only binds is a catch-all: nothing to test.
            let tests: Vec<(crate::LocalId, &Pattern)> = reads
                .iter()
                .filter(|(_, _, _, pat)| {
                    !matches!(pat, Pattern::Ident(_) | Pattern::Wildcard)
                })
                .map(|(_, local, _, pat)| (*local, *pat))
                .collect();

            if tests.is_empty() {
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                    target: arm_body_blocks[i],
                }));
            } else {
                let n = tests.len();
                for (j, (local, pat)) in tests.into_iter().enumerate() {
                    let last = j + 1 == n;
                    // Each test's success moves to the next one; the last one's
                    // success is the arm body. Any failure skips to the next arm.
                    let pass = if last {
                        arm_body_blocks[i]
                    } else {
                        self.builder.create_block()
                    };
                    self.emit_pattern_test(&MirOperand::Local(local), pat, pass, next_arm)?;
                    if !last {
                        self.builder.switch_to_block(pass);
                    }
                }
            }

            self.builder.switch_to_block(arm_body_blocks[i]);

            // The bindings the body reads.
            for (_, local, field_ty, pat) in &reads {
                if let Pattern::Ident(binding) = pat {
                    if let Some(p) = self.mir_type_name(field_ty) {
                        self.meta_mut(binding.as_str()).type_prefix = Some(p);
                    }
                    self.locals
                        .insert(binding.clone(), (*local, field_ty.clone()));
                }
            }
            // A whole-value catch-all (`p => …`) binds the struct itself.
            if let Pattern::Ident(name) = &arm.pattern {
                let bind_local = self.builder.alloc_local(name.clone(), scrutinee_ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: bind_local,
                    rvalue: MirRValue::Use(scrutinee_op.clone()),
                }));
                self.locals
                    .insert(name.clone(), (bind_local, scrutinee_ty.clone()));
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

            if next_arm != merge_block {
                self.builder.switch_to_block(next_arm);
            }
        }

        self.builder.switch_to_block(merge_block);
        Ok((MirOperand::Local(result_local), result_ty))
    }

    /// A struct field's index and layout by name.
    fn struct_field(
        &self,
        ty: &MirType,
        field_name: &str,
    ) -> Option<(usize, rask_mono::FieldLayout)> {
        let MirType::Struct(crate::types::StructLayoutId { id, .. }) = ty else {
            return None;
        };
        let layout = self.ctx.struct_layouts.get(*id as usize)?;
        layout
            .fields
            .iter()
            .enumerate()
            .find(|(_, f)| f.name == field_name)
            .map(|(i, f)| (i, f.clone()))
    }

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
    /// Bind the fields a constructor sub-pattern named, reading them out of one
    /// tuple element. Called after that element's tag test has passed, so the
    /// variant is known and its payload offsets apply.
    ///
    /// A nested pattern (`Value.Pair(Value.Int(n), _)`) isn't handled: only a
    /// plain binding or `_` per field. Nesting would need the whole match tree
    /// rebuilt for tuple scrutinees, and it reports rather than binding the
    /// wrong bytes.
    fn bind_tuple_element_payload(
        &mut self,
        elem_op: &MirOperand,
        elem_ty: &MirType,
        variant_path: &str,
        fields: &[rask_ast::expr::Pattern],
    ) -> Result<(), LoweringError> {
        use rask_ast::expr::Pattern;
        let variant_name = variant_path.rsplit('.').next().unwrap_or(variant_path);
        let MirType::Enum(crate::types::EnumLayoutId { id, .. }) = elem_ty else {
            return Err(LoweringError::InvalidConstruct(format!(
                "`{variant_path}(…)` in a tuple pattern needs an enum element, \
                 found `{elem_ty:?}`"
            )));
        };
        // (mir type, absolute byte offset within the enum, field size)
        let variant_fields: Vec<(MirType, u32, u32)> = self
            .ctx
            .enum_layouts
            .get(*id as usize)
            .and_then(|layout| {
                layout.variants.iter().find(|v| v.name == variant_name).map(|v| {
                    v.fields
                        .iter()
                        .map(|f| {
                            (self.ctx.type_to_mir(&f.ty), v.payload_offset + f.offset, f.size)
                        })
                        .collect()
                })
            })
            .ok_or_else(|| {
                LoweringError::InvalidConstruct(format!(
                    "no variant `{variant_name}` to read a payload from"
                ))
            })?;

        for (j, field_pat) in fields.iter().enumerate() {
            let binding = match field_pat {
                Pattern::Ident(name) => name,
                Pattern::Wildcard => continue,
                other => {
                    return Err(LoweringError::InvalidConstruct(format!(
                        "`{variant_path}(…)` inside a tuple pattern binds names or \
                         `_` per field; `{other:?}` would need a nested match"
                    )));
                }
            };
            let Some((field_ty, offset, size)) = variant_fields.get(j).cloned() else {
                return Err(LoweringError::InvalidConstruct(format!(
                    "`{variant_path}` has {} fields, pattern binds {}",
                    variant_fields.len(),
                    fields.len()
                )));
            };
            let payload_local = self.builder.alloc_local(binding.clone(), field_ty.clone());
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: payload_local,
                rvalue: MirRValue::Field {
                    base: elem_op.clone(),
                    field_index: j as u32,
                    byte_offset: Some(offset),
                    access: FieldAccess::for_field(&field_ty, size),
                },
            }));
            if let Some(prefix) = self.mir_type_name(&field_ty) {
                self.meta_mut(binding).type_prefix = Some(prefix);
            }
            self.locals.insert(binding.clone(), (payload_local, field_ty));
        }
        Ok(())
    }

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
pub(crate) fn pattern_name(pattern: &rask_ast::expr::Pattern) -> Option<&str> {
    use rask_ast::expr::Pattern;
    match pattern {
        Pattern::Ident(name)
        | Pattern::Constructor { name, .. }
        | Pattern::Struct { name, .. } => Some(name),
        Pattern::TypePat { ty_name, .. } => Some(ty_name),
        _ => None,
    }
}
