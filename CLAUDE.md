# Rask Design Objectives

Keep the documents short and concise, but in the chat you need to explain things to me, I'm not a language architect expert.

## Goal
Systems language where **safety is invisible**—it's just how the language works, not something you fight. Eliminate abstraction tax, cover 80%+ of real use cases.

**Non-negotiable:** The language should feel simple, not safe. Safety is a property, not an experience.

The language should feel elegant and well designed, not an overengineered construction with alot of afterthoughts.

## Core Principles

**1. Transparency of Cost (TC ≥ 0.90)**
- Major costs (allocations, locks, I/O) visible in code
- Small costs (bounds checks, generation checks) can be implicit
- ✅ Explicit: `file.read()`, `arena.allocate()`, `channel.send()`
- ✅ Implicit OK: bounds checks, null checks, generation validation
- ❌ Hidden: silent allocation, implicit copies of large data

**2. Mechanical Safety (MC ≥ 0.90)**
- Safety by structure, not runtime checks
- Bug classes impossible by construction:
  - Use-after-free, double-free, data races
  - Null derefs, buffer overflows, memory leaks
  - Uninitialized reads, stale references

**3. Practical Coverage (UCC ≥ 0.80)**
- Must handle web services, CLI tools, data processing, embedded
- Dynamic data structures where needed
- Not limited to fixed-size programs

**4. Ergonomic Simplicity (ES)**
- Common code paths must be LOW CEREMONY
- Error handling should not dominate every line
- Nested blocks/callbacks for simple operations = design smell
- If you need 3+ lines to do what Go does in 1, question the design
- Inference over annotation where safe
- "Happy path" should read clean; edge cases handled but not in your face

## Design Constraints

See [METRICS.md](METRICS.md) for scoring methodology.

**Must achieve:**
```
TC ≥ 0.90 AND MC ≥ 0.90 AND UCC ≥ 0.80 AND PI ≥ 0.85 AND ED ≤ 1.2 AND SN ≤ 0.3 AND RO ≤ 1.10 AND CS ≥ 5×Rust
```

- **RO (Runtime Overhead)**: Hot paths within 10% of C/Rust. GC/RC/deep-copy must be opt-in.
- **CS (Compilation Speed)**: ≥5× faster than Rust. No whole-program analysis.

**Reject if:**
- Common patterns require more ceremony than the simplest mainstream alternative
- Error handling dominates the code
- Nested callbacks/blocks for simple operations
- The language "gets in the way"
- Covers <80% of real use cases
- Hidden costs (allocations, copies, locks)
- Mandatory runtime overhead (GC/RC/deep-copy) with no opt-out
- Whole-program analysis required for safety

**Ergonomics red flags:**
- Excessive annotations
- Nesting > 2 for routine operations
- Declaring things the compiler could infer
- Validation ceremony on every access

## Inspiration Sources

Draw freely from ANY language. Good ideas are everywhere:

- **Zig**: comptime, no hidden allocations, explicit allocators
- **Odin**: implicit context, simple generics, SOA
- **Jai**: compile-time execution, #run, implicit context
- **Vale**: region-based memory, generational references
- **Koka**: algebraic effects, effect handlers
- **Swift**: optionals, value types, ARC
- **Nim**: macros, effect system, compile-time execution
- **OCaml/F#**: powerful inference, algebraic data types
- **Elixir**: pipes, pattern matching, supervision
- **Forth/Factor**: simplicity, stack-based thinking, concatenative
- **APL/J**: notation as a tool of thought
- **Erlang**: let it crash, supervision trees, message passing
- **Pony**: reference capabilities, deny capabilities
- **Austral**: linear types without borrow checker complexity

**Not limited to:** Rust's borrow checker, Go's GC, C's manual memory. These are options, not requirements.

## Focus Areas (CORE)

**These are what we're designing:**

1. **Memory Model**
   - How is memory owned, shared, and freed?
   - What are the primitives (regions, arenas, RC, GC, linear types)?
   - What invariants does the compiler enforce?
   - What does the runtime need to track?

2. **Type System**
   - What can types express? (ownership, effects, capabilities, linearity)
   - What is inferred vs. declared?
   - How do generics/polymorphism work?
   - What compile-time guarantees are provided?

3. **Concurrency Model**
   - How do threads/tasks share data?
   - Message passing vs. shared memory?
   - What race conditions are impossible by construction?
   - Actor model? CSP? Structured concurrency?

4. **Compiler Architecture**
   - What happens at compile time vs. runtime?
   - Comptime execution? Metaprogramming?
   - How much work can be done statically?
   - Incremental compilation? Build system integration?

5. **C Interop**
   - How do we call C? How does C call us?
   - ABI compatibility?
   - Memory ownership across boundaries?
   - Header generation? Binding generation?

**NOT in scope (yet):**
- Surface syntax (keywords, brackets, etc.)
- Standard library design
- Tooling (LSP, formatter, etc.)
- Error message wording

**Use abstract descriptions, not code:**
- "Values in region R are freed when R exits"
- "Types carry ownership: unique, shared, or borrowed"
- "Cross-thread data requires capability C"

Syntax is bikeshedding. Focus on semantics and compiler guarantees.

## Test Programs

Must naturally express:
1. HTTP JSON API server (dynamic requests/responses)
2. grep clone (arbitrary files)
3. Text editor (dynamic buffer, undo)
4. Log aggregation (streaming data)
5. Sensor processor (fixed memory, real-time)
6. Game loop (dynamic entities)
7. Database (indexes, caching)

**Ergonomics litmus test:** For each program, write the core loop in Rask and Go. If Rask is longer or noisier, justify why or fix the design.


No solution is locked in. Test everything against real use cases.

## Specs Organization

```
specs/
├── types/       # What values can be
├── memory/      # How values are owned
├── control/     # How execution flows
├── concurrency/ # How tasks run in parallel
├── structure/   # How code is organized
└── stdlib/      # Standard library
```

See [specs/README.md](specs/README.md) for detailed index.

## Refine

See [REFINEMENT_PROTOCOL.md](REFINEMENT_PROTOCOL.md) for how to refine.