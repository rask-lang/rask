<!-- id: raido.overview -->
<!-- status: proposed -->
<!-- summary: Raido -- deterministic scripting VM, verification-first, scripting-capable -->

# Raido

Independent project. Deterministic scripting VM with static types and Rask-flavored syntax. Lives in this repo for now but is not part of Rask -- no dependency on the Rask compiler, runtime, or stdlib.

Serializable state. Deterministic floats. Sandboxed -- the host controls all capabilities.

Raido is also the verification engine for [Allgard's verifiable transforms](../allgard/README.md#verifiable-transforms). Two domains that both support Raido can mechanically verify each other's scripted transforms instead of relying on trust alone. See [Protocol Role](#protocol-role).

**Verification-first, scripting-capable.** Static types and pure-function defaults serve verification. Pausing hooks and closures serve game scripting. Features that help neither get cut.

Two use case families, one VM:

- **Verification** (Allgard minting, Apeiron physics/combat/crafting): pure functions, determinism is load-bearing, content-addressed proofs. A trading partner re-executes the same bytecode with the same inputs and checks the output matches.
- **Content scripting** (NPC AI, dialogue, game logic, GDL client scripts): multi-tick behavior, host entity interaction, sequential logic that pauses between ticks.

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
| Deterministic | Yes (strict floats) | No | Yes | No |
| Serializable state | Yes (built-in) | No | Manual | No |
| Sandboxed | Yes (no I/O) | Partial | Yes | Partial |
| Safe host interop | Yes (no unsafe) | No | No | No |
| Static types | Yes | No | Yes | No |

## Host API

Example using Rask as the host language (any language with C FFI can host Raido):

```rask
import raido

let vm = raido.Vm.new(raido.Config(
    memory_limit: 256.kilobytes()
    initial_fuel: 100_000
    max_call_depth: 256
))
ensure vm.close()

// Bind what the published API module (game_api.rd) declares
vm.register_extern_struct("Enemy", raido.ExternStruct(
    fields: [
        raido.Field.int("health", get_health, set_health)
        raido.Field.float("x", get_x, set_x)
        raido.Field.float("y", get_y, set_y)
        raido.Field.string("name", get_name, null)  // readonly
    ]
))
vm.register_extern_func("move_to", [raido.Type.extern("Enemy"), raido.Type.struct("Vec2")], raido.Type.void, move_to_handler)

let chunk = try vm.compile("script.rd", source, api)
try vm.load(chunk)  // checks bindings, then runs the script's top-level `let`s

match try vm.call("on_tick", [raido.Value.float(dt)]) {
    raido.Outcome.Done(result) => use(result)
    raido.Outcome.Paused(task, call) => waiting.push(task)  // vm.resume(task, result) later
}

// Serialize entire VM state
let snapshot = vm.serialize()

// Restore later, possibly on a different machine
let vm2 = raido.Vm.deserialize(snapshot)
```

## Script Example

`Enemy`, `move_to`, `announce` and `wait` come from the host's published API module; the script declares nothing to use them.

```raido
const WAVE_SIZE = 10
let kills = 0                                // module state: lives between host calls

func chase(attacker: Enemy, target: Enemy) {
    loop {
        move_to(attacker, Vec2 { x: target.x, y: target.y })
        wait(0.5)                            // pauses; the host resumes this call later
    }
}

func on_enemy_died(e: Enemy) {
    kills += 1
    if kills == WAVE_SIZE { announce("Wave cleared") }
    print("kills:", kills, "of", WAVE_SIZE)
}
```

If a hook fails halfway (`error(...)`, a bad index), everything it changed rolls back, `kills` included, and the host logs the error.

## Key Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Identity | Verification-first, scripting-capable | Static types serve verification. Pausing hooks serve scripting. |
| Syntax | Typed Rask subset | Same feel as Rask. Type annotations on signatures, inference for locals. |
| Types | Static, inferred locals | Catches errors at compile time. Bytecode is verified against register types before it runs, so arithmetic never dispatches on type; values carry a tag for the collector and serialization. |
| VM | Register-based | ~30% fewer dispatched instructions than stack-based. |
| Numbers | `int` (i64) and `float` (f64) | The two every audience already knows. Floats follow a strict profile that gives the same bits on every machine. |
| State | Fully serializable | Save/restore, migration, replay, checkpointing. |
| Sandbox | No I/O. Host provides all capabilities. | Safe to run untrusted code. |
| Structs/enums | User-defined, fixed layout | Structured data is the primary data model for all consumers. |
| Collections | `array<T>` + `map<K, V>` | Maps to Rask's Vec/Map. Homogeneous, typed. `m[k] = v` inserts. |
| Host data | Published API module of `extern struct` / `extern func` | Compile-time type checking. Type mismatch = load error, not runtime error. |
| Script state | Top-level `let` | Lives between host calls, saved and rolled back like the rest of the heap. |
| Functions | Named functions and closures | A closure shares the variables it captures with the function that made it, as in Lua and JS. |
| Optionals | `T?` with `Some`/`None` | Compiler-enforced null safety. No nil. |
| Strings | `"value: {x}"` interpolation | Kills concatenation chains. |
| Random | Seedable PRNG in VM state | Deterministic. Serializable. |
| Errors | `error()` aborts the call | Every host call into a script is a transaction: a failure throws away what the call changed and the host logs it. No error types, `try` or `catch` to learn; absence is `T?`. |
| Pausing | Host functions marked `pausing`, and `spawn` | `wait(2.0)` is an ordinary call that pauses the hook; the host resumes it with the result. `spawn(f, args...)` starts another task. No `yield`, no coroutine objects. Serializable. |
| Stdlib | Always present | Core functions, `math`, `bit` and the built-in methods. Nothing to opt into; the host adds capabilities through its API module. |

## Use Cases

| Use case | Key properties used |
|----------|-------------------|
| Game entity scripts | Extern structs for entities, pausing hooks for AI, determinism for netcode |
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
| [language/types.md](language/types.md) | Static type system, ints and floats, structs, enums, optionals, functions and closures |
| [language/syntax.md](language/syntax.md) | Grammar, variables, functions, control flow, operators, declarations |
| [language/pausing.md](language/pausing.md) | Hooks that wait, and how the host resumes them |
| [language/stdlib.md](language/stdlib.md) | Configurable built-in modules |

### VM -- how the machine executes

| Spec | What it covers |
|------|----------------|
| [vm/architecture.md](vm/architecture.md) | Register VM, heap, instruction set, serialization |
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

### Verification Modes

**Primary: bilateral re-execution.** Both parties run the same script with the same inputs, compare outputs. Simple, fast — typical Raido scripts (combat, crafting, minting) are thousands of instructions, sub-millisecond to re-execute. This is the default verification mode.

**What a verification run is.** Re-execution only proves something if both sides run the same thing the same way, so a run that someone else will check follows four rules:

- **A fresh VM.** The checker loads the chunk into a new VM and makes one `vm.call`. Module variables hold only what their initializers made, so the result can't depend on history the checker never saw.
- **The config is an input.** Fuel, memory limit and call depth change results (enough fuel or not, `MemoryExhausted` or not), so the claim carries them alongside the arguments.
- **Only host functions the checker has.** The chunk's hash already names the API module it was compiled against. A script meant for checking is compiled against an API both sides implement identically, usually one the protocol above publishes, or none at all. Every function in it is immediate and pure.
- **Nothing that only means something live.** No pausing, no `spawn`, no deferred calls. A run that tries one fails, so the claim fails.

**Future: ZK proofs.** For two cases where re-execution doesn't work:

- **Privacy.** Re-execution requires sharing inputs. A domain proving it minted an asset correctly might not want to reveal its crafting formula, resource stockpile, or internal pricing. A ZK proof proves "this output came from this script" without revealing the inputs.
- **Expensive scripts.** Galaxy generation, large physics simulations — scripts where re-execution is costly and many parties need to verify. The executor proves once, verifiers check cheaply.

Chunks compiled under the provable profile use only `int` arithmetic, so with bounded execution, bounded memory and static bytecode they have the properties ZK circuits need. Floats are too costly to prove, so a provable chunk can't use them. See [vm/architecture.md](vm/architecture.md#provable-profile). The ZK prover would be a separate backend sharing the bytecode format — ZK goes underneath the VM, not inside it.

**What Raido provides:** deterministic execution, content-addressed identity (chunk format), versioned serialization, ZK-compatible architecture.

**What Raido doesn't know about:** Allgard primitives, Leden sessions, federation semantics. It's a VM. The protocol integration is Allgard's concern.

Capability negotiation happens at the Leden layer -- two domains agree "we both support Raido v*N* verification." Allgard defines what that means semantically. Raido just executes bytecode and returns the same answer every time.

## Resolved

| Question | Decision | Rationale |
|----------|----------|-----------|
| Fractional numbers | **f64, strict profile** | Replaced 32.32 fixed point, which was strange to work with: a 2.1 billion ceiling, saturation instead of overflow, a CORDIC `sin`. IEEE basic operations are correctly rounded, so with no fused multiply-add, correctly rounded math functions and NaN or infinity made a runtime error, an f64 gives the same bits everywhere (rask-lang/rask#1683). Zero-knowledge proofs move to an `int`-only provable profile (vm/architecture.md). |
| Memory | **Garbage-collected heap the script can't observe.** | Paused hooks, closures and module state keep data across ticks, which a reset-between-evaluations arena can't hold safely. Determinism comes from what the language exposes, not from when the collector runs: no finalizers, no weak references, nothing derived from addresses. The memory limit counts reachable data in a size every implementation computes the same way, checked at points fixed by the allocation count, and serialization walks the object graph in a fixed order, so both are identical on every machine. |
| Serialization versioning | **Yes.** Version header from day one. | Without it, any format change breaks all serialized snapshots. |
| Packaging | **Independent project** (`raido` crate). Lives in this repo for now. No dependency on Rask. | Not part of Rask. Can be embedded by any host via C FFI. |
| Host data model | **A published API module of `extern struct` / `extern func`.** The host declares its shapes once; scripts see them already imported. | Compile-time type checking. Load-time mismatch detection. The module's hash is part of every script's identity. |
| Type system | **Static.** Function signatures typed, locals inferred. | Runtime type errors are a divergence vector. Static types catch bugs before deployment -- critical when scripts are tradeable economic assets. |
| Closures | **Capture by sharing the variable, as in Lua and JS.** | I first forbade reassigning a captured variable, to keep closures free of hidden shared state. But Raido already shares every struct, array and map by reference, so the rule only forced counters into a struct and broke Lua's most common closure idiom. A reassigned captured variable lives in a heap cell; one that's never reassigned is copied, at no cost. |
| Nil | **Cut.** `T?` optionals with `Some`/`None`. | Eliminates null-related runtime errors by construction. Same as Rask. |
| Module state | **Top-level `let`.** | Every mod needs a counter or a flag that outlives one host call. Module variables are heap roots, so they're saved, restored and rolled back with everything else; `const` names a value that never changes. |
| Pausing | **Host functions marked `pausing`; no `yield` or coroutines in the language.** | A modder writes `wait(2.0)` or `let choice = ask(player, "...")` and it reads like any call. The pausing function's signature types what goes out and what comes back, and nothing that calls it needs marking. Script-made coroutines and generators would add a second scheduler beside the host's for little a mod needs; `spawn` covers starting something that runs alongside. |
| Parameters | **Shared by reference, no modes.** | Structs, arrays and maps behave as in JS, Python and Lua: a function can change what it's given. A read-only default needs a rule for every way a reference can be copied, and I'd rather have nothing to learn. |

**String encoding depth.** ASCII-only for case operations (`upper`/`lower`), byte-indexed for slicing (`substring`/`byte`). No Unicode-aware operations in the VM. If a script needs Unicode (rare for game scripting, modding, rules), the host provides it as a host function. Keeps the VM tiny and deterministic -- Unicode case mapping tables are large and version-dependent.

## Deferred

- **Map growth strategy.** Open addressing with linear probing is decided; the load factor is implementation detail. The memory limit counts a logical capacity derived from the entry count, so it doesn't depend on the tuning.
- **Serialization migration.** Version header exists. Forward/backward compatibility policy depends on how the format evolves in practice. Premature to specify migration rules before the first format change.
- **Generics beyond built-ins.** `array<T>`, `map<K, V>`, `T?`, function types cover the need. No user-defined generics.
- **Interfaces.** Functions, not methods.
