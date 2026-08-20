<!-- id: analysis.c3-lessons -->
<!-- status: proposed -->
<!-- summary: C3 checked against Rask — take Ada-shaped type constraints and Raido's arena frame, reject the safe/fast mode split -->
<!-- depends: memory/allocators.md, memory/pools.md, memory/relocatable.md, types/integer-overflow.md, types/type-aliases.md, structure/c-interop.md, tooling/annotate.md -->

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
| `@require` / `@ensure` contracts on declarations | Nothing — never specified, never rejected | **Take the Ada shape instead** — constrain the type, not the function |
| Precondition failure reported at the caller | Bound violations already report at the call site (`type.generics/G2`) | **Take the diagnostic shape** |
| Thread-local temp arena, freed at `@pool` scope exit | `using Allocator` plumbing, no bulk-free region (`mem.alloc`, still proposed) | **Take it** — Raido's frame model, with escape caught at compile time |
| Zero-ceremony C consumption (include the header, call it) | `compile_c()` plus generated bindings (`struct.c-interop`) | **Open** — worth measuring the gap |
| Safe/fast modes; violated contract is UB in fast mode | One behavior in all builds (`type.overflow/OV4`) | **Reject** |
| No destructors; cleanup is manual `defer` | `ensure` blocks plus `@resource` linearity (`mem.linear/L1–L6`) | **Rask already wins** |
| Arenas as the memory-safety story | `Pool` + generational `Handle` catches stale access | **Rask already wins** |
| Macro system (`$if`, `$foreach`, `#expr`, `@macro`, `$$builtin`) | No macros; `comptime` over typed values | **Cautionary tale** |

## Take: constraints, Ada-shaped rather than C3-shaped

C3 puts pre- and postconditions on the function:

```c3
<* @require x > 0 : "positive only" @ensure return >= x *>
fn int grow(int x)
```

Nothing in Rask does this, and nothing ever rejected it — there's no
`rejected-features.md` entry, no spec, no issue, no deleted draft. It's an
unexamined gap, not a settled question.

But before copying C3's shape, note that Ada and VHDL — the languages that actually
made constraints work in production — put them somewhere else. Ada's `subtype Positive is
Integer range 1 .. Integer'Last` and VHDL's ranged subtypes constrain the **type**, not the
function. The difference decides where the check runs and who pays for it:

| | Constraint on the function | Constraint on the type |
|---|---|---|
| Where checked | every call site | once, where the value is built |
| Who pays | every caller, forever | the boundary, once |
| What it can say | relations between arguments, state | one value's own range or predicate |
| Travels with the value | no | yes |

The type-side version is the one Rask is already shaped for. `type Port = u16` exists
(`type.aliases/T1`), its constructor `Port(x)` exists (`T7`), and `primitives/CV11` already
established the idiom of a fallible narrowing that hands you `T or ConvertError` so the
caller picks `try`, `!`, or `catch`. Adding a predicate to the nominal type makes the
constructor fallible and nothing else changes:

<!-- test: skip -->
```rask
type Port = u16 where 1..=65535

let p = try Port(raw)          // checked here, once
listen(p)                      // no check — the type is the proof
```

That's cheaper than a contract, needs no new checking phase, and puts the check exactly
where Rask already puts its others: at the boundary where a value is constructed.

Function-level `@require` remains worth having for what a type can't say — a relation
between two parameters (`lo <= hi`), or a condition on `self`'s state. That's the tail case,
not the headline, and it's where the C3 semantics must be refused: C3 makes a violated
contract unspecified behaviour the optimizer may assume away in fast mode, so one source
file means two different programs. `type.overflow/OV4` exists to forbid exactly that. A Rask
contract is checked in every build, or it's a lint — both fine, the C3 middle position is
not. (Ada's `Pre`/`Post` are also where SPARK does static proof. Not on Rask's table.)

## Take: a scoped arena — and Raido already designed the missing half

C3's answer for scratch memory is two moves. Functions that allocate take an allocator as
their first parameter, and anything intermediate goes on a thread-local temp arena that
rewinds at the closest `@pool` block:

```c3
fn void render(Allocator alloc) {
    @pool() {
        String tmp = string::format(tmem, "%d items", count);  // no free needed
        ...
    };  // temp arena rewinds here
}
```

`mem.alloc` covers the plumbing — `using Allocator` threads a non-default allocator without
touching signatures, `Vec<T, Global>` keeps the zero-cost default. What's missing is the
bulk-free region itself.

Raido already specified that region, and specified it better than C3. `raido.vm/architecture`
has `frame_begin()` saving `top` as `frame_base` and `frame_end()` resetting to it —
everything below the marker persists, everything above dies. It also names the failure C3
ignores: a frame-local value stored into persistent state is a dangling offset after
`frame_end()`, and `debug_frame_writes: true` catches it by validating that any offset in a
stored value sits below `frame_base`, raising `FrameStoreViolation`.

So the three positions on escape are:

| | Rewind | Escape caught |
|---|---|---|
| C3 temp allocator | yes | never — silent use-after-free |
| Raido arena | yes | runtime check, opt-in, off by default |
| Rask | yes | **compile time, no check to run** |

Rask's column is free because of `mem.relocatable/NP2`: a value holds owned values and
integer handles, never an address. Nothing can *store a pointer* into arena memory, so the
only way to escape the region is to move the owning value out of the block — and that's
ownership, which the compiler already tracks. Scope-binding arena values the way borrows are
already scope-bound (`mem.borrowing`) closes it with no runtime cost and no build-mode split.

<!-- test: skip -->
```rask
with arena.scope() {
    let msg = "{count} items"      // arena-allocated
    log(msg)
}                                  // whole region rewinds; escaping msg is a compile error
```

Two details to take from Raido verbatim. **Fixed size, no auto-grow** — its stated reason is
"hides allocation cost", which is principle 1 in Raido's own words; a growing arena is an
invisible malloc. And **exhaustion is an error, not a panic**, at a deterministic point.

This is also the case `Pool` doesn't serve: `Pool` is long-lived identity with generational
handles, not scratch that dies at the end of a frame or a request.

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
- [../../projects/raido/vm/architecture.md](../../projects/raido/vm/architecture.md) — `frame_begin`/`frame_end` and the escape check
- [../types/type-aliases.md](../types/type-aliases.md) — the nominal type a range constraint would hang off
