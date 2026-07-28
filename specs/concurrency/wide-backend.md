<!-- id: conc.wide-backend -->
<!-- status: proposed -->
<!-- summary: The stable contract between the Wide core and a device backend — designed to outlive the hardware it targets -->
<!-- depends: concurrency/data-parallel.md -->

# The Wide Backend Contract

A backend is the thing that actually runs a `Wide` plan on some device. The core ships two (`Simd`, `ThreadPool`); everything else — GPU drivers, remote devices, clusters — is a library implementing this contract (`conc.data-parallel/L5`).

This spec is about the *contract*, not any backend. That's deliberate: the contract is the part that can't be cheaply changed once libraries depend on it, while SPIR-V, Vulkan, CUDA, and whatever replaces them in 2032 are details that must be free to churn underneath. **The energy goes into the API so the language stays extendable when the tech turns over.** Status: `proposed`.

Overview and the decisions behind this contract: **`conc.heterogeneous`** (read it first).

## What has to stay stable, and what must be free to change

| Layer | Who owns it | Stability |
|-------|-------------|-----------|
| The `Wide` algebra + the **plan** it produces | Core | The stable currency (shape not frozen — pre-v1) |
| This contract (the trait below) | Core | Deliberately still moving |
| Kernel IR format (SPIR-V today) | Negotiated | A slot, swappable per backend |
| Device drivers, memory tech, transport | Backend library | Free to change entirely |

The load-bearing idea: **the plan is the stable currency.** Core produces a plan; every backend consumes one. As long as that interface holds, the driver underneath can be rewritten, a new accelerator can appear, or a backend can move a device onto the network — none of it reaches the language.

**Pre-v1: no compatibility burden.** Nobody uses this yet, so there is no versioning story to protect — the energy goes into the *shape*, not stability (`conc.heterogeneous` D9). Additive-versioning discipline is a v1 concern; ignore it for now.

## Principles that keep it evolvable

These are the point of the spec. Everything below is in service of them.

| Rule | Description |
|------|-------------|
| **N1: Capability negotiation, not assumption** | The core never assumes what a backend can do — it reads `info()` and asks. New abilities are advertised as *data* (fields on `Capabilities`), so adding one leaves every existing backend valid; it simply doesn't advertise the new field. This is what lets the algebra and backends grow independently — a new primitive (say `sort`) can appear, and a backend that doesn't implement it just doesn't advertise it, so the core routes elsewhere or to the CPU. |
| **N2: Small mandatory core, few optional extras** | The mandatory methods (below) are all a backend must implement to be usable. Everything else — resident results, overlap, real timing — is optional and advertised. Most plans need only the mandatory set, so the "which backend can run this" logic stays simple. |
| **N3: The core never sees device memory** | The contract speaks host data and plans, never device pointers. Whether a transfer is a `memcpy`, a DMA, unified memory, or an RPC to another machine is entirely the backend's business. This is what lets a *remote* backend exist without the contract mentioning networks. |
| **N4: Graceful degradation** | Any optional feature a backend lacks has a defined fallback — usually "materialize to host" or "route to the CPU baseline." A backend that can't run a plan says so with a structured reason; it never fails silently or half-runs. |
| **N5: Completion is the lifetime boundary** | A device is an **async executor**: work is submitted to a queue and finishes later at a fence. So a submission returns a **must-use completion handle** (mirroring `conc.async` task handles), and a buffer stays live until every submission touching it completes — the backend must not free or reuse it before then. Lifetime is bound to completion, not to a lexical scope. This is why the contract is async, not synchronous. |

## The mandatory contract

<!-- test: skip -->
```rask
// The stable currency. Core produces a Plan; every backend consumes one.
// A Plan is the staged algebra (primitives + their pure-function bodies in a
// negotiated kernel IR). Its shape is versioned and additive.

trait Backend {
    // Identity and abilities. Read this before anything else.
    func info() -> BackendInfo

    // Devices this backend can reach right now. May be empty (no GPU present).
    func devices() -> []DeviceInfo

    // What running `plan` on `device` would cost — WITHOUT running it.
    // Powers `explain` (conc.data-parallel/O1). Memory near-exact; timing best-effort.
    func cost(plan: Plan, device: DeviceId) -> PlanCost or BackendError

    // Prepare `plan` on `device`: compile kernels, reserve scratch. Returns a session.
    func prepare(plan: Plan, device: DeviceId, config: RunConfig) -> Session or BackendError
}

trait Session {
    // Enqueue with these inputs. Non-blocking — returns a must-use completion
    // handle. The device starts here. This is what `Wide.submit` calls.
    func submit(inputs: []Input) -> Submission or BackendError

    // Release device resources. Runs when the device resource `with` block exits;
    // the backend must drain in-flight submissions first (a device can't be torn
    // down with work pending).
    func release(take self)
}

trait Submission {
    // Block until this submission's fence passes, then hand back the outputs.
    // This is what `Wide.await` / `.read()` calls. Failure surfaces here (N5).
    func await(take self) -> []Output or BackendError

    // Fire-and-forget: stop tracking, but the runtime still drains it at release.
    func detach(take self)
}
```

| Rule | Description |
|------|-------------|
| **B1: `info` first** | The core calls `info()` before dispatching anything, and honors what it reports — formats, primitives, capabilities. |
| **B2: `cost` without side effects** | `cost()` must not allocate device memory or run kernels. It reports the footprint the core shows in `explain` and checks against the budget before `submit` (`conc.data-parallel/O2`). |
| **B3: `prepare`, then `submit` (many times)** | Compilation and scratch reservation happen once in `prepare`; `submit` is the hot path and may be called repeatedly on one session. Each `submit` returns its own completion handle, so several can be in flight at once (overlap). |
| **B4: must-use handles, total release** | A `Submission` must be `await`ed or `detach`ed — an un-consumed one is a leaked fence, caught the way `conc.async` catches a dropped task handle. `release` always frees cleanly, after draining in-flight work, even following a failed submission. Ties to the *device resource's* `with` block — no leaked device memory across it. |

## Identity and capabilities — as data

<!-- test: skip -->
```rask
struct BackendInfo {
    name: string
    formats: []KernelFormat     // kernel IRs it accepts; SpirV is the default slot
    primitives: PrimitiveSet    // which algebra primitives it can run
    caps: Capabilities          // optional features — additive
}

struct Capabilities {
    resident_results: bool      // can keep outputs on-device between submits (skip host round-trip)
    multi_queue: bool           // offers many task-owned queues (overlap); false = one serialized queue
    timing: bool                // cost() returns measured/modeled time, not just memory
    // Additive: new fields default false. Adding one leaves existing backends valid.
}
```

Capabilities are data, not a zoo of optional traits, precisely so the set can grow without breaking existing backends (N1). The core reads flags and opportunistically uses what's there. `multi_queue` is the queue decision (`conc.heterogeneous` D10): a v1 backend reports `false` (one serialized queue — simple, correct, no overlap); a growth-path backend reports `true` (many queues, each owned by one task, buffer bound to its queue). The core assumes one serialized queue unless `multi_queue` is set.

## Kernel format — a slot, not a commitment

The core lowers pure functions reachable from a plan to a **portable kernel IR** (`conc.data-parallel/D3`). Today that is **SPIR-V**, and SPIR-V is the default format every GPU backend is expected to accept. But the contract names a *format slot*, not SPIR-V specifically:

| Rule | Description |
|------|-------------|
| **F1: Negotiated format** | `info().formats` lists what a backend accepts. The core emits the first it can satisfy. SPIR-V is the default and the reference. |
| **F2: Room for other IRs** | A backend may request a different or richer form — PTX, WGSL, or even raw Rask MIR it lowers itself — by advertising it. Adding a format is additive; it doesn't touch existing backends. |

This is the escape valve for the one hard tech-tie-down. If SPIR-V is ever the wrong currency for some future accelerator, a backend asks for what it needs and the core grows a new emitter — without the language, the algebra, or any other backend changing.

## Memory and residency — host in, host out (with an optional shortcut)

<!-- test: skip -->
```rask
enum Input {
    Host([]u8)          // upload this
    Resident(Token)     // a buffer kept on-device from a prior submission on THIS backend
}

enum Output {
    Host(Vec[u8])       // materialized back to the host
    Resident(Token)     // kept on-device; opaque token, feedable as a later Input
}
```

| Rule | Description |
|------|-------------|
| **M1: Host is the baseline** | By default the core hands host bytes and gets host bytes back. A minimal backend implements only this. It's the most abstract form — it makes no assumption about how memory works, so remote and unified-memory backends fit unchanged (N3). |
| **M2: Residency is an opt-in shortcut** | A backend advertising `resident_results` may return a `Resident(Token)` — an opaque handle the core feeds into a later submission's inputs, skipping a host round-trip. The core never interprets the token; only the same backend does. This is how a multi-stage pipeline stays on the device without the core knowing what a device pointer is. A resident buffer is live under N5 until every submission touching it completes. |
| **M3: Tokens don't cross backends** | A `Resident` token is valid only for the backend and device that issued it. The core tracks that and errors clearly on misuse — it never passes a token to a foreign backend. |

M2 is how a plan keeps data resident across submits (`conc.heterogeneous` O5) — a capability, not a core assumption.

## Cost, for `explain`

<!-- test: skip -->
```rask
struct PlanCost {
    peak_bytes: usize           // near-exact device-memory high-water mark
    transfers: TransferBytes    // host<->device bytes, in and out
    time: Duration?             // best-effort estimate, or none if the backend can't
    fusion: []KernelGroup       // how primitives fuse — feeds explain's kernel view
}
```

The division of labor: the **core** knows the plan's shape (how many buffers, what sizes, in terms of the input `N`); the **backend** knows the per-primitive hardware cost (scratch a reduce needs, alignment, coalescing). `cost()` composes them into the footprint `explain` prints. Memory is near-exact because it falls out of the plan; time is honestly best-effort and may be absent (`conc.data-parallel` says wall-clock stays empirical — the contract doesn't pretend otherwise).

## Errors

<!-- test: skip -->
```rask
enum BackendError {
    NoDevice
    OutOfMemory(needed: usize, available: usize)
    UnsupportedPrimitive(name: string)
    UnsupportedFormat
    DeviceLost
    Transfer(string)
    Internal(string)
}
```

The core maps `BackendError` into the user-facing `GpuError` with a diagnostic — e.g. `OutOfMemory(needed, available)` becomes the attributed, pre-launch OOM message from `conc.data-parallel/O2`. Structured errors in, good messages out; a backend never formats user-facing prose itself.

## How the core picks a backend

When a plan is submitted (or its device resource is acquired), the core:

1. Enumerates registered backends (the built-in host-inline/`ThreadPool`, plus any a library registered).
2. Filters to those that can reach the target device, accept a kernel format the core can emit (F1), and support every primitive in the plan (`info().primitives`).
3. For a **named device** (`Device.gpu(n)`): picks that backend, or fails acquisition with `NoDevice` if none qualifies.
4. For **`Device.fastest()`**: ranks the survivors by `cost()` and picks the fastest — the selection *policy* is replaceable and lives above the contract (library-tunable, not baked in).
5. Calls `cost()` for `explain`, `prepare()` once, `submit()` per plan run (`await`/`detach` on each handle), `release()` when the device resource's `with` block exits.

## What the contract deliberately keeps out

Staying out is how the core stays vendor-neutral and future-proof. None of these belong in the contract:

| Rule | Description |
|------|-------------|
| **X1: No transport** | Remote devices, sockets, cluster topology, MPI-style collectives. A remote backend implements `Backend` with devices "across a wire"; the contract never mentions networks (N3). |
| **X2: No scheduling policy** | Multi-GPU load-balancing, work stealing, the `Device.fastest()` ranking policy. These compose *above* the contract and are library-replaceable. |
| **X3: No autotuning or kernel cache** | How a backend compiles, caches, or tunes kernels is entirely internal. The core sees only `cost()`, `submit()`, and the completion handle. |
| **X4: No vendor types** | No CUDA, Vulkan, or Metal type ever appears in the contract. The core links no vendor SDK; a backend library links whatever it needs. |

The whole point of the boundary: an HPC or remote-GPU backend can be written, shipped, and swapped as a library, and the compiler never learns that clusters exist.

---

## Appendix (non-normative)

### Why data-driven capabilities beat optional traits

A tempting alternative is many optional traits (`AsyncBackend`, `PersistentBackend`, …) that a backend implements à la carte. Capabilities-as-data (N1) win for evolvability: adding a `bool` field is additive and needs no new trait and no downcasting. The core reads a flag; a backend that predates the flag reports its default and stays valid. Traits are harder to grow.

### The real cost of this design

Naming it honestly, since the whole arc has been about not hiding costs:

- **The core trusts the backend.** Because the core never sees device memory (N3), it can't do cross-backend memory optimization or verify a backend's `cost()` numbers. That's the right layering — but a buggy backend can misreport, and the core will believe it.
- **Capability matrix testing.** Every optional capability doubles a test axis. N2 keeps the set small on purpose; if optional caps proliferate, the "which backend for this plan" logic and its tests get expensive. Resist adding capabilities.
- **The queue model — decided in shape (D10), details deferred.** v1: one serialized queue per device (`multi_queue: false`) — simple, correct, no overlap. Growth path: many queues, each owned by one task (so the driver's externally-synchronized-queue race is prevented by ordinary Rask ownership), buffer bound to its queue, cross-queue sharing an explicit transfer, no auto-sync. Still to pin down for that growth work: opening a second queue, the cross-queue transfer op, and queue-granularity in the location tracker (`conc.heterogeneous` O1).

### TODO

1. **`PrimitiveSet` representation.** How a backend advertises which primitives it supports, in a way that stays additive as the algebra grows.
2. **Path-2 queue details (O1 — direction set by D10).** v1 is one serialized queue; the growth work is many task-owned queues. Still to spec: how a task opens a second queue, the explicit cross-queue transfer op, and how the location tracker tracks *which queue* a buffer belongs to (not just which device).
3. **Backend registration.** How a library registers a backend with the core (link-time? a registry call?) and how `Device.fastest()` discovers registered backends.
4. **Reference backend.** The host-pool backend doubles as the executable reference semantics (`conc.data-parallel/W3`); pin down that it defines "correct" for every primitive.

(Contract versioning / compatibility policy is deliberately *not* here — pre-v1, per D9. It becomes a TODO at v1.)

### See also

- `conc.heterogeneous` — the overview, the decisions, and the ranked open questions
- `conc.data-parallel` — the `Wide` algebra, plan, and `submit`/`await` this contract serves
- `conc.data-parallel` §"What belongs in the language" — L1–L5, the core/library split this implements
- `conc.async` — must-use handles and `using X(config)`, mirrored by `Submission` and `RunConfig`
