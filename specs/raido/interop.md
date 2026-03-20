<!-- id: raido.interop -->
<!-- status: proposed -->
<!-- summary: Rask integration API — VM creation, function registration, pool access, error propagation -->
<!-- depends: raido/vm.md, raido/values.md, memory/pools.md, memory/borrowing.md, memory/resource-types.md -->

# Rask Integration

The host API for embedding Raido in Rask applications. All interaction is safe — no `unsafe` blocks required.

This is the spec that matters most. Everything else (syntax, VM internals, arena allocation) is implementation detail. The integration API is what developers actually use.

## VM Lifecycle

| Rule | Description |
|------|-------------|
| **L1: Creation** | `Vm.new(config)` creates a VM with memory and instruction limits. |
| **L2: Resource** | `Vm` is `@resource` — must be closed explicitly. |
| **L3: Compilation** | `vm.compile(name, source)` produces a `Chunk`. Returns `CompileError` on failure. |
| **L4: Execution** | `vm.exec(chunk)` runs a compiled chunk (defines globals, executes top-level code). |
| **L5: Calling** | `vm.call(name, args)` calls a Raido function by name. Returns result or `ScriptError`. |
| **L6: Close** | `vm.close()` frees all VM memory, runs finalizers for userdata, abandons coroutines. |

```rask
import raido

func main() -> () or Error {
    const vm = raido.Vm.new(raido.Config {
        arena_size: 256.kilobytes(),
        instruction_limit: 100_000,
    })
    ensure vm.close()

    const chunk = try vm.compile("game.raido", source)
    try vm.exec(chunk)

    const result = try vm.call("on_start", [])
    print("Script returned: {result}")
}
```

## Config

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `arena_size` | `u64` | 256 KB | Arena size for VM allocations |
| `instruction_limit` | `u64` | 100,000 | Per-call instruction budget |
| `max_call_depth` | `u32` | 128 | Maximum call stack depth |

Small defaults — these are entity scripts, not application code. Override what you need.

## Function Registration

| Rule | Description |
|------|-------------|
| **R1: Closure-based** | `vm.register(name, closure)` registers a Rask closure as a callable Raido function. |
| **R2: CallContext** | The closure receives a `CallContext` for typed argument access and return values. |
| **R3: Safe** | No unsafe required. Type mismatches return errors, not undefined behavior. |
| **R4: Overwrite** | Registering a name that already exists overwrites the previous function. |
| **R5: Error propagation** | Rask errors (`T or E`) in the closure become Raido runtime errors. |

```rask
// Register a simple function
vm.register("damage", |ctx| {
    const target = try ctx.arg_handle(0)
    const amount = try ctx.arg_int(1)
    ctx.pool("enemies")[target].health -= amount
})

// Register with return value
vm.register("distance", |ctx| {
    const ax = try ctx.arg_number(0)
    const ay = try ctx.arg_number(1)
    const bx = try ctx.arg_number(2)
    const by = try ctx.arg_number(3)
    const dx = bx - ax
    const dy = by - ay
    ctx.return_number(math.sqrt(dx * dx + dy * dy))
})

// Register with error
vm.register("load_config", |ctx| {
    const path = try ctx.arg_string(0)
    const data = try fs.read(path)  // Rask error → Raido error
    ctx.return_string(data)
})
```

### CallContext API

| Method | Description |
|--------|-------------|
| `arg_count() -> u32` | Number of arguments passed |
| `arg_nil(n) -> bool` | Check if argument n is nil |
| `arg_bool(n) -> bool or ArgError` | Get bool argument |
| `arg_int(n) -> i64 or ArgError` | Get integer argument |
| `arg_number(n) -> f64 or ArgError` | Get number argument (accepts int, promotes) |
| `arg_string(n) -> string or ArgError` | Get string argument |
| `arg_handle(n) -> Handle<any> or ArgError` | Get handle argument |
| `arg_table(n) -> TableRef or ArgError` | Get table reference (valid for call duration) |
| `arg_userdata::<T>(n) -> T or ArgError` | Get typed userdata (runtime type check) |
| `arg_value(n) -> Value` | Get raw Value (any type) |
| `pool(name) -> PoolAccess` | Access a pool provided via `exec_with` |
| `return_nil()` | Return nil |
| `return_bool(v)` | Return bool |
| `return_int(v)` | Return int |
| `return_number(v)` | Return number |
| `return_string(v)` | Return string |
| `return_value(v)` | Return any Value |

## Pool Access (exec_with)

| Rule | Description |
|------|-------------|
| **P1: Scoped borrowing** | `vm.exec_with(closure)` borrows pools for the closure's duration. Pools released when closure returns. |
| **P2: provide_pool** | `scope.provide_pool(name, pool)` makes a pool available to scripts under the given name. |
| **P3: Handle resolution** | `h.field` in scripts resolves through the pool provided for that handle's pool tag. |
| **P4: Mutable access** | Pool access is mutable — scripts can read and write entity fields. |
| **P5: Insert/remove** | Host functions can insert into and remove from provided pools during `exec_with`. |
| **P6: Multiple pools** | Multiple pools can be provided simultaneously. Handle pool tags route to the correct pool. |
| **P7: Missing pool** | Accessing a handle whose pool wasn't provided raises a runtime error. |

```rask
// Game loop integration
func game_update(
    vm: Vm,
    enemies: Pool<Enemy>,
    items: Pool<Item>,
    dt: f64,
) -> () or raido.ScriptError {
    try vm.exec_with(|scope| {
        scope.provide_pool("enemies", enemies)
        scope.provide_pool("items", items)
        scope.call("on_update", [raido.Value.number(dt)])
    })
}
```

```raido
func on_update(dt)
    -- handles("enemies") iterates Handle values from the "enemies" pool
    for h in handles("enemies") do
        h.x = h.x + h.vx * dt
        h.y = h.y + h.vy * dt

        if h.health <= 0 then
            -- drop_loot creates items in the "items" pool
            drop_loot(h.x, h.y)
            remove(h)
        end
    end
end
```

### How Handle Field Access Works

When the script does `h.x`:

1. VM reads the pool name tag from the handle value
2. Looks up the pool in the `exec_with` scope by name
3. Validates the handle's generation against the pool entry
4. Reads field `x` from the pool entry
5. Converts the Rask value to a Raido `Value` and pushes to register

For `h.x = expr`:

1-3. Same as above
4. Converts the Raido `Value` to the Rask field type
5. Writes to the field in the pool entry

**Type conversion on field access** is the key detail. The host registers which struct fields are accessible and their types when calling `provide_pool`. Fields not registered are invisible to scripts.

### Field Registration

| Rule | Description |
|------|-------------|
| **FR1: Explicit fields** | The host declares which struct fields are script-accessible when providing a pool. |
| **FR2: Type mapping** | Each field maps to a Raido type: `i32/i64 → int`, `f32/f64 → number`, `bool → bool`, `string → string`. |
| **FR3: Computed fields** | Host can register computed properties: read-only values derived from struct state. |
| **FR4: Hidden fields** | Fields not registered are invisible. Scripts can't access them. |

```rask
scope.provide_pool("enemies", enemies, raido.Fields {
    fields: [
        raido.Field.int("health"),
        raido.Field.number("x"),
        raido.Field.number("y"),
        raido.Field.number("vx"),
        raido.Field.number("vy"),
        raido.Field.bool("aggro"),
        raido.Field.string("name"),
    ],
    computed: [
        raido.Computed.number("speed", |e| math.sqrt(e.vx * e.vx + e.vy * e.vy)),
    ],
})
```

**FR4 (hidden fields)** is important for security. Internal engine state (physics state, render data, network IDs) shouldn't be accessible from scripts. The host explicitly opts in to what's visible.

## Globals

| Rule | Description |
|------|-------------|
| **G1: Set** | `vm.set_global(name, value)` sets a global variable. |
| **G2: Get** | `vm.get_global(name) -> Value` reads a global. |
| **G3: Value conversion** | Rask primitives auto-convert: `i32/i64 → int`, `f64 → number`, `bool → bool`, `string → string`. |
| **G4: Table globals** | Use `raido.Table.new()` to create and populate tables from the host side. |

```rask
vm.set_global("max_health", 100)
vm.set_global("game_mode", "survival")
vm.set_global("gravity", 9.81)

vm.set_global("config", raido.Table.from_map(Map.from([
    ("spawn_rate", raido.Value.number(2.5)),
    ("max_enemies", raido.Value.int(50)),
])))
```

## Userdata

| Rule | Description |
|------|-------------|
| **U1: Move in** | `raido.Value.userdata(own value)` moves a Rask value into the VM's arena. |
| **U2: Copy in** | Copy types are copied. The host retains its copy. |
| **U3: No resources** | `@resource` types cannot be userdata. Compile error. |
| **U4: Typed extraction** | `ctx.arg_userdata::<T>(n)` does a runtime type check. |
| **U5: Opaque** | Scripts can't inspect userdata fields. Pass it to host functions that know the type. |
| **U6: Methods** | Host can register methods on userdata types: `vm.register_method::<Color>("darken", closure)`. |
| **U7: Drop on reset** | Non-Copy userdata has `Drop` called during `vm.reset()`. |

```rask
// Pass a Color as userdata
vm.set_global("red", raido.Value.userdata(Color { r: 255, g: 0, b: 0 }))

// Register method
vm.register_method::<Color>("darken", |ctx| {
    const color = try ctx.self_userdata::<Color>()
    const factor = try ctx.arg_number(0)
    ctx.return_userdata(Color {
        r: (color.r as f64 * factor) as u8,
        g: (color.g as f64 * factor) as u8,
        b: (color.b as f64 * factor) as u8,
    })
})
```

```raido
-- Script side
const darker = red:darken(0.5)
set_background(darker)
```

## Error Propagation

| Rule | Description |
|------|-------------|
| **E1: Script → Host** | Raido runtime errors become `raido.ScriptError` on the Rask side. |
| **E2: Host → Script** | Rask errors in registered functions become Raido runtime errors with the error message. |
| **E3: pcall** | `pcall(func, args...)` catches errors and returns `success, result_or_error`. |
| **E4: error()** | `error(message)` raises a runtime error from script code. |
| **E5: ScriptError contents** | `ScriptError` includes: message, source file, line number, call stack trace. |

```rask
// Host catches script error
match vm.call("on_update", args) {
    Ok(result) => process(result),
    Err(e) => {
        log.warn("Script error: {e.message} at {e.file}:{e.line}")
        log.warn("Stack trace:\n{e.stack_trace}")
    }
}
```

```raido
-- Script catches errors
local ok, err = pcall(func()
    dangerous_operation()
end)
if not ok then
    print("Caught: " .. err)
end
```

## Safety Boundary

| Guarantee | Mechanism | Rule |
|-----------|-----------|------|
| No memory corruption | VM manages own heap; no raw pointers | VM arch |
| No unbounded memory | Memory limit (`raido.vm/M1`) | L1 |
| No unbounded execution | Instruction limit (`raido.vm/IL1`) | L1 |
| No resource leaks | `@resource` types disallowed as userdata | U3 |
| No data races | VM is `Send` but not `Sync` (`raido.vm/VM3`) | VM3 |
| No unauthorized I/O | No stdlib I/O; host provides capabilities | `raido.stdlib` |
| No unauthorized field access | Explicit field registration (FR1) | FR1, FR4 |

Scripts are sandboxed. They can only do what the host explicitly allows through registered functions and provided pools.

## Edge Cases

| Case | Rule | Behavior |
|------|------|----------|
| Call non-existent function | L5 | Returns `ScriptError`: "undefined function 'name'" |
| Wrong argument type | R2 | `ArgError` with expected/actual types |
| `exec_with` while another `exec_with` active | P1 | Compile error — nested `exec_with` not allowed |
| Pool entry removed during iteration | P5 | Handle becomes stale; next access raises error |
| Host function modifies pool and script reads | P4 | Consistent — both go through the same pool reference |
| `vm.call()` outside `exec_with` | P7 | Handle field access raises "pool not available" error |
| Registering same function name twice | R4 | Second registration overwrites first |

## Error Messages

```
ERROR [raido.interop/P7]: pool not available
   |
8  |  h.health -= 10
   |  ^ handle tagged "enemies" but no pool named "enemies" was provided

WHY: Handle field access requires the pool to be provided via exec_with.

FIX: Ensure vm.exec_with provides the pool:
   vm.exec_with(|scope| {
       scope.provide_pool("enemies", enemies)
       scope.call("on_update", args)
   })
```

```
ERROR [raido.interop/FR4]: field not accessible
   |
3  |  h.internal_id
   |    ^^^^^^^^^^^ field "internal_id" is not registered for pool "enemies"

WHY: Only explicitly registered fields are accessible from scripts.

FIX: Register the field when providing the pool, or access it through a host function.
```

---

## Appendix (non-normative)

### Rationale

**P1 (scoped borrowing):** This is the design that makes Raido possible without unsafe. Rask's borrowing model requires borrows to have a known scope. `exec_with` creates that scope — pools are borrowed for the closure's duration, then released. The VM never stores a pool reference beyond the scope.

I considered making pools permanently available to the VM, but that would require either:
- Storing a reference in the VM (impossible — violates `mem.borrowing`)
- Using `Arc<Mutex<Pool>>` (violates Rask's "no shared mutable state" philosophy)

Scoped access is the right answer. Game loops already call scripts per-frame, so the scope matches the natural execution pattern.

**FR1 (explicit fields):** I debated auto-exposing all public struct fields. That's more ergonomic but less safe — engine internals leak into scripts, and refactoring struct fields silently breaks script compatibility. Explicit registration acts as a stable API contract between engine and scripts.

**E2 (host errors → script errors):** The alternative was to have host errors crash the VM. That's too harsh — a file read error in a host function shouldn't kill the entire scripting layer. Errors propagate as Raido runtime errors, catchable via `pcall`.

### Patterns

**Per-frame game loop:**
```rask
// Called every frame
func frame_update(vm: Vm, world: World, dt: f64) -> () or Error {
    try vm.exec_with(|scope| {
        scope.provide_pool("players", world.players)
        scope.provide_pool("enemies", world.enemies)
        scope.provide_pool("items", world.items)
        scope.call("on_frame", [raido.Value.number(dt)])
    })
}
```

**Event-driven:**
```rask
func on_collision(vm: Vm, a: Handle<Entity>, b: Handle<Entity>, world: World) -> () or Error {
    try vm.exec_with(|scope| {
        scope.provide_pool("entities", world.entities)
        scope.call("on_collision", [
            raido.Value.handle(a),
            raido.Value.handle(b),
        ])
    })
}
```

**Server plugin:**
```rask
func handle_chat(vm: Vm, sender: Handle<Player>, message: string, players: Pool<Player>) -> () or Error {
    try vm.exec_with(|scope| {
        scope.provide_pool("players", players)
        scope.call("on_chat", [
            raido.Value.handle(sender),
            raido.Value.string(message),
        ])
    })
}
```

### See Also

- `raido.vm` — VM internals, arena allocation, instruction limits
- `raido.values` — Value types and type conversion
- `mem.borrowing` — Why `exec_with` uses scoped borrowing
- `mem.pools` — Pool/Handle system
- `mem.resource-types` — Why resources can't be userdata
