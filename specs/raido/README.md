<!-- id: raido.overview -->
<!-- status: proposed -->
<!-- summary: Embedded scripting language for Rask — Lua-like VM for games and game servers -->

# Raido

Tiny embedded scripting language for Rask applications. Think Lua, but native to Rask's ownership model and entity systems.

## Why Raido Exists

Games and game servers need a scripting layer. Modders write gameplay logic, AI behaviors, and plugin systems in a dynamic language while the engine runs compiled Rask. The obvious answer is "embed Lua via C FFI" — Rask already supports `import c "lua.h"`.

I chose to build a native alternative because the Lua-via-FFI experience is bad for Rask specifically:

1. **Every Lua API call requires `unsafe`.** Creating a VM, pushing values, calling functions — all unsafe. A game loop touching entities every frame means hundreds of unsafe blocks.

2. **Lua doesn't know about handles.** Rask's entity system is `Pool<T>` + `Handle<T>`. Exposing this to Lua means manual `void*` userdata with C-level lifecycle management. Raido makes handles a first-class type — scripts do `h.health -= 1` and the VM resolves the handle against a host-provided pool.

3. **`longjmp` breaks `ensure`.** Lua uses `longjmp` for errors, which skips Rask's `ensure` cleanup blocks. Raido errors are values that propagate through Rask's `T or E` system.

4. **String copies everywhere.** Lua has its own interned string system. Every string crossing the boundary gets copied. Raido shares Rask's immutable refcounted `string` — zero-copy in both directions.

5. **No C toolchain dependency.** Raido ships as a Rask library. No system Lua, no pkg-config, no header files.

**When to use Lua instead:** If you need Lua's ecosystem — existing scripts, community libraries, LuaJIT performance. Raido is for when you want tight Rask integration without unsafe ceremony.

## The 30-Second Pitch

Host side (Rask):
```rask
import raido

func game_loop(enemies: Pool<Enemy>, dt: f64) -> () or Error {
    const vm = raido.Vm.new(raido.Config {
        arena_size: 256.kilobytes(),
        instruction_limit: 100_000,
    })
    ensure vm.close()

    const script = try vm.compile("ai.raido", ai_source)
    try vm.exec(script)

    // Run script with pool access — scoped borrowing
    try vm.exec_with(|scope| {
        scope.provide_pool("enemies", enemies)
        scope.call("on_update", [raido.Value.number(dt)])
    })
}
```

Script side (Raido):
```raido
func on_update(dt)
    for h in handles("enemies") do
        h.x = h.x + h.vx * dt
        h.y = h.y + h.vy * dt

        if h.health <= 0 then
            remove(h)
        end
    end
end
```

No unsafe. No FFI glue. Handles resolve through pools provided by the host. Arena allocation — no GC pauses. The VM enforces memory and instruction limits.

## Design Principles

| Principle | Means |
|-----------|-------|
| **Tiny** | Small VM (~1 KB base), small stdlib, small language. Hundreds of VMs per server. |
| **Handle-native** | Handles are a first-class type. `h.field` does a pool lookup in the VM. |
| **Safe boundary** | All host interaction through typed API. No unsafe required. |
| **Host controls everything** | No I/O, no filesystem, no networking by default. Host provides capabilities. |
| **Game-loop friendly** | Instruction limits, memory limits, hot reload, coroutines for AI. |

## Specs

| Spec | Description |
|------|-------------|
| [values.md](values.md) | Value model, types, NaN-boxing, Rask type mapping |
| [syntax.md](syntax.md) | Language syntax and semantics |
| [vm.md](vm.md) | Register-based VM, GC, memory/instruction limits |
| [interop.md](interop.md) | Rask integration API — the critical spec |
| [coroutines.md](coroutines.md) | Cooperative multitasking for game AI |
| [stdlib.md](stdlib.md) | Built-in library (minimal, host-controlled) |

## Use Cases

1. **Game AI** — Coroutine-based behavior: patrol, chase, flee. Scripts yield between states, resume each frame.
2. **Modding** — Players write gameplay modifications. Sandboxed: memory-limited, no filesystem access, instruction-capped.
3. **Game server plugins** — Server operators extend behavior. Hot-reloadable without server restart.
4. **Level scripting** — Trigger zones, cutscenes, quest logic. Event-driven with handle-based entity access.
5. **Configuration** — Dynamic config that's more than key-value but less than a full program.

---

## Appendix (non-normative)

### Rationale

I went back and forth on whether Raido should exist at all. The honest answer: for most applications, Lua-via-FFI is fine. Raido exists specifically because Rask's entity system (`Pool<T>` + `Handle<T>`) is central to game architecture, and bridging it through C FFI is a poor experience.

The handle-as-first-class-type decision is what makes this worth building. If Rask used raw pointers for entities (like C++), Lua's userdata model would work fine and Raido wouldn't be necessary.

### See Also

- `mem.pools` — Handle-based entity storage
- `mem.borrowing` — Block-scoped borrowing (basis for `exec_with`)
- `mem.resource-types` — Why `@resource` types can't be userdata
- `struct.c-interop` — The Lua-via-FFI alternative
