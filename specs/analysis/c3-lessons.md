<!-- id: analysis.c3-lessons -->
<!-- status: proposed -->
<!-- summary: C3 checked against Rask — contracts stay out, the scoped arena already exists, Raido names the one rule it's missing -->
<!-- depends: memory/allocators.md, memory/pools.md, memory/relocatable.md, types/integer-overflow.md, structure/c-interop.md, tooling/annotate.md -->

# C3, Checked Against Rask

**Result: almost nothing transfers.** Two of the three things I went in thinking were gaps
turned out to be a settled decision and an existing spec rule. What survives is one
unspecced rule (an arena rewind marker, and it comes from Raido, not C3) and one unmeasured
number (C interop ceremony). Recorded so nobody reruns this.

C3 (Christoffer Lernö, at 0.8.x, LLVM backend, effectively one compiler author) is the only
contemporary language that keeps C's procedural feel as a hard constraint rather than a
starting point it grows out of. Its stated principles are worth quoting because they're
unusually disciplined: procedural "get things done", stay close to C, full C ABI, *data is
inert*, avoid big ideas.

That last pair is the interesting part. "Data is inert" means no constructors, no
destructors, no RAII, no hidden control flow on scope exit — a struct is bytes and nothing
happens to it that you didn't write. Everything C3 adds is bolted beside that rule, never
through it: modules instead of headers, slices with a length, `defer`, optionals plus
faults, contracts, `$`-sigiled compile-time execution, explicit allocators, operator
overloading (arrived 0.7.1), and a safe/fast compile-mode switch.

Rask and C3 are answering the same question from opposite ends. C3 reads lifetime bugs as a
**scoping** problem: pick the right arena, free the whole arena at once, stop thinking about
individual frees. Rask reads them as an **aliasing** problem: ban stored references and the
question of who still points at freed memory can't be asked. Those aren't rival answers to
one question — they solve different halves, which is why C3 is a useful source and not a
competitor to argue with.

## Scorecard

| C3 feature | Rask today | Verdict |
|------------|-----------|---------|
| `@require` / `@ensure` contracts on declarations | Nothing — and no written record of why | **Skip** — always-checked is a straight tax, and C3's escape hatch is the mode split |
| Single-value range constraint (Ada `subtype`, VHDL) | Hand-written validating constructor, `primitives/CV11` shape | **Naming convenience only** — same runtime check either way |
| Thread-local temp arena, freed at `@pool` scope exit | Already specced — `mem.alloc/AL12–AL13`, `Arena.scoped` (unimplemented) | **Have it** — the gap is a rewind marker, one rule |
| Zero-ceremony C consumption (include the header, call it) | `compile_c()` plus generated bindings (`struct.c-interop`) | **Open** — worth measuring the gap |
| Safe/fast modes; violated contract is UB in fast mode | One behavior in all builds (`type.overflow/OV4`) | **Reject** |
| No destructors; cleanup is manual `defer` | `ensure` blocks plus `@resource` linearity (`mem.linear/L1–L6`) | **Rask already wins** |
| Arenas as the memory-safety story | `Pool` + generational `Handle` catches stale access | **Rask already wins** |
| Macro system (`$if`, `$foreach`, `#expr`, `@macro`, `$$builtin`) | No macros; `comptime` over typed values | **Cautionary tale** |

## Skip: contracts — the check is the problem

C3 puts pre- and postconditions on the function:

```c3
<* @require x > 0 : "positive only" @ensure return >= x *>
fn int grow(int x)
```

Nothing in Rask does this. There's no `rejected-features.md` entry, no spec, no issue, no
deleted draft — so the reasoning was never written down, only remembered. It comes down to
one line: **you'd have to check it at runtime.**

That objection holds, and C3 proves it by flinching. Its way out is fast mode, where a
violated contract becomes unspecified behaviour the optimizer may assume away — one source
file, two different programs. `type.overflow/OV4` forbids exactly that. So Rask's only
honest options are check-always or don't-have-it, and check-always is where the cost lands:

- **`@ensure` is pure addition.** A postcondition checks the return value on every return,
  forever, for a property the code was supposed to have anyway. Nobody hand-writes that, so
  there's no existing cost it replaces. Straight tax.
- **Relational `@require` (`lo <= hi`) is nearly as bad.** Checked at every call site,
  including the thousand that obviously satisfy it.

The one case the objection doesn't reach is a constraint on a single value's own range —
Ada's `subtype Positive is Integer range 1 .. Integer'Last`, VHDL's ranged subtypes. There
the check runs once, where the value is built, and the value carries the proof afterward; no
call site pays. But Rask already writes that check by hand:

<!-- test: skip -->
```rask
type Port = u16
func port(raw: u16) -> Port or RangeError {
    if raw < 1 { return RangeError.TooSmall }
    return Port(raw)
}
```

`primitives/CV11` established this shape deliberately — `to()` runtime-checks a narrowing and
yields `T or ConvertError` so the caller picks `try`, `!`, or `catch`. A
`type Port = u16 where 1..=65535` spelling would cost exactly what the function above costs,
because it *is* the function above. That makes it a naming convenience, not a new capability,
and not worth language surface on its own.

So: no contracts. If range-constrained nominal types come up again, the case for them has to
be ergonomic, not safety — the check was always going to run.

## Reject: the safe/fast switch

C3's safe mode inserts bounds checks, null checks, overflow checks and contract asserts;
fast mode removes them. Rask decided the other way — overflow panics in release too
(`OV1`, `OV4`) — and the reason holds up: a mode switch means the program you tested isn't
the program you shipped, and every bug report starts with "which mode". C3 pays for
compatibility with C's expectations here. Rask doesn't have that debt.

## The scoped arena already exists — Raido names what's still missing

C3's answer for scratch memory is two moves: functions that allocate take an allocator as
their first parameter, and anything intermediate goes on a thread-local temp arena that
rewinds at the closest `@pool` block.

I went in thinking Rask lacked the region and needed a spec for it. It doesn't. `mem.alloc`
already has all of it — `AL12` (`using expr { body }` sets the allocator for the block),
`AL13` ("values allocated in a `using` allocator block cannot escape that block"), and
`Arena` in the standard-allocator table as a bump allocator with bulk free on drop. Its own
example is C3's use case, written out:

<!-- test: skip -->
```rask
using Arena.scoped(1.megabytes()) {
    let scratch = Vec.new()       // Arena — cannot escape this block
    scratch.push(1)
    // return scratch             // COMPILE ERROR: arena-scoped, cannot escape
}
// arena freed, all scratch memory gone
```

**AL13 is its own rule, not a consequence of borrowing.** Worth stating, because the obvious
guess is wrong: `mem.borrowing/S3` ("cannot store in struct, return, or send cross-task")
governs *views*, and an arena-allocated `Vec` is an owned value, not a view. Ownership would
happily let you return it. AL13 exists precisely because nothing else stops that.

The three positions on escape:

| | Rewind | Escape caught |
|---|---|---|
| C3 temp allocator | yes | never — silent use-after-free |
| Raido arena | yes | runtime check (`debug_frame_writes`), opt-in, off by default |
| Rask | yes | compile time, `mem.alloc/AL13` |

What Raido has that `mem.alloc` doesn't is the **rewind marker**. `Arena.scoped` ties the
arena's life to one block: enter, allocate, free the whole thing. Raido's
`frame_begin()`/`frame_end()` keeps the arena and everything below the marker alive and
rewinds only what the frame added — so a game loop acquires backing memory once and rewinds
per frame, rather than acquiring and releasing per frame. `mem.alloc` mentions
`alloc.reset()` once in prose (the AL8 aside) and never gives it a rule. That's the gap, and
it's one rule wide.

Two details Raido states better than C3, worth carrying into that rule: **fixed size, no
auto-grow** (its reason — "hides allocation cost" — is principle 1 restated; a growing arena
is an invisible malloc) and **exhaustion is a deterministic error, not a panic**.

This stays an add-on. `Arena` is a stdlib allocator behind the `Allocator` trait (AL1),
reached through `using`, and a reset marker doesn't change that. Nothing here is load-bearing
for the memory model — `Pool` + `Handle` remains the answer for long-lived identity; the
arena is for scratch that dies at a frame or request boundary.

**None of it is built.** `mem.alloc` is `status: proposed`; there is no `Arena` in `stdlib/`
and no AL12/AL13 enforcement in the compiler. The Rask row above is a spec claim, not a
shipped one.

## Where Rask is already ahead

The C3 memory story gets marketed as having solved lifetimes with scopes. It solved *when
to free* — that's real, and it makes leaks cheap to avoid. It did not touch *who still
points at it*. A pointer into a rewound arena is a live use-after-free in C3, silent and
unchecked. Rask's `Pool` handle checks pool id and generation on every access, so the same
mistake is a panic with a message (`mem.pools`, and the trade is written up in
[rust-zig-friction.md](rust-zig-friction.md)). Same for cleanup: "data is inert" means a
forgotten `defer` is invisible, where `@resource` linearity makes a dropped handle a
compile error.

And the macro system is a warning, not a model. C3's metaprogramming needed four sigil
families (`$` compile-time, `#` unevaluated expression, `@` macro, `$$` builtin) to keep
itself readable. Macros are still unspecified in Rask; the lesson is that a macro layer
grows its own grammar, and `comptime` over typed values doesn't.

## Still open: the C interop gap

C3's strongest retention feature is that a C programmer can point it at an existing header
and keep working. Rask goes through `compile_c()` and generated bindings. That's a
defensible design, but the ceremony delta is unmeasured, and "how many lines to call an
existing C library" is the number a C-minded evaluator will actually check.

## On "just do it"

C3's real advantage is that you can write a whole program without thinking about ownership
once. Rask can't offer that — values move, and above 16 bytes you type `.clone()`. That's
the fee for the safety, and it's settled (`mem.value/VS1`).

Where the feel is genuinely at risk isn't the fee, it's concept count at first contact. A
C3 entity list is `Entity[] entities` and a temp allocator: two ideas. The Rask equivalent
can reach for `Pool`, `Handle`, `with`, `using`, and one of `Cell`/`Shared`/`Owned` before
a line runs. Every one of those is justified individually — that's the failure mode.
[complexity-stress-test.md](complexity-stress-test.md) is the right place to keep score,
with C3 as the concept-count baseline rather than Rust or Zig.

## See Also

- [rust-zig-friction.md](rust-zig-friction.md) — same exercise against Rust and Zig
- [complexity-stress-test.md](complexity-stress-test.md) — concept-budget audit
- [../memory/allocators.md](../memory/allocators.md) — AL12/AL13 and the `Arena` allocator
- [../../projects/raido/vm/architecture.md](../../projects/raido/vm/architecture.md) — `frame_begin`/`frame_end` and the escape check
