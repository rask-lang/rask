// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Where a pattern points inside a `T or E`.
//!
//! A value of `T or (A | B)` can be tested at up to three depths: which side
//! it holds, which member of a union error side, and which variant of that
//! member's enum. On a flat `T? or E` the success side is an option, so a
//! variant of `T` sits one step further down: the success side, then present,
//! then the variant. The checker records what a pattern names (`PatternTarget`);
//! this turns the record and the scrutinee's MIR type into the path down, so
//! every `is` test and every `match` arm reads the same tags in the same
//! order. Enums are found by their layout's type id, never by name.

use super::{LoweringError, MirLowerer};
use crate::{
    BlockId, FieldAccess, LocalId, MirOperand, MirRValue, MirStmt, MirStmtKind, MirTerminator,
    MirTerminatorKind, MirType,
};

/// The steps from a `T or E` down to what a pattern names.
pub(super) struct ResultPath {
    /// The error side (`true`) or the success side.
    pub err: bool,
    /// The side is an option, and the path goes on into its value: a variant
    /// of `T` on a flat `T? or E`.
    pub present: bool,
    /// Within a union error side: the member's index and type.
    pub member: Option<(usize, MirType)>,
    /// A variant of the enum the steps above reach.
    pub variant: Option<PathVariant>,
}

pub(super) struct PathVariant {
    /// The enum, as it is laid out where the path reaches it.
    pub enum_ty: MirType,
    /// Index into `enum_layouts`.
    pub layout: usize,
    /// Index into that layout's variants.
    pub index: usize,
}

impl<'a> MirLowerer<'a> {
    /// The path `pat` names inside `val_ty`, a `T or E`. `None` when
    /// `val_ty` isn't one, or the pattern names no step of it (a binding).
    pub(super) fn result_path(&self, pat: &rask_ast::expr::Pattern, val_ty: &MirType) -> Option<ResultPath> {
        let MirType::Result { ok, err } = val_ty else { return None };
        match self.pattern_target(pat)? {
            rask_types::PatternTarget::Side { err: is_err, member } => {
                let member = match (is_err, member, err.as_ref()) {
                    (true, Some(i), MirType::Union(members)) => Some((*i, members.get(*i)?.clone())),
                    _ => None,
                };
                Some(ResultPath { err: *is_err, present: false, member, variant: None })
            }
            rask_types::PatternTarget::Variant { enum_id, variant } => {
                let find = |t: &MirType| -> Option<PathVariant> {
                    let MirType::Enum(eid) = t else { return None };
                    let layout = self.ctx.enum_layouts.get(eid.id as usize)?;
                    if layout.key.as_ref()?.id != *enum_id {
                        return None;
                    }
                    let index = layout.variants.iter().position(|v| v.name == *variant)?;
                    Some(PathVariant { enum_ty: t.clone(), layout: eid.id as usize, index })
                };
                let path = |err, present, member, v| ResultPath { err, present, member, variant: Some(v) };
                if let MirType::Union(members) = err.as_ref() {
                    for (i, m) in members.iter().enumerate() {
                        if let Some(v) = find(m) {
                            return Some(path(true, false, Some((i, m.clone())), v));
                        }
                    }
                } else if let Some(v) = find(err) {
                    return Some(path(true, false, None, v));
                }
                match ok.as_ref() {
                    MirType::Option(inner) => find(inner).map(|v| path(false, true, None, v)),
                    ok => find(ok).map(|v| path(false, false, None, v)),
                }
            }
            rask_types::PatternTarget::Binding => None,
        }
    }

    /// `result_path`, only when it goes deeper than a side: a union member
    /// or a variant. Those are what the plain ok/err tag can't answer.
    /// (`present` only ever leads to a variant, so it needs no check here.)
    pub(super) fn deep_result_path(&self, pat: &rask_ast::expr::Pattern, val_ty: &MirType) -> Option<ResultPath> {
        self.result_path(pat, val_ty).filter(|p| p.member.is_some() || p.variant.is_some())
    }

    /// The value at the end of `path`'s side, present and member steps, read
    /// without testing anything: the side's payload, the option's value, or
    /// the member inside it.
    fn result_path_value(&mut self, val: &MirOperand, val_ty: &MirType, path: &ResultPath) -> (MirOperand, MirType) {
        let (mut side, mut side_ty) = self.result_side_value(val, val_ty, path.err);
        if path.present {
            (side, side_ty) = self.option_value(side, &side_ty);
        }
        match &path.member {
            Some((_, member_ty)) => {
                let member = self.builder.alloc_temp(member_ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: member,
                    rvalue: self.payload_field(side, &side_ty, member_ty),
                }));
                (MirOperand::Local(member), member_ty.clone())
            }
            None => (side, side_ty),
        }
    }

    /// The value inside `opt`, an option, read without testing its tag.
    fn option_value(&mut self, opt: MirOperand, opt_ty: &MirType) -> (MirOperand, MirType) {
        let MirType::Option(inner) = opt_ty else {
            unreachable!("a present step is only made for an option")
        };
        let inner_ty = (**inner).clone();
        let value = self.builder.alloc_temp(inner_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: value,
            rvalue: self.payload_field(opt, opt_ty, &inner_ty),
        }));
        (MirOperand::Local(value), inner_ty)
    }

    /// The checker's type of the value `path` ends at, walked down from
    /// `val`'s the same way `result_path_value` walks the bytes. A generic
    /// enum's payload types come from here.
    fn result_path_checked(&self, path: &ResultPath, val: Option<&rask_types::Type>) -> Option<rask_types::Type> {
        let rask_types::Type::Result { ok, err } = val? else { return None };
        let mut ty: rask_types::Type = if path.err { (**err).clone() } else { (**ok).clone() };
        if path.present {
            ty = ty.as_option()?.clone();
        }
        if let Some((index, _)) = &path.member {
            // The member index counts in canonical order, as `type_to_mir`
            // lays the union out.
            let rask_types::Type::Union(members) = self.ctx.canonical_union(&ty) else { return None };
            ty = members.get(*index)?.clone();
        }
        Some(ty)
    }

    fn result_side_value(&mut self, val: &MirOperand, val_ty: &MirType, err: bool) -> (MirOperand, MirType) {
        let MirType::Result { ok, err: err_ty } = val_ty else {
            unreachable!("a result path is only made for a result")
        };
        let side_ty = if err { (**err_ty).clone() } else { (**ok).clone() };
        let side = self.builder.alloc_temp(side_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: side,
            rvalue: self.payload_field(val.clone(), val_ty, &side_ty),
        }));
        (MirOperand::Local(side), side_ty)
    }

    /// Branch to `fail` unless `cond`; carry on in the block where it held.
    fn pass_or(&mut self, cond: LocalId, fail: BlockId) {
        let pass = self.builder.create_block();
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(cond),
            then_block: pass,
            else_block: fail,
        }));
        self.builder.switch_to_block(pass);
    }

    /// Test `val` against `pattern` along `path`, one step at a time, going to
    /// `fail` at the first that doesn't hold. Each tag is read only once the
    /// step above it has passed, so a payload is never read as something it
    /// isn't. Returns in the block where every step held.
    pub(super) fn emit_result_path_test(
        &mut self,
        val: &MirOperand,
        val_ty: &MirType,
        checked: Option<&rask_types::Type>,
        path: &ResultPath,
        pattern: &rask_ast::expr::Pattern,
        fail: BlockId,
    ) -> Result<(), LoweringError> {
        let tag = self.emit_option_tag(val, None);
        let on_side = self.emit_eq_const(tag, path.err as i64);
        self.pass_or(on_side, fail);
        if path.present {
            // An option's tag is 0 when it holds a value.
            let (side, _) = self.result_side_value(val, val_ty, path.err);
            let tag = self.emit_option_tag(&side, None);
            let present = self.emit_eq_const(tag, 0);
            self.pass_or(present, fail);
        }
        if let Some((index, _)) = &path.member {
            let (side, _) = self.result_side_value(val, val_ty, path.err);
            let member_index = self.emit_enum_tag(side);
            let is_member = self.emit_eq_const(member_index, *index as i64);
            self.pass_or(is_member, fail);
        }
        let Some(variant) = &path.variant else { return Ok(()) };
        let (value, _) = self.result_path_value(val, val_ty, path);
        match &pattern.kind {
            // `Enum.Variant as x` has no fields to test, only the tag.
            rask_ast::expr::PatternKind::TypePat { .. } => {
                let want = self.ctx.enum_layouts[variant.layout].variants[variant.index].tag as i64;
                let tag = self.emit_enum_tag(value);
                let is_variant = self.emit_eq_const(tag, want);
                self.pass_or(is_variant, fail);
                Ok(())
            }
            // A variant pattern tests against its own enum like any other:
            // the tag, then any literal or nested variant in its fields.
            _ => {
                let enum_checked = self.result_path_checked(path, checked);
                self.emit_subpattern_test(&value, &variant.enum_ty, enum_checked.as_ref(), pattern, fail)
            }
        }
    }

    /// Bind what `pattern` names once `emit_result_path_test` has passed.
    /// A variant's fields bind from the variant; `Enum.Variant as x` binds
    /// the variant's payload (its one field, or a tuple of several); any
    /// other `T as x` binds the value the path ends at.
    pub(super) fn bind_result_path(
        &mut self,
        val: &MirOperand,
        val_ty: &MirType,
        checked: Option<&rask_types::Type>,
        path: &ResultPath,
        pattern: &rask_ast::expr::Pattern,
    ) {
        use rask_ast::expr::PatternKind;
        let binding = match &pattern.kind {
            PatternKind::TypePat { binding, .. } => binding.as_ref(),
            _ => None,
        };
        if path.variant.is_none() && binding.is_none() {
            return;
        }
        let (value, value_ty) = self.result_path_value(val, val_ty, path);
        let Some(variant) = &path.variant else {
            let Some(name) = binding else { return };
            self.bind_local(name, MirRValue::Use(value), value_ty);
            return;
        };
        let enum_checked = self.result_path_checked(path, checked);
        let Some(name) = binding else {
            self.bind_pattern_fields(pattern, value, None, false, &variant.enum_ty, enum_checked.as_ref());
            return;
        };
        let variant_name = self.ctx.enum_layouts[variant.layout].variants[variant.index].name.clone();
        let fields = self
            .variant_field_types(&variant.enum_ty, enum_checked.as_ref(), &variant_name)
            .unwrap_or_default();
        match fields.as_slice() {
            // A fieldless variant's payload is unit: nothing to read.
            [] => {}
            [(field_ty, offset, size)] => {
                let rvalue = MirRValue::Field {
                    base: value,
                    field_index: 0,
                    byte_offset: Some(*offset),
                    access: FieldAccess::for_field(field_ty, *size),
                };
                self.bind_local(name, rvalue, field_ty.clone());
            }
            // Several fields bind a tuple, which has to be built: the
            // variant's fields sit at the enum's offsets, the tuple's at its
            // own, and the two don't line up.
            many => {
                let tuple_ty = MirType::Tuple(many.iter().map(|(t, _, _)| t.clone()).collect());
                let local = self.builder.alloc_local(name.clone(), tuple_ty.clone());
                let mut tuple_offset = 0u32;
                for (field_ty, src_offset, size) in many {
                    let align = field_ty.align().max(1);
                    tuple_offset = (tuple_offset + align - 1) & !(align - 1);
                    let read = self.builder.alloc_temp(field_ty.clone());
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                        dst: read,
                        rvalue: MirRValue::Field {
                            base: value.clone(),
                            field_index: 0,
                            byte_offset: Some(*src_offset),
                            access: FieldAccess::for_field(field_ty, *size),
                        },
                    }));
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                        addr: local,
                        offset: tuple_offset,
                        value: MirOperand::Local(read),
                        store_size: field_ty.size(),
                        field: None,
                    }));
                    tuple_offset += field_ty.size();
                }
                self.locals.insert(name.clone(), (local, tuple_ty.clone()));
                if let Some(prefix) = self.mir_type_name(&tuple_ty) {
                    self.meta_mut(name).type_prefix = Some(prefix);
                }
            }
        }
    }

    /// A fresh named local holding `rvalue`, typed `ty`.
    fn bind_local(&mut self, name: &str, rvalue: MirRValue, ty: MirType) {
        let local = self.builder.alloc_local(name.to_string(), ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign { dst: local, rvalue }));
        self.locals.insert(name.to_string(), (local, ty.clone()));
        if let Some(prefix) = self.mir_type_name(&ty) {
            self.meta_mut(name).type_prefix = Some(prefix);
        }
    }
}
