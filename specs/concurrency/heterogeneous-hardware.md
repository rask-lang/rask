<!-- id: conc.heterogeneous -->
<!-- status: proposed -->
<!-- summary: Map and decision record for the heterogeneous-hardware story — Wide data parallelism, device-as-async-executor, the backend contract -->
<!-- depends: concurrency/data-parallel.md, concurrency/wide-backend.md, control/comptime.md, concurrency/async.md -->

# Heterogeneous Hardware — Model and Decisions

This is the entry point and the decision record for Rask's GPU/accelerator story. The two sibling specs — `conc.data-parallel` (the `Wide` algebra) and `conc.wide-backend` (the backend contract) — were written earlier in the design and some of their framing is now superseded. Where they conflict with this file, **this file is current**. See [Status of the sibling specs](#status-of-the-sibling-specs).

Everything here is `proposed`. Nothing is decided. Accelerators may not even be in Rask's target — that's a `CORE_DESIGN` call (open question O8).

## The layers

Top to bottom, each layer knows nothing about the one below it except through a narrow interface:

| Layer | What it is | Where it lives |
|-------|-----------|----------------|
| **Paradigm spine** | A *thin* pattern: stage a description → submit → await, scoped by a device resource. A convention plus one compiler hook — not a framework. | Language (mostly convention) |
| **`Wide` algebra** | *One* paradigm on the spine: data-parallel primitives (`map`, `reduce`, `scan`, …). Quantum, tensor, etc. would be sibling libraries with their own algebras. | Core + stdlib |
| **Device** | A resource that is an **async executor** (owns queues) **and an arena** (owns memory). | Core resource; drivers in libraries |
| **Backend contract** | The library-implemented interface to real hardware. | Stdlib interface, library impls |
| **Hardware / drivers** | Vulkan, CUDA, remote, HPC — free to churn. | Libraries only |

The one idea holding it together: **the core owns the portable middle (algebra, plan, one kernel IR); everything about reaching a physical device is a library behind the contract.**

## The device model, in one example

The current shape (now folded into `conc.data-parallel` and `conc.wide-backend`):

<!-- test: skip -->
```rask
with Device.gpu(0) as dev {           // device is a RESOURCE — many allowed, non-exclusive
    const xs   = img.to(dev)          // explicit upload → resident buffer on dev
    const plan = xs.map(shade).sum()  // stage — host-side description, nothing runs yet
    const h    = plan.submit()        // enqueue on dev's queue (target = where data lives) — async handle
    const out  = try h.await()        // fence-wait + copy home — this is the blocking point
}                                     // block exit: queue drained, device freed (drain-on-exit)
```

`read()` / `commit()` are sugar for `submit().await()` when you don't want overlap. Submitting several plans before awaiting is how you overlap kernels (the whole point of queues).

## What I decided, and why

| # | Decision | Why |
|---|----------|-----|
| **D1** | A device is a `@resource` acquired with `with`, **not** a `using` runtime context. Non-exclusive — many devices at once. | A `using Multitasking` context exists because a scheduler is a heavyweight, process-global, exclusive runtime. A GPU is none of those — it's a driver handle with memory, and multi-device is normal. Forcing it into the context mold was wrong on every axis. |
| **D2** | "Widths" were a false unification. `Simd` is a codegen choice, `ThreadPool` is a runtime (already exists), `Gpu` is a resource. | Treating all three as one `using Width` created the "is a width exclusive or stackable?" confusion. They're three different kinds of thing. |
| **D3** | The device is an **async executor**: `submit → must-use completion handle → await`. This supersedes the single blocking `commit`. | Hardware is asynchronous: work is submitted to ordered queues, completion is a fence. A blocking `commit` modeled it as synchronous and killed overlap. The async shape (and Rask's must-use handles + drain-on-exit) *encode real driver constraints*: an un-awaited submit is a leaked fence; you can't destroy a device with work in flight. |
| **D4** | Buffer lifetime is **completion-bound**: a buffer is live while any un-awaited handle touches it; the runtime defers free until those complete. | The defining hardware constraint is that lifetime and readiness are tied to *completion*, not to a lexical scope. A pool's generation-check fires at access time — the wrong instant. |
| **D5** | Placement is explicit: data has a location, `.to(dev)` is a visible transfer, ops run where their data lives, `.read()` brings it home. Device *location* is flow-tracked (like a borrow), not a type parameter; *representation* (`Wide[T]` vs `[]T`) is in the type. | Explicit to/from keeps transfers honest (no hidden round-trips). Putting location in the type would re-introduce viral coloring — the exact thing the whole design avoids. |
| **D6** | `Wide` is **one** paradigm on a thin spine. Quantum, tensor, etc. are libraries with their own algebras. The nice properties — CPU baseline, reference semantics, determinism, subset guarantee — are **`Wide`-specific**, not spine properties. | Those properties come from data-parallelism being well-behaved (GPU-expressible ⊂ CPU-expressible). Quantum has no cheap CPU superset; its "reference" is an exponential simulator. The spine itself is thin — stage/submit/await/scope. |
| **D7** | Comptime is for **specialization** (reflect over types, unroll, select) — it cannot create types (CT66) or add syntax. The one deep compiler service is **foreign codegen**: lower reachable Rask to a portable kernel IR (SPIR-V default). | Comptime does most of what macros do *safely*, but not syntactic extension and not code-body introspection. The codegen hook is the comptime/macro frontier, and it's the only thing `Wide`-style embedded-closure paradigms need from the compiler. Vocabulary paradigms (quantum gates, tensor ops) need nothing new. |
| **D8** | Core owns the algebra, the subset check, the plan, and lowering to one portable IR; ships only `Simd`/`ThreadPool` backends; links **no** vendor SDK. GPU drivers, multi-GPU, remote, HPC — all libraries behind the backend contract. | This is how Rust/C++ actually do it (Thrust/Kokkos = algebra as library; wgpu/cust = backend as crate; MPI = cluster as library). Keeps the core vendor-neutral and lets a remote/HPC backend exist without the compiler knowing clusters exist. |
| **D9** | Pre-v1: **no backwards-compat burden.** Optimize the shape, not stability. | Nobody uses the language yet; we can change anything. The additive-versioning material in `conc.wide-backend` (N5, compat policy) is premature and demoted. |
| **D10** | **Queues: one per device now, many later.** v1 gives a device a single serialized queue (submits run in order, correct and trivial). The growth path is many queues, each **owned by one task** — so the driver's "one queue, one thread" rule is enforced by ordinary Rask ownership — with a buffer **bound to its queue** and cross-queue sharing an **explicit** transfer. Auto-syncing floating buffers is rejected. | A GPU talks through a rail of work-orders; work in one rail is single-file. Multiple rails let copy overlap compute (2–3× on data-heavy work) — but a rail is *externally synchronized*: two threads on one rail corrupt the driver. Making a queue single-owner turns that driver race into a rule Rask already enforces. Auto-sync across floating buffers would be fast to write but reintroduce the invisible dependency-tracking that makes Python GPU code a debugging nightmare — so it's out. Simple first (one queue), honest-and-fast later (many owned queues), never magic. |

## What's `Wide`-specific vs. general (don't confuse them)

The spine generalizes; the *goodness* mostly doesn't. Keep this straight or the next paradigm inherits promises the spine can't keep:

- **General (spine):** stage → submit → await; device as resource; explicit placement; must-use handles; completion-bound lifetime; the backend contract.
- **`Wide`-only:** CPU-is-baseline, CPU-is-reference-semantics, determinism, the one-subset-checked-once portability guarantee. All gifts of data-parallelism.
- **Out of scope entirely:** streaming / reactive workloads (AR, audio DSP, control loops). `stage → submit → await` is a *batch* model; a standing per-frame pipeline is a different shape (closer to channels) and this design does not cover it. AR was mis-shelved as a paradigm earlier — it isn't one of these.

## Open questions, ranked

Hardest and most design-shaping first:

1. **O1 — the queue model. → Direction decided (D10), details deferred.** Shape settled: one serialized queue per device for v1; the growth path is many task-owned queues (ownership prevents the external-sync race), buffer bound to its queue, cross-queue an explicit transfer, no auto-sync. Still to pin down for the Path-2 growth work: how a task opens a second queue, what the explicit cross-queue transfer looks like, and how the flow-tracker distinguishes *which queue* (not just which device) a buffer belongs to.
2. **O2 — sub-buffer views and aliasing.** Device code slices buffers and aliases sub-regions; hazard tracking for overlapping views has no clean story yet.
3. **O3 — handle escape safety.** Buffer outliving its device: compile-time (borrow-style, non-escaping) or runtime (generation-check)? A real fork with an ergonomics/guarantee tradeoff.
4. **O4 — memory kinds.** Device / pinned-host / unified / per-workgroup-shared are different with different transfer/access semantics. Surface them or hide them?
5. **O5 — cross-paradigm resident dataflow.** Can one device run a `Wide` plan and a tensor plan back-to-back without a host round-trip? (Physically yes; needs the backend to serve multiple paradigm algebras.)
6. **O6 — divergence visibility.** Per-lane branch divergence still has no source-level representation — the open transparency gap from `conc.data-parallel`.
7. **O7 — float reassociation.** If CPU is the reference semantics, define what "matches" means for floating-point reductions (bit-exact vs tolerance).
8. **O8 — scope.** Are accelerators in Rask's stated target at all? A `CORE_DESIGN` decision, not this file's.

## Status of the sibling specs

Both siblings have now been reconciled with the decisions here — the fold-in is done:

- **`conc.data-parallel`** — device-as-resource (D1/D2), `submit`/`await` (D3), explicit flow-tracked placement (D5), and completion-bound lifetime references are in. The `Wide` algebra, primitives, coloring, observability, and core/library split were already current.
- **`conc.wide-backend`** — the async-executor model (D3/D4) is folded in: `submit` returns a must-use `Submission`, `await`/`detach` consume it, `release` drains, and N5 is now "completion is the lifetime boundary." Versioning (old N5, compat TODO) is demoted per D9.

What's left is not a writing pass but a *design* pass: the open questions below, chiefly O1. This file stays the map; the siblings carry the detail.
