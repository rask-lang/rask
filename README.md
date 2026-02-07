# Rask

A systems language where safety is invisible—it's just how the language works, not something you fight.

**Status:** Design phase (language specification only, no implementation yet)

## What is Rask?

Most languages force a tradeoff: safe but slow (GC), safe but complex (borrow checker), or fast but dangerous (manual memory). Rask takes a different path—safety that emerges from simple rules, not from runtime overhead or annotation burden.

**Core insight:** Most safety problems come from dangling references. Rask's memory model makes them structurally impossible without lifetime annotations.

### Key Concepts

| Concept | What It Does |
|---------|--------------|
| **Value semantics** | Everything is a value, no hidden sharing |
| **Single ownership** | Every value has one owner, deterministic cleanup |
| **Scoped borrowing** | Temporary access that cannot escape scope |
| **Handles** | Safe references into collections (key + generation) |
| **Linear resource types** | Resources that must be explicitly consumed |
| **Task isolation** | No shared mutable memory between tasks |
| **Comptime** | Pure computation at compile time |

### The Tradeoffs

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

## Where to Start

| If you want to... | Read this |
|-------------------|-----------|
| Understand the core design | [CORE_DESIGN.md](CORE_DESIGN.md) |
| See specific language features | [specs/](specs/) |
| Understand design constraints | [METRICS.md](METRICS.md) |
| See what's still being designed | [TODO.md](TODO.md) |

## Directory Structure

```
├── CORE_DESIGN.md          # Core language design principles
├── METRICS.md              # Design scoring methodology
├── TODO.md                 # Known gaps and open questions
├── REFINEMENT_PROTOCOL.md  # How specs are iteratively refined
├── specs/                  # Language specifications
│   ├── types/              # What values can be
│   ├── memory/             # How values are owned
│   ├── control/            # How execution flows
│   ├── concurrency/        # How tasks run in parallel
│   ├── structure/          # How code is organized
│   └── stdlib/             # Standard library
├── versions/               # Historical spec versions (design evolution)
└── research/               # Archived exploration and experiments
```

## Design Principles

From [CLAUDE.md](CLAUDE.md) (project objectives):

1. **Transparency of Cost** — Major costs (allocations, I/O) visible in code
2. **Mechanical Safety** — Safety by structure, not runtime checks
3. **Practical Coverage** — Handle 80%+ of real use cases
4. **Ergonomic Simplicity** — Common code paths must be low ceremony


# LLMs

Me, a human, has designed this language. I have used Claude code for the heavy-lifting during implementation to accelerate development, and as an assistant during design. AI should be treated as a tool, not a magic quick fix for all your problems. At its best, it can accelerate your workflow hundredfolds. At its worst it can generate thousands of lines of general AI slop. LLM's will only be as smart as the user who prompts them. 

This is a human-directed, AI-accelerated project.

# License

Licensed under either of
- Apache License, Version 2.0 (LICENSE-APACHE or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license (LICENSE-MIT or http://opensource.org/licenses/MIT)
at your option.

