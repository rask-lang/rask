// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Written types → checker types.

use rask_ast::Span;
use rask_ast::ty::TypeExpr;

use super::type_defs::TypeDef;
use super::type_table::TypeTable;
use super::errors::{TypeArgSite, TypeError};
use super::TypeChecker;

use crate::types::{GenericArg, Type, TypeId};

impl TypeChecker {
    /// A type the program wrote, resolved, with anything wrong with it
    /// reported at `span`. `None` after reporting, so the caller falls back
    /// the way it does for any type it can't use.
    ///
    /// The one place a written type's errors get reported. Resolution happens
    /// in many places, several of them more than once for the same type, and
    /// those stay quiet: each position the program writes a type in reports
    /// through here exactly once.
    ///
    /// Two things can be wrong: the shape (`Box2<i64, string>` on a
    /// one-parameter `Box2`), which resolution itself refuses, and a name
    /// that names nothing (PC2). Resolution can't judge the second, because
    /// an unknown name and a type parameter come out the same, so it's asked
    /// here against the parameters in scope at this position.
    pub(super) fn resolve_written(&mut self, ty: &TypeExpr, span: Span) -> Option<Type> {
        let resolved = match resolve_type_expr(ty, &self.types) {
            Ok(t) => t,
            Err(e) => {
                self.errors.push(e.at(span));
                return None;
            }
        };
        let unknown = self.unknown_type_names(&resolved);
        if unknown.is_empty() {
            return Some(resolved);
        }
        for name in unknown {
            self.report_unknown_type_name(name, span);
        }
        None
    }

    /// Resolve written types with `params` in scope on top of whatever
    /// encloses them: a declaration's own, for its header and members.
    pub(super) fn with_type_params<R>(
        &mut self,
        params: Vec<String>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let outer = self.types.push_type_params(params);
        let out = f(self);
        self.types.pop_type_params(outer);
        out
    }
}

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
        TypeExpr::Any(inner) => Ok(types.interface_object_written(inner)),
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
    // `io.Buffer` is the module's `Buffer`, whatever the program calls its
    // own types (#1470).
    let (path, module) = match path {
        [module, rest @ ..] if !rest.is_empty() && types.module_named(module).is_some() => {
            (rest, Some(module.as_str()))
        }
        _ => (path, None),
    };
    if let (Some(module), [name]) = (module, path) {
        if let Some(id) = types.module_type_id(module, name) {
            if args.is_empty() {
                return Ok(Type::Named(id));
            }
            let args = resolve_type_args(args, types)?;
            return resolve_generic(name, Some(id), args, types);
        }
    }

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
        let args = resolve_type_args(args, types)?;
        return resolve_generic(&name, None, args, types);
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
        return Ok(types.interface_object("Error"));
    }
    Ok(Type::UnresolvedNamed(name))
}

/// Written type arguments, resolved.
fn resolve_type_args(args: &[TypeExpr], types: &TypeTable) -> Result<Vec<GenericArg>, TypeError> {
    args.iter()
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
        .collect()
}

/// `Name<args>` once the arguments are resolved. `pinned` is the declaration
/// a module-qualified spelling already named; `None` looks the name up.
fn resolve_generic(
    name: &str,
    pinned: Option<TypeId>,
    args: Vec<GenericArg>,
    types: &TypeTable,
) -> Result<Type, TypeError> {
    if let Some((params, required)) = declared_params(name, pinned, types) {
        let found = args.len();
        if found < required || found > params.len() {
            return Err(TypeError::TypeArgCount {
                name: name.to_string(),
                expected: if found > params.len() { params.len() } else { required },
                params,
                found,
                site: TypeArgSite::Type,
                span: Span::new(0, 0),
            });
        }
    }
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
            Ok(generic_named(name, pinned, args, types))
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
        _ => Ok(generic_named(name, pinned, args, types)),
    }
}

/// The parameters a generic name declares, and how many of them have to be
/// written. `None` where there's nothing to count against: a type parameter,
/// an interface (GT2 counts those, in bounds and headers), a name nothing
/// declares.
///
/// `Heap` and `Shared` are compiler-provided and have no declaration to read.
/// `Shared`'s strategy is a defaulted parameter (conc.sync/SH2), the one
/// type-side default there is.
fn declared_params(name: &str, pinned: Option<TypeId>, types: &TypeTable) -> Option<(Vec<String>, usize)> {
    match name {
        "Heap" => return Some((vec!["T".to_string()], 1)),
        "Shared" => return Some((vec!["T".to_string(), "S".to_string()], 1)),
        _ => {}
    }
    let id = match pinned {
        Some(id) => id,
        None if types.is_type_param_in_scope(name) => return None,
        None => types.get_type_id(name)?,
    };
    match types.get(id)? {
        TypeDef::Struct { type_params, .. } | TypeDef::Enum { type_params, .. } => {
            Some((type_params.clone(), type_params.len()))
        }
        _ => None,
    }
}

fn generic_named(name: &str, pinned: Option<TypeId>, args: Vec<GenericArg>, types: &TypeTable) -> Type {
    match pinned.or_else(|| types.get_type_id(name)) {
        Some(base) => Type::Generic { base, args },
        None => Type::UnresolvedGeneric { name: name.to_string(), args },
    }
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
