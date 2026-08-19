// Generates compiler/runtime/unicode_case.c from Rust's own Unicode data:
// the case mappings, and the character classes `char.is_alphabetic()` and
// friends answer from.
//
// Native case conversion was ASCII-only, and the classifications were worse —
// `is_alphabetic` said yes to every scalar above 127, so `'€'` and a combining
// accent were letters. The interpreter is the reference and it is Rust's std,
// so the tables come from the same source it uses.
use std::fmt::Write as _;

fn main() {
    let mut simple_up: Vec<(u32, u32)> = Vec::new();
    let mut simple_lo: Vec<(u32, u32)> = Vec::new();
    let mut multi_up: Vec<(u32, Vec<u32>)> = Vec::new();
    let mut multi_lo: Vec<(u32, Vec<u32>)> = Vec::new();
    // Worst-case growth in *bytes* for one scalar, so the C side can size its
    // buffer from a justified constant rather than a guess.
    let mut max_ratio = 1.0f64;

    // Character classes, as sorted inclusive ranges. Same walk, same source.
    let classes: [(&str, fn(char) -> bool); 5] = [
        ("ALPHABETIC", |c| c.is_alphabetic()),
        ("NUMERIC", |c| c.is_numeric()),
        ("LOWERCASE", |c| c.is_lowercase()),
        ("UPPERCASE", |c| c.is_uppercase()),
        ("CONTROL", |c| c.is_control()),
    ];
    let mut class_ranges: Vec<Vec<(u32, u32)>> = vec![Vec::new(); classes.len()];

    for cp in 0u32..=0x10FFFF {
        let Some(c) = char::from_u32(cp) else { continue };
        for (i, (_, test)) in classes.iter().enumerate() {
            if test(c) {
                match class_ranges[i].last_mut() {
                    Some(last) if last.1 + 1 == cp => last.1 = cp,
                    _ => class_ranges[i].push((cp, cp)),
                }
            }
        }
        let src_len = c.len_utf8();
        let up: Vec<char> = c.to_uppercase().collect();
        let lo: Vec<char> = c.to_lowercase().collect();
        for mapped in [&up, &lo] {
            let out_len: usize = mapped.iter().map(|m| m.len_utf8()).sum();
            max_ratio = max_ratio.max(out_len as f64 / src_len as f64);
        }
        if up.len() == 1 {
            if up[0] != c {
                simple_up.push((cp, up[0] as u32));
            }
        } else {
            multi_up.push((cp, up.iter().map(|c| *c as u32).collect()));
        }
        if lo.len() == 1 {
            if lo[0] != c {
                simple_lo.push((cp, lo[0] as u32));
            }
        } else {
            multi_lo.push((cp, lo.iter().map(|c| *c as u32).collect()));
        }
    }

    let mut out = String::new();
    out.push_str("// SPDX-License-Identifier: (MIT OR Apache-2.0)\n");
    out.push_str("//\n");
    out.push_str("// GENERATED — do not edit by hand. Unicode simple and special case\n");
    out.push_str("// mappings, taken from Rust's `char::to_uppercase`/`to_lowercase` so the\n");
    out.push_str("// native runtime answers what the interpreter does (the interpreter is the\n");
    out.push_str("// reference, and it is Rust's std). Regenerate with scripts/gen_unicode_case.\n");
    out.push_str("//\n");
    out.push_str("// Native case conversion used to be ASCII-only: `\"a\\u{f6}b\".to_uppercase()`\n");
    out.push_str("// was `A\\u{f6}B` and Greek was left alone entirely (#779).\n\n");
    out.push_str("#include \"rask_runtime.h\"\n\n");
    writeln!(
        out,
        "// The most a single scalar can grow, in bytes, under either mapping.\n\
         const int RASK_CASE_MAX_GROWTH = {};\n",
        max_ratio.ceil() as i32
    )
    .unwrap();

    let emit_simple = |out: &mut String, name: &str, v: &[(u32, u32)]| {
        writeln!(out, "static const RaskCaseSimple {}[] = {{", name).unwrap();
        for chunk in v.chunks(4) {
            out.push_str("   ");
            for (from, to) in chunk {
                write!(out, " {{0x{:04X},0x{:04X}}},", from, to).unwrap();
            }
            out.push('\n');
        }
        writeln!(out, "}};").unwrap();
        writeln!(
            out,
            "static const int {}_LEN = {};\n",
            name,
            v.len()
        )
        .unwrap();
    };
    let emit_multi = |out: &mut String, name: &str, v: &[(u32, Vec<u32>)]| {
        writeln!(out, "static const RaskCaseMulti {}[] = {{", name).unwrap();
        for (from, tos) in v {
            let mut slots = [0u32; 3];
            for (i, t) in tos.iter().enumerate() {
                slots[i] = *t;
            }
            writeln!(
                out,
                "    {{0x{:04X}, {}, {{0x{:04X},0x{:04X},0x{:04X}}}}},",
                from, tos.len(), slots[0], slots[1], slots[2]
            )
            .unwrap();
        }
        writeln!(out, "}};").unwrap();
        writeln!(out, "static const int {}_LEN = {};\n", name, v.len()).unwrap();
    };

    let emit_ranges = |out: &mut String, name: &str, v: &[(u32, u32)]| {
        writeln!(out, "static const RaskCharRange RASK_{}[] = {{", name).unwrap();
        for chunk in v.chunks(4) {
            out.push_str("   ");
            for (lo, hi) in chunk {
                write!(out, " {{0x{:04X},0x{:04X}}},", lo, hi).unwrap();
            }
            out.push('\n');
        }
        writeln!(out, "}};").unwrap();
        writeln!(out, "static const int RASK_{}_LEN = {};\n", name, v.len()).unwrap();
    };
    for (i, (name, _)) in classes.iter().enumerate() {
        emit_ranges(&mut out, name, &class_ranges[i]);
    }

    emit_simple(&mut out, "RASK_UPPER_SIMPLE", &simple_up);
    emit_simple(&mut out, "RASK_LOWER_SIMPLE", &simple_lo);
    emit_multi(&mut out, "RASK_UPPER_MULTI", &multi_up);
    emit_multi(&mut out, "RASK_LOWER_MULTI", &multi_lo);

    out.push_str(r#"
static int case_simple_lookup(const RaskCaseSimple *tbl, int len, uint32_t cp,
                              uint32_t *out) {
    int lo = 0, hi = len - 1;
    while (lo <= hi) {
        int mid = lo + (hi - lo) / 2;
        if (tbl[mid].from == cp) { *out = tbl[mid].to; return 1; }
        if (tbl[mid].from < cp) { lo = mid + 1; } else { hi = mid - 1; }
    }
    return 0;
}

static int case_multi_lookup(const RaskCaseMulti *tbl, int len, uint32_t cp,
                             uint32_t out[3]) {
    int lo = 0, hi = len - 1;
    while (lo <= hi) {
        int mid = lo + (hi - lo) / 2;
        if (tbl[mid].from == cp) {
            for (int i = 0; i < tbl[mid].n; i++) { out[i] = tbl[mid].to[i]; }
            return tbl[mid].n;
        }
        if (tbl[mid].from < cp) { lo = mid + 1; } else { hi = mid - 1; }
    }
    return 0;
}

// How `cp` maps, writing up to three scalars into `out`. Returns the count,
// always at least 1 — a scalar with no mapping maps to itself.
int rask_case_map(uint32_t cp, int to_upper, uint32_t out[3]) {
    const RaskCaseMulti *multi = to_upper ? RASK_UPPER_MULTI : RASK_LOWER_MULTI;
    int multi_len = to_upper ? RASK_UPPER_MULTI_LEN : RASK_LOWER_MULTI_LEN;
    int n = case_multi_lookup(multi, multi_len, cp, out);
    if (n > 0) { return n; }
    const RaskCaseSimple *simple = to_upper ? RASK_UPPER_SIMPLE : RASK_LOWER_SIMPLE;
    int simple_len = to_upper ? RASK_UPPER_SIMPLE_LEN : RASK_LOWER_SIMPLE_LEN;
    uint32_t mapped;
    if (case_simple_lookup(simple, simple_len, cp, &mapped)) {
        out[0] = mapped;
        return 1;
    }
    out[0] = cp;
    return 1;
}

// The single-scalar answer, for `char.to_uppercase()` / `to_lowercase()`.
//
// A char holds one scalar, so a mapping that produces several has to pick one.
// The interpreter takes the first — `'\u{df}'.to_uppercase()` is `S` there — so
// this does the same.
uint32_t rask_case_map_one(uint32_t cp, int to_upper) {
    uint32_t out[3];
    rask_case_map(cp, to_upper, out);
    return out[0];
}

static int range_contains(const RaskCharRange *tbl, int len, uint32_t cp) {
    int lo = 0, hi = len - 1;
    while (lo <= hi) {
        int mid = lo + (hi - lo) / 2;
        if (cp < tbl[mid].lo) { hi = mid - 1; }
        else if (cp > tbl[mid].hi) { lo = mid + 1; }
        else { return 1; }
    }
    return 0;
}

int rask_char_class(uint32_t cp, int which) {
    switch (which) {
        case RASK_CLASS_ALPHABETIC:
            return range_contains(RASK_ALPHABETIC, RASK_ALPHABETIC_LEN, cp);
        case RASK_CLASS_NUMERIC:
            return range_contains(RASK_NUMERIC, RASK_NUMERIC_LEN, cp);
        case RASK_CLASS_LOWERCASE:
            return range_contains(RASK_LOWERCASE, RASK_LOWERCASE_LEN, cp);
        case RASK_CLASS_UPPERCASE:
            return range_contains(RASK_UPPERCASE, RASK_UPPERCASE_LEN, cp);
        case RASK_CLASS_CONTROL:
            return range_contains(RASK_CONTROL, RASK_CONTROL_LEN, cp);
        default:
            return 0;
    }
}
"#);

    std::fs::write("compiler/runtime/unicode_case.c", out).unwrap();
    eprintln!(
        "wrote compiler/runtime/unicode_case.c: {} simple up, {} simple lo, {} multi up, {} multi lo, max growth {}x",
        simple_up.len(),
        simple_lo.len(),
        multi_up.len(),
        multi_lo.len(),
        max_ratio.ceil() as i32
    );
    for (i, (name, _)) in classes.iter().enumerate() {
        eprintln!("  {}: {} ranges", name, class_ranges[i].len());
    }
}
