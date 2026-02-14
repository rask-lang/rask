# Setting Up Rask

## 1. Install the Rust toolchain

Rask's compiler is written in Rust, so you need Rust to build it:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Follow the prompts (defaults are fine). Then restart your terminal or run:

```bash
source ~/.cargo/env
```

Verify with `cargo --version`.

## 2. Build Rask

```bash
git clone https://github.com/rask-lang/rask.git
cd rask/compiler
cargo build --release
```

This takes a minute or two. The binary ends up at `compiler/target/release/rask`.

## 3. Add to PATH

From the `rask/compiler` directory:

```bash
export PATH="$PWD/target/release:$PATH"
```

Add this line to your `~/.bashrc` or `~/.zshrc` to make it permanent. Then verify:

```bash
rask --version
```

## 4. Install the VSCode extension

1. Open VSCode
2. Open the Extensions panel (`Ctrl+Shift+X` / `Cmd+Shift+X`)
3. Search for **"Rask"**
4. Click Install

The extension gives you:
- Syntax highlighting for `.rk` files
- Error squiggles as you type (from the type checker)
- Go to definition, hover info, autocomplete

If the extension can't find `rask-lsp`, open VSCode settings and set `rask.serverPath`
to the full path of your `rask` binary (e.g. `/home/you/rask/compiler/target/release/rask`).

## 5. Your first run

Open a terminal in VSCode (`Ctrl+`` `) and navigate to the tutorials:

```bash
cd tutorials/learn-rask
```

Run the first lesson:

```bash
rask run 01_hello.rk
```

## 6. The workflow

For each lesson:

```bash
# 1. Open the .rk file in the editor
# 2. Read the teaching comments
# 3. Implement the puzzle functions
# 4. Check your work:
rask test 03_functions.rk

# 5. All tests pass? Move to the next lesson.
```

The test runner shows you which tests pass and which fail, with the expected vs actual values.

## 7. If something goes wrong

- **"command not found: rask"** — PATH isn't set. Re-run the export command.
- **Build errors** — Make sure you have Rust 1.70+ (`rustup update`).
- **No syntax highlighting** — Reload VSCode window (`Ctrl+Shift+P` → "Reload Window").
- **Red squiggles everywhere** — The extension needs `rask-lsp` in PATH. Set `rask.serverPath` in settings.
