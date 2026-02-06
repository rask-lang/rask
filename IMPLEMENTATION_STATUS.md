# Rask Implementation Status

What is specified, implemented, and tested. Updated 2026-02-06.

## Status Legend

| Symbol | Meaning |
|--------|---------|
| ✅ | Fully implemented and tested |
| 🔶 | Partially implemented |
| 📋 | Specified only (not implemented) |
| ❌ | Not started |

## Compiler Pipeline

| Stage | Crate | Status | Notes |
|-------|-------|--------|-------|
| Lexer | `rask-lexer` | ✅ | All tokens, keywords, operators |
| Parser | `rask-parser` | ✅ | Full AST: const/let, func, struct, enum, match, try, ensure, spawn, etc. |
| Name resolution | `rask-resolve` | 🔶 | Scope tree, symbol table. Some gaps |
| Type checker | `rask-types` | 🔶 | Works on simple programs. Gaps: `own` keyword, complex enum patterns |
| Ownership checker | `rask-ownership` | 🔶 | Move tracking, borrow scopes. Simple programs only |
| Interpreter | `rask-interp` | ✅ | Runs real programs end-to-end |
| Comptime | `rask-comptime` | 🔶 | Basic comptime evaluation |
| LSP | `rask-lsp` | 🔶 | Skeleton |
| Code generation | — | ❌ | No backend yet |

## Language Features

| Feature | Spec | Parser | Type Checker | Interpreter | Tests |
|---------|------|--------|--------------|-------------|-------|
| **Bindings** (`let`/`const`) | ✅ | ✅ | ✅ | ✅ | ✅ |
| **Basic types** (i32, f64, bool, string) | ✅ | ✅ | ✅ | ✅ | ✅ |
| **Structs** | ✅ | ✅ | 🔶 | ✅ | ✅ |
| **Enums** | ✅ | ✅ | 🔶 | ✅ | ✅ |
| **Pattern matching** (match, if-is) | ✅ | ✅ | 🔶 | ✅ | ✅ |
| **Functions** | ✅ | ✅ | ✅ | ✅ | ✅ |
| **Explicit return** | ✅ | ✅ | ✅ | ✅ | ✅ |
| **Missing return detection** | ✅ | — | ✅ | — | ✅ |
| **Traits** | ✅ | ✅ | 🔶 | 🔶 | ❌ |
| **Generics** | ✅ | ✅ | 🔶 | 🔶 | ❌ |
| **Closures** | ✅ | ✅ | ❌ | 🔶 | ❌ |
| **Modules/imports** | ✅ | ✅ | 🔶 | ✅ | 🔶 |

## Error Handling

| Feature | Spec | Parser | Interpreter | Tests |
|---------|------|--------|-------------|-------|
| **Result / `T or E`** | ✅ | ✅ | ✅ | ✅ |
| **`try` propagation** | ✅ | ✅ | ✅ | ✅ |
| **Option / `T?`** | ✅ | ✅ | ✅ | ✅ |
| **`??` default** | ✅ | ✅ | ✅ | 🔶 |
| **`ensure` cleanup** | ✅ | ✅ | ✅ | ✅ |
| **`ensure` catch** | ✅ | ✅ | ✅ | ✅ |

## Memory Model

| Feature | Spec | Interpreter | Type Checker | Tests |
|---------|------|-------------|--------------|-------|
| **Value semantics** | ✅ | ✅ | 🔶 | 🔶 |
| **Move semantics** | ✅ | ✅ | 🔶 | 🔶 |
| **Block-scoped borrows** | ✅ | 🔶 | ❌ | ❌ |
| **Expression-scoped borrows** | ✅ | 🔶 | ❌ | ❌ |
| **Field projections** | ✅ | ❌ | ❌ | ❌ |
| **Implicit copy (≤16 bytes)** | ✅ | 🔶 | ❌ | ❌ |

## Resource Types

| Feature | Spec | Interpreter | Tests |
|---------|------|-------------|-------|
| **`@resource` attribute** | ✅ | ✅ | ✅ |
| **Linear consumption tracking** | ✅ | ✅ | ✅ |
| **Leak detection at scope exit** | ✅ | ✅ | ✅ |
| **`ensure` satisfies linearity** | ✅ | ✅ | ✅ |
| **Ownership transfer via return** | ✅ | ✅ | ✅ |

## Collections

| Feature | Spec | Interpreter | Tests |
|---------|------|-------------|-------|
| **Vec** (push, pop, indexing, len) | ✅ | ✅ | ✅ |
| **Vec range indexing** (`v[1..3]`) | ✅ | ✅ | ✅ |
| **Map** (insert, get, remove) | ✅ | ✅ | 🔶 |
| **Pool + Handle** | ✅ | 🔶 | ❌ |
| **Pool auto-resolution** (`with`) | 📋 | ❌ | ❌ |

## Concurrency

| Feature | Spec | Interpreter | Tests |
|---------|------|-------------|-------|
| **`spawn_raw { }` (OS thread)** | ✅ | ✅ | ✅ |
| **`spawn_thread { }` (pool)** | ✅ | ✅ | ✅ |
| **`with threading(n) { }`** | ✅ | ✅ | ✅ |
| **`handle.join()`** | ✅ | ✅ | ✅ |
| **`handle.detach()`** | ✅ | ✅ | ✅ |
| **Channel.buffered(n)** | ✅ | ✅ | ✅ |
| **Channel.unbuffered()** | ✅ | ✅ | ✅ |
| **sender.send / receiver.recv** | ✅ | ✅ | ✅ |
| **receiver.try_recv** | ✅ | ✅ | ✅ |
| **`spawn { }` (green tasks)** | ✅ | ❌ | ❌ |
| **`select` / `select_priority`** | ✅ | ❌ | ❌ |
| **Shared<T>** | 📋 | ❌ | ❌ |
| **No function coloring runtime** | 📋 | ❌ | ❌ |

## String Methods

| Method | Interpreter | Tests |
|--------|-------------|-------|
| `len()` | ✅ | ✅ |
| `contains()` | ✅ | ✅ |
| `starts_with()` / `ends_with()` | ✅ | ✅ |
| `to_lowercase()` / `to_uppercase()` | ✅ | ✅ |
| `trim()` / `trim_start()` / `trim_end()` | ✅ | ✅ |
| `split()` / `split_whitespace()` | ✅ | ✅ |
| `parse()` (→ i64) | ✅ | ✅ |
| `to_owned()` | ✅ | ✅ |
| `chars()` | ✅ | 🔶 |
| String interpolation | ✅ | ✅ |

## Stdlib Modules (Interpreter)

| Module | Status | Notes |
|--------|--------|-------|
| **io** (println, print, read_line) | ✅ | Built-in |
| **fs** (open, create, read, write, close) | ✅ | File I/O works, linear resource tracked |
| **cli** (args) | ✅ | `cli.args()` returns Vec<string> |
| **random** (random_int, random_range) | ✅ | Basic RNG |
| **time** | ❌ | Not implemented |
| **net** | ❌ | Not implemented |
| **json** | ❌ | Not implemented |
| **fmt** | ❌ | String interpolation exists, no format spec |
| **path** | ❌ | Not implemented |

## Examples Status

| Example | Parses | Type Checks | Runs | Notes |
|---------|--------|-------------|------|-------|
| hello_world.rask | ✅ | ✅ | ✅ | |
| simple_grep.rask | ✅ | ❌ | ✅ | Type checker gaps |
| cli_calculator.rask | ✅ | ❌ | ✅ | Waits for stdin |
| file_copy.rask | ✅ | ❌ | ✅ | |
| game_loop.rask | ✅ | ❌ | ✅ | Simplified version |
| grep_clone.rask | ✅ | ❌ | ✅ | Full featured |
| collections_test.rask | ✅ | ❌ | ✅ | |
| pool_test.rask | ✅ | ❌ | 🔶 | Basic pool only |
| http_api_server.rask | ✅ | ❌ | ❌ | Needs net module |
| text_editor.rask | ✅ | ❌ | ❌ | Needs terminal I/O |
| sensor_processor.rask | ✅ | ❌ | ❌ | Needs SIMD, comptime |

## Test Files (root)

All pass:
`test_channels`, `test_ensure`, `test_ensure_catch`, `test_linear_resources`, `test_linear_file_leak`, `test_linear_struct_leak`, `test_spawn_raw`, `test_spawn_thread`, `test_thread_detach`, `test_match_*`, `test_semicolon_block*`

---

*Last updated: 2026-02-06*
