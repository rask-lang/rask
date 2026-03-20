<!-- id: raido.interop -->
<!-- status: proposed -->
<!-- summary: Rask integration API — VM creation, function registration, pool access, error propagation -->
<!-- depends: raido/vm.md, raido/values.md, memory/pools.md, memory/borrowing.md -->

# Rask Integration

Host API for embedding Raido. All interaction is safe — no `unsafe` required.

## VM Lifecycle

```rask
const vm = raido.Vm.new(raido.Config {
    arena_size: 256.kilobytes(),
    instruction_limit: 100_000,
    max_call_depth: 128,
})
ensure vm.close()

const chunk = try vm.compile("game.raido", source)
try vm.exec(chunk)
const result = try vm.call("on_start", [])
```

`Vm` is `@resource` — must be closed. `compile()` can fail. `exec()`/`call()` can fail with `ScriptError`.

## Function Registration

```rask
vm.register("damage", |ctx| {
    const target = try ctx.arg_handle(0)
    const amount = try ctx.arg_int(1)
    ctx.pool("enemies")[target].health -= amount
})
```

Closures receive a `CallContext` with typed argument access (`arg_int`, `arg_number`, `arg_string`, `arg_handle`, `arg_value`), pool access, and return methods. Rask errors in the closure become Raido runtime errors.

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
- **Mutable.** Scripts read and write entity fields.

This is what makes Raido possible without unsafe. Rask's borrowing model requires known scopes — `exec_with` creates that scope.

```raido
func on_update(dt) {
    for h in handles("enemies") {
        h.x = h.x + h.vx * dt
        if h.health <= 0 { remove(h) }
    }
}
```

## Error Propagation

- **Script → Host:** Raido runtime errors become `raido.ScriptError` (message, file, line, stack trace).
- **Host → Script:** Rask errors in registered functions become Raido runtime errors.
- **In-script:** `pcall(func, args...)` catches errors. `error(msg)` raises them.

## Globals and Userdata

```rask
vm.set_global("max_health", 100)
vm.set_global("gravity", 9.81)
vm.set_global("bg_color", raido.Value.userdata(Color { r: 255, g: 0, b: 0 }))
```

Userdata is an opaque box. Scripts can't inspect fields — pass it to host functions that know the type. `@resource` types cannot be userdata (compile error).
