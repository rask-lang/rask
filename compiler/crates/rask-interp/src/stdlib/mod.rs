//! Standard library module dispatch.
//!
//! Routes `module.method()` calls to the appropriate stdlib module handler.

mod cli;
mod env;
mod fs;
mod io;
mod json;
mod math;
mod os;
mod path;
mod random;
mod time;

use crate::interp::{Interpreter, RuntimeError};
use crate::value::{ModuleKind, Value};

impl Interpreter {
    /// Dispatch a module method call to the appropriate stdlib handler.
    pub(crate) fn call_module_method(
        &mut self,
        module: &ModuleKind,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match module {
            ModuleKind::Fs => self.call_fs_method(method, args),
            ModuleKind::Io => self.call_io_method(method, args),
            ModuleKind::Time => self.call_time_module_method(method, args),
            ModuleKind::Random => self.call_random_method(method, args),
            ModuleKind::Math => self.call_math_method(method, args),
            ModuleKind::Os => self.call_os_method(method, args),
            ModuleKind::Json => self.call_json_method(method, args),
            ModuleKind::Path => self.call_path_module_method(method, args),
            // Legacy aliases — forward to new modules
            ModuleKind::Env => self.call_env_method(method, args),
            ModuleKind::Cli => self.call_cli_module_method(method, args),
            ModuleKind::Std => self.call_os_method(method, args),
        }
    }

    /// Handle path module methods (only type access currently).
    fn call_path_module_method(
        &self,
        method: &str,
        _args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        // The path module itself doesn't have methods — Path.new() goes through
        // type method dispatch. But in case someone tries path.something():
        Err(RuntimeError::NoSuchMethod {
            ty: "path".to_string(),
            method: method.to_string(),
        })
    }

    /// Dispatch a type static method (e.g., Instant.now(), Duration.seconds(5), Path.new()).
    pub(crate) fn call_type_method(
        &self,
        type_name: &str,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match type_name {
            "Instant" | "Duration" => self.call_time_type_method(type_name, method, args),
            "Path" => self.call_path_type_method(method, args),
            "Rng" => Err(RuntimeError::TypeError(format!(
                "Rng.{} is not yet implemented", method
            ))),
            _ => Err(RuntimeError::TypeError(format!(
                "type {} has no method '{}'",
                type_name, method
            ))),
        }
    }
}
