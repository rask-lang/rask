# Contributing

Rask is one person's project so far, and the most useful thing a newcomer can
do is use it and say where it breaks. Bug reports are worth more than patches
right now. Patches are welcome too; this page is how to build it, test it and
find your way around.

## Filing a bug

[Open an issue](https://github.com/rask-lang/rask/issues/new) with:

- the smallest program that shows it, whole, so it can be pasted into a file;
- what you ran, and what it printed: the whole error, not a summary;
- what you expected instead;
- `rask --version`.

Run it both ways if you can, `rask run file.rk` and `rask run --interp
file.rk`, and say whether they agree. `rask run` compiles to machine code and
`--interp` runs the interpreter; when the two disagree, the interpreter's answer
is the one the language means, and that's most of what the bug is.

A confusing error message is a bug too, even when the program really was
wrong. So is the book or the tour saying something the compiler doesn't do.

## Build from source

You need Rust ([rustup.rs](https://rustup.rs)), git and a C compiler (`cc`;
`rask` links the programs it builds with it).

```bash
git clone https://github.com/rask-lang/rask
cd rask/compiler
cargo build --release -p rask-cli
```

The binary is `compiler/target/release/rask`, and every script in `tests/`
uses that one. The C runtime and the standard library are built into it, so a
copy of the file works anywhere. After editing `stdlib/*.rk`, rebuild:
it's compiled into the binary too.

## Find your way around

| Where | What |
|---|---|
| `compiler/` | The compiler, in Rust. [COMPILER.md](compiler/COMPILER.md) walks through it; [compiler/CLAUDE.md](compiler/CLAUDE.md) maps tasks to files |
| `compiler/runtime/` | The C runtime that compiled programs link with |
| `stdlib/` | The standard library, written in Rask |
| `specs/` | What the language is. [CORE_DESIGN.md](specs/CORE_DESIGN.md) first, [RULINGS.md](specs/RULINGS.md) for how questions get decided |
| `tests/suite/` | Rask files full of `test` blocks, run on both backends |
| `examples/` | Whole programs, each compiled and run by CI |
| `docs/book/` | The book at [rask-lang.dev/book](https://rask-lang.dev/book/), including the tour in `src/learn/` |

## Run the tests

From the repository root, with the compiler built:

```bash
tests/differential.sh       # every tests/suite file on both backends
(cd compiler && cargo test --release)   # the Rust tests: fixtures, error messages
compiler/target/release/rask test tests/suite/p07_heap.rk   # one file; --interp for the other backend
```

`tests/differential.sh` is the one that matters most. A feature works when
both backends agree on it, and this is what checks that. Files that are known
to fail are listed in `tests/known_divergences.txt` with the issue for each.

CI runs two dozen more gates (`tests/*_gate.sh`), each with a comment at the
top saying what it catches. The ones a change is most likely to trip:

| Change | Run |
|---|---|
| anything in the compiler or runtime | `tests/differential.sh`, `tests/examples_gate.sh`, `tests/leak_gate.sh` |
| the book | `tests/book_gate.sh` |
| the tour | `tests/tour_gate.sh` |
| the standard library | `tests/stdlib_coverage_gate.sh` too |

[docs/TESTING.md](docs/TESTING.md) has the full map and what each layer
proves.

## Sending a change

- Every source file (`.rs`, `.rk`, `.sh`) starts with
  `// SPDX-License-Identifier: (MIT OR Apache-2.0)` (or `#` for shell).
- A bug fix comes with a test that fails without it, usually a file in
  `tests/suite/`, and passes on both backends.
- A change to what the language *is* starts as an issue or a spec edit in
  `specs/`, not in the compiler. [RULINGS.md](specs/RULINGS.md) is what a
  proposal gets weighed against.
- Code a reader is meant to copy (`examples/`, `stdlib/`, the book) is clean
  under `rask lint`; `tests/lint_gate.sh` checks.

Contributions are under the same terms as the project: MIT or Apache 2.0, at
the user's choice.
