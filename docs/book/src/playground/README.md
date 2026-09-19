# Try it in the browser

The [playground](/app/) runs Rask without installing anything. It's the
interpreter compiled to WebAssembly, so the code runs in your browser rather
than on a server.

**[Open the playground →](/app/)**

Write code on the left, `Ctrl+Enter` to run, output on the right. **Run tests**
runs the file's `test` blocks instead of `main`. The examples dropdown loads the
programs from
[examples/](https://github.com/rask-lang/rask/tree/main/examples), and **Copy
link** gives you a URL with your code in it.

## What it can't do

A browser has no files, no sockets, no clock and no threads, so anything
needing one of those is refused with a message rather than half-working:

| | |
|---|---|
| `fs`, `io`, `net`, `http` | no filesystem or sockets |
| `time` | no clock — which is also why `benchmark` blocks don't run here |
| `spawn`, `Thread.spawn`, `using Multitasking`, `using ThreadPool` | no threads |
| `extern "C"`, `import c` | no libc to call |

Everything else runs: collections, structs, enums, generics, traits, pattern
matching, closures, error handling, `comptime`. Recursion is capped a few
hundred frames deep, because the browser puts a much lower ceiling on call
depth than an OS thread does.

The dropdown only lists examples that actually run here. The rest are in
[examples/](https://github.com/rask-lang/rask/tree/main/examples); to run those,
[install Rask](../getting-started/installation.md) and run the file directly.

## Source

[Playground UI](https://github.com/rask-lang/rask/tree/main/docs/playground) ·
[WASM bindings](https://github.com/rask-lang/rask/tree/main/compiler/crates/rask-wasm)
