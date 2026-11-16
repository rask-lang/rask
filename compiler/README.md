# Rask Compiler

The `rask` command: compiler, interpreter, test runner, formatter, linter and
language server, in one binary.

```
.rk → Lexer → Parser → Desugar → Resolve → TypeCheck → Comptime → Ownership → MIR → Codegen / Interp
```

Native code goes through Cranelift and is linked with the system C compiler
against the runtime in `runtime/`. The interpreter (`--interp`) runs the
checked program directly and is the reference when the two disagree.

## Build

```bash
cargo build --release -p rask-cli
```

The binary is `target/release/rask`. It carries the C runtime sources and the
standard library inside it, so the file works on its own anywhere.

## Read on

- [COMPILER.md](COMPILER.md): a walk through the code, assuming no compiler
  background.
- [CLAUDE.md](CLAUDE.md): which crate and file to open for a given task.
- [../CONTRIBUTING.md](../CONTRIBUTING.md): tests, gates, and sending a change.
- `rask help` lists the commands.
