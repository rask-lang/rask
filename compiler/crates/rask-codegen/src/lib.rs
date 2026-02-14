// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Rask code generator — MIR → native code via Cranelift.

mod types;
mod builder;
mod module;
mod tests;

pub use module::CodeGenerator;

use std::error::Error;
use std::fmt;

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
