// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Semantic hashing — fingerprint desugared AST for incremental compilation.
//!
//! Hashes the desugared AST (after desugar, before type checking) with:
//! - Variable names normalized to positional indices (H4)
//! - Source locations and formatting excluded (H3)
//! - Literal values and types included (H2)
//! - Function signatures included (H1)
//!
//! Builds a Merkle tree of function → callee dependencies (MK1-MK4).
//!
//! See `comp.semantic-hash` spec for the full algorithm.

mod hasher;
mod merkle;

pub use hasher::{hash_function, hash_decl, SemanticHash};
pub use merkle::{MerkleTree, FunctionNode};
