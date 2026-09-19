<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/book/src/assets/rask-logo-white@3x.png">
    <source media="(prefers-color-scheme: light)" srcset="docs/book/src/assets/rask-logo-dark@3x.png">
    <img alt="rask logo" src="docs/book/src/assets/rask-logo-dark@3x.png" width="500">
  </picture>
</p>

A programming language I'm building around one question: **what if references can't be stored?**

Make references temporary — never in structs, never returned from functions — and lifetime annotations stop being necessary. The cost is more `.clone()` calls, and a rack to own anything you want shared identity for: graphs, entity systems, observers. The benefit is memory safety without annotations, deterministic cleanup without a GC, and function signatures you can read in one pass.

Somewhere between Rust and Go. Closer to Rust on safety, closer to Go on ceremony. Whether the trade actually works out is what I'm trying to find out.

**[Why a new language?](WHY_RASK.md)**

**Status** (measured 2026-09-11). Compiler (Cranelift backend) and interpreter both run programs, and all five validation programs — including the HTTP JSON server — run natively. Around 80 open issues, mostly codegen getting memory release wrong: see [issues](https://github.com/rask-lang/rask/issues). It's a solo project, so fixes come in waves.

---

## Quick look

<!-- test: compile -->
```rask
import fs
import io

func grep(path: string, pat: string) -> void or io.IoError {
    mut file = try fs.open(path)
    ensure file.close()

    let text = try file.read_text()
    for line in text.lines() {
        if line.contains(pat) { println(line) }
    }
}
```

Full example: [grep_clone.rk](examples/grep_clone.rk).

---

## Getting started

Build from source. You'll need a Rust toolchain for now — bootstrapping the compiler in Rask itself is on the list, just not soon.

```bash
git clone https://github.com/rask-lang/rask.git
cd rask/compiler
cargo build --release
export PATH="$PWD/target/release:$PATH"
```

Then:

```bash
rask run examples/hello_world.rk
```

Other commands: `rask check`, `rask lint`, `rask fmt`, `rask test`.

Next: read [Learning Rask](https://rask-lang.dev/book), or browse [examples/](examples/) if you'd rather read whole programs.

---

## The design

Three ideas do most of the work.

**No storable references.** You can borrow for a call or an expression; you can't store the borrow in a struct, and you can't return it. The whole lifetime system stops being necessary — there's just nothing to track.

Graphs and entity systems get the one exception: a `Rack<T>` owns nodes at stable addresses, and a `Link<T>` into it *may* live in a struct field. Deleting a node sets every edge pointing at it to `none` before the delete returns, so a dangling link never exists — which is what earns the unchecked read. Following a live link is a pointer hop, no generation counter, no liveness test.

**Everything is a value.** No reference types. No `Box<T>`/`Rc<T>`/`Arc<T>` distinction. Small values (≤16 bytes) copy, larger ones move, and you `.clone()` when you want to share. More clones than Rust, but the clones are visible in the code, which I think is the right direction.

**Linearity for I/O.** Files, sockets, and transactions are linear: the compiler makes you consume them exactly once. `ensure file.close()` defers that consumption to scope exit, which is what lets `try` propagate errors without leaking the resource. Three concepts that compose — linearity, deferred consumption, error propagation — and the idiom at the top of this file falls out of them. This is probably the piece of the design I'm happiest with.

Full rationale: [specs/CORE_DESIGN.md](specs/CORE_DESIGN.md).

---

## Tradeoffs

More `.clone()` calls. Some patterns restructure:
- parent pointers → `Link<Parent>` into the rack that owns them
- string slices in structs → `StringView` (zero-copy, refcounted) or `Span` indices
- arbitrary graphs → `Rack<T>` + `Link<T>`

That's most of the cost. What you get back: no lifetime annotations in signatures, no GC pauses, no use-after-free, no data races. I think it's a good trade. Some days I'm less sure.

---

## What works today

- Memory model: ownership, moves, borrows, linearity
- Type system: primitives, structs, enums, generics, traits
- Control flow: if/match/loops
- Concurrency: spawn/join, channels, thread pools
- Error handling: `T or E` with `try` to propagate and `catch e =>` to handle; optionals (`T?`, `??`, `!`, `is none`)
- Native codegen (Cranelift): structs, closures, Vec/Map, threads, channels, file I/O
- Build system: packages, workspaces, watch mode
- Tooling: `rask build/check/lint/fmt/test`, LSP

**Next:** the sequence protocol (`Vec.iter()` returning a `Sequence`, [#1046](https://github.com/rask-lang/rask/issues/1046)), native lowering for Rack and Link, and the memory-release bugs in codegen. See [ROADMAP.md](ROADMAP.md) for the order and why.

---

## Inspiration

Rust for ownership, Results, traits. Go for simplicity (if Rask needs three lines where Go needs one, I've probably designed it wrong). Zig for `comptime` and cost transparency. Jai for build scripts as real code. Swift's `defer` is where `ensure` came from. Kotlin for `extend` blocks and `T?`. Hylo for value semantics. Vale for generational references. Erlang for bitmatch.

---

## Docs

Four documents, four different jobs:

| | For |
|---|---|
| [Learning Rask](https://rask-lang.dev/book) | Learning the language. Install, first program, chapters. Start here |
| [LANGUAGE_CARD.md](LANGUAGE_CARD.md) | Looking a rule up while you write. The whole language, compressed, one page |
| [examples/](examples/) | Reading complete programs. Each one is compiled and run by CI |
| [specs/](specs/) | The normative wording, and the reasoning behind it. Start at [CORE_DESIGN.md](specs/CORE_DESIGN.md) |

Also: the [blog](https://rask-lang.dev/blog/) (written in [writing/](writing/)) for the long-form design
arguments, [tutorials/](tutorials/) for hands-on exercises, and
[specs/RULINGS.md](specs/RULINGS.md) for how design questions get decided.

---

## License

MIT or Apache 2.0, your choice.
