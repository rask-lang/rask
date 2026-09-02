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
        return Ok(Type::union(types_vec?));
    }

    if s.ends_with('?') && !s.starts_with('(') {
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

    if s.starts_with("[]") {
        let inner = parse_type_string(&s[2..], types)?;
        return Ok(Type::Slice(Box::new(inner)));
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
        let inner = parse_type_string(inner, types)?;
        return Ok(Type::Slice(Box::new(inner)));
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
                "Heap" if args.len() == 1 => {
                    // Owned<T> is transparent to the type checker — unwrap to T
                    if let GenericArg::Type(ty) = args.into_iter().next().unwrap() {
                        return Ok(*ty);
                    } else {
                        return Err(TypeError::GenericError(
                            "Owned expects a type argument, not a const".to_string(),
                            Span::new(0, 0),
                        ));
                    }
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
    if let Some(trait_name) = rask_ast::traits::trait_object_name(s) {
        return Ok(Type::TraitObject { trait_name: trait_name.to_string() });
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

    Ok(Type::UnresolvedNamed(s.to_string()))
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

fn split_type_args(s: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut depth = 0;
    let mut paren_depth = 0;
    let mut start = 0;

    for (i, c) in s.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            '(' => paren_depth += 1,
            ')' => paren_depth -= 1,
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

