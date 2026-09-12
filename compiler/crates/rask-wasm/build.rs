// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Reserves the browser playground's stack, and tells the interpreter how much
//! of it to use.
//!
//! The interpreter measures how much stack it has spent and refuses to recurse
//! further when it runs out (`rask_interp::stack_nearly_exhausted`). Off wasm it
//! spawns the thread itself, so it knows the size; on wasm the size is a link
//! argument, so the number has to travel from the linker to the interpreter.
//! Both come from `RESERVED_BYTES` below — that is the one place to change it.
//!
//! wasm-ld's default is 1 MiB, which buys about 30 interpreted Rask frames.

/// Linear memory set aside for the stack, reserved at load.
const RESERVED_BYTES: usize = 32 * 1024 * 1024;

/// What the interpreter is allowed to spend of it.
///
/// Deliberately short of the reservation, because on wasm the interpreter's
/// guard is not the first limit a deep recursion meets: the host counts wasm
/// frames against its own native stack and kills the call at around 1300 of
/// ours, throwing a `RangeError` that no Rust code gets to see. Stopping at
/// half the reservation (~640 frames, measured in Chromium) keeps Rask's own
/// "recursion too deep" diagnostic the thing that fires. Overshooting is not a
/// slightly worse error message: a trap skips every destructor, so
/// wasm-bindgen's borrow of `Playground` is never returned and the instance is
/// dead for good (#1172).
const BUDGET_BYTES: usize = RESERVED_BYTES / 2;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let is_wasm = std::env::var("CARGO_CFG_TARGET_FAMILY")
        .map(|f| f.split(',').any(|x| x == "wasm"))
        .unwrap_or(false);

    if is_wasm {
        println!("cargo:rustc-link-arg=-zstack-size={RESERVED_BYTES}");
        println!("cargo:rustc-env=RASK_WASM_STACK_BYTES={BUDGET_BYTES}");
    }
}
