# Rask Examples

Learning examples demonstrating Rask language features, from basics to advanced.

## Quick Start

Start with `hello_world.rk`, then work through the numbered examples in order.

---

## Learning Path (01-19)

### ✅ Implemented & Runnable

These examples parse and run with the current interpreter:

| # | Example | Topic | Status |
|---|---------|-------|--------|
| - | `hello_world.rk` | Hello World | ✅ Runnable |
| 01 | `01_variables.rk` | Variables (`const`, `let`) | ✅ Runnable |
| 02 | `02_functions.rk` | Functions and returns | ✅ Runnable |
| 03 | `03_collections.rk` | Vec and Map | ✅ Runnable |
| 04 | `04_pattern_matching.rk` | Match expressions | ✅ Runnable |
| 05 | `05_loops.rk` | For and while loops | ✅ Runnable |
| 06 | `06_structs.rk` | Structs and methods | ✅ Runnable |
| 07 | `07_error_handling.rk` | Result types, `try` | ✅ Runnable |
| 08 | `08_traits.rk` | Traits and polymorphism | ✅ Runnable |
| 13 | `13_string_operations.rk` | String methods | ✅ Runnable |
| 16 | `16_concurrency_basics.rk` | Threads and channels | ✅ Runnable |
| 17 | `17_comptime.rk` | Compile-time execution | ✅ Runnable |
| 18 | `18_resource_types.rk` | Linear resources, `@resource` | ✅ Runnable |

### 📝 Spec Examples (Not Yet Fully Implemented)

These examples demonstrate **intended syntax** but require features not yet in the parser/interpreter:

| # | Example | Topic | Missing Features |
|---|---------|-------|------------------|
| 09 | `09_generics.rk` | Generic types | Generic `extend`, full closure types |
| 10 | `10_enums_advanced.rk` | Enum variants with data | Advanced pattern matching |
| 11 | `11_closures.rk` | Closures and lambdas | Full closure syntax, higher-order functions |
| 12 | `12_iterators.rk` | Iterator patterns | Iterator trait, method chaining |
| 14 | `14_borrowing_patterns.rk` | Borrowing rules | Borrow checker, reference types |
| 15 | `15_memory_management.rk` | Pool/Handle pattern | Generic pools, handle types |
| 19 | `19_unsafe.rk` | Unsafe blocks, FFI | Pointer dereference, `extern "C"` |

**Note:** These are valuable learning resources showing Rask's design goals. They will become runnable as the compiler matures.

---

## Validation Programs

Real-world programs demonstrating Rask's capabilities:

| Program | Description | Status |
|---------|-------------|--------|
| `http_api_server.rk` | HTTP JSON API server | ✅ Reference implementation |
| `grep_clone.rk` | Command-line grep tool | ✅ Runnable |
| `text_editor.rk` | Text editor with undo | ✅ Runnable |
| `game_loop.rk` | Game loop with entities | ✅ Runnable |
| `sensor_processor.rk` | Embedded sensor processor | ✅ Runnable |

These represent Rask's 5 validation use cases from the design spec.

---

## Utility Examples

| File | Purpose |
|------|---------|
| `simple_grep.rk` | Simplified grep |
| `file_copy.rk` | File I/O example |
| `cli_calculator.rk` | CLI tool pattern |
| `simple_test.rk` | Testing basics |
| `collections_test.rk` | Collection operations |
| `pool_test.rk` | Pool/Handle testing |

---

## Compile Error Examples

Located in `compile_errors/` - these demonstrate what the type system **prevents**:

| File | Shows |
|------|-------|
| `borrow_stored.rk` | Cannot store borrowed references |
| `comptime_loop.rk` | Loop variable in comptime |
| `context_*.rk` | Context clause errors |
| `error_mismatch.rk` | Error type mismatches |
| `resource_leak.rk` | Linear resource not consumed |

These are **intentionally broken** to teach safety boundaries.

---

## Running Examples

### Parse (syntax check):
```bash
./target/debug/rask parse examples/01_variables.rk
```

### Run (execute):
```bash
./target/debug/rask run examples/01_variables.rk
```

### Type check:
```bash
./target/debug/rask check examples/01_variables.rk
```

---

## Learning Progression

**Beginners** (01-07):
- Start with `hello_world.rk`
- Work through 01-07 sequentially
- Master variables, functions, collections, control flow, error handling

**Intermediate** (08-14):
- Traits for polymorphism (08)
- String operations for text processing (13)
- Read spec examples 09-12, 14 to understand design goals

**Advanced** (15-19):
- Concurrency patterns (16)
- Compile-time optimization (17)
- Resource management (18)
- Read spec examples 15, 19 for advanced patterns

**Production** (validation programs):
- Study the 5 validation programs
- See how features combine in real applications
- Use as templates for your own projects

---

## Status Legend

- ✅ **Runnable** - Fully implemented, runs with current interpreter
- 📝 **Spec Example** - Shows intended syntax, not yet implemented
- 🚧 **Partial** - Some features work, some don't
- ❌ **Broken** - Intentionally demonstrates errors (compile_errors/)

---

## Contributing

When adding new examples:

1. **Follow naming convention**: `NN_topic.rk` for learning path
2. **Add SPDX header**: `// SPDX-License-Identifier: (MIT OR Apache-2.0)`
3. **Use `// Learn:` comment** at the top
4. **Test parsing**: `./target/debug/rask parse <file>`
5. **Update this README** with status
6. **Keep examples focused** - one concept per file
7. **Use simple, clear examples** - prioritize teaching over cleverness
8. **Add explanatory comments** - explain *why*, not just *what*

---

## Next Examples to Implement

Priority order for completing spec examples:

1. **09_generics.rk** - Core abstraction, needed for many patterns
2. **11_closures.rk** - Used throughout real programs
3. **12_iterators.rk** - Functional programming patterns
4. **10_enums_advanced.rk** - Rich data modeling
5. **14_borrowing_patterns.rk** - Memory safety patterns
6. **15_memory_management.rk** - Pool/Handle for graphs
7. **19_unsafe.rk** - C interop for systems programming

These align with compiler roadmap priorities.
