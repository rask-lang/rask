// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Stdlib `@native` declarations answered by symbol.
//!
//! Most of the stdlib the interpreter runs natively is dispatched by module or
//! type (`fs`, `net`, `File`, …). A bodiless `@native("sym")` declaration on
//! anything else had nowhere to land: the call reached a function with no
//! body and failed as an undefined name. Native codegen maps the same symbol
//! through its dispatch table; this is the interpreter's half of that map.

use crate::interp::{Interpreter, RuntimeError};
use crate::value::Value;

impl Interpreter {
    /// The interpreter's implementation of `symbol`, or `None` if it has none.
    pub(crate) fn call_native_symbol(
        &mut self,
        symbol: &str,
        _args: &[Value],
    ) -> Option<Result<Value, RuntimeError>> {
        match symbol {
            // stdlib/sim.rk: is this run under sim? The interpreter has no sim
            // mode, so `sim.require` skips the test as sim-only (sim/F3).
            "rask_sim_enable_faults" => Some(Ok(Value::Bool(false))),
            _ => None,
        }
    }

    /// The `@native` symbol a stdlib type or namespace declares for `method`.
    pub(crate) fn stdlib_native_symbol(type_name: &str, method: &str) -> Option<String> {
        rask_stdlib::StubRegistry::load()
            .lookup_method(type_name, method)
            .and_then(|m| m.native.clone())
            .filter(|s| !s.is_empty())
    }
}
