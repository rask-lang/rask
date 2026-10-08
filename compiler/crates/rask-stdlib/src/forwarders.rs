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
use rask_ast::ty::TypeExpr;

/// A type that reaches the sequence surface through `as_sequence`.
///
/// `(extend header, element type)`. The element is what `Sequence<T>`'s `T`
/// stands for on this host, and it's substituted into every signature.
fn hosts() -> Vec<(TypeExpr, TypeExpr)> {
    let t = || TypeExpr::named("T");
    vec![
        (TypeExpr::generic("Vec", vec![t()]), t()),
        (
            TypeExpr::generic("Map", vec![TypeExpr::named("K"), TypeExpr::named("V")]),
            TypeExpr::Tuple(vec![TypeExpr::named("K"), TypeExpr::named("V")]),
        ),
        (TypeExpr::generic("Set", vec![t()]), t()),
    ]
}

/// Is `name` a chain head — a collection that stands for its own sequence
/// (SEQ48), and so fills a `Sequence<E>` slot through its `as_sequence`?
pub fn is_chain_head(name: &str) -> bool {
    hosts().iter().any(|(header, _)| header.name().as_deref() == Some(name))
}

/// Rask source declaring every host's generated forwarders.
pub fn generated_source(sequence_src: &str, host_srcs: &[&str]) -> String {
    let seq = parse(sequence_src);
    let methods = sequence_surface(&seq);

    let mut out = String::from(
        "// SPDX-License-Identifier: (MIT OR Apache-2.0)\n\
         // Generated from `extend Sequence<T>` — see rask-stdlib/src/forwarders.rs.\n\n",
    );
    for (header, elem) in hosts() {
        let base = header.name().unwrap_or_default();
        let host_params: Vec<String> =
            header.args().iter().filter_map(|a| a.bare_name().map(str::to_string)).collect();
        let declared: Vec<String> = host_srcs
            .iter()
            .flat_map(|src| declared_methods(&parse(src), &base))
            .collect();

        let mut block = String::new();
        for m in &methods {
            if declared.iter().any(|d| d == &m.name) {
                continue;
            }
            // A terminal that rebuilds the host is `clone` under another name,
            // and `std.api/SD5` gives one operation one spelling. `to_vec` on a
            // `Vec` is the whole of that case; on a `Set` it converts and stays.
            if m.params.len() == 1 && m.ret_ty.as_ref() == Some(&header) {
                continue;
            }
            // `count` walks to work out something the host already knows: a
            // collection has `len`, so `v.count()` was a second spelling of it,
            // and the slower one. A sequence keeps `count` because it genuinely
            // has to walk — that's the whole difference between the two words.
            if m.name == "count" && declared.iter().any(|d| d == "len") {
                continue;
            }
            block.push_str(&forwarder(m, &elem, &host_params));
        }
        if !block.is_empty() {
            out.push_str(&format!("extend {} {{\n{}}}\n\n", header.source(), block));
        }
    }
    out
}

/// One forwarder: the sequence method's signature over the host, body
/// delegating through `as_sequence`.
fn forwarder(m: &FnDecl, elem: &TypeExpr, host_params: &[String]) -> String {
    // The method's own parameters, renamed off any the host already uses.
    // `Sequence.min_by_key<K>` over a `Map<K, V>` would otherwise declare a `K`
    // that hides the map's key type, and `func((K, V)) -> K` then means two
    // different things in one signature.
    let renames = collisions(m, elem, host_params);
    let name_of = |n: &str| renames.get(n).cloned().unwrap_or_else(|| n.to_string());
    // Renames first, while the method's `K` is still the only `K` in the type;
    // the host's element goes in after, so its own `K` is never renamed.
    let over_host = |t: &TypeExpr| {
        let mut t = t.clone();
        t.rename(&|n| renames.get(n).cloned());
        t.substitute(&|n| (n == "T").then(|| elem.clone())).source()
    };

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
                    let bounds: Vec<String> = p.bounds.iter().map(|b| b.ty.source()).collect();
                    format!("{}: {}", name_of(&p.name), bounds.join(" + "))
                }
            })
            .collect();
        format!("<{}>", list.join(", "))
    };

    // `self` is the receiver, not an argument to pass on.
    let rest = &m.params[1..];
    let params: Vec<String> = rest
        .iter()
        .map(|p| {
            let ty = p.ty.as_ref().map(&over_host).unwrap_or_default();
            let mode = if p.is_take {
                "take "
            } else if p.is_mutate {
                "mutate "
            } else {
                ""
            };
            format!("{}{}: {}", mode, p.name, ty)
        })
        .collect();
    // `mutate` is written at the call too; `take` isn't.
    let args: Vec<String> = rest
        .iter()
        .map(|p| if p.is_mutate { format!("mutate {}", p.name) } else { p.name.clone() })
        .collect();
    let returns_nothing = matches!(m.ret_ty, None | Some(TypeExpr::Unit));
    let ret = match &m.ret_ty {
        Some(t) if !returns_nothing => format!(" -> {}", over_host(t)),
        _ => String::new(),
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
    elem: &TypeExpr,
    host_params: &[String],
) -> std::collections::HashMap<String, String> {
    let mut taken: std::collections::HashSet<String> = host_params.iter().cloned().collect();
    taken.extend(m.type_params.iter().map(|p| p.name.clone()));
    elem.walk_names(&mut |n| {
        taken.insert(n.to_string());
    });
    let mut out = std::collections::HashMap::new();
    for p in &m.type_params {
        if !host_params.iter().any(|h| h == &p.name) {
            continue;
        }
        let free = ('A'..='Z')
            .map(|c| c.to_string())
            .find(|c| !taken.contains(c))
            .unwrap_or_else(|| format!("{}_", p.name));
        taken.insert(free.clone());
        out.insert(p.name.clone(), free);
    }
    out
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
                if i.target_ty == TypeExpr::generic("Sequence", vec![TypeExpr::named("T")])
                    && i.where_bounds.is_empty()
                    && i.interface.is_none() =>
            {
                Some(i.methods.clone())
            }
            _ => None,
        })
        .flatten()
        .filter(|m| {
            m.is_pub && m.params.first().is_some_and(|p| p.ty.as_ref().is_some_and(|t| t.is_name("Self")))
        })
        .collect()
}

/// Every method name the host already declares, under any `extend` shape.
fn declared_methods(decls: &[Decl], base: &str) -> Vec<String> {
    decls
        .iter()
        .filter_map(|d| match &d.kind {
            DeclKind::Impl(i) if i.target_ty.name().as_deref() == Some(base) => {
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
            for name in ["filter", "map", "any", "fold", "find"] {
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
    fn a_container_keeps_len_and_never_grows_count() {
        // Walking to work out something the host already answers is a second,
        // slower spelling of `len` (`std.api/SD5`). A `Sequence` keeps `count`
        // because it genuinely has to walk.
        for host in ["Vec<T>", "Map<K, V>", "Set<T>"] {
            let block = block_for(host);
            assert!(!block.contains("public func count"), "{}", block);
        }
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
