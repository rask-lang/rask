//! Type system and type checker for the Rask language.
//!
//! Performs type inference and checking on the AST.

mod types;
mod checker;
mod traits;

pub use types::{Type, TypeId, TypeVarId};
pub use checker::{
    typecheck, TypeChecker, TypedProgram, TypeTable, TypeDef,
    TypeError, InferenceContext, TypeConstraint, MethodSig, SelfParam,
    parse_type_string,
};
pub use traits::{
    TraitBound, TraitChecker, TraitError,
    verify_instantiation, implements_trait, implemented_traits,
};
