// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! DWARF debug info emission (DI1–DI4).
//!
//! Writes `.debug_info`, `.debug_abbrev`, `.debug_line`, and `.debug_str`
//! sections into the object file. Gives debuggers line-level stepping,
//! variable names, and type information for Rask source code.
//!
//! Only active in debug builds — release builds skip DWARF entirely.
//!
//! ## Linking requirement
//!
//! The Rask .o must appear *before* the C runtime sources on the link
//! command so our DWARF sections land at offset 0 in each merged section.
//! Without this, the CU's abbrev_offset and stmt_list point into the wrong
//! section data. See `link.rs` for the ordering.

use gimli::write::{
    Address, AttributeValue, DwarfUnit, EndianVec, LineProgram, LineString,
    Sections, UnitEntryId, Writer,
};
use gimli::{
    DW_AT_byte_size, DW_AT_comp_dir, DW_AT_encoding, DW_AT_external,
    DW_AT_high_pc, DW_AT_language, DW_AT_low_pc, DW_AT_name, DW_AT_producer,
    DW_AT_stmt_list, DW_AT_type,
    DW_ATE_address, DW_ATE_boolean, DW_ATE_float, DW_ATE_signed, DW_ATE_unsigned,
    DW_LANG_C99, DW_TAG_base_type, DW_TAG_formal_parameter, DW_TAG_subprogram,
    DW_TAG_variable, Encoding, Format, LineEncoding, RunTimeEndian, SectionId,
};
use object::write::{Object, Relocation, SymbolId};
use rask_ast::LineMap;

use crate::module::SrcLocEntry;
use crate::CodegenError;

// ── Public types (used by module.rs) ────────────────────────────────────────

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

/// Per-function debug info with resolved object symbol ID.
pub struct ResolvedFunctionDebug {
    pub symbol_id: SymbolId,
    pub name: String,
    pub srclocs: Vec<SrcLocEntry>,
    /// Native code byte length (DW_AT_high_pc offset from low_pc).
    pub code_size: u32,
    /// Formal parameters (DI3: DW_TAG_formal_parameter).
    pub params: Vec<VarInfo>,
    /// Named local variables (DI3: DW_TAG_variable).
    pub locals: Vec<VarInfo>,
}

// ── Internal writer with relocation tracking ─────────────────────────────────

/// Tracked relocation from gimli address writes.
#[derive(Clone)]
struct PendingReloc {
    offset: usize,
    size: u8,
    symbol: SymbolId,
    addend: i64,
}

/// Writer that records relocations for Address::Symbol references.
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

/// Create or look up a DW_TAG_base_type / DW_TAG_structure_type entry.
/// Returns the entry ID to use in DW_AT_type references.
fn get_or_create_type(
    unit: &mut gimli::write::Unit,
    root: UnitEntryId,
    type_map: &mut std::collections::HashMap<(String, u32), UnitEntryId>,
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
            // Structs, enums, and complex types — emit as opaque structure.
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

// ── Main entry point ──────────────────────────────────────────────────────────

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

    // Build symbol map: gimli symbol index → object SymbolId
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

    // ── Line program (DI2) ────────────────────────────────────────────────────
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

    // ── DWARF compilation unit ────────────────────────────────────────────────
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

    // ── DI4: Type entries ─────────────────────────────────────────────────────
    let mut type_map: std::collections::HashMap<(String, u32), UnitEntryId> =
        std::collections::HashMap::new();

    for func in functions {
        for var in func.params.iter().chain(func.locals.iter()) {
            if var.byte_size > 0 {
                get_or_create_type(&mut dwarf.unit, root_id, &mut type_map, var);
            }
        }
    }

    // ── DI3: Subprogram + variable DIEs ───────────────────────────────────────
    for (idx, func) in functions.iter().enumerate() {
        let sp = dwarf.unit.add(root_id, DW_TAG_subprogram);
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

    // ── Write sections ────────────────────────────────────────────────────────
    let mut sections = Sections::new(RelocWriter::new(symbol_map));
    dwarf.write(&mut sections)
        .map_err(|e| CodegenError::CraneliftError(format!("DWARF write: {}", e)))?;

    // Collect section data before adding to object (need IDs for reloc pass)
    let mut section_data: Vec<(SectionId, Vec<u8>, Vec<PendingReloc>)> = Vec::new();
    sections.for_each_mut(|id, writer| {
        if !writer.inner.slice().is_empty() {
            section_data.push((id, writer.inner.slice().to_vec(), writer.relocs.clone()));
        }
        Ok(())
    }).map_err(|e: gimli::write::Error| {
        CodegenError::CraneliftError(format!("DWARF section: {}", e))
    })?;

    // Add sections to object, track IDs
    let mut obj_sections: std::collections::HashMap<SectionId, object::write::SectionId> =
        std::collections::HashMap::new();
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

    // Add address relocations (for DW_AT_low_pc and line sequence start addresses)
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
