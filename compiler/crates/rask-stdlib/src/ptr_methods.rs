// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! The one list of raw pointer methods.
//!
//! Four places kept their own copy: the type checker's eager path, the
//! constraint solver, MIR lowering's `RawPtr_*` names, and codegen's C-symbol
//! table. They disagreed. The solver had no pointer arm at all, so a pointer
//! that arrived from another call — `s.as_ptr().offset(1)` — was reported as
//! "no method `offset` found for type `*u8`" even though the eager path knew
//! `offset` perfectly well (#696). Everything reads this table now.

/// What a pointer method takes and gives back. `Self` is the pointer's own
/// type, `T` its pointee.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtrSig {
    /// `self -> T` — read.
    Read,
    /// `(self, T) -> ()` — write.
    Write,
    /// `(self, i64) -> Self` — add, sub, offset.
    Arith,
    /// `self -> bool` — is_null, is_aligned.
    Predicate,
    /// `(self, i64) -> bool` — is_aligned_to.
    PredicateInt,
    /// `(self, Self) -> bool` — eq, ne.
    Comparison,
    /// `(self, i64) -> i64` — align_offset.
    ToInt,
    /// `self -> *U` — cast. Type-only, no runtime call.
    Cast,
}

/// One pointer method: its Rask name, the C symbol backing it natively, and
/// its call shape.
pub struct PtrMethod {
    pub name: &'static str,
    /// C symbol codegen calls. `None` means there's nothing to run — `cast`
    /// only changes the type.
    pub c_symbol: Option<&'static str>,
    pub sig: PtrSig,
    /// Whether the call has to sit in an `unsafe` block. Reading and writing
    /// through a pointer, and moving one, can all break memory. Asking what
    /// address a pointer holds cannot, so `is_null`, `eq` and `ne` are safe.
    pub needs_unsafe: bool,
    /// Whether codegen appends the pointee's size, so `p.offset(1)` steps one
    /// element rather than one byte.
    pub scales_by_elem: bool,
}

const fn m(
    name: &'static str,
    c_symbol: Option<&'static str>,
    sig: PtrSig,
    needs_unsafe: bool,
    scales_by_elem: bool,
) -> PtrMethod {
    PtrMethod { name, c_symbol, sig, needs_unsafe, scales_by_elem }
}

/// Every method a `*T` answers to.
pub const PTR_METHODS: &[PtrMethod] = &[
    // Dereference.
    m("read", Some("rask_ptr_read"), PtrSig::Read, true, true),
    m("write", Some("rask_ptr_write"), PtrSig::Write, true, true),
    // Arithmetic — steps by whole elements, not bytes.
    m("add", Some("rask_ptr_add"), PtrSig::Arith, true, true),
    m("sub", Some("rask_ptr_sub"), PtrSig::Arith, true, true),
    m("offset", Some("rask_ptr_offset"), PtrSig::Arith, true, true),
    // Address questions — no memory is touched, so no `unsafe` needed.
    m("is_null", Some("rask_ptr_is_null"), PtrSig::Predicate, false, false),
    m("eq", Some("rask_ptr_eq"), PtrSig::Comparison, false, false),
    m("ne", Some("rask_ptr_ne"), PtrSig::Comparison, false, false),
    // Alignment.
    m("is_aligned", Some("rask_ptr_is_aligned"), PtrSig::Predicate, true, false),
    m("is_aligned_to", Some("rask_ptr_is_aligned_to"), PtrSig::PredicateInt, true, false),
    m("align_offset", Some("rask_ptr_align_offset"), PtrSig::ToInt, true, false),
    // Retyping.
    m("cast", None, PtrSig::Cast, true, false),
];

/// Look up a pointer method by name.
pub fn lookup(name: &str) -> Option<&'static PtrMethod> {
    PTR_METHODS.iter().find(|m| m.name == name)
}

/// The MIR call name for a pointer method — `RawPtr_offset`.
pub fn mir_name(name: &str) -> String {
    format!("RawPtr_{}", name)
}

/// Method names, for the drift registry.
pub fn method_names() -> Vec<&'static str> {
    PTR_METHODS.iter().map(|m| m.name).collect()
}
