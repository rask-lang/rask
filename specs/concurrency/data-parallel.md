<!-- id: conc.data-parallel -->
<!-- status: proposed -->
<!-- summary: Early sketch of map-only data-parallel offload — deliberately incomplete, known weaknesses documented -->
<!-- depends: memory/boxes.md, memory/linear.md, types/simd.md, concurrency/async.md -->

# Data-Parallel Offload (early sketch — weak on purpose)

**Status: proposed, and known-incomplete.** This is not at the bar of `conc.async`. It covers one easy case (independent map over a buffer), models the hardware as synchronous when it isn't, and leans on an earlier claim — "GPUs without function coloring, same bet as async" — that does not survive scrutiny. It's kept because the *shape* (device buffer as a box, explicit transfers) is worth reacting to, and because writing down why it's fragile is more useful than a confident sketch that hides the holes. Read "Known weaknesses" before taking anything here as a direction.

## The gap it's aimed at

Rask has two parallelism stories, both CPU-only:

- **SIMD** (`type.simd`) — `Vec[T, N]`, tens of lanes, one core.
- **Tasks + thread pool** (`conc.async`) — tens of cores.

Neither reaches GPUs, NPUs, or other wide accelerators where the unit of work is *thousands of identical lanes over a buffer*. Stretching `Vec[T, N]` to `N = 10000` isn't the answer — a GPU has a separate memory space, explicit transfers, launch latency, and severe penalties for divergent control flow. Modelling it as a wider SIMD register would lie about the cost model.

That much is solid. Everything below is where it gets shaky.

## What the surface might look like

Three pieces: a **device box**, a **kernel region**, and **dispatch**. This is the *only* case the sketch handles — an independent per-lane map:

<!-- test: skip -->
```rask
func scale_all(data: []f32, factor: f32) -> Vec[f32] {
    using Gpu {
        with device as d {
            const input  = d.upload(data)              // []f32 → Buffer[f32]  (transfer)
            const output = d.alloc[f32](data.len())    // device allocation

            dispatch(data.len()) |lane| {              // kernel region — map only
                output[lane] = input[lane] * factor
            }

            return output.download()                   // Buffer[f32] → Vec[f32] (transfer + sync)
        }
    }
}
```

The parts worth keeping:

- `with device as d` is the ordinary box `with` (`mem.boxes`). Buffers are reachable only inside it. A `Buffer[T]` fits the linear/`Owned` discipline (`mem.linear`) — consumed by `.download()` or freed at scope end.
- `d.upload` / `d.alloc` / `.download` are explicit calls, so transfers and device allocations are visible in the source. That's the one real transparency win.

The parts that are already wrong are in the next section — starting with the fact that this code *reads* like three blocking calls over hardware that is fundamentally asynchronous.

## Known weaknesses

These are load-bearing, not polish. Ranked worst first.

### W1 — "No function coloring, same bet as async" is false

Async earns no-coloring because there is no real fork: a function runs the *same compiled code* whether called from a green task or sync — the runtime just pauses the task instead of the thread. A GPU kernel is the opposite. The "same" `square` in a `dispatch` is compiled to a **different ISA** (SPIR-V, not x86), runs in a **different address space**, with a **different capability set** (no heap, no I/O, no recursion), under a **different cost model**. That is not one function behaving the same way — it's one source text compiled to two incompatible targets. CUDA/SYCL color functions because the color carries real information ("this artifact exists as device code"); it isn't stubbornness.

What actually survives is much narrower: the compiler can reachability-close from a `dispatch` and emit device code for every pure function it reaches (this is `rask-mono` retargeted). That avoids **annotation**, not coloring. And the closure breaks exactly where async is robust — function pointers, dynamic dispatch, stored closures. Async falls back to a runtime panic there; a kernel *can't* (the device code doesn't exist). So the model is forced to **ban** those constructs rather than handle them. The clean "region not color" story is really "we amputated everything that would force a color."

**Correct framing:** GPU *is* a colored domain. The only design choice is *where the color lives* — Rask's bet is a lexical region plus compiler-driven reachability instead of a signature annotation. That buys no-annotation and pure-function sharing, and buys nothing on portability, reductions, async, or dynamic dispatch.

### W2 — No concurrency model for the device (in a concurrency spec)

Real dispatch is asynchronous: enqueue upload, enqueue kernel, enqueue download, *then synchronize*. The example above reads like three blocking calls. There is no handle, no explicit sync point, and no way to express "launch these three kernels, then wait" — the exact lifetime machinery `conc.async` gets right with must-use handles and drain-on-exit. Making `.download()` the implicit sync point silently serializes independent kernels and throws away the overlap that is the whole point of the hardware. A device dispatch needs a handle story at least as careful as `TaskHandle`; this sketch has none.

### W3 — E1 (per-lane independence) writes a check that can't be cashed

The earlier draft claimed the compiler enforces that no lane reads another lane's output. Deciding that for `out[lane] = in[f(lane)]` is alias analysis over arbitrary index arithmetic — undecidable in general. So the rule is either (a) a brutal syntactic restriction ("you may only write `out[lane]`, index literally `lane`"), which makes stencils, gather, and matmul tiles illegal and reduces the model to a toy, or (b) unsound. It has to pick one, out loud. `conc.async` never claims to check something undecidable — must-use handles are a genuinely checkable property.

### W4 — Map-only covers the easy 20%

`dispatch |lane|` handles scale-a-buffer. It does **not** handle sum, dot product, softmax, prefix-sum, or histogram — anything needing cross-lane communication (workgroups, shared memory, barriers). That layer is absent and it is not optional; it's most of what people actually offload. `conc.async` is a complete model of its domain (channels, select, cancellation, groups). This is the easy slice with the hard majority deferred.

### W5 — "No fallback" is coloring, resurfaced at the block level

A program using `dispatch` can't run on a machine without the accelerator (`with device` is a compile error under `using Gpu`). So the *program* is now colored by whether it contains a `with device` block — the same coloring evicted from signatures, back at the block level. Async runs the same program with or without its runtime; this doesn't. "No silent cost cliff" is a defensible reason, but calling it a transparency win while it reintroduces coloring is dishonest.

### W6 — It fails its own headline metric

By making device code look identical to host code (great syntactic-noise score), the sketch *hides* that a different machine, memory space, and cost model are in play — the opposite of Transparency of Cost, the metric that matters most here. Divergence, the GPU's sharpest cost, has no representation in the source at all.

## How hard is a *real* version?

Honest tiers, against the pipeline (`.rk → Lexer → Parser → Desugar → Resolve → TypeCheck → Comptime → Ownership → MIR → Codegen`):

- **Cheap — frontend.** `with device`, `using Gpu`, `dispatch(N) |lane|` need little new grammar. Days-to-weeks.
- **Moderate — the region check.** One analysis pass walking the call graph from each `dispatch`, rejecting device-illegal effects at the call site. Reuses effect metadata and the `comptime`-legality pattern. The work is diagnostics.
- **The mountain — a second codegen backend.** MIR → SPIR-V (portable across Vulkan/WebGPU) plus a host driver (alloc, transfer, launch). Bounded because kernel MIR is a subset, but it's the bulk of the effort by far.

None of that is the real difficulty, though. W1–W4 are: the design is unfinished at the *model* level, before any backend exists. The cheap frontend would just let you write the toy sooner.

**If anyone prototypes:** do step 1 only — the region check plus a CPU-simulated `dispatch` (run kernels as a parallel-for on the thread pool). That validates the *ergonomics* end-to-end with zero backend work, and — more importantly — forces W2 and W3 into the open, because you have to pick an actual independence rule and an actual sync model to make it run at all.

---

## TODO — what a non-fragile version has to settle

Ordered by how much each unblocks the rest. None is a footnote; each is real design work.

1. **Pick the framing (W1, W5).** Drop "no coloring." State plainly that GPU is a colored domain and Rask's choice is region + reachability over signature annotation. Decide whether the program-level color (has-a-device-block) is acceptable or needs a fallback path. Everything downstream depends on this being honest.
2. **Design the dispatch handle and sync model (W2).** What `dispatch` returns, when the host blocks, how to overlap independent kernels, how buffers stay alive until a kernel that reads them completes. Should reach the care level of `TaskHandle` / drain-on-exit in `conc.async`.
3. **Pin down the independence rule (W3).** Choose the *checkable* syntactic restriction (likely: write only `out[lane]`, reads unrestricted) and state exactly what it forbids. If stencils/gather are out, say so; they belong to the reduction/shared-memory layer, not raw `dispatch`.
4. **Design the cross-lane layer (W4).** Workgroups, shared memory, barriers, and reduction primitives (`sum`, `scan`, `histogram`). This is the 80% the map model skips, and it drags in warp/workgroup size — decide whether that's exposed (like SIMD `native`) or abstract.
5. **Make divergence visible (W6).** Find the source-level representation for per-lane branch divergence, the way an allocation is visible. Without it the design fails its own transparency metric. This may be the hardest item and has no obvious answer yet.
6. **Reachability-to-device closure (W1 detail).** Specify how pure functions reachable from a kernel get compiled to device code, and exactly which constructs (function pointers, dynamic dispatch, closures-over-host-state, recursion) are rejected and with what diagnostic. This is the concrete mechanism the "region not color" claim actually rests on.
7. **Scope decision.** Whether accelerators are in Rask's target at all — a `CORE_DESIGN` question, not this file's. Data pipelines and game physics want it; web/CLI never will.

Until items 1–5 have answers, this stays `proposed` and map-only. Don't cite it as a direction.

### See also

- `type.simd` — CPU vector types (the tens-of-lanes story)
- `conc.async` — the quality bar this note does not yet meet
- `mem.boxes` — scoped access via `with` (the device box shape, the one solid borrowing)
- `mem.linear` — consume-exactly-once (the buffer discipline)
