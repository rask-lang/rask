<!-- id: conc.data-parallel -->
<!-- status: proposed -->
<!-- summary: Wide[T] data-parallel algebra — stage a plan, submit to run, await results; device is a resource, CPU is the baseline -->
<!-- depends: memory/boxes.md, memory/linear.md, types/simd.md, concurrency/async.md -->
<!-- implemented-by: compiler/crates/rask-interp/src/builtins/wide.rs (interpreter), compiler/crates/rask-codegen/src/dispatch.rs + compiler/runtime/vec.c (native, closure-free) -->

# Wide Data Parallelism

> **Prototype status.** A CPU implementation exists. The **interpreter** runs
> the full algebra below (`wide`, `map`, `zip_with`, `sum`, `reduce`, `min`,
> `max`, `read`). **Native** runs the closure-free ops (`wide`, `sum`, `read`)
> and matches the interpreter (the W3 oracle); the closure ops (`map`,
> `zip_with`) are interpreter-only until the native closure-callback path is
> fixed (`rask-lang/rask#441`). Prototype simplifications: `read`/`sum` are the
> run points rather than a single `submit`/`await`; `read` returns a value
> directly (no `T or GpuError` yet). See `NOTES_native_wide.md`.

A `Wide[T]` is a value spread across lanes. You **stage** operations on it — `map`, `sum`, `filter` — which build a plan and run nothing. You **submit** the plan, which starts it running on wherever its data lives, and **await** the handle to get results back. A device is a resource you acquire; the CPU is the baseline that can always run any plan. Same source, one algebra — because the algebra is pinned to the GPU-expressible subset and the CPU is a superset of it, every plan runs everywhere.

This replaces an earlier `dispatch |lane|` sketch that colored functions and hand-rolled lanes. That version is gone; the reasoning that killed it is in [Appendix: what v1 got wrong](#appendix-non-normative).

Overview and the design decisions behind this model: **`conc.heterogeneous`** (read it first). This spec is `proposed`, not decided — but it is a coherent direction, not a toy.

## The staging / submit / await model

The whole design rests on one split: building the plan is free and can't fail; running it is asynchronous, and the two moments that matter — *it starts* and *you wait* — are both explicit.

| Rule | Description |
|------|-------------|
| **C1: Staging is lazy** | Every `Wide[T]` operation returns a new `Wide[T]` (or a pending scalar) that records the operation. Nothing executes — no device allocation, no transfer, no compute. |
| **C2: Submit starts it** | `.submit()` enqueues the whole staged plan on the queue of the target its data lives on, returns immediately with a must-use completion handle, and *that* is where the hardware starts. A host-resident plan submits to the CPU; a device-resident plan submits to that device. |
| **C3: Await waits and returns** | `.await()` blocks on completion (a fence), copies the result home, and returns `T or GpuError`. Submitting several plans before awaiting *can* overlap them — but only on a device that offers multiple queues (the growth path, `conc.heterogeneous` D10). The v1 default is one serialized queue per device: submits run in order, `await` still works, no overlap yet. |
| **C4: Staging is total, `await` is fallible** | Staging cannot fail — it builds a graph. Failure (out of memory, device lost, transfer error) lands at `await`, never mid-staging. The handle carries the pending failure until you wait on it. |
| **C5: Fusion is visible, not magic** | Chained ops fuse into as few kernels as possible — `xs.map(f).map(g).sum()` is one kernel, not three plus two temp buffers. But fusion is *inspectable* (via `explain`) and *steerable* (an explicit barrier forces a materialization point). It is never a hidden decision you can only discover from a profiler. See the Observability section. |
| **C6: `read`/`commit` is the no-overlap sugar** | `.read()` (alias `.commit()`) is `submit().await()` for when you don't need overlap — one call, submit-then-block. Reach for `submit`/`await` when you want several plans in flight at once. |

<!-- test: skip -->
```rask
// A library function takes the device; it doesn't open one (placement is the app's call)
func normalize(data: []f32, dev: Device) -> Vec[f32] or GpuError {
    const xs    = data.to(dev)            // upload → resident on dev  (explicit transfer)
    const total = xs.sum()                // stage a reduce  (pending f32, not run)
    const ys    = xs.map(|x| x / total)   // stage a map     (Wide[f32], not run)
    const h     = ys.submit()             // START here — enqueue on dev's queue, async handle
    return try h.await()                  // WAIT here — block on completion, copy home
}

// The app holds the device resource and picks placement
with Device.gpu(0) as dev {
    const out = try normalize(pixels, dev)
}
```

Two lines carry the cost: `submit` (the GPU starts) and `await` (you block, and any failure surfaces). Everything above `submit` is a plan that ran nothing.

### Why `submit` and `await`, not one blocking `commit`

An earlier version had a single blocking `commit`. It read cleanly but modeled the hardware as synchronous — and a GPU is not synchronous; it runs a queue you enqueue into and later fence on. Collapsing that into one blocking call hides the asynchrony *and* forecloses overlap. Splitting into `submit` (starts) and `await` (waits) matches what actually happens and keeps both moments visible. Overlap itself — several plans genuinely in flight — arrives with multiple queues on the growth path (`conc.heterogeneous` D10); v1's single queue serializes, but the shape is already right for it. `read`/`commit` stays as sugar (C6) for the common no-overlap case — so the simple path is still one line, but it no longer lies about the machine.

## Where a plan runs

The GPU can express *less* than the CPU — pure lane work, no recursion, no dynamic dispatch, no host effects. The `Wide` algebra is deliberately pinned to exactly that GPU-expressible subset. Because every CPU is a superset of it, **CPU is not a fallback — it is the baseline the algebra is defined against**, and it can always run any plan. A device just accelerates a plan the CPU could already run.

| Rule | Description |
|------|-------------|
| **W1: The algebra is the GPU subset** | A plan type-checks against the GPU-expressible subset. There is no per-target capability matrix — a legal plan is legal *everywhere*. You never get "runs on cores but not GPU." Need more than the subset? Drop out of `Wide` into ordinary host Rask. |
| **W2: CPU is baseline, not fallback** | Every plan runs on the CPU by construction (W1). Moving a plan to a device is acceleration, not a different program — the CPU path is always valid, never a degraded emergency route. |
| **W3: CPU run is the reference semantics** | What a plan *means* is defined by its CPU execution. A device backend is correct iff it matches, modulo documented float reassociation in reductions (`type.simd/R2`). This gives a test oracle: run any plan on CPU and device, diff. |
| **W4: A device is a resource, not a context** | You acquire a device with `with Device.gpu(n) as dev`, the same way you open a file. It is **not** a `using` runtime like `Multitasking`, and it is **not exclusive** — hold several devices at once (`with Device.gpu(0) as a, Device.gpu(1) as b`). Config rides on acquisition: `Device.gpu(1, mem_limit: 4.GB)`. |
| **W5: The three targets are three different things** | *Host-inline* (single core, vectorized) is a codegen choice — it **is** `type.simd`, re-expressed; `Vec[T, N]` is its fixed-width corner. *Host-pool* (across cores) uses the existing `using ThreadPool` runtime. *Device* is a resource (W4). They are not one `using Width` construct — treating them uniformly was a mistake. |
| **W6: Placement flows from data, and is explicit** | `data.to(dev)` uploads (a visible transfer); an op runs where its inputs live; `.read()`/`await` brings results home. A plan built from host data runs on the CPU; a plan built from `dev`-resident data runs on `dev`. *Representation* is in the type (`Wide[T]` vs `[]T`); *location* is tracked like a borrow — surfaced by tooling, not stamped into the signature, so it never becomes a viral type parameter. |
| **W7: "As fast as possible" is a value, not a context** | Best-effort placement is `Device.fastest()` (or a policy value) that resolves to the widest available target — device if present, else cores. It's a device you pick, not a magic block, and it may resolve to the CPU for small inputs where launch overhead loses. |

Portability is *structural*, not hoped-for: because the subset is fixed by what the GPU can do (W1), the question was never *can* the CPU run a plan — a superset always can — only whether it runs it *fast*.

## The primitive algebra

You express parallelism *only* through these primitives. Each has a known dependency structure, so the compiler never has to prove independence — proving it for arbitrary index code was the undecidable trap that sank v1; here the dependency structure is encoded in the operator instead.

| Primitive | Shape | Cost class |
|-----------|-------|-----------|
| **P1: `map(f)`** | `Wide[T] → Wide[U]` | Uniform — independent per lane |
| **P2: `zip_with(other, f)`** | `(Wide[T], Wide[U]) → Wide[V]` | Uniform |
| **P3: `iota(n)`** | `→ Wide[usize]` | Uniform generator |
| **P4: `reduce(op, id)`** / `sum`, `min`, `max` | `Wide[T] → T` | Log-depth tree |
| **P5: `scan(op, id)`** | `Wide[T] → Wide[T]` | Log-depth prefix |
| **P6: `filter(pred)`** | `Wide[T] → Wide[T]` | **Divergent** — compaction |
| **P7: `gather(idx)`** | `Wide[usize] → Wide[T]` | **Irregular** — uncoalesced reads |
| **P8: `scatter_with(idx, op)`** | `→ buffer` | **Irregular** + conflict rule |
| **P9: `stencil(radius, f)`** | `Wide[T] → Wide[U]` | Uniform + halo |

Reductions and scans (P4, P5) are first-class — the compiler owns the workgroup / shared-memory / barrier code. `xs.sum()` is not something you hand-roll; that was the biggest gap in the map-only sketch and here it's a primitive.

Cost is visible through *which operator you reached for*. `map` is uniform and cheap. The divergent, throughput-killing operations — `filter`, `gather`, `scatter` — are spelled differently and stand out in the source. Divergence isn't buried in an `if`; it's the operator you chose.

<!-- test: skip -->
```rask
with Device.gpu(0) as dev {
    const xs = data.to(dev); const ys = other.to(dev)   // resident on dev

    // dot product — two primitives, fused, one submit; .read() = submit().await()
    const dot = try xs.zip_with(ys, |a, b| a * b).sum().read()

    // histogram — scatter with an explicit conflict rule (lanes colliding on a bin add)
    const hist = try keys.to(dev).scatter_with(bins.to(dev), |old, _| old + 1).read()

    // running total — a scan
    const totals = try amounts.to(dev).scan(+, 0.0).read()
}
```

### Scatter conflicts

Two lanes writing the same index is the one place the algebra can't stay purely independent. `scatter_with(idx, op)` requires a combining `op` (P8) — colliding writes are combined, not raced. There is no plain `scatter` that lets lanes silently clobber each other; you state how conflicts resolve or you don't scatter.

## Coloring lives on data, not functions

| Rule | Description |
|------|-------------|
| **D1: The type carries the representation, not the location** | `Wide[T]` is a distinct type from `[]T` — it says "this is a staged lane computation," which is honest. *Which device* it's resident on is not in the type; that's tracked like a borrow and surfaced by tooling (W6). Representation is typed; location is flow-tracked. Neither infects function signatures. |
| **D2: Functions stay uncolored** | A `func` or closure used in a `map` needs no annotation. `square(x)` is ordinary Rask, usable on host and in a plan alike. No `@gpu`, no `__device__`, no second copy. |
| **D3: Reachability compiles device code** | The compiler emits device code for every pure function reachable from a submitted plan — `rask-mono` retargeted to SPIR-V. This is a reachability closure, not a signature annotation. |
| **D4: Non-closable calls are rejected** | Function pointers, dynamic dispatch, closures over host state, and unbounded recursion inside a staged op are compile errors — the device-code closure can't resolve them. The error points at the call, with the reason. |

This is the honest version of "no coloring." GPU *is* a colored domain — D1 admits it, on the data where it's true. What Rask avoids is *viral function coloring*: D2 keeps pure code uncolored and shareable. D4 is the cost — the constructs CUDA/SYCL would force you to annotate, Rask instead forbids inside a plan. That's a real restriction, stated out loud, not a claim that the fork doesn't exist.

## Observability — see what it will do

The Python accelerator experience — JAX, `torch.compile`, CuPy — is miserable for one reason: **the plan is hidden.** You can't see where things run, when they run, what fused, or how much memory a step needs, so when it's slow or out of memory you poke buttons and pray. Laziness gets the blame, but laziness isn't the culprit — *invisibility* is. SQL is lazy and heavily optimized and people debug enormous queries fine, because `EXPLAIN` makes the plan a first-class object. JAX is lazy and optimized and undebuggable, because the plan lives inside a JIT you can't open. Same laziness, opposite experience.

Rask's plan is the SQL kind. Two structural facts make that possible:

- **The plan is your data structure**, held before `submit`. It can be printed, measured, and diffed.
- **Compilation is ahead-of-time** (D3). Kernels are built once, at build, into the binary — there is no runtime tracing and no shape-triggered recompilation. JAX's worst "why is it slow now" cause structurally cannot happen.

| Rule | Description |
|------|-------------|
| **O1: `explain` a plan** | `plan.explain()` prints the plan without running it: the kernels it fuses into, where transfers happen, and the peak device-memory footprint as a formula over the input sizes (`peak = 3·N·4 B + …`). SQL's `EXPLAIN`, for kernels. |
| **O2: Memory is accounted before launch** | `submit` computes the peak footprint from the plan and checks it against the budget *before* enqueuing anything. OOM comes back at `await`, attributed to the stage that peaks — not surfaced from deep inside a fused kernel after partial work. |
| **O3: Fusion is steerable** | Fusion boundaries are reported by `explain`. An explicit barrier forces a materialization point when you want to bound or inspect an intermediate. Contrast the profiler-trace archaeology of `fusion_1723`. |
| **O4: Deterministic by construction** | No racing scatter (P8 requires a combining op) and stable kernel selection mean a plan's result and structure don't drift run-to-run. Fewer invisible variables when chasing "why is it different now." |

```
> pixels.map(shade).map(tonemap).reduce(max, 0.0).explain()

plan (device gpu:0)
  upload   pixels          16.0 MB  ──┐
  kernel#1 map shade          fused   │  one kernel, no intermediate buffer
           map tonemap        fused   │
           reduce max      ──────────┘  → scalar
  download scalar             4 B
  peak device memory: 32.0 MB   (input + one working buffer)
```

**The honest boundary.** This makes the *structure* transparent — where, when, what fused, how much memory, why an OOM. It does **not** make wall-clock time predictable; that's cache behavior, occupancy, and bandwidth — hardware physics no plan inspector can foretell. So performance *tuning* stays partly empirical. What dies is the structural mystery: you will never again be unable to answer "what is this doing, and how much is it using." That's the bounded, honest claim — not "no more profiling," but "no more flying blind."

## Escape hatch: explicit kernels

The algebra covers the ~80% that decomposes into primitives. Hand-tuned kernels — custom shared-memory tiling, exotic access patterns — are the other 20%.

| Rule | Description |
|------|-------------|
| **K1: `kernel` is unsafe-tier** | An explicit `kernel(n) |lane| { … }` block writes raw per-lane code. It sits at the same trust level as `unsafe { }` — the compiler does not check per-lane independence. |
| **K2: You own the proof** | Inside a `kernel`, writing `out[lane]` from data other lanes also write is a data race the compiler will not catch. The block is where "I promise this is independent" lives, spelled out, like raw pointers. |

The safe algebra is the main road; `kernel` is the labelled off-ramp. An undecidable independence check doesn't belong in the checked language — it belongs behind the same "I promise" boundary as `unsafe`.

## What belongs in the language, and what doesn't

No language does GPUs, remote acceleration, or HPC *natively* — not CUDA-C++, not Rust. In C++ and Rust the entire story is libraries: Thrust and Kokkos are the array algebra as libraries; `cust`/`wgpu`/`rust-gpu` are the backends as crates; MPI/NCCL/SLURM are the cluster layer as libraries. The lesson: keep the core to what only the compiler can do, and put *reaching a device* behind a stable interface libraries implement.

| Rule | Description |
|------|-------------|
| **L1: Core owns the algebra and the IR** | The `Wide[T]` type, the primitive set, the subset check (W1), staging/`submit`/`await`, `explain`, and — the part only the compiler can do — lowering pure functions reachable from a plan to **one portable kernel IR, SPIR-V** (D3). This cannot be a library; it needs the type system and a codegen target. |
| **L2: Core ships two CPU targets** | Host-inline (SIMD) and host-pool (`ThreadPool`) are built in, because they *are* the existing SIMD path and thread pool — they come for free and give the baseline (W2) with no external dependency. |
| **L3: Core links no vendor SDK** | The compiler emits SPIR-V and nothing else device-specific. With no GPU backend installed, `Wide` code still compiles and runs on CPU. A GPU is opt-in — add a backend library, exactly like adding a crate. No CUDA/Vulkan in the core. |
| **L4: A backend is a small interface** | A backend is anything that can (a) run a plan / consume its SPIR-V and (b) report memory and timing to `explain`. That interface is the whole extension surface. |
| **L5: Everything device-specific is a library** | The GPU driver backend (Vulkan/CUDA/Metal), multi-GPU orchestration, device-selection policies, autotuning, and **all remote / distributed / HPC / multi-node** work are libraries implementing L4. A remote or clustered device is just a backend whose "device" is across a wire — the algebra doesn't change a line. |

The line in one sentence: **the language owns the algebra, the subset guarantee, the portable IR, and the plan you can inspect; a library owns every question of which physical device and how to reach it.** That is what keeps the core vendor-neutral and lets an HPC or remote-GPU library exist without the compiler ever hearing about clusters.

## Errors

```
ERROR [conc.data-parallel/D4]: `render` can't be compiled for the device
   |
14 |     pixels.map(|p| render(p))
   |                    ^^^^^^ called inside a staged `map`, but `render` dispatches dynamically
   |
NOTE: device code is resolved by reachability; a trait-object call at ui.rk:88 can't be resolved
FIX: monomorphize the call, or move it out of the plan (run it on the host before `.to(dev)`)
```

```
ERROR [conc.data-parallel/W6]: plan combines data from two devices
   |
9  |     a_on_gpu0.zip_with(b_on_gpu1, |a, b| a + b)
   |                        ^^^^^^^^^ resident on gpu:1, but `a_on_gpu0` is on gpu:0
   |
FIX: move one first — `b_on_gpu1.to(gpu0_dev)` — the transfer is explicit
```

```
RUNTIME (returned, not panicked) [conc.data-parallel/C4]: GpuError.OutOfMemory
   submitted plan needs 4.2 GB device memory; device has 3.1 GB free
   — surfaced from the `.await()` at physics.rk:52 (peaks at the `gather`, stage 3)
```

## Edge cases

| Case | Rule | Handling |
|------|------|----------|
| Plan combines data resident on two devices | W6 | Compile error (placement mismatch) — insert an explicit `.to(dev)` |
| Staged plan never submitted | C1 | Warning — the plan is dead code, nothing ran (like an unused `Result`) |
| `Device.gpu(n)` with no such device present | W4 | Acquisition returns `DeviceError.NoDevice` — or use `Device.fastest()`, which resolves to the best available target (cores if no GPU) |
| Out of memory | C4 | `await` returns `GpuError.OutOfMemory`, not a panic; checked before enqueue (O2) |
| Buffer used after its `with Device` block released | W4 | Handle into a freed device — see `conc.heterogeneous` O3 (compile-time vs runtime is unresolved) |
| Two lanes scatter to the same index | P8 | Combined by the required `op`; no plain racing scatter exists |
| Dynamic dispatch inside a staged op | D4 | Compile error |
| Recursion inside a staged op | D4 | Compile error (device closure can't bound the stack) |
| Data race inside an explicit `kernel` | K2 | Not caught — programmer's responsibility, like `unsafe` |
| Branchy `map` lambda (divergence) | P1 | Allowed; tooling flags likely divergence (not enforced) |

---

## Appendix (non-normative)

### Rationale

**C2/C3 (`submit` and `await`, not one blocking `commit`):** The answer to "lazy hides when work happens" is that it doesn't — two explicit verbs mark the two moments the hardware actually has: `submit` (the device starts) and `await` (you block, and failure lands). An earlier single blocking `commit` read cleanly but modeled the device as synchronous, hiding the asynchrony and killing overlap. `read`/`commit` survives as sugar (C6) so the simple path stays one line without lying about the machine.

**D1 (color the representation, track the location):** Coloring functions is the CUDA/SYCL disease — viral, splits the ecosystem. But the fork is real; something carries it. The *representation* (`Wide[T]`) is honest in the type; the *device location* is tracked like a borrow and shown by tooling, not stamped into signatures. Pure functions stay clean either way. JAX/Futhark lineage, minus the viral part.

**W4 (device as resource, not context):** A `using Multitasking` context exists because a scheduler is a heavyweight, exclusive, process-global runtime. A GPU is a driver handle with memory — you want several at once and there's nothing global about it. So it's a `@resource` acquired with `with`, like a file. This also dissolves the "widths" confusion: host-inline (codegen), host-pool (runtime), and device (resource) are three different kinds of thing, never one construct.

**P4–P8 (primitives, not raw lanes):** You can't prove arbitrary index code is race-free — it's undecidable. So the language only lets you spell parallelism through operators whose dependency structure is known. Reductions and scans come for free because they're primitives, not hand-rolled barrier code.

**W7 (`fastest` = speed, not device):** Naming a specific device is a guarantee about *where*. `Device.fastest()` drops that for a promise about *speed* — "widest available for this plan and size" — and it's a value you pick, not a magic block. The transparent path is always a named device; `fastest()` is the convenience, and it reports what it resolved to.

**Observability as the point, not a feature:** The reason to trust `Wide` over a Python framework is that the plan is inspectable (`explain`), memory is accounted before enqueue, and there's no runtime retracing (AOT). Those aren't extras — they're what separates "a nicer JAX" from "the thing you trust at 3am when the kernel is OOMing." If they slip, the design loses its reason to exist.

### The one tension left

Laziness genuinely fights "costs visible in code" — that's why `submit` and `await` have to stay loud, explicit verbs, why staging must never sneak in a hidden execution, and why fusion is inspectable rather than magic (O1–O3). If a future convenience makes some op auto-submit, this transparency breaks. Hold the line: nothing runs until `submit`, and the plan is always openable before it does.

### What v1 got wrong

The `dispatch |lane|` sketch failed six ways, and this design is the point-by-point answer:

| v1 weakness | v2 answer |
|-------------|-----------|
| "No coloring, same bet as async" was false | D1 — GPU *is* colored; put it on data, keep it off functions |
| No device concurrency/sync model | C1–C6 — the staged plan is the work, `submit` returns a must-use handle, `await` is the join |
| Per-lane independence check undecidable | P1–P9 — primitives encode dependency; raw lanes only behind `kernel` (K1) |
| Map-only skipped the cross-lane 80% | P4, P5, P8 — reduce/scan/scatter are first-class |
| "No fallback" was coloring at the block level | W1/W2 — the algebra *is* the GPU subset, so CPU (a superset) always runs it; portability is structural, not a fallback |
| Divergence invisible | P6–P8 — irregular ops are named operators, visible in source |

### TODO — what v2 still must settle

The consolidated, ranked open questions live in `conc.heterogeneous`. The ones specific to this algebra:

1. **Residency across submits.** Keeping a result on-device between two `submit`s (no host round-trip) — automatic via a persisted buffer, or an explicit on-device checkpoint? Ties to the residency model (`conc.heterogeneous` O5).
2. **`stencil` boundary handling (P9).** Halo/edge behavior — clamp, wrap, or caller-supplied — needs pinning down.
3. **The `GpuError` set (C4).** Enumerate: `OutOfMemory`, `DeviceLost`, `NoDevice`, `Unsupported`, transfer failures. Mirror the care `conc.async` gave `JoinError`.
4. **Tooling for divergence (P1).** The lint that flags a branchy `map` lambda — spec what it detects and how it reads.
5. **`explain` memory formulas (O1).** How exact the footprint can be when `Wide` lengths are runtime values — symbolic in `N` at compile time, concrete at submit.

### See also

- `conc.heterogeneous` — the overview, the decisions, and the ranked open questions
- `conc.wide-backend` — the stable contract a device backend implements
- `type.simd` — host-inline is this model's narrowest target
- `conc.async` — the quality bar; `submit`/handle/`await` mirrors `spawn`/handle/`join`
- `mem.linear` — `Wide[T]` buffers follow the consume-once discipline
