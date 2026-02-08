# Examples

Real Rask programs that demonstrate practical patterns.

All example code is in the repository's `examples/` folder and runs on the current interpreter.

## Available Examples

- [Grep Clone](grep-clone.md) - File search with pattern matching
- [Game Loop](game-loop.md) - Entity system with handles
- [Text Editor](text-editor.md) - Undo/redo with resource management

## Running Examples

```bash
git clone https://github.com/rask-lang/rask.git
cd rask/compiler
cargo build --release
./target/release/rask ../examples/grep_clone.rk --help
```

[View all examples on GitHub →](https://github.com/rask-lang/rask/tree/main/examples)

## What These Demonstrate

Each example showcases key Rask concepts:

**Grep Clone**
- CLI argument parsing
- File I/O with error handling
- String operations
- Resource cleanup with `ensure`

**Game Loop**
- Entity-component system using `Pool<T>`
- Handle-based indirection
- Game state management

**Text Editor**
- Command pattern for undo/redo
- File I/O and resource management
- State transitions
