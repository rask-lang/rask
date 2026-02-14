# Rask for Visual Studio Code

Language support for [Rask](https://github.com/rask-lang/rask) (`.rk` files).

## Features

- Syntax highlighting for `.rk` files
- Rask code blocks in Markdown (` ```rask `)
- Language Server Protocol support via `rask-lsp`
- Bracket matching, auto-closing pairs, folding

## Setup

1. Install the extension
2. Make sure `rask-lsp` is on your PATH, or set `rask.serverPath` in settings

### Settings

| Setting | Default | Description |
|---------|---------|-------------|
| `rask.serverPath` | `""` | Path to the `rask-lsp` binary. If empty, searches PATH. |

## License

MIT OR Apache-2.0
