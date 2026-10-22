# Ergonomics: Rask vs C, Go and Rust

`benchmarks/micro/` measures speed. This measures ceremony: METRICS.md's **ED**
(Ergonomic Delta, target ≤ 1.2) and the counts behind **SN** (Syntactic Noise).

One program, four languages: `examples/grep_clone.rk` against `grep.c`, `grep.go`
and `grep.rs`. Each is written the way that language's own users would write it,
formatted with its own formatter, standard library only.

## They do the same thing

All four were diffed on stdout and exit code across 12 invocations (bare, `-i`,
`-n`, `-c`, `-v`, `-c -i`, `-n -v` over two files, no match, `-h`, no args,
pattern only, missing file). Everything matches except the wording of the
missing-file error, which is each language's own I/O error text.

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
| LOC | **99** | 113 | 123 | 133 |
| max nesting | 4 | **3** | 4 | 5 |
| cleanup calls | 0 | 0 | 0 | 8 |
| NULL / sentinel checks | 0 | 0 | 0 | 6 |

**ED against Go = 99/113 = 0.88.** Against C, 0.74.

## What the numbers say

**Some of the line lead is formatting.** `rask fmt` keeps `if x { return y }` on one
line and gofmt and rustfmt don't. Seven such lines in the Rask file, so on equal
formatting Rask and Go are close to even. That's the honest reading: Rask is as
short as Go here, not shorter.

**The error type is where the three differ.** Go uses `errors.New` and declares
nothing. Rask declares a 9-line enum whose `@message` templates are the messages,
and `{e}` interpolates them. Rust declares the same enum and then writes the
messages again in a 9-line `Display` impl. In exchange, Rask and Rust callers can
match on which error it was and Go callers can't.

**Go nests one level shallower** because `switch` cases don't open a brace. The
`-h` arm is the only depth-4 block in the Rask file.

**What C spends its lines on** is the thing the other three don't have: 8 cleanup
calls for 3 resources, and adding a fifth early return means remembering a fifth
`free`.

The first version of this page measured an older `grep_clone.rk` that walked
arguments with an index `while` loop, built options with a seven-line literal and
nested four `if`s where `continue` does the job. It came out at 128 lines and
depth 6, a tie with C. The language could already do better; the example didn't.

## Reproducing

```
gcc -O2 -Wall -Wextra -o grep_c grep.c
go build -o grep_go grep.go
rustc -O -o grep_rs grep.rs
rask compile ../../examples/grep_clone.rk -o grep_rk
./grep_rk -n -i alpha ../../tests/fixtures/grep_input.txt
```
