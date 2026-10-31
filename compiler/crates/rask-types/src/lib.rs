// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! The type data every stage after the checker reads: `Type`, and what a
//! type declaration says once its fields and signatures are `Type`s.
//!
//! The checker (`rask-check`) produces these; layout, monomorphization, MIR,
//! codegen and the interpreter read them. Nothing here checks anything.

mod types;
mod defs;
pub mod abi;

pub use types::{FnParam, GenericArg, Type, TypeId, TypeVarId};
pub use defs::{
    conformance_symbol, ErrorWrap, InterfaceAssocType, InterfaceTypeParam, MemberDecl, MethodOrigin,
    MethodSig, SelfParam, TypeBinding, TypeDef, VariantDecl,
};
