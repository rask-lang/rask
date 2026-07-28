<!-- id: conc.wide-backend -->
<!-- status: proposed -->
<!-- summary: The stable contract between the Wide core and a device backend — designed to outlive the hardware it targets -->
<!-- depends: concurrency/data-parallel.md -->

# The Wide Backend Contract

A backend is the thing that actually runs a `Wide` plan on some device. The core ships two (`Simd`, `ThreadPool`); everything else — GPU drivers, remote devices, clusters — is a library implementing this contract (`conc.data-parallel/L5`).

This spec is about the *contract*, not any backend. That's deliberate: the contract is the part that can't be cheaply changed once libraries depend on it, while SPIR-V, Vulkan, CUDA, and whatever replaces them in 2032 are details that must be free to churn underneath. **The energy goes into the API so the language stays extendable when the tech turns over.** Status: `proposed`.

## What has to stay stable, and what must be free to change

| Layer | Who owns it | Stability |
|-------|-------------|-----------|
| The `Wide` algebra + the **plan** it produces | Core | Stable, versioned, additive |
| This contract (the trait below) | Core | Stable, versioned, additive |
| Kernel IR format (SPIR-V today) | Negotiated | A slot, swappable per backend |
| Device drivers, memory tech, transport | Backend library | Free to change entirely |

The load-bearing idea: **the plan is the stable currency.** Core produces a plan; every backend consumes one. As long as that interface holds, the driver underneath can be rewritten, a new accelerator can appear, or a backend can move a device onto the network — none of it reaches the language.

## Five principles that keep it evolvable

These are the point of the spec. Everything below is in service of them.

| Rule | Description |
|------|-------------|
| **N1: Capability negotiation, not assumption** | The core never assumes what a backend can do — it reads `info()` and asks. New abilities are advertised as *data* (fields on `Capabilities`), so adding one leaves every existing backend valid; it simply doesn't advertise the new field. |
| **N2: Small mandatory core, few optional extras** | Four mandatory methods (below) are all a backend must implement to be usable. Everything else — resident results, async overlap, real timing — is optional and advertised. Most plans need only the mandatory set, so the "which backend can run this" logic stays simple. |
| **N3: The core never sees device memory** | The contract speaks host data and plans, never device pointers. Whether a transfer is a `memcpy`, a DMA, unified memory, or an RPC to another machine is entirely the backend's business. This is what lets a *remote* backend exist without the contract mentioning networks. |
| **N4: Graceful degradation** | Any optional feature a backend lacks has a defined fallback — usually "materialize to host" or "route to the CPU baseline." A backend that can't run a plan says so with a structured reason; it never fails silently or half-runs. |
| **N5: Additive versioning** | The contract carries a version. Growth is additive — new capability fields, new primitives, new optional sub-traits — which bumps a minor version and breaks nothing. Breaking changes bump major and are avoided. The core supports a *range* and refuses anything outside it with a clear message. |

The combination of N1 and N5 is what the user's instinct was pointing at: **the algebra and the backends evolve independently.** A new primitive (say `sort`) can be added to the plan; a backend that doesn't implement it advertises that, and the core routes such plans elsewhere or to the CPU — no existing backend breaks, no flag day.

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
    // Run once with these inputs, produce outputs. This is what `commit` calls.
    func run(inputs: []Input) -> []Output or BackendError

    // Release device resources. Runs at `using` block exit.
    func release(take self)
}
```

| Rule | Description |
|------|-------------|
| **B1: `info` first** | The core calls `info()` before dispatching anything, and honors what it reports — formats, primitives, capabilities, contract version. |
| **B2: `cost` without side effects** | `cost()` must not allocate device memory or run kernels. It reports the footprint the core shows in `explain` and checks against the budget before `prepare` (`conc.data-parallel/O2`). |
| **B3: `prepare` then `run`** | Compilation and scratch reservation happen in `prepare`; `run` is the hot path and may be called repeatedly on one session. `commit` maps to one `run`. |
| **B4: `release` is total** | `release` always frees cleanly, even after a failed `run`. Ties to the `using` block lifetime — no leaked device memory across blocks. |

## Identity and capabilities — as data

<!-- test: skip -->
```rask
struct BackendInfo {
    name: string
    contract: Version           // which version of THIS contract it implements
    formats: []KernelFormat     // kernel IRs it accepts; SpirV is the default slot
    primitives: PrimitiveSet    // which algebra primitives it can run
    caps: Capabilities          // optional features — additive
}

struct Capabilities {
    resident_results: bool      // can keep outputs on-device between commits
    async_submit: bool          // can overlap independent plans
    timing: bool                // cost() returns measured/modeled time, not just memory
    // Additive: new fields default false. Old backends built against an older
    // shape stay valid — they simply don't claim the new ability.
}
```

Capabilities are data, not a zoo of optional traits, precisely so the set can grow without a versioning break (N5). The core reads flags and opportunistically uses what's there.

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
    Resident(Token)     // a result kept on-device from a prior run on THIS backend
}

enum Output {
    Host(Vec[u8])       // materialized back to the host
    Resident(Token)     // kept on-device; opaque token, feedable as a later Input
}
```

| Rule | Description |
|------|-------------|
| **M1: Host is the baseline** | By default the core hands host bytes and gets host bytes back. A minimal backend implements only this. It's the most abstract form — it makes no assumption about how memory works, so remote and unified-memory backends fit unchanged (N3). |
| **M2: Residency is an opt-in shortcut** | A backend advertising `resident_results` may return a `Resident(Token)` — an opaque handle the core can feed into a later plan's inputs, skipping a host round-trip. The core never interprets the token; only the same backend does. This is how multi-commit pipelines avoid the bus without the core knowing what a device pointer is. |
| **M3: Tokens don't cross backends** | A `Resident` token is valid only for the backend and device that issued it. The core tracks that and errors clearly on misuse — it never passes a token to a foreign backend. |

M2 is the answer to the parent spec's open question about keeping data on-device between commits — as a capability, not a core assumption.

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

At a `using` block, the core:

1. Enumerates registered backends (the built-in `Simd`/`ThreadPool`, plus any a library registered).
2. Filters to those that can reach a device, accept a kernel format the core can emit (F1), and support every primitive in the plan (`info().primitives`).
3. For a **named width** (`using Gpu`): picks that backend, or errors with `NoDevice` if none qualifies.
4. For **`using Accelerated`**: ranks the survivors by `cost()` and picks the fastest — the selection *policy* is replaceable and lives above the contract (it's library-tunable, not baked in).
5. Calls `cost()` for `explain`, `prepare()` once, `run()` at each `commit`, `release()` at block exit.

## What the contract deliberately keeps out

Staying out is how the core stays vendor-neutral and future-proof. None of these belong in the contract:

| Rule | Description |
|------|-------------|
| **X1: No transport** | Remote devices, sockets, cluster topology, MPI-style collectives. A remote backend implements `Backend` with devices "across a wire"; the contract never mentions networks (N3). |
| **X2: No scheduling policy** | Multi-GPU load-balancing, work stealing, the `Accelerated` ranking policy. These compose *above* the contract and are library-replaceable. |
| **X3: No autotuning or kernel cache** | How a backend compiles, caches, or tunes kernels is entirely internal. The core sees only `cost()` and `run()`. |
| **X4: No vendor types** | No CUDA, Vulkan, or Metal type ever appears in the contract. The core links no vendor SDK; a backend library links whatever it needs. |

The whole point of the boundary: an HPC or remote-GPU backend can be written, shipped, and swapped as a library, and the compiler never learns that clusters exist.

---

## Appendix (non-normative)

### Why data-driven capabilities beat optional traits

A tempting alternative is many optional traits (`AsyncBackend`, `PersistentBackend`, …) that a backend implements à la carte. Capabilities-as-data (N1) win for evolvability: adding a `bool` field is additive and needs no new trait, no downcasting, no version gymnastics. The core reads a flag; an old backend that predates the flag reports its default and stays valid. Traits are harder to grow without a break.

### The real cost of this design

Naming it honestly, since the parent spec's whole arc has been about not hiding costs:

- **The core trusts the backend.** Because the core never sees device memory (N3), it can't do cross-backend memory optimization or verify a backend's `cost()` numbers. That's the right layering — but it means a buggy backend can misreport, and the core will believe it.
- **Capability matrix testing.** Every optional capability doubles a test axis. N2 keeps the set small on purpose; if optional caps proliferate, the "which backend for this plan" logic and its tests get expensive. Resist adding capabilities.
- **Additive-only is a discipline, not a guarantee.** N5 is a promise the maintainers must keep by hand. One convenient breaking change and every backend library breaks. This contract should be treated as semver with a real compatibility policy once it stabilizes.

### TODO

1. **`PrimitiveSet` representation.** How a backend advertises which primitives it supports, in a way that stays additive as the algebra grows.
2. **Contract version policy.** The concrete compatibility rules — supported range, deprecation path, what counts as additive vs breaking.
3. **Backend registration.** How a library registers a backend with the core (link-time? a registry call under `using`?) and how `Accelerated` discovers registered backends.
4. **Async / `async_submit`.** The optional overlap capability's exact shape — how independent plans pipeline without the core managing streams.
5. **Reference backend.** The `ThreadPool` backend doubles as the executable reference semantics (`conc.data-parallel/W6`); pin down that it defines "correct" for every primitive.

### See also

- `conc.data-parallel` — the `Wide` algebra, plan, and `commit` this contract serves
- `conc.data-parallel` §"What belongs in the language" — L1–L5, the core/library split this implements
- `conc.async` — `using X(config)` runtime configuration, mirrored by `RunConfig`
