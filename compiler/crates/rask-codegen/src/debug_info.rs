// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! DWARF debug info emission (DI1, DI2).
//!
//! Writes `.debug_info`, `.debug_abbrev`, `.debug_line`, and `.debug_str`
//! sections into the object file. This gives debuggers (gdb, lldb) line-level
//! stepping through Rask source code.
//!
//! Only active in debug builds — release builds skip DWARF entirely.

use gimli::write::{
    Address, AttributeValue, DwarfUnit, EndianVec, LineProgram, LineString,
    Sections, Writer,
};
use gimli::{DW_AT_comp_dir, DW_AT_language, DW_AT_name, DW_AT_producer, DW_AT_stmt_list,
            DW_LANG_C99, Encoding, Format, RunTimeEndian,
            SectionId, LineEncoding};
use object::write::{Object, Relocation, SymbolId};
use rask_ast::LineMap;

use crate::module::SrcLocEntry;
use crate::CodegenError;

/// Per-function debug info with resolved object symbol ID.
pub struct ResolvedFunctionDebug {
    pub symbol_id: SymbolId,
    pub name: String,
    pub srclocs: Vec<SrcLocEntry>,
}

/// Tracked relocation from gimli address writes.
#[derive(Clone)]
struct PendingReloc {
    /// Offset within the section where the address is written
    offset: usize,
    /// Size of the address field (4 or 8 bytes)
    size: u8,
    /// Object symbol to relocate against
    symbol: SymbolId,
    /// Addend for the relocation
    addend: i64,
}

/// Writer that records relocations for Address::Symbol references.
#[derive(Clone)]
struct RelocWriter {
    inner: EndianVec<RunTimeEndian>,
    relocs: Vec<PendingReloc>,
    /// Map from gimli symbol index → object SymbolId
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
            Address::Constant(val) => {
                self.inner.write_udata(val, size)
            }
            Address::Symbol { symbol, addend } => {
                // Write placeholder zeros; add relocation
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

/// Emit DWARF debug sections into the object file.
pub fn emit_dwarf(
    object: &mut Object<'_>,
    functions: &[ResolvedFunctionDebug],
    line_map: &LineMap,
    source_file: &str,
) -> Result<(), CodegenError> {
    if functions.is_empty() {
        return Ok(());
    }

    // Build symbol map: gimli index → object SymbolId
    let symbol_map: Vec<SymbolId> = functions.iter()
        .map(|f| f.symbol_id)
        .collect();

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

    let dir = LineString::String(comp_dir.as_bytes().to_vec());
    let file_name = LineString::String(file_base.as_bytes().to_vec());

    let mut program = LineProgram::new(
        encoding,
        line_encoding,
        dir,
        file_name.clone(),
        None,
    );

    let dir_id = program.default_directory();
    let file_id = program.add_file(file_name, dir_id, None);

    // Add line entries for each function
    for (idx, func) in functions.iter().enumerate() {
        if func.srclocs.is_empty() {
            continue;
        }

        // Each function starts a new sequence using the symbol map index
        program.begin_sequence(Some(Address::Symbol {
            symbol: idx,
            addend: 0,
        }));

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

    // Build the DWARF unit
    let mut dwarf = DwarfUnit::new(encoding);
    dwarf.unit.line_program = program;

    let root_id = dwarf.unit.root();
    let root = dwarf.unit.get_mut(root_id);
    root.set(DW_AT_producer, AttributeValue::StringRef(dwarf.strings.add("rask 0.1.0")));
    root.set(DW_AT_language, AttributeValue::Language(DW_LANG_C99));
    root.set(DW_AT_name, AttributeValue::StringRef(dwarf.strings.add(source_file)));
    root.set(DW_AT_comp_dir, AttributeValue::StringRef(dwarf.strings.add(comp_dir.as_bytes())));
    root.set(DW_AT_stmt_list, AttributeValue::LineProgramRef);

    // Write DWARF sections using our relocation-tracking writer
    let mut sections = Sections::new(RelocWriter::new(symbol_map));
    dwarf.write(&mut sections)
        .map_err(|e| CodegenError::CraneliftError(format!("DWARF write: {}", e)))?;

    // Add each non-empty DWARF section + its relocations to the object
    sections.for_each_mut(|id, writer| {
        let bytes = writer.inner.slice();
        if bytes.is_empty() {
            return Ok(());
        }
        let name = dwarf_section_name(id);
        if name.is_empty() {
            return Ok(());
        }

        let section_id = object.add_section(
            Vec::new(),
            name.as_bytes().to_vec(),
            object::SectionKind::Debug,
        );
        object.append_section_data(section_id, bytes, 1);

        // Add relocations for address references
        for reloc in &writer.relocs {
            let (kind, encoding, size) = match reloc.size {
                8 => (
                    object::RelocationKind::Absolute,
                    object::RelocationEncoding::Generic,
                    64,
                ),
                4 => (
                    object::RelocationKind::Absolute,
                    object::RelocationEncoding::Generic,
                    32,
                ),
                _ => continue,
            };
            object.add_relocation(section_id, Relocation {
                offset: reloc.offset as u64,
                symbol: reloc.symbol,
                addend: reloc.addend,
                flags: object::RelocationFlags::Generic {
                    kind,
                    encoding,
                    size,
                },
            }).map_err(|e| gimli::write::Error::InvalidAddress)?;
        }

        Ok(())
    }).map_err(|e: gimli::write::Error| {
        CodegenError::CraneliftError(format!("DWARF section: {}", e))
    })?;

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
