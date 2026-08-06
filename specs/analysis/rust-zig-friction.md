<!-- id: analysis.rust-zig-friction -->
<!-- status: decided -->
<!-- summary: External Rust/Zig ergonomics critique checked against Rask — what's gone by construction, what's traded, what's still open -->
<!-- depends: memory/borrowing.md, memory/pools.md, memory/relocatable.md, memory/closures.md, types/generics.md -->

# Rust and Zig Friction, Mapped onto Rask

An external 2026 critique ("The Ergonomics of Safety and Simplicity") catalogs where Rust and Zig actually hurt: Rust front-loads friction into the compiler (borrow checker vs graphs, `Pin`, lifetimes, orphan rule, turbofish), Zig outsources it to the developer (manual vtables, manual closures, `anytype` opacity, no operators). Its closing claim: pushing complexity out of a compiler doesn't eliminate it, it redistributes it.

This doc checks each friction point against Rask's specs and sorts them into three buckets: gone by construction, traded for a different cost, and still open. The third bucket is the one worth rereading.

## Scorecard

| Friction | Whose | Rask's answer | Where |
|----------|-------|---------------|-------|
| Lifetime annotations (`'a`) | Rust | No storable references, so nothing to annotate — borrows are block- or expression-scoped | `mem.borrowing/S3` |
| Turbofish `::<>` | Rust | Same angle brackets, but the parser disambiguates with bounded lookahead; call sites write `sort<i32>(v)` | rask-parser |
| Borrow checker rejects graphs | Rust | `Pool` + `Handle` is the design, not a workaround | `mem.pools` |
| `Pin` / self-referential structs | Rust | Unrepresentable — types hold values and integer handles, never addresses, so every value is trivially movable | `mem.relocatable/NP2` |
| Async coloring | Rust | Fibers, not compiled state machines; effects are tooling metadata, not types | `conc.async`, rejected-features.md |
| Orphan rule / newtype tax | Rust | Core four traits owner-only, everything else open; duplicate conformance is a loud use-site error | issue #312 (open) |
| Specialization soundness hole | Rust | Doesn't arise — no lifetimes to erase, no overlapping conformances at all | — |
| Proc macros on token streams | Rust | `comptime` over typed values (`reflect.fields<T>()`), no syn/quote layer | `ctrl.comptime` |
| `anytype` opacity, errors deep in bodies | Zig | Public bounds explicit (`GF1`), checked at the call site; private bounds inferred from the body | `type.generics/GF1–GF2` |
| Manual vtables, `@fieldParentPtr` UB | Zig | `any Trait` is a language-level fat pointer | `type.generics/G7` |
| No operator overloading | Zig | `a + b` expands to `a.add(b)` — a method someone deliberately wrote | `type.generics/G4, OP1` |
| No closures | Zig | Closures with capture mode visible at the use site: `\|x\|` borrows, `own \|x\|` moves | `mem.closures` |
| `Error!Payload` syntax | Zig | `T or E` reads as words; no `Ok`/`Err` wrappers to unwrap | `type.errors`, rejected-features.md |
| Compile error on unused variables | Zig | Warning, `_` prefix silences | `tool.warnings/W3` |

## Gone by construction

**The two deepest Rust problems share one root, and Rask removed the root.** `Pin` and lifetime annotations both exist because Rust lets programs store memory addresses inside values. Rask doesn't: user-visible types contain owned values and integer handles, never pointers (`mem.relocatable/NP2`). A struct physically cannot point at itself or at anything else, so moving a value is always a plain copy and there is no lifetime to name. The same property that would be a `Pin`-shaped crisis in Rust surfaces here as a feature — pool state serializes and round-trips with handles intact. And because async is stackful fibers rather than compiled state machines, the self-referential-future problem that forced `Pin` into existence never comes up.

**Zig's worst footguns are manual reconstructions of features it refused.** `@fieldParentPtr` is a hand-rolled vtable that corrupts memory if you copy a field; the callback-struct pattern is a hand-rolled closure. Rask keeps the features and adds the cost markers Zig wanted from their absence: dynamic dispatch is an opt-in `any Trait` you can see in the signature, closure capture is one keyword (`own`) at the use site, and an operator always resolves to an authored method on a concrete type (`OP1`) — never an accidental structural match.

**The `anytype` problem is what gradual constraints were built against.** Public generic functions must state their bounds and violations are reported at the call site, not three layers deep in a library body (`G2`). Private functions get Zig-like sketching ergonomics — omit the bounds, the compiler infers them — but the inference can never leak across a package boundary, the same line `DT1` draws for duck traits.

## Traded, not solved

Three of the essay's complaints apply to Rask on purpose. Naming them beats pretending otherwise.

**Stale handles are the SlotMap critique, minus "silent."** The essay's sharpest observation is that Rust developers escape the borrow checker with index-based arenas, trading compile-time safety for silent logical use-after-free. That is Pool's exact territory, and `mem.pools` owns it in as many words: detection, not impossibility. The difference from the DIY version is that detection is guaranteed — every access checks pool id and generation, so a stale handle is a panic with a message, never repurposed data. Dangling is impossible (an integer never points into freed memory), but where Rust would reject the program at compile time, Rask fails at the access. That's the deal every ECS makes; Rask makes it in the language where it can be enforced uniformly, rather than in userland where it's re-implemented per project. The runtime cost of the checks is real and accepted.

**Holding into a growable collection is still restricted — the restriction just got smaller.** `vec[i]` is valid for one expression; multi-statement access needs `with` (`mem.borrowing/B2`). This is the borrow checker's reallocation rule in scoped, teachable form. A developer who wants to keep a reference across an arbitrary region of code will still feel friction; it's shrunk and localized, not deleted.

**Clone ceremony is bought, not owed.** Explicit `.clone()` above the 16-byte threshold is exactly what the essay files under syntactic friction. It's the transparency principle paying its bill — the cost is visible because it exists — and it's settled (`mem.value/VS1`).

## Still open

Where the essay's warnings should keep bothering us:

- **Cross-package conformance (#312) is Rask's seat at the orphan-rule table.** The shape is good — the corruption class Rust's rule prevents is already closed by owner-only core traits, so everything else can be permissive with loud use-site errors on the rare collision. But it's an issue, not a spec, and it must land before the registry sees real use. Until then Rask hasn't actually answered Rust's most-hated restriction; it has a plan to.
- **Angle brackets carry the same grammar debt Rust paid with the turbofish.** Rask pays it in parser lookahead heuristics (`looks_like_generic_method_call`) instead of user-facing sigils. That's the better trade until a heuristic misfires on real code; edge cases here deserve tests, because this is exactly where Rust's debt hid.
- **Unused imports are a hard error (`struct.modules/IM7`).** This is the one Zig-style liveness rule Rask kept, and the essay's "washing dishes mid-dinner" complaint applies verbatim: comment out a call to isolate a bug and the compiler blocks on the now-unused import. Unused *variables* got the warning treatment (`W3`) for exactly this reason; the import rule is the same situation with a colder answer. Cheap to fix mechanically, but it's compiler-enforced tidiness during the phase where tidiness costs the most.
- **The anti-coloring answer rests on unbuilt machinery.** Fibers-without-coloring is only as good as the Phase B runtime — stackful fibers, safe-point preemption, pluggable reactors — which is decided but unprototyped. Rust shipped state-machine async early and bought `Pin` forever; the lesson cuts the other way too — the fiber model is unproven until the prototype exists.
- **The redistribution law applies here too.** Rask moved complexity out of annotations and into runtime checks, `with` scopes, and a wide spec surface. Whether a developer can hold the mechanisms in their head when they collide is an empirical question — [complexity-stress-test.md](complexity-stress-test.md) exists because the answer isn't obviously yes.

## See Also

- [complexity-stress-test.md](complexity-stress-test.md) — the concept-budget audit this doc's last bullet points at
- [rejected-features.md](../rejected-features.md) — effects, Ok/Err wrappers, supervision
- [Pools](../memory/pools.md), [Relocatable](../memory/relocatable.md), [Closures](../memory/closures.md), [Generics](../types/generics.md)
