// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Float functions that aren't exact: the runtime's `math.c`, linked in by
//! `build.rs`, so the interpreter computes the bits native code does
//! (`determinism/D12`). Rust's `f64::sin` is the platform's libm, which differs
//! between machines in the last bit.
//!
//! Exact operations (`sqrt`, `floor`, `abs`, …) stay on Rust's methods: every
//! conforming implementation returns the same bits for them.

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    unsafe extern "C" {
        fn math_sin(x: f64) -> f64;
        fn math_cos(x: f64) -> f64;
        fn math_tan(x: f64) -> f64;
        fn math_asin(x: f64) -> f64;
        fn math_acos(x: f64) -> f64;
        fn math_atan(x: f64) -> f64;
        fn math_atan2(y: f64, x: f64) -> f64;
        fn math_exp(x: f64) -> f64;
        fn math_ln(x: f64) -> f64;
        fn math_log2(x: f64) -> f64;
        fn math_log10(x: f64) -> f64;
        fn math_pow(x: f64, y: f64) -> f64;
        fn math_powi(x: f64, n: i32) -> f64;
        fn math_hypot(x: f64, y: f64) -> f64;
    }

    // SAFETY (all): pure functions of their arguments, no pointers, no state.
    pub fn sin(x: f64) -> f64 { unsafe { math_sin(x) } }
    pub fn cos(x: f64) -> f64 { unsafe { math_cos(x) } }
    pub fn tan(x: f64) -> f64 { unsafe { math_tan(x) } }
    pub fn asin(x: f64) -> f64 { unsafe { math_asin(x) } }
    pub fn acos(x: f64) -> f64 { unsafe { math_acos(x) } }
    pub fn atan(x: f64) -> f64 { unsafe { math_atan(x) } }
    pub fn atan2(y: f64, x: f64) -> f64 { unsafe { math_atan2(y, x) } }
    pub fn exp(x: f64) -> f64 { unsafe { math_exp(x) } }
    pub fn ln(x: f64) -> f64 { unsafe { math_ln(x) } }
    pub fn log2(x: f64) -> f64 { unsafe { math_log2(x) } }
    pub fn log10(x: f64) -> f64 { unsafe { math_log10(x) } }
    pub fn pow(x: f64, y: f64) -> f64 { unsafe { math_pow(x, y) } }
    pub fn powi(x: f64, n: i32) -> f64 { unsafe { math_powi(x, n) } }
    pub fn hypot(x: f64, y: f64) -> f64 { unsafe { math_hypot(x, y) } }
}

/// The playground build has no C toolchain, so it uses Rust's math. That's the
/// one place the interpreter can differ from native in the last bit.
#[cfg(target_arch = "wasm32")]
mod imp {
    pub fn sin(x: f64) -> f64 { x.sin() }
    pub fn cos(x: f64) -> f64 { x.cos() }
    pub fn tan(x: f64) -> f64 { x.tan() }
    pub fn asin(x: f64) -> f64 { x.asin() }
    pub fn acos(x: f64) -> f64 { x.acos() }
    pub fn atan(x: f64) -> f64 { x.atan() }
    pub fn atan2(y: f64, x: f64) -> f64 { y.atan2(x) }
    pub fn exp(x: f64) -> f64 { x.exp() }
    pub fn ln(x: f64) -> f64 { x.ln() }
    pub fn log2(x: f64) -> f64 { x.log2() }
    pub fn log10(x: f64) -> f64 { x.log10() }
    pub fn pow(x: f64, y: f64) -> f64 { x.powf(y) }
    pub fn powi(x: f64, n: i32) -> f64 { x.powf(n as f64) }
    pub fn hypot(x: f64, y: f64) -> f64 { x.hypot(y) }
}

pub(crate) use imp::*;
