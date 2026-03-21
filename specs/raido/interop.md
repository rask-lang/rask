<!-- id: raido.interop -->
<!-- status: proposed -->
<!-- summary: Rask integration API — VM creation, host functions, host references, serialization -->
<!-- depends: raido/vm.md, raido/values.md -->

# Rask Integration

Host API for embedding Raido. All interaction is safe — no `unsafe` required.

## VM Lifecycle

```rask
const vm = raido.Vm.new(raido.Config {
    arena_size: 256.kilobytes(),
    instruction_limit: 100_000,
})
ensure vm.close()

const chunk = try vm.compile("script.raido", source)
try vm.exec(chunk)
const result = try vm.call("process", [raido.Value.int(42)])
```

## Host Functions

```rask
vm.register("send_message", |ctx| {
    const target = try ctx.arg_string(0)
    const body = try ctx.arg_string(1)
    messenger.send(target, body)
})
```

Registered by name. On serialize, only the name is stored — host re-registers after restore. Rask errors in host functions become Raido runtime errors.

## Host References

Opaque references to host-managed data. The VM doesn't know what's behind them. The host registers a vtable per ref type — field names map to slot indices at compile time.

```rask
// Define a reference type with a vtable
vm.register_ref_type("enemy", raido.RefType {
    fields: [
        raido.HostField.int("health", get_health, set_health),  // slot 0
        raido.HostField.number("x", get_x, set_x),              // slot 1
        raido.HostField.number("y", get_y, set_y),              // slot 2
        raido.HostField.string("name", get_name, null),         // slot 3, read-only
    ],
})

// Pass a reference to the script
vm.set_global("target", vm.create_ref("enemy", enemy_id))
```

```raido
// Script sees an object with fields
// Compiler resolves "health" → slot 0, emits GET_REF_FIELD r1 r0 0
target.health -= 10
print("Hit {target.name} at ({target.x}, {target.y})")
```

**Runtime dispatch:** `GET_REF_FIELD` / `SET_REF_FIELD` index into the vtable by slot number. No string hashing, no map lookup. One indexed function pointer call per field access.

**Binding helpers** (`raido.bind`) reduce the boilerplate of mapping host data to refs:

```rask
import raido.bind

// Bind a pool — each handle becomes a host ref
raido.bind.pool(vm, "enemies", enemies, [
    bind.Field.int("health"),
    bind.Field.number("x"),
    bind.Field.number("y"),
])

// Bind a struct directly
raido.bind.struct(vm, "config", config)
```

`raido.bind` is a convenience library, not VM core. It generates `register_ref_type` calls with vtable entries.

## Scoped Bindings

Host references need access to host data (pools, DBs, etc). Scoped bindings provide this safely:

```rask
try vm.with_context(|ctx| {
    ctx.bind("enemies", enemies)  // borrow for this scope
    ctx.call("on_update", [raido.Value.number(dt)])
})
// borrow released
```

Same scoped borrowing pattern as before, but generalized. The host binds whatever data host functions and ref accessors need — pools, database connections, message queues. The VM doesn't care what it is.

## Serialization

```rask
const bytes = vm.serialize()
const vm2 = raido.Vm.deserialize(bytes)
// Re-register host functions and ref types
// Re-bind contexts before calling
```

Format is versioned — version header from day one. Deserialize rejects unknown versions with a clear error.

Serializes: value stack, globals, coroutines, arena, PRNG, instruction counter.
Does not serialize: host function closures (by name), host bindings (re-bound), bytecode (re-loaded).

## Environment Configuration

The host controls what's available:

```rask
const vm = raido.Vm.new(raido.Config {
    arena_size: 256.kilobytes(),
    instruction_limit: 100_000,
    stdlib: [raido.Stdlib.math, raido.Stdlib.string],  // only these modules
})
```

No stdlib modules loaded by default. Host opts in to what scripts can access.

## Error Propagation

- **Script → Host:** `raido.ScriptError` (message, file, line, stack trace).
- **Host → Script:** Rask errors in host functions become Raido runtime errors.
- **In-script:** `try expr` propagates errors. `try expr else |e| { ... }` catches them. `error(msg)` raises them. Same syntax as Rask.
