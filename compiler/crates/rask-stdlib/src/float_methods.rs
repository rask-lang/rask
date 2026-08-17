// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! The one list of float methods.
//!
//! Four places used to keep their own copy of this list: the type checker's
//! signature match, the interpreter's dispatch, codegen's C-symbol table, and
//! the drift registry. They disagreed — `x.floor()` type-checked and ran on
//! the interpreter, then died in codegen with "Function not found: f64_floor"
//! (#687). Everything reads this table now, so a new method is one row.

/// What a float method takes and gives back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatSig {
    /// `self -> Self` — floor, sqrt, sin, neg.
    Unary,
    /// `(self, Self) -> Self` — add, powf.
    BinaryFloat,
    /// `(self, i32) -> Self` — powi.
    BinaryInt,
    /// `self -> bool` — is_nan.
    Predicate,
    /// `(self, Self) -> bool` — eq, lt.
    Comparison,
    /// `(self, Self) -> Ordering`.
    Compare,
    /// `self -> string`.
    ToString,
    /// `self -> i64`.
    ToInt,
    /// `self -> u64` — the raw bit pattern.
    ///
    /// HA4 excludes floats from Hashable, so this is how a float becomes a Map
    /// key: the caller decides what "the same key" means instead of inheriting
    /// `NaN != NaN`.
    ///
    /// u64 at both widths. MIR mangles f32 and f64 receivers to the same
    /// `f64_*` calls, so an f32's own 32-bit pattern isn't recoverable there,
    /// and one width keeps the backends from disagreeing about what the key is.
    ToBits,
}

/// One float method: its Rask name, the C symbol backing it natively, and its
/// call shape.
pub struct FloatMethod {
    pub name: &'static str,
    /// C symbol codegen calls. `None` means codegen lowers it to instructions
    /// instead of a call — arithmetic, comparisons, the int cast.
    pub c_symbol: Option<&'static str>,
    pub sig: FloatSig,
}

const fn m(name: &'static str, c_symbol: Option<&'static str>, sig: FloatSig) -> FloatMethod {
    FloatMethod { name, c_symbol, sig }
}

/// Every method `f32`/`f64` answer to.
///
/// Trig, log and classification go through the `math_*` wrappers in
/// `runtime/math.c`; the rounding family and `sqrt`/`fabs`/`pow` are libm
/// symbols called directly.
pub const FLOAT_METHODS: &[FloatMethod] = &[
    // Arithmetic — Cranelift instructions, no call.
    m("add", None, FloatSig::BinaryFloat),
    m("sub", None, FloatSig::BinaryFloat),
    m("mul", None, FloatSig::BinaryFloat),
    m("div", None, FloatSig::BinaryFloat),
    m("rem", None, FloatSig::BinaryFloat),
    m("neg", None, FloatSig::Unary),
    // Comparison — also instructions.
    m("eq", None, FloatSig::Comparison),
    m("ne", None, FloatSig::Comparison),
    m("lt", None, FloatSig::Comparison),
    m("le", None, FloatSig::Comparison),
    m("gt", None, FloatSig::Comparison),
    m("ge", None, FloatSig::Comparison),
    m("compare", None, FloatSig::Compare),
    // Rounding and magnitude.
    m("abs", Some("fabs"), FloatSig::Unary),
    m("floor", Some("floor"), FloatSig::Unary),
    m("ceil", Some("ceil"), FloatSig::Unary),
    m("round", Some("round"), FloatSig::Unary),
    m("trunc", Some("trunc"), FloatSig::Unary),
    m("fract", Some("math_fract"), FloatSig::Unary),
    m("sqrt", Some("sqrt"), FloatSig::Unary),
    // Powers.
    m("pow", Some("pow"), FloatSig::BinaryFloat),
    m("powf", Some("pow"), FloatSig::BinaryFloat),
    m("powi", Some("pow"), FloatSig::BinaryInt),
    // Trigonometry.
    m("sin", Some("math_sin"), FloatSig::Unary),
    m("cos", Some("math_cos"), FloatSig::Unary),
    m("tan", Some("math_tan"), FloatSig::Unary),
    m("asin", Some("math_asin"), FloatSig::Unary),
    m("acos", Some("math_acos"), FloatSig::Unary),
    m("atan", Some("math_atan"), FloatSig::Unary),
    // Exponential and logarithmic.
    m("exp", Some("math_exp"), FloatSig::Unary),
    m("ln", Some("math_ln"), FloatSig::Unary),
    m("log2", Some("math_log2"), FloatSig::Unary),
    m("log10", Some("math_log10"), FloatSig::Unary),
    // Classification.
    m("is_nan", Some("math_is_nan"), FloatSig::Predicate),
    m("is_inf", Some("math_is_inf"), FloatSig::Predicate),
    m("is_finite", Some("math_is_finite"), FloatSig::Predicate),
    // Conversion.
    m("to_string", Some("rask_f64_to_string"), FloatSig::ToString),
    m("to_int", None, FloatSig::ToInt),
    m("to_bits", Some("rask_f64_to_bits"), FloatSig::ToBits),
];

/// Look up a float method by name.
pub fn lookup(name: &str) -> Option<&'static FloatMethod> {
    FLOAT_METHODS.iter().find(|m| m.name == name)
}

/// Method names, for the drift registry.
pub fn method_names() -> Vec<&'static str> {
    FLOAT_METHODS.iter().map(|m| m.name).collect()
}
