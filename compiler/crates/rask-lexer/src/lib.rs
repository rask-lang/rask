// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Lexer for the Rask language.
//!
//! Tokenizes source code into a stream of tokens for the parser.

mod lexer;

pub use lexer::{LexError, LexResult, Lexer};
