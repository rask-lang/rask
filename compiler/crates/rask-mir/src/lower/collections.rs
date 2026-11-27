// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Collection construction and cloning lowering:
//! Vec.from, Map.from, JSON encode/decode, enum clone.

use crate::FieldAccess;
use super::{LoweringError, MirLowerer, TypedOperand};
use crate::{
    operand::MirConst, FunctionRef, MirOperand, MirRValue,
    MirStmt, MirStmtKind, MirTerminator, MirTerminatorKind, MirType,
};
use rask_ast::expr::{Expr, ExprKind};
use rask_layout::StructLayout;

/// Where `emit_json_value` puts what it encodes: under a key in an object
/// being built, or as the next element of an array.
pub(super) enum JsonSink {
    /// An object member. The key is a string operand: a field's name, or a
    /// Map's key read at run time.
    Field { buf: crate::LocalId, key: MirOperand },
    Element { arr: crate::LocalId },
    /// The whole of `json.encode`'s answer, written into `dst`.
    Text { dst: crate::LocalId },
}

#[derive(Clone, Copy)]
enum JsonScalar {
    String,
    Bool,
    F64,
    I64,
}

impl<'a> MirLowerer<'a> {
    /// Vec.from([a, b, c]) → store elements into stack array, call rask_vec_from_static
    pub(super) fn lower_vec_from_array(
        &mut self,
        elems: &[Expr],
    ) -> Result<TypedOperand, LoweringError> {
        self.lower_vec_from_array_with(elems, None, None)
    }

    /// `lower_vec_from_array`, told what the elements are.
    ///
    /// The first element's own lowered type is a guess: it can't see that the
    /// slot wants a `T?`, so `[1, none, 3]` into a `Vec<i64?>` built 8-byte slots
    /// and dropped every tag. The checker knows, so it says.
    pub(super) fn lower_vec_from_array_with(
        &mut self,
        elems: &[Expr],
        elem_hint: Option<MirType>,
        literal_id: Option<rask_ast::NodeId>,
    ) -> Result<TypedOperand, LoweringError> {
        let mut lowered = Vec::new();
        for elem in elems {
            lowered.push(self.lower_expr(elem)?);
        }
        // The element type is the checker's: the first element's own lowered
        // type is a guess. It can't see that the slot wants a `T?`, so
        // `[1, none, 3]` into a `Vec<i64?>` built 8-byte slots and dropped
        // every tag; and an unsuffixed literal lowers as `i32` where the vector
        // holds `i64`s, which put four-byte elements where eight were read
        // (#461).
        let elem_ty = elem_hint
            .or_else(|| literal_id.and_then(|id| self.container_elem_mir_type(id, 0)))
            .or_else(|| {
                let first = elems.first()?;
                let raw = self.ctx.lookup_raw_type(first.id)?.clone();
                match &*raw {
                    rask_types::Type::Var(_) => None,
                    resolved => Some(self.ctx.type_to_mir(&resolved)),
                }
            })
            .or_else(|| lowered.first().map(|(_, ty)| ty.clone()))
            .unwrap_or(MirType::I64);
        // A bare `T` filling a `T?` slot gets its layers here, same as an array
        // literal's elements and a struct field's value; a scalar is converted
        // to the element's own type.
        let lowered: Vec<MirOperand> = lowered
            .into_iter()
            .map(|(op, val_ty)| {
                let op = self.wrap_collection_element(&elem_ty, &val_ty, op);
                self.to_slot_width(op, &elem_ty)
            })
            .collect();
        let elem_size = rask_layout::size_of(&elem_ty, self.ctx.target());
        let array_ty = MirType::Array {
            elem: Box::new(elem_ty.clone()),
            len: elems.len() as u32,
        };
        let arr_local = self.builder.alloc_temp(array_ty);
        // Every element at its own size. Without one, codegen stored a string
        // constant's address in the slot where its 16 bytes belonged, and
        // `Vec.from(["ab", "cde"])` reported len 15 for every element (#508).
        for (i, op) in lowered.into_iter().enumerate() {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: arr_local,
                offset: i as u32 * elem_size,
                value: op,
                store_size: elem_size,
                field: None,
            }));
        }

        // What the elements are, for the vector to give them back — and
        // anything pushed onto it later too. The checker's type for the
        // literal says it; `[]` has nothing else to read it off, so
        // `mut fns: Vec<func(i64) -> i64> = []` relies on it. Failing that,
        // the first element's own checked type: an element that is a
        // container is a `Ptr` in MIR like every other pointer, so `[a, b]`
        // being a `Vec<Vec<i32>>` is only known there.
        let elem_desc = literal_id
            .and_then(|id| self.container_elem_mir_type(id, 0))
            .or_else(|| {
                let first = elems.first()?;
                let raw = self.ctx.lookup_raw_type(first.id)?.clone();
                Some(self.ctx.type_to_mir(&raw))
            })
            .unwrap_or_else(|| elem_ty.clone());

        // The handle is an address, and the binding that takes it over says
        // so. Typed `i64` here, an unannotated `let v = Vec.from([1, 2])` was
        // an integer local and `{v:debug}` printed the buffer's address.
        let vec_local = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(vec_local),
            func: FunctionRef::keeping("rask_vec_from_static", &[0]).handing_back("rask_vec_free"),
            args: vec![
                MirOperand::Local(arr_local),
                MirOperand::Constant(MirConst::Int(elems.len() as i64)),
                MirOperand::Constant(MirConst::Int(elem_size as i64)),
                crate::elem_strs::elem(elem_desc),
            ],
        }));
        Ok((MirOperand::Local(vec_local), MirType::Ptr))
    }

    /// Map.from([(k, v), ...]) → Map.new() + Map.insert() per pair.
    ///
    /// `call` is the `Map.from(...)` expression and `written` the receiver's
    /// type arguments, because between them they say what the key and value
    /// types are — and the constructor needs to be told.
    ///
    /// It used to be told nothing: the call went out with no arguments and
    /// codegen filled in its generic defaults, one machine word per slot. A
    /// `string` key is sixteen bytes, so `Map.from([("k", 42)])` stored half a
    /// string in the key slot and hashed it as a word. Every later lookup
    /// missed — the map had the entry and could not find it (#812 is the same
    /// bug on `Map.new`, fixed there and not here).
    ///
    /// `arg` is a literal of pairs or an array held somewhere. A held array
    /// keeps its own entries, so the map takes a copy of each
    /// (`rask_map_insert_pairs`).
    pub(super) fn lower_map_from_pairs(
        &mut self,
        call: &Expr,
        arg: &Expr,
    ) -> Result<TypedOperand, LoweringError> {
        let elems: &[Expr] = match &arg.kind {
            ExprKind::Array(elems) => elems,
            _ => &[],
        };
        // The checker knows the map's own type. The first pair is the last
        // resort.
        let arg_ty = |i: usize| -> Option<MirType> {
            self.container_elem_mir_type(call.id, i)
        };
        let pair_ty = |i: usize| -> Option<MirType> {
            elems.first().and_then(|e| match &e.kind {
                ExprKind::Tuple(parts) if parts.len() == 2 => {
                    self.ctx.lookup_node_type(parts[i].id)
                }
                _ => None,
            })
        };

        let key_ty = arg_ty(0).or_else(|| pair_ty(0)).unwrap_or(MirType::I64);
        let val_ty = arg_ty(1).or_else(|| pair_ty(1)).unwrap_or(MirType::I64);

        let (ctor, args) = self.internal_ctor("Map_new", &[key_ty.clone(), val_ty.clone()], Some(call.id))?;
        let map_local = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(map_local),
            func: ctor,
            args,
        }));

        if !matches!(arg.kind, ExprKind::Array(_)) {
            let (arr_op, arr_ty) = self.lower_expr(arg)?;
            let MirType::Array { elem, len } = &arr_ty else {
                return Err(LoweringError::InvalidConstruct(format!(
                    "Map.from takes a fixed array of pairs, and this argument lowered as {arr_ty:?}"
                )));
            };
            let MirType::Tuple(fields) = elem.as_ref() else {
                return Err(LoweringError::InvalidConstruct(format!(
                    "Map.from takes `(key, value)` pairs, and these lowered as {elem:?}"
                )));
            };
            let mut args = self.map_entries_args(MirOperand::Local(map_local), &fields[0], &fields[1]);
            args.insert(1, arr_op);
            args.insert(2, MirOperand::Constant(MirConst::Int(*len as i64)));
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::reading("rask_map_insert_pairs"),
                args,
            }));
            return Ok((MirOperand::Local(map_local), MirType::Ptr));
        }

        for elem in elems {
            let (key_op, val_op) = match &elem.kind {
                ExprKind::Tuple(parts) if parts.len() == 2 => {
                    let (k, k_ty) = self.lower_expr(&parts[0])?;
                    let (v, v_ty) = self.lower_expr(&parts[1])?;
                    // The map holds each at its own width, so a value goes in
                    // as the key or value type it is stored as.
                    let k = self.wrap_collection_element(&key_ty, &k_ty, k);
                    let v = self.wrap_collection_element(&val_ty, &v_ty, v);
                    (self.as_slot_value(k, &key_ty), self.as_slot_value(v, &val_ty))
                }
                _ => {
                    let _ = self.lower_expr(elem)?;
                    continue;
                }
            };
            // `Map_set`, not `Map_insert`: nothing takes the value a repeated
            // key displaces, so the map has to release it.
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: crate::runtime_ops::map_set(),
                args: vec![MirOperand::Local(map_local), key_op, val_op],
            }));
        }

        Ok((MirOperand::Local(map_local), MirType::Ptr))
    }

    /// Expand `json.encode(struct_val)` into a sequence of json_buf_* calls.
    /// Re-indent an encoded JSON string when the call was `encode_pretty`.
    ///
    /// The struct and Vec encoders build text directly, so there's no value
    /// tree to hand a pretty printer — the indentation goes on afterwards, to
    /// the same shape `JsonValue.to_string_pretty` writes (#847).
    pub(super) fn maybe_json_pretty(
        &mut self,
        encoded: TypedOperand,
        pretty: bool,
    ) -> TypedOperand {
        if !pretty {
            return encoded;
        }
        let (op, _) = encoded;
        let dst = self.builder.alloc_temp(MirType::Buffer);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(dst),
            func: FunctionRef::reading("rask_json_pretty").writing_string(),
            args: vec![op],
        }));
        (MirOperand::Local(dst), MirType::Buffer)
    }

    pub(super) fn lower_json_encode_struct(
        &mut self,
        struct_op: MirOperand,
        layout: StructLayout,
    ) -> Result<TypedOperand, LoweringError> {
        let buf = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(buf),
            func: FunctionRef::reading("rask_json_buf_new"),
            args: vec![],
        }));

        for (idx, field) in layout.fields_in_declaration_order() {
            // `@skip` keeps a field out of the serialized form (std.encoding/E19).
            if rask_ast::decl::field_attrs::is_skipped(&field.attrs) {
                continue;
            }
            // `@rename` overrides the key; otherwise it's the field's own name (E18).
            let key = rask_ast::decl::field_attrs::serial_name(&field.attrs, &field.name);
            // Hold the field in a local of its own type. An I64 temp made the
            // f64 load convert *numerically* on the way in, so 0.25 encoded as
            // 0 and 8.0 as 8 (#478).
            let field_val = self.builder.alloc_temp(self.ctx.type_to_mir(&field.ty));
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: field_val,
                rvalue: MirRValue::Field {
                    base: struct_op.clone(),
                    field_index: idx as u32,
                    byte_offset: None,
                    access: FieldAccess::Word,
                },
            }));

            let key = MirOperand::Constant(MirConst::String(key));
            self.emit_json_value(&JsonSink::Field { buf, key }, field_val, Some(&field.ty))?;
        }

        // The StringOutParam adapter writes a 16-byte RaskStr and hands back its
        // address, so this is a string — typing it I64 made `let j =
        // json.encode(v)` print a pointer (#478).
        let result = self.builder.alloc_temp(MirType::Buffer);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(result),
            func: FunctionRef::releasing("rask_json_buf_finish").writing_string(),
            args: vec![MirOperand::Local(buf)],
        }));

        Ok((MirOperand::Local(result), MirType::Buffer))
    }

    /// Encode one already-loaded value into a json buffer slot.
    ///
    /// The one place that decides how a value's type is written: struct
    /// fields, enum payloads, a `T?`'s payload and Vec elements all come here.
    /// They used to keep separate lists of shapes, and the Vec element's was
    /// short: a `Vec<Vec<T>>`, `Vec<T?>` or `Vec<enum>` element fell to the
    /// integer encoder and wrote its address (#1956).
    ///
    /// `ty` is the checker's type. Without one (a `Vec.new()` filled by
    /// `push` can leave a variable), the local's lowered type decides.
    pub(super) fn emit_json_value(
        &mut self,
        sink: &JsonSink,
        value: crate::LocalId,
        ty: Option<&rask_types::Type>,
    ) -> Result<(), LoweringError> {
        use rask_types::Type;
        let Some(ty) = ty.filter(|t| !matches!(t, Type::Var(_))) else {
            return self.emit_json_value_by_mir(sink, value);
        };

        // A type with its own `encode` is written as what that answers
        // (std.encoding/E12b), not as its fields.
        if let Some(text) = self.custom_encode_text(MirOperand::Local(value), ty) {
            self.emit_json_raw(sink, MirOperand::Local(text));
            return Ok(());
        }

        if self.ctx.type_defs.is_lang(ty, rask_types::LangItem::JsonValue) {
            let text = self.json_value_text(MirOperand::Local(value), false)?;
            self.emit_json_raw(sink, MirOperand::Local(text));
            return Ok(());
        }

        if let Some(elem) = self.collection_arg(ty, "Vec", 0) {
            let elem_mir = self.ctx.type_to_mir(&elem);
            let (text, _) =
                self.lower_json_encode_vec(MirOperand::Local(value), Some(elem), Some(elem_mir))?;
            self.emit_json_raw(sink, text);
            return Ok(());
        }

        if let (Some(key), Some(val)) = (self.collection_arg(ty, "Map", 0), self.collection_arg(ty, "Map", 1)) {
            let text = self.lower_json_encode_map(MirOperand::Local(value), &key, &val)?;
            self.emit_json_raw(sink, MirOperand::Local(text));
            return Ok(());
        }

        if let Type::Array { elem, len: rask_types::ArrayLen::Known(len) } = ty {
            let text = self.lower_json_encode_array(value, elem, *len)?;
            self.emit_json_raw(sink, MirOperand::Local(text));
            return Ok(());
        }

        if let Type::Result { ok, err } = ty {
            if **err == Type::None {
                return self.emit_json_optional(sink, value, ok);
            }
        }

        if let Some(layout) = self.struct_layout_of(ty) {
            let (text, _) = self.lower_json_encode_struct(MirOperand::Local(value), layout)?;
            self.emit_json_raw(sink, text);
            return Ok(());
        }

        // E22: an enum is its variant's name, or an object keyed by it.
        if let Some(layout) = self.ctx.enum_of(ty).map(|(_, l)| l.clone()) {
            let (text, _) = self.lower_json_encode_enum(MirOperand::Local(value), &layout)?;
            self.emit_json_raw(sink, text);
            return Ok(());
        }

        let scalar = match ty {
            _ if self.ctx.is_text(ty) => JsonScalar::String,
            Type::Bool => JsonScalar::Bool,
            Type::F32 | Type::F64 => JsonScalar::F64,
            _ => JsonScalar::I64,
        };
        self.emit_json_scalar(sink, value, scalar);
        Ok(())
    }

    /// `emit_json_value` for a value whose checked type isn't known.
    fn emit_json_value_by_mir(
        &mut self,
        sink: &JsonSink,
        value: crate::LocalId,
    ) -> Result<(), LoweringError> {
        let mir = self.builder.local_type(value);
        match &mir {
            Some(MirType::Struct(crate::types::StructLayoutId { id, .. })) => {
                if let Some(layout) = self.ctx.struct_layouts.get(*id as usize).cloned() {
                    let (text, _) = self.lower_json_encode_struct(MirOperand::Local(value), layout)?;
                    self.emit_json_raw(sink, text);
                    return Ok(());
                }
            }
            Some(MirType::Enum(crate::types::EnumLayoutId { id, .. })) => {
                if let Some(layout) = self.ctx.enum_layouts.get(*id as usize).cloned() {
                    let (text, _) = self.lower_json_encode_enum(MirOperand::Local(value), &layout)?;
                    self.emit_json_raw(sink, text);
                    return Ok(());
                }
            }
            _ => {}
        }
        let scalar = match mir {
            Some(MirType::Buffer) => JsonScalar::String,
            Some(MirType::Bool) => JsonScalar::Bool,
            Some(MirType::F32 | MirType::F64) => JsonScalar::F64,
            _ => JsonScalar::I64,
        };
        self.emit_json_scalar(sink, value, scalar);
        Ok(())
    }

    /// A JsonValue's text, from its Rask encoder (`stringify_value` in
    /// stdlib/json.rk, reached through `JsonValue.to_string`).
    pub(super) fn json_value_text(&mut self, value: MirOperand, pretty: bool) -> Result<crate::LocalId, LoweringError> {
        let body = self
            .ctx
            .symbols
            .json_value_encoder(pretty, self.ctx.type_defs)
            .map(|c| c.symbol)
            .ok_or_else(|| {
                LoweringError::InvalidConstruct(
                    "json.encode on a JsonValue, but JsonValue has no encoder in Rask".to_string(),
                )
            })?;
        let text = self.builder.alloc_temp(MirType::Buffer);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(text),
            func: FunctionRef::internal(body),
            args: vec![value],
        }));
        Ok(text)
    }

    /// Put already-encoded JSON text in the slot.
    fn emit_json_raw(&mut self, sink: &JsonSink, text: MirOperand) {
        let (func, args) = match sink {
            JsonSink::Field { buf, key } => (
                "rask_json_buf_add_raw",
                vec![MirOperand::Local(*buf), key.clone(), text],
            ),
            JsonSink::Element { arr } => {
                ("rask_json_buf_array_add_raw", vec![MirOperand::Local(*arr), text])
            }
            JsonSink::Text { dst } => {
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: *dst,
                    rvalue: MirRValue::Use(text),
                }));
                return;
            }
        };
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::reading(func),
            args,
        }));
    }

    /// A string, bool or number, written by the runtime's own formatter for
    /// the slot. A narrower number is converted by its own signedness, and an
    /// f32 is written as the double it widens to.
    fn emit_json_scalar(&mut self, sink: &JsonSink, value: crate::LocalId, scalar: JsonScalar) {
        let value = match scalar {
            JsonScalar::I64 => self.to_slot_width(MirOperand::Local(value), &MirType::I64),
            JsonScalar::F64 => self.to_slot_width(MirOperand::Local(value), &MirType::F64),
            JsonScalar::String | JsonScalar::Bool => MirOperand::Local(value),
        };
        let kind = match scalar {
            JsonScalar::String => "string",
            JsonScalar::Bool => "bool",
            JsonScalar::F64 => "f64",
            JsonScalar::I64 => "i64",
        };
        let (func, args) = match sink {
            JsonSink::Field { buf, key } => (
                format!("rask_json_buf_add_{kind}"),
                vec![MirOperand::Local(*buf), key.clone(), value],
            ),
            JsonSink::Element { arr } => {
                (format!("rask_json_buf_array_add_{kind}"), vec![MirOperand::Local(*arr), value])
            }
            JsonSink::Text { dst } => {
                // The same formatters a member or an element gets, writing
                // a string of their own.
                let helper = match scalar {
                    JsonScalar::String => "rask_json_encode_string",
                    JsonScalar::Bool => "rask_bool_to_string",
                    JsonScalar::F64 => "rask_f64_to_string",
                    JsonScalar::I64 => "rask_json_encode_i64",
                };
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: Some(*dst),
                    func: FunctionRef::borrowing(helper).writing_string(),
                    args: vec![value],
                }));
                return;
            }
        };
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::reading(&func),
            args,
        }));
    }

    /// Encode an enum value as JSON (std.encoding/E22-E25).
    ///
    /// A chain of tag comparisons rather than a jump table: an enum small
    /// enough to be a struct field, or to be the whole argument to
    /// `json.encode()`, has a handful of variants, and this keeps the emitted
    /// MIR to shapes every backend already handles.
    ///
    /// Default (no `@tag` on the enum) is external tagging: a unit variant is
    /// its own name as a bare JSON string (`"Point"`); a variant with one
    /// unnamed field (`Circle(f64)`) puts the payload directly under the
    /// variant name (`{"Circle": 1.0}`, E23); anything else (named fields, or
    /// several unnamed ones) nests an object under the variant name
    /// (`{"Circle": {"radius": 1.0}}`, E22). `@tag("field")` switches to
    /// internal tagging: the tag goes in `field` inside the same object the
    /// payload's fields flatten into (`{"type": "Click", "x": 10}`, E24) —
    /// there's no field to flatten an unnamed payload into, so that
    /// combination panics naming what's missing rather than inventing a key.
    /// `@rename` on a variant overrides its serialized name either way (E25).
    pub(super) fn lower_json_encode_enum(
        &mut self,
        enum_op: MirOperand,
        layout: &rask_layout::EnumLayout,
    ) -> Result<TypedOperand, LoweringError> {
        let tag_field = rask_ast::decl::field_attrs::tag_field(&layout.attrs);

        let tag = self.builder.alloc_temp(MirType::U8);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: tag,
            rvalue: MirRValue::EnumTag { value: enum_op.clone() },
        }));

        let result = self.builder.alloc_temp(MirType::Buffer);
        let done = self.builder.create_block();
        for variant in &layout.variants {
            let hit = self.builder.create_block();
            let miss = self.builder.create_block();
            let eq = self.builder.alloc_temp(MirType::Bool);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: eq,
                rvalue: MirRValue::BinaryOp {
                    op: crate::operand::BinOp::Eq,
                    left: MirOperand::Local(tag),
                    right: MirOperand::Constant(MirConst::Int(variant.tag as i64)),
                },
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                cond: MirOperand::Local(eq),
                then_block: hit,
                else_block: miss,
            }));

            self.builder.switch_to_block(hit);
            let variant_name = rask_ast::decl::field_attrs::serial_name(&variant.attrs, &variant.name);
            let is_single_unnamed = variant.fields.len() == 1 && rask_ast::decl::is_positional_field_name(&variant.fields[0].name);

            if variant.fields.is_empty() {
                match &tag_field {
                    None => {
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                            dst: Some(result),
                            func: FunctionRef::reading("rask_json_encode_string").writing_string(),
                            args: vec![MirOperand::Constant(MirConst::String(variant_name))],
                        }));
                    }
                    Some(tf) => {
                        let buf = self.new_json_buf();
                        self.push_json_add_string(buf, tf, MirOperand::Constant(MirConst::String(variant_name)));
                        self.push_json_finish(buf, result);
                    }
                }
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: done }));
            } else if is_single_unnamed && tag_field.is_none() {
                // E23: the payload goes directly under the variant name.
                let field = &variant.fields[0];
                let field_val = self.load_variant_field(enum_op.clone(), variant, field, 0);
                let buf = self.new_json_buf();
                let key = MirOperand::Constant(MirConst::String(variant_name.clone()));
                self.emit_json_value(&JsonSink::Field { buf, key }, field_val, Some(&field.ty))?;
                self.push_json_finish(buf, result);
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: done }));
            } else if is_single_unnamed {
                // Both @tag shapes below are rejected at the declaration
                // (E0841/E0842), so reaching them means the check was skipped,
                // not that a user wrote something new. Fail the compile rather
                // than emit a panic into the program (ctrl.panic/S7).
                return Err(LoweringError::InvalidConstruct(format!(
                    "`@tag` on `{}.{}`, whose payload is unnamed — should have been rejected as E0841",
                    layout.name, variant.name,
                )));
            } else if tag_field.as_deref().is_some_and(|tf| variant.fields.iter().any(|f| f.name == tf)) {
                return Err(LoweringError::InvalidConstruct(format!(
                    "`@tag(\"{}\")` on `{}.{}` collides with a payload field — should have been rejected as E0842",
                    tag_field.as_deref().unwrap(), layout.name, variant.name,
                )));
            } else {
                // E22 (external) or E24 (internal): a struct-shaped payload.
                let buf = self.new_json_buf();
                if let Some(tf) = &tag_field {
                    self.push_json_add_string(buf, tf, MirOperand::Constant(MirConst::String(variant_name.clone())));
                }
                for (i, field) in variant.fields.iter().enumerate() {
                    let field_val = self.load_variant_field(enum_op.clone(), variant, field, i as u32);
                    let key = MirOperand::Constant(MirConst::String(field.name.clone()));
                    self.emit_json_value(&JsonSink::Field { buf, key }, field_val, Some(&field.ty))?;
                }
                if tag_field.is_none() {
                    let inner = self.builder.alloc_temp(MirType::Buffer);
                    self.push_json_finish(buf, inner);
                    let outer = self.new_json_buf();
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::reading("rask_json_buf_add_raw"),
                        args: vec![
                            MirOperand::Local(outer),
                            MirOperand::Constant(MirConst::String(variant_name)),
                            MirOperand::Local(inner),
                        ],
                    }));
                    self.push_json_finish(outer, result);
                } else {
                    self.push_json_finish(buf, result);
                }
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: done }));
            }

            self.builder.switch_to_block(miss);
        }
        // No variant matched, which a well-formed value can't do. A jump to
        // `done` here reached it with `result` unset: inside a Vec's loop the
        // drop pass took that path as carrying last turn's string through, and
        // freed it again — or, on the first turn, freed stack garbage.
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Unreachable));
        self.builder.switch_to_block(done);
        Ok((MirOperand::Local(result), MirType::Buffer))
    }

    fn new_json_buf(&mut self) -> crate::LocalId {
        let buf = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(buf),
            func: FunctionRef::reading("rask_json_buf_new"),
            args: vec![],
        }));
        buf
    }

    fn push_json_add_string(&mut self, buf: crate::LocalId, key: &str, value: MirOperand) {
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::reading("rask_json_buf_add_string"),
            args: vec![MirOperand::Local(buf), MirOperand::Constant(MirConst::String(key.to_string())), value],
        }));
    }

    fn push_json_finish(&mut self, buf: crate::LocalId, dst: crate::LocalId) {
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(dst),
            func: FunctionRef::releasing("rask_json_buf_finish").writing_string(),
            args: vec![MirOperand::Local(buf)],
        }));
    }

    /// Load one field out of an enum's matched-variant payload, at its exact
    /// offset — same mechanism `match_lower.rs` uses to bind a payload
    /// pattern, since this is the same operation: read a named slot out of
    /// whichever variant the tag already picked.
    fn load_variant_field(
        &mut self,
        enum_op: MirOperand,
        variant: &rask_layout::VariantLayout,
        field: &rask_layout::FieldLayout,
        field_index: u32,
    ) -> crate::LocalId {
        let field_mir_ty = self.ctx.type_to_mir(&field.ty);
        let field_val = self.builder.alloc_temp(field_mir_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: field_val,
            rvalue: MirRValue::Field {
                base: enum_op,
                field_index,
                byte_offset: Some(variant.payload_offset + field.offset),
                access: FieldAccess::for_field(&field_mir_ty, field.size),
            },
        }));
        field_val
    }

    /// A `T?`: the payload, encoded as its own type, when present; `null`
    /// when not.
    fn emit_json_optional(
        &mut self,
        sink: &JsonSink,
        opt_local: crate::LocalId,
        payload_decl: &rask_types::Type,
    ) -> Result<(), LoweringError> {
        let opt_ty = self.builder.local_type(opt_local).unwrap_or_else(|| crate::fallback::unknown_type("lower/collections:json_optional"));
        let payload_ty = Self::payload_of_mir(&opt_ty).unwrap_or_else(|| crate::fallback::unknown_type("lower/collections:json_optional_payload"));

        let tag = self.builder.alloc_temp(MirType::U8);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: tag,
            rvalue: MirRValue::EnumTag { value: MirOperand::Local(opt_local) },
        }));
        let is_some = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: is_some,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Eq,
                left: MirOperand::Local(tag),
                right: MirOperand::Constant(MirConst::Int(0)),
            },
        }));

        let some_block = self.builder.create_block();
        let none_block = self.builder.create_block();
        let done_block = self.builder.create_block();
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(is_some),
            then_block: some_block,
            else_block: none_block,
        }));

        self.builder.switch_to_block(some_block);
        let payload = self.emit_option_payload(MirOperand::Local(opt_local), payload_ty, false);
        self.emit_json_value(sink, payload, Some(payload_decl))?;
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: done_block }));

        self.builder.switch_to_block(none_block);
        self.emit_json_raw(sink, MirOperand::Constant(MirConst::String("null".to_string())));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: done_block }));

        self.builder.switch_to_block(done_block);
        Ok(())
    }

    /// A `Map<string, V>` as a JSON object, one member per entry in the
    /// map's own order. The keys are only known once the map is walked, so
    /// each member's key is the entry's key read at run time.
    ///
    /// The runtime walked maps from a shape of its own, which had no enums:
    /// a `Map<string, Shape>` read a variant's payload as a pointer and
    /// crashed. Walking the entries here hands each value to the encoder
    /// every other value goes through.
    fn lower_json_encode_map(
        &mut self,
        map_op: MirOperand,
        key_ty: &rask_types::Type,
        val_ty: &rask_types::Type,
    ) -> Result<crate::LocalId, LoweringError> {
        let key_mir = self.ctx.type_to_mir(key_ty);
        let val_mir = self.ctx.type_to_mir(val_ty);
        let buf = self.new_json_buf();
        let entries = self.emit_map_entries(map_op, &key_mir, &val_mir);
        let pair = vec![key_mir.clone(), val_mir.clone()];
        let offsets = rask_layout::tuple_offsets(&pair, self.ctx.target());
        let len = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(len),
            func: FunctionRef::internal("Vec_len".to_string()),
            args: vec![MirOperand::Local(entries)],
        }));
        self.emit_json_count_loop(MirOperand::Local(len), |this, idx| {
            let entry = this.builder.alloc_temp(MirType::Tuple(pair.clone()));
            this.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(entry),
                func: crate::runtime_ops::vec_index(),
                args: vec![MirOperand::Local(entries), MirOperand::Local(idx)],
            }));
            let mut parts = Vec::new();
            for (i, part_ty) in pair.iter().enumerate() {
                let part = this.builder.alloc_temp(part_ty.clone());
                let size = rask_layout::size_of(part_ty, this.ctx.target()) as u32;
                this.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: part,
                    rvalue: MirRValue::Field {
                        base: MirOperand::Local(entry),
                        field_index: i as u32,
                        byte_offset: Some(offsets[i]),
                        access: FieldAccess::for_field(part_ty, size),
                    },
                }));
                parts.push(part);
            }
            this.emit_json_value(&JsonSink::Field { buf, key: MirOperand::Local(parts[0]) }, parts[1], Some(val_ty))
        })?;
        let result = self.builder.alloc_temp(MirType::Buffer);
        self.push_json_finish(buf, result);
        Ok(result)
    }

    /// A fixed array `[T; N]` as a JSON array.
    fn lower_json_encode_array(
        &mut self,
        array: crate::LocalId,
        elem_ty: &rask_types::Type,
        len: usize,
    ) -> Result<crate::LocalId, LoweringError> {
        let elem_mir = self.ctx.type_to_mir(elem_ty);
        let arr = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(arr),
            func: FunctionRef::reading("rask_json_buf_new_array"),
            args: vec![],
        }));
        self.emit_json_count_loop(MirOperand::Constant(MirConst::Int(len as i64)), |this, idx| {
            let elem = this.builder.alloc_temp(elem_mir.clone());
            this.read_array_element(elem, array, idx, &elem_mir);
            this.emit_json_value(&JsonSink::Element { arr }, elem, Some(elem_ty))
        })?;
        let result = self.builder.alloc_temp(MirType::Buffer);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(result),
            func: FunctionRef::releasing("rask_json_buf_finish_array").writing_string(),
            args: vec![MirOperand::Local(arr)],
        }));
        Ok(result)
    }

    /// `for idx in 0..count { body(idx) }`, for the encoders' walks.
    fn emit_json_count_loop(
        &mut self,
        count: MirOperand,
        body: impl FnOnce(&mut Self, crate::LocalId) -> Result<(), LoweringError>,
    ) -> Result<(), LoweringError> {
        let idx = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: idx,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
        }));
        let check = self.builder.create_block();
        let body_block = self.builder.create_block();
        let exit = self.builder.create_block();
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: check }));
        self.builder.switch_to_block(check);
        let cond = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: cond,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Lt,
                left: MirOperand::Local(idx),
                right: count,
            },
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(cond),
            then_block: body_block,
            else_block: exit,
        }));
        self.builder.switch_to_block(body_block);
        body(self, idx)?;
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: idx,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Add,
                left: MirOperand::Local(idx),
                right: MirOperand::Constant(MirConst::Int(1)),
            },
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: check }));
        self.builder.switch_to_block(exit);
        Ok(())
    }

    /// Expand `json.encode(vec)` into a loop that encodes each element.
    /// `elem_mir` is the element's lowered type when the checker's type isn't
    /// available — a `mut v = Vec.new()` filled by `push` leaves the checker with
    /// an inference variable, and without the element type every element encoded
    /// as `json_buf_array_add_i64`: a Vec of structs came out `[1,2]`.
    pub(super) fn lower_json_encode_vec(
        &mut self,
        vec_op: MirOperand,
        elem_ty: Option<rask_types::Type>,
        elem_mir: Option<MirType>,
    ) -> Result<TypedOperand, LoweringError> {
        use rask_types::Type;

        let arr_buf = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(arr_buf),
            func: FunctionRef::reading("rask_json_buf_new_array"),
            args: vec![],
        }));

        let collection = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: collection,
            rvalue: MirRValue::Use(vec_op),
        }));

        let len_local = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(len_local),
            func: FunctionRef::internal("Vec_len".to_string()),
            args: vec![MirOperand::Local(collection)],
        }));

        let idx = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: idx,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
        }));

        let check_block = self.builder.create_block();
        let body_block = self.builder.create_block();
        let exit_block = self.builder.create_block();

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: check_block }));

        self.builder.switch_to_block(check_block);
        let cond = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: cond,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Lt,
                left: MirOperand::Local(idx),
                right: MirOperand::Local(len_local),
            },
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(cond),
            then_block: body_block,
            else_block: exit_block,
        }));

        self.builder.switch_to_block(body_block);

        // The element's own type, so a struct element comes back as a pointer to
        // its storage, a string keeps all 16 bytes and a scalar is read at its
        // own width.
        let elem_ty = elem_ty.filter(|t| !matches!(t, Type::Var(_)));
        let elem_local_ty = match (&elem_ty, &elem_mir) {
            (Some(ty), _) => self.ctx.type_to_mir(ty),
            (None, Some(m)) => m.clone(),
            (None, None) => MirType::I64,
        };
        let elem = self.builder.alloc_temp(elem_local_ty);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(elem),
            func: crate::runtime_ops::vec_index(),
            args: vec![MirOperand::Local(collection), MirOperand::Local(idx)],
        }));
        self.emit_json_value(&JsonSink::Element { arr: arr_buf }, elem, elem_ty.as_ref())?;

        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: idx,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Add,
                left: MirOperand::Local(idx),
                right: MirOperand::Constant(MirConst::Int(1)),
            },
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: check_block }));

        self.builder.switch_to_block(exit_block);
        // The StringOutParam adapter writes a 16-byte RaskStr and hands back its
        // address, so this is a string — typing it I64 made `let j =
        // json.encode(v)` print a pointer (#478).
        let result = self.builder.alloc_temp(MirType::Buffer);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(result),
            func: FunctionRef::releasing("rask_json_buf_finish_array").writing_string(),
            args: vec![MirOperand::Local(arr_buf)],
        }));

        Ok((MirOperand::Local(result), MirType::Buffer))
    }

    /// The slot a value takes in a collection — a Vec or Map element, a key,
    /// a channel element, a box payload: the type's own size. Every value sits
    /// at its own width wherever it is, so a `Vec<u8>` is a byte per element
    /// and a `Vec<f32>` holds f32s. `None` while the type is still a variable.
    pub(super) fn slot_size_for_type(&self, ty: &rask_types::Type) -> Option<i64> {
        match ty {
            rask_types::Type::Var(_) => None,
            _ => Some(rask_layout::size_of(&self.ctx.type_to_mir(ty), self.ctx.target()) as i64),
        }
    }

    /// The MIR type of a container's Nth type argument — `Vec<i32?>` at index 0
    /// is `i32?`.
    pub(super) fn container_elem_mir_type(
        &self,
        node_id: rask_ast::NodeId,
        index: usize,
    ) -> Option<MirType> {
        match self.container_elem_rask_type(node_id, index)? {
            rask_types::Type::Var(_) => None,
            resolved => Some(self.ctx.type_to_mir(&resolved)),
        }
    }

    /// The checker's own type for a container's `index`-th type argument.
    fn container_elem_rask_type(
        &self,
        node_id: rask_ast::NodeId,
        index: usize,
    ) -> Option<rask_types::Type> {
        use rask_types::{GenericArg, Type};
        let ty = self.ctx.lookup_raw_type(node_id)?;
        let container = match &*ty {
            Type::Tuple(elems) => elems.first()?,
            other => &other,
        };
        let args = match container {
            Type::Generic { args, .. } | Type::UnresolvedGeneric { args, .. } => args,
            _ => return None,
        };
        let GenericArg::Type(inner) = args.get(index)? else {
            return None;
        };
        Some(inner.as_ref().clone())
    }

    pub(super) fn head_name(&self, ty: &rask_types::Type) -> Option<String> {
        ty.head_name(self.ctx.type_names).map(str::to_string)
    }

    /// Clone function name for a type, or None if the type is Copy.
    pub(super) fn clone_fn_for_type(ty: &rask_types::Type) -> Option<&'static str> {
        match ty {
            rask_types::Type::String | rask_types::Type::Bytes => Some(crate::runtime_ops::STRING_SHARE),
            rask_types::Type::UnresolvedNamed(n) if n == "string" => Some(crate::runtime_ops::STRING_SHARE),
            rask_types::Type::UnresolvedGeneric { name, .. } if name == "Vec" => Some("Vec_clone"),
            rask_types::Type::UnresolvedGeneric { name, .. } if name == "Map" => Some("Map_clone"),
            // A closure is one pointer, and its clone is a new environment.
            rask_types::Type::Fn { .. } => Some("rask_closure_clone"),
            _ => None,
        }
    }

    /// The call that clones a value into a destination, for a name
    /// `clone_fn_for_type` gave.
    pub(super) fn clone_call(cfn: &str) -> FunctionRef {
        if cfn == crate::runtime_ops::STRING_SHARE {
            crate::runtime_ops::string_clone()
        } else {
            FunctionRef::borrowing(cfn)
        }
    }

    /// Emit inline clone for an enum value: shallow copy the full block,
    /// then switch on the tag to deep-clone heap fields per variant.
    pub(super) fn lower_enum_clone(
        &mut self,
        layout: &rask_layout::EnumLayout,
        src: &MirOperand,
        obj_ty: MirType,
    ) -> Result<TypedOperand, LoweringError> {
        let result = self.builder.alloc_temp(obj_ty.clone());

        // Shallow copy: the whole value, at its real size. Copying it a word
        // at a time read and wrote past the end of any enum whose size isn't
        // a multiple of eight.
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result,
            offset: 0,
            value: src.clone(),
            store_size: layout.size,
            field: None,
        }));

        let needs_switch = layout.variants.iter().any(|v| {
            v.fields.iter().any(|f| Self::clone_fn_for_type(&f.ty).is_some())
        });

        if needs_switch {
            // The tag at its own offset and width, the way a match reads it.
            let tag = self.emit_enum_tag(MirOperand::Local(result));

            // The switch belongs on the block that read the tag. Building the
            // variant blocks below moves the builder's cursor onto the last of
            // them, so remember where to come back to — terminating from here
            // put the switch on a variant block and left the tag-reading block
            // with its default `unreachable`, which is what a `.clone()` on an
            // enum carrying a string trapped on.
            let dispatch_block = self.builder.current_block();
            let exit_block = self.builder.create_block();
            let mut cases = Vec::new();

            for variant in &layout.variants {
                let has_heap = variant.fields.iter().any(|f| Self::clone_fn_for_type(&f.ty).is_some());
                if !has_heap {
                    continue;
                }
                let vblock = self.builder.create_block();
                cases.push((variant.tag as u64, vblock));

                self.builder.switch_to_block(vblock);
                for field in &variant.fields {
                    if let Some(cfn) = Self::clone_fn_for_type(&field.ty) {
                        let abs_offset = variant.payload_offset + field.offset;
                        if cfn == crate::runtime_ops::STRING_SHARE {
                            // A string is 16 bytes and the shallow copy above
                            // already moved all of them; the only thing left is
                            // the refcount. Take the field's address and bump
                            // it in place.
                            //
                            // Reading it as one word, "cloning" that, and
                            // storing the word back wrote the call's return
                            // register over the string's first 8 bytes — which
                            // is how `.clone()` on an enum carrying a string
                            // produced a value whose tag then trapped.
                            let field_addr = self.builder.alloc_temp(MirType::I64);
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                                dst: field_addr,
                                rvalue: MirRValue::Field {
                                    base: MirOperand::Local(result),
                                    field_index: abs_offset,
                                    byte_offset: Some(abs_offset),
                                    access: FieldAccess::InPlace(16),
                                },
                            }));
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                                dst: None,
                                func: crate::runtime_ops::string_share_in_place(),
                                args: vec![MirOperand::Local(field_addr)],
                            }));
                            continue;
                        }
                        // Vec/Map payloads are an 8-byte pointer, and their
                        // clone really does hand back a new one.
                        let field_val = self.builder.alloc_temp(MirType::I64);
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                            dst: field_val,
                            rvalue: MirRValue::Field {
                                base: MirOperand::Local(result),
                                field_index: abs_offset,
                                byte_offset: Some(abs_offset),
                                access: FieldAccess::Word,
                            },
                        }));
                        let cloned = self.builder.alloc_temp(MirType::I64);
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                            dst: Some(cloned),
                            func: FunctionRef::borrowing(cfn),
                            args: vec![MirOperand::Local(field_val)],
                        }));
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                            addr: result,
                            offset: abs_offset,
                            value: MirOperand::Local(cloned),
                            store_size: rask_layout::size_of(&MirType::Ptr, self.ctx.target()),
                            field: None,
                        }));
                    }
                }
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: exit_block }));
            }

            self.builder.switch_to_block(dispatch_block);
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Switch {
                value: MirOperand::Local(tag),
                cases,
                default: exit_block,
            }));

            self.builder.switch_to_block(exit_block);
        }

        Ok((MirOperand::Local(result), obj_ty))
    }
}
