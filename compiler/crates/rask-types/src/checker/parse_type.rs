// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Type string parser.

use rask_ast::Span;

use super::type_table::TypeTable;
use super::errors::TypeError;

use crate::types::{GenericArg, Type};

/// Parse a type annotation string into a Type.
pub fn parse_type_string(s: &str, types: &TypeTable) -> Result<Type, TypeError> {
    let s = s.trim();

    // Strip field projection syntax: `Type.{field1, field2}` → `Type`
    // Projections affect borrowing, not the type itself.
    let s = strip_projection(s);
    let s = s.as_ref();

    if s.is_empty() || s == "()" {
        return Ok(Type::Unit);
    }

    if s == "!" {
        return Ok(Type::Never);
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
        return Ok(Type::Option(Box::new(inner)));
    }

    if s.starts_with('(') && s.ends_with(')') {
        let inner = &s[1..s.len() - 1];
        if inner.is_empty() {
            return Ok(Type::Unit);
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
            // Numeric size or comptime param name — use placeholder 0 for symbolic sizes
            // so element type checking proceeds. Actual size resolves at comptime.
            let len: usize = len_str.parse().unwrap_or(0);
            return Ok(Type::Array {
                elem: Box::new(elem),
                len,
            });
        }
        let inner = parse_type_string(inner, types)?;
        return Ok(Type::Slice(Box::new(inner)));
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
                "Owned" if args.len() == 1 => {
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
                "Option" if args.len() == 1 => {
                    // Option takes a single type argument
                    if let GenericArg::Type(ty) = args.into_iter().next().unwrap() {
                        return Ok(Type::Option(ty));
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

/// Strip field projection suffix from a type string.
/// `"GameState.{entities, score}"` → `"GameState"`
/// `"Vec<i32>"` → `"Vec<i32>"` (unchanged)
fn strip_projection(s: &str) -> std::borrow::Cow<'_, str> {
    if let Some(pos) = s.find(".{") {
        // Verify it ends with `}`
        if s.ends_with('}') {
            return std::borrow::Cow::Owned(s[..pos].to_string());
        }
    }
    std::borrow::Cow::Borrowed(s)
}

/// Extract projection fields from a type string, if any.
/// `"GameState.{entities, score}"` → `Some(vec!["entities", "score"])`
/// `"Vec<i32>"` → `None`
pub fn extract_projection(s: &str) -> Option<Vec<String>> {
    if let Some(pos) = s.find(".{") {
        if s.ends_with('}') {
            let fields_str = &s[pos + 2..s.len() - 1];
            let fields: Vec<String> = fields_str
                .split(',')
                .map(|f| f.trim().to_string())
                .filter(|f| !f.is_empty())
                .collect();
            if !fields.is_empty() {
                return Some(fields);
            }
        }
    }
    None
}
