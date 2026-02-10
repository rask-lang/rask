// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Rask Language Server
//!
//! Provides diagnostics for all compilation errors:
//! lexer, parser, resolve, type check, and ownership.
//!
//! Also provides IDE features:
//! - Go to Definition
//! - Hover (type information)
//! - Code Actions (quick fixes)
//! - Completions (dot-completion on types, identifier/keyword completion)

mod backend;
mod completion;
mod convert;
mod position_index;
mod server;
mod type_format;

use tower_lsp::{LspService, Server};

use crate::backend::Backend;

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
