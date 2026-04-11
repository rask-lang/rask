<!-- id: raido.interop -->
<!-- status: proposed -->
<!-- summary: Rask integration API -- VM creation, extern structs/funcs, scoped bindings, serialization -->
<!-- depends: raido/vm/architecture.md, raido/language/types.md -->

# Rask Integration

Host API for integrating Raido. All interaction is safe -- no `unsafe` required.

## VM Lifecycle

```rask
const vm = raido.Vm.new(raido.Config {
    arena_size: 256.kilobytes(),
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
        raido.Field.int("health", get_health, set_health),
        raido.Field.number("x", get_x, set_x),
        raido.Field.number("y", get_y, set_y),
        raido.Field.string("name", get_name, null),  // readonly
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
const chunk = try vm.compile("script.rd", source)

// Or compile with debug info
const chunk = try vm.compile("script.rd", source, raido.CompileOpts {
    debug_info: true,
})

// Or load pre-compiled bytecode (validates on load)
const chunk = try vm.load(bytecode_bytes)

// Inspect before running
chunk.hash()             // content identity (SHA-256)
chunk.imports()          // extern declarations the script needs
chunk.module_imports()   // content-addressed module dependencies
chunk.exports()          // functions the host can call (with typed signatures)

// Load -- fails fast if externs don't match declarations
try vm.exec(chunk)
const result = try vm.call("process", [raido.Value.int(42)])
```

See [chunk-format.md](chunk-format.md) for format details.

## Extern Structs

Scripts declare `extern struct` to describe host-managed data shapes. The compiler type-checks all field access against these declarations. The host binds at `vm.load()`.

**Script side:**
```raido
extern struct Enemy {
    health: int
    x: number
    y: number
    readonly name: string
}

func chase(attacker: Enemy, target: Enemy) {
    const dest = Vec2 { x: target.x, y: target.y }
    move_to(attacker, dest)
}
```

**Host side:**
```rask
vm.register_extern_struct("Enemy", raido.ExternStruct {
    fields: [
        raido.Field.int("health", get_health, set_health),
        raido.Field.number("x", get_x, set_x),
        raido.Field.number("y", get_y, set_y),
        raido.Field.string("name", get_name, null),  // null setter = readonly
    ],
})
```

**Type mismatch = load error.** If the script declares `health: int` but the host binds `health` as `number`, `vm.load()` fails. No runtime surprises.

**Readonly enforcement.** Fields declared `readonly` in the script have no setter in the host binding (`null`). The compiler rejects writes to readonly fields at compile time.

**Runtime dispatch:** `GET_FIELD` / `SET_FIELD` index into the vtable by slot number. No string hashing, no map lookup. One indexed function pointer call per field access.

## Extern Funcs

Scripts declare `extern func` for host-provided functions with typed signatures.

**Script side:**
```raido
extern func move_to(entity: Enemy, target: Vec2)
extern func noise(quality: number, id: int, index: int) -> number
extern func spawn(kind: string, pos: Vec2) -> Enemy
```

**Host side:**
```rask
vm.register_extern_func("move_to", |ctx| {
    const entity_ref = try ctx.arg_extern(0)  // Enemy reference
    const target = try ctx.arg_struct(1)       // Vec2 struct
    // host logic...
})

vm.register_extern_func("noise", |ctx| {
    const quality = try ctx.arg_number(0)
    const id = try ctx.arg_int(1)
    const index = try ctx.arg_int(2)
    return raido.Value.number(compute_noise(quality, id, index))
})
```

Same load-time checking: if a declared `extern func` isn't registered, `vm.load()` fails.

## Binding Helpers

The `raido.bind` helper library reduces boilerplate for mapping host data to extern structs:

```rask
import raido.bind

// Bind a pool -- each handle becomes an extern struct reference
raido.bind.pool(vm, "enemies", enemies, [
    bind.Field.int("health"),
    bind.Field.number("x"),
    bind.Field.number("y"),
])

// Bind a struct directly
raido.bind.struct(vm, "config", config)
```

`raido.bind` is a convenience library, not VM core. It generates `register_extern_struct` calls.

## Arena Lifecycle

The host manages arena memory between evaluations:

```rask
// Full reset -- clears arena, coroutines, everything
vm.reset()

// Frame-based cleanup for game loops
vm.frame_begin()           // save arena position
try vm.call("on_update", [raido.Value.number(dt)])
vm.frame_end()             // reclaim frame-local allocations
```

See [architecture.md](architecture.md#reset-and-frame_end) for details on what's persistent vs frame-local.

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
    ctx.call("on_update", [raido.Value.number(dt)])
})
// borrow released
```

Same scoped borrowing pattern -- the host binds whatever data extern struct accessors and extern funcs need. The VM doesn't care what it is.

## Serialization

```rask
const bytes = vm.serialize()
const vm2 = raido.Vm.deserialize(bytes)
// Re-register extern structs and funcs
// Re-bind contexts before calling
```

Format is versioned -- version header from day one. Deserialize rejects unknown versions with a clear error.

Serializes: registers, call frames, coroutines, arena contents, PRNG state (4 x u32), fuel remaining, frame_base.
Does not serialize: host function closures (by name), host bindings (re-bound), bytecode (re-loaded).

With static types, serialization is simpler -- no type tags per value. The deserializer knows the type of every register from the bytecode metadata.

## Environment Configuration

The host controls what's available:

```rask
const vm = raido.Vm.new(raido.Config {
    arena_size: 256.kilobytes(),
    initial_fuel: 100_000,
    max_call_depth: 256,
    stdlib: [raido.Stdlib.math, raido.Stdlib.string],  // only these modules
})
```

No stdlib modules loaded by default. Host opts in to what scripts can access.

## Error Propagation

- **Script -> Host:** `raido.ScriptError` (kind, message, stack trace).
- **Host -> Script:** Rask errors in extern funcs become Raido runtime errors (`HostError`).
- **In-script:** `try expr` propagates errors. `try expr else |e| { ... }` catches them. `error(msg)` raises them. Same syntax as Rask.
