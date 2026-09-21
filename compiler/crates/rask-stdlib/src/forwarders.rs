// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Generates each collection's sequence surface from `Sequence`'s own.
//!
//! `type.sequence/SEQ48` says a collection is its own chain head: `v.filter(p)`
//! starts a sequence over `v`, there is no `.iter()`. The mechanism behind that
//! was sixteen forwarders copied by hand into `extend Vec<T>`, each one
//! `return self.as_sequence().<name>(…)`. Copies rot: `take` was there and
//! `take_while` wasn't, so half a pair worked. `Map` and `Set` had none at all.
//!
//! So the forwarders are generated instead. A type opts in by declaring
//! `as_sequence(self) -> Sequence<E>` and appearing in `HOSTS`; every method of
//! an unconditional `extend Sequence<T>` block it doesn't already declare comes
//! back as a forwarder. Writing the method by hand still wins — `Vec.flat_map`
//! takes a `Vec<U>` rather than a `Sequence<U>` and has to stay hand-written.
//!
//! The result is Rask source, parsed as one more stub file, so the checker, MIR
//! and the interpreter see ordinary declarations and need to know nothing about
//! this.

use rask_ast::decl::{Decl, DeclKind, FnDecl};

/// A type that reaches the sequence surface through `as_sequence`.
///
/// `(extend header, element type)`. The element is what `Sequence<T>`'s `T`
/// stands for on this host, and it's substituted into every signature.
const HOSTS: &[(&str, &str)] = &[("Vec<T>", "T")];

/// Rask source declaring every host's generated forwarders.
pub fn generated_source(sequence_src: &str, host_srcs: &[&str]) -> String {
    let seq = parse(sequence_src);
    let methods = sequence_surface(&seq);

    let mut out = String::from(
        "// SPDX-License-Identifier: (MIT OR Apache-2.0)\n\
         // Generated from `extend Sequence<T>` — see rask-stdlib/src/forwarders.rs.\n\n",
    );
    for (header, elem) in HOSTS {
        let base = header.split('<').next().unwrap_or(header);
        let declared: Vec<String> = host_srcs
            .iter()
            .flat_map(|src| declared_methods(&parse(src), base))
            .collect();

        let mut block = String::new();
        for m in &methods {
            if declared.iter().any(|d| d == &m.name) {
                continue;
            }
            // A terminal that rebuilds the host is `clone` under another name,
            // and `std.api/SD5` gives one operation one spelling. `to_vec` on a
            // `Vec` is the whole of that case; on a `Set` it converts and stays.
            if m.params.len() == 1 && m.ret_ty.as_deref() == Some(*header) {
                continue;
            }
            block.push_str(&forwarder(m, elem));
        }
        if !block.is_empty() {
            out.push_str(&format!("extend {} {{\n{}}}\n\n", header, block));
        }
    }
    out
}

/// One forwarder: the sequence method's signature over the host, body
/// delegating through `as_sequence`.
fn forwarder(m: &FnDecl, elem: &str) -> String {
    let type_params = if m.type_params.is_empty() {
        String::new()
    } else {
        let list: Vec<String> = m
            .type_params
            .iter()
            .map(|p| {
                if p.bounds.is_empty() {
                    p.name.clone()
                } else {
                    format!("{}: {}", p.name, p.bounds.join(" + "))
                }
            })
            .collect();
        format!("<{}>", list.join(", "))
    };

    // `self` is the receiver, not an argument to pass on.
    let rest = &m.params[1..];
    let params: Vec<String> = rest
        .iter()
        .map(|p| format!("{}: {}", p.name, substitute(&p.ty, elem)))
        .collect();
    let args: Vec<String> = rest.iter().map(|p| p.name.clone()).collect();
    let returns_nothing = matches!(m.ret_ty.as_deref(), None | Some("()") | Some("void"));
    let ret = if returns_nothing {
        String::new()
    } else {
        format!(" -> {}", substitute(m.ret_ty.as_deref().unwrap_or("void"), elem))
    };

    let signature = format!(
        "    public func {}{}(self{}{}){} {{\n",
        m.name,
        type_params,
        if params.is_empty() { "" } else { ", " },
        params.join(", "),
        ret,
    );
    // `void` has nothing to return, and `return f()` on a void call is not a
    // thing you can write.
    let call = format!("self.as_sequence().{}({})", m.name, args.join(", "));
    let body = if returns_nothing {
        format!("        {}\n", call)
    } else {
        format!("        return {}\n", call)
    };
    format!("{}{}    }}\n", signature, body)
}

/// Rewrite `Sequence<T>`'s element name to the host's.
///
/// Whole-word only: `T` in `func(T) -> bool` is the element, the `T` inside
/// `Token` is not.
fn substitute(ty: &str, elem: &str) -> String {
    if elem == "T" {
        return to_source(ty);
    }
    let mut out = String::with_capacity(ty.len());
    let bytes = ty.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_alphanumeric() || c == '_' {
            let start = i;
            while i < bytes.len() && {
                let c = bytes[i] as char;
                c.is_alphanumeric() || c == '_'
            } {
                i += 1;
            }
            let word = &ty[start..i];
            out.push_str(if word == "T" { elem } else { word });
        } else {
            out.push(c);
            i += 1;
        }
    }
    to_source(&out)
}

/// Undo the parser's internal spelling for a closure that returns nothing.
///
/// `func(T)` is stored as `func(T) -> ()`, and `()` isn't a type you can write
/// — the parser rejects it with "use `void`". Same rule the formatter applies
/// on the way out (`rask-fmt/src/printer.rs`).
fn to_source(ty: &str) -> String {
    ty.replace(" -> ()", "")
}

/// Methods of every unconditional `extend Sequence<T>` block.
///
/// A bounded block (`where T: Numeric`) or a shaped one (`extend
/// Sequence<string>`) is left alone: the condition has to hold on the host too,
/// and nothing here checks that.
fn sequence_surface(decls: &[Decl]) -> Vec<FnDecl> {
    decls
        .iter()
        .filter_map(|d| match &d.kind {
            DeclKind::Impl(i)
                if i.target_ty == "Sequence<T>"
                    && i.where_bounds.is_empty()
                    && i.trait_names.is_empty()
                    && !i.is_scoped =>
            {
                Some(i.methods.clone())
            }
            _ => None,
        })
        .flatten()
        .filter(|m| m.is_pub && m.params.first().is_some_and(|p| p.ty == "Self"))
        .collect()
}

/// Every method name the host already declares, under any `extend` shape.
fn declared_methods(decls: &[Decl], base: &str) -> Vec<String> {
    decls
        .iter()
        .filter_map(|d| match &d.kind {
            DeclKind::Impl(i) if i.target_ty.split('<').next() == Some(base) => {
                Some(i.methods.iter().map(|m| m.name.clone()).collect::<Vec<_>>())
            }
            _ => None,
        })
        .flatten()
        .collect()
}

fn parse(src: &str) -> Vec<Decl> {
    let lexed = rask_lexer::Lexer::new(src).tokenize();
    if !lexed.is_ok() {
        return Vec::new();
    }
    rask_parser::Parser::new(lexed.tokens)
        .allow_keyword_fn_names()
        .parse()
        .decls
}

#[cfg(test)]
mod tests {
    fn generated() -> String {
        crate::stubs::stub_sources()
            .find(|(name, _, _)| *name == crate::stubs::FORWARDERS_FILE)
            .map(|(_, src, _)| src.to_string())
            .expect("the generated source is one of the stub sources")
    }

    #[test]
    fn fills_the_gaps_a_hand_copied_surface_left() {
        let src = generated();
        for name in ["take_while", "skip_while", "chain", "for_each", "min_by_key"] {
            assert!(
                src.contains(&format!("public func {}", name)),
                "`Vec` should inherit `{}`:\n{}",
                name,
                src
            );
        }
    }

    #[test]
    fn a_hand_written_method_wins() {
        // `Vec.flat_map` takes a `Vec<U>` where the sequence one takes a
        // `Sequence<U>`, so generating over it would change the call sites.
        let src = generated();
        assert!(!src.contains("public func flat_map"), "{}", src);
        // `to_vec` on a `Vec` is `clone` under a second name (`std.api/SD5`).
        assert!(!src.contains("public func to_vec"), "{}", src);
    }

    #[test]
    fn the_generated_source_parses() {
        let src = generated();
        let lexed = rask_lexer::Lexer::new(&src).tokenize();
        assert!(lexed.is_ok(), "generated source doesn't lex:\n{}", src);
        let parsed = rask_parser::Parser::new(lexed.tokens)
            .allow_keyword_fn_names()
            .parse();
        assert!(
            parsed.errors.is_empty(),
            "generated source doesn't parse: {:?}\n{}",
            parsed.errors,
            src
        );
    }
}
