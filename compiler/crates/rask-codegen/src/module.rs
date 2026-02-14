// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Cranelift module setup and code generation orchestration.

use cranelift::prelude::*;
use cranelift_codegen::ir::GlobalValue;
use cranelift_module::{DataDescription, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use std::collections::HashMap;

use rask_mir::{MirConst, MirFunction, MirOperand};
use rask_mono::{EnumLayout, MonoProgram, StructLayout};
use crate::builder::FunctionBuilder;
use crate::types::mir_to_cranelift_type;
use crate::{CodegenError, CodegenResult};

pub struct CodeGenerator {
    module: ObjectModule,
    ctx: codegen::Context,
    func_ids: HashMap<String, cranelift_module::FuncId>,
    /// Struct layouts from monomorphization
    struct_layouts: Vec<StructLayout>,
    /// Enum layouts from monomorphization
    enum_layouts: Vec<EnumLayout>,
    /// String literal data (content → DataId in the object module)
    string_data: HashMap<String, cranelift_module::DataId>,
}

impl CodeGenerator {
    pub fn new() -> CodegenResult<Self> {
        let isa_builder = cranelift_native::builder()
            .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
        let isa = isa_builder.finish(settings::Flags::new(settings::builder()))
            .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;

        let builder = ObjectBuilder::new(
            isa,
            "rask_module",
            cranelift_module::default_libcall_names(),
        ).map_err(|e| CodegenError::CraneliftError(e.to_string()))?;

        let module = ObjectModule::new(builder);

        Ok(CodeGenerator {
            module,
            ctx: codegen::Context::new(),
            func_ids: HashMap::new(),
            struct_layouts: Vec::new(),
            enum_layouts: Vec::new(),
            string_data: HashMap::new(),
        })
    }

    /// Declare runtime functions as external imports.
    /// These are provided by the C runtime (compiler/runtime/runtime.c).
    pub fn declare_runtime_functions(&mut self) -> CodegenResult<()> {
        // rask_print_i64(val: i64) -> void
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64));
            let id = self.module
                .declare_function("rask_print_i64", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("rask_print_i64".to_string(), id);
        }

        // rask_print_bool(val: i8) -> void
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I8));
            let id = self.module
                .declare_function("rask_print_bool", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("rask_print_bool".to_string(), id);
        }

        // rask_print_newline() -> void
        {
            let sig = self.module.make_signature();
            let id = self.module
                .declare_function("rask_print_newline", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("rask_print_newline".to_string(), id);
        }

        // rask_print_string(ptr: i64) -> void
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64));
            let id = self.module
                .declare_function("rask_print_string", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("rask_print_string".to_string(), id);
        }

        // rask_print_f64(val: f64) -> void
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::F64));
            let id = self.module
                .declare_function("rask_print_f64", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("rask_print_f64".to_string(), id);
        }

        // rask_exit(code: i64) -> void
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64));
            let id = self.module
                .declare_function("rask_exit", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("rask_exit".to_string(), id);
        }

        // panic_unwrap() -> void (diverges, but declared as void return)
        {
            let sig = self.module.make_signature();
            let id = self.module
                .declare_function("rask_panic_unwrap", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("panic_unwrap".to_string(), id);
        }

        // assert_fail() -> void (diverges)
        {
            let sig = self.module.make_signature();
            let id = self.module
                .declare_function("rask_assert_fail", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("assert_fail".to_string(), id);
        }

        // ─── I/O functions ──────────────────────────────────────

        // rask_io_write(fd: i64, buf: i64, len: i64) -> i64
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64));
            sig.params.push(AbiParam::new(types::I64));
            sig.params.push(AbiParam::new(types::I64));
            sig.returns.push(AbiParam::new(types::I64));
            let id = self.module
                .declare_function("rask_io_write", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("rask_io_write".to_string(), id);
        }

        // rask_io_read(fd: i64, buf: i64, len: i64) -> i64
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64));
            sig.params.push(AbiParam::new(types::I64));
            sig.params.push(AbiParam::new(types::I64));
            sig.returns.push(AbiParam::new(types::I64));
            let id = self.module
                .declare_function("rask_io_read", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("rask_io_read".to_string(), id);
        }

        // rask_io_open(path: i64, flags: i64, mode: i64) -> i64
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64));
            sig.params.push(AbiParam::new(types::I64));
            sig.params.push(AbiParam::new(types::I64));
            sig.returns.push(AbiParam::new(types::I64));
            let id = self.module
                .declare_function("rask_io_open", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("rask_io_open".to_string(), id);
        }

        // rask_io_close(fd: i64) -> i64
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64));
            sig.returns.push(AbiParam::new(types::I64));
            let id = self.module
                .declare_function("rask_io_close", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("rask_io_close".to_string(), id);
        }

        Ok(())
    }

    /// Declare stdlib functions (Vec, Map, string, resource tracking, etc.).
    ///
    /// Call this after `declare_runtime_functions()` and before `declare_functions()`.
    /// User-defined functions declared later will shadow any matching stdlib names.
    pub fn declare_stdlib_functions(&mut self) -> CodegenResult<()> {
        crate::dispatch::declare_stdlib(&mut self.module, &mut self.func_ids)
    }

    /// Declare all functions first (for forward references).
    pub fn declare_functions(&mut self, mono: &MonoProgram, mir_functions: &[MirFunction]) -> CodegenResult<()> {
        // Store layouts for use during code generation
        self.struct_layouts = mono.struct_layouts.clone();
        self.enum_layouts = mono.enum_layouts.clone();

        for mir_fn in mir_functions {
            let mut sig = self.module.make_signature();

            // Build parameter list
            for param in &mir_fn.params {
                let param_ty = mir_to_cranelift_type(&param.ty)?;
                sig.params.push(AbiParam::new(param_ty));
            }

            // Build return type
            let ret_ty = mir_to_cranelift_type(&mir_fn.ret_ty)?;
            if !matches!(mir_fn.ret_ty, rask_mir::MirType::Void) {
                sig.returns.push(AbiParam::new(ret_ty));
            }

            // Rename "main" to "rask_main" to avoid conflict with C runtime's main()
            let export_name = if mir_fn.name == "main" {
                "rask_main"
            } else {
                &mir_fn.name
            };

            let func_id = self.module
                .declare_function(export_name, Linkage::Export, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;

            // Store under the MIR name so internal calls resolve correctly
            self.func_ids.insert(mir_fn.name.clone(), func_id);
        }
        Ok(())
    }

    /// Scan MIR functions for string constants and create data objects for each unique string.
    /// Must be called after declare_functions and before gen_function.
    pub fn register_strings(&mut self, mir_functions: &[MirFunction]) -> CodegenResult<()> {
        let mut counter = 0usize;
        for mir_fn in mir_functions {
            for block in &mir_fn.blocks {
                for stmt in &block.statements {
                    self.collect_string_constants(stmt, &mut counter)?;
                }
            }
        }
        Ok(())
    }

    fn collect_string_constants(&mut self, stmt: &rask_mir::MirStmt, counter: &mut usize) -> CodegenResult<()> {
        // Walk operands looking for string constants
        match stmt {
            rask_mir::MirStmt::Assign { rvalue, .. } => {
                self.scan_rvalue_strings(rvalue, counter)?;
            }
            rask_mir::MirStmt::Store { value, .. } => {
                self.register_operand_string(value, counter)?;
            }
            rask_mir::MirStmt::Call { args, .. } => {
                for arg in args {
                    self.register_operand_string(arg, counter)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn scan_rvalue_strings(&mut self, rvalue: &rask_mir::MirRValue, counter: &mut usize) -> CodegenResult<()> {
        match rvalue {
            rask_mir::MirRValue::Use(op) => self.register_operand_string(op, counter),
            rask_mir::MirRValue::BinaryOp { left, right, .. } => {
                self.register_operand_string(left, counter)?;
                self.register_operand_string(right, counter)
            }
            rask_mir::MirRValue::UnaryOp { operand, .. } => self.register_operand_string(operand, counter),
            rask_mir::MirRValue::Cast { value, .. } => self.register_operand_string(value, counter),
            rask_mir::MirRValue::Field { base, .. } => self.register_operand_string(base, counter),
            rask_mir::MirRValue::EnumTag { value } => self.register_operand_string(value, counter),
            rask_mir::MirRValue::Deref(op) => self.register_operand_string(op, counter),
            rask_mir::MirRValue::Ref(_) => Ok(()),
        }
    }

    fn register_operand_string(&mut self, op: &MirOperand, counter: &mut usize) -> CodegenResult<()> {
        if let MirOperand::Constant(MirConst::String(s)) = op {
            if !self.string_data.contains_key(s) {
                let name = format!(".str.{}", counter);
                *counter += 1;

                let data_id = self.module
                    .declare_data(&name, Linkage::Local, false, false)
                    .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;

                // Store null-terminated bytes
                let mut bytes = s.as_bytes().to_vec();
                bytes.push(0);

                let mut desc = DataDescription::new();
                desc.define(bytes.into_boxed_slice());

                self.module
                    .define_data(data_id, &desc)
                    .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;

                self.string_data.insert(s.clone(), data_id);
            }
        }
        Ok(())
    }

    /// Generate code for a single MIR function.
    pub fn gen_function(&mut self, mir_fn: &MirFunction) -> CodegenResult<()> {
        let func_id = self.func_ids.get(&mir_fn.name)
            .ok_or_else(|| CodegenError::FunctionNotFound(mir_fn.name.clone()))?;

        self.ctx.clear();

        // Build the signature (must match declaration)
        let mut sig = self.module.make_signature();
        for param in &mir_fn.params {
            let param_ty = mir_to_cranelift_type(&param.ty)?;
            sig.params.push(AbiParam::new(param_ty));
        }
        let ret_ty = mir_to_cranelift_type(&mir_fn.ret_ty)?;
        if !matches!(mir_fn.ret_ty, rask_mir::MirType::Void) {
            sig.returns.push(AbiParam::new(ret_ty));
        }
        self.ctx.func.signature = sig;

        // Pre-import all declared functions into this function's namespace.
        // This must happen before FunctionBuilder borrows ctx.func.
        let mut func_refs = HashMap::new();
        for (name, fid) in &self.func_ids {
            let func_ref = self.module.declare_func_in_func(*fid, &mut self.ctx.func);
            func_refs.insert(name.clone(), func_ref);
        }

        // Pre-import string data globals into this function
        let mut string_globals: HashMap<String, GlobalValue> = HashMap::new();
        for (content, data_id) in &self.string_data {
            let gv = self.module.declare_data_in_func(*data_id, &mut self.ctx.func);
            string_globals.insert(content.clone(), gv);
        }

        // Build the function
        let mut builder = FunctionBuilder::new(
            &mut self.ctx.func,
            mir_fn,
            &func_refs,
            &self.struct_layouts,
            &self.enum_layouts,
            &string_globals,
        )?;
        builder.build()?;

        // Define the function in the module
        self.module
            .define_function(*func_id, &mut self.ctx)
            .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;

        Ok(())
    }

    /// Emit the final object file. Consumes self because finish() takes ownership.
    pub fn emit_object(self, path: &str) -> CodegenResult<()> {
        let product = self.module.finish();
        let bytes = product.emit()
            .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;

        std::fs::write(path, bytes)
            .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;

        Ok(())
    }
}
