<!-- id: raido.overview -->
<!-- status: proposed -->
<!-- summary: Raido — scratchpad scripting language with VM and Rask interop -->

# Raido

Dynamic subset of Rask syntax running in an arena-allocated VM. Scratchpad language for custom entity scripts on game servers.

**Rask without types.** Same `{}` blocks, `if`/`else if`, `match`/`=>`, `for`/`in`, `||` closures. No type annotations, no ownership, no `try`/`ensure`. Modders learning Raido are learning Rask syntax.

## Why Not Lua-via-FFI

1. Every Lua API call requires `unsafe`. Raido's host API is fully safe.
2. Lua doesn't know about `Handle<T>`. Raido makes handles first-class — `h.health -= 1` resolves through the host pool.
3. Lua's `longjmp` errors skip `ensure` blocks. Raido errors propagate through `T or E`.
4. Syntax discontinuity. Raido scripts read like untyped Rask code.

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
| Values | NaN-boxed 8 bytes, 9 types | nil, bool, int, number, string, array, map, function, handle (+userdata) |
| Collections | Separate array `[]` and map `{k: v}` | Maps to Rask's Vec/Map. No Lua table confusion. |
| Handles | First-class type with pool-resolved field access | `h.field` does a pool lookup. Core innovation. |
| Memory | Arena allocation, no GC | Bump alloc, bulk reset. Hundreds of VMs per server. |
| Strings | Arena-allocated, copied at boundary | Simple. No shared refcount lifetimes. |
| Safety | `exec_with` scoped pool borrowing | Pools borrowed for closure duration. No unsafe. |
| Globals | Explicit `global` keyword | No accidental globals (Lua's worst footgun). |
| Comments | `//` and `/* */` | Matches Rask. |
| Equality | `!=`, `&&`, `\|\|` | Matches Rask. No `~=`/`and`/`or`. |
| String interpolation | `"damage: {amount}"` | Kills `..` concatenation chains. |
| Iteration | `for x in arr {}`, `for k,v in map {}` | No `pairs()`/`ipairs()`. VM dispatches by type. |
| Coroutines | yield/resume, `wait(seconds)` | Sequential AI without state machines. |
| Stdlib | Math (with clamp/lerp), string, array, map, bit. No I/O. | Host provides capabilities. Scripts are sandboxed. |
| VM | Register-based, 32-bit instructions | ~1 KB base. Instruction budget per call. |
| Hot reload | `vm.reset()` + recompile | Arena reset destroys script state. Pool data survives. |

## Detailed Specs

| Spec | What it covers |
|------|----------------|
| [values.md](values.md) | NaN-boxing layout, type rules, array/map semantics, handle resolution, userdata |
| [syntax.md](syntax.md) | Grammar, variables, functions, control flow, operators, closures |
| [vm.md](vm.md) | Arena allocation, instruction set, bytecode format, call frames, limits |
| [interop.md](interop.md) | VM lifecycle, function registration, `exec_with`, field registration, error propagation |
| [coroutines.md](coroutines.md) | Cooperative multitasking, yield/resume, `wait()`, game AI patterns |
| [stdlib.md](stdlib.md) | Built-in functions: math, string, array, map, bit, core |

## Open Questions

- Should `for i, v in arr {}` use tuple destructuring or magic multiple binding?
- String interning strategy — intern all strings or only literals?
- Should closures capture by reference or by value?
- Method syntax for array/map (`arr.push(x)`) — how does the VM dispatch this?
- Error recovery in compilation — one error or collect many?
