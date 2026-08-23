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
    /// declared enum, spelled as the source wrote them. `None` when nothing by
    /// that name is declared.
    fn member_type_names(&self, name: &str) -> Option<Vec<String>>;
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
/// `type_name` is `T` as spelled at the call site, already substituted by
/// monomorphization on the native path (std.reflect/R5) — so it's a concrete
/// name like `Point` or `Vec<i32>`, never a bare type parameter.
pub fn answer(method: &str, type_name: &str, decls: &dyn ReflectDecls) -> ReflectAnswer {
    use ReflectAnswer::*;
    match method {
        "name_of" => Str(type_name.to_string()),
        "is_struct" => Bool(decls.declares_struct(type_name)),
        "is_enum" => Bool(decls.declares_enum(type_name)),
        "is_optional" => Bool(is_optional(type_name)),
        "is_vec" => Bool(container_is(type_name, "Vec")),
        "is_map" => Bool(container_is(type_name, "Map")),
        "is_integer" => Bool(is_integer(type_name)),
        "is_float" => Bool(matches!(type_name, "f32" | "f64")),

        // mem.resource-types: the annotation is the whole answer.
        "is_resource" => Bool(decls.is_resource(type_name)),
        // mem.relocatable/FL1-FL5.
        "is_flat" => match flatness(type_name, decls, &mut Vec::new()) {
            Flatness::Flat => Bool(true),
            Flatness::NotFlat => Bool(false),
            Flatness::Unknown => Unsupported(NEEDS_INSTANTIATION),
        },

        "size_of" | "align_of" | "is_copy" => Unsupported(NEEDS_LAYOUT),

        _ => NoSuchMethod,
    }
}

/// `T?` — the sugar, and the `T or none` spelling it desugars from.
///
/// Nesting doesn't matter here: `T??` is optional at the outer layer, which is
/// the layer every operator sees (type.optionals/OPT30).
fn is_optional(name: &str) -> bool {
    name.ends_with('?') || name.trim_end().ends_with(" or none")
}

/// `Vec<…>` / `Map<…>`, and the bare name a not-yet-parameterized spelling
/// leaves behind. Matching on the prefix rather than parsing the argument list
/// is enough — nothing else in the language is named `Vec` or `Map`.
fn container_is(name: &str, base: &str) -> bool {
    name == base || (name.starts_with(base) && name[base.len()..].starts_with('<'))
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
/// FL2 makes the primitives flat, FL3 makes `Handle<T>` flat — its components are
/// integers, which is the point of a handle — and FL5 extends the walk to an
/// enum's variant payloads. A resource type is never flat, whatever it holds.
///
/// `seen` breaks the cycle a self-referential type makes. A struct that reaches
/// itself does so through a `Handle<Self>` or an `Owned<Self>`; both terminate on
/// their own, but a type that reached itself some other way would not.
fn flatness(name: &str, decls: &dyn ReflectDecls, seen: &mut Vec<String>) -> Flatness {
    let name = name.trim();

    // `T?` is flat exactly when its payload is: a tag byte holds no pointer.
    if let Some(inner) = name.strip_suffix('?') {
        return flatness(inner, decls, seen);
    }
    if let Some(inner) = name.strip_suffix(" or none") {
        return flatness(inner, decls, seen);
    }

    let base = base_name(name);

    if is_flat_primitive(base) {
        return Flatness::Flat;
    }
    // FL3: index and generation, no pointer.
    if base == "Handle" || base == "WeakHandle" {
        return Flatness::Flat;
    }
    if is_heap_backed(base) || name.starts_with("any ") || name.starts_with("func(") {
        return Flatness::NotFlat;
    }
    if decls.is_resource(base) {
        return Flatness::NotFlat;
    }

    let Some(members) = decls.member_type_names(base) else {
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
    for member in members {
        match flatness(&member, decls, seen) {
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

/// `Vec<i32>` -> `Vec`, `time.Instant` -> `Instant`, `*u8` -> `*u8`.
fn base_name(name: &str) -> &str {
    let name = name.trim();
    let base = name.split('<').next().unwrap_or(name).trim();
    base.rsplit('.').next().unwrap_or(base)
}

/// mem.relocatable/FL2, plus the widths and aliases the spec's list implies.
fn is_flat_primitive(name: &str) -> bool {
    is_integer(name)
        || matches!(name, "bool" | "f32" | "f64" | "char" | "()" | "int" | "uint")
}

/// FL1's list: the types that own or point at heap memory. A raw pointer counts —
/// the bytes would survive an mmap and mean nothing on the way back.
fn is_heap_backed(name: &str) -> bool {
    name.starts_with('*')
        || matches!(
            name,
            "string"
                | "Path"
                | "StringView"
                | "Vec"
                | "Wide"
                | "Map"
                | "Set"
                | "Pool"
                | "Cell"
                | "Shared"
                | "Mutex"
                | "Heap"
                | "Channel"
                | "Sender"
                | "Receiver"
                | "TaskHandle"
                | "ThreadHandle"
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
        fn member_type_names(&self, name: &str) -> Option<Vec<String>> {
            let m: &[&str] = match name {
                "Point" => &["f64", "f64"],
                "Named" => &["string", "i32"],
                "Boxed" => &["T"],
                // Self-referential through a handle, which is flat (FL3).
                "Node" => &["i64", "Handle<Node>"],
                "Conn" => &["i64"],
                "Colour" => &[],
                "Shape" => &["f64", "Point"],
                "Payload" => &["string"],
                _ => return None,
            };
            Some(m.iter().map(|s| s.to_string()).collect())
        }
        fn type_params(&self, name: &str) -> Vec<String> {
            if name == "Boxed" { vec!["T".to_string()] } else { Vec::new() }
        }
    }

    fn ask(method: &str, ty: &str) -> ReflectAnswer {
        answer(method, ty, &Decls)
    }

    #[test]
    fn category_predicates_answer_the_spec_table() {
        assert_eq!(ask("is_struct", "Point"), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_struct", "Colour"), ReflectAnswer::Bool(false));
        assert_eq!(ask("is_enum", "Colour"), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_enum", "Point"), ReflectAnswer::Bool(false));
        // The two the interpreter answered `false` for before #775.
        assert_eq!(ask("is_integer", "i32"), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_integer", "usize"), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_integer", "f64"), ReflectAnswer::Bool(false));
        assert_eq!(ask("is_float", "f64"), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_float", "i32"), ReflectAnswer::Bool(false));
    }

    #[test]
    fn container_shapes_match_on_the_base_name() {
        assert_eq!(ask("is_vec", "Vec<i32>"), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_vec", "Vec"), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_map", "Map<string, i32>"), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_map", "Vec<i32>"), ReflectAnswer::Bool(false));
        // A user type whose name merely starts with Vec is not a Vec.
        assert_eq!(ask("is_vec", "Vector"), ReflectAnswer::Bool(false));
        assert_eq!(ask("is_map", "MapEntry"), ReflectAnswer::Bool(false));
    }

    #[test]
    fn optional_matches_both_spellings_and_nests() {
        assert_eq!(ask("is_optional", "Point?"), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_optional", "i32??"), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_optional", "Point or none"), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_optional", "Point"), ReflectAnswer::Bool(false));
        // `T or E` for a real E is a result, not an optional.
        assert_eq!(ask("is_optional", "Point or ParseError"), ReflectAnswer::Bool(false));
    }

    #[test]
    fn name_of_hands_back_the_spelling() {
        assert_eq!(ask("name_of", "Vec<i32>"), ReflectAnswer::Str("Vec<i32>".into()));
    }

    #[test]
    fn size_dependent_methods_say_so_rather_than_guessing_zero() {
        for m in ["size_of", "align_of", "is_copy"] {
            assert!(
                matches!(ask(m, "Point"), ReflectAnswer::Unsupported(_)),
                "{m} must not answer with a placeholder",
            );
        }
    }

    #[test]
    fn is_resource_reads_the_annotation() {
        assert_eq!(ask("is_resource", "Conn"), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_resource", "Point"), ReflectAnswer::Bool(false));
        assert_eq!(ask("is_resource", "i32"), ReflectAnswer::Bool(false));
    }

    #[test]
    fn flatness_walks_fields_recursively() {
        // FL2: the primitives.
        assert_eq!(ask("is_flat", "i32"), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_flat", "f64"), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_flat", "bool"), ReflectAnswer::Bool(true));
        // FL1: not the heap-backed ones.
        assert_eq!(ask("is_flat", "string"), ReflectAnswer::Bool(false));
        assert_eq!(ask("is_flat", "Vec<i32>"), ReflectAnswer::Bool(false));
        assert_eq!(ask("is_flat", "Map<string, i32>"), ReflectAnswer::Bool(false));
        assert_eq!(ask("is_flat", "any Shape"), ReflectAnswer::Bool(false));
        // FL3: a handle is integers.
        assert_eq!(ask("is_flat", "Handle<Node>"), ReflectAnswer::Bool(true));
        // FL1 recursively.
        assert_eq!(ask("is_flat", "Point"), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_flat", "Named"), ReflectAnswer::Bool(false));
        // A resource is never flat.
        assert_eq!(ask("is_flat", "Conn"), ReflectAnswer::Bool(false));
        // FL5: an enum follows its variant payloads.
        assert_eq!(ask("is_flat", "Colour"), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_flat", "Shape"), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_flat", "Payload"), ReflectAnswer::Bool(false));
        // A self-referential type through a handle terminates.
        assert_eq!(ask("is_flat", "Node"), ReflectAnswer::Bool(true));
        // An optional follows its payload.
        assert_eq!(ask("is_flat", "Point?"), ReflectAnswer::Bool(true));
        assert_eq!(ask("is_flat", "Named?"), ReflectAnswer::Bool(false));
    }

    #[test]
    fn a_generic_declaration_cannot_answer_for_an_instantiation() {
        // R5 wants the monomorphized type; the declaration's fields are written
        // in its type parameters.
        assert!(matches!(ask("is_flat", "Boxed<i32>"), ReflectAnswer::Unsupported(_)));
    }

    #[test]
    fn an_unknown_name_is_not_a_reflect_method() {
        assert_eq!(ask("is_purple", "Point"), ReflectAnswer::NoSuchMethod);
    }
}
