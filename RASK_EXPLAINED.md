# Rask Explained

A systems language where safety is invisible—it's just how the language works, not something you fight.

## Design Philosophy

**Goal:** Eliminate the abstraction tax. Safety as a structural property, not extra work.

Most languages make you choose: safe but slow (GC), safe but complex (borrow checker), or fast but dangerous (manual memory). Rask takes a different path—safety that emerges from simple rules, not from runtime overhead or annotation burden.

**Core insight:** Most safety problems come from one thing: dangling references. Rask's memory model makes dangling references structurally impossible without requiring lifetime annotations.

## Memory Model

### Value Semantics

All types are values. No distinction between "value types" and "reference types."

**Assignment behavior:**
- Small types (≤16 bytes): copy automatically
- Large types: move (source becomes invalid)

This threshold matches CPU register-passing conventions—copies below it are essentially free. Above it, you explicitly clone when you need a copy, making allocation costs visible.

### Ownership

Every value has exactly one owner. When you create something, you own it. You can:
- Use it
- Pass it to a function (transfer or borrow)
- Let it go out of scope (automatically cleaned up)

No garbage collector runs in the background. No reference counting overhead. When scope ends, memory is freed deterministically.

### Borrowing

Functions can borrow values temporarily. Two flavors:

**Block-scoped borrowing** (plain values): Borrow lives until the enclosing block ends. Multiple borrows can coexist. Mutation blocked while borrows exist.

**Expression-scoped borrowing** (collections): Borrow lives only within a single expression. Released at the semicolon. This allows patterns like:

```
collection[key].field = value    // borrow released
if collection[key].dead {        // new borrow
    collection.remove(key)       // no borrow active—allowed
}
```

Borrows cannot escape—can't store them in structs, return them, or send them cross-task. This eliminates dangling references without lifetime annotations.

### Handles for Dynamic Data

Collections use handles (key + generation counter) instead of pointers. When you get a reference into a collection, you get a handle—a ticket that lets you look up the item later.

If the item is removed, the generation changes. Accessing with a stale handle returns `None` or panics (your choice). No dangling pointers, no use-after-free.

### Linear Types for Resources

Some things must be explicitly consumed: files, sockets, locks. These are linear—the compiler ensures you consume them exactly once.

```
file = open("data.txt")
ensure file.close()        // registers cleanup
data = file.read()         // still usable
// file.close() runs here, guaranteed
```

The `ensure` construct registers cleanup that runs on scope exit, even through early returns or errors.

## Type System

### What Types Express

- **Ownership:** Who owns this value? Can it be copied implicitly?
- **Resource nature:** Is this linear (must consume)? Move-only (no implicit copy)?
- **Size:** Affects copy vs move behavior

### Inference Over Annotation

The compiler infers:
- Whether a type is Copy (based on size and fields)
- Borrow scopes (based on usage)
- Move vs copy at each site

You declare intent in function signatures. The compiler handles the rest.

### Generics

Standard parametric polymorphism with trait bounds:

```
fn process<T: Serializable>(item: T) -> Bytes
```

No lifetime parameters. Generic functions work uniformly over owned values.

## Concurrency Model

### Tasks Don't Share

Each task owns its data. No shared mutable memory. This eliminates data races by construction.

### Channels Transfer Ownership

Sending on a channel moves the data:

```
channel.send(data)    // data is gone from this task
```

The receiving task becomes the owner. No locks needed, no synchronization bugs.

### Sync-First

OS threads and channels cover 80% of concurrency needs. Async is available as an optimization for high-connection scenarios (10k+ connections), not as the default.

### Structured Concurrency

Tasks are scoped. A "nursery" spawns child tasks that must complete before the nursery exits. No orphan tasks, no forgotten background work.

## Compiler Architecture

### Compile-Time Execution

The `comptime` keyword marks code that runs at compile time:

- Compute lookup tables
- Select types based on configuration
- Conditional compilation

Comptime code runs in a restricted interpreter—pure computation only, no I/O or runtime features.

### Local Analysis Only

Type checking and safety verification happen per-function. No whole-program analysis. This enables:
- Fast incremental compilation
- Parallel compilation
- Predictable compile times

### Build Scripts (Separate)

For build-time I/O (code generation, asset bundling), use build scripts—separate programs that run before compilation with full language access.

## C Interop

### FFI Design

Call C functions directly. C can call Rask functions with C-compatible signatures.

At the boundary:
- Raw pointers exist in `unsafe` blocks
- Convert to/from safe types at the edge
- Ownership transfers are explicit

### ABI Compatibility

Rask types can be declared C-compatible for direct struct sharing without marshaling.

## The Tradeoffs

**What Rask makes harder:**
- Graph structures with arbitrary cross-references (use handles or arenas)
- Shared mutable state across tasks (use channels)
- Escaping references (by design—prevents bugs)

**What Rask eliminates:**
- Use-after-free, double-free, dangling pointers
- Data races (impossible by construction)
- Null pointer crashes (optional types)
- Memory leaks for linear resources
- Lifetime annotation burden
- GC pauses and overhead

## Summary

| Concept | What It Does |
|---------|--------------|
| **Value semantics** | Everything is a value, no hidden sharing |
| **Single ownership** | Every value has one owner, deterministic cleanup |
| **Scoped borrowing** | Temporary access, cannot escape scope |
| **Handles** | Safe references into collections |
| **Linear types** | Resources that must be consumed |
| **Task isolation** | No shared mutable memory |
| **Channel ownership** | Sending transfers ownership |
| **Comptime** | Pure computation at compile time |

The result: code that reads like a dynamic language but runs like C, with safety guarantees stronger than most GC'd languages.
