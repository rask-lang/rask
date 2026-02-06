//! Tree-walk interpreter for the Rask language.
//!
//! Executes the AST directly without compilation.

mod value;
mod env;
mod resource;
mod interp;

pub use interp::{Interpreter, RuntimeError};
