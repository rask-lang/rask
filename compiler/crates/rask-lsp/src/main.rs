// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Rask Language Server.
//!
//! Feature surface:
//!   - Diagnostics: lex, parse, desugar, resolve, typecheck, ownership, effects
//!   - Hover, goto-definition, completions (dot + identifier)
//!   - Code actions (quickfixes from diagnostic suggestions)
//!   - Inlay hints, semantic tokens
//!   - Signature help
//!   - Document symbols, workspace symbols
//!   - Formatting
//!   - Find references, rename
//!
//! The server is panic-resilient: handlers run under `catch_unwind`, and a
//! global panic hook records the error so the process stays up.

mod backend;
mod completion;
mod convert;
mod format;
mod goto;
mod hover;
mod incremental;
mod inlay_hints;
mod position_index;
mod references;
mod semantic_tokens;
mod server;
mod signature_help;
mod symbols;
mod type_format;
mod util;

use std::sync::Arc;

use tower_lsp::{LspService, Server};

use crate::backend::Backend;
use crate::server::BackendHandle;

#[tokio::main]
async fn main() {
    install_panic_hook();

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| {
        let inner = Arc::new(Backend::new(client));
        BackendHandle::new(inner)
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}

/// Keep stderr as a last-resort log sink — tower_lsp already catches panics
/// per request, but a misbehaving panic hook in a dependency could still tear
/// down the process. Writing the location to stderr gives the LSP client (VS
/// Code) something to show in the Output panel instead of silent death.
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        eprintln!("rask-lsp panic: {}", info);
        default(info);
    }));
}
