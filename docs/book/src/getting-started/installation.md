# Installation

> **Note:** Rask is in early development. Currently only the interpreter is available.

## Prerequisites

- Rust toolchain (for building from source)
- Git

## Building from Source

```bash
git clone https://github.com/dritory/rask.git
cd rask/compiler
cargo build --release
```

The `rask` binary will be in `compiler/target/release/rask`.

## Verify Installation

Run a test to verify the interpreter works:

```bash
./target/release/rask --version
```

## Running Examples

The repository includes working examples:

```bash
./target/release/rask ../examples/hello_world.rk
```

## Next Steps

- [Your First Program](first-program.md)
- [Basic Syntax](../guide/basic-syntax.md)
