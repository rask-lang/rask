// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Suggestion helpers for actionable error messages.
//!
//! Provides did-you-mean, type conversion hints, and ownership fix suggestions.

/// Compute edit distance (Levenshtein) between two strings.
fn edit_distance(a: &str, b: &str) -> usize {
    let a_len = a.len();
    let b_len = b.len();

    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    let mut prev: Vec<usize> = (0..=b_len).collect();
    let mut curr = vec![0; b_len + 1];

    for (i, a_ch) in a.chars().enumerate() {
        curr[0] = i + 1;
        for (j, b_ch) in b.chars().enumerate() {
            let cost = if a_ch == b_ch { 0 } else { 1 };
            curr[j + 1] = (prev[j + 1] + 1)
                .min(curr[j] + 1)
                .min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[b_len]
}

/// Find the best match for `name` among `candidates`.
///
/// Returns `Some("did you mean `closest`?")` if a close match is found.
pub fn did_you_mean<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let max_distance = match name.len() {
        0..=2 => 1,
        3..=5 => 2,
        _ => 3,
    };

    let mut best: Option<(&str, usize)> = None;

    for candidate in candidates {
        // Quick length check to avoid computing distance for very different strings
        let len_diff = name.len().abs_diff(candidate.len());
        if len_diff > max_distance {
            continue;
        }

        let dist = edit_distance(name, candidate);
        if dist <= max_distance {
            if best.is_none() || dist < best.unwrap().1 {
                best = Some((candidate, dist));
            }
        }
    }

    best.map(|(closest, _)| format!("did you mean `{}`?", closest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_did_you_mean() {
        let candidates = ["counter", "count", "name", "value"];

        assert_eq!(
            did_you_mean("conter", candidates.iter().copied()),
            Some("did you mean `counter`?".to_string())
        );
        assert_eq!(
            did_you_mean("cout", candidates.iter().copied()),
            Some("did you mean `count`?".to_string())
        );
        assert_eq!(
            did_you_mean("xyz", candidates.iter().copied()),
            None
        );
    }

    #[test]
    fn test_edit_distance() {
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("", "hello"), 5);
        assert_eq!(edit_distance("abc", "abc"), 0);
        assert_eq!(edit_distance("abc", "abd"), 1);
    }
}
