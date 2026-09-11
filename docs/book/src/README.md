<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/rask-logo-white@3x.png">
    <source media="(prefers-color-scheme: light)" srcset="assets/rask-logo-dark@3x.png">
    <img alt="rask logo" src="assets/rask-logo-dark@3x.png" width="500">
  </picture>
</p>

**Safety without the pain.**

Rask is a systems programming language that sits between Rust and Go:
- Rust's safety guarantees without lifetime annotations
- Go's simplicity without garbage collection

**Status:** Early development with working compiler (Cranelift backend)

## Quick Look

<!-- test: compile -->
```rask
import fs
import io

func search(path: string, pattern: string) -> i64 or io.IoError {
    let content = try fs.read_text(path)
    mut hits: i64 = 0

    for line in content.lines() {
        if line.contains(pattern) {
            println(line)
            hits += 1
        }
    }

    return hits
}
```

No lifetime annotations. No borrow checker fights. No GC pauses.

## Core Ideas

- **Value semantics** - Everything is a value, no hidden sharing
- **Single ownership** - Deterministic cleanup, no GC
- **Scoped borrowing** - Temporary access that can't escape
- **Stored links, not pointers** - Graphs and cycles without lifetime annotations
- **Linear resources** - Files and sockets must be explicitly consumed
- **No function coloring** - I/O just works, no async/await split

## Where to start

> **Note:** Rask is in early development. Expect gaps in the docs and bugs in the compiler.

**New here?** [Install it](getting-started/installation.md), write
[your first program](getting-started/first-program.md), then read the
[Language Guide](guide/README.md). No install needed to look around: the
[playground](/app/) runs Rask in the browser.

**Already writing Rask?** The [language card](https://github.com/rask-lang/rask/blob/main/LANGUAGE_CARD.md)
is the whole language on one page for looking a rule up, the
[example programs](examples/README.md) are complete and CI-checked, and the
[specifications](reference/specs-link.md) are the normative wording when the other two disagree.

## Design Philosophy

Want to understand the "why" behind Rask's design choices?
- [Design Principles](https://github.com/rask-lang/rask/blob/main/specs/CORE_DESIGN.md)
- [How design questions get decided](https://github.com/rask-lang/rask/blob/main/specs/RULINGS.md)
- [Formal Specifications](reference/specs-link.md)
- [Blog](../blog/) - Development updates and design discussions
