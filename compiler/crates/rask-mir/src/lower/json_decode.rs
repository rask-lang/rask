// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! `json.decode<T>(input)` expansion.
//!
//! There's no reflection at runtime, so the call site describes T to the
//! runtime instead: a few `json_shape_*` calls spell out the target's fields
//! (serialized name, byte offset, kind), and one `json_decode_into` fills the
//! destination from the parsed input. Nesting, lists, and maps recurse inside
//! the C decoder rather than unrolling into MIR here.
//!
//! What comes back is a status code, which this turns into the `T or JsonError`
//! the checker gave the expression: 0 → Ok(value), anything else → Err with the
//! matching variant and the runtime's message.

use rask_ast::decl::field_attrs;
use rask_mono::StructLayout;
use rask_types::{GenericArg, Type};

use super::{LoweringError, MirLowerer, TypedOperand};
use crate::{
    operand::MirConst,
    types::EnumLayoutId,
    FieldAccess, FunctionRef, MirOperand, MirRValue, MirStmt, MirStmtKind, MirTerminator,
    MirTerminatorKind, MirType,
};

// Shape kinds — must match the RASK_JSHAPE_* defines in runtime/rask_runtime.h.
const JSHAPE_BOOL: i64 = 0;
const JSHAPE_I8: i64 = 1;
const JSHAPE_I16: i64 = 2;
const JSHAPE_I32: i64 = 3;
const JSHAPE_I64: i64 = 4;
const JSHAPE_U8: i64 = 5;
const JSHAPE_U16: i64 = 6;
const JSHAPE_U32: i64 = 7;
const JSHAPE_U64: i64 = 8;
const JSHAPE_F32: i64 = 9;
const JSHAPE_F64: i64 = 10;
const JSHAPE_STRING: i64 = 11;

/// Shape field flags — must match the decoder's reading of them in json.c.
/// A missing key is fine for this field; whatever is already there stands.
const FIELD_OPTIONAL: i64 = 1;

/// Status codes from `rask_json_decode_into`, in JsonError variant order.
const ERR_PARSE: i64 = 1;
const ERR_TYPE: i64 = 2;
const ERR_MISSING: i64 = 3;

impl<'a> MirLowerer<'a> {
    /// Expand `json.decode<T>(input)` into shape construction + one decode call
    /// + a `T or JsonError` result.
    pub(super) fn lower_json_decode(
        &mut self,
        input: MirOperand,
        target: &Type,
    ) -> Result<TypedOperand, LoweringError> {
        let target_mir = self.ctx.type_to_mir(target);

        // `decode<JsonValue>` is the untyped path — that's exactly what
        // `json.parse` already does, in Rask, so call it instead of teaching the
        // shape decoder how to build the enum a second time.
        if self.type_name_of(target).as_deref() == Some("JsonValue") {
            let err_ty = self
                .ctx
                .find_enum("JsonError")
                .map(|(idx, l)| MirType::Enum(EnumLayoutId::new(idx, l.size, l.align)))
                .unwrap_or(MirType::I64);
            let result_ty = MirType::Result {
                ok: Box::new(target_mir),
                err: Box::new(err_ty),
            };
            let out = self.builder.alloc_temp(result_ty.clone());
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(out),
                func: FunctionRef::internal("json_parse".to_string()),
                args: vec![input],
            }));
            return Ok((MirOperand::Local(out), result_ty));
        }

        let Some(shape) = self.emit_shape(target) else {
            return Err(LoweringError::InvalidConstruct(format!(
                "json.decode cannot build `{}` — only bool, integers, floats, string, \
                 Vec<T>, Map<string, T>, T?, and structs of those are JSON-compatible",
                type_label(target)
            )));
        };

        // The destination. Aggregates already live in their own storage, so the
        // call receives the address directly; a scalar gets a 16-byte scratch
        // buffer to be written into and read back out of.
        let scalar_dst = !target_mir.passed_by_address();
        let dst = if scalar_dst {
            self.builder.alloc_temp(MirType::Array {
                elem: Box::new(MirType::U8),
                len: 16,
            })
        } else {
            self.builder.alloc_temp(target_mir.clone())
        };
        let dst_size = if scalar_dst { 16 } else { target_mir.size() as i64 };
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::internal("json_decode_zero".to_string()),
            args: vec![
                MirOperand::Local(dst),
                MirOperand::Constant(MirConst::Int(dst_size)),
            ],
        }));

        // `@skip` and `@default` fields aren't read from the input, so their
        // values go in before the decoder runs. Zeroed bytes aren't a safe
        // stand-in — an all-zero RaskStr reads as fifteen NUL bytes, and a null
        // Vec pointer isn't an empty Vec.
        if !scalar_dst {
            self.emit_prefilled_fields(dst, 0, target);
        }

        let status = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(status),
            func: FunctionRef::internal("json_decode_into".to_string()),
            args: vec![MirOperand::Local(dst), MirOperand::Local(shape), input],
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::internal("json_shape_free".to_string()),
            args: vec![MirOperand::Local(shape)],
        }));

        let err_ty = self
            .ctx
            .find_enum("JsonError")
            .map(|(idx, l)| MirType::Enum(EnumLayoutId::new(idx, l.size, l.align)))
            .unwrap_or(MirType::I64);
        let result_ty = MirType::Result {
            ok: Box::new(target_mir.clone()),
            err: Box::new(err_ty.clone()),
        };
        let result = self.builder.alloc_temp(result_ty.clone());

        let ok_block = self.builder.create_block();
        let err_block = self.builder.create_block();
        let done_block = self.builder.create_block();

        // status is 0 on success, so it doubles as the branch condition.
        self.builder
            .terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                cond: MirOperand::Local(status),
                then_block: err_block,
                else_block: ok_block,
            }));

        self.builder.switch_to_block(ok_block);
        self.store_result_header(result, 0);
        let payload = if scalar_dst {
            // Read the scalar the runtime wrote back out of the scratch buffer.
            let v = self.builder.alloc_temp(target_mir.clone());
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: v,
                rvalue: MirRValue::Field {
                    base: MirOperand::Local(dst),
                    field_index: 0,
                    byte_offset: Some(0),
                    access: FieldAccess::Sized(target_mir.size()),
                },
            }));
            v
        } else {
            dst
        };
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result,
            offset: crate::types::RESULT_PAYLOAD_OFFSET,
            value: MirOperand::Local(payload),
            store_size: Some(target_mir.size()),
        }));
        self.builder
            .terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                target: done_block,
            }));

        self.builder.switch_to_block(err_block);
        let err_val = self.emit_json_error(status, &err_ty);
        self.store_result_header(result, 1);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result,
            offset: crate::types::RESULT_PAYLOAD_OFFSET,
            value: MirOperand::Local(err_val),
            store_size: Some(err_ty.size()),
        }));
        self.builder
            .terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                target: done_block,
            }));

        self.builder.switch_to_block(done_block);
        Ok((MirOperand::Local(result), result_ty))
    }

    /// Result tag plus the two ER15 origin words. Decode doesn't attribute an
    /// origin — the `try` site fills that in when it propagates.
    fn store_result_header(&mut self, result: crate::LocalId, tag: i64) {
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result,
            offset: crate::types::RESULT_TAG_OFFSET,
            value: MirOperand::Constant(MirConst::Int(tag)),
            store_size: Some(8),
        }));
        for offset in [
            crate::types::RESULT_ORIGIN_FILE_OFFSET,
            crate::types::RESULT_ORIGIN_LINE_OFFSET,
        ] {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: result,
                offset,
                value: MirOperand::Constant(MirConst::Int(0)),
                store_size: Some(8),
            }));
        }
    }

    /// Turn the runtime's status code into a `JsonError` carrying its message.
    fn emit_json_error(&mut self, status: crate::LocalId, err_ty: &MirType) -> crate::LocalId {
        let msg = self.builder.alloc_temp(MirType::String);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(msg),
            func: FunctionRef::internal("json_error_message".to_string()),
            args: vec![],
        }));

        let err_val = self.builder.alloc_temp(err_ty.clone());
        let variants: Vec<(i64, u32, u32)> = [
            (ERR_PARSE, "ParseError"),
            (ERR_TYPE, "TypeError"),
            (ERR_MISSING, "MissingField"),
        ]
        .iter()
        .filter_map(|(code, name)| {
            let (_, layout) = self.ctx.find_enum("JsonError")?;
            let v = layout.variants.iter().find(|v| v.name == *name)?;
            Some((*code, v.tag as u32, v.payload_offset))
        })
        .collect();

        // No JsonError layout (the module wasn't pulled in) — the message alone
        // is the best that can be stored.
        if variants.is_empty() {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: err_val,
                offset: 0,
                value: MirOperand::Local(msg),
                store_size: Some(16),
            }));
            return err_val;
        }

        let tag_offset = self
            .ctx
            .find_enum("JsonError")
            .map(|(_, l)| l.tag_offset)
            .unwrap_or(0);

        // The switch belongs on the block we're standing in now — filling the
        // case blocks moves the builder, and terminating afterwards would
        // overwrite the last case's own jump.
        let entry = self.builder.next_stmt_pos().0;
        let join = self.builder.create_block();
        let mut cases = Vec::new();
        for (code, tag, payload_offset) in &variants {
            let block = self.builder.create_block();
            cases.push((*code as u64, block));
            self.builder.switch_to_block(block);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: err_val,
                offset: tag_offset,
                value: MirOperand::Constant(MirConst::Int(*tag as i64)),
                store_size: Some(8),
            }));
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: err_val,
                offset: *payload_offset,
                value: MirOperand::Local(msg),
                store_size: Some(16),
            }));
            self.builder
                .terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: join }));
        }
        // An unrecognised code lands on ParseError — a status the runtime can't
        // name is still a failure to read the input.
        let default = cases[0].1;
        self.builder.switch_to_block(entry);
        self.builder
            .terminate(MirTerminator::dummy(MirTerminatorKind::Switch {
                value: MirOperand::Local(status),
                cases,
                default,
            }));
        self.builder.switch_to_block(join);
        err_val
    }

    /// Emit the calls that build a runtime shape for `ty`. `None` when the type
    /// isn't JSON-compatible.
    fn emit_shape(&mut self, ty: &Type) -> Option<crate::LocalId> {
        if let Some(kind) = prim_shape_kind(ty) {
            return Some(self.emit_shape_call("json_shape_prim", vec![
                MirOperand::Constant(MirConst::Int(kind)),
            ]));
        }

        // `T?` — [tag:8][payload].
        if let Type::Result { ok, err } = ty {
            if **err == Type::None {
                let inner = self.emit_shape(ok)?;
                let total = self.ctx.type_to_mir(ty).size() as i64;
                return Some(self.emit_shape_call("json_shape_opt", vec![
                    MirOperand::Local(inner),
                    MirOperand::Constant(MirConst::Int(total)),
                ]));
            }
            return None;
        }

        if let Some(elem) = self.collection_arg(ty, "Vec", 0) {
            let elem_shape = self.emit_shape(&elem)?;
            let slot = self.slot_size_for_type(&elem).unwrap_or(8);
            return Some(self.emit_shape_call("json_shape_vec", vec![
                MirOperand::Local(elem_shape),
                MirOperand::Constant(MirConst::Int(slot)),
            ]));
        }

        if let Some(key) = self.collection_arg(ty, "Map", 0) {
            // JSON object keys are strings, so that's the only Map a decode can
            // fill (std.encoding/E15 says the same).
            if !matches!(key, Type::String) && !matches!(&key, Type::UnresolvedNamed(n) if n == "string") {
                return None;
            }
            let val = self.collection_arg(ty, "Map", 1)?;
            let val_shape = self.emit_shape(&val)?;
            let slot = self.slot_size_for_type(&val).unwrap_or(8);
            return Some(self.emit_shape_call("json_shape_map", vec![
                MirOperand::Local(val_shape),
                MirOperand::Constant(MirConst::Int(slot)),
            ]));
        }

        let layout = self.struct_layout_of(ty)?;
        self.emit_struct_shape(&layout)
    }

    fn emit_struct_shape(&mut self, layout: &StructLayout) -> Option<crate::LocalId> {
        let shape = self.emit_shape_call("json_shape_struct", vec![MirOperand::Constant(
            MirConst::Int(layout.size as i64),
        )]);
        for field in &layout.fields {
            // `@skip` fields aren't in the serialized form at all (E19), so the
            // decoder never looks for them.
            if field_attrs::is_skipped(&field.attrs) {
                continue;
            }
            let field_shape = self.emit_shape(&field.ty)?;
            // A field with `@default` tolerates a missing key (E20) — the value
            // is already in place, so the decoder leaves it alone.
            let flags = if field_attrs::default_literal(&field.attrs).is_some() {
                FIELD_OPTIONAL
            } else {
                0
            };
            let key = field_attrs::serial_name(&field.attrs, &field.name);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::internal("json_shape_field".to_string()),
                args: vec![
                    MirOperand::Local(shape),
                    MirOperand::Constant(MirConst::String(key)),
                    MirOperand::Constant(MirConst::Int(field.offset as i64)),
                    MirOperand::Local(field_shape),
                    MirOperand::Constant(MirConst::Int(flags)),
                ],
            }));
        }
        Some(shape)
    }

    /// Write the values the decoder won't: `@default` literals, and a usable
    /// empty value for every `@skip` field. Recurses into nested structs so a
    /// skipped struct's own strings and collections come out valid too.
    fn emit_prefilled_fields(&mut self, dst: crate::LocalId, base: u32, ty: &Type) {
        let Some(layout) = self.struct_layout_of(ty) else {
            return;
        };
        for field in &layout.fields {
            let at = base + field.offset;
            let skipped = field_attrs::is_skipped(&field.attrs);
            let default = field_attrs::default_literal(&field.attrs).map(str::to_string);
            if !skipped && default.is_none() {
                // Not prefilled itself, but a nested struct may hold fields that are.
                self.emit_prefilled_fields(dst, at, &field.ty);
                continue;
            }
            if let Some(literal) = default {
                if self.emit_literal_store(dst, at, &field.ty, &literal) {
                    continue;
                }
            }
            self.emit_zero_value(dst, at, &field.ty);
        }
    }

    /// Store a `@default(…)` literal into a field. False when the literal and
    /// the field type don't go together — the caller falls back to the empty
    /// value rather than writing nonsense.
    fn emit_literal_store(
        &mut self,
        dst: crate::LocalId,
        offset: u32,
        ty: &Type,
        literal: &str,
    ) -> bool {
        let literal = literal.trim();
        let (value, size) = match prim_shape_kind(ty) {
            Some(JSHAPE_STRING) => match field_attrs::string_literal(literal) {
                Some(text) => (MirConst::String(text), 16u32),
                None => return false,
            },
            Some(JSHAPE_BOOL) => match literal {
                "true" => (MirConst::Int(1), 1),
                "false" => (MirConst::Int(0), 1),
                _ => return false,
            },
            Some(JSHAPE_F32) | Some(JSHAPE_F64) => match literal.parse::<f64>() {
                Ok(f) => (MirConst::Float(f), self.ctx.type_to_mir(ty).size()),
                Err(_) => return false,
            },
            Some(_) => match literal.parse::<i64>() {
                Ok(n) => (MirConst::Int(n), self.ctx.type_to_mir(ty).size()),
                Err(_) => return false,
            },
            None => return false,
        };
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: dst,
            offset,
            value: MirOperand::Constant(value),
            store_size: Some(size),
        }));
        true
    }

    /// The empty value for a type, written in place: `""`, an empty Vec or Map,
    /// `none`, a struct of the same. Numbers and bools are already zero.
    fn emit_zero_value(&mut self, dst: crate::LocalId, offset: u32, ty: &Type) {
        if matches!(prim_shape_kind(ty), Some(JSHAPE_STRING)) {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: dst,
                offset,
                value: MirOperand::Constant(MirConst::String(String::new())),
                store_size: Some(16),
            }));
            return;
        }
        if let Type::Result { err, .. } = ty {
            if **err == Type::None {
                // tag 1 = none; the payload stays zero and is never read.
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                    addr: dst,
                    offset,
                    value: MirOperand::Constant(MirConst::Int(1)),
                    store_size: Some(8),
                }));
                return;
            }
        }
        let ctor = if self.collection_arg(ty, "Vec", 0).is_some() {
            Some("Vec_new")
        } else if self.is_map_type(ty) {
            Some("Map_new_string_keys")
        } else {
            None
        };
        if let Some(ctor) = ctor {
            let empty = self.builder.alloc_temp(MirType::I64);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(empty),
                func: FunctionRef::internal(ctor.to_string()),
                args: vec![],
            }));
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: dst,
                offset,
                value: MirOperand::Local(empty),
                store_size: Some(8),
            }));
            return;
        }
        if let Some(layout) = self.struct_layout_of(ty) {
            for field in &layout.fields {
                self.emit_zero_value(dst, offset + field.offset, &field.ty.clone());
            }
        }
    }

    fn emit_shape_call(&mut self, name: &str, args: Vec<MirOperand>) -> crate::LocalId {
        let local = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(local),
            func: FunctionRef::internal(name.to_string()),
            args,
        }));
        local
    }

    /// The Nth generic argument of `Vec<…>` / `Map<…>`, whichever spelling the
    /// checker left behind.
    fn collection_arg(&self, ty: &Type, wanted: &str, index: usize) -> Option<Type> {
        let args = match ty {
            Type::UnresolvedGeneric { name, args } if name == wanted => args,
            Type::Generic { base, args }
                if self.ctx.type_names.get(base).map(|n| n == wanted).unwrap_or(false) =>
            {
                args
            }
            _ => return None,
        };
        match args.get(index)? {
            GenericArg::Type(t) => Some(t.as_ref().clone()),
            _ => None,
        }
    }

    fn type_name_of(&self, ty: &Type) -> Option<String> {
        match ty {
            Type::UnresolvedNamed(n) => Some(n.clone()),
            Type::UnresolvedGeneric { name, .. } => Some(name.clone()),
            Type::Named(id) => self.ctx.type_names.get(id).cloned(),
            Type::Generic { base, .. } => self.ctx.type_names.get(base).cloned(),
            _ => None,
        }
    }

    fn struct_layout_of(&self, ty: &Type) -> Option<StructLayout> {
        let name = match ty {
            Type::UnresolvedNamed(n) => n.clone(),
            Type::UnresolvedGeneric { name, .. } => name.clone(),
            Type::Named(id) => self.ctx.type_names.get(id)?.clone(),
            Type::Generic { base, .. } => self.ctx.type_names.get(base)?.clone(),
            _ => return None,
        };
        // Vec/Map have stdlib layouts too, and matching them here would decode a
        // list as if it were a two-field struct.
        if name == "Vec" || name == "Map" || name == "string" {
            return None;
        }
        self.ctx.find_struct(&name).map(|(_, l)| l.clone())
    }

    pub(super) fn is_map_type(&self, ty: &Type) -> bool {
        match ty {
            Type::UnresolvedGeneric { name, .. } => name == "Map",
            Type::Generic { base, .. } => {
                self.ctx.type_names.get(base).map(|n| n == "Map").unwrap_or(false)
            }
            _ => false,
        }
    }

    /// Is this `T?` holding something the runtime has to walk?
    pub(super) fn is_optional_collection(&self, ty: &Type) -> bool {
        let Type::Result { ok, err } = ty else { return false };
        if **err != Type::None {
            return false;
        }
        self.collection_arg(ok, "Vec", 0).is_some() || self.is_map_type(ok)
    }

    /// Encode a value the runtime has to walk (a Map, or a `T?` around a
    /// collection) by handing it the same shape decoding uses. Returns the
    /// local holding the JSON fragment.
    pub(super) fn lower_json_encode_shaped(
        &mut self,
        value: crate::LocalId,
        ty: &Type,
    ) -> Option<crate::LocalId> {
        // The encoder reads through a pointer. An aggregate local already *is*
        // its storage, so its address goes straight in; a word-sized value has
        // to be parked in a buffer the call can point at.
        let by_address = self
            .builder
            .local_type(value)
            .map(|t| t.passed_by_address())
            .unwrap_or(false);
        let slot = if by_address {
            value
        } else {
            let buf = self.builder.alloc_temp(MirType::Array {
                elem: Box::new(MirType::U8),
                len: 8,
            });
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: buf,
                offset: 0,
                value: MirOperand::Local(value),
                store_size: Some(8),
            }));
            buf
        };
        let shape = self.emit_shape(ty)?;
        let out = self.builder.alloc_temp(MirType::String);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(out),
            func: FunctionRef::internal("json_encode_shaped".to_string()),
            args: vec![MirOperand::Local(slot), MirOperand::Local(shape)],
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::internal("json_shape_free".to_string()),
            args: vec![MirOperand::Local(shape)],
        }));
        Some(out)
    }

    /// What T is in `json.decode<T>(…)`.
    ///
    /// The checker's type for the call is the better source — it has already
    /// resolved `T` against the binding it flows into, so `const u: User = try
    /// json.decode(body)` works without the turbofish. The written type
    /// argument is the fallback for when inference left a variable behind.
    pub(super) fn json_decode_target(
        &self,
        expr: &rask_ast::expr::Expr,
        type_args: &Option<Vec<String>>,
    ) -> Result<Type, LoweringError> {
        if let Some(Type::Result { ok, .. }) = self.ctx.lookup_raw_type(expr.id) {
            if !matches!(**ok, Type::Var(_)) {
                return Ok((**ok).clone());
            }
        }
        if let Some(written) = type_args.as_ref().and_then(|a| a.first()) {
            return Ok(parse_type_str(written));
        }
        Err(LoweringError::InvalidConstruct(
            "json.decode needs to know what to build — write it as \
             `json.decode<Type>(input)` or annotate the binding"
                .to_string(),
        ))
    }
}

/// A written type argument, back into a checker `Type`. Only the shapes JSON
/// can produce need to survive this: primitives, `Vec<T>`, `Map<K, V>`, `T?`,
/// and named structs.
fn parse_type_str(s: &str) -> Type {
    let s = s.trim();
    if let Some(inner) = s.strip_suffix('?') {
        return Type::Result {
            ok: Box::new(parse_type_str(inner)),
            err: Box::new(Type::None),
        };
    }
    if let Some(open) = s.find('<') {
        if s.ends_with('>') {
            let name = s[..open].trim().to_string();
            let args = split_type_args(&s[open + 1..s.len() - 1])
                .into_iter()
                .map(|a| GenericArg::Type(Box::new(parse_type_str(&a))))
                .collect();
            return Type::UnresolvedGeneric { name, args };
        }
    }
    match s {
        "bool" => Type::Bool,
        "i8" => Type::I8,
        "i16" => Type::I16,
        "i32" => Type::I32,
        "i64" => Type::I64,
        "isize" => Type::I64,
        "u8" => Type::U8,
        "u16" => Type::U16,
        "u32" => Type::U32,
        "u64" => Type::U64,
        "usize" => Type::U64,
        "f32" => Type::F32,
        "f64" => Type::F64,
        "string" => Type::String,
        other => Type::UnresolvedNamed(other.to_string()),
    }
}

/// Split on commas that aren't inside a nested `<…>`.
fn split_type_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => {
                out.push(s[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    let tail = s[start..].trim();
    if !tail.is_empty() {
        out.push(tail.to_string());
    }
    out
}

fn prim_shape_kind(ty: &Type) -> Option<i64> {
    let kind = match ty {
        Type::Bool => JSHAPE_BOOL,
        Type::I8 => JSHAPE_I8,
        Type::I16 => JSHAPE_I16,
        Type::I32 => JSHAPE_I32,
        Type::I64 => JSHAPE_I64,
        Type::U8 => JSHAPE_U8,
        Type::U16 => JSHAPE_U16,
        Type::U32 => JSHAPE_U32,
        Type::U64 => JSHAPE_U64,
        Type::F32 => JSHAPE_F32,
        Type::F64 => JSHAPE_F64,
        Type::String => JSHAPE_STRING,
        Type::UnresolvedNamed(n) => match n.as_str() {
            "bool" => JSHAPE_BOOL,
            "i8" => JSHAPE_I8,
            "i16" => JSHAPE_I16,
            "i32" => JSHAPE_I32,
            "i64" | "isize" => JSHAPE_I64,
            "u8" => JSHAPE_U8,
            "u16" => JSHAPE_U16,
            "u32" => JSHAPE_U32,
            "u64" | "usize" => JSHAPE_U64,
            "f32" => JSHAPE_F32,
            "f64" => JSHAPE_F64,
            "string" => JSHAPE_STRING,
            _ => return None,
        },
        _ => return None,
    };
    Some(kind)
}

fn type_label(ty: &Type) -> String {
    match ty {
        Type::UnresolvedNamed(n) => n.clone(),
        Type::UnresolvedGeneric { name, .. } => name.clone(),
        other => format!("{:?}", other),
    }
}
