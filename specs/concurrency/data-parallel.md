<!-- id: conc.data-parallel -->
<!-- status: proposed -->
<!-- summary: Wide[T] data-parallel algebra — stage operations freely, commit to run; one model across SIMD, cores, and GPU -->
<!-- depends: memory/boxes.md, memory/linear.md, types/simd.md, concurrency/async.md -->

# Wide Data Parallelism

A `Wide[T]` is a value spread across lanes. You **stage** operations on it — `map`, `sum`, `filter` — which build a plan and run nothing. You **commit** the plan, and that is the single point where the hardware runs. The `using` context decides how wide the lanes are: one core's vector unit, the thread pool, or a GPU's thousands of threads. Same source, one algebra, any width — because the algebra is fixed to the GPU-expressible subset, and the CPU is a superset of it, every plan runs everywhere.

This replaces an earlier `dispatch |lane|` sketch that colored functions and hand-rolled lanes. That version is gone; the reasoning that killed it is in [Appendix: what v1 got wrong](#appendix-non-normative). This spec is `proposed`, not decided — but it is a coherent direction, not a toy.

## The staging/commit model

The whole design rests on one split: building the plan is free, running it is the one visible, fallible event.

| Rule | Description |
|------|-------------|
| **C1: Staging is lazy** | Every `Wide[T]` operation returns a new `Wide[T]` (or a pending scalar) that records the operation. Nothing executes. No allocation on the device, no transfer, no compute. |
| **C2: Commit runs it** | `.commit()` executes the entire staged plan on the context's backend, blocks until the device finishes, and returns the result to the host. This is the *only* place work happens. |
| **C3: Staging is total, commit is fallible** | Staging cannot fail — it builds a graph. `commit` can (device out of memory, device lost, transfer error), so it returns `T or GpuError`. Errors surface at the commit line, never mid-staging. |
| **C4: Fusion is visible, not magic** | Chained ops fuse into as few kernels as possible — `xs.map(f).map(g).sum()` is one kernel, not three plus two temp buffers. But fusion is *inspectable* (via `explain`) and *steerable* (an explicit barrier forces a materialization point). It is never a hidden decision you can only discover from a profiler. See the Observability section. |
| **C5: Commit is the transfer** | Upload (`.wide()`), compute, and download all happen inside the one `commit`. `.wide()` marks data to cross the bus (visible in source); the crossing executes at commit with everything else. |

<!-- test: skip -->
```rask
func normalize(data: []f32) -> Vec[f32] or GpuError {
    using Gpu {
        const xs    = data.wide()             // stage upload
        const total = xs.sum()                // stage a reduce  (pending f32, not run)
        const ys    = xs.map(|x| x / total)   // stage a map     (Wide[f32], not run)
        return try ys.commit()                // RUN HERE: upload → fused kernel → download
    }
}
```

Read that function and you can point at exactly one line where the GPU runs: `commit`. Everything above it is a plan.

### Why `commit`, not `read`

`.read()` sounds cheap — "just fetch the array" — and hides that it triggers all deferred compute. `commit` carries the transaction meaning: do it now, for real, blocking, this is the point where cost and failure land. That connotation is what makes laziness honest. **If you can't see where the device runs, the model has failed** — `commit` is how you always can.

## Width: the GPU subset runs everywhere

The GPU can express *less* than the CPU — pure lane work, no recursion, no dynamic dispatch, no host effects. The `Wide` algebra is deliberately pinned to exactly that GPU-expressible subset. Because every CPU is a superset of it, **CPU is not a fallback — it is the baseline the algebra is defined against**, and it can always run any plan. GPU is acceleration of a plan the CPU could already run.

| Rule | Description |
|------|-------------|
| **W1: Named widths pick a specific device** | `using Simd` = one core's vector unit. `using ThreadPool` = cores. `using Gpu` = the device. These name *where* the work runs — a guarantee, not a preference. |
| **W2: `Accelerated` is a speed promise, not a device promise** | `using Accelerated` means "as fast as possible for this plan and size," *not* "run on the GPU." It may pick the GPU, cores, or even single-core SIMD for tiny inputs where launch overhead loses. You opt into not knowing the device in exchange for best-effort speed — and tooling reports which width it chose. |
| **W3: The width is always named in source** | Whether you pick a specific device (W1) or the speed policy (W2), it's the `using` block at the top of the region — explicit, never inferred. |
| **W4: One subset, checked once** | A plan type-checks against the GPU subset regardless of width. No per-backend capability matrix — legal is legal on *all* widths. You never get "runs on cores but not GPU." Need more than the subset? Drop out of `Wide` into ordinary host Rask. |
| **W5: CPU is baseline, not fallback** | Switching `using Gpu` → `using ThreadPool` is a one-line, always-valid change, *guaranteed* by the subset relation — not a degraded emergency path. |
| **W6: CPU run is the reference semantics** | What a plan *means* is defined by its CPU execution. The GPU backend is correct iff it matches, modulo documented float reassociation in reductions (`type.simd/R2`). This gives a test oracle: run any plan on both widths and diff. |

**Runtime device control.** Device selection, memory budget, and stream count are *runtime* config on the context — exactly like `using Multitasking(workers: 4)` in `conc.async`. The kernels are compiled ahead of time; only the launch configuration is runtime:

<!-- test: skip -->
```rask
using Gpu(device: 1, mem_limit: 4.GB) { … }   // pick GPU #1, cap device memory
```

The payoff is twofold. First, `using Simd` **is** today's `type.simd`, re-expressed — the narrowest width of one model, not a bolt-on; `Vec[T, N]` is the fixed-width, one-core corner of `Wide[T]`. Second, portability is *structural*, not hoped-for: because the subset is fixed by what the GPU can do, the question was never *can* the CPU run a plan (a superset always can) — only whether it runs it *fast*.

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
using Gpu {
    // dot product — two primitives, fused, one commit
    const dot = try xs.zip_with(ys, |a, b| a * b).sum().commit()

    // histogram — scatter with an explicit conflict rule (lanes colliding on a bin add)
    const hist = try keys.scatter_with(bins, |old, _| old + 1).commit()

    // running total — a scan
    const totals = try amounts.scan(+, 0.0).commit()
}
```

### Scatter conflicts

Two lanes writing the same index is the one place the algebra can't stay purely independent. `scatter_with(idx, op)` requires a combining `op` (P8) — colliding writes are combined, not raced. There is no plain `scatter` that lets lanes silently clobber each other; you state how conflicts resolve or you don't scatter.

## Coloring lives on data, not functions

| Rule | Description |
|------|-------------|
| **D1: The type carries the color** | `Wide[T]` is a distinct type from `[]T`. It says "this data lives where the lanes are." The color is on the buffer, which is honest — the memory space really is different. |
| **D2: Functions stay uncolored** | A `func` or closure used in a `map` needs no annotation. `square(x)` is ordinary Rask, usable on host and in a plan alike. No `@gpu`, no `__device__`, no second copy. |
| **D3: Reachability compiles device code** | The compiler emits device code for every pure function reachable from a committed plan — `rask-mono` retargeted to SPIR-V. This is a reachability closure, not a signature annotation. |
| **D4: Non-closable calls are rejected** | Function pointers, dynamic dispatch, closures over host state, and unbounded recursion inside a staged op are compile errors — the device-code closure can't resolve them. The error points at the call, with the reason. |

This is the honest version of "no coloring." GPU *is* a colored domain — D1 admits it, on the data where it's true. What Rask avoids is *viral function coloring*: D2 keeps pure code uncolored and shareable. D4 is the cost — the constructs CUDA/SYCL would force you to annotate, Rask instead forbids inside a plan. That's a real restriction, stated out loud, not a claim that the fork doesn't exist.

## Observability — see what it will do

The Python accelerator experience — JAX, `torch.compile`, CuPy — is miserable for one reason: **the plan is hidden.** You can't see where things run, when they run, what fused, or how much memory a step needs, so when it's slow or out of memory you poke buttons and pray. Laziness gets the blame, but laziness isn't the culprit — *invisibility* is. SQL is lazy and heavily optimized and people debug enormous queries fine, because `EXPLAIN` makes the plan a first-class object. JAX is lazy and optimized and undebuggable, because the plan lives inside a JIT you can't open. Same laziness, opposite experience.

Rask's plan is the SQL kind. Two structural facts make that possible:

- **The plan is your data structure**, held before `commit`. It can be printed, measured, and diffed.
- **Compilation is ahead-of-time** (D3). Kernels are built once, at build, into the binary — there is no runtime tracing and no shape-triggered recompilation. JAX's worst "why is it slow now" cause structurally cannot happen.

| Rule | Description |
|------|-------------|
| **O1: `explain` a plan** | `plan.explain()` prints the plan without running it: the kernels it fuses into, where transfers happen, and the peak device-memory footprint as a formula over the input sizes (`peak = 3·N·4 B + …`). SQL's `EXPLAIN`, for kernels. |
| **O2: Memory is accounted before launch** | `commit` computes the peak footprint from the plan and checks it against the budget *before* running anything. OOM is reported at the commit line, attributed to the stage that peaks — not surfaced from deep inside a fused kernel after partial work. |
| **O3: Fusion is steerable** | Fusion boundaries are reported by `explain`. An explicit barrier forces a materialization point when you want to bound or inspect an intermediate. Contrast the profiler-trace archaeology of `fusion_1723`. |
| **O4: Deterministic by construction** | No racing scatter (P8 requires a combining op) and stable kernel selection mean a plan's result and structure don't drift run-to-run. Fewer invisible variables when chasing "why is it different now." |

```
> pixels.map(shade).map(tonemap).reduce(max, 0.0).explain()

plan (using Gpu, device 0)
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
| **L1: Core owns the algebra and the IR** | The `Wide[T]` type, the primitive set, the subset check (W4), staging/`commit`, `explain`, and — the part only the compiler can do — lowering pure functions reachable from a plan to **one portable kernel IR, SPIR-V** (D3). This cannot be a library; it needs the type system and a codegen target. |
| **L2: Core ships two backends** | `Simd` and `ThreadPool` are built in, because they *are* the existing SIMD path and thread pool — they come for free and give the baseline (W5) with no external dependency. |
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
FIX: monomorphize the call, or move it out of the plan (run it on the host before `.wide()`)
```

```
ERROR [conc.data-parallel/W3]: no active width context
   |
7  |     data.wide()
   |     ^^^^^^ `.wide()` needs a width block: `using Simd`, `using ThreadPool`, `using Gpu`, or `using Accelerated`
```

```
RUNTIME (returned, not panicked) [conc.data-parallel/C3]: GpuError.OutOfMemory
   committed plan needs 4.2 GB device memory; device has 3.1 GB free
   — surfaced from the `.commit()` at physics.rk:52
```

## Edge cases

| Case | Rule | Handling |
|------|------|----------|
| `.wide()` outside any width context | W3 | Compile error |
| Staged plan never committed | C1 | Warning — the plan is dead code, nothing ran (like an unused `Result`) |
| `commit` under `using Gpu` with no device present | C3, W5 | Returns `GpuError.NoDevice` — or use `using Accelerated`, which picks the fastest available width (here, cores) |
| Device out of memory at commit | C3 | Returns `GpuError.OutOfMemory`, not a panic |
| Two lanes scatter to the same index | P8 | Combined by the required `op`; no plain racing scatter exists |
| Dynamic dispatch inside a staged op | D4 | Compile error |
| Recursion inside a staged op | D4 | Compile error (device closure can't bound the stack) |
| Data race inside an explicit `kernel` | K2 | Not caught — programmer's responsibility, like `unsafe` |
| Branchy `map` lambda (divergence) | P1 | Allowed; tooling flags likely divergence (not enforced) |

---

## Appendix (non-normative)

### Rationale

**C2 (`commit` as the one run point):** The design's whole answer to "lazy hides when work happens" is that it doesn't — one verb, with transaction connotation, marks the run. `read` would have hidden it; `commit` advertises it. Cost and failure both land there (C3), so there's exactly one line to look at.

**D1 (color the data):** Coloring functions is the CUDA/SYCL disease — it's viral and splits the ecosystem. But the fork is real; something has to carry it. Putting it on the buffer type is honest (the memory space differs) and local (pure functions stay clean). This is the JAX/Futhark lineage.

**P4–P8 (primitives, not raw lanes):** You can't prove arbitrary index code is race-free — it's undecidable. So the language only lets you spell parallelism through operators whose dependency structure is known. Reductions and scans come for free because they're primitives, not hand-rolled barrier code.

**W2 (`Accelerated` = speed, not device):** Naming a specific width (`Gpu`, `ThreadPool`) is a guarantee about *where*. `Accelerated` deliberately drops that guarantee for a promise about *speed* — "fastest available for this plan and size." It's the one context where you don't statically know the device, opted into by name. The transparent path is always a specific width; `Accelerated` is the convenience, and even it reports what it chose.

**Observability as the point, not a feature:** The reason to trust `Wide` over a Python framework is that the plan is inspectable (`explain`), memory is accounted before launch, and there's no runtime retracing (AOT). Those aren't extras — they're what separates "a nicer JAX" from "the thing you trust at 3am when the kernel is OOMing." If they slip, the design loses its reason to exist.

### The one tension left

Laziness genuinely fights "costs visible in code" — that's why `commit` has to stay a loud, explicit, mandatory verb, why staging must never sneak in a hidden execution, and why fusion is inspectable rather than magic (O1–O3). If a future convenience makes some op auto-commit, this transparency breaks. Hold the line: nothing runs until `commit`, and the plan is always openable before it does.

### What v1 got wrong

The `dispatch |lane|` sketch failed six ways, and this design is the point-by-point answer:

| v1 weakness | v2 answer |
|-------------|-----------|
| "No coloring, same bet as async" was false | D1 — GPU *is* colored; put it on data, keep it off functions |
| No device concurrency/sync model | C1–C5 — staged plan is the handle, `commit` is the join |
| Per-lane independence check undecidable | P1–P9 — primitives encode dependency; raw lanes only behind `kernel` (K1) |
| Map-only skipped the cross-lane 80% | P4, P5, P8 — reduce/scan/scatter are first-class |
| "No fallback" was coloring at the block level | W4/W5 — the algebra *is* the GPU subset, so CPU (a superset) always runs it; portability is structural, not a fallback |
| Divergence invisible | P6–P8 — irregular ops are named operators, visible in source |

### TODO — what v2 still must settle

1. **Materialization across commits.** Keeping a result on-device between two commits (avoiding a host round-trip) — is that automatic via fusion, or an explicit on-device checkpoint? Affects multi-stage pipelines.
2. **`stencil` boundary handling (P9).** Halo/edge behavior — clamp, wrap, or caller-supplied — needs pinning down.
3. **Mixed-width plans.** Can one plan span `using Gpu` and `using ThreadPool` (offload part, keep part on cores)? Probably no for v1; confirm.
4. **The `GpuError` set (C3).** Enumerate: `OutOfMemory`, `DeviceLost`, `NoDevice`, `Unsupported`, transfer failures. Mirror the care `conc.async` gave `JoinError`. (Note: under `using Accelerated`, `NoDevice` can't occur — it degrades to cores instead.)
5. **Tooling for divergence (P1).** The lint that flags a branchy `map` lambda — spec what it detects and how it reads.
6. **The backend interface (L4).** Drafted in `conc.wide-backend` — the stable contract a library backend implements. Its own open questions (primitive-set representation, version policy, registration, async) live there.
7. **The `Accelerated` policy (W2).** How it decides (input size thresholds, measured vs. modeled cost), and whether the policy is a fixed stdlib default or itself pluggable. Must never become a hidden autotuner.
8. **`explain` memory formulas (O1).** How exact the footprint can be when `Wide` lengths are runtime values — symbolic in `N` at compile time, concrete at commit. Pin down what's guaranteed.
9. **Scope decision.** Whether accelerators are in Rask's stated target at all — a `CORE_DESIGN` call, not this file's.

### See also

- `conc.wide-backend` — the stable contract a device backend implements
- `type.simd` — the one-core width of this model (`using Simd`)
- `conc.async` — the quality bar; `commit`/plan mirrors `join`/handle
- `mem.boxes` — scoped access; `using Gpu` scopes device memory
- `mem.linear` — `Wide[T]` buffers follow the consume-once discipline
