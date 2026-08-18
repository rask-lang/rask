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
/// work. Both backends can answer these two from the tables they already hold —
/// the interpreter from its declaration maps, native from monomorphized layouts.
pub trait ReflectDecls {
    /// Does the program declare a struct with this exact name?
    fn declares_struct(&self, name: &str) -> bool;
    /// Does the program declare an enum with this exact name?
    fn declares_enum(&self, name: &str) -> bool;
}

/// The methods that need layout data or declaration attributes. Native has the
/// first, the interpreter has the second, and neither has both — so rather than
/// let them answer differently, both say so. Tracked in #791.
const NEEDS_LAYOUT: &str =
    "needs the monomorphized layout, which only the native backend has — \
     answering it on one backend and not the other would make the two disagree";

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

        "size_of" | "align_of" | "is_copy" | "is_flat" | "is_resource" => Unsupported(NEEDS_LAYOUT),

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

#[cfg(test)]
mod tests {
    use super::*;

    struct Decls;
    impl ReflectDecls for Decls {
        fn declares_struct(&self, name: &str) -> bool {
            name == "Point"
        }
        fn declares_enum(&self, name: &str) -> bool {
            name == "Colour"
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
    fn layout_dependent_methods_say_so_rather_than_guessing_zero() {
        for m in ["size_of", "align_of", "is_copy", "is_flat", "is_resource"] {
            assert!(
                matches!(ask(m, "Point"), ReflectAnswer::Unsupported(_)),
                "{m} must not answer with a placeholder",
            );
        }
    }

    #[test]
    fn an_unknown_name_is_not_a_reflect_method() {
        assert_eq!(ask("is_purple", "Point"), ReflectAnswer::NoSuchMethod);
    }
}
