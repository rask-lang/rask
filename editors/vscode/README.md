# Rask for Visual Studio Code

Language support for [Rask](https://github.com/rask-lang/rask) (`.rk` files).

## Features

- Syntax highlighting for `.rk` files
- Rask code blocks in Markdown (` ```rask `)
- Language Server Protocol support via `rask-lsp`
- Bracket matching, auto-closing pairs, folding

## Setup

### 1. Build Rask (if working from source)

```bash
cd compiler
cargo build --release
```

This creates binaries at:
- `compiler/target/release/rask` — CLI (for `rask run`, `rask fmt`, etc.)
- `compiler/target/release/rask-lsp` — Language server

### 2. Install the extension

Install from the marketplace or from a `.vsix` file.

### 3. Configure the language server path

**Option A: Add to PATH** (recommended)

Add `compiler/target/release` to your `PATH`. The extension will find `rask-lsp` automatically.

**Option B: Set `rask.serverPath` in VS Code settings**

Open Settings (Cmd+, / Ctrl+,), search for "rask", and set:

```
Rask: Server Path
/absolute/path/to/compiler/target/release/rask-lsp
```

Example:
```
/Users/yourname/rask/compiler/target/release/rask-lsp
```

The extension infers the `rask` CLI path from the same directory as `rask-lsp`.

### Settings Reference

| Setting | Default | Description |
|---------|---------|-------------|
| `rask.serverPath` | `""` | Absolute path to `rask-lsp` binary. If empty, searches PATH. |

## License

MIT OR Apache-2.0
