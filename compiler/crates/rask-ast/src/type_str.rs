// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Reading the type strings the parser produces.
//!
//! Types in the AST are stored as rendered strings (`FnDecl::ret_ty` and
//! friends), and the parser renders `T or E` in a canonical form the source
//! never uses: `Result<T, E>`. Anything downstream that wants to know "is this
//! a result type?" has to ask about that canonical form, not about the surface
//! syntax — the lint's `try_*` rule asked whether the string contained `" or "`,
//! which no rendered result type ever does, so it rejected every correctly
//! written `try_*` in the language (#893).
//!
//! Several places were splitting `Result<…>` apart by hand before this existed,
//! and they had already drifted: `rask-mono`'s copy counted `[` `]` as nesting
//! and `rask-types`' copy didn't. That matters because `Vec[T, N]` is a real
//! type form (type.simd/T1), so `Result<Vec[f32, 4], SimdError>` split at the
//! comma inside the brackets in one place and at the right one in the other.
//! Nothing in the tree hits it yet — SIMD is still pending — which is exactly
//! how a divergence like that survives. One answer now, in `result_parts`.

/// Split canonical `Result<T, E>` into its two halves.
///
/// Splits at the top-level comma, so nested generics survive:
/// `Result<(), GrowError<T>>` gives `("()", "GrowError<T>")`.
pub fn result_parts(s: &str) -> Option<(&str, &str)> {
    let s = s.trim();
    let inner = s.strip_prefix("Result<")?.strip_suffix('>')?;
    let mut depth = 0i32;
    for (i, b) in inner.bytes().enumerate() {
        match b {
            b'<' | b'(' | b'[' => depth += 1,
            b'>' | b')' | b']' => depth -= 1,
            b',' if depth == 0 => {
                return Some((inner[..i].trim(), inner[i + 1..].trim()));
            }
            _ => {}
        }
    }
    None
}

/// Is this rendered type a `T or E` result?
pub fn is_result(s: &str) -> bool {
    result_parts(s).is_some()
}

/// Is this rendered type a `T?` optional?
pub fn is_optional(s: &str) -> bool {
    s.trim().ends_with('?')
}

/// Drop the namespace a type is reached through: `time.Duration` → `Duration`.
///
/// A module import binds the module and nothing else (structure.modules/IM1), so
/// a qualified name is the ordinary way to write a stdlib type — and what it is
/// reached *through* says nothing about the type itself. Whatever the head is,
/// the last segment names the type: a module, a module alias, an external
/// package, a C header's namespace.
///
/// This is the question a *layout* asks. Resolution has a narrower one — there a
/// wrong strip changes which type resolves, so `rask_stdlib::modules::
/// strip_module_qualifier` drops the head only when it names a real module.
pub fn bare_name(s: &str) -> &str {
    let s = s.trim();
    s.rsplit_once('.').map_or(s, |(_, tail)| tail)
}

/// Render a parsed type back the way it's written in source, for messages.
///
/// A diagnostic that prints `Result<(), TrySendError>` while telling you to
/// write `T or E` is naming a spelling Rask doesn't have.
pub fn to_source(s: &str) -> String {
    match result_parts(s) {
        Some((ok, err)) => format!("{} or {}", to_source(ok), to_source(err)),
        None if s.trim() == "()" => "void".to_string(),
        None => s.trim().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_name_drops_any_namespace() {
        assert_eq!(bare_name("time.Duration"), "Duration");
        assert_eq!(bare_name("h.Response"), "Response");
        assert_eq!(bare_name("c.Rect"), "Rect");
        assert_eq!(bare_name("Duration"), "Duration");
        assert_eq!(bare_name(" os.Output "), "Output");
    }

    #[test]
    fn splits_plain_result() {
        assert_eq!(result_parts("Result<i64, ParseError>"), Some(("i64", "ParseError")));
    }

    #[test]
    fn splits_past_nested_generics() {
        // The naive "split on the first comma" answer gets `GrowError<V` here.
        assert_eq!(
            result_parts("Result<Option<V>, GrowError<V>>"),
            Some(("Option<V>", "GrowError<V>"))
        );
        assert_eq!(result_parts("Result<(), GrowError<T>>"), Some(("()", "GrowError<T>")));
    }

    #[test]
    fn splits_past_a_simd_lane_count() {
        // `Vec[T, N]` (type.simd/T1) carries a top-level-looking comma inside
        // brackets. A splitter that only tracks `<` and `(` returns
        // ("Vec[f32", "4], SimdError") here — the divergence this module
        // replaced. Nothing in the tree reaches it yet, so only a test does.
        assert_eq!(
            result_parts("Result<Vec[f32, 4], SimdError>"),
            Some(("Vec[f32, 4]", "SimdError"))
        );
        assert_eq!(
            result_parts("Result<Vec[f32, native], SimdError>"),
            Some(("Vec[f32, native]", "SimdError"))
        );
        assert_eq!(to_source("Result<Vec[f32, 4], SimdError>"), "Vec[f32, 4] or SimdError");
    }

    #[test]
    fn non_results_are_not_results() {
        for s in ["i64", "Option<V>", "Handle<T>?", "void", "()", "Vec<i64>"] {
            assert!(!is_result(s), "{} read as a result", s);
        }
    }

    #[test]
    fn renders_back_to_rask_syntax() {
        assert_eq!(to_source("Result<(), TrySendError>"), "void or TrySendError");
        assert_eq!(to_source("Result<T, TryReceiveError>"), "T or TryReceiveError");
        assert_eq!(to_source("i64"), "i64");
        assert_eq!(to_source("()"), "void");
    }

    #[test]
    fn optionals() {
        assert!(is_optional("Handle<T>?"));
        assert!(!is_optional("Handle<T>"));
    }
}

/// `Ring<i64>` split into its base name and its written type arguments.
///
/// `None` for a name with no `<…>`. Nested arguments stay whole:
/// `Pair<Vec<i64>, string>` gives `["Vec<i64>", "string"]`.
pub fn split_generic_name(name: &str) -> Option<(&str, Vec<&str>)> {
    let open = name.find('<')?;
    if !name.trim_end().ends_with('>') {
        return None;
    }
    let base = name[..open].trim();
    let inner = name[open + 1..name.trim_end().len() - 1].trim();
    let mut args = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (i, c) in inner.char_indices() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                args.push(inner[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < inner.len() {
        args.push(inner[start..].trim());
    }
    if args.is_empty() {
        return None;
    }
    Some((base, args))
}

/// The base name of a written instantiation — `Ring` for `Ring<i64>`.
pub fn generic_base_name(name: &str) -> Option<String> {
    split_generic_name(name).map(|(base, _)| base.to_string())
}

/// The key mono lays a generic instantiation out under: `Ring<i64>` → `Ring$i64`.
///
/// Mirrors `rask_mono::generic_instance_name`, which builds the same string from
/// the resolved types. Reflection only has the written spelling, so it rebuilds
/// it from that.
pub fn generic_instance_key(name: &str) -> Option<String> {
    let (base, args) = split_generic_name(name)?;
    Some(format!("{}${}", base, args.join("$")))
}

/// Replace whole type-parameter names in a rendered type: `Vec<T>` with
/// `T → i64` becomes `Vec<i64>`.
///
/// Textual because that is the shape reflection reports — `FieldInfo.type_name`
/// is a string, and the layout has already rendered the declared type. Only
/// whole identifiers are replaced, so a `T` inside `Trait` is left alone.
pub fn substitute_type_params(rendered: &str, subst: &[(String, String)]) -> String {
    if subst.is_empty() {
        return rendered.to_string();
    }
    let mut out = String::with_capacity(rendered.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if word.is_empty() {
            return;
        }
        match subst.iter().find(|(p, _)| p == word) {
            Some((_, arg)) => out.push_str(arg),
            None => out.push_str(word),
        }
        word.clear();
    };
    for c in rendered.chars() {
        if c.is_alphanumeric() || c == '_' {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// A declaration's type parameters paired with an instantiation's written
/// arguments — `Ring<i64>` on `struct Ring<T>` gives `[("T", "i64")]`.
///
/// Empty when the name isn't an instantiation or the counts disagree: a partial
/// mapping would substitute some fields and not others, which reads as a
/// compiler bug rather than as the declared types it fell back to.
pub fn generic_type_subst(type_name: &str, type_params: &[String]) -> Vec<(String, String)> {
    let Some((_, args)) = split_generic_name(type_name) else {
        return Vec::new();
    };
    if type_params.len() != args.len() {
        return Vec::new();
    }
    type_params
        .iter()
        .cloned()
        .zip(args.iter().map(|a| a.to_string()))
        .collect()
}

#[cfg(test)]
mod generic_name_tests {
    use super::*;

    #[test]
    fn a_plain_name_is_not_an_instantiation() {
        assert_eq!(split_generic_name("Ring"), None);
        assert_eq!(generic_instance_key("Ring"), None);
    }

    #[test]
    fn one_argument() {
        assert_eq!(split_generic_name("Ring<i64>"), Some(("Ring", vec!["i64"])));
        assert_eq!(generic_instance_key("Ring<i64>").as_deref(), Some("Ring$i64"));
    }

    #[test]
    fn a_nested_argument_stays_whole() {
        assert_eq!(
            split_generic_name("Pair<Vec<i64>, string>"),
            Some(("Pair", vec!["Vec<i64>", "string"]))
        );
    }

    #[test]
    fn substitution_replaces_whole_identifiers_only() {
        let subst = vec![("T".to_string(), "i64".to_string())];
        assert_eq!(substitute_type_params("Vec<T>", &subst), "Vec<i64>");
        assert_eq!(substitute_type_params("T", &subst), "i64");
        // `T` inside a longer name is part of that name.
        assert_eq!(substitute_type_params("Trait", &subst), "Trait");
        assert_eq!(substitute_type_params("Map<T, T>", &subst), "Map<i64, i64>");
    }

    #[test]
    fn a_count_mismatch_substitutes_nothing() {
        assert!(generic_type_subst("Ring<i64>", &["T".into(), "U".into()]).is_empty());
        assert_eq!(
            generic_type_subst("Ring<i64>", &["T".into()]),
            vec![("T".to_string(), "i64".to_string())]
        );
    }
}
