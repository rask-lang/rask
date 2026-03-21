<!-- id: raido.overview -->
<!-- status: proposed -->
<!-- summary: Raido — deterministic, serializable, embeddable scripting VM for Rask -->

# Raido

Deterministic embeddable scripting VM for Rask. Dynamic subset of Rask syntax. Serializable state. Fixed-point arithmetic. Sandboxed — host controls all capabilities.

**Rask without types.** Same `{}` blocks, `if`/`else if`, `match`/`=>`, `for`/`in`, `||` closures, `try`/`else` error handling. No type annotations, no ownership, no `ensure`.

## Why Raido

Rask needs an answer to "run user-provided code safely." Embedding Lua via C FFI works but:

1. Every Lua API call requires `unsafe`.
2. Lua is not deterministic (hardware floats, platform-dependent behavior).
3. Lua state is not trivially serializable.
4. Lua's `longjmp` errors skip `ensure` blocks.
5. Syntax discontinuity — Raido scripts read like untyped Rask.

Raido's differentiators vs Lua/WASM/JS:

| Property | Raido | Lua | WASM | JS |
|----------|-------|-----|------|----|
| Deterministic | Yes (fixed-point) | No | Yes | No |
| Serializable state | Yes (built-in) | No | Manual | No |
| Sandboxed | Yes (no I/O) | Partial | Yes | Partial |
| Safe Rask interop | Yes (no unsafe) | No | No | No |
| Tiny VM | ~1 KB base | ~20 KB | Runtime-dependent | Heavy |

## Host API

```rask
import raido

const vm = raido.Vm.new(raido.Config {
    arena_size: 256.kilobytes(),
    instruction_limit: 100_000,
})
ensure vm.close()

const chunk = try vm.compile("script.raido", source)
try vm.exec(chunk)

// Call a script function
const result = try vm.call("process", [raido.Value.int(42)])

// Serialize entire VM state
const snapshot = vm.serialize()

// Restore later, possibly on a different machine
const vm2 = raido.Vm.deserialize(snapshot)
```

## Script Example

```raido
func process(input) {
    const data = try transform(input)       // propagate errors
    return if data > threshold { data } else { 0 }
}

func transform(x) {
    if x < 0 { error("negative input") }
    return x * scale + offset
}

// Error handling
const result = try process(42) else |e| {
    log("failed: {e}")
    return 0
}
```

## Key Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Syntax | Dynamic Rask subset | No new language to learn. Modders are learning Rask. |
| VM | Stack-based | Simple to implement, simple to serialize. |
| Numbers | Fixed-point 32.32 | Deterministic (integer math) + fast (hardware ALU). |
| State | Fully serializable | Save/restore, migration, replay, checkpointing. |
| Sandbox | No I/O. Host provides all capabilities. | Safe to run untrusted code. |
| Collections | Array `[]` + Map `{k: v}` | Maps to Rask's Vec/Map. |
| Host data | Opaque references with host-registered accessors | VM doesn't know about pools/ECS/DB. Host decides. |
| Functions | Host functions by name | Serializable. Re-registered on restore. |
| Globals | Explicit `global` keyword | No accidental globals. |
| Strings | `"value: {x}"` interpolation | Kills concatenation chains. |
| Random | Seedable PRNG in VM state | Deterministic. Serializable. |
| Errors | `try`/`else`, `error()` | Same syntax as Rask. No `pcall`. |
| Coroutines | `coroutine(f)`, methods | Create/resume/yield. Method-based, not Lua module-style. |
| Config | Host configures available stdlib, limits, capabilities | VM is a blank slate. Host shapes the environment. |

## Use Cases

| Use case | Key properties used |
|----------|-------------------|
| Game entity scripts | Host refs for entities, coroutines for AI, determinism for netcode |
| Workflow engine | Serialize/resume at each step, deterministic for audit |
| Plugin system | Sandbox, instruction limits, host-controlled capabilities |
| Rule engine | Deterministic evaluation, reproducible results |
| Bot scripting | Sandbox, limits prevent abuse, host provides API |
| Data transforms | Deterministic, serializable for checkpointing |
| Simulation | Deterministic, serializable for snapshots/replay |

## Detailed Specs

| Spec | What it covers |
|------|----------------|
| [values.md](values.md) | Types, fixed-point, serializable representation, host references |
| [syntax.md](syntax.md) | Grammar, variables, functions, control flow, operators |
| [vm.md](vm.md) | Stack VM, determinism, serialization, arena, instruction set |
| [interop.md](interop.md) | VM lifecycle, host functions, host references, error propagation |
| [coroutines.md](coroutines.md) | Cooperative multitasking |
| [stdlib.md](stdlib.md) | Configurable built-in modules |

## Resolved

| Question | Decision | Rationale |
|----------|----------|-----------|
| Fixed-point format | **32.32** | ~9.6 decimal digits of precision vs 48.16's ~4.8. Simulation shows 48.16 drifts badly in physics chains (2.6 error after 1000 damping frames vs 6e-5). The 2.1B integer ceiling is mitigated by `int` (i64) being a separate type — use `int` for large values. |
| Arena strategy | **Fixed arena + explicit reset.** `frame_end()` opt-in. | Default: fixed-size arena, `reset()` between evaluations. Game-loop embedders opt into `frame_end()` for per-frame cleanup. No auto-grow — hides allocation cost. |
| Serialization versioning | **Yes.** Version header from day one. | Without it, any format change breaks all serialized snapshots. |
| Packaging | **Separate crate** (`raido`), not part of Rask stdlib. | Most programs won't embed a scripting VM. No reason to bloat the stdlib. |

## Open Questions

- How do closures serialize when they capture mutable upvalues?
- Host reference field access: callback-based or vtable?
