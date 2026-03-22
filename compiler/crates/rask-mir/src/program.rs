// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! MirProgram — unified container for all MIR-level program data.
//!
//! Consolidates functions, type layouts, globals, and metadata into a single
//! value that flows through optimization and codegen. Eliminates the 10+
//! loose variables currently threaded through the compile pipeline.

use std::collections::HashMap;

use rask_mono::{EnumLayout, StructLayout};

use crate::lower::ComptimeGlobalMeta;
use crate::MirFunction;

/// Complete MIR program — everything needed after lowering.
#[derive(Debug, Clone)]
pub struct MirProgram {
    /// All lowered (and optimized) functions.
    pub functions: Vec<MirFunction>,

    /// Struct layouts indexed by StructLayoutId.
    pub struct_layouts: Vec<StructLayout>,

    /// Enum layouts indexed by EnumLayoutId.
    pub enum_layouts: Vec<EnumLayout>,

    /// Compile-time evaluated global data sections.
    pub comptime_globals: HashMap<String, ComptimeGlobalMeta>,

    /// Trait name → method names, for vtable construction.
    pub trait_methods: HashMap<String, Vec<String>>,

    /// Source file path (for error messages in generated code).
    pub source_file: Option<String>,
}

impl MirProgram {
    /// Look up a struct layout by id.
    pub fn struct_layout(&self, id: crate::StructLayoutId) -> &StructLayout {
        &self.struct_layouts[id.id as usize]
    }

    /// Look up an enum layout by id.
    pub fn enum_layout(&self, id: crate::EnumLayoutId) -> &EnumLayout {
        &self.enum_layouts[id.id as usize]
    }

    /// Iterate over all functions.
    pub fn functions(&self) -> &[MirFunction] {
        &self.functions
    }

    /// Mutable access to all functions (for optimization passes).
    pub fn functions_mut(&mut self) -> &mut Vec<MirFunction> {
        &mut self.functions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlockId, MirBlock, MirLocal, MirTerminator, MirTerminatorKind, MirType};

    fn empty_program() -> MirProgram {
        MirProgram {
            functions: vec![],
            struct_layouts: vec![],
            enum_layouts: vec![],
            comptime_globals: HashMap::new(),
            trait_methods: HashMap::new(),
            source_file: None,
        }
    }

    #[test]
    fn empty_program_has_no_functions() {
        let prog = empty_program();
        assert!(prog.functions().is_empty());
    }

    #[test]
    fn add_and_access_function() {
        let mut prog = empty_program();
        prog.functions.push(MirFunction {
            name: "main".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals: vec![MirLocal {
                id: crate::LocalId(0),
                name: Some("x".to_string()),
                ty: MirType::I32,
                is_param: false,
            }],
            blocks: vec![MirBlock {
                id: BlockId(0),
                statements: vec![],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        });
        assert_eq!(prog.functions().len(), 1);
        assert_eq!(prog.functions()[0].name, "main");
    }

    #[test]
    fn struct_layout_lookup() {
        let mut prog = empty_program();
        prog.struct_layouts.push(StructLayout {
            name: "Point".to_string(),
            size: 16,
            align: 8,
            fields: vec![],
        });
        let layout = prog.struct_layout(crate::StructLayoutId::new(0, 16, 8));
        assert_eq!(layout.name, "Point");
        assert_eq!(layout.size, 16);
    }

    #[test]
    fn mutable_function_access() {
        let mut prog = empty_program();
        prog.functions.push(MirFunction {
            name: "foo".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals: vec![],
            blocks: vec![MirBlock {
                id: BlockId(0),
                statements: vec![],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        });
        prog.functions_mut()[0].name = "bar".to_string();
        assert_eq!(prog.functions()[0].name, "bar");
    }
}
