// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Rask code generator — MIR → native code via Cranelift.

mod types;
mod builder;
pub mod closures;
mod debug_info;
pub mod dispatch;
mod module;
mod tests;
pub mod vtable;

pub use module::CodeGenerator;

use std::collections::HashMap;
use std::error::Error;
use std::fmt;

use rask_mir::MirFunction;
use rask_mono::MonoProgram;

/// Controls safety checks and optimization level.
/// Debug: all runtime checks (null, pool_id, occupied, bounds, generation).
/// Release: only bounds + generation checks, inlined in codegen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildMode {
    Debug,
    Release,
}

/// Extern function signature for codegen declaration.
/// Decoupled from AST — callers convert from their own representation.
pub struct ExternFuncSig {
    pub name: String,
    pub param_types: Vec<String>,
    pub ret_ty: Option<String>,
}

#[derive(Debug, Clone)]
pub enum CodegenError {
    UnsupportedFeature(String),
    TypeConversionFailed(String),
    FunctionNotFound(String),
    CraneliftError(String),
}

impl fmt::Display for CodegenError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            CodegenError::UnsupportedFeature(msg) => write!(f, "Unsupported feature: {}", msg),
            CodegenError::TypeConversionFailed(msg) => write!(f, "Type conversion failed: {}", msg),
            CodegenError::FunctionNotFound(name) => write!(f, "Function not found: {}", name),
            CodegenError::CraneliftError(msg) => write!(f, "Cranelift error: {}", msg),
        }
    }
}

impl Error for CodegenError {}

pub type CodegenResult<T> = Result<T, CodegenError>;

/// Backend abstraction — implemented by Cranelift today, LLVM in the future.
///
/// Covers the full lifecycle: declare functions/data → generate IR → emit object.
/// Constructors are backend-specific (different config), so not part of the trait.
pub trait Backend {
    /// Declare C runtime functions (rask_alloc, rask_free, print helpers, etc.).
    fn declare_runtime_functions(&mut self) -> CodegenResult<()>;

    /// Declare stdlib dispatch functions (Vec_push, Map_get, etc.).
    fn declare_stdlib_functions(&mut self) -> CodegenResult<()>;

    /// Declare extern "C" functions from user code.
    fn declare_extern_functions(&mut self, extern_decls: &[ExternFuncSig]) -> CodegenResult<()>;

    /// Declare all MIR functions (signatures only — no codegen yet).
    fn declare_functions(&mut self, mono: &MonoProgram, mir_functions: &[MirFunction]) -> CodegenResult<()>;

    /// Register string literals for data sections.
    fn register_strings(&mut self, mir_functions: &[MirFunction]) -> CodegenResult<()>;

    /// Register compile-time evaluated globals.
    fn register_comptime_globals(
        &mut self,
        globals: &HashMap<String, rask_mir::ComptimeGlobalMeta>,
    ) -> CodegenResult<()>;

    /// Register vtable data for trait objects.
    fn register_vtables(&mut self, vtables: &[vtable::VTableInfo]) -> CodegenResult<()>;

    /// Generate native code for one MIR function.
    fn gen_function(&mut self, mir_fn: &MirFunction) -> CodegenResult<()>;

    /// Generate benchmark runner entry point.
    fn gen_benchmark_runner(&mut self, benchmarks: &[(String, String)]) -> CodegenResult<()>;

    /// Generate test runner entry point.
    fn gen_test_runner(&mut self, tests: &[(String, String)]) -> CodegenResult<()>;

    /// Emit the compiled code as an object file.
    fn emit_object(self: Box<Self>, path: &str) -> CodegenResult<()>;
}
