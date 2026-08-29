// Generates compiler/runtime/unicode_text.c — the tables the native runtime
// needs for the text-unit operations in `std.strings` (U1–U5): display width,
// grapheme cluster breaking, and NFC normalization.
//
// These were hand-written twice, once in string.c and once in the interpreter,
// with a comment asking a human to keep the two copies in step. That is rot with
// a delay fuse. The interpreter is the reference and it runs on Rust's crates,
// so the C side is generated from those same crates and there is nothing left
// for the two backends to disagree about.
use std::collections::BTreeMap;
use std::fmt::Write as _;

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::canonical_combining_class;
use unicode_width::UnicodeWidthChar;

/// Collapse a sorted scalar list into inclusive ranges.
fn ranges_of(mut probe: impl FnMut(char) -> bool) -> Vec<(u32, u32)> {
    let mut out: Vec<(u32, u32)> = Vec::new();
    for cp in 0u32..=0x10FFFF {
        let Some(c) = char::from_u32(cp) else { continue };
        if probe(c) {
            match out.last_mut() {
                Some(last) if last.1 + 1 == cp => last.1 = cp,
                _ => out.push((cp, cp)),
            }
        }
    }
    out
}

fn emit_ranges(out: &mut String, name: &str, ranges: &[(u32, u32)]) {
    writeln!(out, "static const RaskCharRange {name}[] = {{").unwrap();
    for chunk in ranges.chunks(4) {
        let line: Vec<String> = chunk
            .iter()
            .map(|(lo, hi)| format!("{{0x{lo:04X},0x{hi:04X}}}"))
            .collect();
        writeln!(out, "    {},", line.join(", ")).unwrap();
    }
    writeln!(out, "}};").unwrap();
    writeln!(out, "#define {name}_LEN {}\n", ranges.len()).unwrap();
}

fn main() {
    // ── Width ──────────────────────────────────────────────────────────
    // Two ranges is all the C side needs: what renders double-width, and what
    // renders in no columns at all. Everything else is one column.
    let wide = ranges_of(|c| c.width().unwrap_or(1) == 2);
    let zero = ranges_of(|c| c.width() == Some(0));

    // ── Grapheme cluster breaking ──────────────────────────────────────
    // Rather than transcribe UAX #29's property table and rule set into C, ask
    // the segmenter where the breaks fall and record the *properties* it needs.
    // Extend|ZWJ|SpacingMark join what precedes them; Prepend joins what
    // follows; the rest is regional-indicator pairing and Hangul, which the C
    // side handles by rule.
    let joins_left = ranges_of(|c| {
        // A scalar that never starts a new cluster after a plain letter.
        let s: String = ['a', c].iter().collect();
        unicode_segmentation::UnicodeSegmentation::graphemes(s.as_str(), true).count() == 1
    });
    let prepend = ranges_of(|c| {
        let s: String = [c, 'a'].iter().collect();
        unicode_segmentation::UnicodeSegmentation::graphemes(s.as_str(), true).count() == 1
            && {
                // Distinguish a real Prepend from a scalar that simply joins
                // leftward (which the previous table already covers).
                let t: String = ['a', c].iter().collect();
                unicode_segmentation::UnicodeSegmentation::graphemes(t.as_str(), true).count() == 2
            }
    });

    // ── NFC ────────────────────────────────────────────────────────────
    // Canonical decomposition, the combining classes canonical ordering needs,
    // and the pairs that recompose. Composition exclusions fall out for free:
    // a pair is only listed if NFC actually puts it back together.
    //
    // The pairs have to come from walking the decomposition left to right, not
    // from "does this character's fully-unfolded form have exactly 2 parts".
    // `nfd()` is already recursive, so a character needing two marks — every
    // Vietnamese tone vowel, much of polytonic Greek — unfolds to 3 parts and
    // that test skipped it entirely. The intermediate pair was never recorded,
    // so the runtime composed e+dot-below into ẹ and then had nothing for
    // ẹ+circumflex, leaving ệ as two codepoints where the interpreter had one.
    let mut decomp: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let mut ccc: Vec<(u32, u8)> = Vec::new();
    let mut compose: Vec<(u32, u32, u32)> = Vec::new();

    for cp in 0u32..=0x10FFFF {
        let Some(c) = char::from_u32(cp) else { continue };

        let k = canonical_combining_class(c);
        if k != 0 {
            ccc.push((cp, k));
        }

        let d: Vec<u32> = c.nfd().map(|x| x as u32).collect();
        if d.len() != 1 || d[0] != cp {
            decomp.insert(cp, d.clone());
            // Hangul is algorithmic on both sides; no need to store it.
            if (0xAC00..=0xD7A3).contains(&cp) {
                decomp.remove(&cp);
                continue;
            }
            // Walk the marks, composing as far as each step allows. Every
            // primary composite shows up as one of these steps, including the
            // intermediates that never appear as anyone's full decomposition.
            let mut cur = d[0];
            for &mark in &d[1..] {
                let pair: String = [
                    char::from_u32(cur).unwrap(),
                    char::from_u32(mark).unwrap(),
                ]
                .iter()
                .collect();
                let back: Vec<char> = pair.nfc().collect();
                if back.len() == 1 {
                    compose.push((cur, mark, back[0] as u32));
                    cur = back[0] as u32;
                }
            }
        }
    }
    compose.sort_unstable();
    compose.dedup();

    // ── Emit ───────────────────────────────────────────────────────────
    let mut out = String::new();
    out.push_str(
        "// SPDX-License-Identifier: (MIT OR Apache-2.0)\n\
         //\n\
         // GENERATED — do not edit by hand. Display width, grapheme cluster\n\
         // breaking and NFC data for std.strings U1-U5, taken from the same\n\
         // crates the interpreter uses so the two backends cannot drift.\n\
         // Regenerate with scripts/gen_unicode_text.\n\
         \n\
         #include \"rask_runtime.h\"\n\
         #include <string.h>\n\n",
    );

    emit_ranges(&mut out, "RASK_WIDE", &wide);
    emit_ranges(&mut out, "RASK_ZERO_WIDTH", &zero);
    emit_ranges(&mut out, "RASK_GRAPHEME_JOIN_LEFT", &joins_left);
    emit_ranges(&mut out, "RASK_GRAPHEME_PREPEND", &prepend);

    // Combining classes, as (scalar, class) pairs.
    writeln!(out, "typedef struct {{ uint32_t cp; uint8_t ccc; }} RaskCcc;").unwrap();
    writeln!(out, "static const RaskCcc RASK_CCC[] = {{").unwrap();
    for chunk in ccc.chunks(6) {
        let line: Vec<String> = chunk
            .iter()
            .map(|(cp, k)| format!("{{0x{cp:04X},{k}}}"))
            .collect();
        writeln!(out, "    {},", line.join(", ")).unwrap();
    }
    writeln!(out, "}};").unwrap();
    writeln!(out, "#define RASK_CCC_LEN {}\n", ccc.len()).unwrap();

    // Canonical decomposition, flattened: an index table into one scalar pool
    // keeps this a pair of flat arrays rather than a pointer per entry.
    let mut pool: Vec<u32> = Vec::new();
    writeln!(
        out,
        "typedef struct {{ uint32_t cp; uint32_t off; uint32_t len; }} RaskDecomp;"
    )
    .unwrap();
    writeln!(out, "static const RaskDecomp RASK_DECOMP[] = {{").unwrap();
    for (cp, d) in &decomp {
        let off = pool.len();
        pool.extend(d.iter().copied());
        writeln!(
            out,
            "    {{0x{cp:04X},{off},{}}},",
            d.len()
        )
        .unwrap();
    }
    writeln!(out, "}};").unwrap();
    writeln!(out, "#define RASK_DECOMP_LEN {}\n", decomp.len()).unwrap();

    writeln!(out, "static const uint32_t RASK_DECOMP_POOL[] = {{").unwrap();
    for chunk in pool.chunks(8) {
        let line: Vec<String> = chunk.iter().map(|c| format!("0x{c:04X}")).collect();
        writeln!(out, "    {},", line.join(",")).unwrap();
    }
    writeln!(out, "}};\n").unwrap();

    writeln!(
        out,
        "typedef struct {{ uint32_t a; uint32_t b; uint32_t c; }} RaskCompose;"
    )
    .unwrap();
    writeln!(out, "static const RaskCompose RASK_COMPOSE[] = {{").unwrap();
    for chunk in compose.chunks(4) {
        let line: Vec<String> = chunk
            .iter()
            .map(|(a, b, c)| format!("{{0x{a:04X},0x{b:04X},0x{c:04X}}}"))
            .collect();
        writeln!(out, "    {},", line.join(", ")).unwrap();
    }
    writeln!(out, "}};").unwrap();
    writeln!(out, "#define RASK_COMPOSE_LEN {}\n", compose.len()).unwrap();

    out.push_str(include_str!("runtime_tail.c"));

    std::fs::write("compiler/runtime/unicode_text.c", &out).unwrap();
    eprintln!(
        "wrote compiler/runtime/unicode_text.c: {} wide ranges, {} zero-width ranges, \
         {} grapheme-join ranges, {} prepend ranges, {} ccc, {} decompositions \
         ({} scalars), {} compositions",
        wide.len(),
        zero.len(),
        joins_left.len(),
        prepend.len(),
        ccc.len(),
        decomp.len(),
        pool.len(),
        compose.len()
    );
}
