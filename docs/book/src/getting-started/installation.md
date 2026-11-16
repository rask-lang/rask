# Install it

Rask is young and changes between releases. Expect breaking changes.

## The one line

```bash
curl -fsSL https://rask-lang.dev/install.sh | sh
```

That downloads the latest release's `rask` binary for your machine into
`~/.local/bin`. Releases have binaries for **Linux on x86_64** and **macOS on
Apple silicon**; on anything else the script says so and stops, and you build
from source (below). It doesn't touch anything else, so uninstalling is
deleting `~/.local/bin/rask`.

Two things it checks for and tells you about:

- **`~/.local/bin` on your PATH.** If it isn't, the script prints the line to
  add to your shell's startup file.
- **A C compiler.** `rask run` compiles your program to machine code, and the
  last step, linking, uses the system's `cc`. Without one only the interpreter
  works (`rask run --interp`). On macOS, `xcode-select --install` gets you
  one; on Linux, your package manager's `gcc`.

Check it worked:

```bash
rask --version
```

To pick a release other than the latest, or install somewhere else, set
`RASK_VERSION` or `RASK_INSTALL_DIR` for the `sh`:

```bash
curl -fsSL https://rask-lang.dev/install.sh | RASK_VERSION=0.6.0 sh
```

The script
itself is [docs/install.sh](https://github.com/rask-lang/rask/blob/main/docs/install.sh)
if you'd rather read it first.

## Get the tour

The tour is a set of files you run and edit, so get a copy of them:

```bash
git clone --depth 1 https://github.com/rask-lang/rask
cd rask/docs/book/src/learn
rask run 01-hello.rk
```

Open `README.md` in that directory, or [read it here](../learn/README.md).
The tour follows the repository's main branch, which can be ahead of the
latest release. If a step fails with an error its text doesn't mention, your
`rask` is older than the tour: build it from source as below.

## Build from source

You need Rust (from [rustup.rs](https://rustup.rs)), git and a C compiler.

```bash
git clone https://github.com/rask-lang/rask
cd rask/compiler
cargo build --release -p rask-cli
```

The binary is `compiler/target/release/rask`. Put that directory on your
PATH, or copy the file to `~/.local/bin`.

## The commands

| Command | What it does |
|---------|-------------|
| `rask run file.rk` | Compile and run a program |
| `rask run --interp file.rk` | Run it on the interpreter instead, no C compiler needed |
| `rask test file.rk` | Run the file's `test` blocks |
| `rask check file.rk` | Type-check without running |
| `rask build` | Build the package in this directory into `build/` |
| `rask fmt file.rk` | Format the file |
| `rask lint file.rk` | Point out style and naming problems |

`rask help` lists the rest.

## Next

[Take the tour](../learn/README.md): one idea per step, starting with hello
world.
