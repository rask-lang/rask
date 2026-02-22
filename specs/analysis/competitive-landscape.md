# Competitive Landscape

Where Rask sits among languages targeting the same niche, why most of them haven't
broken through, and what I think we can learn.

## The Niche

Rask occupies a specific gap: **safe systems programming without Rust's complexity or
Go's GC**. Dozens of languages target some version of this. Most are dead or stalled.
Understanding why matters more than cataloging features.

## Who's Actually in the Race

### Tier 1: Real traction, shipping software

**Zig** — The frontrunner among C replacements. Expected 1.0 in mid-to-late 2026.
Production users include TigerBeetle (database), Bun (JS runtime), and Ghostty
(terminal emulator). Jumped from rank 61 to 42 in language rankings in a single year,
4th most admired language globally. Key strength: seamless C interop (import C headers
directly, no bindings). Key weakness: no memory safety guarantees — it's a "better C,"
not a safe language. Zig explicitly chose *not* to enforce safety at the type level.

**Odin** — The game dev darling. Stable language spec (rarely breaks), custom backend
bypassing LLVM landed in 2026. Validated by JangaFX shipping commercial software
(EmberGen, LiquiGen). Deliberate "joy of programming" philosophy, no package manager
by design. Strength: fastest path from "idea to pixels on screen" for game devs.
Weakness: small community, no safety guarantees, niche positioning limits growth.

**Mojo** — The AI/Python play. Pre-1.0, targeting H1 2026 release. Closed-source
compiler. Initial hype (35,000× benchmarks) cooled significantly — real-world gains
closer to 2-3×. Not a true Python superset (missing classes, comprehensions, `global`).
Python interop carries GIL overhead. Strength: MLIR-based architecture, real
performance for numerical code. Weakness: vendor lock-in risk, closed source, cooling
community interest, "faster Python" pitch narrows the audience.

### Tier 2: Promising but pre-production

**Carbon** (Google) — C++ interop successor, not a standalone language. *Still
experimental*. MVP v0.1 expected late 2026 at earliest, production v1.0 after 2028.
2025 focus was just getting non-template C++ interop working. Strength: Google backing,
clear migration story from C++. Weakness: glacial pace, trust concerns (Google's
abandonment track record), doesn't target greenfield — it's for migrating C++ codebases.

**Nim** — Technically impressive (Python-like syntax, compiles to C, metaprogramming),
but stuck at 0.4% developer share. Took 10 years to reach 1.0 (2019), 2.0 shipped
2023. Strength: elegant syntax, powerful macros. Weakness: small ecosystem, no killer
app, niche positioning, quality-of-implementation concerns persisted through critical
early years.

### Tier 3: Research / stalled

**Vale** — Most conceptually similar to Rask. Generational references (every object gets
a generation counter, every pointer remembers the generation). Regions for zero-cost
borrowing. Creator (Evan Ovadia) now works on Mojo. Last substantive update: July 2023.
At v0.2, was working on v0.3. Strength: novel memory model, academic rigor. Weakness:
effectively stalled, one-person project that lost its person.

**Hylo** (formerly Val) — The closest philosophical competitor. Mutable value semantics
as the organizing principle: all values are independent, sharing is banned in safe code.
References are second-class (parameter passing modes only, never stored). Law of
Exclusivity enforced at call sites prevents aliased mutation. Subscripts (coroutine-like
constructs) yield temporary access without stored references. Backed by research from
Dave Abrahams (C++ STL architect) and Dimitri Racordon (EPFL). Strength: conceptual
purity — one mental model (values) covers everything. Weakness: many hard problems
unsolved — graphs are "use trees or unsafe," concurrency is unspecified, error handling
is missing, stdlib is minimal. If Hylo solves these problems within its purity
constraints, it could be compelling. If it can't, the purity is academic. See [Hylo
Deep Dive](#hylo-deep-dive) below.

**Austral** — Linear types + capability-based security. Borrow checker in under 600
lines of OCaml. Spec-complete compiler exists. Strength: proves linear types can be
simple. Weakness: personal project, no ecosystem, no stdlib, no adoption path.

### Tier 4: Cautionary tales

**D** — The original "better C++" (1999). Technically sound. Failed because: no
big-company backing, proprietary compiler backend, fragmentation (multiple compilers,
language changes), couldn't articulate why you'd switch from C++ for the pain. D proved
that being technically better isn't enough. Peaked at TIOBE rank 12 in 2009, now
outside top 20.

## Why They Haven't Succeeded

The Meyerovich & Rabkin study at UC Berkeley ("Empirical Analysis of Programming
Language Adoption," OOPSLA 2013 Most Influential Paper) surveyed 13,000+ developers and
analyzed 200K+ projects. Key findings:

1. **Libraries are the #1 adoption factor.** Not language features. Not performance. Not
   safety. Open source libraries and existing code dominate language selection.

2. **Intrinsic features are secondary.** Performance, reliability, and semantics don't
   drive adoption directly. Developers prioritize expressivity over correctness.

3. **Adoption follows a power law.** A few languages dominate; the rest serve niches.
   Breaking into the top tier requires extraordinary circumstances.

4. **Safety benefits are hard to observe.** Like clean water or safe sex — the benefit
   is the absence of something. Developers can't directly see bugs that *didn't* happen.

These findings explain the pattern: technically superior languages fail because they
can't bootstrap an ecosystem. The chicken-and-egg problem is real — no libraries means
no users, no users means no libraries.

## What Actually Worked (Success Patterns)

### Rust
Succeeded despite extreme complexity. Why:
- **Solved a visible, painful problem** — memory bugs in Firefox, Android, Windows were
  headline news. The US government recommended memory-safe languages.
- **Mozilla backing** gave it credibility and initial investment. Later: Amazon, Google,
  Microsoft, the Rust Foundation.
- **Cargo was exceptional from day one** — the package manager was better than anything
  in C/C++. Libraries followed.
- **Community was the product** — "most loved language" 7 years running on Stack
  Overflow. The community became a recruiting tool.
- **Killer apps** — Servo, then ripgrep, then Deno, then parts of Android/Windows/Linux
  kernel.

### Go
Succeeded despite deliberate feature omission. Why:
- **Google backing** + credibility of Pike/Thompson/Griesemer.
- **Solved a real problem** — Google's C++ compile times were unbearable. Go compiled
  fast and deployed as a single binary.
- **Killer app** — Docker (2013), then Kubernetes. The entire cloud-native ecosystem
  was built in Go.
- **Opinionated tooling** — `gofmt` ended style debates. `go test` was built in. One
  way to do things.
- **Entire spec fits in your head** — developers became productive in weeks.

### Common Pattern
Every successful language had: (1) a sponsor with credibility and resources, (2) a
killer app that forced adoption, (3) an ecosystem that grew faster than alternatives,
(4) tooling that reduced friction from day one.

No language succeeded on technical merit alone. Not one.

## Where Rask Stands Today

### What's strong

**Design maturity is exceptional.** 73 formally decided specs, 88 documents, measurable
success metrics (Transparency Coefficient, Mechanical Correctness, etc.). The "no
storable references" constraint is novel and principled — not a compromise but a genuine
design choice that eliminates bug classes by construction. Rejected features have
detailed technical justifications. This level of design rigor is rare.

**The interpreter works.** grep clone, text editor with undo, game loop with entities,
sensor processor — all run correctly. This proves the design handles real programs, not
just toy examples. Most language projects at this stage are vaporware.

**Compiler pipeline exists.** 22 well-decomposed crates. Lexer through Cranelift codegen.
4 of 5 validation programs compile and run natively. The frontend is production-quality.

**Tooling is ahead of schedule.** CLI (run, check, build, fmt, lint), package manager
with registry, LSP, benchmark suite. Languages years older than Rask don't have this.

**Documentation is honest.** Tradeoffs acknowledged, gaps listed, design FAQ explains
the "why." The Language Guide is accessible to non-experts.

### What's incomplete

**Codegen bugs block the concurrency story.** `Shared<T>`, `Channel<T>`, and green
`spawn()` generate incorrect allocation sizes. The HTTP server validation program
crashes. These are implementation bugs (not design flaws), but they block the most
important validation: that Rask's concurrency model works end-to-end natively.

**Performance claims are unvalidated.** Handle access "costs 1-2ns" — plausible, but
not benchmarked. Compilation "5× faster than Rust" — not measured. Unvalidated claims
are a credibility risk.

**Stdlib is mostly specified, mostly unimplemented.** Core types work. HTTP, net, TLS,
and most modules are specs without code.

**No ecosystem.** Zero third-party packages. No killer app. No external contributors.

### Honest assessment

Rask is a well-executed design with a working proof of concept. It is not a language
anyone can use for real work yet. The distance between "interpreter runs 5 programs"
and "developers ship production software" is enormous, and that gap is mostly ecosystem,
not engineering.

## What Rask Can Learn

### From the failures

1. **D's lesson: "better X" isn't a pitch.** D was a better C++. Nobody cared enough
   to switch. Rask can't just be "simpler Rust" — it needs to be the best tool for
   *specific* tasks that people actually do. The validation programs (grep, HTTP
   server) point the right direction, but "I can write grep in this" doesn't motivate
   a language switch.

2. **Nim's lesson: time-to-stability matters.** Nim took 10 years to hit 1.0. By then,
   Go and Rust had eaten its lunch. If Rask's codegen bugs and stdlib gaps persist for
   years, the window closes. The current pace of progress is good — don't lose it.

3. **Vale's lesson: one person isn't enough.** Vale had arguably the most similar
   technical vision to Rask (generational references ≈ handles). Its creator got hired
   away and the project stalled. Bus factor of 1 is an existential risk.

4. **Mojo's lesson: hype without substance backfires.** 35,000× benchmarks that don't
   hold in practice, "Python superset" claims that aren't true, closed-source compiler
   — all eroded trust. Rask's honesty about tradeoffs is a strength. Protect it.

### From the successes

5. **Rust's lesson: tooling is the ecosystem seed.** Cargo wasn't an afterthought — it
   was *why* Rust grew libraries faster than alternatives. Rask already has a package
   manager and registry. Making it excellent (not just functional) is high-leverage
   work. If writing and publishing a Rask package is genuinely easier than a Rust
   crate, that's a differentiator.

6. **Go's lesson: compile speed is a feature.** Go developers cite fast compilation as
   a top reason they stay. Rask's local-only analysis should deliver this. Proving it
   with benchmarks matters — it's a concrete, measurable advantage over Rust.

7. **Zig's lesson: C interop is an adoption ramp.** Zig's explosive growth is partly
   because you can incrementally replace C code module by module. Rask has C interop
   via `compile_rust()` and unsafe blocks, but the story isn't as clean. A path where
   someone can mix Rask into an existing C/Rust project *incrementally* lowers the
   adoption barrier.

8. **Odin's lesson: find your people.** Odin didn't try to be everything — it targeted
   game developers, built the right bindings (SDL2, Vulkan), and let JangaFX validate
   it. One company shipping real software in your language is worth more than a
   thousand blog posts.

### Rask-specific opportunities

9. **The "no lifetime annotations" pitch is real.** 45% of Rust developers cite
   complexity as a barrier. Rask's pitch — same safety guarantees for 80% of code,
   none of the annotation burden — directly addresses Rust's biggest weakness. But it
   only works if Rask can demonstrate the safety is real (not just claimed) and the
   performance is acceptable (not just theorized).

10. **The observability problem cuts both ways.** Safety benefits are invisible
    (Meyerovich). But safety *failures* are visible. If Rask can show that common bugs
    in C/Go/Zig programs are structurally impossible in Rask — with concrete examples,
    not abstract claims — that's persuasive. "Here's a real CVE. Here's why it can't
    happen in Rask" is a better pitch than "our type system prevents use-after-free."

11. **Compile speed as identity.** If Rask demonstrably compiles 5-10× faster than
    Rust, that's not just a feature — it's a reason to exist. Fast feedback loops
    change how people write code. This needs to be validated and promoted.

## Hylo Deep Dive

Hylo is the most interesting comparison because it targets the same problem from a
different direction. Both languages reject stored references and lifetime annotations.
Both enforce safety structurally. Both use local-only analysis. But the philosophies
diverge sharply, and the divergence reveals what Rask actually *is*.

### The Core Bet

**Hylo bets on purity:** everything is a value. No aliasing means no aliasing bugs. The
compiler manages copies vs moves behind the scenes (CoW optimization, reference
borrowing). The programmer thinks in terms of independent values and trusts the compiler
to make it fast.

**Rask bets on transparency:** you see the costs. Moves are explicit. Copies require
`.clone()`. The 16-byte copy threshold is fixed and predictable. When you need shared
mutable state, you use Pools with visible handle access costs.

This isn't just an implementation choice — it determines what kind of programmer each
language attracts. Hylo appeals to developers who want fewer concepts. Rask appeals to
developers who want to see what's happening.

### Where Rask Wins

**Graphs and indirection.** This is the clearest gap. Rask's `Pool<T>` + `Handle<T>`
provides a safe, first-class mechanism for entity systems, graph structures, caches, and
observer patterns. Handles are opaque IDs with generation counters (~1ns validation,
eliminable with frozen pools). Hylo's answer is "restructure as trees" or use unsafe
pointers. That's honest, but it punts on a large category of real programs — games,
compilers, UI frameworks, any graph algorithm.

**Resource types.** Rask's `@resource` + `ensure` provides visible, guaranteed cleanup.
Files must be consumed, connections must be closed, the compiler enforces it at every
exit path. Hylo has deinit (RAII-style) but nothing comparable to must-consume
enforcement with recovery patterns.

**Error handling.** Rask has a complete system: `T or E` results, `try` propagation,
union error composition with automatic widening, `T?` optionals. Hylo's error handling
is unspecified.

**Concurrency.** Rask has three spawn mechanisms, channels, `Shared<T>`, task groups,
must-use handles. Hylo is exploring spawn/join-only concurrency (no channels, no shared
state) which preserves value semantics purity but is almost certainly insufficient for
real servers. HTTP handlers sharing a cache, rate limiters, connection pools — these
need more than fork-join.

### Where Hylo Wins

**Conceptual simplicity.** One concept (value semantics) vs Rask's multi-concept stack
(two borrowing tiers, pools, context clauses, frozen pools, generation coalescing). Each
Rask concept is justified individually, but the total weight is real.

**Borrowing.** Hylo has one rule: Law of Exclusivity at call sites. Rask has two tiers
— block-scoped views for fixed-layout sources, statement-scoped views for growable
sources. You need to know which types fall into which tier. Hylo sidesteps this
entirely.

**Subscripts.** Hylo's coroutine-like subscripts yield temporary access to projections.
They compose naturally. Rask's closure-based `modify()` / `with...as` achieves similar
goals but feels like a workaround for scoping constraints rather than a first-class
feature.

**Ownership model.** Adding a field to a struct in Rask can silently change its
semantics from copy to move (crossing the 16-byte threshold). Hylo avoids this entirely
— the compiler decides, and the programmer doesn't need to care.

### What Matters

The comparison is somewhat unfair to Hylo because they haven't confronted the hard
problems yet. Graphs, concurrency, error handling, resource cleanup — these are the
problems that separate "beautiful foundation" from "usable language." Rask has done this
work. Hylo hasn't.

But the risk for Rask is real: if Hylo eventually solves these problems within its
purity constraints, its simpler mental model could win on developer experience. Rask
needs to watch for this and ensure its additional mechanism is justified by additional
capability, not just accumulated complexity.

The honest comparison today: Rask is a pragmatic language with working solutions to hard
problems. Hylo is an elegant theory that hasn't yet been tested against hard problems.
Pragmatism ships software. Elegance publishes papers. Both matter.

## The Hard Truth

Technical merit doesn't determine language adoption. Ecosystem, tooling, sponsorship,
killer apps, and community do. Rask's design is among the most thoughtful I've seen in
this space, but design quality is table stakes — necessary but not sufficient.

The languages that came furthest (Zig, Odin) did so by finding specific communities
with specific pain points and solving them well. Zig owns "better C with great C
interop." Odin owns "joyful game dev." Go owns "simple cloud services." Rust owns
"safe systems programming."

Rask needs to own something specific. "Simpler Rust" is a positioning, not a community.
The question isn't "is Rask well-designed?" (it is) but "who will ship their next
project in Rask instead of what they're using today, and why?"

The answer to that question determines everything else.
