<!-- id: raido.overview -->
<!-- status: proposed -->
<!-- summary: Raido -- deterministic scripting VM, verification-first, scripting-capable -->

# Raido

Independent project. Deterministic scripting VM with static types and Rask-flavored syntax. Lives in this repo for now but is not part of Rask -- no dependency on the Rask compiler, runtime, or stdlib.

Serializable state. Fixed-point arithmetic. Sandboxed -- the host controls all capabilities.

Raido is also the verification engine for [Allgard's verifiable transforms](../allgard/README.md#verifiable-transforms). Two domains that both support Raido can mechanically verify each other's scripted transforms instead of relying on trust alone. See [Protocol Role](#protocol-role).

**Verification-first, scripting-capable.** Static types and pure-function defaults serve verification. Coroutines and function references serve game scripting. Features that help neither get cut.

Two use case families, one VM:

- **Verification** (Allgard minting, Apeiron physics/combat/crafting): pure functions, determinism is load-bearing, content-addressed proofs. A trading partner re-executes the same bytecode with the same inputs and checks the output matches.
- **Content scripting** (NPC AI, dialogue, game logic, GDL client scripts): multi-tick behavior, host entity interaction, sequential logic that yields between ticks.

What they share: determinism, bounded resources, host interop, structured data (structs/enums), content addressing.

## Why Raido

Applications need an answer to "run user-provided code safely." Hosting Lua via C FFI works but:

1. Every Lua API call requires `unsafe`.
2. Lua is not deterministic (hardware floats, platform-dependent behavior).
3. Lua state is not trivially serializable.
4. Lua's `longjmp` errors skip `ensure` blocks.
5. Syntax discontinuity -- Raido scripts read like Rask.

Raido's differentiators vs Lua/WASM/JS:

| Property | Raido | Lua | WASM | JS |
|----------|-------|-----|------|----|
| Deterministic | Yes (fixed-point) | No | Yes | No |
| Serializable state | Yes (built-in) | No | Manual | No |
| Sandboxed | Yes (no I/O) | Partial | Yes | Partial |
| Safe host interop | Yes (no unsafe) | No | No | No |
| Static types | Yes | No | Yes | No |
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

// Register extern bindings before loading
vm.register_extern_struct("Enemy", raido.ExternStruct {
    fields: [
        raido.Field.int("health", get_health, set_health),
        raido.Field.number("x", get_x, set_x),
        raido.Field.number("y", get_y, set_y),
        raido.Field.string("name", get_name, null),  // readonly
    ],
})
vm.register_extern_func("move_to", move_to_handler)

const chunk = try vm.compile("script.rd", source)
try vm.load(chunk)  // fails if extern declarations don't match bindings

const result = try vm.call("process", [raido.Value.int(42)])

// Serialize entire VM state
const snapshot = vm.serialize()

// Restore later, possibly on a different machine
const vm2 = raido.Vm.deserialize(snapshot)
```

## Script Example

```raido
struct Vec2 { x: number, y: number }

extern struct Enemy {
    health: int
    x: number
    y: number
    readonly name: string
}

extern func move_to(entity: Enemy, target: Vec2)

func chase(attacker: Enemy, target: Enemy) {
    const dest = Vec2 { x: target.x, y: target.y }
    move_to(attacker, dest)
}

func process(input: int) -> int {
    const data = try transform(input)
    return if data > threshold { data } else { 0 }
}

func transform(x: int) -> int or string {
    if x < 0: error("negative input")
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
| Identity | Verification-first, scripting-capable | Static types serve verification. Coroutines serve scripting. |
| Syntax | Typed Rask subset | Same feel as Rask. Type annotations on signatures, inference for locals. |
| Types | Static, inferred locals | Catches errors at compile time. Eliminates runtime type checking. 8-byte values. |
| VM | Register-based | ~30% fewer dispatched instructions than stack-based. |
| Numbers | Fixed-point 32.32 | Deterministic (integer math) + fast (hardware ALU). |
| State | Fully serializable | Save/restore, migration, replay, checkpointing. |
| Sandbox | No I/O. Host provides all capabilities. | Safe to run untrusted code. |
| Structs/enums | User-defined, fixed layout | Structured data is the primary data model for all consumers. |
| Collections | `array<T>` + `map<K, V>` | Maps to Rask's Vec/Map. Homogeneous, typed. |
| Host data | `extern struct` / `extern func` | Compile-time type checking. Type mismatch = load error, not runtime error. |
| Functions | Function references (no closures) | No captured state, no arena allocation. Enables composition without verification hazard. |
| Optionals | `T?` with `Some`/`None` | Compiler-enforced null safety. No nil. |
| Strings | `"value: {x}"` interpolation | Kills concatenation chains. |
| Random | Seedable PRNG in VM state | Deterministic. Serializable. |
| Errors | `try`/`else`, `error()` | Same syntax as Rask. No `pcall`. |
| Coroutines | `coroutine(f, args...)`, methods | Create/resume/yield. Method-based. Serializable. |
| Stdlib | Core functions + built-in methods always present, `math`/`bit` opt-in | Collection/string methods are always available. Host controls math and bitwise access. |

## Use Cases

| Use case | Key properties used |
|----------|-------------------|
| Game entity scripts | Extern structs for entities, coroutines for AI, determinism for netcode |
| Workflow engine | Serialize/resume at each step, deterministic for audit |
| Plugin system | Sandbox, instruction limits, host-controlled capabilities |
| Rule engine | Deterministic evaluation, reproducible results |
| Bot scripting | Sandbox, limits prevent abuse, host provides API |
| Data transforms | Deterministic, serializable for checkpointing |
| Simulation | Deterministic, serializable for snapshots/replay |
| Verification | Content-addressed bytecode, re-execution produces identical results |

## Specs

### Language -- what Raido programs mean

| Spec | What it covers |
|------|----------------|
| [language/types.md](language/types.md) | Static type system, fixed-point numbers, structs, enums, optionals, function references |
| [language/syntax.md](language/syntax.md) | Grammar, variables, functions, control flow, operators, declarations |
| [language/coroutines.md](language/coroutines.md) | Cooperative multitasking |
| [language/stdlib.md](language/stdlib.md) | Configurable built-in modules |

### VM -- how the machine executes

| Spec | What it covers |
|------|----------------|
| [vm/architecture.md](vm/architecture.md) | Register VM, arena, instruction set, serialization |
| [vm/chunk-format.md](vm/chunk-format.md) | Bytecode format, imports/exports, validation, content identity |
| [vm/interop.md](vm/interop.md) | Host API, extern structs/funcs, scoped bindings, error propagation |

## Protocol Role

Raido is independent -- usable by any host, no dependency on Allgard or Leden. But its properties (determinism, serializable state, content-addressed bytecode) make it a natural fit as a verification engine for federated systems.

[Allgard](../allgard/) defines verifiable transforms as an optional extension. The flow:

1. Domain A publishes a Raido script as a content-addressed chunk
2. A cross-domain Transform references the script hash + inputs + outputs
3. Domain B fetches the script, re-executes with the same inputs
4. Determinism guarantees identical output -- the Proof is mechanically verified

This turns Allgard's trust-based Proofs (Conservation Law 4) into independently verifiable ones for any transform backed by a Raido script. Simple transforms (transfer, burn) don't need this -- signature + causal link is sufficient.

**What Raido provides:** deterministic execution, content-addressed identity (chunk format), versioned serialization.

**What Raido doesn't know about:** Allgard primitives, Leden sessions, federation semantics. It's a VM. The protocol integration is Allgard's concern.

Capability negotiation happens at the Leden layer -- two domains agree "we both support Raido v*N* verification." Allgard defines what that means semantically. Raido just executes bytecode and returns the same answer every time.

## Resolved

| Question | Decision | Rationale |
|----------|----------|-----------|
| Fixed-point format | **32.32** | ~9.6 decimal digits of precision vs 48.16's ~4.8. Simulation shows 48.16 drifts badly in physics chains (2.6 error after 1000 damping frames vs 6e-5). The 2.1B integer ceiling is mitigated by `int` (i64) being a separate type -- use `int` for large values. |
| Arena strategy | **Fixed arena + explicit reset.** `frame_end()` opt-in. | Default: fixed-size arena, `reset()` between evaluations. Game-loop hosts opt into `frame_end()` for per-frame cleanup. No auto-grow -- hides allocation cost. |
| Serialization versioning | **Yes.** Version header from day one. | Without it, any format change breaks all serialized snapshots. |
| Packaging | **Independent project** (`raido` crate). Lives in this repo for now. No dependency on Rask. | Not part of Rask. Can be embedded by any host via C FFI. |
| Host data model | **`extern struct` / `extern func`.** Script declares expected shapes, host binds at load time. | Compile-time type checking. Load-time mismatch detection. Replaces dynamic `host_ref` + vtable pattern. |
| Type system | **Static.** Function signatures typed, locals inferred. | Runtime type errors are a divergence vector. Static types catch bugs before deployment -- critical when scripts are tradeable economic assets. |
| Closures | **Cut.** Function references only. | Shared mutable upvalues are a verification hazard. NPC state lives in host entities, not captured variables. Function references cover the composition need. |
| Nil | **Cut.** `T?` optionals with `Some`/`None`. | Eliminates null-related runtime errors by construction. Same as Rask. |
| Globals | **Cut.** No mutable global state. | Undermines statelessness between host calls. Script state lives in coroutine locals or host entities. |

**String encoding depth.** ASCII-only for case operations (`upper`/`lower`), byte-indexed for slicing (`sub`/`byte`). No Unicode-aware operations in the VM. If a script needs Unicode (rare for game scripting, modding, rules), the host provides it as a host function. Keeps the VM tiny and deterministic -- Unicode case mapping tables are large and version-dependent.

## Deferred

- **Map growth strategy.** Load factor threshold and growth factor need benchmarking. Open addressing with linear probing is decided; the tuning constants are implementation detail.
- **Serialization migration.** Version header exists. Forward/backward compatibility policy depends on how the format evolves in practice. Premature to specify migration rules before the first format change.
- **Generics beyond built-ins.** `array<T>`, `map<K, V>`, `T?`, function types cover the need. No user-defined generics.
- **Traits / interfaces.** Functions, not methods.
