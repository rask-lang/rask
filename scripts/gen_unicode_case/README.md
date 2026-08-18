# gen_unicode_case

Generates `compiler/runtime/unicode_case.c` — the Unicode case-mapping tables the
native runtime uses for `string.to_uppercase()`, `string.to_lowercase()`,
`char.to_uppercase()` and `char.to_lowercase()`.

Native case conversion was ASCII-only, so `"aöb".to_uppercase()` came back `AöB`
and Greek was left untouched entirely, while the interpreter answered `AÖB` and
`αβγ` (#779). The interpreter is the reference and it uses Rust's `std`, so the
tables are generated from that same source rather than transcribed — there's
nothing for the two backends to disagree about.

Run from the repository root:

```
cargo run --release --manifest-path scripts/gen_unicode_case/Cargo.toml
```

It writes `compiler/runtime/unicode_case.c` and prints the counts. Rebuild the
runtime afterwards (`cd compiler/runtime && make`) plus the compiler, since the
runtime is a static lib.

Regenerate when the toolchain's Unicode version moves. The output is checked in so
a build needs no extra step, and the counts in the commit message are the record
of what changed.
