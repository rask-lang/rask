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

Opaque references to host-managed data. The VM doesn't know what's behind them. The host defines field access.

```rask
// Define a reference type with field accessors
vm.register_ref_type("enemy", raido.RefType {
    fields: [
        raido.HostField.int("health", |r| r.health, |r, v| r.health = v),
        raido.HostField.number("x", |r| r.x, |r, v| r.x = v),
        raido.HostField.number("y", |r| r.y, |r, v| r.y = v),
        raido.HostField.string("name", |r| r.name, |r, _| error("read-only")),
    ],
})

// Pass a reference to the script
vm.set_global("target", vm.create_ref("enemy", enemy_id))
```

```raido
// Script sees an object with fields
target.health -= 10
print("Hit {target.name} at ({target.x}, {target.y})")
```

**For game servers using pools:** build a `provide_pool` helper that creates host refs for each entity and registers field accessors against the pool. This is a library on top of the core ref mechanism, not a VM built-in.

```rask
// Game extension (library, not VM core)
import raido.game

raido.game.provide_pool(vm, "enemies", enemies, [
    raido.game.Field.int("health"),
    raido.game.Field.number("x"),
    raido.game.Field.number("y"),
])
```

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
- **In-script:** `pcall(f, ...)` catches errors. `error(msg)` raises them.
