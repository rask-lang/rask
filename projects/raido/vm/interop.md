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
vm.register_extern_func("move_to", [raido.Type.extern("Enemy"), raido.Type.struct("Vec2")], raido.Type.void, move_to_handler)
vm.register_extern_func("noise", [raido.Type.float, raido.Type.int, raido.Type.int], raido.Type.float, noise_handler)

// Register module import resolver
vm.set_import_resolver(|name: string| -> Vec<u8> or raido.Error {
    return try load_chunk_bytes(name)
})

// Override print handler (default: no-op)
vm.set_print(|msg: string| { log.info("raido: {msg}") })

// Compile against the published API module -- validates, computes content hash
let chunk = try vm.compile("script.rd", source, api)

// Or with debug info, or as provable
let chunk = try vm.compile("script.rd", source, api, raido.CompileOpts {
    debug_info: true,
})

// Or decode pre-compiled bytecode (validates)
let chunk = try raido.Chunk.decode(bytecode_bytes)

// Inspect before running
chunk.hash()             // content identity (SHA-256)
chunk.imports()          // extern declarations the script uses
chunk.module_imports()   // content-addressed module dependencies
chunk.exports()          // functions the host can call (with typed signatures)

// Load -- checks bindings, resolves imports, runs top-level `let`s as one run
try vm.load(chunk)

// Call a hook. It either finishes or pauses in a pausing host function.
match try vm.call("on_tick", [raido.Value.float(dt)]) {
    raido.Outcome.Done(result) => use(result)
    raido.Outcome.Paused(task, call) => schedule(task, call)   // e.g. wait(0.5)
}
// Later: resume it with that function's result
let next = try vm.resume(task, raido.Value.none())   // `wait` returns nothing
```

A task handle is a root handle ([Host References](#host-references)): the task stays alive until the host drops it with `vm.unroot(task)`. Resuming a finished task is an error.

Tasks a script starts with `spawn` come out of `vm.spawned()` after the run that made them commits, in spawn order, not yet started. The host starts one with `vm.resume(task, raido.Value.none())`:

```rask
for task in vm.spawned() {
    schedule(task)          // later: vm.resume(task, raido.Value.none())
}
```

See [chunk-format.md](chunk-format.md) for format details.

## The Published API

The host publishes its API as one Raido module: `extern` declarations for the structs it exposes and the functions scripts may call, and any ordinary Raido types and functions it wants to give every script (`Vec2`). Every script compiled for that host sees the module already imported, under the host's chosen name (a game might call it `game`, or import its names directly). Scripts never write `extern` themselves.

```raido
// game_api.rd -- written by the engine authors, or generated from the
// host's registrations, the way `rask bindc` generates bindings from a C header
extern struct Enemy {
    health: int
    x: float
    y: float
    readonly name: string
}

struct Vec2 { x: float, y: float }

extern deferred func move_to(entity: Enemy, target: Vec2)
extern func noise(quality: float, id: int, index: int) -> float
extern deferred func send_message(target: string, body: string)
extern pausing func wait(seconds: float)
```

- **One copy of the API.** Adding a field or function to the engine changes the module; scripts don't each carry a stale copy.
- **Checked at load.** The host's registered bindings must match the module's declarations, or `vm.load()` fails, so a host can't publish an API it doesn't implement.
- **Part of identity.** The module's content hash is part of every chunk's import graph, so two scripts compiled against different API versions have different identities, and a chunk built for an old API refuses to load against a new one instead of misbehaving.

## Extern Structs

The published API declares `extern struct` for host-managed data shapes. The compiler type-checks all field access against these declarations. The host binds at `vm.load()`.

Fields hold only `int`, `float`, `bool`, `string`, payload-free enums, other extern structs, or optionals of those ([types.md](../language/types.md#extern-types)). A field holding a struct or a collection would hand the script a value it could change without going through a setter, where rollback can't see it.

An extern struct value is the host's 64-bit id for the object, tagged with the struct's type. The id is what gets saved, so it has to stay meaningful after a save and load: an entity id or a pool handle, not a pointer.

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

**Getters and setters are plain field access.** A getter returns the field and changes nothing; a setter stores the value and does nothing else. The VM relies on that to roll a write back: it reads the old value with the getter, stores the new one with the setter, and on failure stores the old ones back, newest first. A setter that wants to do more (play a sound when health drops) belongs in a deferred extern instead.

**Runtime dispatch:** `GET_FIELD` / `SET_FIELD` index into the vtable by slot number. No string hashing, no map lookup. One indexed function pointer call per field access.

## Extern Funcs

The API module declares `extern func` for host-provided functions with typed signatures.

An extern function runs in one of two ways, and the API module says which, because a script can tell them apart:

- **Immediate** (`extern func`): runs at the call and can return a value. It must either change nothing the script could later observe (`noise`, `distance`, reads), or be registered with an undo callback the VM runs if the call fails (`spawn_unit` with a despawn). It sees the script's field writes so far, since those went straight through the setters.
- **Deferred** (`extern deferred func`): can't return a value. Its calls are queued and run after the script's call succeeds, in call order. A deferred effect isn't visible to the call that made it, and lands after all of that call's field writes: `kill(e); e.health = 50` reaches the host as the health write, then the kill. Most world-changing actions are deferred: `move_to`, `attack`, `play_sound`.

```raido
extern func noise(quality: float, id: int, index: int) -> float
extern func spawn_unit(kind: string, pos: Vec2) -> Enemy   // registered with an undo
extern deferred func send_message(target: string, body: string)
```

A third kind, **pausing** (`extern pausing func`), takes time: calling it pauses the script, and the host's `vm.call` or `vm.resume` returns `Paused(task, call)` with the function's name and arguments. The host does whatever the function means and later calls `vm.resume(task, result)` ([language/pausing.md](../language/pausing.md)).

An extern function, getter, setter, undo callback or deferred handler can't call back into the VM: `vm.call` or `vm.resume` from inside one is a `HostError`.

**Values from the host are checked.** Every value that enters the VM is checked against the type the API declares for it before a script sees it: `vm.call` arguments against the export's signature, an extern's result against its return type, a getter's value against the field type, a resume value against the pausing function's return type. A mismatch is a `HostError`. So the script's types hold at the boundary too, whatever the host does. `raido.Value.none()` is what a host passes for a function that returns nothing.

**Host side:**
```rask
let T = raido.Type
vm.register_extern_func("move_to", [T.extern("Enemy"), T.struct("Vec2")], T.void, |ctx| {
    let entity_ref = try ctx.arg_extern(0)  // Enemy reference
    let target = try ctx.arg_struct(1)       // Vec2 struct
    // host logic...
})

vm.register_extern_func("noise", [T.float, T.int, T.int], T.float, |ctx| {
    let quality = try ctx.arg_float(0)
    let id = try ctx.arg_int(1)
    let index = try ctx.arg_int(2)
    return raido.Value.float(compute_noise(quality, id, index))
})

vm.register_extern_func("spawn_unit", [T.string, T.struct("Vec2")], T.extern("Enemy"),
    spawn_handler, undo: |unit| despawn(unit))
```

A registration states its parameter and return types, and `vm.load()` checks them against the module: a missing binding, or one whose types differ, fails the load.

A deferred handler that fails after the run committed can't roll the run back, since the host may already have acted on it. The VM reports it to the host as a `HostError` and runs the remaining deferred calls.

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
// Full reset between independent evaluations -- clears the heap, tasks and
// roots, and runs the module initializers again
vm.reset()
```

Loading is one run too. `vm.load(chunk)` runs every initializer of the chunk and its imports as one transaction: deferred calls and spawns happen only if all of them succeed, and a failed load leaves the VM as it was.

## Host References

A heap value the host gets back from a call (a struct, array, string or closure) stays alive until the host's next call into the VM starts, and so does one the host makes for an argument with `vm.string(...)`. Passed into that call, it's in a register from then on. To keep one longer, the host roots it, like Lua's registry:

```rask
let result = try vm.call("make_plan", [])
let plan = vm.root(result)        // stays alive and valid until unrooted
// ... later ticks ...
let value = vm.get(plan)
vm.unroot(plan)
```

Roots are what the collector and serialization start from, and they're part of the serialized state, in creation order. A root's index survives a save and load, which is how a host finds its tasks again.

Whether the host may still use a value is decided by reachability, never by whether the collector got to it: a value passed into a call must be reachable from a root, the last call's result, or something the host made since. Anything else is a `HostError`, collected or not, so the host can't observe the collector. Every heap value the host holds is a slot index plus a generation number, never a pointer, so a slot reused by a newer object is never mistaken for the old one.

## Module Import Resolution

Scripts use `import "name" as alias`. The host resolves import names to compiled chunks via a resolver callback:

```rask
vm.set_import_resolver(|name: string| -> Vec<u8> or raido.Error {
    // Resolve however you want: file lookup, content store, network fetch
    return try content_store.get(name)
})
```

The resolver receives the import name string and returns the raw chunk bytes. The VM verifies and loads the imported chunk. Import resolution happens during `vm.load()` -- all imports must resolve or the load fails.

Each chunk is loaded once per VM, keyed by content hash: two names that resolve to the same chunk share one instance and one set of module variables. Load order is depth first, imports in the order they're written, each chunk after everything it imports (post-order), so a diamond `A -> B, C -> D` loads `D, B, C, A`. That order is also initializer order and serialization order.

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
let vm2 = raido.Vm.deserialize(bytes, chunks)   // chunks looked up by the hashes in the snapshot
// Re-register extern structs and funcs
// Re-bind contexts before calling
```

Format is versioned -- version header from day one. Deserialize rejects unknown versions with a clear error.

Serializes the config, module variables, root handles (tasks included), the spawn queue and the heap reachable from them, in canonical order, with PRNG state, fuel and the allocation count ([architecture.md](architecture.md#serialization)).
Does not serialize: host bindings (re-registered), bytecode (supplied again).


## Environment Configuration

The host controls what's available:

```rask
let vm = raido.Vm.new(raido.Config {
    memory_limit: 256.kilobytes(),
    initial_fuel: 100_000,
    max_call_depth: 256,
})
```

The standard library is always present and has no I/O. Everything else a script can reach is in the host's API module.

## Error Propagation

- **Script -> Host:** `raido.ScriptError` (kind, message, stack trace).
- **Host -> Script:** Rask errors in extern funcs become Raido runtime errors (`HostError`).
- **In-script:** `error(msg)` raises. Scripts don't catch; absence is `T?`.

## Calls Are Transactions

Every run of a script, from `vm.call` or `vm.resume` to the next pause or the end, is one transaction: it either commits or changes nothing. A task that pauses three times is four transactions; each pause commits what came before it, so the host can act on it while the task waits.

- **Objects allocated** during the run become unreachable when it fails; the collector reclaims them.
- **Writes to objects that existed before the run** (a struct in a module variable, a map held by a root) go into an undo log. A slot is logged once per run, with the value it had when the run started; a container's first change of shape (push, remove, a new key, sort) saves the whole of it once. Module variable writes are logged the same way. On failure the log is replayed newest first. Objects allocated during the run need no logging; each object records the run it was made in.
- **Tasks.** Resuming a task logs its status and saved frames first. Saved frames are replaced as a whole at each pause, never edited, so rolling back puts the task back where it was paused.
- **The PRNG state** is logged at its first use in the run and restored on failure.
- **Extern field writes** go straight through the setter, after the VM reads the old value with the getter and logs it. On failure the old values are stored back, newest first.
- **Immediate extern functions** with effects have their undo callbacks queued; on failure they run, newest first.
- **Deferred extern functions** and **spawned tasks** are queued, and happen only if the run commits: the deferred calls in call order, then the tasks appear in `vm.spawned()` in spawn order.

A failed run leaves the VM, and a task, exactly as before it started; the host gets the error, logs it, and usually drops the task.

Two things aren't rolled back: fuel spent, and `print`, which is a log. Both are deterministic, so replay still matches.

So a mod that fails halfway through an event never leaves the world half-updated, and the host only has to log the error and carry on.
