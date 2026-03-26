<!-- id: raido.overview -->
<!-- status: proposed -->
<!-- summary: Raido — Allgard's verification layer, deterministic scripting VM -->

# Raido

[Allgard](../allgard/)'s verification layer. Deterministic scripting VM with Rask-flavored syntax. No dependency on the Rask compiler, runtime, or stdlib — also usable standalone.

Serializable state. Fixed-point arithmetic. Sandboxed — the host controls all capabilities.

Within Allgard, Raido is the engine for [verifiable transforms](../allgard/README.md#verifiable-transforms). Every mint and burn is a Raido script that any domain can re-execute to verify independently. General transforms can optionally be verified the same way. See [Protocol Role](#protocol-role).

**Rask-flavored syntax.** Same `{}` blocks, `if`/`else if`, `match`/`=>`, `for`/`in`, `||` closures, `try`/`else` error handling. No type annotations, no ownership, no `ensure`.

## Why Raido

Applications need an answer to "run user-provided code safely." Hosting Lua via C FFI works but:

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
| Safe host interop | Yes (no unsafe) | No | No | No |
| Tiny VM | ~1 KB base | ~20 KB | Runtime-dependent | Heavy |

## Host API

Example using Rask as the host language (any language with C FFI can host Raido):

```rask
import raido

const vm = raido.Vm.new(raido.Config {
    arena_size: 256.kilobytes(),
    initial_fuel: 100_000,
    max_call_depth: 256,
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
| VM | Register-based | ~30% fewer dispatched instructions than stack-based. Serialization equivalent (register window = value array per frame). Compiler cost is one-time. |
| Numbers | Fixed-point 32.32 | Deterministic (integer math) + fast (hardware ALU). |
| State | Fully serializable | Save/restore, migration, replay, checkpointing. |
| Sandbox | No I/O. Host provides all capabilities. | Safe to run untrusted code. |
| Collections | Array `[]` + Map `{k: v}` | Maps to Rask's Vec/Map. |
| Host data | Opaque references with host-registered vtables | Field name → slot index at compile time. No string lookup at runtime. |
| Functions | Host functions by name | Serializable. Re-registered on restore. |
| Globals | Explicit `global` keyword | No accidental globals. |
| Strings | `"value: {x}"` interpolation | Kills concatenation chains. |
| Random | Seedable PRNG in VM state | Deterministic. Serializable. |
| Errors | `try`/`else`, `error()` | Same syntax as Rask. No `pcall`. |
| Coroutines | `coroutine(f)`, methods | Create/resume/yield. Method-based, not Lua module-style. |
| Stdlib | `core` always present, modules opt-in by host | `core` is the language. Modules are capabilities. Host controls the surface area. |

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

## Specs

### Language — what Raido programs mean

| Spec | What it covers |
|------|----------------|
| [language/types.md](language/types.md) | Value types, fixed-point numbers, closures, host references |
| [language/syntax.md](language/syntax.md) | Grammar, variables, functions, control flow, operators |
| [language/coroutines.md](language/coroutines.md) | Cooperative multitasking |
| [language/stdlib.md](language/stdlib.md) | Configurable built-in modules |

### VM — how the machine executes

| Spec | What it covers |
|------|----------------|
| [vm/architecture.md](vm/architecture.md) | Register VM, arena, instruction set, upvalue storage, serialization |
| [vm/chunk-format.md](vm/chunk-format.md) | Bytecode format, imports/exports, validation, content identity |
| [vm/interop.md](vm/interop.md) | Host API, vtables, host functions, scoped bindings, error propagation |

## Protocol Role

Raido is independent — usable by any host, no dependency on Allgard or Leden. But its properties (determinism, serializable state, content-addressed bytecode) make it a natural fit as a verification engine for federated systems.

[Allgard](../allgard/) defines verifiable transforms as an optional extension. The flow:

1. Domain A publishes a Raido script as a content-addressed chunk
2. A cross-domain Transform references the script hash + inputs + outputs
3. Domain B fetches the script, re-executes with the same inputs
4. Determinism guarantees identical output — the Proof is mechanically verified

This turns Allgard's trust-based Proofs (Conservation Law 4) into independently verifiable ones for any transform backed by a Raido script. Simple transforms (transfer, burn) don't need this — signature + causal link is sufficient.

**What Raido provides:** deterministic execution, content-addressed identity (chunk format), versioned serialization.

**What Raido doesn't know about:** Allgard primitives, Leden sessions, federation semantics. It's a VM. The protocol integration is Allgard's concern.

Capability negotiation happens at the Leden layer — two domains agree "we both support Raido v*N* verification." Allgard defines what that means semantically. Raido just executes bytecode and returns the same answer every time.

## Resolved

| Question | Decision | Rationale |
|----------|----------|-----------|
| Fixed-point format | **32.32** | ~9.6 decimal digits of precision vs 48.16's ~4.8. Simulation shows 48.16 drifts badly in physics chains (2.6 error after 1000 damping frames vs 6e-5). The 2.1B integer ceiling is mitigated by `int` (i64) being a separate type — use `int` for large values. |
| Arena strategy | **Fixed arena + explicit reset.** `frame_end()` opt-in. | Default: fixed-size arena, `reset()` between evaluations. Game-loop hosts opt into `frame_end()` for per-frame cleanup. No auto-grow — hides allocation cost. |
| Serialization versioning | **Yes.** Version header from day one. | Without it, any format change breaks all serialized snapshots. |
| Packaging | **Independent project** (`raido` crate). Lives in this repo for now. No dependency on Rask. | Not part of Rask. Can be embedded by any host via C FFI. |
| Closure upvalues | **Arena-allocated.** Closures hold arena offsets. | No heap cells, no GC. Multiple closures sharing a variable point to the same arena slot. Serializable as part of arena contents. |
| Host ref field access | **Vtable.** Field name → slot index at compile time. | No string hashing at runtime. One indexed function pointer call per access. Slot indices stable because field order declared by host. |

## Resolved

**String encoding depth.** ASCII-only for case operations (`upper`/`lower`), byte-indexed for slicing (`sub`/`byte`). No Unicode-aware operations in the VM. If a script needs Unicode (rare for game scripting, modding, rules), the host provides it as a host function. Keeps the VM tiny and deterministic — Unicode case mapping tables are large and version-dependent.

## Deferred

- **Map growth strategy.** Load factor threshold and growth factor need benchmarking. Open addressing with linear probing is decided; the tuning constants are implementation detail.
- **Serialization migration.** Version header exists. Forward/backward compatibility policy depends on how the format evolves in practice. Premature to specify migration rules before the first format change.
