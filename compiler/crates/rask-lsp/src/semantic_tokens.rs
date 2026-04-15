// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Semantic tokens — classifier-driven highlighting that knows about
//! resolved types.
//!
//! TextMate grammars can't tell a local variable from a function call; the
//! semantic layer sits on top of the grammar and upgrades colors based on
//! what the name really is.

use tower_lsp::lsp_types::*;

use rask_resolve::SymbolKind;

use crate::backend::CompilationResult;

/// The legend negotiated in `initialize` — order matters: the `token_type`
/// field in each token is an index into this Vec.
pub fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: vec![
            SemanticTokenType::VARIABLE,       // 0
            SemanticTokenType::PARAMETER,      // 1
            SemanticTokenType::FUNCTION,       // 2
            SemanticTokenType::METHOD,         // 3
            SemanticTokenType::STRUCT,         // 4
            SemanticTokenType::ENUM,           // 5
            SemanticTokenType::ENUM_MEMBER,    // 6
            SemanticTokenType::INTERFACE,      // 7 (trait)
            SemanticTokenType::TYPE,           // 8
            SemanticTokenType::NAMESPACE,      // 9
            SemanticTokenType::PROPERTY,       // 10 (field)
        ],
        token_modifiers: vec![
            SemanticTokenModifier::DEFAULT_LIBRARY, // 0 (stdlib)
            SemanticTokenModifier::READONLY,        // 1 (const)
        ],
    }
}

const TYPE_VARIABLE: u32 = 0;
const TYPE_PARAMETER: u32 = 1;
const TYPE_FUNCTION: u32 = 2;
const TYPE_STRUCT: u32 = 4;
const TYPE_ENUM: u32 = 5;
const TYPE_ENUM_MEMBER: u32 = 6;
const TYPE_TRAIT: u32 = 7;
const TYPE_TYPE: u32 = 8;
const TYPE_NAMESPACE: u32 = 9;
const TYPE_PROPERTY: u32 = 10;

const MOD_STDLIB: u32 = 1 << 0;
const MOD_READONLY: u32 = 1 << 1;

pub fn tokens(cached: &CompilationResult) -> SemanticTokensResult {
    let mut raw: Vec<(u32, u32, u32, u32, u32)> = Vec::new();
    // (line, col, length, token_type, modifiers)

    for (span, node_id, _name) in &cached.position_index.idents {
        let Some(&sid) = cached.typed.resolutions.get(node_id) else {
            continue;
        };
        let Some(symbol) = cached.typed.symbols.get(sid) else {
            continue;
        };
        let (tt, mods) = classify(&symbol.kind);

        let start = cached.line_index.offset_to_position(&cached.source, span.start);
        let end = cached.line_index.offset_to_position(&cached.source, span.end);
        if start.line != end.line {
            // Skip multi-line names — unusual and delta-encoded protocol
            // demands single-line tokens.
            continue;
        }
        let length = end.character.saturating_sub(start.character);
        if length == 0 {
            continue;
        }
        raw.push((start.line, start.character, length, tt, mods));
    }

    // Sort by (line, col) — LSP requires ascending delta encoding.
    raw.sort_by_key(|t| (t.0, t.1));
    // Deduplicate exact hits.
    raw.dedup();

    let mut data = Vec::with_capacity(raw.len());
    let mut prev_line = 0u32;
    let mut prev_col = 0u32;
    for (line, col, length, tt, mods) in raw {
        let delta_line = line - prev_line;
        let delta_start = if delta_line == 0 { col - prev_col } else { col };
        data.push(SemanticToken {
            delta_line,
            delta_start,
            length,
            token_type: tt,
            token_modifiers_bitset: mods,
        });
        prev_line = line;
        prev_col = col;
    }

    SemanticTokensResult::Tokens(SemanticTokens {
        result_id: None,
        data,
    })
}

fn classify(kind: &SymbolKind) -> (u32, u32) {
    match kind {
        SymbolKind::Variable { mutable } => {
            let mods = if *mutable { 0 } else { MOD_READONLY };
            (TYPE_VARIABLE, mods)
        }
        SymbolKind::Parameter { .. } => (TYPE_PARAMETER, 0),
        SymbolKind::Function { .. } => (TYPE_FUNCTION, 0),
        SymbolKind::ExternFunction { .. } => (TYPE_FUNCTION, 0),
        SymbolKind::Struct { .. } => (TYPE_STRUCT, 0),
        SymbolKind::Enum { .. } => (TYPE_ENUM, 0),
        SymbolKind::EnumVariant { .. } => (TYPE_ENUM_MEMBER, 0),
        SymbolKind::Trait { .. } => (TYPE_TRAIT, 0),
        SymbolKind::Field { .. } => (TYPE_PROPERTY, 0),
        SymbolKind::BuiltinType { .. } => (TYPE_TYPE, MOD_STDLIB),
        SymbolKind::BuiltinFunction { .. } => (TYPE_FUNCTION, MOD_STDLIB),
        SymbolKind::BuiltinModule { .. } => (TYPE_NAMESPACE, MOD_STDLIB),
        SymbolKind::ExternalPackage { .. } => (TYPE_NAMESPACE, 0),
        SymbolKind::TypeAlias { .. } => (TYPE_TYPE, 0),
        SymbolKind::CNamespace { .. } => (TYPE_NAMESPACE, 0),
    }
}
