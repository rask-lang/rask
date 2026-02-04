# Rask Implementation Status

This document tracks what is specified, implemented, and tested. Updated manually as features mature.

## Status Legend

| Symbol | Meaning |
|--------|---------|
| ✅ | Fully implemented and tested |
| 🔶 | Partially implemented |
| 📋 | Specified only (not implemented) |
| ❌ | Not started |

## Language Features

| Feature | Spec | Interpreter | Compiler | Tests | Example |
|---------|------|-------------|----------|-------|---------|
| **Bindings** (`let`/`const`) | ✅ | ✅ | ❌ | ✅ | All |
| **Basic types** (i32, f64, bool, string) | ✅ | ✅ | ❌ | ✅ | All |
| **Structs** | ✅ | ✅ | ❌ | 🔶 | game_loop |
| **Enums** | ✅ | ✅ | ❌ | 🔶 | cli_calculator |
| **Pattern matching** | ✅ | 🔶 | ❌ | 🔶 | cli_calculator |
| **Functions** | ✅ | ✅ | ❌ | ✅ | All |
| **Traits** | ✅ | 🔶 | ❌ | ❌ | game_loop |
| **Generics** | ✅ | 🔶 | ❌ | ❌ | - |
| **Closures** | ✅ | 🔶 | ❌ | ❌ | - |
| **Modules** | ✅ | ✅ | ❌ | 🔶 | All |

## Memory Model

| Feature | Spec | Interpreter | Compiler | Tests |
|---------|------|-------------|----------|-------|
| **Value semantics** | ✅ | ✅ | ❌ | 🔶 |
| **Move semantics** | ✅ | ✅ | ❌ | 🔶 |
| **Block-scoped borrows** | ✅ | 🔶 | ❌ | ❌ |
| **Expression-scoped borrows** | ✅ | 🔶 | ❌ | ❌ |
| **Field projections** | ✅ | ❌ | ❌ | ❌ |
| **Implicit copy (≤16 bytes)** | ✅ | 🔶 | ❌ | ❌ |

## Collections

| Feature | Spec | Interpreter | Compiler | Tests | Example |
|---------|------|-------------|----------|-------|---------|
| **Vec** | ✅ | ✅ | ❌ | 🔶 | All |
| **Map** | ✅ | ✅ | ❌ | 🔶 | http_api_server |
| **Pool + Handle** | ✅ | 🔶 | ❌ | ❌ | game_loop |
| **Pool auto-resolution** | 📋 | ❌ | ❌ | ❌ | - |

## Concurrency

| Feature | Spec | Interpreter | Compiler | Tests | Example |
|---------|------|-------------|----------|-------|---------|
| **spawn (green tasks)** | ✅ | ❌ | ❌ | ❌ | http_api_server |
| **spawn_thread** | ✅ | ❌ | ❌ | ❌ | sensor_processor |
| **Channels** | ✅ | ❌ | ❌ | ❌ | http_api_server |
| **Shared<T>** | ✅ | ❌ | ❌ | ❌ | http_api_server |
| **No function coloring** | 📋 | ❌ | ❌ | ❌ | - |

## Resource Types

| Feature | Spec | Interpreter | Compiler | Tests |
|---------|------|-------------|----------|-------|
| **@resource attribute** | ✅ | 🔶 | ❌ | ❌ |
| **Linear consumption** | ✅ | ❌ | ❌ | ❌ |
| **ensure cleanup** | ✅ | ❌ | ❌ | ❌ |

## Comptime

| Feature | Spec | Interpreter | Compiler | Tests |
|---------|------|-------------|----------|-------|
| **comptime functions** | ✅ | 🔶 | ❌ | ❌ |
| **comptime constants** | ✅ | 🔶 | ❌ | ❌ |
| **Iteration limits** | ✅ | ❌ | ❌ | ❌ |
| **Mutable arrays at comptime** | 📋 | ❌ | ❌ | ❌ |

## Stdlib Modules

| Module | Spec | Interpreter | Tests | Example |
|--------|------|-------------|-------|---------|
| **io** (print, read_line) | 📋 | 🔶 | ❌ | cli_calculator |
| **fs** (File, read, write) | 📋 | ❌ | ❌ | file_copy |
| **cli** (args) | 📋 | ❌ | ❌ | grep_clone |
| **time** (now, Duration) | 📋 | ❌ | ❌ | game_loop |
| **net** (TcpListener) | 📋 | ❌ | ❌ | http_api_server |
| **json** | 📋 | ❌ | ❌ | http_api_server |
| **regex** | 📋 | ❌ | ❌ | grep_clone |

## Examples Status

| Example | Parses | Runs | Tests Pass | Notes |
|---------|--------|------|------------|-------|
| file_copy.rask | ✅ | ❌ | N/A | Needs fs module |
| cli_calculator.rask | ✅ | 🔶 | ❌ | Needs test runner |
| grep_clone.rask | ✅ | ❌ | N/A | Needs fs, cli, regex |
| http_api_server.rask | ✅ | ❌ | N/A | Needs net, concurrency |
| game_loop.rask | ✅ | ❌ | N/A | Needs Pool, time |
| sensor_processor.rask | ✅ | ❌ | N/A | Needs threading, SIMD |
| text_editor.rask | ✅ | ❌ | N/A | Needs terminal I/O |

## Next Milestones

### M1: First End-to-End Example
- [ ] Implement minimal `fs` module (open, read, write, close)
- [ ] Implement minimal `cli` module (args)
- [ ] Run file_copy.rask end-to-end

### M2: Test Runner
- [ ] Implement inline `test` block execution
- [ ] Run cli_calculator.rask tests
- [ ] Validate syntax through passing tests

### M3: Concurrency Foundation
- [ ] Implement basic task runtime
- [ ] Implement channels
- [ ] Run simple spawn/join example

---

*Last updated: 2026-02-05*
