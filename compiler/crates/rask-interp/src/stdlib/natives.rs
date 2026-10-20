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
            #[cfg(not(target_arch = "wasm32"))]
            "rask_io_read_line" => Some(self.io_read_line()),
            // stdlib/terminal.rk: the same two questions runtime/args.c answers.
            "rask_terminal_is_tty" => Some(Ok(Value::Bool(stdout_is_tty()))),
            "rask_terminal_size" => Some(Ok(Value::Int(stdout_size(), crate::value::IntKind::I64))),
            #[cfg(not(target_arch = "wasm32"))]
            _ => self.call_net_native(symbol, _args),
            #[cfg(target_arch = "wasm32")]
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

fn stdout_is_tty() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}

/// Columns << 16 | rows, or -1 when stdout isn't a terminal or reports 0x0.
#[cfg(unix)]
fn stdout_size() -> i64 {
    let mut ws = libc::winsize { ws_row: 0, ws_col: 0, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: TIOCGWINSZ writes one `winsize` through the pointer and nothing else.
    if unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) } != 0 {
        return -1;
    }
    if ws.ws_col == 0 || ws.ws_row == 0 {
        return -1;
    }
    ((ws.ws_col as i64) << 16) | ws.ws_row as i64
}

#[cfg(not(unix))]
fn stdout_size() -> i64 {
    -1
}
