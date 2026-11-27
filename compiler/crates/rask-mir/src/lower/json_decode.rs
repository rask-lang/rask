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
use rask_ast::ty::ParamMode;
use rask_layout::{EnumLayout, StructLayout};
use rask_types::{GenericArg, Type};

use super::{LoweringError, MirLowerer, TypedOperand};
use crate::{
    operand::MirConst,
    types::EnumLayoutId,
    FieldAccess, FunctionRef, MirOperand, MirRValue, MirStmt, MirStmtKind, MirTerminator,
    MirTerminatorKind, MirType,
};

use rask_mono::runtime_abi::{
    RASK_JFIELD_NOT_SERIALIZED as FIELD_NOT_SERIALIZED, RASK_JSHAPE_BOOL as JSHAPE_BOOL,
    RASK_JSHAPE_F32 as JSHAPE_F32, RASK_JSHAPE_F64 as JSHAPE_F64, RASK_JSHAPE_I16 as JSHAPE_I16,
    RASK_JSHAPE_I32 as JSHAPE_I32, RASK_JSHAPE_I64 as JSHAPE_I64, RASK_JSHAPE_I8 as JSHAPE_I8,
    RASK_JSHAPE_CHAR as JSHAPE_CHAR, RASK_JSHAPE_I128 as JSHAPE_I128, RASK_JSHAPE_U128 as JSHAPE_U128, RASK_JSHAPE_STRING as JSHAPE_STRING, RASK_JSHAPE_U16 as JSHAPE_U16,
    RASK_JSHAPE_U32 as JSHAPE_U32, RASK_JSHAPE_U64 as JSHAPE_U64, RASK_JSHAPE_U8 as JSHAPE_U8,
    RASK_JSON_ERR_MISSING as ERR_MISSING, RASK_JSON_ERR_PARSE as ERR_PARSE,
    RASK_JSON_ERR_REFUSED as ERR_REFUSED, RASK_JSON_ERR_TYPE as ERR_TYPE,
    RASK_JSON_TEXT_MESSAGE as TEXT_MESSAGE, RASK_JSON_TEXT_PATH as TEXT_PATH,
};

impl<'a> MirLowerer<'a> {
    /// Expand `json.decode<T>(input)` into shape construction + one decode call
    /// + a `T or JsonError` result.
    pub(super) fn lower_json_decode(
        &mut self,
        input: MirOperand,
        target: &Type,
        call_ty: Option<&Type>,
    ) -> Result<TypedOperand, LoweringError> {
        let target_mir = self.ctx.type_to_mir(target);
        // `T or JsonError`, as the checker typed the call.
        let json_error = match call_ty {
            Some(Type::Result { err, .. }) => self.ctx.enum_of(err).map(|(idx, l)| (idx, l.clone())),
            _ => None,
        };
        // The same type as a wrapper's payload: a `Vec<T>` target keeps what it
        // is there, so releasing the result frees the vector the decoder built.
        let payload_mir = self.ctx.type_to_mir(target);

        // `decode<JsonValue>` is the untyped path — that's exactly what
        // `json.parse` already does, in Rask, so call it instead of teaching the
        // shape decoder how to build the enum a second time.
        if self.ctx.type_defs.is_lang(target, rask_types::LangItem::JsonValue) {
            let err_ty = json_error
                .as_ref()
                .map(|(idx, l)| MirType::Enum(EnumLayoutId::new(*idx, l.size, l.align)))
                .unwrap_or_else(|| crate::fallback::unknown_type("lower/json_decode:68"));
            let result_ty = MirType::Result {
                ok: Box::new(payload_mir),
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
                 Vec<T>, [T; N], Map<string, T>, T?, and structs and enums of those are JSON-compatible",
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
        let dst_size = if scalar_dst { 16 } else { rask_layout::size_of(&target_mir, self.ctx.target()) as i64 };
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::reading("rask_json_decode_zero"),
            args: vec![
                MirOperand::Local(dst),
                MirOperand::Constant(MirConst::Int(dst_size)),
            ],
        }));

        let status = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(status),
            func: FunctionRef::reading("rask_json_decode_into"),
            args: vec![
                MirOperand::Local(dst),
                MirOperand::Local(shape),
                input,
                crate::elem_strs::elem(target_mir.clone()),
            ],
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::releasing("rask_json_shape_free"),
            args: vec![MirOperand::Local(shape)],
        }));

        let err_ty = json_error
            .as_ref()
            .map(|(idx, l)| MirType::Enum(EnumLayoutId::new(*idx, l.size, l.align)))
            .unwrap_or_else(|| crate::fallback::unknown_type("lower/json_decode:136"));
        let result_ty = MirType::Result {
            ok: Box::new(payload_mir),
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
        self.store_wrapper_tag(result, &result_ty, rask_mono::runtime_abi::RASK_TAG_PRESENT);
        let payload = if scalar_dst {
            // Read the scalar the runtime wrote back out of the scratch buffer.
            let v = self.builder.alloc_temp(target_mir.clone());
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: v,
                rvalue: MirRValue::Field {
                    base: MirOperand::Local(dst),
                    field_index: 0,
                    byte_offset: Some(0),
                    access: FieldAccess::for_field(&target_mir, rask_layout::size_of(&target_mir, self.ctx.target())),
                },
            }));
            v
        } else {
            dst
        };
        self.store_wrapper_payload(result, &result_ty, MirOperand::Local(payload), &target_mir);
        self.builder
            .terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                target: done_block,
            }));

        self.builder.switch_to_block(err_block);
        let err_val = self.emit_json_error(status, &err_ty, json_error.as_ref().map(|(_, l)| l));
        self.store_wrapper_tag(result, &result_ty, rask_mono::runtime_abi::RASK_TAG_ABSENT);
        self.store_wrapper_payload(result, &result_ty, MirOperand::Local(err_val), &err_ty);
        self.builder
            .terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                target: done_block,
            }));

        self.builder.switch_to_block(done_block);
        Ok((MirOperand::Local(result), result_ty))
    }

    /// Turn the runtime's status code into a `JsonError` carrying its message.
    fn emit_json_error(
        &mut self,
        status: crate::LocalId,
        err_ty: &MirType,
        json_error: Option<&EnumLayout>,
    ) -> crate::LocalId {
        let msg = self.builder.alloc_temp(MirType::Buffer);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(msg),
            func: FunctionRef::reading("rask_json_error_message").writing_string(),
            args: vec![MirOperand::Constant(MirConst::Int(TEXT_MESSAGE))],
        }));

        let err_val = self.builder.alloc_temp(err_ty.clone());
        // Each variant's tag and where its payload fields sit.
        let variants: Vec<(i64, u32, u32, Vec<u32>)> = [
            (ERR_PARSE, "ParseError"),
            (ERR_TYPE, "TypeError"),
            (ERR_MISSING, "MissingField"),
            (ERR_REFUSED, "Refused"),
        ]
        .iter()
        .filter_map(|(code, name)| {
            let v = json_error?.variants.iter().find(|v| v.name == *name)?;
            let at = v.fields.iter().map(|f| v.payload_offset + f.offset).collect();
            Some((*code, v.tag as u32, v.payload_offset, at))
        })
        .collect();

        // No JsonError layout (the module wasn't pulled in) — the message alone
        // is the best that can be stored.
        if variants.is_empty() {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: err_val,
                offset: 0,
                value: MirOperand::Local(msg),
                store_size: 16,
                field: None,
            }));
            return err_val;
        }

        let (tag_offset, tag_bytes) = json_error.map(|l| (l.tag_offset, l.tag_bytes())).unwrap_or((0, 1));

        // The switch belongs on the block we're standing in now — filling the
        // case blocks moves the builder, and terminating afterwards would
        // overwrite the last case's own jump.
        let entry = self.builder.next_stmt_pos().0;
        let join = self.builder.create_block();
        let mut cases = Vec::new();
        for (code, tag, payload_offset, fields) in &variants {
            let block = self.builder.create_block();
            cases.push((*code as u64, block));
            self.builder.switch_to_block(block);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: err_val,
                offset: tag_offset,
                value: MirOperand::Constant(MirConst::Int(*tag as i64)),
                store_size: tag_bytes,
                field: None,
            }));
            // A refusal holds where and why, apart; the others hold the
            // message alone.
            let (message_at, path_at) = match fields.as_slice() {
                [path, reason] if *code == ERR_REFUSED => (*reason, Some(*path)),
                _ => (*payload_offset, None),
            };
            if let Some(path_at) = path_at {
                let path = self.builder.alloc_temp(MirType::Buffer);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: Some(path),
                    func: FunctionRef::reading("rask_json_error_message").writing_string(),
                    args: vec![MirOperand::Constant(MirConst::Int(TEXT_PATH))],
                }));
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                    addr: err_val,
                    offset: path_at,
                    value: MirOperand::Local(path),
                    store_size: 16,
                    field: None,
                }));
            }
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: err_val,
                offset: message_at,
                value: MirOperand::Local(msg),
                store_size: 16,
                field: None,
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
        self.emit_shape_for(ty, ShapeFor::Json)
    }

    fn emit_shape_for(&mut self, ty: &Type, purpose: ShapeFor) -> Option<crate::LocalId> {
        if let Some(kind) = prim_shape_kind(ty, self.ctx.type_defs.target()) {
            return Some(self.emit_shape_call("rask_json_shape_prim", vec![
                MirOperand::Constant(MirConst::Int(kind)),
            ]));
        }
        if purpose == ShapeFor::Claims {
            if let Some(shape) = self.emit_generated_shape(ty) {
                return Some(shape);
            }
        }
        // A type with its own `decode` is read as its wire type and handed to
        // that (std.encoding/E12b).
        if let Some(shape) = self.emit_custom_decode_shape(ty, purpose) {
            return shape;
        }
        if purpose == ShapeFor::Claims {
            if let Some(shape) = self.emit_claim_only_shape(ty) {
                return Some(shape);
            }
        }

        // `T?`: its size and where its payload sits.
        if let Type::Result { ok, err } = ty {
            if **err == Type::None {
                let inner = self.emit_shape_for(ok, purpose)?;
                let opt = self.ctx.type_to_mir(ty);
                return Some(self.emit_shape_call("rask_json_shape_opt", vec![
                    MirOperand::Local(inner),
                    MirOperand::Constant(MirConst::Int(rask_layout::size_of(&opt, self.ctx.target()) as i64)),
                    MirOperand::Constant(MirConst::Int(rask_layout::payload_offset_of(&opt, self.ctx.target()) as i64)),
                ]));
            }
            return None;
        }

        if let Some(elem) = self.collection_arg(ty, "Vec", 0) {
            let elem_shape = self.emit_shape_for(&elem, purpose)?;
            let slot = self.slot_size_for_type(&elem)?;
            let owned = crate::elem_strs::elem(self.ctx.type_to_mir(&elem));
            return Some(self.emit_shape_call("rask_json_shape_vec", vec![
                MirOperand::Local(elem_shape),
                MirOperand::Constant(MirConst::Int(slot)),
                owned,
            ]));
        }

        if let Some(key) = self.collection_arg(ty, "Map", 0) {
            // JSON object keys are strings, so that's the only Map a decode can
            // fill (std.encoding/E15 says the same).
            if !matches!(key, Type::String) && !matches!(&key, Type::UnresolvedNamed(n) if n == "string") {
                return None;
            }
            let val = self.collection_arg(ty, "Map", 1)?;
            let val_shape = self.emit_shape_for(&val, purpose)?;
            let slot = self.slot_size_for_type(&val)?;
            let owned = crate::elem_strs::elem(self.ctx.type_to_mir(&val));
            return Some(self.emit_shape_call("rask_json_shape_map", vec![
                MirOperand::Local(val_shape),
                MirOperand::Constant(MirConst::Int(slot)),
                owned,
            ]));
        }

        if let Type::Array { elem, len: rask_types::ArrayLen::Known(len) } = ty {
            let elem_shape = self.emit_shape_for(elem, purpose)?;
            let stride = self.slot_size_for_type(elem)?;
            return Some(self.emit_shape_call("rask_json_shape_array", vec![
                MirOperand::Local(elem_shape),
                MirOperand::Constant(MirConst::Int(stride)),
                MirOperand::Constant(MirConst::Int(*len as i64)),
            ]));
        }

        if let MirType::Enum(id) = self.ctx.type_to_mir(ty) {
            return self.emit_enum_shape(id.id as usize, purpose);
        }

        let layout = self.struct_layout_of(ty)?;
        self.emit_struct_shape(&layout, purpose)
    }

    /// An enum's shape: where its tag sits, and per variant its wire name and
    /// payload, tagged the way `lower_json_encode_enum` writes it
    /// (std.encoding/E22–E25). A lone unnamed payload is `bare`: the JSON
    /// value is the payload itself, not an object of fields (E23).
    fn emit_enum_shape(&mut self, index: usize, purpose: ShapeFor) -> Option<crate::LocalId> {
        let int = |v: i64| MirOperand::Constant(MirConst::Int(v));
        let layout = self.ctx.enum_layouts.get(index)?.clone();
        let shape = self.emit_shape_call("json_shape_enum", vec![
            int(layout.size as i64),
            int(layout.tag_offset as i64),
            int(layout.tag_bytes() as i64),
        ]);
        if let Some(tag) = field_attrs::tag_field(&layout.attrs) {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::reading("rask_json_shape_enum_tag"),
                args: vec![MirOperand::Local(shape), MirOperand::Constant(MirConst::String(tag))],
            }));
        }
        for variant in &layout.variants {
            let payload = if variant.fields.is_empty() {
                int(0)
            } else {
                let payload = self.emit_shape_call("rask_json_shape_struct", vec![int(layout.size as i64)]);
                for f in &variant.fields {
                    let field_shape = self.emit_shape_for(&f.ty, purpose)?;
                    self.add_shape_field(payload, &f.name, (variant.payload_offset + f.offset) as i64, field_shape);
                }
                MirOperand::Local(payload)
            };
            let bare = variant.fields.len() == 1 && rask_ast::decl::is_positional_field_name(&variant.fields[0].name);
            let name = field_attrs::serial_name(&variant.attrs, &variant.name);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::reading("rask_json_shape_variant"),
                args: vec![
                    MirOperand::Local(shape),
                    int(variant.tag as i64),
                    payload,
                    MirOperand::Constant(MirConst::String(name)),
                    int(bare as i64),
                ],
            }));
        }
        Some(shape)
    }

    fn emit_struct_shape(&mut self, layout: &StructLayout, purpose: ShapeFor) -> Option<crate::LocalId> {
        let shape = self.emit_shape_call("rask_json_shape_struct", vec![MirOperand::Constant(
            MirConst::Int(layout.size as i64),
        )]);
        for field in &layout.fields {
            // A `@no_serialize` field isn't in the serialized form at all (E19):
            // the decoder never looks for it and always runs its default.
            // A claim's input is what a decode could build, so a field the
            // wire form leaves out takes its default for a claim too.
            let skipped = field_attrs::is_skipped(&field.attrs);
            let fill = self.decode_default_fill(&layout.name, field);
            let (field_shape, flags) = if skipped {
                // E13a rejects a decode of a type with nothing to fill this.
                fill.as_ref()?;
                (MirOperand::Constant(MirConst::Int(0)), FIELD_NOT_SERIALIZED)
            } else {
                (MirOperand::Local(self.emit_shape_for(&field.ty, purpose)?), 0)
            };
            let key = field_attrs::serial_name(&field.attrs, &field.name);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::reading("rask_json_shape_field"),
                args: vec![
                    MirOperand::Local(shape),
                    MirOperand::Constant(MirConst::String(key)),
                    MirOperand::Constant(MirConst::Int(field.offset as i64)),
                    field_shape,
                    MirOperand::Constant(MirConst::Int(flags)),
                    fill.unwrap_or(MirOperand::Constant(MirConst::Int(0))),
                ],
            }));
        }
        Some(shape)
    }

    /// `void (*)(void *slot)` that writes a field's default into its slot, for
    /// the decoder to call when the input leaves the field out. The default
    /// itself is the method desugar wrote (`__decode_default_<field>`), which
    /// monomorphization queued for this decode; `None` when the field has none.
    ///
    /// Running the default as code is what lets it be anything a default can
    /// be. The decoder used to copy in a literal and zero everything else, so
    /// `@default(2 + 3)` made the field required and `st: Status =
    /// Status.Closed` decoded as `Open` (#1528).
    fn decode_default_fill(
        &mut self,
        struct_name: &str,
        field: &rask_layout::FieldLayout,
    ) -> Option<MirOperand> {
        let method = self
            .ctx
            .decode_defaults
            .get(&(struct_name.to_string(), field.name.clone()))?
            .clone();
        let name = format!("{}__decode_fill_{}", self.parent_name, self.closure_counter);
        self.closure_counter += 1;
        let field_ty = self.ctx.type_to_mir(&field.ty);
        let mut wb = crate::builder::BlockBuilder::new(name.clone(), MirType::Void);
        let slot = wb.add_param("__slot".to_string(), MirType::Ptr, rask_ast::ty::ParamMode::Borrow);
        let value = wb.alloc_temp(field_ty.clone());
        wb.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(value),
            func: FunctionRef::internal(method),
            args: vec![],
        }));
        wb.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: slot,
            offset: 0,
            value: MirOperand::Local(value),
            store_size: field.size,
            field: None,
        }));
        wb.terminate(MirTerminator::dummy(MirTerminatorKind::Return { value: None }));
        self.synthesized_functions.push(wb.finish());
        let addr = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: addr,
            rvalue: MirRValue::FuncAddr(name),
        }));
        Some(MirOperand::Local(addr))
    }

    /// The shapes only a claim's inputs use: `char`, 128-bit integers, tuples,
    /// `T or E`, and maps with a key that isn't a string. JSON has no
    /// form for any of them yet (std.json), so a decode never asks.
    fn emit_claim_only_shape(&mut self, ty: &Type) -> Option<crate::LocalId> {
        let int = |v: i64| MirOperand::Constant(MirConst::Int(v));
        let named = |want: &str| matches!(ty, Type::UnresolvedNamed(n) if n == want);
        if matches!(ty, Type::Char) || named("char") {
            return Some(self.emit_shape_call("rask_json_shape_prim", vec![int(JSHAPE_CHAR)]));
        }
        if matches!(ty, Type::I128) || named("i128") {
            return Some(self.emit_shape_call("rask_json_shape_prim", vec![int(JSHAPE_I128)]));
        }
        if matches!(ty, Type::U128) || named("u128") {
            return Some(self.emit_shape_call("rask_json_shape_prim", vec![int(JSHAPE_U128)]));
        }
        // `T or E`, either side. (`T?` is a result with `none` for its error,
        // and JSON reads that one.)
        if let Type::Result { ok, err } = ty {
            if **err != Type::None {
                let ok_shape = self.emit_shape_for(ok, ShapeFor::Claims)?;
                let err_shape = self.emit_shape_for(err, ShapeFor::Claims)?;
                let mir = self.ctx.type_to_mir(ty);
                return Some(self.emit_shape_call("json_shape_result", vec![
                    MirOperand::Local(ok_shape),
                    MirOperand::Local(err_shape),
                    int(rask_layout::size_of(&mir, self.ctx.target()) as i64),
                    int(rask_layout::payload_offset_of(&mir, self.ctx.target()) as i64),
                ]));
            }
        }
        // A map keyed by a primitive. JSON keys are strings; a claim's map
        // can have any key a map compares by its bytes. A key with its own
        // `eq` and `hash` (a struct, an enum) isn't built yet.
        if let Some(key) = self.collection_arg(ty, "Map", 0) {
            if compared_by_bytes(&key, self.ctx.type_defs.target()) {
                let key_shape = self.emit_shape_for(&key, ShapeFor::Claims)?;
                let val = self.collection_arg(ty, "Map", 1)?;
                let val_shape = self.emit_shape_for(&val, ShapeFor::Claims)?;
                let slot = self.slot_size_for_type(&val)?;
                let owned = crate::elem_strs::elem(self.ctx.type_to_mir(&val));
                let map = self.emit_shape_call("rask_json_shape_map", vec![
                    MirOperand::Local(val_shape),
                    int(slot),
                    owned,
                ]);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: None,
                    func: FunctionRef::internal("json_shape_map_key".to_string()),
                    args: vec![MirOperand::Local(map), MirOperand::Local(key_shape)],
                }));
                return Some(map);
            }
        }
        if let Type::Tuple(elems) = ty {
            let MirType::Tuple(fields) = self.ctx.type_to_mir(ty) else { return None };
            let shape = self.emit_shape_call("rask_json_shape_struct", vec![int(rask_layout::size_of(&self.ctx.type_to_mir(ty), self.ctx.target()) as i64)]);
            // Tuple fields sit one after the other, as `debug_render_tuple`
            // and field access read them.
            let mut offset: i64 = 0;
            for (elem, fty) in elems.iter().zip(&fields) {
                let elem_shape = self.emit_shape_for(elem, ShapeFor::Claims)?;
                self.add_shape_field(shape, "", offset, elem_shape);
                offset += rask_layout::size_of(fty, self.ctx.target()) as i64;
            }
            return Some(shape);
        }
        None
    }

    /// The symbol of the `__encode_json` of a type that writes itself out
    /// through its own `encode` (std.encoding/E12b), for a type as the checker
    /// spells it. `None` for any other type.
    fn custom_encode_adapter(&self, ty: &Type) -> Option<String> {
        let name = match ty {
            Type::Named(id) => self.ctx.type_names.get(id)?,
            Type::UnresolvedNamed(name) => name,
            _ => return None,
        };
        self.ctx.custom_encodes.get(name).cloned()
    }

    /// The JSON text of `value`, a `ty` with its own `encode`: its adapter's
    /// answer. Where an encoder would have walked the type's fields, it adds
    /// this instead. `None` when `ty` has no `encode` of its own.
    pub(super) fn custom_encode_text(&mut self, value: MirOperand, ty: &Type) -> Option<crate::LocalId> {
        let adapter = self.custom_encode_adapter(ty)?;
        let text = self.builder.alloc_temp(MirType::Buffer);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(text),
            func: FunctionRef::internal(adapter),
            args: vec![value],
        }));
        Some(text)
    }

    /// A type that decodes through its own `decode` (std.encoding/E12b): the
    /// wire type's shape, and a function the decoder calls with the wire value
    /// and the slot to fill. `None` for any other type; `Some(None)` when the
    /// type has one but its wire type has no shape.
    fn emit_custom_decode_shape(&mut self, ty: &Type, purpose: ShapeFor) -> Option<Option<crate::LocalId>> {
        let name = match ty {
            Type::Named(id) => self.ctx.type_names.get(id)?.clone(),
            Type::UnresolvedNamed(name) => name.clone(),
            _ => return None,
        };
        let custom = self.ctx.custom_decodes.get(&name)?.clone();
        let Some(wire_shape) = self.emit_shape_for(&custom.wire, purpose) else { return Some(None) };
        let ty_mir = self.ctx.type_to_mir(ty);
        let wire_mir = self.ctx.type_to_mir(&custom.wire);
        let string_mir = self.ctx.type_to_mir(&Type::String);
        let result_ty = Type::Result { ok: Box::new(ty.clone()), err: Box::new(Type::String) };
        let result_mir = self.ctx.type_to_mir(&result_ty);

        // `i64 (void *wire, void *dst)`: run the type's adapter on the wire
        // value, store what it made, or hand the runtime the rejection's
        // message. 0 is made, 1 is refused.
        let fn_name = format!("{}__decode_custom_{}", self.parent_name, self.closure_counter);
        self.closure_counter += 1;
        let mut wb = crate::builder::BlockBuilder::new(fn_name.clone(), MirType::I64);
        // Both parameters first: a parameter's local id is its position, so a
        // temp allocated between them would take the second parameter's slot
        // and hand `__dst` a local of the wire's type.
        // An aggregate arrives as its own address, the way any aggregate
        // parameter does; a word is read out of the buffer the runtime points at.
        // An aggregate wire is the trampoline's to hand on: the runtime made it
        // and gives it up here, and the adapter takes it. Left as a borrowed
        // parameter, the adapter's release of its parts was dropped as the
        // trampoline's problem, and a wire struct holding a `Vec` leaked it.
        let wire_param = if wire_mir.passed_by_address() {
            wb.add_param("__wire".to_string(), wire_mir.clone(), ParamMode::Take)
        } else {
            wb.add_param("__wire".to_string(), MirType::Ptr, rask_ast::ty::ParamMode::Borrow)
        };
        let dst = wb.add_param("__dst".to_string(), MirType::Ptr, rask_ast::ty::ParamMode::Borrow);
        let wire = if wire_mir.passed_by_address() {
            wire_param
        } else {
            let wire_ptr = wire_param;
            let v = wb.alloc_temp(wire_mir.clone());
            wb.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: v,
                rvalue: MirRValue::Field {
                    base: MirOperand::Local(wire_ptr),
                    field_index: 0,
                    byte_offset: Some(0),
                    access: FieldAccess::for_field(&wire_mir, rask_layout::size_of(&wire_mir, self.ctx.target())),
                },
            }));
            v
        };
        let result = wb.alloc_temp(result_mir.clone());
        wb.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(result),
            func: FunctionRef::internal(custom.adapter.clone()),
            args: vec![MirOperand::Local(wire)],
        }));
        let tag = wb.alloc_temp(MirType::U8);
        wb.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: tag,
            rvalue: MirRValue::EnumTag { value: MirOperand::Local(result) },
        }));
        let made = wb.create_block();
        let refused = wb.create_block();
        wb.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(tag),
            then_block: refused,
            else_block: made,
        }));
        let payload = |wb: &mut crate::builder::BlockBuilder, arm: &MirType| {
            let value = wb.alloc_temp(arm.clone());
            wb.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: value,
                rvalue: MirRValue::Field {
                    base: MirOperand::Local(result),
                    field_index: 0,
                    byte_offset: Some(rask_layout::payload_offset_of(&result_mir, self.ctx.target())),
                    access: FieldAccess::for_field(arm, rask_layout::size_of(arm, self.ctx.target())),
                },
            }));
            value
        };
        wb.switch_to_block(made);
        let value = payload(&mut wb, &ty_mir);
        wb.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: dst,
            offset: 0,
            value: MirOperand::Local(value),
            store_size: rask_layout::size_of(&ty_mir, self.ctx.target()),
            field: None,
        }));
        wb.terminate(MirTerminator::dummy(MirTerminatorKind::Return {
            value: Some(MirOperand::Constant(MirConst::Int(0))),
        }));
        wb.switch_to_block(refused);
        let message = payload(&mut wb, &string_mir);
        wb.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::internal("json_custom_reject".to_string()),
            args: vec![MirOperand::Local(message)],
        }));
        wb.terminate(MirTerminator::dummy(MirTerminatorKind::Return {
            value: Some(MirOperand::Constant(MirConst::Int(1))),
        }));
        self.synthesized_functions.push(wb.finish());

        let addr = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: addr,
            rvalue: MirRValue::FuncAddr(fn_name),
        }));
        let int = |v: i64| MirOperand::Constant(MirConst::Int(v));
        Some(Some(self.emit_shape_call("rask_json_shape_custom", vec![
            int(rask_layout::size_of(&ty_mir, self.ctx.target()) as i64),
            MirOperand::Local(wire_shape),
            int(rask_layout::size_of(&wire_mir, self.ctx.target()) as i64),
            MirOperand::Local(addr),
            crate::elem_strs::elem(wire_mir.clone()),
        ])))
    }

    /// A type with its own `claims.Generate` (type.claims/IN2): a function
    /// that calls its `generate` and writes the result where the runtime
    /// says, wrapped in a shape. `None` for any other type.
    fn emit_generated_shape(&mut self, ty: &Type) -> Option<crate::LocalId> {
        let Type::Named(id) = ty else { return None };
        let method = self.ctx.claim_generators.get(self.ctx.type_names.get(id)?)?.clone();
        let ty_mir = self.ctx.type_to_mir(ty);
        let choices = self.ctx.type_defs.lang(rask_types::LangItem::Choices)?;
        let choices_mir = self.ctx.type_to_mir(&Type::Named(choices));
        let name = format!("{}__claim_generate_{}", self.parent_name, self.closure_counter);
        self.closure_counter += 1;
        let mut wb = crate::builder::BlockBuilder::new(name.clone(), MirType::Void);
        let slot = wb.add_param("__slot".to_string(), MirType::Ptr, rask_ast::ty::ParamMode::Borrow);
        let from = wb.alloc_temp(choices_mir);
        let value = wb.alloc_temp(ty_mir.clone());
        wb.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(value),
            func: FunctionRef::internal(method),
            args: vec![MirOperand::Local(from)],
        }));
        wb.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: slot,
            offset: 0,
            value: MirOperand::Local(value),
            store_size: rask_layout::size_of(&ty_mir, self.ctx.target()),
            field: None,
        }));
        wb.terminate(MirTerminator::dummy(MirTerminatorKind::Return { value: None }));
        self.synthesized_functions.push(wb.finish());
        let addr = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: addr,
            rvalue: MirRValue::FuncAddr(name),
        }));
        Some(self.emit_shape_call("json_shape_generated", vec![
            MirOperand::Constant(MirConst::Int(rask_layout::size_of(&ty_mir, self.ctx.target()) as i64)),
            MirOperand::Local(addr),
        ]))
    }

    /// A positional field: no key, no flags, no default.
    fn add_shape_field(&mut self, shape: crate::LocalId, key: &str, offset: i64, field_shape: crate::LocalId) {
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::reading("rask_json_shape_field"),
            args: vec![
                MirOperand::Local(shape),
                MirOperand::Constant(MirConst::String(key.to_string())),
                MirOperand::Constant(MirConst::Int(offset)),
                MirOperand::Local(field_shape),
                MirOperand::Constant(MirConst::Int(0)),
                MirOperand::Constant(MirConst::Int(0)),
            ],
        }));
    }

    /// `claims.input<T>(name)` (type.claims): fill a `T` from the claim's
    /// choices, through the shape a decode of `T` would read, and note how it
    /// prints as `name = value` for a counterexample.
    pub(super) fn lower_claim_input(
        &mut self,
        name: &rask_ast::expr::Expr,
        target: &Type,
    ) -> Result<TypedOperand, LoweringError> {
        let (value, target_mir) = self.build_claim_value(target)?;
        let (name_op, _) = self.lower_expr(name)?;
        let shown = self.debug_render_value(&MirOperand::Local(value), &target_mir, Some(target), 0)?;
        let text = self.concat_all(vec![
            name_op,
            MirOperand::Constant(MirConst::String(" = ".to_string())),
            shown,
        ]);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::internal("claims_note_input".to_string()),
            args: vec![text],
        }));
        Ok((MirOperand::Local(value), target_mir))
    }

    /// A `T` built from the claim's choices, through the shape a decode of
    /// `T` would read: what `claims.input<T>` and a generator's
    /// `Choices.make<T>()` both are.
    pub(super) fn build_claim_value(&mut self, target: &Type) -> Result<(crate::LocalId, MirType), LoweringError> {
        let target_mir = self.ctx.type_to_mir(target);
        let Some(shape) = self.emit_shape_for(target, ShapeFor::Claims) else {
            return Err(LoweringError::InvalidConstruct(format!(
                "a claim can't be given a `{}` — its inputs are built the way \
                 json.decode builds a value, plus char, 128-bit integers, tuples, enums, \
                 `T or E` and maps with primitive keys",
                type_label(target)
            )));
        };
        let scalar_dst = !target_mir.passed_by_address();
        let dst = if scalar_dst {
            self.builder.alloc_temp(MirType::Array { elem: Box::new(MirType::U8), len: 16 })
        } else {
            self.builder.alloc_temp(target_mir.clone())
        };
        let dst_size = if scalar_dst { 16 } else { rask_layout::size_of(&target_mir, self.ctx.target()) as i64 };
        for (func, args) in [
            ("rask_json_decode_zero", vec![MirOperand::Local(dst), MirOperand::Constant(MirConst::Int(dst_size))]),
            ("claims_fill", vec![MirOperand::Local(dst), MirOperand::Local(shape)]),
            ("rask_json_shape_free", vec![MirOperand::Local(shape)]),
        ] {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::internal(func.to_string()),
                args,
            }));
        }
        let value = if scalar_dst {
            let v = self.builder.alloc_temp(target_mir.clone());
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: v,
                rvalue: MirRValue::Field {
                    base: MirOperand::Local(dst),
                    field_index: 0,
                    byte_offset: Some(0),
                    access: FieldAccess::for_field(&target_mir, rask_layout::size_of(&target_mir, self.ctx.target())),
                },
            }));
            v
        } else {
            dst
        };
        Ok((value, target_mir))
    }

    fn emit_shape_call(&mut self, name: &str, args: Vec<MirOperand>) -> crate::LocalId {
        let local = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(local),
            func: FunctionRef::reading(name),
            args,
        }));
        local
    }

    /// The Nth generic argument of `Vec<…>` / `Map<…>`, whichever spelling the
    /// checker left behind.
    pub(super) fn collection_arg(&self, ty: &Type, wanted: &str, index: usize) -> Option<Type> {
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
        }
    }

    pub(super) fn struct_layout_of(&self, ty: &Type) -> Option<StructLayout> {
        self.ctx.struct_of(ty).map(|(_, l)| l.clone())
    }

    /// Is `object` a generator's `claims.Choices`?
    pub(super) fn is_claim_choices(&self, object: &rask_ast::expr::Expr) -> bool {
        self.ctx
            .lookup_raw_type(object.id)
            .is_some_and(|t| self.ctx.type_defs.is_lang(&t, rask_types::LangItem::Choices))
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

    /// What T is in `json.decode<T>(…)`.
    ///
    /// The checker's type for the call is the better source — it has already
    /// resolved `T` against the binding it flows into, so `const u: User = try
    /// json.decode(body)` works without the turbofish. The written type
    /// argument is the fallback for when inference left a variable behind.
    pub(super) fn json_decode_target(
        &self,
        expr: &rask_ast::expr::Expr,
        type_args: &Option<Vec<rask_ast::ty::TypeExpr>>,
    ) -> Result<Type, LoweringError> {
        if let Some(Type::Result { ok, .. }) = self.ctx.lookup_raw_type(expr.id).as_deref() {
            if !matches!(**ok, Type::Var(_)) {
                return Ok((**ok).clone());
            }
        }
        if let Some(written) = type_args.as_ref().and_then(|a| a.first()) {
            return Ok(rask_layout::field_type(written));
        }
        Err(LoweringError::InvalidConstruct(
            "json.decode needs to know what to build — write it as \
             `json.decode<Type>(input)` or annotate the binding"
                .to_string(),
        ))
    }
}

/// Who a shape is for. JSON can't read a `char`, a tuple or an enum yet;
/// a claim's inputs can be any of them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ShapeFor {
    Json,
    Claims,
}

fn prim_shape_kind(ty: &Type, target: rask_ast::Target) -> Option<i64> {
    let kind = match &ty.at_target(target) {
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
            "i64" => JSHAPE_I64,
            "isize" => return prim_shape_kind(&Type::Isize, target),
            "u8" => JSHAPE_U8,
            "u16" => JSHAPE_U16,
            "u32" => JSHAPE_U32,
            "u64" => JSHAPE_U64,
            "usize" => return prim_shape_kind(&Type::Usize, target),
            "f32" => JSHAPE_F32,
            "f64" => JSHAPE_F64,
            "string" => JSHAPE_STRING,
            _ => return None,
        },
        _ => return None,
    };
    Some(kind)
}

/// A key a map compares by its bytes (`Map_new`): a number, a `bool`, a
/// `char`. Not a string, which compares by contents.
fn compared_by_bytes(ty: &Type, target: rask_ast::Target) -> bool {
    let named = |n: &str| matches!(ty, Type::UnresolvedNamed(x) if x == n);
    let kind = prim_shape_kind(ty, target);
    (kind.is_some() && kind != Some(JSHAPE_STRING))
        || matches!(ty, Type::Char | Type::I128 | Type::U128)
        || named("char")
        || named("i128")
        || named("u128")
}

fn type_label(ty: &Type) -> String {
    match ty {
        Type::UnresolvedNamed(n) => n.clone(),
        Type::UnresolvedGeneric { name, .. } => name.clone(),
        other => format!("{:?}", other),
    }
}
