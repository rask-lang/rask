<!-- id: analysis.c3-lessons -->
<!-- status: proposed -->
<!-- summary: C3 checked against Rask — contracts and scoped arenas are worth taking, the safe/fast mode split is not -->
<!-- depends: memory/allocators.md, memory/pools.md, types/integer-overflow.md, structure/c-interop.md, tooling/annotate.md -->

# What Rask Can Learn From C3

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
| `@require` / `@ensure` contracts on declarations | Nothing — `assert` exists only inside `test` blocks | **Take it**, with always-checked semantics |
| Precondition failure reported at the caller | Bound violations already report at the call site (`type.generics/G2`) | **Take the diagnostic shape** |
| Thread-local temp arena, freed at `@pool` scope exit | `using Allocator` plumbing, no scoped-arena primitive (`mem.alloc`, still proposed) | **Take it** as a stdlib arena |
| Zero-ceremony C consumption (include the header, call it) | `compile_c()` plus generated bindings (`struct.c-interop`) | **Open** — worth measuring the gap |
| Safe/fast modes; violated contract is UB in fast mode | One behavior in all builds (`type.overflow/OV4`) | **Reject** |
| No destructors; cleanup is manual `defer` | `ensure` blocks plus `@resource` linearity (`mem.linear/L1–L6`) | **Rask already wins** |
| Arenas as the memory-safety story | `Pool` + generational `Handle` catches stale access | **Rask already wins** |
| Macro system (`$if`, `$foreach`, `#expr`, `@macro`, `$$builtin`) | No macros; `comptime` over typed values | **Cautionary tale** |

## Take: contracts, but always checked

C3 puts pre- and postconditions in the declaration:

```c3
<* @require x > 0 : "positive only" @ensure return >= x *>
fn int grow(int x)
```

Rask has no equivalent anywhere. A precondition today is either a comment or a runtime
`if` that returns an error — and if it's a comment, no tool can see it. Contracts land
squarely in principle 9 (information without enforcement): the compiler knows the
constraint, `rask annotate` and the IDE ghost layer can show it at the **call site**, and a
violation is a diagnostic with an argument name in it instead of a panic three frames down.
The machinery is mostly built — call-site checking for generic bounds already reports where
the bad argument was written.

**Copy the syntax, not the semantics.** C3 makes a violated contract unspecified behaviour
that the optimizer may assume away in fast mode, so the same source means two different
things in two builds. `type.overflow/OV4` exists to forbid exactly that. So: a Rask
contract is checked in every build, or it isn't a contract — it's a lint. Both are fine;
the C3 middle position is not.

## Take: a scoped arena that frees everything

C3's working answer to "where does this string go" is two moves. Functions that allocate
take an allocator as their first parameter, and anything intermediate goes on a
thread-local temp arena that's reset at the closest `@pool` block:

```c3
fn void render(Allocator alloc) {
    @pool() {
        String tmp = string::format(tmem, "%d items", count);  // no free needed
        ...
    };  // temp arena rewinds here
}
```

`mem.alloc` covers the plumbing — `using Allocator` threads a non-default allocator without
touching signatures, and `Vec<T, Global>` keeps the zero-cost default. What it doesn't have
is the scoped bulk-free primitive: a region you allocate into freely and rewind in one
instruction. Rask's shape for it is obvious and already in the language —

<!-- test: skip -->
```rask
with arena.scope() {
    let msg = "{count} items"      // arena-allocated
    log(msg)
}                                  // whole region rewinds
```

— and it's the case `Pool` doesn't serve, because `Pool` is for long-lived identity with
handles, not for scratch that dies at the end of a frame or a request. Worth spec'ing.

## Reject: the safe/fast switch

C3's safe mode inserts bounds checks, null checks, overflow checks and contract asserts;
fast mode removes them. Rask decided the other way — overflow panics in release too
(`OV1`, `OV4`) — and the reason holds up: a mode switch means the program you tested isn't
the program you shipped, and every bug report starts with "which mode". C3 pays for
compatibility with C's expectations here. Rask doesn't have that debt.

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
- [../memory/allocators.md](../memory/allocators.md) — where the arena scope would land
