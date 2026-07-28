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
| **C4: Fusion between commits** | The scheduler sees the whole plan at commit, so chained ops fuse into as few kernels as possible — intermediates never touch device memory. `xs.map(f).map(g).sum()` is one kernel, not three plus two temp buffers. |
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
| **W1: Widths** | `using Simd` = one core's vector unit. `using ThreadPool` = cores. `using Gpu` = the device. `using Accelerated` = the widest available (GPU if present, else cores). |
| **W2: Context picks the width, visibly** | The width is the `using` block at the top of the region — explicit, never inferred. `using Accelerated` is the one adaptive choice, and it too is named in the source, so degradation is never a silent surprise. |
| **W3: One subset, checked once** | A plan type-checks against the GPU subset regardless of width. There is no per-backend capability matrix — a plan that is legal is legal on *all* widths. You never get "runs on cores but not GPU." Need more than the subset? Drop out of `Wide` into ordinary host Rask. |
| **W4: CPU is baseline, not fallback** | Switching `using Gpu` → `using ThreadPool` is a one-line, always-valid change, *guaranteed* by the subset relation — not a degraded emergency path. |
| **W5: CPU run is the reference semantics** | What a plan *means* is defined by its CPU execution. The GPU backend is correct iff it matches, modulo documented float reassociation in reductions (`type.simd/R2`). This gives a test oracle: run any plan on both widths and diff. |

The payoff is twofold. First, `using Simd` **is** today's `type.simd`, re-expressed — the narrowest width of one model, not a bolt-on; `Vec[T, N]` is the fixed-width, one-core corner of `Wide[T]`. Second, portability is *structural*, not hoped-for: because the subset is fixed by what the GPU can do, the question was never *can* the CPU run a plan (a superset always can) — only whether it runs it *fast*.

## The primitive algebra

You express parallelism *only* through these primitives. Each has a known dependency structure, so the compiler never has to prove independence (see W3 in the old note — that was undecidable; here it's encoded in the operator).

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

## Escape hatch: explicit kernels

The algebra covers the ~80% that decomposes into primitives. Hand-tuned kernels — custom shared-memory tiling, exotic access patterns — are the other 20%.

| Rule | Description |
|------|-------------|
| **K1: `kernel` is unsafe-tier** | An explicit `kernel(n) |lane| { … }` block writes raw per-lane code. It sits at the same trust level as `unsafe { }` — the compiler does not check per-lane independence. |
| **K2: You own the proof** | Inside a `kernel`, writing `out[lane]` from data other lanes also write is a data race the compiler will not catch. The block is where "I promise this is independent" lives, spelled out, like raw pointers. |

The safe algebra is the main road; `kernel` is the labelled off-ramp. An undecidable independence check doesn't belong in the checked language — it belongs behind the same "I promise" boundary as `unsafe`.

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
ERROR [conc.data-parallel/W2]: no active width context
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
| `.wide()` outside any width context | W2 | Compile error |
| Staged plan never committed | C1 | Warning — the plan is dead code, nothing ran (like an unused `Result`) |
| `commit` under `using Gpu` with no device present | C3, W4 | Returns `GpuError.NoDevice` — or use `using Accelerated` to run the same plan on cores automatically |
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

### The one tension left

Laziness genuinely fights "costs visible in code" — that's why `commit` has to stay a loud, explicit, mandatory verb, and why staging must never sneak in a hidden execution. If a future convenience makes some op auto-commit, this transparency breaks. Hold the line: nothing runs until `commit`.

### What v1 got wrong

The `dispatch |lane|` sketch failed six ways, and this design is the point-by-point answer:

| v1 weakness | v2 answer |
|-------------|-----------|
| "No coloring, same bet as async" was false | D1 — GPU *is* colored; put it on data, keep it off functions |
| No device concurrency/sync model | C1–C5 — staged plan is the handle, `commit` is the join |
| Per-lane independence check undecidable | P1–P9 — primitives encode dependency; raw lanes only behind `kernel` (K1) |
| Map-only skipped the cross-lane 80% | P4, P5, P8 — reduce/scan/scatter are first-class |
| "No fallback" was coloring at the block level | W3/W4 — the algebra *is* the GPU subset, so CPU (a superset) always runs it; portability is structural, not a fallback |
| Divergence invisible | P6–P8 — irregular ops are named operators, visible in source |

### TODO — what v2 still must settle

1. **Materialization across commits.** Keeping a result on-device between two commits (avoiding a host round-trip) — is that automatic via fusion, or an explicit on-device checkpoint? Affects multi-stage pipelines.
2. **`stencil` boundary handling (P9).** Halo/edge behavior — clamp, wrap, or caller-supplied — needs pinning down.
3. **Mixed-width plans.** Can one plan span `using Gpu` and `using ThreadPool` (offload part, keep part on cores)? Probably no for v1; confirm.
4. **The `GpuError` set (C3).** Enumerate: `OutOfMemory`, `DeviceLost`, `NoDevice`, `Unsupported`, transfer failures. Mirror the care `conc.async` gave `JoinError`. (Note: under `using Accelerated`, `NoDevice` can't occur — it degrades to cores instead.)
5. **Tooling for divergence (P1).** The lint that flags a branchy `map` lambda — spec what it detects and how it reads.
6. **Scope decision.** Whether accelerators are in Rask's stated target at all — a `CORE_DESIGN` call, not this file's.

### See also

- `type.simd` — the one-core width of this model (`using Simd`)
- `conc.async` — the quality bar; `commit`/plan mirrors `join`/handle
- `mem.boxes` — scoped access; `using Gpu` scopes device memory
- `mem.linear` — `Wide[T]` buffers follow the consume-once discipline
