// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! What `std.reflect` answers, in one place.
//!
//! Every one of these is a compile-time constant once monomorphization has
//! picked `T` (std.reflect/R5), so neither backend should be *calling* anything
//! — each folds the call to a literal. The rules live here because the two
//! backends share nothing below the AST: native reads monomorphized layouts and
//! the interpreter reads AST declarations, and when each derived its own answers
//! they drifted. The interpreter returned `false` for `is_integer<i32>()` and
//! native failed to lower the call at all (#775).
//!
//! `Unsupported` is deliberate rather than a placeholder. `size_of` used to
//! answer 0 on the interpreter, which reads as "this type is empty" instead of
//! "nobody implemented this" — a wrong number is worse than a message.

use std::collections::HashMap;

use rask_ast::ty::TypeExpr;

use crate::checker::{is_type_param_name, resolve_type_expr, TypeDef, TypeTable};
use crate::types::{GenericArg, Type, TypeId};

/// The value a reflect method folds to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReflectAnswer {
    Bool(bool),
    /// `usize` result — `size_of`, `align_of`.
    Int(u64),
    Str(String),
    /// The method exists in the spec but neither backend can answer it yet.
    /// Carries the reason, which goes straight into the diagnostic.
    Unsupported(&'static str),
    /// Not a reflect method at all.
    NoSuchMethod,
}

/// What a backend has to be able to say about a type name for the classifier to
/// work. Both answer these from the declarations — the interpreter from its
/// declaration maps, native from the monomorphized decl list it lowers from.
///
/// Declarations, not layouts: a layout has dropped `@resource` and has
/// substituted its field types by the time it exists, and both of those are what
/// `is_resource` and `is_flat` are asking about.
pub trait ReflectDecls {
    /// Does the program declare a struct with this exact name?
    fn declares_struct(&self, name: &str) -> bool;
    /// Does the program declare an enum with this exact name?
    fn declares_enum(&self, name: &str) -> bool;
    /// Is the declaration marked `@resource` (mem.resource-types)?
    fn is_resource(&self, name: &str) -> bool;
    /// Field types of a declared struct, or every variant payload type of a
    /// declared enum, as the source wrote them. `None` when nothing by that name
    /// is declared.
    fn member_types(&self, name: &str) -> Option<Vec<TypeExpr>>;
    /// Type parameter names of a declared struct or enum, if it has any.
    ///
    /// A field written with one of these has no concrete type until the
    /// instantiation says so, and neither backend carries that substitution on
    /// the declaration — so a walk over the fields would be reading the template.
    fn type_params(&self, name: &str) -> Vec<String>;
}

/// A name the type table doesn't know.
const NO_SUCH_TYPE: &str = "names no type the program declares";

/// A generic instantiation whose fields depend on its type arguments. R5 says
/// reflection sees the monomorphized type, and the declaration alone isn't it.
const NEEDS_INSTANTIATION: &str =
    "is a generic instantiation, and the walk would read the declaration's type \
     parameters rather than what this instantiation substituted for them";

/// Fold `reflect.<method><T>()` to its constant.
///
/// `ty` is `T` as written at the call site, already substituted by
/// monomorphization on the native path (std.reflect/R5) — so it's a concrete
/// type like `Point` or `Vec<i32>`, never a bare type parameter.
pub fn answer(method: &str, ty: &TypeExpr, decls: &dyn ReflectDecls, types: &TypeTable) -> ReflectAnswer {
    use ReflectAnswer::*;
    let head = ty.name().unwrap_or_default();
    match method {
        "name_of" => Str(ty.source()),
        "is_struct" => Bool(decls.declares_struct(&head)),
        "is_enum" => Bool(decls.declares_enum(&head)),
        "is_optional" => Bool(optional_payload(ty).is_some()),
        "is_vec" => Bool(head == "Vec"),
        "is_map" => Bool(head == "Map"),
        "is_integer" => Bool(ty.bare_name().is_some_and(is_integer)),
        "is_float" => Bool(matches!(ty.bare_name(), Some("f32" | "f64"))),

        // mem.resource-types: the annotation is the whole answer.
        "is_resource" => Bool(decls.is_resource(&head)),
        // mem.relocatable/FL1-FL5.
        "is_flat" => match flatness(ty, decls, &mut Vec::new()) {
            Flatness::Flat => Bool(true),
            Flatness::NotFlat => Bool(false),
            Flatness::Unknown => Unsupported(NEEDS_INSTANTIATION),
        },

        // The size the Copy rule measures is the size codegen lays the value
        // out in (`TypeTable::value_layout`), so these can't disagree with
        // whether the value copies.
        "size_of" | "align_of" | "is_copy" => {
            let Ok(resolved) = resolve_type_expr(ty, types) else {
                return Unsupported(NO_SUCH_TYPE);
            };
            let resolved = named(types, resolved);
            if resolved.contains(&|t| matches!(t, Type::UnresolvedNamed(_))) {
                return Unsupported(NO_SUCH_TYPE);
            }
            match method {
                "size_of" => Int(types.value_layout(&resolved).0 as u64),
                "align_of" => Int(types.value_layout(&resolved).1 as u64),
                _ => match types.copy_verdict_with(&resolved, &|_| false) {
                    crate::CopyVerdict::Copy => Bool(true),
                    crate::CopyVerdict::Move => Bool(false),
                    crate::CopyVerdict::Unknown => Unsupported(NEEDS_INSTANTIATION),
                },
            }
        }

        _ => NoSuchMethod,
    }
}

/// The payload of `T?`, either spelling: the sugar or `T or none`.
///
/// Nesting doesn't matter here: `T??` is optional at the outer layer, which is
/// the layer every operator sees (type.optionals/OPT30).
fn optional_payload(ty: &TypeExpr) -> Option<&TypeExpr> {
    match ty {
        TypeExpr::Optional(inner) => Some(inner),
        TypeExpr::Result { ok, err } if **err == TypeExpr::NoneType => Some(ok),
        _ => None,
    }
}

/// std.reflect: the integer primitives. `usize`/`isize` count — they're the
/// index and length type, and a format library asking "is this an integer"
/// wants yes for them.
fn is_integer(name: &str) -> bool {
    matches!(
        name,
        "i8" | "i16" | "i32" | "i64" | "i128"
            | "u8" | "u16" | "u32" | "u64" | "u128"
            | "usize" | "isize"
    )
}


/// Three answers, because "I can't tell" is not the same as "no".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flatness {
    Flat,
    NotFlat,
    Unknown,
}

/// mem.relocatable/FL1: a type is flat when it contains no heap-backed field,
/// recursively.
///
/// FL2 makes the primitives flat, FL3 makes every reference into a container
/// not flat, and FL5 extends the walk to an enum's variant payloads. A resource
/// type is never flat, whatever it holds.
///
/// `seen` breaks the cycle a self-referential type makes. A struct that reaches
/// itself does so through a reference or an `Owned<Self>`; both terminate on
/// their own, but a type that reached itself some other way would not.
fn flatness(ty: &TypeExpr, decls: &dyn ReflectDecls, seen: &mut Vec<String>) -> Flatness {
    // `T?` is flat exactly when its payload is: a tag byte holds no pointer.
    if let Some(inner) = optional_payload(ty) {
        return flatness(inner, decls, seen);
    }
    match ty {
        TypeExpr::Unit => return Flatness::Flat,
        // FL1: a raw pointer's bytes would survive an mmap and mean nothing on
        // the way back; an interface object and a closure point at their parts.
        TypeExpr::RawPtr(_) | TypeExpr::Any(_) | TypeExpr::Func { .. } => {
            return Flatness::NotFlat;
        }
        _ => {}
    }
    // `time.Instant` is `Instant`.
    let Some(base) = ty.last_segment() else { return Flatness::NotFlat };

    if is_flat_primitive(base) {
        return Flatness::Flat;
    }
    // FL3: a reference into a container is never flat — a `Link` is an
    // address. Answering here also terminates the walk: it is generic, and the
    // generic arm below would return Unknown.
    if base == "Link" || is_heap_backed(base) || decls.is_resource(base) {
        return Flatness::NotFlat;
    }

    let Some(members) = decls.member_types(base) else {
        // Not declared here and not a name the tables know: an opaque runtime
        // handle (`File`, `TcpListener`) or something out of scope. Neither is
        // safe to call flat.
        return Flatness::NotFlat;
    };

    if seen.iter().any(|s| s == base) {
        // Already on the stack — this arm contributes nothing new.
        return Flatness::Flat;
    }
    seen.push(base.to_string());
    // R5: reflection sees the monomorphized type, and a generic declaration's
    // fields are written in parameters the declaration can't fill in. Each
    // parameter stands in as `()`, the flattest type there is. Flatness only
    // gets worse as an argument does, so a field that's not flat even then
    // isn't flat for any argument: `Set<T>` holds a `Map`.
    let params = decls.type_params(base);
    let members: Vec<TypeExpr> = if params.is_empty() {
        members
    } else {
        members
            .iter()
            .map(|m| m.substitute(&|n| params.iter().any(|p| p == n).then_some(TypeExpr::Unit)))
            .collect()
    };
    let mut answer = Flatness::Flat;
    for member in &members {
        match flatness(member, decls, seen) {
            Flatness::Flat => {}
            Flatness::NotFlat => {
                answer = Flatness::NotFlat;
                break;
            }
            Flatness::Unknown => answer = Flatness::Unknown,
        }
    }
    seen.pop();
    // Flat with `()` filled in says nothing about other arguments.
    if answer != Flatness::NotFlat && !params.is_empty() {
        return Flatness::Unknown;
    }
    answer
}

/// mem.relocatable/FL2, plus the widths the spec's list implies.
fn is_flat_primitive(name: &str) -> bool {
    is_integer(name) || matches!(name, "bool" | "f32" | "f64" | "char")
}

/// FL1: the types that own or point at heap memory. A runtime handle's
/// storage is the runtime's; a struct holding one is found through its fields.
fn is_heap_backed(name: &str) -> bool {
    name == "string" || rask_stdlib::is_runtime_handle(name)
}

// ─── Questions the checker's type table answers ─────────────────────
//
// `fields` and `variants` describe storage, so they read layouts. The rest —
// which methods a type has, which interfaces it conforms to — are what the
// checker decided while checking the program, and both backends hold the same
// `TypeTable`. Each answer is computed here once, from that table, so neither
// backend has a version of its own to drift.

/// One method of a concrete type, as `reflect.methods<T>()` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodFact {
    /// The name a call writes: `mul`, not the `mul$f64` it's filed under.
    pub name: String,
    pub is_public: bool,
    /// Parameters a call passes in parentheses — `self` isn't one.
    pub param_count: usize,
    pub return_type_name: String,
}

/// `ty` as the table knows it: its declaration and the arguments this
/// instantiation gives the declaration's parameters.
struct Concrete<'t> {
    id: TypeId,
    def: &'t TypeDef,
    ty: Type,
    /// Declaration parameter → this instantiation's argument (R5).
    subst: HashMap<String, Type>,
}

fn concrete<'t>(types: &'t TypeTable, ty: &TypeExpr, method: &str) -> Result<Concrete<'t>, String> {
    let resolved = resolve_type_expr(ty, types)
        .map_err(|_| format!("reflect.{method}<{}>(): no type by that name", ty.source()))?;
    let resolved = named(types, resolved);
    let args: Vec<Type> = match &resolved {
        Type::Generic { args, .. } | Type::UnresolvedGeneric { args, .. } => args
            .iter()
            .filter_map(|a| match a {
                GenericArg::Type(t) => Some((**t).clone()),
                GenericArg::ConstUsize(_) => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    let not_declared = || {
        format!(
            "reflect.{method}<{}>(): `{}` has no declaration to read — this asks about a \
             struct, an enum or a nominal type",
            ty.source(),
            ty.source()
        )
    };
    let id = types.conformance_target(&resolved).ok_or_else(not_declared)?;
    let def = types.get(id).ok_or_else(not_declared)?;
    if matches!(def, TypeDef::Interface { .. }) {
        return Err(format!(
            "reflect.{method}<{}>(): `{}` is an interface, not a type",
            ty.source(),
            ty.source()
        ));
    }
    let params = def.type_params();
    if params.len() != args.len() {
        return Err(format!(
            "reflect.{method}<{}>(): `{}` takes {} type argument(s) — reflection describes \
             one instantiation, so write them (std.reflect/R5)",
            ty.source(),
            types.type_name(id),
            params.len()
        ));
    }
    let subst = params.iter().cloned().zip(args.iter().cloned()).collect();
    Ok(Concrete { id, def, ty: resolved, subst })
}

/// A declared type's name, as the id it resolves to — so two spellings of one
/// type compare equal.
fn named(types: &TypeTable, ty: Type) -> Type {
    match &ty {
        Type::UnresolvedNamed(n) => types.get_type_id(n).map(Type::Named).unwrap_or(ty),
        _ => ty,
    }
}

/// Does `ty` satisfy `iface`, by any route the checker accepts: a written
/// conformance, a derived one, or one the compiler answers by shape?
fn satisfies(types: &TypeTable, ty: &Type, iface: &TypeExpr) -> bool {
    crate::InterfaceChecker::new(types)
        .check_satisfies(ty, iface, rask_ast::Span::new(0, 0))
        .is_ok()
}

/// `reflect.methods<T>()`: every method a `T` has — what `T.name(…)` or
/// `t.name(…)` can call. Declared, conformance and derived methods alike, in
/// the order the checker recorded them. A method from an `extend` block whose
/// header or `where` clause this instantiation doesn't meet isn't one of its
/// methods: `Vec<f64>` has no `sort`.
pub fn methods(types: &TypeTable, ty: &TypeExpr) -> Result<Vec<MethodFact>, String> {
    let c = concrete(types, ty, "methods")?;
    let sigs: &[crate::MethodSig] = match c.def {
        TypeDef::Struct { methods, .. }
        | TypeDef::Enum { methods, .. }
        | TypeDef::NominalAlias { methods, .. }
        | TypeDef::Primitive { methods, .. } => methods,
        TypeDef::Union { .. } | TypeDef::Interface { .. } => &[],
    };
    let args: Vec<GenericArg> = c
        .def
        .type_params()
        .iter()
        .map(|p| GenericArg::Type(Box::new(c.subst[p].clone())))
        .collect();
    let mut out: Vec<MethodFact> = Vec::new();
    for sig in sigs {
        let mut subst = c.subst.clone();
        subst.insert("Self".to_string(), c.ty.clone());
        if !sig.owner_patterns.is_empty() {
            let Some(bound) = crate::bind_header_patterns(types, &sig.owner_patterns, &args) else {
                continue;
            };
            // The matcher binds every leaf, a concrete one too: `extend
            // Vec<u8>` "binds" `u8` to whatever a `Vec<f64>` holds. A concrete
            // leaf has to *be* the argument.
            let fits = bound.iter().all(|(leaf, actual)| {
                is_type_param_name(leaf)
                    || c.def.type_params().iter().any(|p| p == leaf)
                    || resolve_type_expr(&TypeExpr::named(leaf), types)
                        .is_ok_and(|t| named(types, t) == named(types, actual.clone()))
            });
            if !fits {
                continue;
            }
            subst.extend(bound);
        }
        let meets_where = sig.owner_bounds.iter().all(|(param, bounds)| {
            subst.get(param).is_none_or(|t| bounds.iter().all(|b| satisfies(types, t, b)))
        });
        if !meets_where {
            continue;
        }
        let name = rask_ast::operators::method_display(&sig.name).to_string();
        let ret = crate::substitute_type(&sig.ret, &subst);
        let fact = MethodFact {
            is_public: types.method_is_public(c.id, &name),
            param_count: sig.params.len(),
            return_type_name: types.resolve_type_names(&ret).to_type_expr().to_string(),
            name,
        };
        // The stdlib's declarations are read once as stubs and once with
        // bodies; the second sighting is the same method.
        if !out.contains(&fact) {
            out.push(fact);
        }
    }
    Ok(out)
}

/// `reflect.interfaces<T>()`: the conformances written for `T`, each as its
/// applied interface (`Mul<i64>`), in the order they were written. A derived
/// conformance isn't written anywhere and isn't listed — `implements` answers
/// for those. A conditional one is listed when this instantiation meets its
/// condition (R5).
pub fn interfaces(types: &TypeTable, ty: &TypeExpr) -> Result<Vec<String>, String> {
    let c = concrete(types, ty, "interfaces")?;
    let mut written = types.written_conformances(c.id);
    if let TypeDef::NominalAlias { with_interfaces, .. } = c.def {
        written.extend(with_interfaces.iter().cloned());
    }
    let mut out: Vec<String> = Vec::new();
    for iface in written {
        let name = iface.to_string();
        if satisfies(types, &c.ty, &iface) && !out.contains(&name) {
            out.push(name);
        }
    }
    Ok(out)
}

/// `reflect.implements<T, I>()`: whether `T` satisfies `I` the way a generic
/// bound `T: I` would be checked.
pub fn implements(types: &TypeTable, ty: &TypeExpr, iface: &TypeExpr) -> Result<bool, String> {
    if !crate::InterfaceChecker::new(types).names_an_interface(iface) {
        return Err(format!(
            "reflect.implements<{}, {}>(): `{}` isn't an interface",
            ty.source(),
            iface.source(),
            iface.source()
        ));
    }
    let resolved = resolve_type_expr(ty, types)
        .map_err(|_| format!("reflect.implements<{}, …>(): no type by that name", ty.source()))?;
    Ok(satisfies(types, &resolved, iface))
}

/// `reflect.has_field<T>(name)`: the struct's field names, in declaration
/// order. Anything that isn't a struct has none, so the answer is false rather
/// than an error — a predicate is how a caller finds out what it's holding.
pub fn field_names(types: &TypeTable, ty: &TypeExpr) -> Vec<String> {
    match concrete(types, ty, "has_field") {
        Ok(Concrete { def: TypeDef::Struct { fields, .. }, .. }) => {
            fields.iter().map(|(n, _)| n.clone()).collect()
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Decls;
    impl ReflectDecls for Decls {
        fn declares_struct(&self, name: &str) -> bool {
            matches!(name, "Point" | "Named" | "Boxed" | "Node" | "Conn")
        }
        fn declares_enum(&self, name: &str) -> bool {
            matches!(name, "Colour" | "Shape" | "Payload")
        }
        fn is_resource(&self, name: &str) -> bool {
            name == "Conn"
        }
        fn member_types(&self, name: &str) -> Option<Vec<TypeExpr>> {
            let m: Vec<TypeExpr> = match name {
                "Point" => vec![n("f64"), n("f64")],
                "Named" => vec![n("string"), n("i32")],
                "Boxed" => vec![n("T")],
                // Self-referential through a reference — the walk has to stop
                // there (FL3) rather than recurse into `Node` again.
                "Node" => vec![n("i64"), g("Link", n("Node"))],
                "Conn" => vec![n("i64")],
                "Colour" => vec![],
                "Shape" => vec![n("f64"), n("Point")],
                "Payload" => vec![n("string")],
                _ => return None,
            };
            Some(m)
        }
        fn type_params(&self, name: &str) -> Vec<String> {
            if name == "Boxed" { vec!["T".to_string()] } else { Vec::new() }
        }
    }

    fn n(s: &str) -> TypeExpr {
        TypeExpr::named(s)
    }

    fn g(head: &str, arg: TypeExpr) -> TypeExpr {
        TypeExpr::generic(head, vec![arg])
    }

    fn opt(t: TypeExpr) -> TypeExpr {
        TypeExpr::Optional(Box::new(t))
    }

    fn ask(method: &str, ty: TypeExpr) -> ReflectAnswer {
        answer(method, &ty, &Decls, &TypeTable::new())
    }

    #[test]
    fn category_predicates_answer_the_spec_table() {
        assert_eq!(ask("is_struct", n("Point")), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_struct", n("Colour")), ReflectAnswer::Bool(false));
        assert_eq!(ask("is_enum", n("Colour")), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_enum", n("Point")), ReflectAnswer::Bool(false));
        // The two the interpreter answered `false` for before #775.
        assert_eq!(ask("is_integer", n("i32")), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_integer", n("usize")), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_integer", n("f64")), ReflectAnswer::Bool(false));
        assert_eq!(ask("is_float", n("f64")), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_float", n("i32")), ReflectAnswer::Bool(false));
    }

    #[test]
    fn container_shapes_match_on_the_base_name() {
        assert_eq!(ask("is_vec", g("Vec", n("i32"))), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_vec", n("Vec")), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_map", g("Vec", n("i32"))), ReflectAnswer::Bool(false));
        // A user type whose name merely starts with Vec is not a Vec.
        assert_eq!(ask("is_vec", n("Vector")), ReflectAnswer::Bool(false));
        assert_eq!(ask("is_map", n("MapEntry")), ReflectAnswer::Bool(false));
    }

    #[test]
    fn optional_matches_both_spellings_and_nests() {
        assert_eq!(ask("is_optional", opt(n("Point"))), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_optional", opt(opt(n("i32")))), ReflectAnswer::Bool(true));
        let or_none = TypeExpr::Result { ok: Box::new(n("Point")), err: Box::new(TypeExpr::NoneType) };
        assert_eq!(ask("is_optional", or_none), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_optional", n("Point")), ReflectAnswer::Bool(false));
        // `T or E` for a real E is a result, not an optional.
        let result = TypeExpr::Result { ok: Box::new(n("Point")), err: Box::new(n("ParseError")) };
        assert_eq!(ask("is_optional", result), ReflectAnswer::Bool(false));
    }

    #[test]
    fn name_of_hands_back_the_spelling() {
        assert_eq!(ask("name_of", g("Vec", n("i32"))), ReflectAnswer::Str("Vec<i32>".into()));
    }

    #[test]
    fn sizes_come_from_the_layout_rules() {
        assert_eq!(ask("size_of", n("i32")), ReflectAnswer::Int(4));
        assert_eq!(ask("align_of", n("i64")), ReflectAnswer::Int(8));
        assert_eq!(ask("size_of", opt(n("i32"))), ReflectAnswer::Int(8));
        assert_eq!(ask("is_copy", n("string")), ReflectAnswer::Bool(true));
    }

    #[test]
    fn a_size_of_an_undeclared_name_says_so_rather_than_guessing() {
        assert!(matches!(ask("size_of", n("Nowhere")), ReflectAnswer::Unsupported(_)));
    }

    #[test]
    fn is_resource_reads_the_annotation() {
        assert_eq!(ask("is_resource", n("Conn")), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_resource", n("Point")), ReflectAnswer::Bool(false));
        assert_eq!(ask("is_resource", n("i32")), ReflectAnswer::Bool(false));
    }

    #[test]
    fn flatness_walks_fields_recursively() {
        // FL2: the primitives.
        assert_eq!(ask("is_flat", n("i32")), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_flat", n("f64")), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_flat", n("bool")), ReflectAnswer::Bool(true));
        // FL1: not the heap-backed ones.
        assert_eq!(ask("is_flat", n("string")), ReflectAnswer::Bool(false));
        assert_eq!(ask("is_flat", g("Vec", n("i32"))), ReflectAnswer::Bool(false));
        assert_eq!(ask("is_flat", TypeExpr::Any(Box::new(n("Shape")))), ReflectAnswer::Bool(false));
        // FL3: a link is an address.
        assert_eq!(ask("is_flat", g("Link", n("Node"))), ReflectAnswer::Bool(false));
        // FL1 recursively.
        assert_eq!(ask("is_flat", n("Point")), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_flat", n("Named")), ReflectAnswer::Bool(false));
        // A resource is never flat.
        assert_eq!(ask("is_flat", n("Conn")), ReflectAnswer::Bool(false));
        // FL5: an enum follows its variant payloads.
        assert_eq!(ask("is_flat", n("Colour")), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_flat", n("Shape")), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_flat", n("Payload")), ReflectAnswer::Bool(false));
        // A self-referential type through a reference terminates, and answers
        // false because FL3 stops at the reference.
        assert_eq!(ask("is_flat", n("Node")), ReflectAnswer::Bool(false));
        // An optional follows its payload.
        assert_eq!(ask("is_flat", opt(n("Point"))), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_flat", opt(n("Named"))), ReflectAnswer::Bool(false));
    }

    #[test]
    fn a_generic_declaration_cannot_answer_for_an_instantiation() {
        // R5 wants the monomorphized type; the declaration's fields are written
        // in its type parameters.
        assert!(matches!(ask("is_flat", g("Boxed", n("i32"))), ReflectAnswer::Unsupported(_)));
    }

    #[test]
    fn an_unknown_name_is_not_a_reflect_method() {
        assert_eq!(ask("is_purple", n("Point")), ReflectAnswer::NoSuchMethod);
    }
}
