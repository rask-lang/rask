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

**Status:** Design phase with working interpreter (no compiler yet)

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

No lifetime annotations. No borrow checker fights. No GC pauses.

## Core Ideas

- **Value semantics** - Everything is a value, no hidden sharing
- **Single ownership** - Deterministic cleanup, no GC
- **Scoped borrowing** - Temporary access that can't escape
- **Handles over pointers** - Validated indices for graphs and cycles
- **Linear resources** - Files and sockets must be explicitly consumed
- **No function coloring** - I/O just works, no async/await split

## Get Started

- [Installation](getting-started/installation.md)
- [First Program](getting-started/first-program.md)
- [Language Guide](guide/README.md)
- [Examples](examples/README.md)

## Design Philosophy

Want to understand the "why" behind Rask's design choices?
- [Design Principles](https://github.com/dritory/rask/blob/main/CORE_DESIGN.md)
- [Formal Specifications](reference/specs-link.md)
