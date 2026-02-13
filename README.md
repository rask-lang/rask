<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/book/src/assets/rask-logo-white@3x.png">
    <source media="(prefers-color-scheme: light)" srcset="docs/book/src/assets/rask-logo-dark@3x.png">
    <img alt="rask logo" src="docs/book/src/assets/rask-logo-dark@3x.png" width="500">
  </picture>
</p>

A research language exploring one question: **what if references can't be stored?**

Rask sits somewhere between Rust and Go — memory safety without lifetime annotations or garbage collection, by making references temporary. They can't be stored in structs or returned from functions.

It's a hobby project. I'm figuring out how far this approach can go.

**Status:** Design phase with working interpreter (grep, game loop, text editor all run). No compiler yet.

---

## Quick Look

```rask
func search_file(path: string, pattern: string) -> () or IoError {
    const file = try fs.open(path)
    ensure file.close()

    for line in file.lines() {
        if line.contains(pattern): println(line)
    }
}
```

Full example: [grep_clone.rk](examples/grep_clone.rk)

---

**Jump to:**
- [Getting Started](#getting-started) - Build and run
- [The Idea](#the-idea) - What I'm trying and why
- [What This Costs](#what-this-costs) - Tradeoffs
- [Implementation Status](#implementation-status) - What works today
- [Design Principles](#design-principles) - Core philosophy
- [Documentation](#documentation) - Where to look

---

## Getting Started

### Build

```bash
git clone https://github.com/rask-lang/rask.git
cd rask/compiler
cargo build --release
```

### Add to PATH

```bash
export PATH="$PWD/target/release:$PATH"
```

Add to your shell profile (`~/.bashrc`, `~/.zshrc`) to make it permanent.

### Run

```bash
cd ..
rask run examples/hello_world.rk
```

### What you can do

```bash
rask run <file>       # execute a .rk program
rask check <file>     # type-check only
rask lint <file>      # style/idiom check
rask fmt <file>       # auto-format
```

### Next steps

- Browse [examples/](examples/) for working programs
- Try the [tutorials](tutorials/) — hands-on challenges with built-in reference
- Read the [Language Guide](LANGUAGE_GUIDE.md) for the full explanation

---

## The Idea

### No Storable References
The core experiment. You can borrow a value temporarily (for a function call or expression), but you can't store that borrow in a struct or return it. This sounds limiting — and it is — but it sidesteps the need for lifetime annotations entirely. For graphs and complex structures, you use handles (validated indices) instead.

### No Garbage Collection
Cleanup happens deterministically when values go out of scope. For I/O resources, the `ensure` keyword guarantees cleanup even on early returns.

### Composition Over Inheritance
Structs hold data, traits define behavior, you extend types with methods. No inheritance hierarchies, no vtable gymnastics unless you explicitly want runtime polymorphism (`any Trait`).

### What I've Landed On So Far

| Concept | What It Means |
|---------|--------------|
| **Value semantics** | Everything is a value, no hidden sharing |
| **Single ownership** | Every value has one owner, cleanup is deterministic |
| **Two-tier borrowing** | "Can it grow?" — fixed sources keep views to block end, collections release at semicolon |
| **Handles for graphs** | Entity systems and cycles use validated indices. Regular structs stay on the stack |
| **Context clauses** | Handle functions declare pool needs; compiler threads them implicitly |
| **Linear resource types** | Files, sockets must be explicitly consumed—can't forget to close them |
| **No function coloring** | I/O operations just work, no async/await split |

---

## What This Costs

I'm not pretending there aren't tradeoffs. Here's what you give up:

**Handle overhead:** Accessing through handles costs ~1-2ns (generation check + bounds check, needs actual benchmark proof). In most code this doesn't matter. In tight loops processing millions of items, copy data out and batch process. Compiler coalesces redundant checks; `pool.freeze()` eliminates them entirely for read-heavy phases.

**Restructuring some patterns:**
- Parent pointers → store handles
- String slices in structs → store indices or use StringPool
- Arbitrary graphs → use Pool<T> with handles

**More `.clone()` calls:** In string-heavy code (CLI parsing, HTTP routing) you'll see ~5% of lines with an explicit clone. I think that's better than lifetime annotations everywhere.

The upside, if the approach works out:
- No use-after-free, no dangling pointers, no data races
- No lifetime annotations
- No GC pauses
- Readable function signatures

---

## Implementation Status

**Right now:** Everything runs interpreted. The lexer, parser, type checker, and interpreter work for the core language features. Three of five litmus test programs run (grep, game loop, text editor). Probably buggy — haven't had time testing everything.

**What works:**
- Memory model: ownership, moves, borrows, handles
- Type system: primitives, structs, enums, generics, traits
- Control flow: if/match/loops with explicit returns
- Concurrency: spawn/join, channels, thread pools
- Resource types: files must be closed, linear tracking works
- Error handling: `T or E` results, `try` propagation
- Standard types: Vec, Map, Pool, String, Option, Result
- vscode extension with LSP

**What's next:**
- Code generation (right now it's all interpreted)
- Network I/O (HTTP server example is blocked on this)
- SIMD and const generics (embedded example needs this)
---

## Design Principles

1. **Transparency of Cost** — Major costs visible in code (allocations, locks, I/O)
2. **Mechanical Safety** — Safety by construction, not runtime checks
3. **Practical Coverage** — Handle 80%+ of real use cases
4. **Ergonomic Simplicity** — Common patterns should be low ceremony.

The constant balancing act is keeping ergonomics high without hiding costs. When in doubt, I choose visibility over convenience.

## Inspiration

Rask borrows ideas from everywhere:

**From Rust:** Ownership, move semantics, Result types, traits. Don't fix what isn't broken.

**From Go:** The focus on simplicity and getting out of the developer's way. If Rask needs 3+ lines where Go needs 1, something's wrong.

**From Zig:** Compile-time execution (`comptime`) and transparency of cost. I want you to see where allocations happen.

**From Jai:** Build scripts as real code. In Rask, `build.rk` files use the actual language, not some separate format.

**From Swift:** `defer` became `ensure` for guaranteed cleanup. When a function can exit early, resources still get freed.

**From Kotlin:** Extension methods (`extend` blocks) and `T?` syntax for optionals. I rejected the implicit scope functions though—Rask uses explicit closure parameters instead.

**From Hylo:** Value semantics rather than pointer chasing. Hylo takes a more formal approach; I'm going for something more pragmatic, but I'm watching their work closely.

**From Vale:** Vale proved that generational references are a valid memory model. I'm trying to limit them to where they're actually needed.

**From Erlang:** Bitmatch and supervision pattern. When you need it, it's irreplaceable.


---

## Documentation

| Resource | What |
|----------|------|
| [Language Guide](LANGUAGE_GUIDE.md) | Full explanation of every feature, jargon-free |
| [Tutorials](tutorials/) | Hands-on challenges with built-in reference |
| [Examples](examples/) | Working programs from hello world to grep clone |
| [Book](https://rask-lang.dev/book) | Online guide (work in progress) |
| [Specs](specs/) | Formal language specifications |

### Project Structure

```
├── LANGUAGE_GUIDE.md       # The full language explanation
├── CORE_DESIGN.md          # Design philosophy and core mechanisms
├── tutorials/              # Hands-on challenges (5 levels)
├── examples/               # Working .rk programs
├── specs/                  # Language specifications
│   ├── types/              # Type system, generics, traits
│   ├── memory/             # Ownership, borrowing, resources
│   ├── control/            # Loops, match, comptime
│   ├── concurrency/        # Tasks, threads, channels
│   ├── structure/          # Modules, packages, builds
│   └── stdlib/             # Standard library APIs
└── compiler/               # The implementation
    ├── rask-lexer/         # Tokenization
    ├── rask-parser/        # AST construction
    ├── rask-types/         # Type checking
    ├── rask-interp/        # Interpreter (current execution)
    └── ...
```

---

## License

Licensed under either of Apache License or MIT license at your option.
