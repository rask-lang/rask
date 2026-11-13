# Installation

> **Note:** Rask is in early development. Expect breaking changes.

## Prerequisites

- Rust toolchain (for building from source)
- Git
- A C compiler (cc) — the runtime is compiled from C

## Building from Source

```bash
git clone https://github.com/rask-lang/rask.git
cd rask/compiler
cargo build --release
```

The `rask` binary will be in `compiler/target/release/rask`.

## Verify Installation

```bash
./target/release/rask --version
```

## CLI Commands

| Command | What it does |
|---------|-------------|
| `rask run <file>` | Compile and execute a `.rk` program |
| `rask check <file>` | Type-check without running |
| `rask fmt <file>` | Format source code |
| `rask lint <file>` | Check style and idioms |

Compiled binaries are written to `build/debug/`.

## Running Examples

The repository includes working examples:

```bash
./target/release/rask run ../examples/hello_world.rk
```

## Next Steps

- [Learn the language](../learn/README.md), starting with your first program
