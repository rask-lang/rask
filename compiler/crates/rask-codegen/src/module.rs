// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Cranelift module setup and code generation orchestration.

use cranelift::prelude::*;
use cranelift_codegen::ir::GlobalValue;
use cranelift_module::{DataDescription, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use std::collections::{HashMap, HashSet};

use rask_ast::LineMap;
use rask_mir::{MirConst, MirFunction, MirOperand};
use rask_mono::{EnumLayout, MonoProgram, StructLayout};
use crate::builder::FunctionBuilder;
use crate::types::{mir_to_cranelift_type, type_string_to_mir};
use crate::{BuildMode, CodegenError, CodegenResult};

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
    /// Element string-offset lists, one data object per distinct list.
    ///
    /// A container's `free` is handed the map of where the strings sit inside
    /// one of its elements. The lists are tiny and repeat heavily — `[0]` for
    /// every `Vec<string>` in the program — so they're keyed by content.
    element_offset_data: HashMap<Vec<i32>, cranelift_module::DataId>,
    /// The same literals again, laid out as a `RaskStr` heap header:
    /// `[refcount: u32 = sentinel][capacity: u32][bytes][NUL]`.
    ///
    /// A literal used to be materialized by calling `rask_string_from` at every
    /// use, which allocates and sets the refcount to 1 — so every use of a
    /// literal longer than fifteen bytes was a fresh allocation that nothing
    /// ever freed, because `RE3` exempts literals from RC ops on the grounds
    /// that they carry a sentinel refcount. They didn't. Now they do, and the
    /// call goes away with the allocation.
    string_header_data: HashMap<String, cranelift_module::DataId>,
    /// Comptime global data (const name → DataId in the object module)
    pub comptime_data: HashMap<String, cranelift_module::DataId>,
    /// MIR names of stdlib functions that can panic at runtime
    panicking_fns: HashSet<String>,
    /// Names of functions compiled as Rask code (not C stdlib)
    internal_fns: HashSet<String>,
    /// Declared param types per Rask function. Call sites need these to pass
    /// aggregates by pointer even when the caller's own local is a scalar.
    fn_param_types: HashMap<String, Vec<rask_mir::MirType>>,
    /// Debug or Release — controls inlining of pool checks
    build_mode: BuildMode,
    /// VTable data sections for trait objects (vtable_name → DataId)
    vtable_data: HashMap<String, cranelift_module::DataId>,
    /// Per-concrete-type drop glue, generated once and shared across every
    /// vtable that boxes the same type behind a different trait.
    drop_glue_fns: HashMap<String, cranelift_module::FuncId>,
    /// Collected debug info per function (debug builds only)
    debug_srclocs: Vec<crate::debug_info::FunctionDebugInfo>,
    /// Line map for converting byte offsets to line:col (debug builds)
    line_map: Option<LineMap>,
    /// Source file name for DWARF emission
    source_file_name: Option<String>,
    /// DI5: inline region metadata from the inlining pass (caller name → regions)
    inline_regions: HashMap<String, Vec<rask_mir::InlineRegion>>,
}

impl CodeGenerator {
    pub fn new(build_mode: BuildMode) -> CodegenResult<Self> {
        let isa_builder = cranelift_native::builder()
            .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
        let mut flag_builder = settings::builder();
        let _ = flag_builder.set("opt_level", "speed");
        // Without this Cranelift refuses an `i128` in any signature, including
        // the runtime imports 128-bit arithmetic calls into. The extension is
        // LLVM's own convention — a register pair on SysV — which is what a C
        // compiler already does with `__int128`, so the two agree (#762).
        let _ = flag_builder.set("enable_llvm_abi_extensions", "true");
        let isa = isa_builder.finish(settings::Flags::new(flag_builder))
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
            string_header_data: HashMap::new(),
            element_offset_data: HashMap::new(),
            comptime_data: HashMap::new(),
            panicking_fns: crate::dispatch::panicking_functions(),
            internal_fns: HashSet::new(),
            fn_param_types: HashMap::new(),
            build_mode,
            vtable_data: HashMap::new(),
            drop_glue_fns: HashMap::new(),
            debug_srclocs: Vec::new(),
            line_map: None,
            source_file_name: None,
            inline_regions: HashMap::new(),
        })
    }

    /// Create a code generator targeting a specific platform (XT2).
    pub fn new_with_target(triple: &str, build_mode: BuildMode) -> CodegenResult<Self> {
        use std::str::FromStr;
        let target = target_lexicon::Triple::from_str(triple)
            .map_err(|e| CodegenError::CraneliftError(format!("invalid target '{}': {}", triple, e)))?;

        let mut flag_builder = settings::builder();
        let _ = flag_builder.set("opt_level", "speed");
        // See `new` — 128-bit values in signatures need this (#762).
        let _ = flag_builder.set("enable_llvm_abi_extensions", "true");
        // Set is_pic for position-independent code on relevant targets
        if matches!(target.operating_system, target_lexicon::OperatingSystem::Linux) {
            let _ = flag_builder.set("is_pic", "true");
        }
        let flags = settings::Flags::new(flag_builder);

        let isa = isa::lookup(target)
            .map_err(|e| CodegenError::CraneliftError(format!("unsupported target '{}': {}", triple, e)))?
            .finish(flags)
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
            string_header_data: HashMap::new(),
            element_offset_data: HashMap::new(),
            comptime_data: HashMap::new(),
            panicking_fns: crate::dispatch::panicking_functions(),
            internal_fns: HashSet::new(),
            fn_param_types: HashMap::new(),
            build_mode,
            vtable_data: HashMap::new(),
            drop_glue_fns: HashMap::new(),
            debug_srclocs: Vec::new(),
            line_map: None,
            source_file_name: None,
            inline_regions: HashMap::new(),
        })
    }

    /// Set debug info context for DWARF emission.
    /// Call before gen_function() if you want debug line tables.
    pub fn set_debug_context(&mut self, source_file: &str, line_map: LineMap) {
        self.source_file_name = Some(source_file.to_string());
        self.line_map = Some(line_map);
        // Register source file name in data section for error origin (ER15)
        let _ = self.register_string(source_file);
    }

    /// Set DI5 inline region metadata from the inlining pass.
    /// Call before gen_function() in debug builds.
    pub fn set_inline_regions(&mut self, regions: HashMap<String, Vec<rask_mir::InlineRegion>>) {
        self.inline_regions = regions;
    }

    /// Declare runtime functions as external imports.
    /// These are provided by the C runtime (compiler/runtime/runtime.c).
    pub fn declare_runtime_functions(&mut self) -> CodegenResult<()> {
        // stdlib `math` — every entry is f64 in, f64 or bool out, so declare
        // them from a table instead of 19 near-identical blocks. Symbol names
        // match what MIR mangles for `math.foo(x)`; runtime/math.c provides them.
        {
            const MATH_F64: &[(&str, usize)] = &[
                ("math_sin", 1), ("math_cos", 1), ("math_tan", 1),
                ("math_asin", 1), ("math_acos", 1), ("math_atan", 1),
                ("math_atan2", 2),
                ("math_exp", 1), ("math_ln", 1), ("math_log2", 1), ("math_log10", 1),
                ("math_hypot", 2),
                ("math_to_radians", 1), ("math_to_degrees", 1),
            ];
            // is_nan/is_inf/is_finite are f64 methods (not math module functions);
            // their MIR call name is `f64_is_nan` etc. — declared as stdlib
            // entries in dispatch.rs, which reuse the same `math_is_nan`-family
            // C symbols runtime/math.c provides.

            for (name, arity) in MATH_F64 {
                let mut sig = self.module.make_signature();
                for _ in 0..*arity {
                    sig.params.push(AbiParam::new(types::F64));
                }
                sig.returns.push(AbiParam::new(types::F64));
                let id = self.module
                    .declare_function(name, Linkage::Import, &sig)
                    .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
                self.func_ids.insert((*name).to_string(), id);
            }
        }

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

        // Stream locks bracketing one print/println call, so its writes land
        // together instead of splicing into another thread's line; and the FFI
        // panic boundary bracketing an `extern "C"` export's body.
        for name in ["rask_print_lock", "rask_print_unlock",
                     "rask_eprint_lock", "rask_eprint_unlock",
                     "rask_ffi_boundary_enter", "rask_ffi_boundary_exit"] {
            let sig = self.module.make_signature();
            let id = self.module
                .declare_function(name, Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert(name.to_string(), id);
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

        // rask_print_f32(val: f32) -> void
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::F32));
            let id = self.module
                .declare_function("rask_print_f32", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("rask_print_f32".to_string(), id);
        }

        // rask_print_char(codepoint: i32) -> void
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I32));
            let id = self.module
                .declare_function("rask_print_char", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("rask_print_char".to_string(), id);
        }

        // rask_print_u64(val: i64) -> void
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64));
            let id = self.module
                .declare_function("rask_print_u64", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("rask_print_u64".to_string(), id);
        }

        // The stderr half of the print family — same signatures, one per type,
        // so `eprint`/`eprintln` lower exactly like `print`/`println` with the
        // other set of symbols.
        for (name, param) in [
            ("rask_eprint_i64", Some(types::I64)),
            ("rask_eprint_bool", Some(types::I8)),
            ("rask_eprint_string", Some(types::I64)),
            ("rask_eprint_f64", Some(types::F64)),
            ("rask_eprint_f32", Some(types::F32)),
            ("rask_eprint_char", Some(types::I32)),
            ("rask_eprint_u64", Some(types::I64)),
            ("rask_eprint_i128", Some(types::I128)),
            ("rask_eprint_u128", Some(types::I128)),
            ("rask_eprint_newline", None),
        ] {
            let mut sig = self.module.make_signature();
            if let Some(p) = param {
                sig.params.push(AbiParam::new(p));
            }
            let id = self.module
                .declare_function(name, Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert(name.to_string(), id);
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

        // panic_unwrap(was_error: i32) -> void (diverges, but declared as void return)
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I32)); // was_error
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

        // panic_unwrap_at(file: ptr, line: i32, col: i32, was_error: i32) -> void
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64)); // file ptr
            sig.params.push(AbiParam::new(types::I32)); // line
            sig.params.push(AbiParam::new(types::I32)); // col
            sig.params.push(AbiParam::new(types::I32)); // was_error
            let id = self.module
                .declare_function("rask_panic_unwrap_at", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("panic_unwrap_at".to_string(), id);
        }

        // assert_fail_at(file: ptr, line: i32, col: i32) -> void (diverges)
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64)); // file ptr
            sig.params.push(AbiParam::new(types::I32)); // line
            sig.params.push(AbiParam::new(types::I32)); // col
            let id = self.module
                .declare_function("rask_assert_fail_at", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("assert_fail_at".to_string(), id);
        }

        // assert_fail_msg_at(msg: ptr, file: ptr, line: i32, col: i32) -> void
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64)); // msg ptr
            sig.params.push(AbiParam::new(types::I64)); // file ptr
            sig.params.push(AbiParam::new(types::I32)); // line
            sig.params.push(AbiParam::new(types::I32)); // col
            let id = self.module
                .declare_function("rask_assert_fail_msg_at", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("assert_fail_msg_at".to_string(), id);
        }

        // assert_fail_cmp_i64(left: i64, right: i64, op: ptr, file: ptr, line: i32, col: i32)
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64)); // left
            sig.params.push(AbiParam::new(types::I64)); // right
            sig.params.push(AbiParam::new(types::I64)); // op str ptr
            sig.params.push(AbiParam::new(types::I64)); // file ptr
            sig.params.push(AbiParam::new(types::I32)); // line
            sig.params.push(AbiParam::new(types::I32)); // col
            let id = self.module
                .declare_function("rask_assert_fail_cmp_i64", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("assert_fail_cmp_i64".to_string(), id);
        }

        // assert_fail_cmp_i128 / _u128 — same shape, 128-bit operands. The i64
        // helper can't stand in: narrowing to report the values would print
        // exactly the wrong ones, since a 128-bit assertion is about values
        // that don't fit 64 bits (#762).
        for (mir_name, c_name) in [
            ("assert_fail_cmp_i128", "rask_assert_fail_cmp_i128"),
            ("assert_fail_cmp_u128", "rask_assert_fail_cmp_u128"),
        ] {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I128)); // left
            sig.params.push(AbiParam::new(types::I128)); // right
            sig.params.push(AbiParam::new(types::I64));  // op str ptr
            sig.params.push(AbiParam::new(types::I64));  // file ptr
            sig.params.push(AbiParam::new(types::I32));  // line
            sig.params.push(AbiParam::new(types::I32));  // col
            let id = self.module
                .declare_function(c_name, Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert(mir_name.to_string(), id);
        }

        // assert_fail_cmp_char(left: i64, right: i64, op: ptr, file: ptr, line: i32, col: i32)
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64)); // left codepoint
            sig.params.push(AbiParam::new(types::I64)); // right codepoint
            sig.params.push(AbiParam::new(types::I64)); // op str ptr
            sig.params.push(AbiParam::new(types::I64)); // file ptr
            sig.params.push(AbiParam::new(types::I32)); // line
            sig.params.push(AbiParam::new(types::I32)); // col
            let id = self.module
                .declare_function("rask_assert_fail_cmp_char", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("assert_fail_cmp_char".to_string(), id);
        }

        // main_error_exit(msg: *RaskStr | null) — prints and exits 1 (EX4)
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64)); // message str ptr (may be null)
            let id = self.module
                .declare_function("rask_main_error_exit", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("main_error_exit".to_string(), id);
        }

        // assert_fail_cmp_str(left: ptr, right: ptr, op: ptr, file: ptr, line: i32, col: i32)
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64)); // left str ptr
            sig.params.push(AbiParam::new(types::I64)); // right str ptr
            sig.params.push(AbiParam::new(types::I64)); // op str ptr
            sig.params.push(AbiParam::new(types::I64)); // file ptr
            sig.params.push(AbiParam::new(types::I32)); // line
            sig.params.push(AbiParam::new(types::I32)); // col
            let id = self.module
                .declare_function("rask_assert_fail_cmp_str", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("assert_fail_cmp_str".to_string(), id);
        }

        // assert_fail_cmp_f64/_f32(left, right, op: ptr, file: ptr, line: i32, col: i32)
        // Two widths, not one: the shortest round-tripping decimal depends on
        // the width it's checked against, so an f32 widened to double reports
        // its exact binary expansion instead of what `println` shows.
        for (internal, symbol, value_ty) in [
            ("assert_fail_cmp_f64", "rask_assert_fail_cmp_f64", types::F64),
            ("assert_fail_cmp_f32", "rask_assert_fail_cmp_f32", types::F32),
        ] {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(value_ty)); // left
            sig.params.push(AbiParam::new(value_ty)); // right
            sig.params.push(AbiParam::new(types::I64)); // op str ptr
            sig.params.push(AbiParam::new(types::I64)); // file ptr
            sig.params.push(AbiParam::new(types::I32)); // line
            sig.params.push(AbiParam::new(types::I32)); // col
            let id = self.module
                .declare_function(symbol, Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert(internal.to_string(), id);
        }

        // pool_get_checked(pool: i64, handle: i64, file: ptr, line: i32, col: i32) -> ptr
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64)); // pool
            sig.params.push(AbiParam::new(types::I64)); // packed handle
            sig.params.push(AbiParam::new(types::I64)); // file ptr
            sig.params.push(AbiParam::new(types::I32)); // line
            sig.params.push(AbiParam::new(types::I32)); // col
            sig.returns.push(AbiParam::new(types::I64)); // element ptr
            let id = self.module
                .declare_function("rask_pool_get_checked", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("pool_get_checked".to_string(), id);
        }

        // rask_panic_at(file: ptr, line: i32, col: i32, msg: ptr) -> noreturn
        // Used by inline pool access (release mode) for bounds/generation failures.
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64)); // file ptr
            sig.params.push(AbiParam::new(types::I32)); // line
            sig.params.push(AbiParam::new(types::I32)); // col
            sig.params.push(AbiParam::new(types::I64)); // msg ptr
            let id = self.module
                .declare_function("rask_panic_at", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("panic_at".to_string(), id);
        }

        // The checked-arithmetic panics that name their operands (ctrl.panic/F3).
        // `tail` is the static "<type> range [min, max]" half codegen already
        // registered as a string global; the operands come from the live values
        // at the check.
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64)); // file ptr
            sig.params.push(AbiParam::new(types::I32)); // line
            sig.params.push(AbiParam::new(types::I32)); // col
            sig.params.push(AbiParam::new(types::I64)); // op symbol ptr
            sig.params.push(AbiParam::new(types::I64)); // tail ptr
            sig.params.push(AbiParam::new(types::I64)); // lhs
            sig.params.push(AbiParam::new(types::I64)); // rhs
            sig.params.push(AbiParam::new(types::I32)); // is_unsigned
            let id = self.module
                .declare_function("rask_panic_overflow_binary", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("panic_overflow_binary".to_string(), id);
        }
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64)); // file ptr
            sig.params.push(AbiParam::new(types::I32)); // line
            sig.params.push(AbiParam::new(types::I32)); // col
            sig.params.push(AbiParam::new(types::I64)); // tail ptr
            sig.params.push(AbiParam::new(types::I64)); // operand
            let id = self.module
                .declare_function("rask_panic_overflow_neg", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("panic_overflow_neg".to_string(), id);
        }
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64)); // file ptr
            sig.params.push(AbiParam::new(types::I32)); // line
            sig.params.push(AbiParam::new(types::I32)); // col
            sig.params.push(AbiParam::new(types::I64)); // tail ptr
            sig.params.push(AbiParam::new(types::I64)); // amount
            let id = self.module
                .declare_function("rask_panic_shift_amount", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("panic_shift_amount".to_string(), id);
        }
        // The 128-bit forms: same messages, operands passed at their own width.
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64));  // file ptr
            sig.params.push(AbiParam::new(types::I32));  // line
            sig.params.push(AbiParam::new(types::I32));  // col
            sig.params.push(AbiParam::new(types::I64));  // op symbol ptr
            sig.params.push(AbiParam::new(types::I64));  // tail ptr
            sig.params.push(AbiParam::new(types::I128)); // lhs
            sig.params.push(AbiParam::new(types::I128)); // rhs
            sig.params.push(AbiParam::new(types::I32));  // is_unsigned
            let id = self.module
                .declare_function("rask_panic_overflow_binary_i128", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("panic_overflow_binary_i128".to_string(), id);
        }
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64));  // file ptr
            sig.params.push(AbiParam::new(types::I32));  // line
            sig.params.push(AbiParam::new(types::I32));  // col
            sig.params.push(AbiParam::new(types::I64));  // tail ptr
            sig.params.push(AbiParam::new(types::I128)); // operand
            let id = self.module
                .declare_function("rask_panic_overflow_neg_i128", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("panic_overflow_neg_i128".to_string(), id);
        }

        // set_panic_location(file: ptr, line: i32, col: i32) -> void
        // Codegen calls this before any runtime function that can panic.
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64)); // file ptr
            sig.params.push(AbiParam::new(types::I32)); // line
            sig.params.push(AbiParam::new(types::I32)); // col
            let id = self.module
                .declare_function("rask_set_panic_location", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("set_panic_location".to_string(), id);
        }

        // rask_set_origin_file(file: *const char) -> void
        // Sets the source file name for error origin formatting (ER15).
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64)); // file name C string
            let id = self.module
                .declare_function("rask_set_origin_file", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("rask_set_origin_file".to_string(), id);
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

        // ─── Allocator (used by closure heap allocation) ─────────

        // rask_alloc(size: i64) -> i64 (pointer)
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64));
            sig.returns.push(AbiParam::new(types::I64));
            let id = self.module
                .declare_function("rask_alloc", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("rask_alloc".to_string(), id);
        }

        // rask_free(ptr: i64) -> void
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64));
            let id = self.module
                .declare_function("rask_free", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("rask_free".to_string(), id);
        }

        // rask_closure_alloc(block_size: i64) -> i64 (pointer past the header)
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64));
            sig.returns.push(AbiParam::new(types::I64));
            let id = self.module
                .declare_function("rask_closure_alloc", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("rask_closure_alloc".to_string(), id);
        }

        // rask_closure_free(ptr: i64) -> void
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64));
            let id = self.module
                .declare_function("rask_closure_free", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("rask_closure_free".to_string(), id);
        }

        // rask_bench_run(fn_ptr: i64, name_ptr: i64) -> void
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64)); // function pointer
            sig.params.push(AbiParam::new(types::I64)); // name (const char*)
            let id = self.module
                .declare_function("rask_bench_run", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("rask_bench_run".to_string(), id);
        }

        // rask_test_run(fn_ptr: i64, name_ptr: i64) -> i32 (0=pass, 1=fail)
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64)); // function pointer
            sig.params.push(AbiParam::new(types::I64)); // name (const char*)
            sig.returns.push(AbiParam::new(types::I32)); // 0=pass, 1=fail
            let id = self.module
                .declare_function("rask_test_run", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("rask_test_run".to_string(), id);
        }

        // assert_eq failure reporters — the comparison is generated inline, so
        // these only format. One per operand shape (i64/char/f64/str/none).
        for (internal, symbol, value_ty) in [
            ("assert_eq_fail_i64", "rask_assert_eq_fail_i64", Some(types::I64)),
            ("assert_eq_fail_bool", "rask_assert_eq_fail_bool", Some(types::I64)),
            ("assert_eq_fail_char", "rask_assert_eq_fail_char", Some(types::I64)),
            ("assert_eq_fail_f64", "rask_assert_eq_fail_f64", Some(types::F64)),
            ("assert_eq_fail_f32", "rask_assert_eq_fail_f32", Some(types::F32)),
            ("assert_eq_fail_str", "rask_assert_eq_fail_str", Some(types::I64)),
            ("assert_eq_fail", "rask_assert_eq_fail", None),
        ] {
            let mut sig = self.module.make_signature();
            if let Some(ty) = value_ty {
                sig.params.push(AbiParam::new(ty)); // got
                sig.params.push(AbiParam::new(ty)); // expected
            }
            sig.params.push(AbiParam::new(types::I64)); // file ptr
            sig.params.push(AbiParam::new(types::I32)); // line
            sig.params.push(AbiParam::new(types::I32)); // col
            let id = self.module
                .declare_function(symbol, Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert(internal.to_string(), id);
        }

        // rask_test_skip(reason: ptr) -> noreturn (unwinds via panic)
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64));
            let id = self.module
                .declare_function("rask_test_skip", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("rask_test_skip".to_string(), id);
        }

        // rask_test_skip_flag() -> void (sets thread-local skip flag)
        {
            let sig = self.module.make_signature();
            let id = self.module
                .declare_function("rask_test_skip_flag", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("rask_test_skip_flag".to_string(), id);
        }

        // rask_test_expect_fail() -> void (sets thread-local flag)
        {
            let sig = self.module.make_signature();
            let id = self.module
                .declare_function("rask_test_expect_fail", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("rask_test_expect_fail".to_string(), id);
        }

        // rask_check_fail(msg: ptr) -> void (records failure, doesn't unwind)
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64));
            let id = self.module
                .declare_function("rask_check_fail", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("rask_check_fail".to_string(), id);
        }

        // rask_i64_to_string(out: *RaskStr, val: i64) -> void
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64)); // out ptr
            sig.params.push(AbiParam::new(types::I64)); // val
            let id = self.module
                .declare_function("rask_i64_to_string", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("rask_i64_to_string".to_string(), id);
        }

        // 128-bit helpers (#762). Cranelift lowers `iadd`/`isub` and their
        // overflow forms at `I128`, but not `smul_overflow`/`umul_overflow` and
        // not division or remainder at all — those come through the runtime,
        // returning a status the caller turns into the usual panic.
        for name in [
            "rask_i128_mul", "rask_u128_mul",
            "rask_i128_div", "rask_i128_rem",
            "rask_u128_div", "rask_u128_rem",
        ] {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I128)); // a
            sig.params.push(AbiParam::new(types::I128)); // b
            sig.params.push(AbiParam::new(types::I64));  // out ptr
            sig.returns.push(AbiParam::new(types::I32)); // 0 ok, 1 div-zero, 2 overflow
            let id = self.module
                .declare_function(name, Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert(name.to_string(), id);
        }

        // rask_i128_to_string / rask_u128_to_string(out: *RaskStr, val: i128)
        for name in ["rask_i128_to_string", "rask_u128_to_string"] {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64));  // out ptr
            sig.params.push(AbiParam::new(types::I128)); // val
            let id = self.module
                .declare_function(name, Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert(name.to_string(), id);
        }

        // rask_print_i128 / rask_print_u128(val: i128) -> void
        for name in ["rask_print_i128", "rask_print_u128"] {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I128));
            let id = self.module
                .declare_function(name, Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert(name.to_string(), id);
        }

        // rask_bool_to_string(out: *RaskStr, val: i64) -> void
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64)); // out ptr
            sig.params.push(AbiParam::new(types::I64)); // val
            let id = self.module
                .declare_function("rask_bool_to_string", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("rask_bool_to_string".to_string(), id);
        }

        // rask_f64_to_string(out: *RaskStr, val: f64) -> void
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64)); // out ptr
            sig.params.push(AbiParam::new(types::F64)); // val
            let id = self.module
                .declare_function("rask_f64_to_string", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("rask_f64_to_string".to_string(), id);
        }

        // rask_f32_to_string(out: *RaskStr, val: f32) -> void
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64)); // out ptr
            sig.params.push(AbiParam::new(types::F32)); // val
            let id = self.module
                .declare_function("rask_f32_to_string", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("rask_f32_to_string".to_string(), id);
        }

        // rask_char_to_string(out: *RaskStr, codepoint: i32) -> void
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(types::I64)); // out ptr
            sig.params.push(AbiParam::new(types::I32)); // codepoint
            let id = self.module
                .declare_function("rask_char_to_string", Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert("rask_char_to_string".to_string(), id);
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

    /// Declare extern "C" functions as imported symbols.
    ///
    /// Each extern decl becomes a Cranelift function import with the declared
    /// parameter and return types. The linker resolves these to actual symbols.
    pub fn declare_extern_functions(&mut self, extern_decls: &[crate::ExternFuncSig]) -> CodegenResult<()> {
        for decl in extern_decls {
            // Skip if already declared (e.g. a runtime or stdlib function with the same name)
            if self.func_ids.contains_key(&decl.name) {
                continue;
            }
            let mut sig = self.module.make_signature();
            for param_ty in &decl.param_types {
                let mir_ty = type_string_to_mir(param_ty);
                let cl_ty = mir_to_cranelift_type(&mir_ty)?;
                sig.params.push(AbiParam::new(cl_ty));
            }
            if let Some(ret) = &decl.ret_ty {
                let mir_ty = type_string_to_mir(ret);
                if !matches!(mir_ty, rask_mir::MirType::Void) {
                    let cl_ty = mir_to_cranelift_type(&mir_ty)?;
                    sig.returns.push(AbiParam::new(cl_ty));
                }
            }
            let func_id = self.module
                .declare_function(&decl.name, Linkage::Import, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.func_ids.insert(decl.name.clone(), func_id);
        }
        Ok(())
    }

    /// Declare all functions first (for forward references).
    pub fn declare_functions(&mut self, mono: &MonoProgram, mir_functions: &[MirFunction]) -> CodegenResult<()> {
        // Store layouts for use during code generation
        self.struct_layouts = mono.struct_layouts.clone();
        self.enum_layouts = mono.enum_layouts.clone();

        for mir_fn in mir_functions {
            // Skip empty-body stubs that shadow stdlib entries (e.g.
            // fs.write_bytes has an empty .rk body but dispatches to
            // rask_fs_write_bytes in the C runtime via the dispatch table).
            if self.func_ids.contains_key(&mir_fn.name) && is_empty_stub(mir_fn) {
                continue;
            }
            let mut sig = self.module.make_signature();

            // Build parameter list
            for param in &mir_fn.params {
                let param_ty = mir_to_cranelift_type(&param.ty)?;
                sig.params.push(AbiParam::new(param_ty));
            }

            // Build return type.
            // "main" is called from C as void rask_main(void), so it must
            // not declare a return type even when the Rask source returns a Result.
            let is_main = mir_fn.name == "main";
            let ret_ty = mir_to_cranelift_type(&mir_fn.ret_ty)?;
            if !matches!(mir_fn.ret_ty, rask_mir::MirType::Void) && !is_main {
                sig.returns.push(AbiParam::new(ret_ty));
            }

            // extern "C" functions keep their exact name for C ABI compatibility.
            // Regular "main" is renamed to "rask_main" to avoid conflict with C runtime's main().
            let export_name = if mir_fn.is_extern_c {
                &mir_fn.name
            } else if mir_fn.name == "main" {
                "rask_main"
            } else {
                &mir_fn.name
            };

            let func_id = self.module
                .declare_function(export_name, Linkage::Export, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;

            // Store under the MIR name so internal calls resolve correctly
            self.func_ids.insert(mir_fn.name.clone(), func_id);
            self.internal_fns.insert(mir_fn.name.clone());
            self.fn_param_types.insert(
                mir_fn.name.clone(),
                mir_fn.params.iter().map(|p| p.ty.clone()).collect(),
            );
        }
        Ok(())
    }

    /// Scan MIR functions for string constants and create data objects for each unique string.
    /// Must be called after declare_functions and before gen_function.
    pub fn register_strings(&mut self, mir_functions: &[MirFunction]) -> CodegenResult<()> {
        let mut counter = 0usize;

        // Pre-register the separator string for multi-arg print/println calls
        let needs_separator = mir_functions.iter().any(|f| {
            f.blocks.iter().any(|b| {
                b.statements.iter().any(|s| {
                    matches!(&s.kind, rask_mir::MirStmtKind::Call { func, args, .. }
                        if matches!(func.name.as_str(),
                                    "print" | "println" | "eprint" | "eprintln")
                            && args.len() > 1)
                })
            })
        });
        if needs_separator {
            self.register_operand_string(
                &MirOperand::Constant(MirConst::String(" ".to_string())),
                &mut counter,
            )?;
        }

        // Checked-arithmetic panic messages (type.overflow). Registered in all
        // builds so overflow panics print a message consistently (OV4).
        for &msg in crate::builder::OVERFLOW_MESSAGES {
            self.register_string(msg)?;
        }
        for &msg in crate::builder::OVERFLOW_FALLBACKS {
            self.register_string(msg)?;
        }
        for &sym in crate::builder::OVERFLOW_OP_SYMBOLS {
            self.register_string(sym)?;
        }

        // Pre-register panic message for inline pool access (release mode)
        if self.build_mode == BuildMode::Release {
            let has_pool_access = mir_functions.iter().any(|f| {
                f.blocks.iter().any(|b| {
                    b.statements.iter().any(|s| matches!(s.kind, rask_mir::MirStmtKind::PoolCheckedAccess { .. }))
                })
            });
            if has_pool_access {
                self.register_string("pool access with invalid handle")?;
            }
        }

        for mir_fn in mir_functions {
            for block in &mir_fn.blocks {
                for stmt in &block.statements {
                    self.collect_string_constants(stmt, &mut counter)?;
                }
                // Also scan terminators for string constants (e.g. return "hello")
                self.collect_terminator_strings(&block.terminator, &mut counter)?;
            }
        }
        Ok(())
    }

    fn collect_string_constants(&mut self, stmt: &rask_mir::MirStmt, counter: &mut usize) -> CodegenResult<()> {
        // Walk operands looking for string constants
        match &stmt.kind {
            rask_mir::MirStmtKind::Assign { rvalue, .. } => {
                self.scan_rvalue_strings(rvalue, counter)?;
            }
            rask_mir::MirStmtKind::Store { value, .. } => {
                self.register_operand_string(value, counter)?;
            }
            rask_mir::MirStmtKind::Call { args, .. }
            | rask_mir::MirStmtKind::ClosureCall { args, .. } => {
                for arg in args {
                    self.register_operand_string(arg, counter)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn collect_terminator_strings(&mut self, term: &rask_mir::MirTerminator, counter: &mut usize) -> CodegenResult<()> {
        match &term.kind {
            rask_mir::MirTerminatorKind::Return { value: Some(op) } => {
                self.register_operand_string(op, counter)?;
            }
            rask_mir::MirTerminatorKind::Switch { value, .. } => {
                self.register_operand_string(value, counter)?;
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
            rask_mir::MirRValue::Cast { value, .. } | rask_mir::MirRValue::Convert { value, .. } => self.register_operand_string(value, counter),
            rask_mir::MirRValue::Field { base, .. } => self.register_operand_string(base, counter),
            rask_mir::MirRValue::EnumTag { value } => self.register_operand_string(value, counter),
            rask_mir::MirRValue::Deref(op) => self.register_operand_string(op, counter),
            rask_mir::MirRValue::Ref(_) => Ok(()),
            rask_mir::MirRValue::ArrayIndex { base, index, .. } => {
                self.register_operand_string(base, counter)?;
                self.register_operand_string(index, counter)
            }
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
                self.register_string_header(s)?;
            }
        }
        Ok(())
    }

    /// Emit a literal in `RaskStr` heap-header form, with a sentinel refcount.
    ///
    /// Only for literals that don't fit SSO — a short one is built from two
    /// immediates at the use site and has no refcount at all.
    fn register_string_header(&mut self, s: &str) -> CodegenResult<()> {
        const SSO_MAX: usize = 15;
        if s.len() <= SSO_MAX || self.string_header_data.contains_key(s) {
            return Ok(());
        }
        // Numbered from this map's own length — sharing the `.str.N` counter
        // let the two sequences collide once anything registered a string
        // outside the MIR walk.
        let name = format!(".strhdr.{}", self.string_header_data.len());

        let data_id = self
            .module
            .declare_data(&name, Linkage::Local, false, false)
            .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;

        let mut bytes = Vec::with_capacity(8 + s.len() + 1);
        bytes.extend_from_slice(&u32::MAX.to_le_bytes()); // refcount: never freed
        bytes.extend_from_slice(&(s.len() as u32).to_le_bytes()); // capacity
        bytes.extend_from_slice(s.as_bytes());
        bytes.push(0);

        let mut desc = DataDescription::new();
        desc.define(bytes.into_boxed_slice());
        // The runtime reads the two u32s straight out of the header.
        desc.set_align(8);

        self.module
            .define_data(data_id, &desc)
            .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;

        self.string_header_data.insert(s.to_string(), data_id);
        Ok(())
    }

    /// Emit one offset list as read-only data.
    fn register_element_offsets(&mut self, offsets: &[i32]) -> CodegenResult<()> {
        if offsets.is_empty() || self.element_offset_data.contains_key(offsets) {
            return Ok(());
        }
        let name = format!(".elemoffs.{}", self.element_offset_data.len());
        let data_id = self
            .module
            .declare_data(&name, Linkage::Local, false, false)
            .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;

        let mut bytes = Vec::with_capacity(offsets.len() * 4);
        for off in offsets {
            bytes.extend_from_slice(&off.to_le_bytes());
        }
        let mut desc = DataDescription::new();
        desc.define(bytes.into_boxed_slice());
        desc.set_align(4);

        self.module
            .define_data(data_id, &desc)
            .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;

        self.element_offset_data.insert(offsets.to_vec(), data_id);
        Ok(())
    }

    /// Register a string as a data section (if not already registered).
    pub fn register_string(&mut self, s: &str) -> CodegenResult<()> {
        if self.string_data.contains_key(s) {
            return Ok(());
        }
        let name = format!(".str.srcfile.{}", self.string_data.len());
        let data_id = self.module
            .declare_data(&name, Linkage::Local, false, false)
            .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;

        let mut bytes = s.as_bytes().to_vec();
        bytes.push(0);
        let mut desc = DataDescription::new();
        desc.define(bytes.into_boxed_slice());

        self.module
            .define_data(data_id, &desc)
            .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;

        self.string_data.insert(s.to_string(), data_id);
        self.register_string_header(s)?;
        Ok(())
    }

    /// Register comptime-evaluated global constants as data sections.
    pub fn register_comptime_globals(
        &mut self,
        globals: &HashMap<String, rask_mir::ComptimeGlobalMeta>,
    ) -> CodegenResult<()> {
        for (name, meta) in globals {
            let data_name = format!(".comptime.{}", name);
            let data_id = self.module
                .declare_data(&data_name, Linkage::Local, false, false)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;

            let mut desc = DataDescription::new();
            desc.define(meta.bytes.clone().into_boxed_slice());

            self.module
                .define_data(data_id, &desc)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;

            self.comptime_data.insert(name.clone(), data_id);
        }
        Ok(())
    }

    /// Register a writable slot for each module-level const, found by scanning
    /// MIR for the slot names lowering emitted. Each holds 8 bytes: the value
    /// for scalars, a heap pointer for aggregates. An init thunk fills it once
    /// before main; every reference loads from it (#470).
    pub fn register_const_slots(&mut self, mir_functions: &[MirFunction]) -> CodegenResult<()> {
        let mut names: Vec<&str> = mir_functions
            .iter()
            .flat_map(|f| f.blocks.iter())
            .flat_map(|b| b.statements.iter())
            .filter_map(|s| match &s.kind {
                rask_mir::MirStmtKind::GlobalRef { name, .. }
                    if name.starts_with(rask_mir::lower::CONST_SLOT_PREFIX) =>
                {
                    Some(name.as_str())
                }
                _ => None,
            })
            .collect();
        names.sort_unstable();
        names.dedup();

        for name in names {
            if self.comptime_data.contains_key(name) {
                continue;
            }
            let data_id = self
                .module
                .declare_data(name, Linkage::Local, true, false)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            let mut desc = DataDescription::new();
            desc.define_zeroinit(8);
            self.module
                .define_data(data_id, &desc)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
            self.comptime_data.insert(name.to_string(), data_id);
        }
        Ok(())
    }

    /// Register vtable data sections for trait objects.
    ///
    /// Each vtable is a static data section: [size, align, drop_fn, method_0, method_1, ...]
    /// Function pointers are emitted as relocations resolved by the linker.
    pub fn register_vtables(
        &mut self,
        vtables: &[crate::vtable::VTableInfo],
    ) -> CodegenResult<()> {
        for vt in vtables {
            let data_id = self.module
                .declare_data(&vt.data_name, Linkage::Local, false, false)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;

            let total_size = vt.byte_size() as usize;
            let mut bytes = vec![0u8; total_size];

            // Write size at offset 0
            bytes[0..8].copy_from_slice(&(vt.concrete_size as i64).to_le_bytes());
            // Write align at offset 8
            bytes[8..16].copy_from_slice(&(vt.concrete_align as i64).to_le_bytes());
            // Drop fn at offset 16 stays null for a genuinely trivial type —
            // only types with a refcounted (string) field need one (#366).

            let mut desc = DataDescription::new();
            desc.define(bytes.into_boxed_slice());

            if !vt.drop_string_offsets.is_empty() {
                let drop_func_id = self.get_or_create_drop_glue(&vt.concrete_type, &vt.drop_string_offsets)?;
                let func_ref = self.module.declare_func_in_data(drop_func_id, &mut desc);
                desc.write_function_addr(crate::vtable::VTABLE_DROP_OFFSET, func_ref);
            }

            // Write function pointer relocations for each method
            for method in &vt.methods {
                if let Some(&func_id) = self.func_ids.get(&method.func_name) {
                    let func_ref = self.module.declare_func_in_data(func_id, &mut desc);
                    desc.write_function_addr(method.vtable_offset, func_ref);
                } else {
                    return Err(CodegenError::FunctionNotFound(format!(
                        "vtable method {}.{} (expected {})",
                        vt.concrete_type, method.name, method.func_name
                    )));
                }
            }

            self.module
                .define_data(data_id, &desc)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;

            self.vtable_data.insert(vt.data_name.clone(), data_id);
        }
        Ok(())
    }

    /// Build (or reuse) the drop-glue function for a concrete type behind a
    /// trait object: `fn(data_ptr: i64)` that releases each of its string
    /// fields, at the byte offsets `collect_string_field_offsets` found.
    /// This is what `TraitDrop` calls through the vtable's drop slot before
    /// freeing the boxed allocation itself (#366).
    fn get_or_create_drop_glue(
        &mut self,
        concrete_type: &str,
        string_offsets: &[u32],
    ) -> CodegenResult<cranelift_module::FuncId> {
        if let Some(&func_id) = self.drop_glue_fns.get(concrete_type) {
            return Ok(func_id);
        }

        let free_id = *self.func_ids.get("rask_string_free")
            .ok_or_else(|| CodegenError::FunctionNotFound("rask_string_free".to_string()))?;

        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));

        let name = format!(".dropglue.{}", concrete_type);
        let func_id = self.module
            .declare_function(&name, Linkage::Local, &sig)
            .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;

        self.ctx.clear();
        self.ctx.func.signature = sig;

        let free_ref = self.module.declare_func_in_func(free_id, &mut self.ctx.func);

        let mut fn_builder_ctx = cranelift::prelude::FunctionBuilderContext::new();
        let mut fb = cranelift::prelude::FunctionBuilder::new(&mut self.ctx.func, &mut fn_builder_ctx);

        let entry = fb.create_block();
        fb.append_block_params_for_function_params(entry);
        fb.switch_to_block(entry);
        fb.seal_block(entry);

        let data_ptr = fb.block_params(entry)[0];
        for &offset in string_offsets {
            let field_ptr = if offset == 0 {
                data_ptr
            } else {
                fb.ins().iadd_imm(data_ptr, offset as i64)
            };
            fb.ins().call(free_ref, &[field_ptr]);
        }
        fb.ins().return_(&[]);
        fb.finalize();

        self.module
            .define_function(func_id, &mut self.ctx)
            .map_err(|e| CodegenError::CraneliftError(format!("{:?}", e)))?;

        self.drop_glue_fns.insert(concrete_type.to_string(), func_id);
        Ok(func_id)
    }

    /// Generate code for a single MIR function.
    pub fn gen_function(&mut self, mir_fn: &MirFunction) -> CodegenResult<()> {
        // Skip empty stubs — they were not declared as internal functions,
        // so the stdlib dispatch table handles them at the call site.
        if !self.internal_fns.contains(&mir_fn.name) {
            return Ok(());
        }

        // Pre-register source file string for runtime panic locations
        if let Some(ref src_file) = mir_fn.source_file {
            self.register_string(src_file)?;
        }

        // Every offset list this function's container frees will ask for. Has
        // to happen before the borrow below, and before any body references one.
        for offsets in collect_element_offsets(mir_fn, &self.struct_layouts) {
            self.register_element_offsets(&offsets)?;
        }

        let func_id = self.func_ids.get(&mir_fn.name)
            .ok_or_else(|| CodegenError::FunctionNotFound(mir_fn.name.clone()))?;

        self.ctx.clear();

        // Build the signature (must match declaration)
        let is_main = mir_fn.name == "main";
        let mut sig = self.module.make_signature();
        for param in &mir_fn.params {
            let param_ty = mir_to_cranelift_type(&param.ty)?;
            sig.params.push(AbiParam::new(param_ty));
        }
        let ret_ty = mir_to_cranelift_type(&mir_fn.ret_ty)?;
        if !matches!(mir_fn.ret_ty, rask_mir::MirType::Void) && !is_main {
            sig.returns.push(AbiParam::new(ret_ty));
        }
        self.ctx.func.signature = sig;

        // Pre-import all declared functions into this function's namespace.
        // This must happen before FunctionBuilder borrows ctx.func.
        // Runtime functions are statically linked — mark colocated for direct calls.
        let mut func_refs = HashMap::new();
        for (name, fid) in &self.func_ids {
            let func_ref = self.module.declare_func_in_func(*fid, &mut self.ctx.func);
            self.ctx.func.stencil.dfg.ext_funcs[func_ref].colocated = true;
            func_refs.insert(name.clone(), func_ref);
        }

        // Import only the string data globals that this function actually uses
        let needed_strings = collect_used_strings(mir_fn);
        let mut element_offset_globals: HashMap<Vec<i32>, GlobalValue> = HashMap::new();
        for (offsets, data_id) in &self.element_offset_data {
            let gv = self.module.declare_data_in_func(*data_id, &mut self.ctx.func);
            element_offset_globals.insert(offsets.clone(), gv);
        }

        let mut string_globals: HashMap<String, GlobalValue> = HashMap::new();
        let mut string_header_globals: HashMap<String, GlobalValue> = HashMap::new();
        for s in &needed_strings {
            if let Some(data_id) = self.string_data.get(s) {
                let gv = self.module.declare_data_in_func(*data_id, &mut self.ctx.func);
                string_globals.insert(s.clone(), gv);
            }
            if let Some(data_id) = self.string_header_data.get(s) {
                let gv = self.module.declare_data_in_func(*data_id, &mut self.ctx.func);
                string_header_globals.insert(s.clone(), gv);
            }
        }
        // The checked-arithmetic panic messages are emitted by codegen, not
        // referenced in MIR, so import them into every function that might
        // overflow (type.overflow). Unused imports are harmless.
        for &msg in crate::builder::OVERFLOW_MESSAGES
            .iter()
            .chain(crate::builder::OVERFLOW_OP_SYMBOLS)
        {
            if !string_globals.contains_key(msg) {
                if let Some(data_id) = self.string_data.get(msg) {
                    let gv = self.module.declare_data_in_func(*data_id, &mut self.ctx.func);
                    string_globals.insert(msg.to_string(), gv);
                }
            }
        }
        // For main: import source file name global for rask_set_origin_file (ER15)
        if mir_fn.name == "main" {
            if let Some(file_name) = &self.source_file_name {
                if let Some(data_id) = self.string_data.get(file_name) {
                    if !string_globals.contains_key(file_name) {
                        let gv = self.module.declare_data_in_func(*data_id, &mut self.ctx.func);
                        string_globals.insert(file_name.clone(), gv);
                    }
                }
            }
        }

        // Import only the comptime data globals that this function actually uses
        let mut comptime_globals: HashMap<String, GlobalValue> = HashMap::new();
        for (name, data_id) in &self.comptime_data {
            // Comptime globals are rare; import all for simplicity
            let gv = self.module.declare_data_in_func(*data_id, &mut self.ctx.func);
            comptime_globals.insert(name.clone(), gv);
        }

        // Import only the vtable data globals that this function actually uses
        let needed_vtables = collect_used_vtables(mir_fn);
        let mut vtable_globals: HashMap<String, GlobalValue> = HashMap::new();
        for name in &needed_vtables {
            if let Some(data_id) = self.vtable_data.get(name) {
                let gv = self.module.declare_data_in_func(*data_id, &mut self.ctx.func);
                vtable_globals.insert(name.clone(), gv);
            }
        }

        // Build the function
        let mut builder = FunctionBuilder::new(
            &mut self.ctx.func,
            mir_fn,
            &func_refs,
            &self.struct_layouts,
            &self.enum_layouts,
            &string_globals,
            &string_header_globals,
            &element_offset_globals,
            &comptime_globals,
            &vtable_globals,
            &self.panicking_fns,
            &self.internal_fns,
            &self.fn_param_types,
            self.build_mode,
        )?;
        if let Some(lm) = &self.line_map {
            builder.set_line_map(lm);
        }
        builder.build()?;

        // Temporary: dump CLIF IR for debugging
        if std::env::var("RASK_DUMP_CLIF").is_ok() {
            eprintln!("=== CLIF IR for {} ===\n{}", mir_fn.name, self.ctx.func.display());
        }

        // Define the function in the module
        self.module
            .define_function(*func_id, &mut self.ctx)
            .map_err(|e| CodegenError::CraneliftError(format!("{:?}", e)))?;

        // Collect debug info (srclocs, variables, inline regions)
        if self.build_mode == BuildMode::Debug {
            if let Some(compiled) = self.ctx.compiled_code() {
                let inline_regions = self.inline_regions.get(&mir_fn.name)
                    .map(|v| v.as_slice()).unwrap_or(&[]);
                if let Some(info) = crate::debug_info::collect_function_debug(
                    compiled, *func_id, mir_fn, inline_regions,
                    &self.struct_layouts, &self.enum_layouts, self.line_map.as_ref(),
                ) {
                    self.debug_srclocs.push(info);
                }
            }
        }

        Ok(())
    }

}

/// A MIR function is an empty stub if it has a single block with no
/// statements and a bare return. These come from empty-body `.rk` stubs
/// that exist only so the type checker sees a signature — the real
/// implementation lives in the C runtime dispatch table.
fn is_empty_stub(mir_fn: &MirFunction) -> bool {
    if mir_fn.blocks.len() != 1 {
        return false;
    }
    let block = &mir_fn.blocks[0];
    block.statements.is_empty()
        && matches!(&block.terminator.kind, rask_mir::MirTerminatorKind::Return { .. })
}

/// Collect all string constants referenced by a single MIR function.
fn collect_used_strings(mir_fn: &MirFunction) -> HashSet<String> {
    let mut strings = HashSet::new();
    for block in &mir_fn.blocks {
        for stmt in &block.statements {
            collect_operand_strings_stmt(stmt, &mut strings);
        }
        collect_operand_strings_term(&block.terminator, &mut strings);
    }
    // Source file is used for panic locations
    if let Some(ref src) = mir_fn.source_file {
        strings.insert(src.clone());
    }
    strings
}

fn collect_operand_strings_stmt(stmt: &rask_mir::MirStmt, out: &mut HashSet<String>) {
    match &stmt.kind {
        rask_mir::MirStmtKind::Assign { rvalue, .. } => collect_rvalue_strings(rvalue, out),
        rask_mir::MirStmtKind::Store { value, .. } => collect_operand_string(value, out),
        rask_mir::MirStmtKind::Call { args, .. }
        | rask_mir::MirStmtKind::ClosureCall { args, .. } => {
            for arg in args { collect_operand_string(arg, out); }
        }
        rask_mir::MirStmtKind::PoolCheckedAccess { .. } => {
            // Pool access may need panic strings
            out.insert("pool access with invalid handle".to_string());
        }
        _ => {}
    }
}

fn collect_operand_strings_term(term: &rask_mir::MirTerminator, out: &mut HashSet<String>) {
    match &term.kind {
        rask_mir::MirTerminatorKind::Return { value: Some(op) } => collect_operand_string(op, out),
        rask_mir::MirTerminatorKind::Switch { value, .. } => collect_operand_string(value, out),
        _ => {}
    }
}

fn collect_rvalue_strings(rvalue: &rask_mir::MirRValue, out: &mut HashSet<String>) {
    match rvalue {
        rask_mir::MirRValue::Use(op) => collect_operand_string(op, out),
        rask_mir::MirRValue::BinaryOp { left, right, .. } => {
            collect_operand_string(left, out);
            collect_operand_string(right, out);
        }
        rask_mir::MirRValue::UnaryOp { operand, .. } => collect_operand_string(operand, out),
        rask_mir::MirRValue::Cast { value, .. } | rask_mir::MirRValue::Convert { value, .. } => collect_operand_string(value, out),
        rask_mir::MirRValue::Field { base, .. } => collect_operand_string(base, out),
        rask_mir::MirRValue::EnumTag { value } => collect_operand_string(value, out),
        rask_mir::MirRValue::Deref(op) => collect_operand_string(op, out),
        rask_mir::MirRValue::Ref(_) => {}
        rask_mir::MirRValue::ArrayIndex { base, index, .. } => {
            collect_operand_string(base, out);
            collect_operand_string(index, out);
        }
    }
}

fn collect_operand_string(op: &MirOperand, out: &mut HashSet<String>) {
    if let MirOperand::Constant(MirConst::String(s)) = op {
        out.insert(s.clone());
    }
}

/// Collect vtable names used by a single MIR function.
fn collect_used_vtables(mir_fn: &MirFunction) -> HashSet<String> {
    let mut vtables = HashSet::new();
    for block in &mir_fn.blocks {
        for stmt in &block.statements {
            if let rask_mir::MirStmtKind::TraitBox { vtable_name, .. } = &stmt.kind {
                vtables.insert(vtable_name.clone());
            }
        }
    }
    vtables
}

impl CodeGenerator {
    /// Generate a benchmark runner entry point (`main`) that calls `rask_bench_run`
    /// for each benchmark function.
    ///
    /// `benchmarks` is a list of (display_name, function_name) pairs.
    /// Each function_name must already be declared via `declare_functions`.
    /// Run each module-level constant's init thunk, in declaration order.
    ///
    /// A normal program does this at the top of `main`. The test and benchmark
    /// runners replace `main`, so without this a `const store = Mutex.new(…)`
    /// stayed a zero slot and the first `with store as s` dereferenced null.
    /// Names without a thunk (plain literals, folded at compile time) are
    /// skipped.
    fn emit_const_inits(
        fn_builder: &mut cranelift::prelude::FunctionBuilder,
        func_refs: &HashMap<String, cranelift::codegen::ir::FuncRef>,
        const_names: &[String],
    ) {
        for name in const_names {
            if let Some(func_ref) = func_refs.get(&rask_mir::lower::const_init_fn_name(name)) {
                fn_builder.ins().call(*func_ref, &[]);
            }
        }
    }

    pub fn gen_benchmark_runner(
        &mut self,
        benchmarks: &[(String, String)],
        const_names: &[String],
    ) -> CodegenResult<()> {
        use cranelift::prelude::*;

        // Register benchmark name strings as data
        for (name, _) in benchmarks {
            self.register_string(name)?;
        }

        // Declare rask_main (the entry point)
        let sig = self.module.make_signature(); // void -> void
        let main_id = self.module
            .declare_function("rask_main", Linkage::Export, &sig)
            .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;

        self.ctx.clear();
        self.ctx.func.signature = sig.clone();

        // Import all functions into this function's namespace
        let mut func_refs = HashMap::new();
        for (name, fid) in &self.func_ids {
            let func_ref = self.module.declare_func_in_func(*fid, &mut self.ctx.func);
            self.ctx.func.stencil.dfg.ext_funcs[func_ref].colocated = true;
            func_refs.insert(name.clone(), func_ref);
        }

        // Import string data globals
        let mut string_globals: HashMap<String, GlobalValue> = HashMap::new();
        for (content, data_id) in &self.string_data {
            let gv = self.module.declare_data_in_func(*data_id, &mut self.ctx.func);
            string_globals.insert(content.clone(), gv);
        }

        // Build the runner function body
        let mut fn_builder_ctx = cranelift::prelude::FunctionBuilderContext::new();
        let mut fn_builder = cranelift::prelude::FunctionBuilder::new(
            &mut self.ctx.func, &mut fn_builder_ctx,
        );

        let entry_block = fn_builder.create_block();
        fn_builder.switch_to_block(entry_block);
        fn_builder.seal_block(entry_block);

        Self::emit_const_inits(&mut fn_builder, &func_refs, const_names);

        let bench_run_ref = func_refs.get("rask_bench_run")
            .ok_or_else(|| CodegenError::FunctionNotFound("rask_bench_run".to_string()))?;

        for (name, fn_name) in benchmarks {
            // Get function address
            let bench_fn_ref = func_refs.get(fn_name)
                .ok_or_else(|| CodegenError::FunctionNotFound(fn_name.clone()))?;
            let fn_addr = fn_builder.ins().func_addr(types::I64, *bench_fn_ref);

            // Get name string pointer (raw char*)
            let name_gv = string_globals.get(name)
                .ok_or_else(|| CodegenError::FunctionNotFound(
                    format!("string global for benchmark name '{}'", name)
                ))?;
            let name_ptr = fn_builder.ins().global_value(types::I64, *name_gv);

            // Call rask_bench_run(fn_addr, name_ptr)
            fn_builder.ins().call(*bench_run_ref, &[fn_addr, name_ptr]);
        }

        fn_builder.ins().return_(&[]);
        fn_builder.finalize();

        self.module
            .define_function(main_id, &mut self.ctx)
            .map_err(|e| CodegenError::CraneliftError(format!("{:?}", e)))?;

        Ok(())
    }

    /// Generate a test runner entry point (`rask_main`).
    ///
    /// For each test, calls `rask_test_run(fn_addr, name_ptr)` which returns
    /// 0 on pass, 1 on fail. Accumulates failures and calls `rask_exit(1)` if
    /// any test failed.
    pub fn gen_test_runner(
        &mut self,
        tests: &[(String, String)],
        const_names: &[String],
    ) -> CodegenResult<()> {
        use cranelift::prelude::*;

        // Register test name strings as data
        for (name, _) in tests {
            self.register_string(name)?;
        }

        // Declare rask_main (the entry point)
        let sig = self.module.make_signature(); // void -> void
        let main_id = self.module
            .declare_function("rask_main", Linkage::Export, &sig)
            .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;

        self.ctx.clear();
        self.ctx.func.signature = sig.clone();

        // Import all functions into this function's namespace
        let mut func_refs = HashMap::new();
        for (name, fid) in &self.func_ids {
            let func_ref = self.module.declare_func_in_func(*fid, &mut self.ctx.func);
            self.ctx.func.stencil.dfg.ext_funcs[func_ref].colocated = true;
            func_refs.insert(name.clone(), func_ref);
        }

        // Import string data globals
        let mut string_globals: HashMap<String, GlobalValue> = HashMap::new();
        for (content, data_id) in &self.string_data {
            let gv = self.module.declare_data_in_func(*data_id, &mut self.ctx.func);
            string_globals.insert(content.clone(), gv);
        }

        // Build the runner function body
        let mut fn_builder_ctx = cranelift::prelude::FunctionBuilderContext::new();
        let mut fn_builder = cranelift::prelude::FunctionBuilder::new(
            &mut self.ctx.func, &mut fn_builder_ctx,
        );

        let entry_block = fn_builder.create_block();
        fn_builder.switch_to_block(entry_block);
        fn_builder.seal_block(entry_block);

        Self::emit_const_inits(&mut fn_builder, &func_refs, const_names);

        let test_run_ref = func_refs.get("rask_test_run")
            .ok_or_else(|| CodegenError::FunctionNotFound("rask_test_run".to_string()))?;
        let exit_ref = func_refs.get("rask_exit")
            .ok_or_else(|| CodegenError::FunctionNotFound("rask_exit".to_string()))?;

        // Track total failures
        let mut failures = fn_builder.ins().iconst(types::I64, 0);

        for (name, fn_name) in tests {
            let test_fn_ref = func_refs.get(fn_name)
                .ok_or_else(|| CodegenError::FunctionNotFound(fn_name.clone()))?;
            let fn_addr = fn_builder.ins().func_addr(types::I64, *test_fn_ref);

            let name_gv = string_globals.get(name)
                .ok_or_else(|| CodegenError::FunctionNotFound(
                    format!("string global for test name '{}'", name)
                ))?;
            let name_ptr = fn_builder.ins().global_value(types::I64, *name_gv);

            // rask_test_run returns i32: 0=pass, 1=fail
            let call = fn_builder.ins().call(*test_run_ref, &[fn_addr, name_ptr]);
            let result = fn_builder.inst_results(call)[0];
            let result_i64 = fn_builder.ins().sextend(types::I64, result);
            failures = fn_builder.ins().iadd(failures, result_i64);
        }

        // If failures > 0, exit(1)
        let zero = fn_builder.ins().iconst(types::I64, 0);
        let has_failures = fn_builder.ins().icmp(IntCC::NotEqual, failures, zero);

        let fail_block = fn_builder.create_block();
        let ok_block = fn_builder.create_block();

        fn_builder.ins().brif(has_failures, fail_block, &[], ok_block, &[]);

        fn_builder.switch_to_block(fail_block);
        fn_builder.seal_block(fail_block);
        let one = fn_builder.ins().iconst(types::I64, 1);
        fn_builder.ins().call(*exit_ref, &[one]);
        fn_builder.ins().return_(&[]);

        fn_builder.switch_to_block(ok_block);
        fn_builder.seal_block(ok_block);
        fn_builder.ins().return_(&[]);

        fn_builder.finalize();

        self.module
            .define_function(main_id, &mut self.ctx)
            .map_err(|e| CodegenError::CraneliftError(format!("{:?}", e)))?;

        Ok(())
    }

    /// Emit the final object file. Consumes self because finish() takes ownership.
    pub fn emit_object(self, path: &str) -> CodegenResult<()> {
        let mut product = self.module.finish();

        // Emit DWARF debug info in debug builds
        if self.build_mode == BuildMode::Debug {
            if let (Some(line_map), Some(source_file)) = (&self.line_map, &self.source_file_name) {
                let resolved = crate::debug_info::resolve_debug_info(
                    &self.debug_srclocs, &product,
                );
                crate::debug_info::emit_dwarf(
                    &mut product.object, &resolved, line_map, source_file,
                )?;
            }
        }

        let bytes = product.emit()
            .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;

        std::fs::write(path, bytes)
            .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;

        Ok(())
    }
}

impl crate::Backend for CodeGenerator {
    fn declare_runtime_functions(&mut self) -> CodegenResult<()> {
        self.declare_runtime_functions()
    }

    fn declare_stdlib_functions(&mut self) -> CodegenResult<()> {
        self.declare_stdlib_functions()
    }

    fn declare_extern_functions(&mut self, extern_decls: &[crate::ExternFuncSig]) -> CodegenResult<()> {
        self.declare_extern_functions(extern_decls)
    }

    fn declare_functions(&mut self, mono: &MonoProgram, mir_functions: &[MirFunction]) -> CodegenResult<()> {
        self.declare_functions(mono, mir_functions)
    }

    fn register_strings(&mut self, mir_functions: &[MirFunction]) -> CodegenResult<()> {
        self.register_strings(mir_functions)
    }

    fn register_comptime_globals(
        &mut self,
        globals: &std::collections::HashMap<String, rask_mir::ComptimeGlobalMeta>,
    ) -> CodegenResult<()> {
        self.register_comptime_globals(globals)
    }

    fn register_const_slots(&mut self, mir_functions: &[MirFunction]) -> CodegenResult<()> {
        self.register_const_slots(mir_functions)
    }

    fn register_vtables(&mut self, vtables: &[crate::vtable::VTableInfo]) -> CodegenResult<()> {
        self.register_vtables(vtables)
    }

    fn gen_function(&mut self, mir_fn: &MirFunction) -> CodegenResult<()> {
        self.gen_function(mir_fn)
    }

    fn gen_benchmark_runner(
        &mut self,
        benchmarks: &[(String, String)],
        const_names: &[String],
    ) -> CodegenResult<()> {
        self.gen_benchmark_runner(benchmarks, const_names)
    }

    fn gen_test_runner(
        &mut self,
        tests: &[(String, String)],
        const_names: &[String],
    ) -> CodegenResult<()> {
        self.gen_test_runner(tests, const_names)
    }

    fn emit_object(self: Box<Self>, path: &str) -> CodegenResult<()> {
        // Unbox to call the owned-self method
        (*self).emit_object(path)
    }
}

/// The offset lists this function's container frees will ask for.
///
/// Mirrors the tag encoding `container_drop.rs` writes and the flattening
/// `FunctionBuilder::element_string_offsets` does — kept here because the data
/// objects have to exist before any function body references one.
fn collect_element_offsets(
    mir_fn: &MirFunction,
    struct_layouts: &[rask_mono::StructLayout],
) -> Vec<Vec<i32>> {
    let mut lists = Vec::new();
    for block in &mir_fn.blocks {
        for stmt in &block.statements {
            let rask_mir::MirStmtKind::Call { func, args, .. } = &stmt.kind else { continue };
            let Some((leading, tags)) = rask_mir::elem_strs::ctor_shape(&func.name) else {
                continue;
            };
            for i in 0..tags {
                let Some(rask_mir::MirOperand::Constant(rask_mir::MirConst::Int(tag))) =
                    args.get(leading + i)
                else {
                    continue;
                };
                if let Some(offs) =
                    crate::elem_offsets::string_offsets_for_tag(*tag, struct_layouts)
                {
                    lists.push(offs);
                }
            }
        }
    }
    lists
}
