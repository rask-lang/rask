// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Type string parser.

use rask_ast::Span;

use super::type_table::TypeTable;
use super::errors::TypeError;

use crate::types::{GenericArg, Type};

/// Parse a type annotation string into a Type.
pub fn parse_type_string(s: &str, types: &TypeTable) -> Result<Type, TypeError> {
    let s = rask_stdlib::modules::strip_module_qualifier(s.trim());

    if s.is_empty() || s == "()" || s == "void" {
        return Ok(Type::Unit);
    }

    if s == "!" {
        return Ok(Type::Never);
    }

    if s == "none" {
        return Ok(Type::None);
    }

    // Union type: "IoError|ParseError" (pipe-separated at depth 0)
    if contains_pipe_at_depth_0(s) {
        let parts = split_at_pipe(s);
        let types_vec: Result<Vec<_>, _> = parts.iter()
            .map(|p| parse_type_string(p, types))
            .collect();
        return Ok(Type::union_named(types_vec?, |id| Some(types.type_name(id))));
    }

    // `T?`, including a parenthesised `T`. The parenthesised case used to be
    // excluded outright, so `(i64, bool)?` matched neither this nor the tuple
    // arm below — it ends with `?`, not `)` — and fell through to a name. What
    // reported it was `if o? as v`, which then read the annotation as something
    // that isn't an optional and asked for a Result (#1238).
    if s.ends_with('?') {
        let inner = parse_type_string(&s[..s.len() - 1], types)?;
        return Ok(Type::option(inner));
    }

    if s.starts_with('(') && s.ends_with(')') {
        let inner = &s[1..s.len() - 1];
        if inner.is_empty() {
            return Ok(Type::Unit);
        }
        // TU4: single-element tuple "(T,)" — trailing comma distinguishes from parens
        if inner.ends_with(',') {
            let elem_str = inner[..inner.len() - 1].trim();
            let elem = parse_type_string(elem_str, types)?;
            return Ok(Type::Tuple(vec![elem]));
        }
        let parts = split_type_args(inner);
        if parts.len() == 1 && !inner.contains(',') {
            return parse_type_string(inner, types);
        }
        let elems: Result<Vec<_>, _> = parts.iter().map(|p| parse_type_string(p, types)).collect();
        return Ok(Type::Tuple(elems?));
    }

    // `[]T` is not a type. There is no slice — a run of elements is a `Vec<T>`,
    // and part of one is `v.skip(a).take(n)`. The parser rejects the spelling
    // with a fix; this catches an internally built one, which is a compiler bug
    // rather than something an author wrote.
    if s.starts_with("[]") {
        return Err(TypeError::GenericError(
            format!("`{}` is not a type — write `Vec<{}>`", s, &s[2..]),
            Span::new(0, 0),
        ));
    }

    if s.starts_with('[') && s.ends_with(']') {
        let inner = &s[1..s.len() - 1];
        if let Some(semi_pos) = inner.find(';') {
            let elem_str = inner[..semi_pos].trim();
            let len_str = inner[semi_pos + 1..].trim();
            let elem = parse_type_string(elem_str, types)?;
            // A literal size, then a module-level `const` naming one. Anything
            // still symbolic (a comptime parameter, a computed const) keeps the
            // 0 placeholder so element checking can proceed — the length then
            // resolves at comptime.
            let len: usize = len_str
                .parse()
                .ok()
                .or_else(|| types.const_length(len_str))
                .unwrap_or(0);
            return Ok(Type::Array {
                elem: Box::new(elem),
                len,
            });
        }
        return Err(TypeError::GenericError(
            format!("`{}` is not a type — write `Vec<{}>`", s, inner),
            Span::new(0, 0),
        ));
    }

    // Raw pointer: *T
    if s.starts_with('*') {
        let inner = parse_type_string(&s[1..], types)?;
        return Ok(Type::RawPtr(Box::new(inner)));
    }

    if s.starts_with("func(") || s.starts_with("fn(") {
        return parse_fn_type(s, types);
    }

    if let Some(lt_pos) = s.find('<') {
        if s.ends_with('>') {
            let name = s[..lt_pos].trim();
            let args_str = &s[lt_pos + 1..s.len() - 1];
            let arg_strs = split_type_args(args_str);
            let args: Result<Vec<GenericArg>, _> =
                arg_strs.iter().map(|a| parse_generic_arg(a, types)).collect();
            let args = args?;

            match name {
                // `Heap<T>` keeps its wrapper. HP5 says it behaves as `T`, and
                // this used to implement that by unwrapping here — which is
                // transparency and erasure at once. Erasure is the part that
                // costs: nothing downstream can tell a block from the value in
                // it, so `func() -> Heap<i64>` is checked as `func() -> i64`
                // and `*f()` has nothing to load through. `unify` peels it
                // instead, so `T` still fits a `Heap<T>` slot and the other way
                // round (#1256).
                "Heap" if args.len() == 1 => {
                    if !matches!(args.first(), Some(GenericArg::Type(_))) {
                        return Err(TypeError::GenericError(
                            "Heap expects a type argument, not a const".to_string(),
                            Span::new(0, 0),
                        ));
                    }
                    return Ok(Type::UnresolvedGeneric { name: "Heap".to_string(), args });
                }
                // `Shared<T, S = Readers>` (conc.sync/SH2). The strategy is a
                // defaulted type parameter, so fill it in here rather than
                // leaving the arity short: `Shared<T>` and `Shared<T, Local>`
                // are different types, and while one of them carried no
                // strategy at all, unify had nothing to compare and a `Local`
                // box flowed into a `Readers` annotation unchallenged — then
                // deadlocked at the first access (#960).
                //
                // `extend Shared<T, S>` writes both parameters, so the
                // strategy-generic declarations in `stdlib/sync.rk` are
                // unaffected: they already have two args.
                "Shared" if args.len() == 1 => {
                    let mut args = args;
                    args.push(GenericArg::Type(Box::new(
                        Type::UnresolvedNamed("Readers".to_string()),
                    )));
                    // Built the same way the fallback below builds every other
                    // generic, so `Shared<T>` and `Shared<T, Readers>` are the
                    // same `Type` and not two spellings unify has to reconcile.
                    if let Some(base_id) = types.get_type_id(name) {
                        return Ok(Type::Generic { base: base_id, args });
                    }
                    return Ok(Type::UnresolvedGeneric {
                        name: name.to_string(),
                        args,
                    });
                }
                "Option" if args.len() == 1 => {
                    // Option takes a single type argument
                    if let GenericArg::Type(ty) = args.into_iter().next().unwrap() {
                        return Ok(Type::option(*ty));
                    } else {
                        return Err(TypeError::GenericError(
                            "Option expects a type argument, not a const".to_string(),
                            Span::new(0, 0),
                        ));
                    }
                }
                "Result" if args.len() == 2 => {
                    // Result takes two type arguments
                    let mut iter = args.into_iter();
                    let ok_arg = iter.next().unwrap();
                    let err_arg = iter.next().unwrap();

                    match (ok_arg, err_arg) {
                        (GenericArg::Type(ok), GenericArg::Type(err)) => {
                            return Ok(Type::Result { ok, err });
                        }
                        _ => {
                            return Err(TypeError::GenericError(
                                "Result expects two type arguments, not const".to_string(),
                                Span::new(0, 0),
                            ));
                        }
                    }
                }
                _ => {
                    if let Some(base_id) = types.get_type_id(name) {
                        return Ok(Type::Generic { base: base_id, args });
                    }
                    return Ok(Type::UnresolvedGeneric {
                        name: name.to_string(),
                        args,
                    });
                }
            }
        }
    }

    // Trait object: "any TraitName".
    //
    // A qualified name is the module's trait under the name the table holds it
    // by — the same unwrapping `resolve_named` does for `io.Buffer`. Without
    // it, `any io.Writer` parsed (since #1159) and then named a trait nothing
    // could satisfy, so every conformance check against it failed and the
    // methods weren't found either.
    if let Some(trait_name) = rask_ast::traits::trait_object_name(s) {
        return Ok(Type::TraitObject { trait_name: unqualify_trait(trait_name, types) });
    }

    // A declared type parameter wins over a type of the same name. Without
    // this, `struct Holder<Output>` resolved `Output` to the stdlib's
    // `os.Output` and every use of the field mismatched against a type nobody
    // wrote (#915). Single letters never reach the lookup at all (PC1), so this
    // is about the descriptive names — `Output`, `Item`, `Error` — which are
    // exactly the ones likely to collide.
    if types.is_type_param_in_scope(s) {
        return Ok(Type::UnresolvedNamed(s.to_string()));
    }

    if let Some(ty) = types.lookup(s) {
        return Ok(ty);
    }

    // Bare `Error` means the erased error box, same as `any Error` (#1095).
    //
    // It resolved to nothing before: `i64 or Error` became plain `i64`, so
    // `return Boom.Bad` from such a function reported "expected `i64`, found
    // `Boom`" — the error side had quietly gone. `any Error` did work, so the
    // only thing missing was reading the short spelling as the long one.
    //
    // After the type-parameter check above on purpose: a `<Error>` parameter is
    // still the parameter.
    if rask_ast::traits::is_bare_error(s) {
        return Ok(Type::TraitObject { trait_name: "Error".to_string() });
    }

    Ok(Type::UnresolvedNamed(s.to_string()))
}

/// The name a trait is registered under, for a possibly module-qualified
/// spelling. `io.Writer` is `Writer` when that is what the table holds, or
/// `io$Writer` when the module prefix was folded into the key. Anything the
/// table doesn't know keeps the spelling it was written with, so the
/// "no trait named `io.Writer`" message still names what the author typed.
fn unqualify_trait(name: &str, types: &TypeTable) -> String {
    if types.get_type_id(name).is_some() {
        return name.to_string();
    }
    let Some(dot) = name.find('.') else { return name.to_string() };
    let tail = &name[dot + 1..];
    if types.get_type_id(tail).is_some() {
        return tail.to_string();
    }
    let prefixed = format!("{}${}", &name[..dot], tail);
    if types.get_type_id(&prefixed).is_some() {
        return prefixed;
    }
    name.to_string()
}

/// Parse a single generic argument, which can be either a type or a const value.
fn parse_generic_arg(s: &str, types: &TypeTable) -> Result<GenericArg, TypeError> {
    let trimmed = s.trim();

    // Try to parse as a usize literal (const generic)
    if let Ok(n) = trimmed.parse::<usize>() {
        return Ok(GenericArg::ConstUsize(n));
    }

    // Otherwise parse as a type
    let ty = parse_type_string(trimmed, types)?;
    Ok(GenericArg::Type(Box::new(ty)))
}

pub(crate) fn split_type_args(s: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut depth = 0;
    let mut paren_depth = 0;
    let mut start = 0;

    // `>` only closes a `<` that is open. The arrow of a function type carries
    // one too, and counting it drove the depth negative — so the comma in
    // `Shared<func(i64) -> i64, Local>` was never at depth 0, the whole thing
    // came back as one argument, and the defaulting step then filled in the
    // missing strategy: `Shared.local(f)` was checked as a `Readers` box and
    // rejected the annotation that said `Local` (#1241). Same cause under
    // `Map<string, func(i64, i64) -> i64>`, which split down the middle of the
    // parameter list (#1151).
    for (i, c) in s.char_indices() {
        match c {
            '<' => depth += 1,
            '>' if depth > 0 => depth -= 1,
            '(' => paren_depth += 1,
            ')' if paren_depth > 0 => paren_depth -= 1,
            ',' if depth == 0 && paren_depth == 0 => {
                result.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }

    if start < s.len() {
        result.push(s[start..].trim());
    }

    result
}

/// Check if `|` appears at depth 0 (not inside `<>` or `()`).
fn contains_pipe_at_depth_0(s: &str) -> bool {
    let mut angle = 0;
    let mut paren = 0;
    for c in s.chars() {
        match c {
            '<' => angle += 1,
            '>' if angle > 0 => angle -= 1,
            '(' => paren += 1,
            ')' if paren > 0 => paren -= 1,
            '|' if angle == 0 && paren == 0 => return true,
            _ => {}
        }
    }
    false
}

/// Split a type string at `|` at depth 0.
fn split_at_pipe(s: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut angle = 0;
    let mut paren = 0;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '<' => angle += 1,
            '>' if angle > 0 => angle -= 1,
            '(' => paren += 1,
            ')' if paren > 0 => paren -= 1,
            '|' if angle == 0 && paren == 0 => {
                result.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < s.len() {
        result.push(s[start..].trim());
    }
    result
}

fn parse_fn_type(s: &str, types: &TypeTable) -> Result<Type, TypeError> {
    let prefix = if s.starts_with("func(") {
        "func("
    } else {
        "fn("
    };
    let rest = &s[prefix.len()..];

    let mut depth = 1;
    let mut paren_end = 0;
    for (i, c) in rest.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    paren_end = i;
                    break;
                }
            }
            _ => {}
        }
    }

    let params_str = &rest[..paren_end];
    let after_paren = &rest[paren_end + 1..].trim();

    let params: Result<Vec<_>, _> = if params_str.is_empty() {
        Ok(Vec::new())
    } else {
        split_type_args(params_str)
            .iter()
            .map(|p| parse_type_string(p, types))
            .collect()
    };
    let params = params?;

    let ret = if after_paren.starts_with("->") {
        let ret_str = after_paren[2..].trim();
        parse_type_string(ret_str, types)?
    } else {
        Type::Unit
    };

    Ok(Type::Fn {
        params,
        ret: Box::new(ret),
    })
}

