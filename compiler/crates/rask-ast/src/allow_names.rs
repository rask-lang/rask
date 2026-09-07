// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Every name `@allow(...)` accepts.
//!
//! Two sets of names meet in one annotation: compiler warnings, matched in the
//! type checker, and lint rule ids, matched in `rask-lint`. Neither crate can
//! see the other's list, so a misspelled name matched nothing and the warning
//! fired as if the annotation weren't there — indistinguishable from one you
//! suppressed correctly that later stopped firing for its own reasons (#1085).
//!
//! The names live here, below both, and each side checks its half against this
//! list in a test so the two can't drift apart.

/// Compiler warnings that can be suppressed at a declaration.
pub const WARNINGS: &[&str] = &["torn_lock_update"];

/// Lint rule ids. Must match `rask_lint`'s registry exactly — a rule with no id
/// here can't be suppressed, and an id with no rule suppresses nothing.
pub const LINT_RULES: &[&str] = &[
    "naming/from",
    "naming/into",
    "naming/as",
    "naming/to",
    "naming/is",
    "naming/with",
    "naming/try",
    "naming/or_suffix",
    "idiom/unwrap-production",
    "idiom/missing-ensure",
    "idiom/ensure-ordering",
    "idiom/large-unsafe-block",
    "idiom/duck-trait",
    "idiom/equality-absent-check",
    "idiom/mod-for-index",
    "idiom/too-many-contexts",
    "style/snake-case-func",
    "style/pascal-case-type",
    "style/public-return-type",
];

/// Is this a name `@allow` can act on?
pub fn is_known(name: &str) -> bool {
    let name = name.trim();
    WARNINGS.contains(&name) || LINT_RULES.contains(&name)
}

/// Every accepted name, warnings first.
pub fn all() -> impl Iterator<Item = &'static str> {
    WARNINGS.iter().copied().chain(LINT_RULES.iter().copied())
}

/// The closest accepted name, when one is close enough to be worth showing.
pub fn nearest(name: &str) -> Option<&'static str> {
    let name = name.trim();
    // A third of the length, so a short name tolerates one typo and a long rule
    // id tolerates a few — a fixed budget either misses `naming/or_sufix` or
    // suggests `naming/to` for `xyz`.
    let budget = (name.len() / 3).max(1);
    all()
        .map(|candidate| (edit_distance(name, candidate), candidate))
        .filter(|(d, _)| *d <= budget)
        .min_by_key(|(d, _)| *d)
        .map(|(_, candidate)| candidate)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut row = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            row[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(row[j] + 1);
        }
        std::mem::swap(&mut prev, &mut row);
    }
    prev[b.len()]
}

/// The name inside `allow(...)`, for an attribute stored verbatim.
pub fn allowed_name(attr: &str) -> Option<&str> {
    attr.strip_prefix("allow(")?.strip_suffix(')').map(str::trim)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typo_finds_its_name() {
        assert_eq!(nearest("torn_lock_updat"), Some("torn_lock_update"));
        assert_eq!(nearest("style/snake-case-fun"), Some("style/snake-case-func"));
    }

    #[test]
    fn nothing_close_suggests_nothing() {
        assert_eq!(nearest("completely_unrelated"), None);
    }

    #[test]
    fn names_are_unique() {
        let mut seen: Vec<&str> = all().collect();
        let before = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), before, "a name is listed twice");
    }

    #[test]
    fn the_attribute_text_parses() {
        assert_eq!(allowed_name("allow(naming/as)"), Some("naming/as"));
        assert_eq!(allowed_name("resource"), None);
    }
}
