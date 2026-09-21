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
const HOSTS: &[(&str, &str)] = &[("Vec<T>", "T"), ("Map<K, V>", "(K, V)"), ("Set<T>", "T")];

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
        let host_params: Vec<String> = header
            .find('<')
            .map(|i| {
                header[i + 1..]
                    .trim_end_matches('>')
                    .split(',')
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty())
                    .collect()
            })
            .unwrap_or_default();
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
            block.push_str(&forwarder(m, elem, &host_params));
        }
        if !block.is_empty() {
            out.push_str(&format!("extend {} {{\n{}}}\n\n", header, block));
        }
    }
    out
}

/// One forwarder: the sequence method's signature over the host, body
/// delegating through `as_sequence`.
fn forwarder(m: &FnDecl, elem: &str, host_params: &[String]) -> String {
    // The method's own parameters, renamed off any the host already uses.
    // `Sequence.min_by_key<K>` over a `Map<K, V>` would otherwise declare a `K`
    // that hides the map's key type, and `func((K, V)) -> K` then means two
    // different things in one signature.
    let renames = collisions(m, elem, host_params);
    let name_of = |n: &str| renames.get(n).cloned().unwrap_or_else(|| n.to_string());

    let type_params = if m.type_params.is_empty() {
        String::new()
    } else {
        let list: Vec<String> = m
            .type_params
            .iter()
            .map(|p| {
                if p.bounds.is_empty() {
                    name_of(&p.name)
                } else {
                    format!("{}: {}", name_of(&p.name), p.bounds.join(" + "))
                }
            })
            .collect();
        format!("<{}>", list.join(", "))
    };

    // `self` is the receiver, not an argument to pass on.
    let rest = &m.params[1..];
    let params: Vec<String> = rest
        .iter()
        .map(|p| format!("{}: {}", p.name, substitute(&rename(&p.ty, &renames), elem)))
        .collect();
    let args: Vec<String> = rest.iter().map(|p| p.name.clone()).collect();
    let returns_nothing = matches!(m.ret_ty.as_deref(), None | Some("()") | Some("void"));
    let ret = if returns_nothing {
        String::new()
    } else {
        format!(
            " -> {}",
            substitute(&rename(m.ret_ty.as_deref().unwrap_or("void"), &renames), elem)
        )
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

/// A new name for each of the method's type parameters that the host already
/// spells, picked from the letters neither of them uses.
fn collisions(
    m: &FnDecl,
    elem: &str,
    host_params: &[String],
) -> std::collections::HashMap<String, String> {
    let mut taken: std::collections::HashSet<String> = host_params.iter().cloned().collect();
    taken.extend(m.type_params.iter().map(|p| p.name.clone()));
    let mut out = std::collections::HashMap::new();
    for p in &m.type_params {
        if !host_params.iter().any(|h| h == &p.name) {
            continue;
        }
        let free = ('A'..='Z')
            .map(|c| c.to_string())
            .find(|c| !taken.contains(c) && !elem.contains(c.as_str()))
            .unwrap_or_else(|| format!("{}_", p.name));
        taken.insert(free.clone());
        out.insert(p.name.clone(), free);
    }
    out
}

/// Apply `collisions`' renames to a rendered type, whole words only.
///
/// Runs *before* the element substitution, so that a method's `K` becomes `A`
/// while it is still the only `K` in the string — substituting `(K, V)` in
/// first would make the host's key indistinguishable from it.
fn rename(ty: &str, renames: &std::collections::HashMap<String, String>) -> String {
    if renames.is_empty() {
        return ty.to_string();
    }
    map_words(ty, |w| renames.get(w).cloned().unwrap_or_else(|| w.to_string()))
}

/// Rewrite `Sequence<T>`'s element name to the host's.
///
/// Whole-word only: `T` in `func(T) -> bool` is the element, the `T` inside
/// `Token` is not.
fn substitute(ty: &str, elem: &str) -> String {
    if elem == "T" {
        return to_source(ty);
    }
    to_source(&map_words(ty, |w| {
        if w == "T" { elem.to_string() } else { w.to_string() }
    }))
}

/// Rewrite each identifier in a rendered type, leaving the punctuation alone.
fn map_words(ty: &str, f: impl Fn(&str) -> String) -> String {
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
            out.push_str(&f(&ty[start..i]));
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
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

    /// The block a host's forwarders land in.
    fn block_for(host: &str) -> String {
        let src = generated();
        let start = src
            .find(&format!("extend {} {{", host))
            .unwrap_or_else(|| panic!("no block for `{}`:\n{}", host, src));
        let end = src[start..].find("\n}").expect("unterminated block");
        src[start..start + end].to_string()
    }

    #[test]
    fn fills_the_gaps_a_hand_copied_surface_left() {
        let block = block_for("Vec<T>");
        for name in ["take_while", "skip_while", "chain", "for_each", "min_by_key"] {
            assert!(
                block.contains(&format!("public func {}", name)),
                "`Vec` should inherit `{}`:\n{}",
                name,
                block
            );
        }
    }

    #[test]
    fn a_map_and_a_set_get_the_same_surface() {
        for host in ["Map<K, V>", "Set<T>"] {
            let block = block_for(host);
            for name in ["filter", "map", "count", "any", "fold"] {
                assert!(
                    block.contains(&format!("public func {}", name)),
                    "`{}` should inherit `{}`:\n{}",
                    host,
                    name,
                    block
                );
            }
        }
    }

    #[test]
    fn a_methods_own_parameter_is_renamed_off_the_hosts() {
        // `Sequence.min_by_key<K>` over a `Map<K, V>` would declare a second
        // `K`, and `func((K, V)) -> K` then means two things at once.
        let block = block_for("Map<K, V>");
        assert!(
            block.contains("public func min_by_key<A: Comparable>(self, key: func((K, V)) -> A)"),
            "{}",
            block
        );
    }

    #[test]
    fn a_hand_written_method_wins() {
        // `Vec.flat_map` takes a `Vec<U>` where the sequence one takes a
        // `Sequence<U>`, so generating over it would change the call sites.
        let block = block_for("Vec<T>");
        assert!(!block.contains("public func flat_map"), "{}", block);
        // `to_vec` on a `Vec` is `clone` under a second name (`std.api/SD5`).
        // A `Map`'s builds a `Vec<(K, V)>`, so it stays.
        assert!(!block.contains("public func to_vec"), "{}", block);
        assert!(block_for("Map<K, V>").contains("public func to_vec"));
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
