# Learn Rask

You know Python. You can crunch numbers, plot data, run simulations.
But you've never touched the kind of language where the compiler argues back.

This tutorial teaches you Rask by making you build things — physics sims,
flight dynamics, train networks. Each lesson is a `.rk` file with puzzles.
Solve them, run the tests, feel the dopamine. The compiler will catch your
mistakes before you even run the code. That's the whole point of Rask.

## Setup

See [00_setup.md](00_setup.md) to get running. Takes about 5 minutes.

## How it works

1. Open a lesson file (e.g. `03_functions.rk`) in VSCode
2. Read the comments at the top — they explain each concept with Python comparisons
3. Study the examples
4. Fill in the puzzle functions (they start as `panic("TODO")`)
5. Run `rask test 03_functions.rk` to check your answers
6. All tests green? Next lesson.

Some puzzles ask you to *break things on purpose* — try something the compiler
won't allow, read the error, and understand why. Those are the most important ones.

## Quick cheat sheet: Python → Rask

| Python | Rask | What changed |
|--------|------|-------|
| `x = 42` | `const x = 42` | Immutable by default |
| `x = 0; x = 1` | `let x = 0; x = 1` | `let` = mutable |
| `def f(a, b):` | `func f(a: i32, b: i32) -> i32 {` | You declare types |
| `return x` | `return x` | Same, but always required |
| `[1, 2, 3]` | `Vec.from([1, 2, 3])` | Vec = Python's list |
| `{"a": 1}` | Map + `.insert()` | No dict literals (yet) |
| `for x in range(10):` | `for x in 0..10 {` | `..` = range |
| `if x > 0:` | `if x > 0 {` | Braces instead of colons |
| `class Foo:` | `struct Foo {}` + `extend Foo {}` | Data and methods are separate |
| `try/except` | `try expr` / `match result { Ok/Err }` | No exceptions — errors are values |
| `# comment` | `// comment` | |
| `print(x)` | `println(x)` | |

## Lessons

| # | File | What you'll build |
|---|------|-------|
| 00 | [00_setup.md](00_setup.md) | Get Rask and VSCode running |
| 01 | [01_hello.rk](01_hello.rk) | Your first program — printing and strings |
| 02 | [02_variables.rk](02_variables.rk) | Variables, types, and physical constants |
| 03 | [03_functions.rk](03_functions.rk) | Functions — kinetic energy, orbital mechanics |
| 04 | [04_collections.rk](04_collections.rk) | Lists and maps — particle data, periodic table |
| 05 | [05_control_flow.rk](05_control_flow.rk) | Loops — train schedules, city grids |
| 06 | [06_pattern_matching.rk](06_pattern_matching.rk) | Pattern matching — signal routing, unit conversion |
| 07 | [07_structs.rk](07_structs.rk) | Structs — 3D vectors, projectile simulation |
| 08 | [08_error_handling.rk](08_error_handling.rk) | Error handling — sensor validation, flight data |
| 09 | [09_ownership.rk](09_ownership.rk) | Ownership — why Rask doesn't need a garbage collector |
| 10 | [10_closures.rk](10_closures.rk) | Closures — numerical integration, autopilot filters |
| 11 | [11_generics.rk](11_generics.rk) | Generics — write once, use with any type |
| 12 | [12_traits.rk](12_traits.rk) | Traits — aircraft interfaces, polymorphism |
| 13 | [13_enums_advanced.rk](13_enums_advanced.rk) | Rich enums — flight logs, airport status |
| 14 | [14_strings.rk](14_strings.rk) | Strings — METAR parsing, flight plans, ATC |
| 15 | [15_concurrency.rk](15_concurrency.rk) | Concurrency — parallel physics, Monte Carlo |
| 16 | [16_shared_state.rk](16_shared_state.rk) | Shared data — flight tracker, thread-safe counters |
| 17 | [17_resource_types.rk](17_resource_types.rk) | Resource cleanup — files, transactions |
| 18 | [18_comptime.rk](18_comptime.rk) | Compile-time computation — lookup tables, atmosphere model |
| 19 | [19_final_project.rk](19_final_project.rk) | Final project — flight dynamics simulator |

## Solutions

Stuck? Solutions are in [solutions/](solutions/). But try for at least 10 minutes first —
the struggle is where the learning happens.

## Running tests

```bash
rask test 05_control_flow.rk              # all tests in a lesson
rask test 05_control_flow.rk -f "train"   # only tests matching "train"
```
