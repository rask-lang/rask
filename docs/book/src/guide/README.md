# Language Guide

The guide is being written, chapter by chapter. The
[specifications](../reference/specs-link.md) stay authoritative — the guide teaches the same rules
in the order you'd meet them, and says why each one is the way it is.

Every chapter's Rask is pulled straight out of a program in
[examples/](https://github.com/rask-lang/rask/blob/main/examples/) that CI runs on both backends,
and every compile error shown is a pinned rendering of real `rask check` output. `tests/book_gate.sh`
enforces both, so a chapter can't drift from the compiler — and improving a diagnostic shows up as a
diff on the page that teaches it.

## Chapters

- [Passing Values](passing-values.md) — borrow, `mutate`, `take`, and why only one of them needs a
  marker at the call site

The rules a chapter has to follow are in
[how this book is built](https://github.com/rask-lang/rask/blob/main/docs/book/README.md).

## Elsewhere

- [Examples](../examples/README.md) — complete programs, each one run by CI on both backends
- [Getting Started](../getting-started/README.md) — installation and first program
- [Specifications](../reference/specs-link.md) — the normative rules
