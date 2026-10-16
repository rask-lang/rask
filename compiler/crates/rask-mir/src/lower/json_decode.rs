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

use rask_mono::runtime_abi::{
    RASK_JFIELD_NOT_SERIALIZED as FIELD_NOT_SERIALIZED, RASK_JSHAPE_BOOL as JSHAPE_BOOL,
    RASK_JSHAPE_F32 as JSHAPE_F32, RASK_JSHAPE_F64 as JSHAPE_F64, RASK_JSHAPE_I16 as JSHAPE_I16,
    RASK_JSHAPE_I32 as JSHAPE_I32, RASK_JSHAPE_I64 as JSHAPE_I64, RASK_JSHAPE_I8 as JSHAPE_I8,
    RASK_JSHAPE_STRING as JSHAPE_STRING, RASK_JSHAPE_U16 as JSHAPE_U16,
    RASK_JSHAPE_U32 as JSHAPE_U32, RASK_JSHAPE_U64 as JSHAPE_U64, RASK_JSHAPE_U8 as JSHAPE_U8,
    RASK_JSON_ERR_MISSING as ERR_MISSING, RASK_JSON_ERR_PARSE as ERR_PARSE,
    RASK_JSON_ERR_TYPE as ERR_TYPE,
};

impl<'a> MirLowerer<'a> {
    /// Expand `json.decode<T>(input)` into shape construction + one decode call
    /// + a `T or JsonError` result.
    pub(super) fn lower_json_decode(
        &mut self,
        input: MirOperand,
        target: &Type,
    ) -> Result<TypedOperand, LoweringError> {
        let target_mir = self.ctx.type_to_mir(target);
        // The same type as a wrapper's payload: a `Vec<T>` target keeps what it
        // is there, so releasing the result frees the vector the decoder built.
        let payload_mir = self.ctx.payload_to_mir(target);

        // `decode<JsonValue>` is the untyped path — that's exactly what
        // `json.parse` already does, in Rask, so call it instead of teaching the
        // shape decoder how to build the enum a second time.
        if self.type_name_of(target).as_deref() == Some("JsonValue") {
            let err_ty = self
                .ctx
                .find_enum("JsonError")
                .map(|(idx, l)| MirType::Enum(EnumLayoutId::new(idx, l.size, l.align)))
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
            // The kind is on the local now; hand the caller the plain type, the
            // way every other lowering does (`MirType::Container`).
            return Ok((MirOperand::Local(out), result_ty.without_container_kinds()));
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
            field: None,
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
            field: None,
        }));
        self.builder
            .terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                target: done_block,
            }));

        self.builder.switch_to_block(done_block);
        Ok((MirOperand::Local(result), result_ty.without_container_kinds()))
    }

    /// Result tag plus the two ER15 origin words. Decode doesn't attribute an
    /// origin — the `try` site fills that in when it propagates.
    fn store_result_header(&mut self, result: crate::LocalId, tag: i64) {
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result,
            offset: crate::types::RESULT_TAG_OFFSET,
            value: MirOperand::Constant(MirConst::Int(tag)),
            store_size: Some(8),
            field: None,
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
                field: None,
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
                field: None,
            }));
            return err_val;
        }

        let (tag_offset, tag_bytes) = self
            .ctx
            .find_enum("JsonError")
            .map(|(_, l)| (l.tag_offset, l.tag_bytes()))
            .unwrap_or((0, 1));

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
                store_size: Some(tag_bytes),
                field: None,
            }));
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: err_val,
                offset: *payload_offset,
                value: MirOperand::Local(msg),
                store_size: Some(16),
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
            let owned = crate::elem_strs::elem(self.ctx.payload_to_mir(&elem));
            return Some(self.emit_shape_call("json_shape_vec", vec![
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
            let val_shape = self.emit_shape(&val)?;
            let slot = self.slot_size_for_type(&val).unwrap_or(8);
            let owned = crate::elem_strs::elem(self.ctx.payload_to_mir(&val));
            return Some(self.emit_shape_call("json_shape_map", vec![
                MirOperand::Local(val_shape),
                MirOperand::Constant(MirConst::Int(slot)),
                owned,
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
            // A `@no_serialize` field isn't in the serialized form at all (E19):
            // the decoder never looks for it and always runs its default.
            let skipped = field_attrs::is_skipped(&field.attrs);
            let fill = self.decode_default_fill(&layout.name, field);
            let (field_shape, flags) = if skipped {
                // E13a rejects a decode of a type with nothing to fill this.
                fill.as_ref()?;
                (MirOperand::Constant(MirConst::Int(0)), FIELD_NOT_SERIALIZED)
            } else {
                (MirOperand::Local(self.emit_shape(&field.ty)?), 0)
            };
            let key = field_attrs::serial_name(&field.attrs, &field.name);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::internal("json_shape_field".to_string()),
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
        field: &rask_mono::FieldLayout,
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
        let slot = wb.add_param("__slot".to_string(), MirType::Ptr);
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
            store_size: Some(field.size),
            field: None,
        }));
        wb.terminate(MirTerminator::dummy(MirTerminatorKind::Return { value: None }));
        self.func_sigs.insert(name.clone(), super::FuncSig {
            ret_ty: MirType::Void,
            scalar_mutate_params: Vec::new(),
            aggregate_mutate_params: Vec::new(),
            ret_vec_elem: None,
            param_tys: Vec::new(),
        });
        self.synthesized_functions.push(wb.finish());
        let addr = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: addr,
            rvalue: MirRValue::FuncAddr(name),
        }));
        Some(MirOperand::Local(addr))
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
                field: None,
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
        type_args: &Option<Vec<rask_ast::ty::TypeExpr>>,
    ) -> Result<Type, LoweringError> {
        if let Some(Type::Result { ok, .. }) = self.ctx.lookup_raw_type(expr.id) {
            if !matches!(**ok, Type::Var(_)) {
                return Ok((**ok).clone());
            }
        }
        if let Some(written) = type_args.as_ref().and_then(|a| a.first()) {
            return Ok(rask_mono::field_type(written));
        }
        Err(LoweringError::InvalidConstruct(
            "json.decode needs to know what to build — write it as \
             `json.decode<Type>(input)` or annotate the binding"
                .to_string(),
        ))
    }
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
            "i64" => JSHAPE_I64,
            "isize" => if rask_ast::primitives::pointer_bits() == 32 { JSHAPE_I32 } else { JSHAPE_I64 },
            "u8" => JSHAPE_U8,
            "u16" => JSHAPE_U16,
            "u32" => JSHAPE_U32,
            "u64" => JSHAPE_U64,
            "usize" => if rask_ast::primitives::pointer_bits() == 32 { JSHAPE_U32 } else { JSHAPE_U64 },
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
