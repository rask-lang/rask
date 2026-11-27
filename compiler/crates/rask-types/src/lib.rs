// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! The type data every stage after the checker reads: `Type`, and what a
//! type declaration says once its fields and signatures are `Type`s.
//!
//! The checker (`rask-check`) produces these; layout, monomorphization, MIR,
//! codegen and the interpreter read them. Nothing here checks anything.

mod types;
mod defs;
mod lang;
mod handle;
pub mod abi;

pub use lang::LangItem;
pub use handle::RuntimeHandle;

pub use types::{ArrayLen, FnParam, GenericArg, Type, TypeId, TypeVarId};

/// ctrl.comptime/CT68: an instantiation chain longer than this is an error.
/// Polymorphic recursion (`nest<T>` calling `nest<Vec<T>>`, a struct holding a
/// `Vec` of itself at `(T, T)`) makes one that never ends; anything legitimate
/// is far shorter. Every walk that follows a generic's instantiations stops
/// here, the monomorphizer's among them.
pub const INSTANTIATION_LIMIT: usize = 64;

/// The other half of the same limit. A step that puts a parameter twice into
/// its argument (`Grow<T>` holding `Grow<(T, T)>`) doubles the type each level,
/// so the depth limit is never reached before memory is: 2^40 nodes at level
/// 40. A type with more nodes than this is past any chain that ends.
pub const INSTANTIATION_SIZE_LIMIT: usize = 4096;
pub use defs::{
    conformance_symbol, ErrorWrap, FnSignature, InterfaceAssocType, InterfaceTypeParam, MemberDecl, MethodOrigin,
    MethodSig, PatternTarget, SelfParam, TypeBinding, TypeDef, VariantDecl,
};
