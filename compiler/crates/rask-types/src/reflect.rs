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

use rask_ast::ty::TypeExpr;

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

/// The methods that need a size. Two size models live in the compiler — the
/// language one behind the 16-byte Copy threshold (`i32` is 4 bytes) and the
/// codegen one where every scalar occupies a word — and they disagree about
/// every struct with a narrow field. Answering with either before that's settled
/// would bake the choice in. Tracked in #791.
const NEEDS_LAYOUT: &str =
    "needs a size, and the compiler has two size models that disagree — the \
     language one behind the 16-byte Copy threshold, and the 8-byte-slot one \
     codegen lays out with";

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
pub fn answer(method: &str, ty: &TypeExpr, decls: &dyn ReflectDecls) -> ReflectAnswer {
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

        "size_of" | "align_of" | "is_copy" => Unsupported(NEEDS_LAYOUT),

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

    // R5: reflection sees the monomorphized type. A generic declaration's fields
    // are written in its type parameters, and substituting them needs the
    // instantiation, which isn't on the declaration.
    if !decls.type_params(base).is_empty() {
        return Flatness::Unknown;
    }

    if seen.iter().any(|s| s == base) {
        // Already on the stack — this arm contributes nothing new.
        return Flatness::Flat;
    }
    seen.push(base.to_string());
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
    answer
}

/// mem.relocatable/FL2, plus the widths and aliases the spec's list implies.
fn is_flat_primitive(name: &str) -> bool {
    is_integer(name) || matches!(name, "bool" | "f32" | "f64" | "char" | "int" | "uint")
}

/// FL1's list: the types that own or point at heap memory.
fn is_heap_backed(name: &str) -> bool {
    matches!(
        name,
        "string"
            | "Path"
            | "StringView"
            | "Vec"
            | "Wide"
            | "Map"
            | "Set"
            | "Shared"
            | "Mutex"
            | "Heap"
            | "Channel"
            | "Sender"
            | "Receiver"
            | "Handle"
            | "ThreadPool"
            | "StringBuilder"
            | "Iterator"
    )
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
        answer(method, &ty, &Decls)
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
    fn size_dependent_methods_say_so_rather_than_guessing_zero() {
        for m in ["size_of", "align_of", "is_copy"] {
            assert!(
                matches!(ask(m, n("Point")), ReflectAnswer::Unsupported(_)),
                "{m} must not answer with a placeholder",
            );
        }
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
