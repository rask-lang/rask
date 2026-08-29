# gen_unicode_text

Generates `compiler/runtime/unicode_text.c` — the display width, grapheme cluster
breaking and NFC data the native runtime needs for the text-unit operations in
`std.strings` (U1–U5): `width()`, `graphemes()`, `truncate()`, `reverse()` and
`normalized()`.

These tables were written by hand twice — once in `string.c`, once in the
interpreter — with a comment asking a human to keep the two copies in step. That
is rot with a delay fuse. The interpreter is the reference and it runs on
`unicode-normalization`, `unicode-segmentation` and `unicode-width`, so the C side
is generated from those same crates and there is nothing left for the two backends
to disagree about.

Run from the repository root:

```
cargo run --release --manifest-path scripts/gen_unicode_text/Cargo.toml
```

It writes `compiler/runtime/unicode_text.c` and prints the counts. Rebuild the
runtime afterwards (`cd compiler/runtime && make`) plus the compiler, since the
runtime is a static lib.

## What gets generated, and what doesn't

Tabulated: wide and zero-width ranges, the grapheme properties that join left or
right, canonical combining classes, canonical decompositions, and the pairs that
recompose.

By rule in C, not tabulated: Hangul composition and decomposition (algorithmic in
UAX #15), regional-indicator pairing, CRLF, and ZWJ sequences. A ZWJ cluster is
two columns rather than the sum of its parts — summing them made a family emoji
measure six columns, which is what `tests/suite/t_text_units.rk` now pins.

Composition exclusions need no separate table: a pair is only emitted if NFC
actually puts it back together, so the generator asks rather than transcribes.

## Checking it

`tests/suite/t_text_units.rk` fuzzes against Python's `unicodedata` — an
implementation sharing no code with either backend. The pool is drawn from every
codepoint whose NFC and NFD differ, plus the combining marks: an earlier version
hand-picked a couple of dozen characters and missed a real bug, because none of
them needed two marks to recompose. Regenerate when the crates'
Unicode version moves, and run that test: if the generator and the reference
disagree, it fails.
