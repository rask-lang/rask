# Ergonomics: Rask vs C, Go and Rust

`benchmarks/micro/` measures speed. This measures ceremony: METRICS.md's **ED**
(Ergonomic Delta, target ≤ 1.2) and the counts behind **SN** (Syntactic Noise).

One program, four languages: `examples/grep_clone.rk` against `grep.c`, `grep.go`
and `grep.rs`. Each is written the way that language's own users would write it,
formatted with its own formatter, standard library only. That last rule decides
argument parsing: Rask has `cli` and Go has `flag`, while Rust's standard library
has nothing (the usual answer is the `clap` crate), and neither does C's. So the
Rust and C versions parse arguments and print usage by hand.

## They do the same thing

All four were run on 11 invocations (bare, `-i`, `-n`, `-c`, `-v`, `-c -i`, `-n -v`
over two files, no match, missing file, no args, pattern only) and agree on every
matching line and every exit code: 0 on a match, 1 on none, 2 on an error. The
wording of error and help text differs, since two of them come from a library.

## Counting rules

- **LOC**: non-blank, non-comment. Brace-only lines count.
- **Nesting**: maximum brace depth anywhere in the file, string literals ignored.
- **C only**: `free`/`fclose` calls and NULL/sentinel checks. Counts calls, not
  sites, because each exit path needs its own.

`grep.c` checks every `fopen` and `malloc`, checks `ferror` after the read loop and
frees on every exit path. Sloppy C would score better here and be worse code.

## Results

| | Rask | Go | Rust | C |
|---|---:|---:|---:|---:|
| LOC | **67** | 75 | 123 | 133 |
| max nesting | **3** | **3** | 4 | 5 |
| cleanup calls | 0 | 0 | 0 | 8 |
| NULL / sentinel checks | 0 | 0 | 0 | 6 |

**ED against Go = 67/75 = 0.89.**

## What the numbers say

**Rask and Go are even.** `rask fmt` keeps `if x { return y }` on one line, gofmt
doesn't, and five such lines account for the gap. Written the same way they'd be
within two lines of each other.

**Rust and C pay for the missing argument parser.** Rust's error enum, its
`Display` impl, the flag loop and the usage text come to 60 lines. With `clap`
it would land near the other two.

**Rask's help is the more complete one.** `cli` prints every flag with its long
name and describes the positionals. Go's `flag` lists single-letter flags only and
leaves positionals to a hand-written usage function.

**What C spends its lines on** is the thing the other three don't have: 8 cleanup
calls for 3 resources, and adding a fifth early return means remembering a fifth
`free`.

An earlier `grep_clone.rk` walked argv with an index `while` loop, built options
with a field-by-field literal, wrote out a `message()` match and nested four `if`s
where `continue` does the job. It measured 128 lines and depth 6, a tie with C.
Nothing in the language forced that. The example just predated `cli`.

## Reproducing

```
gcc -O2 -Wall -Wextra -o grep_c grep.c
go build -o grep_go grep.go
rustc -O -o grep_rs grep.rs
rask compile ../../examples/grep_clone.rk -o grep_rk
./grep_rk -n -i alpha ../../tests/fixtures/grep_input.txt
```
