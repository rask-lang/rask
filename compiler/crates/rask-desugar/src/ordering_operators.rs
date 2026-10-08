// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! ORD1: `<`, `<=`, `>`, `>=` follow from `compare`.
//!
//! A type that writes its own `compare` and none of the four gets them here, as
//! ordinary methods that call it. `Comparable` is compiler-provided, so there is
//! no interface declaration for TD2 to copy defaults from (#1331). Without
//! these the type wasn't Comparable at all — the checker wants all five — so
//! `sort()`, which needs `T: Comparable`, couldn't take a type whose order
//! isn't its fields' order. And the derived structural `<` would have ignored
//! the hand-written order anyway.
//!
//! Only a `compare` with a Rask body: a native one belongs to a type whose
//! operators the runtime already answers.

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use std::collections::{HashMap, HashSet};

use crate::interface_defaults::Injected;

const OPERATORS: [(&str, &str); 4] = [
    ("lt", "self.compare(other) == Ordering.Less"),
    ("le", "self.compare(other) != Ordering.Greater"),
    ("gt", "self.compare(other) == Ordering.Greater"),
    ("ge", "self.compare(other) != Ordering.Less"),
];

pub(crate) fn inject(decls: &mut [Decl], injected: &mut Injected) {
    // Every method name each type has, across its declaration and all blocks:
    // one hand-written `lt` anywhere means the type has chosen its operators.
    let mut owned: HashMap<String, HashSet<String>> = HashMap::new();
    for decl in decls.iter() {
        let (ty, methods) = match &decl.kind {
            DeclKind::Struct(s) => (s.name.to_string(), &s.methods),
            DeclKind::Enum(e) => (e.name.to_string(), &e.methods),
            DeclKind::Impl(i) => (i.target_ty.name().unwrap_or_default(), &i.methods),
            _ => continue,
        };
        owned.entry(ty).or_default().extend(methods.iter().map(|m| m.name.clone()));
    }

    for (decl_index, decl) in decls.iter_mut().enumerate() {
        let DeclKind::Impl(block) = &mut decl.kind else { continue };
        let target = block.target_ty.name().unwrap_or_default();
        let Some(compare) = block.methods.iter().find(|m| is_written_compare(m)) else {
            continue;
        };
        let Some(other_ty) = compare.params.get(1).and_then(|p| p.ty.clone()) else {
            continue;
        };
        let span = compare.span;
        let is_pub = compare.is_pub;
        let have = owned.entry(target).or_default();
        if OPERATORS.iter().any(|(name, _)| have.contains(*name)) {
            continue;
        }
        for (name, body) in OPERATORS {
            let Some(mut method) = template(name, body, span) else { continue };
            method.is_pub = is_pub;
            if let Some(other) = method.params.get_mut(1) {
                other.ty = Some(other_ty.clone());
            }
            have.insert(name.to_string());
            injected.entry(decl_index).or_default().insert(block.methods.len());
            block.methods.push(method);
        }
    }
}

fn is_written_compare(m: &FnDecl) -> bool {
    m.name == "compare"
        && m.params.len() == 2
        && m.params[0].name == "self"
        && !m.body_lives_elsewhere()
}

/// `func {name}(self, other: Placeholder) -> bool { return {body} }`, parsed.
/// The placeholder type is replaced by the caller with `compare`'s own.
fn template(name: &str, body: &str, span: rask_ast::Span) -> Option<FnDecl> {
    let text = format!(
        "extend Placeholder {{\n    func {name}(self, other: Placeholder) -> bool {{\n        return {body}\n    }}\n}}\n"
    );
    let lex = rask_lexer::Lexer::new_with_file_id(&text, span.file_id).tokenize();
    if !lex.errors.is_empty() {
        return None;
    }
    let mut parser = rask_parser::Parser::new_with_file_id(lex.tokens, 0, span.file_id);
    let parsed = parser.parse();
    if !parsed.errors.is_empty() {
        return None;
    }
    let DeclKind::Impl(block) = parsed.decls.into_iter().next()?.kind else { return None };
    let mut method = block.methods.into_iter().next()?;
    method.span = span;
    method.decl_start = span.start;
    Some(method)
}
