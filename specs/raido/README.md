<!-- id: raido.overview -->
<!-- status: proposed -->
<!-- summary: Raido — deterministic scratchpad scripting language with serializable VM and Rask interop -->

# Raido

Deterministic scratchpad scripting language for custom entity scripts on game servers. Dynamic subset of Rask syntax. Serializable VM state. Fixed-point arithmetic.

**Rask without types.** Same `{}` blocks, `if`/`else if`, `match`/`=>`, `for`/`in`, `||` closures. No type annotations, no ownership, no `try`/`ensure`. Modders learning Raido are learning Rask syntax.

## Why Not Lua-via-FFI

1. Every Lua API call requires `unsafe`. Raido's host API is fully safe.
2. Lua doesn't know about `Handle<T>`. Raido makes handles first-class — `h.health -= 1` resolves through the host pool.
3. Lua's `longjmp` errors skip `ensure` blocks. Raido errors propagate through `T or E`.
4. Lua is not deterministic. Raido uses fixed-point math for bitwise-identical cross-platform results.
5. Lua state is not trivially serializable. Raido VM state serializes to bytes — save, migrate, replay.

## Host API

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

    try vm.exec_with(|scope| {
        scope.provide_pool("enemies", enemies)
        scope.call("on_update", [raido.Value.number(dt)])
    })
    vm.frame_end()  // arena wraps — frame temporaries freed
}
```

## Script Example

```raido
func on_update(dt) {
    for h in handles("enemies") {
        h.x = h.x + h.vx * dt
        h.y = h.y + h.vy * dt

        if h.health <= 0 {
            remove(h)
        }
    }
}

// Coroutine-based AI
func patrol(h) {
    while true {
        const wp = next_waypoint(h)
        while !near(h, wp) {
            move_toward(h, wp)
            yield()
        }
        wait(2.0)
    }
}
```

## Key Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Syntax | Dynamic Rask subset | Modders learn Rask syntax. No new language to learn. |
| VM | Stack-based | Simpler to implement, simpler to serialize. |
| Determinism | Fixed-point 32.32 `number` | Integer math = deterministic + fast. Scripts write `3.14`, compiler handles conversion. |
| Serialization | Entire VM state → bytes | Save/restore, server migration, replay. No pointers in values — arena offsets only. |
| Values | 8 bytes, 10 types | nil, bool, int, number (32.32 fixed), string, array, map, function, handle, userdata. |
| Collections | Separate array `[]` and map `{k: v}` | Maps to Rask's Vec/Map. No Lua table confusion. |
| Handles | First-class with pool-resolved field access | `h.field` does a pool lookup. Core innovation. |
| Memory | Arena with per-frame wrapping | Temporaries freed at `frame_end()`. Persistent state (globals, coroutines) survives. |
| Functions | Host functions by name, not pointer | Serializable. Re-registered on deserialize. |
| Safety | `exec_with` scoped pool borrowing | Pools borrowed for closure duration. No unsafe. |
| Globals | Explicit `global` keyword | No accidental globals. |
| Strings | `"damage: {amount}"` interpolation | Kills concatenation chains. |
| Random | Seedable PRNG in VM state | Deterministic. Serializes with the VM. |
| Coroutines | yield/resume, `wait(seconds)` | Sequential AI without state machines. State serializable. |
| Stdlib | Math (fixed-point clamp/lerp), string, array, map, bit. No I/O. | Host provides capabilities. |

## Detailed Specs

| Spec | What it covers |
|------|----------------|
| [values.md](values.md) | Types, softfloat, serializable representation, handles |
| [syntax.md](syntax.md) | Grammar, variables, functions, control flow, operators |
| [vm.md](vm.md) | Stack VM, determinism, serialization, frame wrapping, instruction set |
| [interop.md](interop.md) | VM lifecycle, function registration, `exec_with`, error propagation |
| [coroutines.md](coroutines.md) | Cooperative multitasking, game AI patterns |
| [stdlib.md](stdlib.md) | Built-in functions |

## Open Questions

- Arena frame wrapping: how does the persistent region grow? Fixed budget? Promote on assignment to global?
- Coroutine locals: persistent or frame-local? (Must be persistent — they survive across yields.)
- Fixed-point: 32.32 or 48.16? More integer range vs more precision.
- Serialization format: custom binary? Versioned for forward compat?
- How do closures serialize when they capture mutable upvalues?
- Should Raido ship as part of Rask stdlib or as a separate crate?
