// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Written types → checker types.

use rask_ast::Span;
use rask_ast::ty::TypeExpr;

use super::type_table::TypeTable;
use super::errors::TypeError;

use crate::types::{GenericArg, Type};

/// The checker's type for a written one.
pub fn resolve_type_expr(ty: &TypeExpr, types: &TypeTable) -> Result<Type, TypeError> {
    let resolve_all = |ts: &[TypeExpr]| -> Result<Vec<Type>, TypeError> {
        ts.iter().map(|t| resolve_type_expr(t, types)).collect()
    };
    match ty {
        TypeExpr::Unit => Ok(Type::Unit),
        TypeExpr::NoneType => Ok(Type::None),
        TypeExpr::Union(parts) => {
            Ok(Type::union_named(resolve_all(parts)?, |id| Some(types.type_name(id))))
        }
        TypeExpr::Optional(inner) => Ok(Type::option(resolve_type_expr(inner, types)?)),
        TypeExpr::Result { ok, err } => Ok(Type::Result {
            ok: Box::new(resolve_type_expr(ok, types)?),
            err: Box::new(resolve_type_expr(err, types)?),
        }),
        TypeExpr::Tuple(elems) => Ok(Type::Tuple(resolve_all(elems)?)),
        TypeExpr::Array { elem, len } => {
            let elem = resolve_type_expr(elem, types)?;
            // A literal size, then a module-level `const` naming one. Anything
            // still symbolic (a comptime parameter, a computed const) keeps the
            // 0 placeholder so element checking can proceed — the length then
            // resolves at comptime.
            let len: usize = len.parse().ok().or_else(|| types.const_length(len)).unwrap_or(0);
            Ok(Type::Array { elem: Box::new(elem), len })
        }
        // `[N]T` is a layout spelling for `@binary` fields, not a value type.
        TypeExpr::FixedCount { .. } => Err(TypeError::GenericError(
            format!("`{}` is only a field layout in a `@binary` struct", ty.source()),
            Span::new(0, 0),
        )),
        TypeExpr::RawPtr(inner) => Ok(Type::RawPtr(Box::new(resolve_type_expr(inner, types)?))),
        TypeExpr::Func { params, ret } => Ok(Type::Fn {
            params: resolve_all(params)?,
            ret: Box::new(resolve_type_expr(ret, types)?),
        }),
        // A qualified name is the module's interface under the name the table
        // holds it by — the same unwrapping `resolve_named` does for
        // `io.Buffer`. Without it, `any io.Writer` named an interface nothing
        // could satisfy.
        TypeExpr::Any(inner) => Ok(Type::InterfaceObject {
            interface_name: unqualify_interface(&inner.name().unwrap_or_else(|| inner.to_string()), types),
        }),
        TypeExpr::Int(n) => Err(TypeError::GenericError(
            format!("`{}` is a value, not a type", n),
            Span::new(0, 0),
        )),
        TypeExpr::Named { path, args } => resolve_named_expr(path, args, types),
    }
}

fn resolve_named_expr(
    path: &[String],
    args: &[TypeExpr],
    types: &TypeTable,
) -> Result<Type, TypeError> {
    // `io.Buffer` is the module's `Buffer`.
    let path = match path {
        [module, rest @ ..] if !rest.is_empty() && rask_stdlib::modules::is_module(module) => rest,
        _ => path,
    };

    // AT3: a projection — `Self.Out`, `T.Out`. A dot that isn't one of these
    // belongs to a C namespace, registered under its dotted spelling.
    if let [head, tail] = path {
        if args.is_empty() && is_projection(head, tail, types) {
            return Ok(Type::Assoc {
                base: Box::new(resolve_named_expr(std::slice::from_ref(head), &[], types)?),
                name: tail.clone(),
            });
        }
    }

    let name = path.join(".");

    if !args.is_empty() {
        let args: Vec<GenericArg> = args
            .iter()
            .map(|a| match a {
                TypeExpr::Int(n) => n
                    .parse::<usize>()
                    .map(GenericArg::ConstUsize)
                    .map_err(|_| TypeError::GenericError(
                        format!("`{}` is not a size", n),
                        Span::new(0, 0),
                    )),
                t => Ok(GenericArg::Type(Box::new(resolve_type_expr(t, types)?))),
            })
            .collect::<Result<_, _>>()?;
        return resolve_generic(&name, args, types);
    }

    // A declared type parameter wins over a type of the same name. Without
    // this, `struct Holder<Output>` resolved `Output` to the stdlib's
    // `os.Output` and every use of the field mismatched against a type nobody
    // wrote (#915).
    if types.is_type_param_in_scope(&name) {
        return Ok(Type::UnresolvedNamed(name));
    }
    if let Some(ty) = types.lookup(&name) {
        return Ok(ty);
    }
    // Bare `Error` means the erased error box, same as `any Error` (#1095).
    // After the type-parameter check on purpose: a `<Error>` parameter is
    // still the parameter.
    if rask_ast::interfaces::is_bare_error(&name) {
        return Ok(Type::InterfaceObject { interface_name: "Error".to_string() });
    }
    Ok(Type::UnresolvedNamed(name))
}

/// `Name<args>` once the arguments are resolved.
fn resolve_generic(name: &str, args: Vec<GenericArg>, types: &TypeTable) -> Result<Type, TypeError> {
    match name {
        // `Heap<T>` keeps its wrapper. HP5 says it behaves as `T`, and this
        // used to implement that by unwrapping — which is transparency and
        // erasure at once. Nothing downstream could tell a block from the value
        // in it, so `func() -> Heap<i64>` was checked as `func() -> i64` and
        // `*f()` had nothing to load through. `unify` peels it instead (#1256).
        "Heap" if args.len() == 1 => {
            if !matches!(args.first(), Some(GenericArg::Type(_))) {
                return Err(TypeError::GenericError(
                    "Heap expects a type argument, not a const".to_string(),
                    Span::new(0, 0),
                ));
            }
            Ok(Type::UnresolvedGeneric { name: "Heap".to_string(), args })
        }
        // `Shared<T, S = Readers>` (conc.sync/SH2). The strategy is a defaulted
        // type parameter, so fill it in rather than leaving the arity short:
        // while `Shared<T>` carried no strategy, unify had nothing to compare
        // and a `Local` box flowed into a `Readers` annotation unchallenged —
        // then deadlocked at the first access (#960).
        "Shared" if args.len() == 1 => {
            let mut args = args;
            args.push(GenericArg::Type(Box::new(Type::UnresolvedNamed("Readers".to_string()))));
            Ok(generic_named(name, args, types))
        }
        "Option" if args.len() == 1 => match args.into_iter().next() {
            Some(GenericArg::Type(ty)) => Ok(Type::option(*ty)),
            _ => Err(TypeError::GenericError(
                "Option expects a type argument, not a const".to_string(),
                Span::new(0, 0),
            )),
        },
        "Result" if args.len() == 2 => {
            let mut iter = args.into_iter();
            match (iter.next(), iter.next()) {
                (Some(GenericArg::Type(ok)), Some(GenericArg::Type(err))) => Ok(Type::Result { ok, err }),
                _ => Err(TypeError::GenericError(
                    "Result expects two type arguments, not const".to_string(),
                    Span::new(0, 0),
                )),
            }
        }
        _ => Ok(generic_named(name, args, types)),
    }
}

fn generic_named(name: &str, args: Vec<GenericArg>, types: &TypeTable) -> Type {
    match types.get_type_id(name) {
        Some(base) => Type::Generic { base, args },
        None => Type::UnresolvedGeneric { name: name.to_string(), args },
    }
}

/// The name an interface is registered under, for a possibly module-qualified
/// spelling. `io.Writer` is `Writer` when that is what the table holds, or
/// `io$Writer` when the module prefix was folded into the key. Anything the
/// table doesn't know keeps the spelling it was written with, so the
/// "no interface named `io.Writer`" message still names what the author typed.
fn unqualify_interface(name: &str, types: &TypeTable) -> String {
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


/// AT3: does `head.tail` name an associated type rather than a module or C type?
///
/// `Self` on the left always does. Otherwise the left has to be a type
/// parameter — a name nothing declares — and the right has to look like a type.
/// `c.Rect` fails on both counts: it's registered under that exact spelling,
/// and `c` isn't a type name.
fn is_projection(head: &str, tail: &str, types: &TypeTable) -> bool {
    let plain = |n: &str| !n.is_empty() && n.chars().all(|c| c.is_alphanumeric() || c == '_');
    if !plain(head) || !plain(tail) {
        return false;
    }
    if !tail.starts_with(|c: char| c.is_ascii_uppercase()) {
        return false;
    }
    if head == "Self" {
        return true;
    }
    head.starts_with(|c: char| c.is_ascii_uppercase())
        && types.get_type_id(head).is_none()
        && types.get_type_id(&format!("{}.{}", head, tail)).is_none()
}
