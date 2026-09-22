<!-- id: analysis.rust-zig-friction -->
<!-- status: decided -->
<!-- summary: External Rust/Zig ergonomics critique checked against Rask — what's gone by construction, what's traded, what's still open -->
<!-- depends: memory/borrowing.md, memory/racks.md, memory/relocatable.md, memory/closures.md, types/generics.md -->

# Rust and Zig Friction, Mapped onto Rask

An external 2026 critique ("The Ergonomics of Safety and Simplicity") catalogs where Rust and Zig actually hurt: Rust front-loads friction into the compiler (borrow checker vs graphs, `Pin`, lifetimes, orphan rule, turbofish), Zig outsources it to the developer (manual vtables, manual closures, `anytype` opacity, no operators). Its closing claim: pushing complexity out of a compiler doesn't eliminate it, it redistributes it.

This doc checks each friction point against Rask's specs and sorts them into three buckets: gone by construction, traded for a different cost, and still open. The third bucket is the one worth rereading.

## Scorecard

| Friction | Whose | Rask's answer | Where |
|----------|-------|---------------|-------|
| Lifetime annotations (`'a`) | Rust | No storable references, so nothing to annotate — borrows are block- or expression-scoped | `mem.borrowing/S3` |
| Turbofish `::<>` | Rust | None — call sites write plain `sort<i32>(v)`; the parser resolves `<` with bounded lookahead instead of a user-facing sigil | rask-parser |
| Borrow checker rejects graphs | Rust | `Rack` + `Link` is the design, not a workaround — a link is storable in a field, and delete nulls every edge aimed at the node | `mem.racks` |
| `Pin` / self-referential structs | Rust | Unrepresentable — the one storable reference is a `Link<T>` into a rack, and rack nodes never move (`mem.racks/RK1`), so a value that holds one is still trivially movable | `mem.racks/RK1`, `mem.relocatable/NP2` |
| Async coloring | Rust | Fibers, not compiled state machines; effects are tooling metadata, not types | `conc.async`, rejected-features.md |
| Orphan rule / newtype tax | Rust | Core four traits owner-only, everything else open; duplicate conformance is a loud use-site error | issue #312 (open) |
| Specialization soundness hole | Rust | Doesn't arise — no lifetimes to erase, no overlapping conformances at all | — |
| Proc macros on token streams | Rust | `comptime` over typed values (`reflect.fields<T>()`), no syn/quote layer | `ctrl.comptime` |
| `anytype` opacity, errors deep in bodies | Zig | Public bounds explicit (`GF1`), checked at the call site; private bounds inferred from the body | `type.generics/GF1–GF2` |
| Manual vtables, `@fieldParentPtr` UB | Zig | `any Trait` is a language-level fat pointer | `type.generics/G7` |
| No operator overloading | Zig | `a + b` expands to `a.add(b)` — a method someone deliberately wrote | `type.generics/G4, OP1` |
| No closures | Zig | Closures with capture mode visible at the use site: `\|x\|` borrows, `own \|x\|` moves | `mem.closures` |
| `Error!Payload` syntax | Zig | `T or E` reads as words; no `Ok`/`Err` wrappers to unwrap | `type.errors`, rejected-features.md |
| Compile errors on unused variables/imports | Zig | Warnings, promotable to errors per-package or in CI | `tool.warnings/W1, W3` |

## Gone by construction

**The two deepest Rust problems share one root, and Rask removed the root.** `Pin` and lifetime annotations both exist because Rust lets programs store a reference to *anything* inside a value, so the compiler has to track what that reference outlives and whether moving the holder invalidates it. Rask allows exactly one storable reference — a `Link<T>` into a rack — and a rack owns its nodes at addresses nothing moves (`mem.racks/RK1`). So the two questions never arise: moving a value that holds a link is a plain copy, because the pointee isn't going anywhere, and there is no lifetime to name, because the rack's is the only one and it outlives every link into it by construction (`mem.racks/RK6`). Crossing out of the process is the one place position has to be recovered, and the container does it (`mem.relocatable/RB1`). And because async is stackful fibers rather than compiled state machines, the self-referential-future problem that forced `Pin` into existence never comes up.

**Zig's worst footguns are manual reconstructions of features it refused.** `@fieldParentPtr` is a hand-rolled vtable that corrupts memory if you copy a field; the callback-struct pattern is a hand-rolled closure. Rask keeps the features and adds the cost markers Zig wanted from their absence: dynamic dispatch is an opt-in `any Trait` you can see in the signature, closure capture is one keyword (`own`) at the use site, and an operator always resolves to an authored method on a concrete type (`OP1`) — never an accidental structural match.

**The `anytype` problem is what gradual constraints were built against.** Public generic functions must state their bounds and violations are reported at the call site, not three layers deep in a library body (`G2`). Private functions get Zig-like sketching ergonomics — omit the bounds, the compiler infers them — but the inference can never leak across a package boundary, the same line `DT1` draws for duck traits.

## Traded, not solved

Three of the essay's complaints apply to Rask on purpose. Naming them beats pretending otherwise.

**The SlotMap critique is answered, and not the way this doc used to say.** The essay's sharpest observation is that Rust developers escape the borrow checker with index-based arenas, trading compile-time safety for silent logical use-after-free. This paragraph used to concede the point on Pool's terms — detection, not impossibility, a generation check at every access, a panic rather than repurposed data.

Racks don't make that trade. `rack.delete(n)` sets every `Link<T>?` field pointing at `n` to `none` before it returns, so for a stored edge the invalid state doesn't exist and a read needs no check at all (`mem.racks/RK3`, RK4). The one case the rack can't reach is a link in a *local*, and that is a compile error at the use, reported as a use-after-free (RK5, E0328). So against the essay's complaint Rask now rejects at compile time exactly where the DIY arena fails silently — which is a better answer than the one written here before, not a worse one.

What's traded is elsewhere: reads are free but an edge *write* touches the target as well as the holder, because the rack records the incoming edge so delete can find it. Measured at ~2.6 ns against ~2.9 ns for a raw pointer store. The old generation-check story is still true of `Pool`, which still ships and is deprecated (rask-lang/rask#908).

**Holding into a growable collection is still restricted — the restriction just got smaller.** `vec[i]` is valid for one expression; multi-statement access needs `with` (`mem.borrowing/B2`). This is the borrow checker's reallocation rule in scoped, teachable form. A developer who wants to keep a reference across an arbitrary region of code will still feel friction; it's shrunk and localized, not deleted.

**Clone ceremony is bought, not owed.** Explicit `.clone()` above the 16-byte threshold is exactly what the essay files under syntactic friction. It's the transparency principle paying its bill — the cost is visible because it exists — and it's settled (`mem.value/VS1`).

## What the surveys say, as opposed to one essayist

The section above checks Rask against a single 2026 critique. That is one person's
view of where Rust hurts. The annual Rust survey asks thousands of people, and it
is worth keeping the two apart — an essay finds the sharpest problem, a survey
finds the *common* one, and they are not the same list.

Read: the [2024 annual survey](https://raw.githubusercontent.com/rust-lang/surveys/main/surveys/2024-annual-survey/report/annual-survey-2024-report.pdf),
2026-09-21. It rates nineteen named problems. Its own wording, in its own order:

> Slow compilation · Subpar debugging experience · High disk space usage ·
> Writing executor-agnostic async code · Splitting code across crates (orphan
> rule) · Not being able to do enough in `const fn` · Large binary size ·
> Interoperating with other languages · Achieving structured concurrency with
> async code · Subpar IDE support · Implementing logic for tuples of various
> sizes · Borrow checker not allowing valid code · Writing correct `unsafe`
> code · Having to implement `Iterator` manually · Opaque compiler error
> messages · Dynamic library plugins · Lacking documentation · Compiler bugs ·
> Slow runtime performance

**No percentages here on purpose.** The figures decompress out of that PDF as
bare arrays and I could not align them to their labels with certainty. A number
attached to the wrong row is worse than no number, so this records the list and
not the ranking. Someone with the HTML version should add them.

What the learning-curve research agrees on across sources is narrower than the
practitioner list and more useful: **the borrow checker, lifetimes, and
unlearning garbage-collected thinking**, at roughly 4–8 weeks to stop fighting
the checker and 3–6 months to productivity. Complexity as a worry for Rust's
future: 45.2% in 2024, 41.6% in 2025.

One item recurs in beginner material that appears on neither list as such:
**`Arc<Mutex<T>>`**. Not either half — the *combining*. Learn `Arc`, learn
`Mutex`, then learn which nests inside which, plus lock ordering.

### Where that leaves Rask

| Struggle | Rask | Shipped? |
|---|---|---|
| Lifetime annotations | Nothing to annotate | **yes** |
| `Arc<Mutex<T>>` composition | `Shared<T, S>`, one type, strategy picks the lock | **yes** |
| `Box`/`Rc`/`RefCell` zoo | `Heap`/`Shared`/`Rack`, chosen by problem not composed | **yes** |
| Executor-agnostic async | No coloring; fibers | no — Phase B, designed not built |
| Structured concurrency with async | Same | no |
| Implementing `Iterator` manually | `Sequence` is a closure | **yes** |
| `const fn` limits | `comptime` runs a large subset of the language | partly |
| Borrow checker rejecting valid code | Still has one. Fewer ways to trip it, not none | n/a |
| Writing correct `unsafe` | Same word, same gate, same difficulty | n/a |
| C/C++ interop | Still there | n/a |
| Unlearning GC thinking | Identical. No non-GC language escapes it | n/a |
| Opaque compiler errors | Claimed as first-class — and #1263 is one that is confidently wrong | aspiration |
| Slow compilation | Frontend is ~70 ms on a 300-line file; unproven at dependency-tree scale | unproven |

**The design conclusion, which is the reason to keep this section.** The two
largest clusters of Rust pain are lifetimes/borrowing and async. Rask targets
both. It has shipped the first and not the second, and the second's answer rests
on a runtime that does not exist yet — which is the same warning the "Still
open" section below already makes about fibers, now with outside evidence
behind it rather than an internal worry.

It also sets the honest public claim. Not "smaller book" and not "simpler than
Go", but: *the thing that takes people four to eight weeks in Rust is not in
this language.* That one is checkable.

## Still open

Where the essay's warnings should keep bothering us:

- **Cross-package conformance (#312) is Rask's seat at the orphan-rule table.** The shape is good — the corruption class Rust's rule prevents is already closed by owner-only core traits, so everything else can be permissive with loud use-site errors on the rare collision. But it's an issue, not a spec, and it must land before the registry sees real use. Until then Rask hasn't actually answered Rust's most-hated restriction; it has a plan to.
- **No turbofish, but the grammar debt didn't vanish** — it moved into parser lookahead heuristics (`looks_like_generic_method_call`). Better trade than a user-facing sigil, until a heuristic misfires on real code; edge cases here deserve tests, because this is exactly where Rust's debt hid.
- **The anti-coloring answer rests on unbuilt machinery.** Fibers-without-coloring is only as good as the Phase B runtime — stackful fibers, safe-point preemption, pluggable reactors — which is decided but unprototyped. Rust shipped state-machine async early and bought `Pin` forever; the lesson cuts the other way too — the fiber model is unproven until the prototype exists.
- **The redistribution law applies here too.** Rask moved complexity out of annotations and into runtime checks, `with` scopes, and a wide spec surface. Whether a developer can hold the mechanisms in their head when they collide is an empirical question — [complexity-stress-test.md](complexity-stress-test.md) exists because the answer isn't obviously yes.

## See Also

- [complexity-stress-test.md](complexity-stress-test.md) — the concept-budget audit this doc's last bullet points at
- [rejected-features.md](../rejected-features.md) — effects, Ok/Err wrappers, supervision
- [Racks](../memory/racks.md), [Relocatable](../memory/relocatable.md), [Closures](../memory/closures.md), [Generics](../types/generics.md)
