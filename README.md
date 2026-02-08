<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/book/src/assets/rask-logo-white@3x.png">
    <source media="(prefers-color-scheme: light)" srcset="docs/book/src/assets/rask-logo-dark@3x.png">
    <img alt="rask logo" src="docs/book/src/assets/rask-logo-dark@3x.png" width="500">
  </picture>
</p>

Safety without the pain!    
  


Rask is a new programming language that aims to sit between Rust (compile-time safety, unergonomic) and go (runtime heavy, ergonomic).

Rask targets the 80% of "systems programming" that's actually application code: servers, tools, games, embedded apps.

You get Rust's safety guarantees without lifetime annotations.  
You get Go's simplicity without garbage collection.

Can you build Linux in it? Probably not.  
Can you build the next web server? The next game? Absolutely.

No annotations. No fights. No hidden costs.

**Status:** Design phase with working interpreter (grep, game loop, text editor all run). No compiler.

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

No lifetime annotations. No borrow checker fights. No GC pauses. Just clean code that's safe by construction.

Full example: [grep_clone.rk](examples/grep_clone.rk)

---

**Jump to:**
- [Why Rask?](#why-rask) - The design choices that make this possible
- [What This Costs](#what-this-costs) - Honest tradeoffs
- [Implementation Status](#implementation-status) - What works today
- [Design Principles](#design-principles) - Core philosophy
- [Documentation](#documentation) - Where to look

---

## Why Rask?

### No Lifetime Annotations
They're the wrong abstraction. Instead of tracking how long references live, I made references impossible to store. This eliminates use-after-free without any annotations.

### No Storable References
You can borrow a value temporarily (for a function call or expression), but you can't store that borrow in a struct or return it. This sounds limiting—and it is—but it removes entire categories of bugs by construction. For graphs and complex structures, you use handles (validated indices) instead.

### No Traditional Classes
For this memory model, composition works better. Structs hold data, traits define behavior, you extend types with methods. No inheritance hierarchies, no vtable gymnastics unless you explicitly want runtime polymorphism (`any Trait`).

### No Garbage Collection
Predictable performance. Every allocation is visible in the code. Cleanup happens deterministically when values go out of scope. For I/O resources, the `ensure` keyword guarantees cleanup even on early returns.

### The Core Ideas

Getting the ergonomics right without sacrificing transparency took a lot of iteration. Here's what I landed on:

| Concept | What It Means |
|---------|--------------|
| **Value semantics** | Everything is a value, no hidden sharing |
| **Single ownership** | Every value has one owner, cleanup is deterministic |
| **Scoped borrowing** | Temporary access that can't escape scope |
| **Handles instead of pointers** | Collections use validated indices (pool ID + generation check) |
| **Linear resource types** | Files, sockets must be explicitly consumed—can't forget to close them |
| **No function coloring** | I/O operations just work, no async/await split |

---

## What This Costs

I'm not pretending there aren't tradeoffs. Here's what you give up:

**Handle overhead:** Accessing through handles costs ~1-2ns (generation check + bounds check, needs actual benchmark proof). In most code this doesn't matter. In tight loops processing millions of items, copy data out and batch process.

**Restructuring some patterns:**
- Parent pointers → store handles
- String slices in structs → store indices or use StringPool
- Arbitrary graphs → use Pool<T> with handles

**More `.clone()` calls:** In string-heavy code (CLI parsing, HTTP routing) you'll see ~5% of lines with an explicit clone. I think that's better than lifetime annotations everywhere.

In return, you get:
- No use-after-free, no dangling pointers, no data races
- No lifetime annotation burden
- No GC pauses
- No borrow checker fights
- Function signatures that are actually readable

---

## Implementation Status

**Right now:** Everything runs interpreted. The lexer, parser, type checker, and interpreter work for the core language features. Three of the five litmus test programs run (grep, game loop, text editor). It might be buggy, haven't have time testing everything.

**What works:**
- Memory model: ownership, moves, borrows, handles
- Type system: primitives, structs, enums, generics, traits
- Control flow: if/match/loops with explicit returns
- Concurrency: spawn/join, channels, thread pools
- Resource types: files must be closed, linear tracking works
- Error handling: `T or E` results, `try` propagation
- Standard types: Vec, Map, Pool, String, Option, Result

**What's next:**
- Code generation (right now it's all interpreted)
- Network I/O (HTTP server example is blocked on this)
- SIMD and const generics (embedded example needs this)
- LSP completion and real IDE support

---

## Design Principles

1. **Transparency of Cost** — Major costs visible in code (allocations, locks, I/O)
2. **Mechanical Safety** — Safety by construction, not runtime checks
3. **Practical Coverage** — Handle 80%+ of real use cases
4. **Ergonomic Simplicity** — Common patterns should be low ceremony.

The constant balancing act is keeping ergonomics high without hiding costs. When in doubt, I choose visibility over convenience.

## Inspiration

Rask borrows ideas from across the systems language landscape:

**From Rust:** Ownership, move semantics, Result types, traits. Don't fix what isn't broken.

**From Go:** The focus on simplicity and getting out of the developer's way. If Rask needs 3+ lines where Go needs 1, something's wrong.

**From Zig:** Compile-time execution (`comptime`) and transparency of cost. I want you to see where allocations happen.

**From Jai:** Build scripts as real code. In Rask, `build.rk` files use the actual language, not some separate format.

**From Swift:** `defer` became `ensure` for guaranteed cleanup. When a function can exit early, resources still get freed.

**From Kotlin:** Extension methods (`extend` blocks) and `T?` syntax for optionals. I rejected the implicit scope functions though—Rask uses explicit closure parameters instead.

**From Hylo:** Value semantics rather than pointer chasing. Where Hylo chooses the academic approach, Rask is pragmatic.

**From Vale:** Vale proved that generational references are a valid memory model. I just made it less necessary to use in most code.

**From Erlang:** Bitmatch and Supervision pattern. When you need it it is irreplaceable.


---

## Documentation

For developers: See the [book](https://rask-lang.dev).  
For language designers, see the lanugage specification in [specs/](specs/) directory.

### Project Structure

```
├── CORE_DESIGN.md          # Design philosophy and core mechanisms
├── METRICS.md              # How I measure whether the design works
├── TODO.md                 # What's done, what's next
├── specs/                  # Language specifications
│   ├── types/              # Type system, generics, traits
│   ├── memory/             # Ownership, borrowing, resources
│   ├── control/            # Loops, match, comptime
│   ├── concurrency/        # Tasks, threads, channels
│   ├── structure/          # Modules, packages, builds
│   └── stdlib/             # Standard library APIs
├── compiler/               # The actual implementation
│   ├── rask-lexer/         # Tokenization
│   ├── rask-parser/        # AST construction
│   ├── rask-types/         # Type checking
│   ├── rask-interp/        # Interpreter (current execution)
│   └── ...
└── examples/               # Real programs that run today
    ├── grep_clone.rk
    ├── game_loop.rk
    ├── text_editor.rk
    └── ...
```

---

## License

Licensed under either of Apache License or MIT license at your option.
