// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! DWARF debug info — all debug concerns live here (DI1–DI5).
//!
//! Collects per-function debug data after Cranelift compilation, then emits
//! `.debug_info`, `.debug_abbrev`, `.debug_line`, and `.debug_str` sections.
//! Only active in debug builds — release builds skip this module entirely.
//!
//! ## Linking requirement
//!
//! The Rask .o must appear *before* the C runtime sources on the link
//! command so our DWARF sections land at offset 0 in each merged section.
//! See `link.rs` for the ordering.

use std::collections::HashMap;

use gimli::write::{
    Address, AttributeValue, DwarfUnit, EndianVec, LineProgram, LineString,
    Sections, UnitEntryId, Writer,
};
use gimli::{
    DW_AT_abstract_origin, DW_AT_byte_size, DW_AT_call_file, DW_AT_call_line,
    DW_AT_comp_dir, DW_AT_encoding, DW_AT_external, DW_AT_high_pc, DW_AT_inline,
    DW_AT_language, DW_AT_low_pc, DW_AT_name, DW_AT_producer, DW_AT_stmt_list,
    DW_AT_type,
    DW_ATE_address, DW_ATE_boolean, DW_ATE_float, DW_ATE_signed, DW_ATE_unsigned,
    DW_INL_inlined, DW_LANG_C99, DW_TAG_base_type, DW_TAG_formal_parameter,
    DW_TAG_inlined_subroutine, DW_TAG_subprogram, DW_TAG_variable,
    Encoding, Format, LineEncoding, RunTimeEndian, SectionId,
};
use object::write::{Object, Relocation, SymbolId};
use rask_ast::LineMap;
use rask_mono::{EnumLayout, StructLayout};

use crate::CodegenError;

// ── Public types ─────────────────────────────────────────────────────────────

/// Source location entry mapping native code offset to source byte offset.
#[derive(Debug, Clone)]
pub struct SrcLocEntry {
    /// Offset within the function's native code
    pub native_offset: u32,
    /// Byte offset in the source file (the SourceLoc value)
    pub source_offset: u32,
}

/// DWARF type category for a variable's type.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeKind {
    Signed,
    Unsigned,
    Float,
    Boolean,
    Address, // pointer / raw address
    Other,   // struct, enum, string — emitted as opaque structure
}

/// Debug info for a single variable or parameter.
#[derive(Debug, Clone)]
pub struct VarInfo {
    pub name: String,
    pub type_name: String,
    pub byte_size: u32,
    pub type_kind: TypeKind,
}

/// DI5: An inlined callee with its native address range.
#[derive(Debug, Clone)]
pub struct InlinedCalleeInfo {
    pub callee_name: String,
    pub call_site_offset: u32,
    pub native_start: u32,
    pub native_end: u32,
}

/// Per-function debug info collected after compilation.
#[derive(Debug, Clone)]
pub struct FunctionDebugInfo {
    pub func_id: cranelift_module::FuncId,
    pub name: String,
    pub srclocs: Vec<SrcLocEntry>,
    /// Native code byte length (for DW_AT_high_pc).
    pub code_size: u32,
    /// Formal parameters (DI3).
    pub params: Vec<VarInfo>,
    /// Named local variables (DI3).
    pub locals: Vec<VarInfo>,
    /// DI5: inlined callees with their native address ranges.
    pub inlined_callees: Vec<InlinedCalleeInfo>,
}

/// Per-function debug info with resolved object symbol ID (ready for DWARF emission).
pub struct ResolvedFunctionDebug {
    pub symbol_id: SymbolId,
    pub name: String,
    pub srclocs: Vec<SrcLocEntry>,
    pub code_size: u32,
    pub params: Vec<VarInfo>,
    pub locals: Vec<VarInfo>,
    pub inlined_callees: Vec<InlinedCalleeInfo>,
}

// ── Debug info collection (called from module.rs after each function compile) ─

/// Extract srcloc mappings and variable info from a compiled function.
///
/// Called by CodeGenerator::gen_function after Cranelift compilation.
/// Returns None if no srclocs were emitted.
pub fn collect_function_debug(
    compiled: &cranelift_codegen::CompiledCode,
    func_id: cranelift_module::FuncId,
    mir_fn: &rask_mir::MirFunction,
    inline_regions: &[rask_mir::InlineRegion],
    struct_layouts: &[StructLayout],
    enum_layouts: &[EnumLayout],
    line_map: Option<&LineMap>,
) -> Option<FunctionDebugInfo> {
    let all_srclocs = compiled.buffer.get_srclocs_sorted();
    let mut srclocs = Vec::new();
    for entry in all_srclocs {
        if !entry.loc.is_default() && entry.loc.bits() > 0 {
            srclocs.push(SrcLocEntry {
                native_offset: entry.start,
                // Decode: apply_srcloc encodes as offset+1
                source_offset: entry.loc.bits() - 1,
            });
        }
    }

    if srclocs.is_empty() {
        return None;
    }

    let code_size = all_srclocs.last().map(|e| e.end).unwrap_or(0);
    let params: Vec<_> = mir_fn.params.iter()
        .map(|p| mir_type_to_var_info(
            p.name.as_deref().unwrap_or("_"),
            &p.ty, struct_layouts, enum_layouts,
        ))
        .collect();
    let locals: Vec<_> = mir_fn.locals.iter()
        .filter(|l| l.name.is_some() && !l.is_param)
        .map(|l| mir_type_to_var_info(
            l.name.as_deref().unwrap(),
            &l.ty, struct_layouts, enum_layouts,
        ))
        .collect();

    let inlined_callees = compute_inline_ranges(&srclocs, inline_regions, line_map);

    Some(FunctionDebugInfo {
        func_id,
        name: mir_fn.name.clone(),
        srclocs,
        code_size,
        params,
        locals,
        inlined_callees,
    })
}

/// Resolve FuncId → SymbolId and prepare for DWARF emission.
pub fn resolve_debug_info(
    debug_info: &[FunctionDebugInfo],
    product: &cranelift_object::ObjectProduct,
) -> Vec<ResolvedFunctionDebug> {
    debug_info.iter()
        .map(|f| {
            let sym = product.function_symbol(f.func_id);
            ResolvedFunctionDebug {
                symbol_id: sym,
                name: f.name.clone(),
                srclocs: f.srclocs.clone(),
                code_size: f.code_size,
                params: f.params.clone(),
                locals: f.locals.clone(),
                inlined_callees: f.inlined_callees.clone(),
            }
        })
        .collect()
}

// ── MIR type → VarInfo conversion ────────────────────────────────────────────

/// Convert a MIR local's type to the VarInfo needed for DWARF DI3/DI4.
fn mir_type_to_var_info(
    name: &str,
    ty: &rask_mir::MirType,
    struct_layouts: &[StructLayout],
    enum_layouts: &[EnumLayout],
) -> VarInfo {
    use rask_mir::{EnumLayoutId, StructLayoutId};

    let (type_name, byte_size, type_kind) = match ty {
        rask_mir::MirType::Void     => ("void".into(), 0u32, TypeKind::Other),
        rask_mir::MirType::Bool     => ("bool".into(), 1, TypeKind::Boolean),
        rask_mir::MirType::I8       => ("i8".into(), 1, TypeKind::Signed),
        rask_mir::MirType::I16      => ("i16".into(), 2, TypeKind::Signed),
        rask_mir::MirType::I32      => ("i32".into(), 4, TypeKind::Signed),
        rask_mir::MirType::I64      => ("i64".into(), 8, TypeKind::Signed),
        rask_mir::MirType::U8       => ("u8".into(), 1, TypeKind::Unsigned),
        rask_mir::MirType::U16      => ("u16".into(), 2, TypeKind::Unsigned),
        rask_mir::MirType::U32      => ("u32".into(), 4, TypeKind::Unsigned),
        rask_mir::MirType::U64      => ("u64".into(), 8, TypeKind::Unsigned),
        rask_mir::MirType::F32      => ("f32".into(), 4, TypeKind::Float),
        rask_mir::MirType::F64      => ("f64".into(), 8, TypeKind::Float),
        rask_mir::MirType::Char     => ("char".into(), 4, TypeKind::Unsigned),
        rask_mir::MirType::Ptr      => ("ptr".into(), 8, TypeKind::Address),
        rask_mir::MirType::String   => ("string".into(), 8, TypeKind::Address),
        rask_mir::MirType::Handle   => ("Handle".into(), 8, TypeKind::Signed),
        rask_mir::MirType::Struct(StructLayoutId { id, byte_size, .. }) => {
            let sname = struct_layouts.get(*id as usize)
                .map(|s| s.name.clone())
                .unwrap_or_else(|| "struct".into());
            (sname, *byte_size, TypeKind::Other)
        }
        rask_mir::MirType::Enum(EnumLayoutId { id, byte_size, .. }) => {
            let ename = enum_layouts.get(*id as usize)
                .map(|e| e.name.clone())
                .unwrap_or_else(|| "enum".into());
            (ename, *byte_size, TypeKind::Other)
        }
        rask_mir::MirType::Option(inner) => {
            let inner = mir_type_to_var_info("", inner, struct_layouts, enum_layouts);
            (format!("Option<{}>", inner.type_name), inner.byte_size + 8, TypeKind::Other)
        }
        rask_mir::MirType::Tuple(_) => ("tuple".into(), 8, TypeKind::Other),
        rask_mir::MirType::Slice(_) => ("slice".into(), 16, TypeKind::Other),
        _ => ("unknown".into(), 8, TypeKind::Other),
    };
    VarInfo { name: name.to_owned(), type_name, byte_size, type_kind }
}

// ── DI5: inline range computation ────────────────────────────────────────────

/// For each inline region, find the native offset range of srclocs
/// whose source_offset falls within the callee's body span.
fn compute_inline_ranges(
    srclocs: &[SrcLocEntry],
    inline_regions: &[rask_mir::InlineRegion],
    _line_map: Option<&LineMap>,
) -> Vec<InlinedCalleeInfo> {
    let mut result = Vec::new();
    for region in inline_regions {
        let callee_start = region.callee_body_span.start as u32;
        let callee_end = region.callee_body_span.end as u32;
        if callee_end == 0 {
            continue;
        }

        let mut native_min = u32::MAX;
        let mut native_max = 0u32;
        for entry in srclocs {
            if entry.source_offset >= callee_start && entry.source_offset < callee_end {
                native_min = native_min.min(entry.native_offset);
                native_max = native_max.max(entry.native_offset);
            }
        }

        if native_max > 0 && native_min < u32::MAX {
            result.push(InlinedCalleeInfo {
                callee_name: region.callee_name.clone(),
                call_site_offset: region.call_site.start as u32,
                native_start: native_min,
                native_end: native_max + 1, // past-the-end
            });
        }
    }
    result
}

// ── Internal writer with relocation tracking ─────────────────────────────────

#[derive(Clone)]
struct PendingReloc {
    offset: usize,
    size: u8,
    symbol: SymbolId,
    addend: i64,
}

#[derive(Clone)]
struct RelocWriter {
    inner: EndianVec<RunTimeEndian>,
    relocs: Vec<PendingReloc>,
    symbol_map: Vec<SymbolId>,
}

impl RelocWriter {
    fn new(symbol_map: Vec<SymbolId>) -> Self {
        Self {
            inner: EndianVec::new(RunTimeEndian::Little),
            relocs: Vec::new(),
            symbol_map,
        }
    }
}

impl Writer for RelocWriter {
    type Endian = RunTimeEndian;

    fn endian(&self) -> RunTimeEndian {
        self.inner.endian()
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn write(&mut self, bytes: &[u8]) -> gimli::write::Result<()> {
        self.inner.write(bytes)
    }

    fn write_at(&mut self, offset: usize, bytes: &[u8]) -> gimli::write::Result<()> {
        self.inner.write_at(offset, bytes)
    }

    fn write_address(&mut self, address: Address, size: u8) -> gimli::write::Result<()> {
        let offset = self.inner.len();
        match address {
            Address::Constant(val) => self.inner.write_udata(val, size),
            Address::Symbol { symbol, addend } => {
                self.inner.write_udata(0, size)?;
                if let Some(&sym_id) = self.symbol_map.get(symbol) {
                    self.relocs.push(PendingReloc {
                        offset,
                        size,
                        symbol: sym_id,
                        addend: addend as i64,
                    });
                }
                Ok(())
            }
        }
    }
}

// ── DWARF type deduplication ──────────────────────────────────────────────────

fn get_or_create_type(
    unit: &mut gimli::write::Unit,
    root: UnitEntryId,
    type_map: &mut HashMap<(String, u32), UnitEntryId>,
    var: &VarInfo,
) -> UnitEntryId {
    let key = (var.type_name.clone(), var.byte_size);
    if let Some(&id) = type_map.get(&key) {
        return id;
    }

    let entry_id = match var.type_kind {
        TypeKind::Signed | TypeKind::Unsigned | TypeKind::Float
        | TypeKind::Boolean | TypeKind::Address => {
            let id = unit.add(root, DW_TAG_base_type);
            let encoding = match var.type_kind {
                TypeKind::Signed => DW_ATE_signed,
                TypeKind::Unsigned => DW_ATE_unsigned,
                TypeKind::Float => DW_ATE_float,
                TypeKind::Boolean => DW_ATE_boolean,
                TypeKind::Address => DW_ATE_address,
                TypeKind::Other => unreachable!(),
            };
            unit.get_mut(id).set(
                DW_AT_name,
                AttributeValue::String(var.type_name.as_bytes().to_vec()),
            );
            if var.byte_size > 0 {
                unit.get_mut(id).set(DW_AT_byte_size, AttributeValue::Udata(var.byte_size as u64));
            }
            unit.get_mut(id).set(DW_AT_encoding, AttributeValue::Encoding(encoding));
            id
        }
        TypeKind::Other => {
            let id = unit.add(root, gimli::DW_TAG_structure_type);
            unit.get_mut(id).set(
                DW_AT_name,
                AttributeValue::String(var.type_name.as_bytes().to_vec()),
            );
            if var.byte_size > 0 {
                unit.get_mut(id).set(DW_AT_byte_size, AttributeValue::Udata(var.byte_size as u64));
            }
            id
        }
    };

    type_map.insert(key, entry_id);
    entry_id
}

// ── DWARF emission ───────────────────────────────────────────────────────────

/// Emit DWARF debug sections into the object file.
///
/// Assumes the Rask .o is linked before the C runtime (see module-level docs).
pub fn emit_dwarf(
    object: &mut Object<'_>,
    functions: &[ResolvedFunctionDebug],
    line_map: &LineMap,
    source_file: &str,
) -> Result<(), CodegenError> {
    if functions.is_empty() {
        return Ok(());
    }

    let symbol_map: Vec<SymbolId> = functions.iter().map(|f| f.symbol_id).collect();

    let encoding = Encoding {
        format: Format::Dwarf32,
        version: 4,
        address_size: 8,
    };

    let line_encoding = LineEncoding {
        minimum_instruction_length: 1,
        maximum_operations_per_instruction: 1,
        default_is_stmt: true,
        line_base: -5,
        line_range: 14,
    };

    let comp_dir = std::path::Path::new(source_file)
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .to_string_lossy()
        .into_owned();
    let file_base = std::path::Path::new(source_file)
        .file_name()
        .unwrap_or(std::ffi::OsStr::new(source_file))
        .to_string_lossy()
        .into_owned();

    // ── Line program (DI2) ──────────────────────────────────────────────
    let dir = LineString::String(comp_dir.as_bytes().to_vec());
    let file_name = LineString::String(file_base.as_bytes().to_vec());
    let mut program = LineProgram::new(encoding, line_encoding, dir, file_name.clone(), None);

    let dir_id = program.default_directory();
    let file_id = program.add_file(file_name, dir_id, None);

    for (idx, func) in functions.iter().enumerate() {
        if func.srclocs.is_empty() {
            continue;
        }
        program.begin_sequence(Some(Address::Symbol { symbol: idx, addend: 0 }));

        let mut prev_line = 0u32;
        for entry in &func.srclocs {
            let (line, col) = line_map.offset_to_line_col(entry.source_offset as usize);
            if line == prev_line {
                continue;
            }
            prev_line = line;
            program.row().file = file_id;
            program.row().line = line as u64;
            program.row().column = col as u64;
            program.row().address_offset = entry.native_offset as u64;
            program.generate_row();
        }
        program.end_sequence(
            func.srclocs.last().map_or(0, |e| e.native_offset as u64 + 1),
        );
    }

    // ── Compilation unit (DI1) ──────────────────────────────────────────
    let mut dwarf = DwarfUnit::new(encoding);
    dwarf.unit.line_program = program;

    let root_id = dwarf.unit.root();
    {
        let root = dwarf.unit.get_mut(root_id);
        root.set(DW_AT_producer, AttributeValue::StringRef(dwarf.strings.add("rask 0.1.0")));
        root.set(DW_AT_language, AttributeValue::Language(DW_LANG_C99));
        root.set(DW_AT_name, AttributeValue::StringRef(dwarf.strings.add(source_file)));
        root.set(DW_AT_comp_dir, AttributeValue::StringRef(dwarf.strings.add(comp_dir.as_bytes())));
        root.set(DW_AT_stmt_list, AttributeValue::LineProgramRef);
    }

    // ── DI4: Type entries ───────────────────────────────────────────────
    let mut type_map: HashMap<(String, u32), UnitEntryId> = HashMap::new();

    for func in functions {
        for var in func.params.iter().chain(func.locals.iter()) {
            if var.byte_size > 0 {
                get_or_create_type(&mut dwarf.unit, root_id, &mut type_map, var);
            }
        }
    }

    // ── DI3: Subprogram + variable DIEs ─────────────────────────────────
    let mut subprogram_map: HashMap<String, UnitEntryId> = HashMap::new();

    for (idx, func) in functions.iter().enumerate() {
        let sp = dwarf.unit.add(root_id, DW_TAG_subprogram);
        subprogram_map.insert(func.name.clone(), sp);
        dwarf.unit.get_mut(sp).set(
            DW_AT_name,
            AttributeValue::StringRef(dwarf.strings.add(func.name.as_str())),
        );
        dwarf.unit.get_mut(sp).set(DW_AT_external, AttributeValue::Flag(true));
        dwarf.unit.get_mut(sp).set(
            DW_AT_low_pc,
            AttributeValue::Address(Address::Symbol { symbol: idx, addend: 0 }),
        );
        if func.code_size > 0 {
            dwarf.unit.get_mut(sp).set(
                DW_AT_high_pc,
                AttributeValue::Udata(func.code_size as u64),
            );
        }

        for param in &func.params {
            let p = dwarf.unit.add(sp, DW_TAG_formal_parameter);
            dwarf.unit.get_mut(p).set(
                DW_AT_name,
                AttributeValue::String(param.name.as_bytes().to_vec()),
            );
            if param.byte_size > 0 {
                if let Some(&type_id) = type_map.get(&(param.type_name.clone(), param.byte_size)) {
                    dwarf.unit.get_mut(p).set(DW_AT_type, AttributeValue::UnitRef(type_id));
                }
            }
        }

        for local in &func.locals {
            let v = dwarf.unit.add(sp, DW_TAG_variable);
            dwarf.unit.get_mut(v).set(
                DW_AT_name,
                AttributeValue::String(local.name.as_bytes().to_vec()),
            );
            if local.byte_size > 0 {
                if let Some(&type_id) = type_map.get(&(local.type_name.clone(), local.byte_size)) {
                    dwarf.unit.get_mut(v).set(DW_AT_type, AttributeValue::UnitRef(type_id));
                }
            }
        }
    }

    // ── DI5: Abstract instances + inlined subroutine DIEs ───────────────
    for func in functions {
        for ic in &func.inlined_callees {
            if !subprogram_map.contains_key(&ic.callee_name) {
                let abs = dwarf.unit.add(root_id, DW_TAG_subprogram);
                dwarf.unit.get_mut(abs).set(
                    DW_AT_name,
                    AttributeValue::StringRef(dwarf.strings.add(ic.callee_name.as_str())),
                );
                dwarf.unit.get_mut(abs).set(
                    DW_AT_inline,
                    AttributeValue::Inline(DW_INL_inlined),
                );
                subprogram_map.insert(ic.callee_name.clone(), abs);
            }
        }
    }

    for (idx, func) in functions.iter().enumerate() {
        if func.inlined_callees.is_empty() {
            continue;
        }
        let caller_sp = match subprogram_map.get(&func.name) {
            Some(&sp) => sp,
            None => continue,
        };

        for ic in &func.inlined_callees {
            let callee_sp = match subprogram_map.get(&ic.callee_name) {
                Some(&sp) => sp,
                None => continue,
            };

            let is = dwarf.unit.add(caller_sp, DW_TAG_inlined_subroutine);
            dwarf.unit.get_mut(is).set(
                DW_AT_abstract_origin,
                AttributeValue::UnitRef(callee_sp),
            );
            dwarf.unit.get_mut(is).set(
                DW_AT_low_pc,
                AttributeValue::Address(Address::Symbol {
                    symbol: idx,
                    addend: ic.native_start as i64,
                }),
            );
            dwarf.unit.get_mut(is).set(
                DW_AT_high_pc,
                AttributeValue::Udata((ic.native_end - ic.native_start) as u64),
            );
            let (call_line, _call_col) = line_map.offset_to_line_col(ic.call_site_offset as usize);
            dwarf.unit.get_mut(is).set(DW_AT_call_file, AttributeValue::FileIndex(Some(file_id)));
            dwarf.unit.get_mut(is).set(DW_AT_call_line, AttributeValue::Udata(call_line as u64));
        }
    }

    // ── Write sections ──────────────────────────────────────────────────
    let mut sections = Sections::new(RelocWriter::new(symbol_map));
    dwarf.write(&mut sections)
        .map_err(|e| CodegenError::CraneliftError(format!("DWARF write: {}", e)))?;

    let mut section_data: Vec<(SectionId, Vec<u8>, Vec<PendingReloc>)> = Vec::new();
    sections.for_each_mut(|id, writer| {
        if !writer.inner.slice().is_empty() {
            section_data.push((id, writer.inner.slice().to_vec(), writer.relocs.clone()));
        }
        Ok(())
    }).map_err(|e: gimli::write::Error| {
        CodegenError::CraneliftError(format!("DWARF section: {}", e))
    })?;

    let mut obj_sections: HashMap<SectionId, object::write::SectionId> = HashMap::new();
    for (id, bytes, _) in &section_data {
        let name = dwarf_section_name(*id);
        if name.is_empty() {
            continue;
        }
        let sec_id = object.add_section(
            Vec::new(),
            name.as_bytes().to_vec(),
            object::SectionKind::Debug,
        );
        object.append_section_data(sec_id, bytes, 1);
        obj_sections.insert(*id, sec_id);
    }

    for (id, _, relocs) in &section_data {
        if let Some(&sec_id) = obj_sections.get(id) {
            for reloc in relocs {
                let (kind, encoding, size) = match reloc.size {
                    8 => (object::RelocationKind::Absolute, object::RelocationEncoding::Generic, 64),
                    4 => (object::RelocationKind::Absolute, object::RelocationEncoding::Generic, 32),
                    _ => continue,
                };
                let _ = object.add_relocation(sec_id, Relocation {
                    offset: reloc.offset as u64,
                    symbol: reloc.symbol,
                    addend: reloc.addend,
                    flags: object::RelocationFlags::Generic { kind, encoding, size },
                });
            }
        }
    }

    Ok(())
}

fn dwarf_section_name(id: SectionId) -> &'static str {
    match id {
        SectionId::DebugInfo => ".debug_info",
        SectionId::DebugAbbrev => ".debug_abbrev",
        SectionId::DebugLine => ".debug_line",
        SectionId::DebugStr => ".debug_str",
        SectionId::DebugRanges => ".debug_ranges",
        SectionId::DebugStrOffsets => ".debug_str_offsets",
        SectionId::DebugLineStr => ".debug_line_str",
        _ => "",
    }
}
