<!-- id: raido.interop -->
<!-- status: proposed -->
<!-- summary: Rask integration API — VM creation, serialization, pool access, error propagation -->
<!-- depends: raido/vm.md, raido/values.md, memory/pools.md, memory/borrowing.md -->

# Rask Integration

Host API for embedding Raido. All interaction is safe — no `unsafe` required.

## VM Lifecycle

```rask
const vm = raido.Vm.new(raido.Config {
    arena_size: 256.kilobytes(),
    instruction_limit: 100_000,
})
ensure vm.close()

const chunk = try vm.compile("game.raido", source)
try vm.exec(chunk)
```

`Vm` is `@resource` — must be closed. `compile()` can fail. `exec()`/`call()` can fail with `ScriptError`.

## Game Loop

```rask
while running {
    try vm.exec_with(|scope| {
        scope.provide_pool("enemies", enemies)
        scope.call("on_update", [raido.Value.number(dt)])
    })
    vm.frame_end()  // arena wraps — frame temporaries freed, persistent state kept
}
```

## Function Registration

```rask
vm.register("damage", |ctx| {
    const target = try ctx.arg_handle(0)
    const amount = try ctx.arg_int(1)
    ctx.pool("enemies")[target].health -= amount
})
```

Host functions are registered by name. On serialize/deserialize, only the name is stored — the host must re-register functions after restoring a VM. Rask errors in registered functions become Raido runtime errors.

## Pool Access (exec_with)

```rask
try vm.exec_with(|scope| {
    scope.provide_pool("enemies", enemies, raido.Fields {
        fields: [
            raido.Field.int("health"),
            raido.Field.number("x"),
            raido.Field.number("y"),
        ],
    })
    scope.call("on_update", [raido.Value.number(dt)])
})
```

- **Scoped borrowing.** Pools borrowed for the closure's duration, then released.
- **Explicit field registration.** Only registered fields are visible to scripts.
- **Multiple pools.** Handle pool tags route field access to the correct pool.

This is what makes Raido possible without unsafe. Rask's borrowing model requires known scopes — `exec_with` creates that scope.

## Serialization

```rask
// Snapshot
const bytes = vm.serialize()

// Restore
const vm2 = raido.Vm.deserialize(bytes)
// Re-register host functions
vm2.register("damage", damage_fn)
// Re-provide pools via exec_with as usual
```

Serialize captures: value stack, globals, coroutine states, arena contents, PRNG state, instruction counter. Does not capture: host function closures (by name), pool references (re-provided), bytecode (re-loaded).

## Error Propagation

- **Script → Host:** Runtime errors become `raido.ScriptError` (message, file, line, stack trace).
- **Host → Script:** Rask errors in registered functions become Raido runtime errors.
- **In-script:** `pcall(func, args...)` catches errors. `error(msg)` raises them.

## Globals and Userdata

```rask
vm.set_global("max_health", 100)
vm.set_global("gravity", 9.81)
```

Userdata must be serializable — host registers serialize/deserialize pairs. `@resource` types and non-serializable types cannot be userdata.
