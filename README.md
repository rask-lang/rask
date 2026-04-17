<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/book/src/assets/rask-logo-white@3x.png">
    <source media="(prefers-color-scheme: light)" srcset="docs/book/src/assets/rask-logo-dark@3x.png">
    <img alt="rask logo" src="docs/book/src/assets/rask-logo-dark@3x.png" width="500">
  </picture>
</p>

A systems language I'm building around one question: **what if references can't be stored?**

Make references temporary — never in structs, never returned from functions — and lifetime annotations stop being necessary. You pay for it with handles where you'd want shared identity (graphs, entity systems, observers). You get memory safety without annotations and deterministic cleanup without a GC.

Somewhere between Rust and Go. Closer to Rust on safety, closer to Go on ceremony.

**[Why a new language?](WHY_RASK.md)**

**Status.** Compiler (Cranelift backend) and interpreter both run programs. The core language works end-to-end. A few codegen regressions are open — see [issues](https://github.com/rask-lang/rask/issues).

---

## Quick look

```rask
func search_file(path: string, pattern: string) -> () or IoError {
    const file = try fs.open(path)
    ensure file.close()

    for line in file.lines() {
        if line.contains(pattern): println(line)
    }
}
```

Full example: [grep_clone.rk](examples/grep_clone.rk).

---

## Getting started

Build from source (you'll need a Rust toolchain):

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

Next steps: browse [examples/](examples/), try the [tutorials](tutorials/), or read the [Language Guide](LANGUAGE_GUIDE.md).

---

## The design

Three ideas do most of the work.

**No storable references.** You can borrow for a call or expression; you can't store the borrow in a struct or return it. The whole lifetime system becomes unnecessary — there's nothing to track. For graphs and entity systems, you use `Handle<T>`: an integer key into a `Pool<T>`, validated by a generation counter. Accesses are ~2ns; the compiler coalesces redundant checks and eliminates them entirely in `using frozen Pool<T>` contexts.

**Everything is a value.** No reference types, no `Box<T>`/`Rc<T>`/`Arc<T>` distinction. Small values (≤16 bytes) copy; larger ones move. If you need to share, you `.clone()` — which keeps allocations and copies visible where they happen.

**Linearity for I/O.** Files, sockets, and transactions are linear: the compiler makes you consume them exactly once. `ensure file.close()` defers that consumption to scope exit, which is what lets `try` propagate errors without leaking the resource. Three concepts — linearity, deferred consumption, error propagation — and the three-line idiom at the top of this file falls out of them.

Full rationale: [specs/CORE_DESIGN.md](specs/CORE_DESIGN.md).

---

## Tradeoffs

More `.clone()` calls. Some patterns restructure around handles:
- parent pointers → `Handle<Parent>`
- string slices in structs → `Span` indices or `StringPool`
- arbitrary graphs → `Pool<T>`

That's most of the cost. In exchange: no lifetime annotations in signatures, no GC pauses, no use-after-free, no data races. I think that's a good trade.

---

## What works today

- Memory model: ownership, moves, borrows, handles, linearity
- Type system: primitives, structs, enums, generics, traits
- Control flow: if/match/loops
- Concurrency: spawn/join, channels, thread pools
- Error handling: `T or E`, `try`, optionals (`T?`, `??`, `!`)
- Native codegen (Cranelift): structs, closures, Vec/Map, threads, channels, file I/O
- Build system: packages, workspaces, watch mode
- Tooling: `rask build/check/lint/fmt/test`, LSP

**Next:** validation-program regressions ([#203](https://github.com/rask-lang/rask/issues/203)); HTTP and JSON stdlib in Rask — see [ROADMAP.md](ROADMAP.md).

---

## Inspiration

Rust for ownership, Results, traits. Go for simplicity (if Rask needs three lines where Go needs one, I've probably designed it wrong). Zig for `comptime` and cost transparency. Jai for build scripts as real code. Swift's `defer` is where `ensure` came from. Kotlin for `extend` blocks and `T?`. Hylo for value semantics. Vale for generational references. Erlang for bitmatch.

---

## Docs

- [Language Guide](LANGUAGE_GUIDE.md) — the full explanation, jargon-free
- [Tutorials](tutorials/) — hands-on challenges
- [Examples](examples/) — working programs
- [Specs](specs/) — formal language specifications, starting with [CORE_DESIGN.md](specs/CORE_DESIGN.md)
- [Book](https://rask-lang.dev/book) — online guide (work in progress)

---

## License

MIT or Apache 2.0, your choice.
