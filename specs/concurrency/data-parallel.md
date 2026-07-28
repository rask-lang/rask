<!-- id: conc.data-parallel -->
<!-- status: proposed -->
<!-- summary: Data-parallel offload to GPUs and accelerators via a device box, a kernel region, and explicit dispatch — no function coloring -->
<!-- depends: memory/boxes.md, memory/linear.md, types/simd.md, concurrency/async.md -->

# Data-Parallel Offload (exploration)

**Status: proposed.** This is a design sketch, not a decision. It answers one question — *can Rask say "run this across 10,000 lanes" without betraying its principles?* — and shows what the answer would cost to build. Nothing here is settled.

## The gap

Rask has two parallelism stories today, both CPU-only:

- **SIMD** (`type.simd`) — `Vec[T, N]`, tens of lanes, one core.
- **Tasks + thread pool** (`conc.async`) — tens of cores.

Neither reaches the hardware this note is about: GPUs, NPUs, and other wide accelerators where the unit of work is *thousands of identical lanes over a buffer*. That class of program is growing, and Rask currently has nothing to say to it. Stretching `Vec[T, N]` to `N = 10000` isn't the answer — a GPU has a separate memory space, explicit transfers, launch latency, and severe penalties for divergent control flow. Pretending it's just a wider SIMD register would be a lie about the cost model, and cost honesty is principle #1.

So this is a genuinely new execution target, and the interesting question isn't "add a backend." It's whether Rask's existing shapes — boxes, `with`, regions, effect metadata — already compose into a data-parallel model that *avoids the mistakes other languages made here*. I think they mostly do.

## What it looks like

Three pieces: a **device box**, a **kernel region**, and **dispatch**.

<!-- test: skip -->
```rask
func scale_all(data: []f32, factor: f32) -> Vec[f32] {
    using Gpu {
        with device as d {
            const input  = d.upload(data)              // []f32 → Buffer[f32]  (transfer — visible)
            const output = d.alloc[f32](data.len())    // device allocation      (visible)

            dispatch(data.len()) |lane| {              // kernel region
                output[lane] = input[lane] * factor
            }

            return output.download()                   // Buffer[f32] → Vec[f32] (transfer — visible)
        }
    }
}
```

- `using Gpu { }` declares the capability, exactly like `using ThreadPool { }`. No device, no offload — a compile error at `with device`, not a silent CPU fallback.
- `with device as d` is the ordinary box `with` (`mem.boxes`). Buffers are reachable only inside it; at scope end, device allocations are freed. Same discipline as every other box.
- `d.upload` / `d.alloc` / `.download` are the cost. Each is an explicit call, so every host↔device transfer and every device allocation shows up in the source. Nothing moves across the bus invisibly.
- `dispatch(N) |lane| { … }` runs the closure body for lanes `0..N`. `lane` is the only per-invocation input. This is the kernel.

That's the whole surface. It reads like the rest of Rask because it *is* the rest of Rask — a capability, a box, a `with`, a closure.

## The one idea that matters: a region, not a color

Every other systems language solves "which code runs on the device" by **coloring functions**:

- CUDA/C++ tags functions `__global__` / `__device__`. A `__device__` function can't be called from the host. Two worlds, split at the signature.
- SYCL kernels are lambdas, but any function they call must live in the device translation unit.
- Rust's GPU story (`rust-gpu`, `cust`) puts kernels in separate crates with `#[spirv]` entry points and separate compilation.

This is the same disease as `async`/sync coloring: a function's *type* now records where it can run, and that color propagates up every caller. Rask rejected that for I/O (principle #5, `conc.async`). It would be incoherent to reintroduce it for GPUs.

**So the kernel constraint lives on the region, not on the function.** `dispatch(N) |lane| { … }` is a *lexical region*, like `comptime { }` or `unsafe { }` — a place in the source, not a property of a signature. Inside it, some things are illegal: no I/O, no host allocation, no locks, no unbounded recursion, no dynamic dispatch. The compiler checks the region by walking the code reachable from it and consulting the **effect metadata it already tracks** (`conc.async` calls effects "information without enforcement" — surfaced by tooling, not baked into types). If a called function performs a device-illegal effect, the error points at the call site *inside the region*, not at a missing annotation.

The payoff, concretely:

<!-- test: skip -->
```rask
func square(x: f32) -> f32 { return x * x }   // ordinary function. no @gpu. no @device.

// host use
const y = square(3.0)

// device use — same function, no second version, no color on its signature
dispatch(buf.len()) |lane| { out[lane] = square(in[lane]) }
```

`square` is legal in a kernel because *it does nothing device-illegal*, and the compiler can see that from the effect info it already has. No annotation, no duplicate `__device__` copy, no signature split. A function is kernel-legal when its behavior is, and Rask learns that by looking — not by making you paint it.

**This is the thing to react to.** If Rask ships GPU support as a region checked against effect metadata rather than a color on function types, it solves the exact problem that makes CUDA and SYCL codebases bifurcate. The effect-tracking machinery that principle #5 already committed to is what makes it possible. That's not a lucky accident — it's the same bet paying off twice (async was the first time).

## How the pieces reuse what exists

| Piece | Reuses | New? |
|-------|--------|------|
| `using Gpu { }` | `using` capability blocks (`conc.async`) | Capability wiring only |
| `with device as d` | box `with` access (`mem.boxes`) | `Device` is a resource box |
| `Buffer[T]` | linear/`Owned` discipline (`mem.linear`) | Consumed by `.download()` or freed at scope end |
| kernel region | `comptime`/`unsafe` region checking + effect metadata (`conc.async`) | One new analysis pass |
| `dispatch(N) \|lane\|` | closure syntax | A builtin, not new grammar |

The buffer is a box in the family (`mem.boxes`), sendable and linear like `Owned<T>` — you can't read device memory from the host without a `.download()`, and you can't leak a buffer past its `with device`. The linearity rules (`mem.linear`) already give "consumed exactly once"; a `Buffer` freed at scope end or moved into `.download()` fits without new rules.

## What it deliberately won't do

Naming the non-goals is how the region check stays simple and the cost model stays honest:

- **No host closures captured into kernels.** A kernel captures buffers and scalars, not arbitrary host state. (Keeps the address-space boundary real.)
- **No dynamic dispatch or heap allocation on-device.** Kernels are monomorphic and flat.
- **No I/O, no locks, no `ensure` inside a kernel.** Those are host effects; the region check rejects them.
- **No automatic CPU fallback.** If there's no device, `with device` fails to compile under `using Gpu`. Silent fallback would hide a 100× cost cliff — the opposite of transparency.
- **Divergence is not yet visible in the source.** Per-lane branches that diverge are the GPU's worst cost, and this sketch has no way to surface them. That's the biggest honesty gap here — see open questions.

## How hard is it, really?

Honest tiers, against the pipeline (`.rk → Lexer → Parser → Desugar → Resolve → TypeCheck → Comptime → Ownership → MIR → Codegen`):

**Cheap — frontend.** `with device`, `using Gpu`, and `dispatch(N) |lane|` need essentially no new grammar. `with` and `using` exist; `dispatch` is a builtin taking a count and a closure. Resolve/typecheck treat `Buffer[T]` as another box type. Days-to-weeks of parser/type work, not months.

**Moderate — the region check.** One new analysis pass: from each `dispatch` closure, walk the reachable call graph in MIR and reject device-illegal effects, reporting at the offending call site. This reuses the effect metadata and mirrors how `comptime`-legality is already checked. The work is writing good diagnostics (a first-class concern here — "you called `print` inside a kernel, that's a host effect" with the fix), not inventing analysis.

**The mountain — a second codegen backend.** Cranelift lowers MIR to CPU. Kernels need MIR → **SPIR-V** (portable across Vulkan/WebGPU) or WGSL. This is the real cost. Two things make it *bounded* rather than open-ended:

1. Kernel MIR is a **subset** — numeric ops, array indexing, bounded loops, per-lane branches. No heap, no runtime calls, no closures-in-closures. You lower that subset, not all of MIR.
2. The region check *guarantees* only that subset reaches the backend, so the backend can reject-by-assertion anything it doesn't handle, and the error still surfaces as a clean diagnostic upstream.

Still, a working MIR→SPIR-V lowering plus a host-side driver (buffer alloc, transfer, launch — a runtime library alongside the existing C runtime, targeting Vulkan or WebGPU first for portability) is the bulk of the effort by a wide margin. Everything else is small next to it.

**Net:** the language design is cheap and reuses Rask's existing shapes almost perfectly. The compiler cost is concentrated in one place — a second backend — and that place is a bounded subset problem, not a rewrite. If someone wanted to prototype, the order is: (1) region check with diagnostics over a CPU-simulated `dispatch` (runs kernels as a parallel-for on the thread pool — proves the model end-to-end with zero backend work), then (2) SPIR-V backend behind the same surface.

That staging matters: **step 1 delivers the whole programming model — coloring-free kernels, cost-visible transfers, the region check — using only the CPU.** You can validate whether the design feels like Rask before committing to the backend mountain.

## Open questions

- **Divergence cost.** How does per-lane branch divergence become visible in the source, the way an allocation is? Without an answer, the model hides the GPU's sharpest cost — a real tension with principle #1.
- **`native` lane groups.** SIMD has `native` width; does dispatch expose workgroup/warp size, or stay fully abstract? Abstract is simpler; exposing it may be necessary for reductions.
- **Reductions across lanes.** `dispatch` writes per-lane outputs cleanly. Cross-lane reductions (sum over 10,000 lanes) need either a device-side reduce primitive or a documented multi-pass pattern.
- **Buffer aliasing.** Can two buffers in one `with device` overlap? The box/linear rules say no by construction, but gather/scatter (`type.simd/MEM4`) complicates it.
- **Is this in scope at all?** CORE_DESIGN draws the line at "web services, CLI, games, data pipelines." Data pipelines and game physics *want* this; web/CLI never will. Committing means widening the stated target — a decision for `CORE_DESIGN`, not this file.

---

## Appendix (non-normative)

### Why the region approach is the whole argument

If you take one thing from this note: the reason Rask can plausibly do GPUs *well* is that it already refused to color functions for async. That refusal forced an effect-metadata system (principle #5). GPU kernels are the second customer for that same system. A language that colored async would have to color kernels too, and end up with the two-worlds split that makes CUDA/SYCL code hard to share. Rask gets to check a region instead of paint a signature — and the same `square` runs in both worlds, unannotated. That's the payoff, and it's worth prototyping the CPU-simulated version just to feel whether it holds up.

### See also

- `type.simd` — CPU vector types (the tens-of-lanes story)
- `conc.async` — capability blocks, no function coloring (the precedent)
- `mem.boxes` — scoped access via `with` (the device box shape)
- `mem.linear` — consume-exactly-once (the buffer discipline)
