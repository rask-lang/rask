# Ergonomics: Rask vs C

`benchmarks/micro/` measures speed. This measures ceremony — METRICS.md's **ED**
(Ergonomic Delta, target ≤ 1.2) and the counts behind **SN** (Syntactic Noise).

Right now that's one pair: `grep.c` against `examples/grep_clone.rk`.

## The pair is verified equivalent

Before any counting, both implementations were diffed on stdout and exit code across
8 invocations (bare, `-i`, `-n`, `-c`, `-v`, `-c -i`, no-match, `-h`). All 8 match
exactly. The error path matches in shape and exit code; only the message wording
differs (Rask's `IoError` is chattier than `strerror`).

A comparison of two programs that don't do the same thing isn't worth anything, so
that check comes first.

## Counting rules

Stated so the numbers can be argued with:

- **LOC** — non-blank, non-comment. Brace-only lines count; so do `#include` and
  `#define`. Same rule both sides.
- **Nesting depth** — maximum brace depth reached anywhere in the file.
- **Cleanup calls** — `free` / `fclose`. Counts calls, not sites, because each exit
  path needs its own.
- **NULL / sentinel checks** — `== NULL`, `!= NULL`, `< 0)`, `ferror(`. C's way of
  asking "did that fail".
- **`catch` / `try` sites** — Rask's way of asking the same thing.
- **Type-name tokens** — declared type names written out by hand.

`grep.c` was written to be idiomatic and correctly error-handled, not golfed: every
`fopen` and `malloc` is checked, `ferror` is checked after the read loop, and every
allocation is freed on every exit path. It compiles clean under `-Wall -Wextra`.
Sloppy C would score better here and be worse code.

## Results

| metric | C | Rask |
|---|---:|---:|
| LOC (non-blank, non-comment) | 133 | 128 |
| max block nesting depth | 5 | **6** |
| free()/fclose() calls | **8** | 0 |
| NULL / sentinel checks | 6 | 0 |
| catch / try sites | 0 | 2 |
| type-name tokens written | 36 | 22 |

**ED = 128/133 = 0.96.** Under the 1.2 target, but the honest reading is that it's a
tie. For a program this size, in the domain C is best at, Rask is not shorter.

## What the numbers actually say

**The LOC tie is the finding.** I expected Rask to win on line count and it doesn't.
What differs isn't volume, it's what the lines are made of: C spends 8 calls and 6
checks on memory and failure bookkeeping, Rask spends 2 `catch` sites. Same length,
different content.

**Rask nests deeper (6 vs 5), and that's a real loss.** It comes from `grep_file`'s
`for` → `if show` → `if !count_only` → `if line_numbers` ladder, where C flattens the
same logic with an early `continue`. Worth revisiting; it's the one metric here that
went the wrong way.

**8 cleanup calls for 3 resources.** `positional` is freed on 4 different exit paths,
`line` and `f` on 2 each. None of those are hard individually; the failure mode is
that adding a fifth early return means remembering a fifth `free`. That's the class
`ensure` exists to delete, and the count is the size of the thing being deleted.

## Why only grep

The other examples don't give a clean comparison, and padding the table with bad
pairs would make the numbers worse, not better:

- `file_copy.rk` leans on `fs.copy` and `fs.metadata`. C has no equivalent, so the C
  version is a manual read/write loop — that measures stdlib breadth, not language
  ceremony.
- `http_api_server.rk` would need a hand-rolled socket stack in C.
- `game_loop.rk` uses `Pool`/`Handle`, which has no C counterpart at all — the
  interesting comparison there is concept count, not lines.

grep is the one program in the example set where both languages use only primitives
both have. A second honest pair needs a program picked for that property, not
whichever example is next in the directory.

## Reproducing

```
gcc -O2 -Wall -Wextra -o grep_c grep.c
./grep_c alpha ../../tests/fixtures/grep_input.txt
rask run ../../examples/grep_clone.rk -- alpha tests/fixtures/grep_input.txt
```
