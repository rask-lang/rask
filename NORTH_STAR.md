# North Star

Rask is a systems language for a world where code is increasingly written by machines and owned by people. Fast, safe, no GC — and every design decision graded against one question: **does this make the feedback loop stricter, faster, or more reproducible?**

Rust proved that a compiler can carry correctness. It also front-loads its complexity into deep, composing concepts (lifetimes, variance, `Pin`) and pays for zero-cost purity with slow feedback. Go proved that simplicity wins adoption, and pays with GC pauses, nil, and silent resource leaks. Rask takes the third position: **strict like Rust, local like Go, reproducible like neither.**

## The four commitments

Every feature, rule, and tool decision must serve these. When two conflict, they're listed in priority order.

**1. Maximum static checking per millisecond of feedback.**
Soundness matters most when the author is a machine iterating against the compiler. But a check is only as good as the loop it runs in: analysis stays function-local so checking is fast and errors point at the code you just touched — never three functions away. Local analysis is a latency promise, not an implementation detail.

**2. No invisible knowledge.**
Anything the compiler knows about your code is either written in the source or one CLI call away — never trapped in an IDE overlay. Diffs are reviewed as text; agents read text; the source is the single source of truth. Ghost annotations are a rendering of this knowledge, not its home.

**3. No unreproducible failures.**
Every failure a Rask developer meets is one of two things: a compile error (fix it now), or a deterministic runtime failure that replays from a seed (fix it with the repro in hand). Nothing flaky, nothing "works on my machine," no UB ever. Memory unsafety doesn't become silent corruption — it becomes a deterministic panic with a name and a location. See [specs/determinism.md](specs/determinism.md).

**4. Every rule teaches.**
A rule the compiler enforces is a rule the compiler explains: diagnostics cite the spec rule they enforce, say why, and show the fix as code. The error messages are the tutorial — for people and for models. A confusing error is a bug.

## What this rules out

- Softening sound compile-time rules into advisory lints. The runtime backstop exists for what static analysis *can't* prove, not for what it won't.
- Whole-program analysis, however tempting the precision. It breaks commitment 1.
- Compiler knowledge that only surfaces in an IDE. It breaks commitment 2.
- Hidden nondeterminism in language or stdlib semantics. Each source is enumerated and disposed of in the determinism contract.
- Complexity justified by "it's zero-cost." Rask doesn't promise zero-cost; it promises visible cost. We don't pay Rust's complexity tax for a purity we don't sell.

## How decisions get made

Measure before changing. The instrument is real programs plus the agent benchmark: have models write Rask against the compiler, measure convergence, and read the failure transcripts. A rule that models and people converge on quickly is earning its keep; a rule they thrash on is a redesign candidate — with data, not taste. Shelved ideas (runtime unification of the borrow rules, relaxed transparency) come back only through that door.
