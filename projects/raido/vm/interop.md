<!-- id: raido.interop -->
<!-- status: proposed -->
<!-- summary: Rask integration API -- VM creation, extern structs/funcs, scoped bindings, serialization -->
<!-- depends: raido/vm/architecture.md, raido/language/types.md -->

# Rask Integration

Host API for integrating Raido. All interaction is safe -- no `unsafe` required.

## VM Lifecycle

```rask
let vm = raido.Vm.new(raido.Config {
    memory_limit: 256.kilobytes(),
    initial_fuel: 100_000,
    max_call_depth: 256,
    stdlib: [raido.Stdlib.math],
})
ensure vm.close()

// Seed the PRNG (deterministic -- same seed = same random sequence)
vm.seed(12345)

// Register extern bindings before loading
vm.register_extern_struct("Enemy", raido.ExternStruct {
    fields: [
        raido.Field.int("health", get_health, set_health)
        raido.Field.float("x", get_x, set_x)
        raido.Field.float("y", get_y, set_y)
        raido.Field.string("name", get_name, null)  // readonly
    ],
})
vm.register_extern_func("move_to", move_to_handler)
vm.register_extern_func("noise", noise_handler)

// Register module import resolver
vm.set_import_resolver(|name: string| -> Vec<u8> or raido.Error {
    return try load_chunk_bytes(name)
})

// Override print handler (default: no-op)
vm.set_print(|msg: string| { log.info("raido: {msg}") })

// Compile -- validates, derives imports/exports, computes content hash
let chunk = try vm.compile("script.rd", source)

// Or compile with debug info
let chunk = try vm.compile("script.rd", source, raido.CompileOpts {
    debug_info: true,
})

// Or load pre-compiled bytecode (validates on load)
let chunk = try vm.load(bytecode_bytes)

// Inspect before running
chunk.hash()             // content identity (SHA-256)
chunk.imports()          // extern declarations the script needs
chunk.module_imports()   // content-addressed module dependencies
chunk.exports()          // functions the host can call (with typed signatures)

// Load -- fails fast if externs don't match declarations
try vm.exec(chunk)
let result = try vm.call("process", [raido.Value.int(42)])
```

See [chunk-format.md](chunk-format.md) for format details.

## The Published API

The host publishes its API as one Raido module of `extern` declarations: the structs it exposes, their fields, and the functions scripts may call. Every script compiled for that host sees the module already imported, under the host's chosen name (a game might call it `game`, or import its names directly). Scripts never write `extern` themselves.

```raido
// game_api.rd -- written by the engine authors, or generated from the
// host's registrations, the way `rask bindc` generates bindings from a C header
extern struct Enemy {
    health: int
    x: float
    y: float
    readonly name: string
}

extern func move_to(entity: Enemy, target: Vec2)
extern func noise(quality: float, id: int, index: int) -> float
```

- **One copy of the API.** Adding a field or function to the engine changes the module; scripts don't each carry a stale copy.
- **Checked at load.** The host's registered bindings must match the module's declarations, or `vm.load()` fails, so a host can't publish an API it doesn't implement.
- **Part of identity.** The module's content hash is part of every chunk's import graph, so two scripts compiled against different API versions have different identities, and a chunk built for an old API refuses to load against a new one instead of misbehaving.

## Extern Structs

The published API declares `extern struct` for host-managed data shapes. The compiler type-checks all field access against these declarations. The host binds at `vm.load()`.

**API module:**
```raido
extern struct Enemy {
    health: int
    x: float
    y: float
    readonly name: string
}

func chase(attacker: Enemy, target: Enemy) {
    let dest = Vec2 { x: target.x, y: target.y }
    move_to(attacker, dest)
}
```

**Host side:**
```rask
vm.register_extern_struct("Enemy", raido.ExternStruct {
    fields: [
        raido.Field.int("health", get_health, set_health)
        raido.Field.float("x", get_x, set_x)
        raido.Field.float("y", get_y, set_y)
        raido.Field.string("name", get_name, null)  // null setter = readonly
    ],
})
```

**Type mismatch = load error.** If the API module declares `health: int` but the host binds `health` as `float`, `vm.load()` fails. No runtime surprises.

**Readonly enforcement.** Fields declared `readonly` in the API module have no setter in the host binding (`null`). The compiler rejects writes to readonly fields at compile time.

**Runtime dispatch:** `GET_FIELD` / `SET_FIELD` index into the vtable by slot number. No string hashing, no map lookup. One indexed function pointer call per field access.

## Extern Funcs

The API module declares `extern func` for host-provided functions with typed signatures.

**API module:**
```raido
extern func move_to(entity: Enemy, target: Vec2)
extern func noise(quality: float, id: int, index: int) -> float
extern func spawn(kind: string, pos: Vec2) -> Enemy
```

**Host side:**
```rask
vm.register_extern_func("move_to", |ctx| {
    let entity_ref = try ctx.arg_extern(0)  // Enemy reference
    let target = try ctx.arg_struct(1)       // Vec2 struct
    // host logic...
})

vm.register_extern_func("noise", |ctx| {
    let quality = try ctx.arg_number(0)
    let id = try ctx.arg_int(1)
    let index = try ctx.arg_int(2)
    return raido.Value.float(compute_noise(quality, id, index))
})
```

Same load-time checking: if a declared `extern func` isn't registered, `vm.load()` fails.

## Binding Helpers

The `raido.bind` helper library reduces boilerplate for mapping host data to extern structs:

```rask
import raido.bind

// Bind a pool -- each handle becomes an extern struct reference
raido.bind.pool(vm, "enemies", enemies, [
    bind.Field.int("health")
    bind.Field.float("x")
    bind.Field.float("y")
])

// Bind a struct directly
raido.bind.struct(vm, "config", config)
```

`raido.bind` is a convenience library, not VM core. It generates `register_extern_struct` calls.

## Memory

The heap is garbage collected, invisibly to scripts ([architecture.md](architecture.md#heap)). The host has one call to make:

```rask
// Full reset between independent evaluations -- clears the heap and coroutines
vm.reset()
```

## Host References

A heap value the host gets back from the VM (a struct, array, string, closure or coroutine) stays alive while the host's call is in progress. To keep one after that, the host roots it, like Lua's registry:

```rask
let result = try vm.call("make_plan", [])
let plan = vm.root(result)        // stays alive and valid until unrooted
// ... later ticks ...
let value = vm.get(plan)
vm.unroot(plan)
```

Roots are what the collector and serialization start from, and they're part of the serialized state, in creation order. A heap value the host kept without rooting may be collected; in debug builds the VM detects its use and raises a host error.

## Module Import Resolution

Scripts use `import "name" as alias`. The host resolves import names to compiled chunks via a resolver callback:

```rask
vm.set_import_resolver(|name: string| -> Vec<u8> or raido.Error {
    // Resolve however you want: file lookup, content store, network fetch
    return try content_store.get(name)
})
```

The resolver receives the import name string and returns the raw chunk bytes. The VM validates and loads the imported chunk. Import resolution happens during `vm.exec()` -- all imports must resolve or exec fails.

The import name is opaque to the VM. The host decides what it means: a filename, a content hash, a registry key. The import graph (names + resolved hashes) is part of the chunk's content identity.

## Scoped Bindings

Extern struct field accessors need access to host data (pools, DBs, etc). Scoped bindings provide this safely:

```rask
try vm.with_context(|ctx| {
    ctx.bind("enemies", enemies)  // borrow for this scope
    ctx.call("on_update", [raido.Value.float(dt)])
})
// borrow released
```

Same scoped borrowing pattern -- the host binds whatever data extern struct accessors and extern funcs need. The VM doesn't care what it is.

## Serialization

```rask
let bytes = vm.serialize()
let vm2 = raido.Vm.deserialize(bytes)
// Re-register extern structs and funcs
// Re-bind contexts before calling
```

Format is versioned -- version header from day one. Deserialize rejects unknown versions with a clear error.

Serializes: the heap reachable from root handles, in canonical order ([architecture.md](architecture.md#serialization)), PRNG state (4 x u32), fuel remaining.
Does not serialize: host function closures (by name), host bindings (re-bound), bytecode (re-loaded).


## Environment Configuration

The host controls what's available:

```rask
let vm = raido.Vm.new(raido.Config {
    memory_limit: 256.kilobytes(),
    initial_fuel: 100_000,
    max_call_depth: 256,
    stdlib: [raido.Stdlib.math, raido.Stdlib.string],  // only these modules
})
```

No stdlib modules loaded by default. Host opts in to what scripts can access.

## Error Propagation

- **Script -> Host:** `raido.ScriptError` (kind, message, stack trace).
- **Host -> Script:** Rask errors in extern funcs become Raido runtime errors (`HostError`).
- **In-script:** `error(msg)` raises. Scripts don't catch; absence is `T?`.

## Calls Are Transactions

Every call the host makes into a script (`vm.call`, resuming a waiting handler) either completes or changes nothing:

- **Objects allocated** during the call become unreachable when it fails; the collector reclaims them.
- **Writes to objects that existed before the call** (a coroutine's struct, a map held by a root) go into an undo log: each store records the slot's old value. On failure the log is replayed backwards. Objects allocated during the call need no logging; each object records the call it was made in, so the check is one comparison.
- **Extern field writes** go into a write buffer. Reads within the call see the buffered value; the buffer is applied when the call returns and dropped if it fails.
- **Extern functions with effects** (`send_message`, `spawn_unit`) are registered as deferred: their calls are queued and run after the call succeeds, in call order. Pure extern functions (`noise`, `distance`) run immediately.

So a mod that fails halfway through an event never leaves the world half-updated, and the host only has to log the error and carry on.
