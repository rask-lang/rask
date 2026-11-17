// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Match expression lowering: enum/tagged dispatch, string match, tuple match.

use crate::FieldAccess;
use super::{LoweringError, MirLowerer, TypedOperand};

/// A struct pattern's field, read out of the matched value: where it was put,
/// what it is lowered as, what the checker calls it, and the pattern for it.
struct FieldRead<'p> {
    local: crate::LocalId,
    ty: MirType,
    checked: Type,
    pattern: &'p Pattern,
}
use crate::{
    operand::MirConst, BlockId, FunctionRef, MirOperand, MirRValue, MirStmt,
    MirStmtKind, MirTerminator, MirTerminatorKind, MirType,
};
use rask_ast::expr::{Expr, ExprKind, Pattern};
use rask_types::Type;
use rask_ast::ty::TypeExpr;

/// Walk a pattern to see if it contains a range pattern anywhere.
fn contains_range_pattern(pattern: &rask_ast::expr::Pattern) -> bool {
    use rask_ast::expr::PatternKind;
    match &pattern.kind {
        PatternKind::Range { .. } => true,
        PatternKind::Or(pats) => pats.iter().any(contains_range_pattern),
        _ => false,
    }
}

/// One row of an enum `match`: a single alternative of an arm's pattern, and
/// the block the switch sends its tag to.
struct EnumRow {
    arm: usize,
    pat: Pattern,
    entry: BlockId,
}

/// Does `pattern` hold an or-pattern anywhere?
pub(super) fn contains_or(pattern: &Pattern) -> bool {
    use rask_ast::expr::PatternKind;
    match &pattern.kind {
        PatternKind::Or(_) => true,
        PatternKind::Constructor { fields, .. } | PatternKind::Tuple(fields) => fields.iter().any(contains_or),
        PatternKind::Struct { fields, .. } => fields.iter().any(|(_, p)| contains_or(p)),
        _ => false,
    }
}

/// Every way to pick one expansion of each part.
fn product(parts: Vec<Vec<Pattern>>) -> Vec<Vec<Pattern>> {
    parts.into_iter().fold(vec![Vec::new()], |acc, options| {
        acc.iter()
            .flat_map(|prefix| {
                options.iter().map(move |o| {
                    let mut next = prefix.clone();
                    next.push(o.clone());
                    next
                })
            })
            .collect()
    })
}

/// The patterns an or-pattern stands for, with no or-pattern left in them:
/// `Inner(Blue | Red)` is `Inner(Blue)` and `Inner(Red)`, `(A | B, C | D)` is
/// four tuples. A pattern with none comes back as itself.
///
/// Each alternative then lowers like an arm of its own, with its tests and
/// bindings in place, and the alternatives of one arm share its body.
pub(super) fn expand_or(pattern: &Pattern) -> Vec<Pattern> {
    use rask_ast::expr::PatternKind;
    if !contains_or(pattern) {
        return vec![pattern.clone()];
    }
    let rebuilt = |kind: PatternKind| Pattern::new(pattern.id, kind, pattern.span);
    match &pattern.kind {
        PatternKind::Or(alts) => alts.iter().flat_map(expand_or).collect(),
        PatternKind::Constructor { name, fields } => product(fields.iter().map(expand_or).collect())
            .into_iter()
            .map(|fields| rebuilt(PatternKind::Constructor { name: name.clone(), fields }))
            .collect(),
        PatternKind::Tuple(fields) => product(fields.iter().map(expand_or).collect())
            .into_iter()
            .map(|fields| rebuilt(PatternKind::Tuple(fields)))
            .collect(),
        PatternKind::Struct { name, fields, rest } => {
            product(fields.iter().map(|(_, p)| expand_or(p)).collect())
                .into_iter()
                .map(|pats| {
                    let fields = fields.iter().map(|(n, _)| n.clone()).zip(pats).collect();
                    rebuilt(PatternKind::Struct { name: name.clone(), fields, rest: *rest })
                })
                .collect()
        }
        _ => vec![pattern.clone()],
    }
}

/// Switch cases with each tag kept once, for the first arm that named it.
/// Two arms naming one variant (the first guarded) both list its tag, and
/// arms are tried in order.
fn first_per_tag(cases: impl Iterator<Item = (u64, BlockId)>) -> Vec<(u64, BlockId)> {
    let mut seen = std::collections::HashSet::new();
    cases.filter(|(t, _)| seen.insert(*t)).collect()
}

/// Flatten an Or pattern into its alternatives. Non-Or patterns return themselves.
fn flatten_pattern_alternatives(pattern: &rask_ast::expr::Pattern) -> Vec<&rask_ast::expr::Pattern> {
    use rask_ast::expr::PatternKind;
    match &pattern.kind {
        PatternKind::Or(pats) => pats.iter().collect(),
        _ => vec![pattern],
    }
}

impl<'a> MirLowerer<'a> {

    /// Where a match goes when no arm took the value: a panic, as on the
    /// interpreter (R0011). The checker proves most matches cover everything,
    /// but a guarded arm can still fail with nothing after it. This used to be
    /// the merge block, which read a result no arm had written, and an owned
    /// result arriving there from one path and garbage from another left the
    /// release pass unable to say who held it (text_editor's undo leaked).
    fn no_arm_matched_block(&mut self) -> BlockId {
        let here = self.builder.current_block();
        let block = self.builder.create_block();
        self.builder.switch_to_block(block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::internal("panic".to_string()),
            args: vec![MirOperand::Constant(MirConst::String("no matching arm in match".to_string()))],
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Unreachable));
        self.builder.switch_to_block(here);
        block
    }

    /// A value that can fail *and* be absent — `T? or E` — carries an error
    /// tag around an option tag. Nothing else in MIR nests two wrappers.
    pub(super) fn is_flat_two_layer(ty: &MirType) -> bool {
        match ty {
            MirType::Result { ok, err } => {
                !matches!(**err, MirType::Void)
                    && matches!(**ok, MirType::Option(_))
            }
            _ => false,
        }
    }

    /// Which leaf of a flat `T? or E` a pattern name picks: 0 the payload,
    /// 1 `none`, 2 the error. Same numbering `emit_flat_leaf` computes.
    pub(super) fn flat_leaf_of(&self, pat: &Pattern, name: &str, ty: &MirType) -> Result<u64, LoweringError> {
        Ok(if name == "none" {
            1
        } else if self.pattern_is_err_side(pat, name, ty)? {
            2
        } else {
            0
        })
    }

    /// Flatten a `T? or E` into the leaf it holds, as `flat_leaf_of` numbers
    /// them. Also hands back the inner `T?`, which is where the payload reads
    /// from: it sits inline in the result's payload slot, so what's wanted is
    /// its address. Loading a word there would hand back the option's tag and
    /// the next read would dereference it.
    pub(super) fn emit_flat_leaf(
        &mut self,
        value: &MirOperand,
        inner_opt_ty: &MirType,
    ) -> (crate::LocalId, crate::LocalId) {
        let inner_local = self.builder.alloc_temp(inner_opt_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: inner_local,
            rvalue: self.payload_field(value.clone(), &MirType::Result { ok: Box::new(inner_opt_ty.clone()), err: Box::new(MirType::Void) }, inner_opt_ty),
        }));

        // The inner tag is only read on the success side. On the error side
        // those bytes belong to the error.
        let leaf = self.builder.alloc_temp(MirType::U8);
        let outer_tag = self.emit_option_tag(value, None);
        let err_blk = self.builder.create_block();
        let ok_blk = self.builder.create_block();
        let done_blk = self.builder.create_block();
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
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: done_blk }));
        self.builder.switch_to_block(ok_blk);
        let inner_tag = self.emit_option_tag(&MirOperand::Local(inner_local), None);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: leaf,
            rvalue: MirRValue::Use(MirOperand::Local(inner_tag)),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: done_blk }));
        self.builder.switch_to_block(done_blk);
        (inner_local, leaf)
    }

    /// `match` on a `T or E` whose arms reach past the plain ok/err split:
    /// an ordered chain, one `is` test per arm.
    ///
    /// Two shapes need it. A flat `T? or E`'s leaves (`T`, `none`, `E`) sit
    /// behind two tags. And an arm naming a union member or a variant, of
    /// either side's enum or of one member of a union error side, is two or
    /// three tags deep. `value is <pattern>` already reads each of those and
    /// knows where its payload binds from, so going through that same test
    /// means `match` and `is` can't disagree. A switch on one flattened
    /// discriminant gave every error arm the same case, so
    /// `MyErr.Bad(m) => …, MyErr.Worse => …` both landed in the first, and
    /// `m` was never bound.
    fn lower_result_chain_match(
        &mut self,
        scrutinee: &Expr,
        scrutinee_op: MirOperand,
        scrutinee_ty: MirType,
        arms: &[rask_ast::expr::MatchArm],
    ) -> Result<TypedOperand, LoweringError> {
        use rask_ast::expr::PatternKind;

        // A flat value's error leaf binds the error; its payload leaf binds
        // out of the inner option, which the test hands back.
        let flat_err_ty = match &scrutinee_ty {
            MirType::Result { err, .. } if Self::is_flat_two_layer(&scrutinee_ty) => Some((**err).clone()),
            _ => None,
        };
        let no_match = self.no_arm_matched_block();
        let merge_block = self.builder.create_block();
        let mut result_ty = MirType::Void;
        let result_local = self.builder.alloc_temp(MirType::I64);

        for (i, arm) in arms.iter().enumerate() {
            let next = self.builder.create_block();
            let arm_block = self.builder.create_block();
            let catch_all = match &arm.pattern.kind {
                PatternKind::Wildcard => true,
                PatternKind::Ident(_) => self.pattern_binds(&arm.pattern),
                _ => false,
            };
            let test = if catch_all {
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: arm_block }));
                None
            } else if contains_or(&arm.pattern) {
                // One test per alternative. Each alternative that passes
                // binds its own names in a block of its own, copies them into
                // the locals the body reads, and jumps to the shared body.
                let mut merged: Vec<(String, crate::LocalId, MirType)> = Vec::new();
                for alt in expand_or(&arm.pattern) {
                    let test = self.emit_is_test(scrutinee, &scrutinee_op, &scrutinee_ty, None, &alt)?;
                    let bind_block = self.builder.create_block();
                    let try_next = self.builder.create_block();
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                        cond: MirOperand::Local(test.matches),
                        then_block: bind_block,
                        else_block: try_next,
                    }));
                    self.builder.switch_to_block(bind_block);
                    let snapshot = self.snapshot_bindable(&alt);
                    self.bind_tested_pattern(
                        &test, &alt, scrutinee, scrutinee_op.clone(),
                        flat_err_ty.clone(), false, &scrutinee_ty,
                    );
                    self.merge_alt_bindings(&snapshot, &mut merged);
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: arm_block }));
                    self.builder.switch_to_block(try_next);
                }
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: next }));
                self.builder.switch_to_block(arm_block);
                self.install_merged(&merged);
                None
            } else {
                let test = self.emit_is_test(scrutinee, &scrutinee_op, &scrutinee_ty, None, &arm.pattern)?;
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: MirOperand::Local(test.matches),
                    then_block: arm_block,
                    else_block: next,
                }));
                Some(test)
            };

            self.builder.switch_to_block(arm_block);
            if let Some(test) = &test {
                self.bind_tested_pattern(
                    test, &arm.pattern, scrutinee, scrutinee_op.clone(),
                    flat_err_ty.clone(), false, &scrutinee_ty,
                );
            } else if let PatternKind::Ident(name) = &arm.pattern.kind {
                let bind_local = self.builder.alloc_local(name.clone(), scrutinee_ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: bind_local,
                    rvalue: MirRValue::Use(scrutinee_op.clone()),
                }));
                self.locals.insert(name.clone(), (bind_local, scrutinee_ty.clone()));
                self.retain_pattern_copies(&arm.pattern, scrutinee, &scrutinee_ty);
            }

            if let Some(guard_expr) = &arm.guard {
                // A failed guard carries on down the chain.
                let (guard_val, _) = self.lower_expr(guard_expr)?;
                let guard_pass = self.builder.create_block();
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: guard_val,
                    then_block: guard_pass,
                    else_block: next,
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
            self.builder.switch_to_block(next);
        }
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: no_match }));

        self.builder.switch_to_block(merge_block);
        // The local was allocated before any arm was lowered, so its type
        // started as a placeholder word. Now that the arms have reported one,
        // give it the real one: assigning an f64 into an `i64` local converts
        // rather than reinterprets, so a `match` used as an expression handed
        // back its float arms truncated — `match n { 1 => 2.5, _ => 0.0 }` was
        // 2 (#973).
        self.builder.set_local_type(result_local, result_ty.clone());
        Ok((MirOperand::Local(result_local), result_ty))
    }

    /// Match expression lowering (spec L2).
    pub(super) fn lower_match(
        &mut self,
        scrutinee: &Expr,
        arms: &[rask_ast::expr::MatchArm],
    ) -> Result<TypedOperand, LoweringError> {
        use rask_ast::expr::{Pattern, PatternKind};

        // Tuple pattern matching
        let has_tuple_patterns = arms.iter().any(|a| matches!(&a.pattern.kind, PatternKind::Tuple(_)));
        if has_tuple_patterns {
            return self.lower_tuple_match(scrutinee, arms);
        }

        // Range patterns can't be a switch case — fall back to an if-chain.
        let has_range = arms.iter().any(|a| contains_range_pattern(&a.pattern));
        if has_range {
            let (scrutinee_op, scrutinee_ty) = self.lower_expr(scrutinee)?;
            let (scrutinee_op, scrutinee_ty) = self.peel_heap_value(scrutinee_op, scrutinee_ty);
            return self.lower_scalar_chain_match(scrutinee_op, scrutinee_ty, arms);
        }

        let niche = self.niche_sentinel_of_expr(scrutinee);
        let is_niche = niche.is_some();
        // A `Heap<T>` matched as a `T` (HP5) — `match rest` on the tail of a
        // `Cons(i64, Heap<List>)`. Without this the scrutinee's type was the
        // block rather than the enum, so no layout was found and every payload
        // binding fell back to a word.
        let (scrutinee_op, scrutinee_ty) = self.lower_expr(scrutinee)?;
        let (scrutinee_op, scrutinee_ty) = self.peel_heap_value(scrutinee_op, scrutinee_ty);

        // String match
        let is_string_match = matches!(scrutinee_ty, MirType::Buffer)
            || self.ctx.lookup_raw_type(scrutinee.id)
                .map_or(false, |ty| matches!(&*ty, rask_types::Type::String));
        if is_string_match {
            return self.lower_string_match(scrutinee_op, arms);
        }

        // A flat `T? or E` wears two tags, and an arm naming a union member
        // or a variant reads two or three: neither is one switch.
        let reaches_past_side = matches!(scrutinee_ty, MirType::Result { .. })
            && arms.iter().any(|a| {
                flatten_pattern_alternatives(&a.pattern)
                    .into_iter()
                    .any(|p| self.deep_result_path(p, &scrutinee_ty).is_some())
            });
        if Self::is_flat_two_layer(&scrutinee_ty) || reaches_past_side {
            return self.lower_result_chain_match(scrutinee, scrutinee_op, scrutinee_ty, arms);
        }

        // A `match` on a plain struct: the arms name fields, not variants, so
        // it's an ordered if-chain over field tests rather than a tag switch.
        // structs.md and SYNTAX.md both document it and neither backend had it —
        // the parser only accepted the qualified `Enum.Variant { … }` form, so it
        // never reached lowering at all (#307).
        if matches!(scrutinee_ty, MirType::Struct(_))
            && arms.iter().any(|a| matches!(&a.pattern.kind, PatternKind::Struct { .. }))
        {
            return self.lower_struct_match(scrutinee, scrutinee_op, scrutinee_ty, arms);
        }

        let is_enum = matches!(scrutinee_ty, MirType::Enum(_));

        let is_result_or_option = if !is_enum {
            self.ctx.lookup_raw_type(scrutinee.id).map_or(false, |ty| {
                matches!(&*ty, rask_types::Type::Result { .. })
            })
        } else {
            false
        };

        let patterns_imply_enum = if !is_enum && !is_result_or_option {
            arms.iter().any(|arm| match &arm.pattern.kind {
                PatternKind::Constructor { .. } | PatternKind::Struct { .. } => {
                    self.pattern_names_variant(&arm.pattern)
                }
                PatternKind::Ident(name) => {
                    matches!(self.pattern_target(&arm.pattern), Some(rask_types::PatternTarget::Variant { .. }))
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

        let switch_val = if has_tag {
            let tag_local = self.emit_option_tag(&scrutinee_op, niche);
            MirOperand::Local(tag_local)
        } else {
            scrutinee_op.clone()
        };

        let merge_block = self.builder.create_block();
        let arm_blocks: Vec<BlockId> = arms.iter().map(|_| self.builder.create_block()).collect();
        // One row per alternative of each arm's or-pattern (`A | B(1)` is two
        // rows, `(A | B, C | D)` four). Rows share the arm's body block; an
        // arm with one row is its own entry.
        let mut rows: Vec<EnumRow> = Vec::new();
        let mut alts_of_arm: Vec<usize> = Vec::with_capacity(arms.len());
        for (i, arm) in arms.iter().enumerate() {
            let alts = expand_or(&arm.pattern);
            alts_of_arm.push(alts.len());
            let multi = alts.len() > 1;
            for alt in alts {
                let entry = if multi { self.builder.create_block() } else { arm_blocks[i] };
                rows.push(EnumRow { arm: i, pat: alt, entry });
            }
        }
        let no_match = self.no_arm_matched_block();

        let mut cases: Vec<(u64, BlockId, usize)> = Vec::new();
        // Rows that take any value, by index. A failed guard dispatches again
        // over the arms below it, and these are its default.
        let mut catch_all = vec![false; rows.len()];
        let mut default_block = no_match;
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

        for (r, row) in rows.iter().enumerate() {
            let i = row.arm;
            match &row.pat.kind {
                PatternKind::Wildcard => {
                    catch_all[r] = true;
                    if !default_claimed {
                        default_block = row.entry;
                        default_claimed = true;
                    }
                }
                PatternKind::Ident(name) => {
                    if let Some(tag) = self.pattern_variant_tag(&row.pat, &scrutinee_ty) {
                        cases.push((tag as u64, row.entry, r));
                    } else if self.pattern_binds(&row.pat) {
                        // A plain binding pattern is a catch-all.
                        catch_all[r] = true;
                        if !default_claimed {
                            default_block = row.entry;
                            default_claimed = true;
                        }
                    } else if has_tag && is_result_or_option {
                        // Result match: ok arm = tag 0, err arm = tag 1, decided
                        // by the scrutinee's real ok/err type identities.
                        let tag = self.pattern_is_err_side(&row.pat, name, &scrutinee_ty)? as u64;
                        cases.push((tag, row.entry, r));
                    } else if has_tag && self.pattern_names_variant(&row.pat) {
                        return Err(Self::no_such_variant(name, &scrutinee_ty));
                    } else {
                        // A plain binding pattern is a catch-all too.
                        catch_all[r] = true;
                        if !default_claimed {
                            default_block = row.entry;
                            default_claimed = true;
                        }
                    }
                }
                PatternKind::Constructor { name, .. } | PatternKind::Struct { name, .. } => {
                    if let Some(tag) = self.pattern_variant_tag(&row.pat, &scrutinee_ty) {
                        cases.push((tag as u64, row.entry, r));
                    } else if has_tag {
                        return Err(Self::no_such_variant(name, &scrutinee_ty));
                    } else {
                        cases.push((i as u64, row.entry, r));
                    }
                }
                PatternKind::Literal(lit_expr) => {
                    if let ExprKind::Int(v, _) = &lit_expr.kind {
                        cases.push((*v as u64, row.entry, r));
                    } else if let ExprKind::Bool(b) = &lit_expr.kind {
                        cases.push((if *b { 1 } else { 0 }, row.entry, r));
                    } else if let ExprKind::Char(c) = &lit_expr.kind {
                        // A char switches on its code point, same as an int.
                        // Without this the arm fell through to the index case
                        // below and `match c { '&' => … }` compared the code
                        // point against 0, 1, 2 — so no arm ever matched and
                        // every char took the wildcard. `c == '&'` was fine,
                        // which is why this hid for so long (#693).
                        cases.push((u32::from(*c) as u64, row.entry, r));
                    } else {
                        cases.push((i as u64, row.entry, r));
                    }
                }
                PatternKind::TypePat { ty, .. } => {
                    let ty_name = &super::type_pat_name(ty);
                    if is_result_or_option {
                        // Result/Option match: ok arm = tag 0, err arm = tag 1,
                        // decided by the real ok/err type identities.
                        let tag = self.pattern_is_err_side(&row.pat, ty_name, &scrutinee_ty)? as u64;
                        cases.push((tag, row.entry, r));
                    } else {
                        cases.push((i as u64, row.entry, r));
                    }
                }
                _ => {
                    cases.push((i as u64, row.entry, r));
                }
            }
        }

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Switch {
            value: switch_val.clone(),
            cases: first_per_tag(cases.iter().map(|(t, b, _)| (*t, *b))),
            default: default_block,
        }));
        // The rows' catch-alls are what a failed test dispatches to last.
        // `redispatch(from)` is a block that routes the value over the rows
        // from `from` down the way the first switch did: the first of them
        // whose tag it has, else the first catch-all, else no arm.
        let mut result_ty = MirType::Void;
        let result_local = self.builder.alloc_temp(MirType::I64);
        // The checker's type of the scrutinee: a generic enum's payload
        // declared as a type parameter has only a placeholder in the
        // shared layout, so its type comes from here (#871).
        let scrutinee_raw = self.ctx.lookup_raw_type(scrutinee.id).map(std::borrow::Cow::into_owned);
        let mut redispatch_blocks: std::collections::HashMap<usize, BlockId> = std::collections::HashMap::new();
        let mut merged: Vec<(String, crate::LocalId, MirType)> = Vec::new();
        for (r, row) in rows.iter().enumerate() {
            let i = row.arm;
            let arm = &arms[i];
            let multi = alts_of_arm[i] > 1;
            let first_row_of_arm = r + 1 - rows[..=r].iter().rev().take_while(|x| x.arm == i).count();
            let last_row_of_arm = rows[r..].iter().take_while(|x| x.arm == i).count() + r - 1;
            self.builder.switch_to_block(row.entry);
            if multi && r == first_row_of_arm {
                merged.clear();
            }
            let snapshot = if multi { self.snapshot_bindable(&row.pat) } else { Vec::new() };

            // A literal, range or nested variant inside the row's variant
            // pattern: the field's value, its type, and what it has to match.
            // The switch only tested the outer tag.
            let field_tests = if has_tag {
                self.bind_enum_row(
                    &row.pat, &scrutinee_op, &scrutinee_ty, scrutinee_raw.as_ref(), is_niche,
                    is_result_or_option, &ok_payload_ty, &err_payload_ty,
                )?
            } else {
                Vec::new()
            };
            self.bind_enum_row_name(&row.pat, &scrutinee_op, &scrutinee_ty);

            // Where the value goes when this row turns out not to take it
            // after all: a literal in its variant pattern didn't match, or the
            // guard failed. The rows below it, in order: the first of them
            // whose pattern takes it. One switch can't say that, so the
            // failure dispatches again over what's left.
            //
            //   match s {
            //       Circle(r) if r > 10 => "big circle"
            //       Circle(0) => "dot"
            //       Circle(r) => "circle"
            //       Square(w) => "square"
            //   }
            //
            // Handing it to the next arm's body ran that arm whether its
            // pattern matched or not (#875). Handing it to the next catch-all
            // skipped `Circle(r)`, and with none below, native left the match
            // with no result at all. A literal in the pattern was never tested
            // at all, so `Circle(0)` took every circle (#1532).
            //
            // An or-pattern is one row per alternative (`Inner(Blue) |
            // Inner(Red)` shares a tag between them), so a test that fails in
            // one alternative tries the next. A failed guard skips the whole
            // arm: it was judged once, on the alternative that matched.
            let mut redispatch = |this: &mut Self, from: usize| -> BlockId {
                if let Some(b) = redispatch_blocks.get(&from) {
                    return *b;
                }
                let next_catch_all = (from..rows.len())
                    .find(|&j| catch_all[j])
                    .map(|j| rows[j].entry)
                    .unwrap_or(no_match);
                let rest = first_per_tag(
                    cases.iter().filter(|(_, _, j)| *j >= from).map(|(t, b, _)| (*t, *b)),
                );
                let here = this.builder.current_block();
                let block = this.builder.create_block();
                this.builder.switch_to_block(block);
                this.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Switch {
                    value: switch_val.clone(),
                    cases: rest,
                    default: next_catch_all,
                }));
                this.builder.switch_to_block(here);
                redispatch_blocks.insert(from, block);
                block
            };
            let test_fail = if field_tests.is_empty() { no_match } else { redispatch(self, r + 1) };

            for (value, ty, checked, pattern) in &field_tests {
                self.emit_subpattern_test(value, ty, checked.as_ref(), pattern, test_fail)?;
            }
            // Every tag on the way down has passed, so a nested variant's
            // fields are that variant's: bind them now.
            for (value, ty, checked, pattern) in &field_tests {
                match &pattern.kind {
                    PatternKind::Constructor { .. } | PatternKind::Struct { .. } | PatternKind::Tuple(_) => {
                        self.bind_pattern_fields(pattern, value.clone(), None, false, ty, checked.as_ref());
                    }
                    PatternKind::TypePat { .. } => {
                        self.bind_field_type_pattern(value, ty, checked.as_ref(), pattern);
                    }
                    _ => {}
                }
            }

            if multi {
                // Each alternative bound its own locals; the body reads one
                // set. Copy this alternative's into it and share the body.
                self.merge_alt_bindings(&snapshot, &mut merged);
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                    target: arm_blocks[i],
                }));
                if r != last_row_of_arm {
                    continue;
                }
                self.builder.switch_to_block(arm_blocks[i]);
                self.install_merged(&merged);
            }
            // After the nested bindings, so their copies are retained too.
            self.retain_pattern_copies(&arm.pattern, scrutinee, &scrutinee_ty);

            if let Some(guard_expr) = &arm.guard {
                let (guard_val, _) = self.lower_expr(guard_expr)?;
                let guard_fail_block = redispatch(self, last_row_of_arm + 1);
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
        // The local was allocated before any arm was lowered, so its type
        // started as a placeholder word. Now that the arms have reported one,
        // give it the real one: assigning an f64 into an `i64` local converts
        // rather than reinterprets, so a `match` used as an expression handed
        // back its float arms truncated — `match n { 1 => 2.5, _ => 0.0 }` was
        // 2 (#973). Nothing narrows it back, so `if/else` was right and `match`
        // was not.
        self.builder.set_local_type(result_local, result_ty.clone());
        Ok((MirOperand::Local(result_local), result_ty))
    }

    /// Read the payloads one row's variant pattern names out of the matched
    /// value, binding plain names and returning the fields that still need a
    /// test (a literal, a nested variant, a type test) for the caller to run.
    #[allow(clippy::too_many_arguments)]
    fn bind_enum_row(
        &mut self,
        pat: &Pattern,
        scrutinee_op: &MirOperand,
        scrutinee_ty: &MirType,
        scrutinee_raw: Option<&rask_types::Type>,
        is_niche: bool,
        is_result_or_option: bool,
        ok_payload_ty: &Option<MirType>,
        err_payload_ty: &Option<MirType>,
    ) -> Result<Vec<(MirOperand, MirType, Option<rask_types::Type>, Pattern)>, LoweringError> {
        use rask_ast::expr::PatternKind;
        let scrutinee_op = scrutinee_op.clone();
        let scrutinee_ty = scrutinee_ty.clone();
        let scrutinee_raw = scrutinee_raw.cloned();
        let mut field_tests: Vec<(MirOperand, MirType, Option<rask_types::Type>, Pattern)> = Vec::new();
        if let PatternKind::Constructor { name, fields } = &pat.kind {
            // Qualified variant names like "Tagged.With" — strip the
            // enum prefix so the layout lookup matches the variant's
            // bare name (same as the PatternKind::Struct path below).
            let variant_name = name.rsplit('.').next().unwrap_or(name);
            let payload_base = scrutinee_op.clone();
            // (mir type, absolute byte offset within the enum, field size)
            let variant_fields =
                self.variant_field_types(&scrutinee_ty, scrutinee_raw.as_ref(), variant_name);

            for (j, field_pat) in fields.iter().enumerate() {
                let tested = self.nested_needs_test(field_pat);
                let binding = match &field_pat.kind {
                    PatternKind::Ident(b) if !tested => Some(b),
                    _ => None,
                };
                if binding.is_some() || tested {
                    // For a user enum, pass the exact payload offset + size so codegen
                    // doesn't guess the variant (mixed variants share field indices).
                    let (field_ty, field_loc) = if let Some(ref vf) = variant_fields {
                        vf.get(j)
                            .map(|(ty, off, sz)| (ty.clone(), Some((*off, *sz))))
                            .unwrap_or_else(|| (
                                crate::fallback::unknown_type(
                                    "lower/match_lower:enum_variant_field"),
                                None,
                            ))
                    } else {
                        let ty = match name.as_str() {
                            "Err" => err_payload_ty.clone(),
                            _ => ok_payload_ty.clone(),
                        }
                        .unwrap_or_else(|| crate::fallback::unknown_type(
                            "lower/match_lower:ok_err_binding"));
                        (ty, None)
                    };
                    let payload_local = match binding {
                        Some(b) => self.builder.alloc_local(b.clone(), field_ty.clone()),
                        None => self.builder.alloc_temp(field_ty.clone()),
                    };
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
                    let Some(binding) = binding else {
                        let field_checked =
                            self.ctx.variant_payload_type(scrutinee_raw.as_ref(), variant_name, j);
                        field_tests.push((MirOperand::Local(payload_local), field_ty, field_checked, field_pat.clone()));
                        continue;
                    };
                    self.meta_mut(binding).checked_type =
                        self.ctx.variant_payload_type(scrutinee_raw.as_ref(), variant_name, j);
                    self.locals.insert(binding.clone(), (payload_local, field_ty));
                }
            }
        } else if let PatternKind::Struct { name, fields, .. } = &pat.kind {
            let variant_name = name.rsplit('.').next().unwrap_or(name);
            let enum_layout_id = match &scrutinee_ty {
                MirType::Enum(crate::types::EnumLayoutId { id, .. }) => Some(*id),
                _ => None,
            };
            let payload_base = scrutinee_op.clone();
            if let Some(idx) = enum_layout_id {
                if let Some(layout) = self.ctx.enum_layouts.get(idx as usize) {
                    if let Some(variant) = layout.variants.iter().find(|v| v.name == variant_name) {
                        for (field_name, field_pat) in fields {
                            let tested = self.nested_needs_test(field_pat);
                            let binding = match &field_pat.kind {
                                PatternKind::Ident(b) if !tested => Some(b),
                                _ => None,
                            };
                            if binding.is_some() || tested {
                                if let Some((field_idx, field_layout)) = variant.fields.iter()
                                    .enumerate()
                                    .find(|(_, f)| f.name == *field_name)
                                {
                                    let field_ty = self.ctx
                                        .variant_payload_mir(scrutinee_raw.as_ref(), variant_name, field_idx)
                                        .unwrap_or_else(|| self.ctx.type_to_mir(&field_layout.ty));
                                    let payload_local = match binding {
                                        Some(b) => self.builder.alloc_local(b.clone(), field_ty.clone()),
                                        None => self.builder.alloc_temp(field_ty.clone()),
                                    };
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
                                    let Some(binding) = binding else {
                                        let field_checked = self.ctx.variant_payload_type(
                                            scrutinee_raw.as_ref(), variant_name, field_idx,
                                        );
                                        field_tests.push((MirOperand::Local(payload_local), field_ty, field_checked, field_pat.clone()));
                                        continue;
                                    };
                                    self.meta_mut(binding).checked_type = self.ctx.variant_payload_type(
                                        scrutinee_raw.as_ref(), variant_name, field_idx,
                                    );
                                    self.locals.insert(binding.clone(), (payload_local, field_ty));
                                }
                            }
                        }
                    }
                }
            }
        // TypePat { ty, binding } — `T as name` in a Result/Option match.
        // The switch case routing is already correct (arm index → tag).
        // Here we emit the payload extraction for the binding.
        } else if let PatternKind::TypePat { ty, binding } = &pat.kind {
            let ty_name = &super::type_pat_name(ty);
            if let Some(binding_name) = binding {
                if is_result_or_option {
                    // Bind the matching side's payload — the side is
                    // decided by type identity, same as the tag routing.
                    let payload_ty = if self.pattern_is_err_side(pat, ty_name, &scrutinee_ty)? {
                        err_payload_ty.clone()
                    } else {
                        ok_payload_ty.clone()
                    }
                    .unwrap_or_else(|| crate::fallback::unknown_type(
                        "lower/match_lower:typepat_payload"));
                    let payload_local = self.builder.alloc_local(
                        binding_name.clone(), payload_ty.clone(),
                    );
                    let rvalue = self.payload_field(
                        scrutinee_op.clone(), &scrutinee_ty, &payload_ty,
                    );
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                        dst: payload_local,
                        rvalue,
                    }));
                    self.locals.insert(binding_name.clone(), (payload_local, payload_ty));
                }
            }
        }
        Ok(field_tests)
    }

    /// A plain name as the whole pattern is a catch-all that also *names* the
    /// value.
    fn bind_enum_row_name(&mut self, pat: &Pattern, scrutinee_op: &MirOperand, scrutinee_ty: &MirType) {
        use rask_ast::expr::PatternKind;
        // A plain binding pattern is a catch-all that also *names* the
        // value — `match n { x if x < 0 => … }`. The switch above routes it
        // to the default and nothing bound `x`, so the guard referred to a
        // variable that didn't exist:
        //
        //     error: MIR lowering 'bucket': unresolved variable `x`
        //
        // The if-chain path taken by a match with range patterns has always
        // bound it; this one hadn't. Before the guard, because the guard is
        // usually what reads it (#875).
        if let PatternKind::Ident(name) = &pat.kind {
            let names_a_variant = !self.pattern_binds(pat);
            if !names_a_variant {
                let bind_local =
                    self.builder.alloc_local(name.clone(), scrutinee_ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: bind_local,
                    rvalue: MirRValue::Use(scrutinee_op.clone()),
                }));
                self.locals.insert(name.clone(), (bind_local, scrutinee_ty.clone()));
            }
        }
    }

    /// The names `pat` can bind, with the local each held before the pattern
    /// bound it, so `merge_alt_bindings` can tell the names this alternative
    /// bound from variant names and outer variables.
    fn snapshot_bindable(&self, pat: &Pattern) -> Vec<(String, Option<crate::LocalId>)> {
        let mut seen = std::collections::HashSet::new();
        pat.bound_names()
            .into_iter()
            .filter(|n| seen.insert(*n))
            .map(|n| (n.to_string(), self.locals.get(n).map(|(l, _)| *l)))
            .collect()
    }

    /// Copy the locals one alternative of an or-pattern bound into the locals
    /// the arm's body reads, creating those on the first alternative.
    fn merge_alt_bindings(
        &mut self,
        snapshot: &[(String, Option<crate::LocalId>)],
        merged: &mut Vec<(String, crate::LocalId, MirType)>,
    ) {
        for (name, before) in snapshot {
            let Some((local, ty)) = self.locals.get(name).cloned() else { continue };
            if Some(local) == *before {
                continue;
            }
            let into = match merged.iter().find(|(n, _, _)| n == name) {
                Some((_, into, _)) => *into,
                None => {
                    let into = self.builder.alloc_local(name.clone(), ty.clone());
                    merged.push((name.clone(), into, ty));
                    into
                }
            };
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: into,
                rvalue: MirRValue::Use(MirOperand::Local(local)),
            }));
        }
    }

    /// Make the merged locals the ones the arm's names resolve to.
    fn install_merged(&mut self, merged: &[(String, crate::LocalId, MirType)]) {
        for (name, local, ty) in merged {
            self.locals.insert(name.clone(), (*local, ty.clone()));
        }
    }

    /// `scrutinee is A | B(x)` as a bool: one test per alternative, the first
    /// that passes binds its names (into one set of locals, as an or-pattern
    /// arm's alternatives do) and answers true. On the failed path a string
    /// binding is emptied, as `lower_is_pattern_binding` does, so releasing it
    /// after an `&&` chain is harmless.
    pub(super) fn lower_is_or_pattern(
        &mut self,
        scrutinee: &Expr,
        val: MirOperand,
        val_ty: MirType,
        pattern: &Pattern,
    ) -> Result<TypedOperand, LoweringError> {
        let niche = self.option_niche(scrutinee, &val_ty);
        let is_niche = niche.is_some();
        let payload_ty = self.payload_type_of_niche(scrutinee, &val_ty, is_niche);
        let result = self.builder.alloc_temp(MirType::Bool);
        let merge_block = self.builder.create_block();
        let mut merged: Vec<(String, crate::LocalId, MirType)> = Vec::new();
        for alt in expand_or(pattern) {
            let test = self.emit_is_test(scrutinee, &val, &val_ty, niche, &alt)?;
            let bind_block = self.builder.create_block();
            let next = self.builder.create_block();
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                cond: MirOperand::Local(test.matches),
                then_block: bind_block,
                else_block: next,
            }));
            self.builder.switch_to_block(bind_block);
            let snapshot = self.snapshot_bindable(&alt);
            self.bind_tested_pattern(
                &test, &alt, scrutinee, val.clone(), payload_ty.clone(), is_niche, &val_ty,
            );
            self.merge_alt_bindings(&snapshot, &mut merged);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: result,
                rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Bool(true))),
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));
            self.builder.switch_to_block(next);
        }
        for (_, local, ty) in &merged {
            if *ty == MirType::Buffer {
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: *local,
                    rvalue: MirRValue::Use(MirOperand::Constant(MirConst::String(String::new()))),
                }));
            }
        }
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: result,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Bool(false))),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));
        self.builder.switch_to_block(merge_block);
        self.install_merged(&merged);
        Ok((MirOperand::Local(result), MirType::Bool))
    }

    /// Lower match on strings: emit chain of string_eq comparisons.
    pub(super) fn lower_string_match(
        &mut self,
        scrutinee_op: MirOperand,
        arms: &[rask_ast::expr::MatchArm],
    ) -> Result<TypedOperand, LoweringError> {
        use rask_ast::expr::PatternKind;

        let merge_block = self.builder.create_block();
        let no_match = self.no_arm_matched_block();
        let result_local = self.builder.alloc_temp(MirType::I64);
        let mut result_ty = MirType::Void;

        // Arms are tried in order, each a test and then its guard; either
        // failing moves on to the next arm. A guard used to be dropped here,
        // so `x if x.len() > 3 => "long"` took every string.
        for (i, arm) in arms.iter().enumerate() {
            let next_arm = if i + 1 < arms.len() { self.builder.create_block() } else { no_match };
            let catch_all = matches!(&arm.pattern.kind, PatternKind::Wildcard | PatternKind::Ident(_));
            if !catch_all {
                let literals: Vec<String> = flatten_pattern_alternatives(&arm.pattern)
                    .into_iter()
                    .filter_map(|p| match &p.kind {
                        PatternKind::Literal(lit) => match &lit.kind {
                            ExprKind::String(s) => Some(s.clone()),
                            _ => None,
                        },
                        _ => None,
                    })
                    .collect();
                let body = self.builder.create_block();
                for lit in literals {
                    let eq_result = self.builder.alloc_temp(MirType::Bool);
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: Some(eq_result),
                        func: FunctionRef::borrowing("string_eq"),
                        args: vec![scrutinee_op.clone(), MirOperand::Constant(MirConst::String(lit))],
                    }));
                    let next_test = self.builder.create_block();
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                        cond: MirOperand::Local(eq_result),
                        then_block: body,
                        else_block: next_test,
                    }));
                    self.builder.switch_to_block(next_test);
                }
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: next_arm }));
                self.builder.switch_to_block(body);
            }

            if let PatternKind::Ident(name) = &arm.pattern.kind {
                let bind_local = self.builder.alloc_local(name.clone(), MirType::Buffer);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: bind_local,
                    rvalue: MirRValue::Use(scrutinee_op.clone()),
                }));
                self.locals.insert(name.clone(), (bind_local, MirType::Buffer));
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
            if next_arm != no_match {
                self.builder.switch_to_block(next_arm);
            }
        }

        self.builder.switch_to_block(merge_block);
        // The local was allocated before any arm was lowered, so its type
        // started as a placeholder word. Now that the arms have reported one,
        // give it the real one: assigning an f64 into an `i64` local converts
        // rather than reinterprets, so a `match` used as an expression handed
        // back its float arms truncated — `match n { 1 => 2.5, _ => 0.0 }` was
        // 2 (#973). Nothing narrows it back, so `if/else` was right and `match`
        // was not.
        self.builder.set_local_type(result_local, result_ty.clone());
        Ok((MirOperand::Local(result_local), result_ty))
    }

    /// Lower match with tuple patterns.
    pub(super) fn lower_tuple_match(
        &mut self,
        scrutinee: &Expr,
        arms: &[rask_ast::expr::MatchArm],
    ) -> Result<TypedOperand, LoweringError> {
        use rask_ast::expr::{Pattern, PatternKind};

        // An arm that is a plain name binds the whole tuple, so the tuple has
        // to exist as a value; a literal scrutinee is otherwise read element by
        // element and never built.
        let binds_whole = arms.iter().any(|a| {
            flatten_pattern_alternatives(&a.pattern)
                .iter()
                .any(|p| matches!(&p.kind, PatternKind::Ident(_)) && self.pattern_binds(p))
        });
        let mut whole: Option<(MirOperand, MirType)> = None;
        let tuple_elems: Vec<(MirOperand, MirType)> = if let (ExprKind::Tuple(elems), false) = (&scrutinee.kind, binds_whole) {
            let mut result = Vec::new();
            for elem in elems {
                result.push(self.lower_expr(elem)?);
            }
            result
        } else {
            // Non-literal scrutinee — if its type is a tuple, project each
            // field; otherwise treat it as a single-element vec.
            let (op, ty) = self.lower_expr(scrutinee)?;
            whole = Some((op.clone(), ty.clone()));
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

        // The checker's type of each element, for a generic enum's payload
        // (`variant_field_types`).
        let checked_elems: Vec<rask_types::Type> = match self.ctx.lookup_raw_type(scrutinee.id).as_deref() {
            Some(rask_types::Type::Tuple(elems)) => elems.clone(),
            Some(single) if tuple_elems.len() == 1 => vec![single.clone()],
            _ => Vec::new(),
        };

        let merge_block = self.builder.create_block();
        let result_local = self.builder.alloc_temp(MirType::I64);
        let mut result_ty = MirType::Void;

        // One row per alternative of each arm's or-pattern, `(A | B, C)` being
        // `(A, C)` and `(B, C)`. A row whose test fails tries the next row;
        // the rows of one arm share its body.
        let mut rows: Vec<(usize, Pattern)> = Vec::new();
        for (i, arm) in arms.iter().enumerate() {
            for alt in expand_or(&arm.pattern) {
                rows.push((i, alt));
            }
        }
        let arm_test_blocks: Vec<BlockId> = rows.iter().map(|_| self.builder.create_block()).collect();
        let arm_body_blocks: Vec<BlockId> = arms.iter().map(|_| self.builder.create_block()).collect();
        let fallthrough = self.no_arm_matched_block();
        let mut merged: Vec<(String, crate::LocalId, MirType)> = Vec::new();

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: arm_test_blocks[0] }));

        for (r, (i, row_pat)) in rows.iter().enumerate() {
            let i = *i;
            let arm = &arms[i];
            let multi = rows.iter().filter(|(a, _)| *a == i).count() > 1;
            let first_row_of_arm = rows.iter().position(|(a, _)| *a == i).unwrap_or(r);
            let last_row_of_arm = rows.iter().rposition(|(a, _)| *a == i).unwrap_or(r);
            self.builder.switch_to_block(arm_test_blocks[r]);
            if multi && r == first_row_of_arm {
                merged.clear();
            }
            let snapshot = if multi { self.snapshot_bindable(row_pat) } else { Vec::new() };
            let next_arm = if r + 1 < arm_test_blocks.len() {
                arm_test_blocks[r + 1]
            } else {
                fallthrough
            };
            // A failed guard skips the whole arm: it was judged once, on the
            // first alternative that matched.
            let after_arm = if last_row_of_arm + 1 < arm_test_blocks.len() {
                arm_test_blocks[last_row_of_arm + 1]
            } else {
                fallthrough
            };

            // Each element of the tuple is matched by the same sub-pattern
            // machinery a variant's field uses: a variant name or constructor
            // tests the tag and what's inside, a literal or range compares, a
            // struct or tuple pattern reads its parts, a type pattern tests the
            // element's type, and a plain name binds it. A shape that machinery
            // has no case for is an error, never an arm that matches anyway.
            let names_whole = matches!(&row_pat.kind, PatternKind::Ident(_)) && self.pattern_binds(row_pat);
            let sub_patterns: Vec<Pattern> = match &row_pat.kind {
                PatternKind::Tuple(pats) => pats.clone(),
                PatternKind::Wildcard => vec![],
                // `t => …` takes the whole tuple.
                PatternKind::Ident(_) if names_whole && whole.is_some() => vec![],
                // A single value matched against one pattern.
                _ if tuple_elems.len() == 1 => vec![row_pat.clone()],
                _ => {
                    return Err(LoweringError::InvalidConstruct(format!(
                        "a `match` on {} values has an arm that isn't a tuple pattern: `{:?}`",
                        tuple_elems.len(),
                        row_pat.kind,
                    )));
                }
            };
            if sub_patterns.len() != tuple_elems.len()
                && !matches!(&row_pat.kind, PatternKind::Wildcard)
                && !(names_whole && whole.is_some())
            {
                return Err(LoweringError::InvalidConstruct(format!(
                    "a tuple pattern names {} parts, the matched tuple has {}",
                    sub_patterns.len(),
                    tuple_elems.len(),
                )));
            }

            for (j, pat) in sub_patterns.iter().enumerate() {
                let (elem_op, elem_ty) = tuple_elems[j].clone();
                self.emit_subpattern_test(&elem_op, &elem_ty, checked_elems.get(j), pat, next_arm)?;
            }
            // Every test has passed, so the parts are the right variants':
            // bind them.
            for (j, pat) in sub_patterns.iter().enumerate() {
                let (elem_op, elem_ty) = tuple_elems[j].clone();
                self.bind_field_value(elem_op, elem_ty, checked_elems.get(j), pat);
            }
            if let (true, Some((whole_op, whole_ty))) = (names_whole, &whole) {
                let whole_checked = self.ctx.lookup_raw_type(scrutinee.id).map(std::borrow::Cow::into_owned);
                self.bind_field_value(whole_op.clone(), whole_ty.clone(), whole_checked.as_ref(), row_pat);
            }

            // A literal scrutinee is read element by element, so each element
            // is its own source.
            match (&scrutinee.kind, &row_pat.kind) {
                (ExprKind::Tuple(elem_exprs), PatternKind::Tuple(pats)) => {
                    for ((pat, elem_expr), (_, elem_ty)) in
                        pats.iter().zip(elem_exprs.iter()).zip(tuple_elems.iter())
                    {
                        self.retain_pattern_copies(pat, elem_expr, elem_ty);
                    }
                }
                _ => {
                    let whole = MirType::Tuple(tuple_elems.iter().map(|(_, t)| t.clone()).collect());
                    self.retain_pattern_copies(row_pat, scrutinee, &whole);
                }
            }

            if multi {
                // Each alternative bound its own locals; the body reads one
                // set. Copy this alternative's into it and share the body.
                self.merge_alt_bindings(&snapshot, &mut merged);
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                    target: arm_body_blocks[i],
                }));
                if r != last_row_of_arm {
                    continue;
                }
                self.builder.switch_to_block(arm_body_blocks[i]);
                self.install_merged(&merged);
            }

            if let Some(guard_expr) = &arm.guard {
                let (guard_val, _) = self.lower_expr(guard_expr)?;
                let guard_pass = self.builder.create_block();
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: guard_val,
                    then_block: guard_pass,
                    else_block: after_arm,
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
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));
            }
        }

        self.builder.switch_to_block(merge_block);
        // The local was allocated before any arm was lowered, so its type
        // started as a placeholder word. Now that the arms have reported one,
        // give it the real one: assigning an f64 into an `i64` local converts
        // rather than reinterprets, so a `match` used as an expression handed
        // back its float arms truncated — `match n { 1 => 2.5, _ => 0.0 }` was
        // 2 (#973). Nothing narrows it back, so `if/else` was right and `match`
        // was not.
        self.builder.set_local_type(result_local, result_ty.clone());
        Ok((MirOperand::Local(result_local), result_ty))
    }

    /// Chain lowering for scalar (int/char) matches that include range patterns.
    /// Each arm becomes a boolean test branching to the body or the next arm.
    /// `match p { Point(x: 0, y) => …, Point(x, y) => … }`.
    ///
    /// Arms are tried in order. An arm matches when every field pattern that
    /// tests a value agrees; field patterns that only bind always agree, and are
    /// what the body reads. Modelled on `lower_scalar_chain_match` — the shape is
    /// the same, the condition is per-field instead of on the scrutinee (#307).
    pub(super) fn lower_struct_match(
        &mut self,
        scrutinee: &Expr,
        scrutinee_op: MirOperand,
        scrutinee_ty: MirType,
        arms: &[rask_ast::expr::MatchArm],
    ) -> Result<TypedOperand, LoweringError> {
        use rask_ast::expr::{Pattern, PatternKind};

        let merge_block = self.builder.create_block();
        let arm_body_blocks: Vec<BlockId> =
            arms.iter().map(|_| self.builder.create_block()).collect();
        let result_local = self.builder.alloc_temp(MirType::I64);
        let mut result_ty = MirType::Void;

        let no_match = self.no_arm_matched_block();
        for (i, arm) in arms.iter().enumerate() {
            let next_arm = if i + 1 < arms.len() {
                self.builder.create_block()
            } else {
                no_match
            };

            // Every field the pattern names is read once, whether it's tested or
            // bound — a test needs the value and so does the body.
            let mut reads: Vec<FieldRead> = Vec::new();
            if let PatternKind::Struct { fields, .. } = &arm.pattern.kind {
                for (field_name, field_pat) in fields {
                    let Some((field_idx, field_layout)) =
                        self.struct_field(&scrutinee_ty, field_name)
                    else {
                        continue;
                    };
                    let field_ty = self.ctx.type_to_mir(&field_layout.ty);
                    // Named for the binding when there is one, so debug info and
                    // the local's name agree.
                    let local_name = match &field_pat.kind {
                        PatternKind::Ident(binding) if self.pattern_binds(field_pat) => binding.clone(),
                        _ => format!("__match_{}", field_name),
                    };
                    let local = self.builder.alloc_local(local_name, field_ty.clone());
                    let fl = field_layout.clone();
                    self.read_layout_field(local, scrutinee_op.clone(), field_idx as u32, &fl, &field_ty, fl.offset);
                    reads.push(FieldRead { local, ty: field_ty, checked: fl.ty.clone(), pattern: field_pat });
                }
            }

            // A pattern that only binds is a catch-all: nothing to test.
            self.emit_field_tests(&reads, next_arm)?;
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                target: arm_body_blocks[i],
            }));

            self.builder.switch_to_block(arm_body_blocks[i]);

            // The bindings the body reads.
            self.bind_field_reads(&reads);
            // A whole-value catch-all (`p => …`) binds the struct itself.
            if let PatternKind::Ident(name) = &arm.pattern.kind {
                let bind_local = self.builder.alloc_local(name.clone(), scrutinee_ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: bind_local,
                    rvalue: MirRValue::Use(scrutinee_op.clone()),
                }));
                self.locals
                    .insert(name.clone(), (bind_local, scrutinee_ty.clone()));
            }

            self.retain_pattern_copies(&arm.pattern, scrutinee, &scrutinee_ty);

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

            if next_arm != no_match {
                self.builder.switch_to_block(next_arm);
            }
        }

        self.builder.switch_to_block(merge_block);
        // The local was allocated before any arm was lowered, so its type
        // started as a placeholder word. Now that the arms have reported one,
        // give it the real one: assigning an f64 into an `i64` local converts
        // rather than reinterprets, so a `match` used as an expression handed
        // back its float arms truncated — `match n { 1 => 2.5, _ => 0.0 }` was
        // 2 (#973). Nothing narrows it back, so `if/else` was right and `match`
        // was not.
        self.builder.set_local_type(result_local, result_ty.clone());
        Ok((MirOperand::Local(result_local), result_ty))
    }

    /// Test each field of a struct pattern that can fail, in order, branching
    /// to `fail` at the first that does. Carries on in the block where they
    /// all passed. A field that only binds, or is `_`, has nothing to test.
    fn emit_field_tests(&mut self, reads: &[FieldRead], fail: BlockId) -> Result<(), LoweringError> {
        use rask_ast::expr::PatternKind;
        for read in reads {
            let pattern = read.pattern;
            let value = MirOperand::Local(read.local);
            if self.field_pattern_tests_type(pattern) {
                self.emit_field_type_test(&value, &read.ty, Some(&read.checked), pattern, fail)?;
            } else if !matches!(&pattern.kind, PatternKind::Ident(_) | PatternKind::Wildcard) {
                let pass = self.builder.create_block();
                self.emit_pattern_test(&value, pattern, pass, fail)?;
                self.builder.switch_to_block(pass);
            }
        }
        Ok(())
    }

    /// The names a struct pattern's fields bind, which the arm body reads.
    fn bind_field_reads(&mut self, reads: &[FieldRead]) {
        use rask_ast::expr::PatternKind;
        for read in reads {
            match &read.pattern.kind {
                PatternKind::Ident(binding) if self.pattern_binds(read.pattern) => {
                    self.meta_mut(binding.as_str()).checked_type = Some(read.checked.clone());
                    self.locals.insert(binding.clone(), (read.local, read.ty.clone()));
                }
                PatternKind::TypePat { .. } => {
                    self.bind_field_type_pattern(
                        &MirOperand::Local(read.local), &read.ty, Some(&read.checked), read.pattern,
                    );
                }
                _ => {}
            }
        }
    }

    /// `p is Point(x: 0, y)` on a plain struct, as a bool: read each field the
    /// pattern names, test the ones that can fail, bind the rest. The
    /// bindings stay in scope past the test — an `&&` chain reads them — so
    /// on the failed path a string binding is emptied, the way
    /// `lower_is_pattern_binding` does, and releasing it is harmless.
    pub(super) fn lower_struct_is(
        &mut self,
        scrutinee: &Expr,
        val: MirOperand,
        val_ty: MirType,
        pattern: &rask_ast::expr::Pattern,
    ) -> Result<TypedOperand, LoweringError> {
        use rask_ast::expr::{Pattern, PatternKind};
        let PatternKind::Struct { fields, .. } = &pattern.kind else {
            unreachable!("lower_struct_is takes a struct pattern");
        };
        let result = self.builder.alloc_temp(MirType::Bool);
        let mut reads: Vec<FieldRead> = Vec::new();
        for (field_name, field_pat) in fields {
            let Some((field_idx, field_layout)) = self.struct_field(&val_ty, field_name) else {
                continue;
            };
            let field_ty = self.ctx.type_to_mir(&field_layout.ty);
            let local_name = match &field_pat.kind {
                PatternKind::Ident(binding) if self.pattern_binds(field_pat) => binding.clone(),
                _ => format!("__is_{}", field_name),
            };
            let local = self.builder.alloc_local(local_name, field_ty.clone());
            let fl = field_layout.clone();
            self.read_layout_field(local, val.clone(), field_idx as u32, &fl, &field_ty, fl.offset);
            reads.push(FieldRead { local, ty: field_ty, checked: fl.ty.clone(), pattern: field_pat });
        }

        let pass_block = self.builder.create_block();
        let fail_block = self.builder.create_block();
        let merge_block = self.builder.create_block();
        self.emit_field_tests(&reads, fail_block)?;
        self.bind_field_reads(&reads);
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: pass_block }));

        self.builder.switch_to_block(pass_block);
        self.retain_pattern_copies(pattern, scrutinee, &val_ty);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: result,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(1))),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

        self.builder.switch_to_block(fail_block);
        for read in &reads {
            if matches!(&read.pattern.kind, PatternKind::Ident(_)) && read.ty == MirType::Buffer {
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: read.local,
                    rvalue: MirRValue::Use(MirOperand::Constant(MirConst::String(String::new()))),
                }));
            }
        }
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: result,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

        self.builder.switch_to_block(merge_block);
        Ok((MirOperand::Local(result), MirType::Bool))
    }

    /// A struct field's index and layout by name.
    pub(super) fn struct_field(
        &self,
        ty: &MirType,
        field_name: &str,
    ) -> Option<(usize, rask_layout::FieldLayout)> {
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
        use rask_ast::expr::PatternKind;

        let merge_block = self.builder.create_block();
        let arm_body_blocks: Vec<BlockId> = arms.iter().map(|_| self.builder.create_block()).collect();
        let result_local = self.builder.alloc_temp(MirType::I64);
        let mut result_ty = MirType::Void;

        let no_match = self.no_arm_matched_block();
        for (i, arm) in arms.iter().enumerate() {
            let next_arm = if i + 1 < arms.len() {
                self.builder.create_block()
            } else {
                no_match
            };

            // Catch-all patterns jump straight to the body.
            let is_catch_all = matches!(
                &arm.pattern.kind,
                PatternKind::Wildcard
                    | PatternKind::Ident(_)
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
            if let PatternKind::Ident(name) = &arm.pattern.kind {
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

            if next_arm != no_match {
                self.builder.switch_to_block(next_arm);
            }
        }

        self.builder.switch_to_block(merge_block);
        // The local was allocated before any arm was lowered, so its type
        // started as a placeholder word. Now that the arms have reported one,
        // give it the real one: assigning an f64 into an `i64` local converts
        // rather than reinterprets, so a `match` used as an expression handed
        // back its float arms truncated — `match n { 1 => 2.5, _ => 0.0 }` was
        // 2 (#973). Nothing narrows it back, so `if/else` was right and `match`
        // was not.
        self.builder.set_local_type(result_local, result_ty.clone());
        Ok((MirOperand::Local(result_local), result_ty))
    }

    /// Emit a boolean test for a non-Or pattern and branch accordingly.
    pub(super) fn emit_pattern_test(
        &mut self,
        scrutinee_op: &MirOperand,
        pattern: &rask_ast::expr::Pattern,
        pass_block: BlockId,
        fail_block: BlockId,
    ) -> Result<(), LoweringError> {
        use rask_ast::expr::PatternKind;

        let cond_local = self.builder.alloc_temp(MirType::Bool);
        match &pattern.kind {
            PatternKind::Literal(lit_expr) => {
                let is_equal = self.emit_literal_eq(scrutinee_op, lit_expr)?;
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: cond_local,
                    rvalue: MirRValue::Use(MirOperand::Local(is_equal)),
                }));
            }
            PatternKind::Range { start, end } => {
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
            PatternKind::Wildcard | PatternKind::Ident(_) => {
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

    /// `value == literal` as a bool local, for a literal in a pattern. A string
    /// literal compares by content: `==` on two buffers compares their
    /// addresses, so `Has("ab")` never matched `Has("ab")` held in a variant.
    pub(super) fn emit_literal_eq(
        &mut self,
        value: &MirOperand,
        lit: &Expr,
    ) -> Result<crate::LocalId, LoweringError> {
        let (lit_op, _) = self.lower_expr(lit)?;
        let out = self.builder.alloc_temp(MirType::Bool);
        if matches!(&lit.kind, ExprKind::String(_)) {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(out),
                func: FunctionRef::borrowing("string_eq"),
                args: vec![value.clone(), lit_op],
            }));
        } else {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: out,
                rvalue: MirRValue::BinaryOp { op: crate::BinOp::Eq, left: value.clone(), right: lit_op },
            }));
        }
        Ok(out)
    }

    /// A variant pattern the checker resolved that the scrutinee's layout
    /// doesn't have: a lowering bug, said as one rather than switched on a
    /// made-up tag.
    fn no_such_variant(name: &str, scrutinee_ty: &MirType) -> LoweringError {
        LoweringError::InvalidConstruct(format!(
            "pattern `{name}` names no variant of the matched value's type `{scrutinee_ty:?}`"
        ))
    }

    /// Give each Copy value a pattern bound out of borrowed storage
    /// references of its own. Called once the pattern's names are bound.
    ///
    /// `match v[0] { Slot.Full(l) => … }` copies `l` out of the slot, the same
    /// as `let l = …` does, so it is retained the same way
    /// (`retain_bound_copy`): otherwise `v[0] = Slot.Empty` in the arm freed
    /// the strings `l` was reading.
    pub(super) fn retain_pattern_copies(
        &mut self,
        pattern: &rask_ast::expr::Pattern,
        scrutinee: &Expr,
        scrutinee_ty: &MirType,
    ) {
        if !self.reads_borrowed_storage(scrutinee) {
            return;
        }
        let Some(ty) = self.ctx.lookup_raw_type(scrutinee.id).map(std::borrow::Cow::into_owned) else { return };
        let mut bound = Vec::new();
        self.pattern_binding_types(pattern, scrutinee_ty, &ty, &mut bound);
        for (name, ty) in bound {
            if let Some((local, _)) = self.locals.get(&name).cloned() {
                self.retain_bound_copy(local, &ty);
            }
        }
    }

    /// The checker's type for each name `pattern` binds against a value of
    /// type `ty` (`mir_ty` as lowered). A binding the walk can't place is
    /// left out, which costs a retain and never adds one.
    fn pattern_binding_types(
        &self,
        pattern: &rask_ast::expr::Pattern,
        mir_ty: &MirType,
        ty: &rask_types::Type,
        out: &mut Vec<(String, rask_types::Type)>,
    ) {
        use rask_ast::expr::PatternKind;
        match &pattern.kind {
            PatternKind::Ident(name) => {
                if !self.pattern_names_variant(pattern) {
                    out.push((name.clone(), ty.clone()));
                }
            }
            PatternKind::Constructor { name, fields } => {
                let Some((_, payload)) = self.enum_variant_payload(ty, name) else { return };
                for (pat, field_ty) in fields.iter().zip(payload.iter()) {
                    let field_mir = self.ctx.type_to_mir(field_ty);
                    self.pattern_binding_types(pat, &field_mir, field_ty, out);
                }
            }
            PatternKind::Struct { name, fields, .. } => {
                let named: Vec<(String, rask_types::Type)> = match self.enum_variant_payload(ty, name) {
                    Some((Some(names), payload)) => names.into_iter().zip(payload).collect(),
                    Some((None, _)) => return,
                    None => match self.struct_fields_of(ty) {
                        Some(fields) => fields,
                        None => return,
                    },
                };
                for (field_name, pat) in fields {
                    if let Some((_, field_ty)) = named.iter().find(|(n, _)| n == field_name) {
                        let field_mir = self.ctx.type_to_mir(field_ty);
                        self.pattern_binding_types(pat, &field_mir, field_ty, out);
                    }
                }
            }
            PatternKind::Tuple(pats) => {
                let rask_types::Type::Tuple(elems) = ty else { return };
                let mir_elems = match mir_ty {
                    MirType::Tuple(m) => m.clone(),
                    _ => elems.iter().map(|e| self.ctx.type_to_mir(e)).collect(),
                };
                for ((pat, elem), elem_mir) in pats.iter().zip(elems.iter()).zip(mir_elems.iter()) {
                    self.pattern_binding_types(pat, elem_mir, elem, out);
                }
            }
            PatternKind::TypePat { ty: written, binding: Some(name) } => {
                let ty_name = super::type_pat_name(written);
                // An error variant's or a union member's own payload is bound,
                // not the side it sits in.
                if self.deep_result_path(pattern, mir_ty).is_some() {
                    return;
                }
                let rask_types::Type::Result { ok, err } = ty else { return };
                let Ok(err_side) = self.pattern_is_err_side(pattern, &ty_name, mir_ty) else { return };
                let side = if err_side { err } else { ok };
                // On a flat `T? or E`, `T as v` binds the `T` inside the
                // success side's option. Only a written `T?` takes the option.
                let side = match side.as_option() {
                    Some(inner) if !matches!(written, TypeExpr::Optional(_)) => inner,
                    _ => side.as_ref(),
                };
                out.push((name.clone(), side.clone()));
            }
            PatternKind::Or(alts) => {
                if let Some(first) = alts.first() {
                    self.pattern_binding_types(first, mir_ty, ty, out);
                }
            }
            _ => {}
        }
    }

    /// The payload types of variant `variant` of the enum `ty`, instantiated,
    /// with the field names when the variant is struct-shaped.
    fn enum_variant_payload(
        &self,
        ty: &rask_types::Type,
        variant: &str,
    ) -> Option<(Option<Vec<String>>, Vec<rask_types::Type>)> {
        let (id, args) = match ty {
            rask_types::Type::Named(id) => (*id, &[][..]),
            rask_types::Type::Generic { base, args } => (*base, args.as_slice()),
            _ => return None,
        };
        let table = self.ctx.type_defs;
        let rask_types::TypeDef::Enum { type_params, variants, .. } = table.get(id)? else {
            return None;
        };
        let bare = variant.rsplit('.').next().unwrap_or(variant);
        let (_, data) = variants.iter().find(|(v, _)| v == bare)?;
        let subst = rask_check::TypeTable::generic_field_subst(type_params, args);
        let payload = data
            .iter()
            .map(|t| rask_check::TypeTable::substitute_generic_field(t, &subst))
            .collect();
        let names = table
            .struct_variant_fields(&format!("{}.{}", table.type_name(id), bare))
            .map(|fields| fields.into_iter().map(|(n, _)| n).collect());
        Some((names, payload))
    }

    /// A struct type's fields, instantiated.
    pub(super) fn struct_fields_of(&self, ty: &rask_types::Type) -> Option<Vec<(String, rask_types::Type)>> {
        let (id, args) = match ty {
            rask_types::Type::Named(id) => (*id, &[][..]),
            rask_types::Type::Generic { base, args } => (*base, args.as_slice()),
            _ => return None,
        };
        let rask_types::TypeDef::Struct { type_params, fields, .. } = self.ctx.type_defs.get(id)? else {
            return None;
        };
        let subst = rask_check::TypeTable::generic_field_subst(type_params, args);
        Some(
            fields
                .iter()
                .map(|(n, t)| (n.clone(), rask_check::TypeTable::substitute_generic_field(t, &subst)))
                .collect(),
        )
    }
}

/// The type-or-variant name a pattern matches on, if it names one.
pub(crate) fn pattern_name(pattern: &rask_ast::expr::Pattern) -> Option<String> {
    use rask_ast::expr::PatternKind;
    match &pattern.kind {
        PatternKind::Ident(name)
        | PatternKind::Constructor { name, .. }
        | PatternKind::Struct { name, .. } => Some(name.clone()),
        PatternKind::TypePat { ty, .. } => Some(super::type_pat_name(ty)),
        _ => None,
    }
}
