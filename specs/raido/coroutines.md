<!-- id: raido.coroutines -->
<!-- status: proposed -->
<!-- summary: Cooperative multitasking for game AI — yield/resume with arena-allocated state -->
<!-- depends: raido/vm.md, raido/syntax.md -->

# Coroutines

Cooperative multitasking for game AI. A coroutine yields mid-function, the host resumes it next frame. State (local variables, call stack position) preserved in the arena between yields.

## Why Coroutines

Game AI without coroutines:
```raido
-- State machine approach: manual, error-prone
func on_update(h, dt)
    match h.ai_state
        case "patrol" then
            move_toward(h, h.waypoint)
            if arrived(h) then
                h.ai_state = "wait"
                h.wait_timer = 2.0
            end
        case "wait" then
            h.wait_timer = h.wait_timer - dt
            if h.wait_timer <= 0 then
                h.waypoint = next_waypoint(h)
                h.ai_state = "patrol"
            end
        case _ then end
    end
end
```

Game AI with coroutines:
```raido
-- Coroutine approach: reads like a sequence
func patrol(h)
    while true do
        const wp = next_waypoint(h)
        move_toward(h, wp)
        while not arrived(h) do yield() end
        wait(2.0)
    end
end
```

The coroutine version is shorter, sequential, and doesn't need explicit state management. The `yield()` returns control to the host; next frame, the coroutine resumes where it left off.

## Rules

| Rule | Description |
|------|-------------|
| **C1: Create** | `coroutine.create(func)` wraps a function in a coroutine object. |
| **C2: Resume** | `coroutine.resume(co, args...)` resumes execution. Returns `true, values` on yield/return, `false, error` on error. |
| **C3: Yield** | `yield(values...)` suspends the coroutine, returning values to the resumer. |
| **C4: Status** | `coroutine.status(co)` returns `"suspended"`, `"running"`, or `"dead"`. |
| **C5: Arena-allocated** | Coroutine state (saved registers, call stack) lives in the VM's arena. |
| **C6: Instruction budget** | Coroutines share the per-call instruction budget. Yielding saves remaining budget. |
| **C7: Dead on finish** | When the coroutine's function returns, the coroutine becomes `"dead"`. Resuming a dead coroutine returns an error. |

```raido
-- Basic yield/resume
func counter(start)
    local n = start
    while true do
        yield(n)
        n = n + 1
    end
end

const co = coroutine.create(counter)
print(coroutine.resume(co, 10))  -- true, 10
print(coroutine.resume(co))       -- true, 11
print(coroutine.resume(co))       -- true, 12
```

## Game AI Pattern

The typical pattern: host creates a coroutine per entity, resumes it each frame.

```rask
// Rask host — create coroutine for entity AI
try vm.exec_with(|scope| {
    scope.provide_pool("enemies", enemies)

    // Create coroutine from script function
    const co = try scope.call("create_ai", [raido.Value.handle(enemy_h)])
    vm.set_global("ai_" .. enemy_id, co)
})

// Each frame — resume the coroutine
try vm.exec_with(|scope| {
    scope.provide_pool("enemies", enemies)
    scope.call("resume_ai", [
        raido.Value.string("ai_" .. enemy_id),
        raido.Value.number(dt),
    ])
})
```

```raido
func create_ai(h)
    return coroutine.create(func()
        patrol(h)
    end)
end

func resume_ai(co_name, dt)
    const co = _G[co_name]
    if co and coroutine.status(co) ~= "dead" then
        coroutine.resume(co, dt)
    end
end

-- AI behavior — reads sequentially, yields between steps
func patrol(h)
    while true do
        const wp = next_waypoint(h)

        -- Move toward waypoint, yielding each frame
        while not near(h, wp) do
            move_toward(h, wp)
            yield()
        end

        -- Wait 2 seconds
        local timer = 2.0
        while timer > 0 do
            const dt = yield()
            timer = timer - dt
        end
    end
end

func near(h, target)
    const dx = h.x - target.x
    const dy = h.y - target.y
    return dx * dx + dy * dy < 1.0
end
```

## wait() Helper

| Rule | Description |
|------|-------------|
| **W1: Built-in helper** | `wait(seconds)` yields repeatedly until the duration has elapsed. |
| **W2: Uses dt** | `wait` expects `dt` (delta time) to be passed via `coroutine.resume(co, dt)`. |

```raido
-- wait() is roughly:
func wait(seconds)
    local remaining = seconds
    while remaining > 0 do
        const dt = yield()
        remaining = remaining - dt
    end
end

-- Usage in AI
func guard_behavior(h)
    while true do
        patrol(h)
        wait(3.0)      -- stand still for 3 seconds
        look_around(h)
        wait(1.0)
    end
end
```

## Arena Implications

| Rule | Description |
|------|-------------|
| **A1: State in arena** | Coroutine saved state (registers, PC, call frames) allocated in arena. |
| **A2: Reset kills all** | `vm.reset()` destroys all coroutines. No way to preserve them. |
| **A3: Size** | Each suspended coroutine uses ~200-500 bytes of arena space (saved registers + call stack). |

With 256 KB arenas, you can comfortably have ~100 suspended coroutines. For entity AI where each entity has one coroutine, this limits per-VM entity count. If you need more, increase the arena size.

**A2 (reset kills all)** means hot reload destroys coroutine state. AI behaviors restart from the beginning. This is usually fine — entity AI scripts are designed to be interruptible. If you need persistent state across reloads, store it in pool fields, not coroutine locals.

## Edge Cases

| Case | Rule | Behavior |
|------|------|----------|
| Resume dead coroutine | C7 | Returns `false, "cannot resume dead coroutine"`. |
| Yield outside coroutine | C3 | Runtime error: "yield called outside coroutine". |
| Error inside coroutine | C2 | `resume` returns `false, error_message`. Coroutine becomes dead. |
| Instruction limit hit in coroutine | C6 | Runtime error propagated to resumer. Coroutine becomes dead. |
| Nested coroutines | C3 | `yield` suspends the innermost running coroutine. |
| Coroutine yields during `exec_with` | C3, `raido.interop/P1` | Allowed — pool access remains valid until `exec_with` returns. |

## Error Messages

```
ERROR [raido.coroutines/C7]: cannot resume dead coroutine
   |
15 |  coroutine.resume(co)
   |  ^^^^^^^^^^^^^^^^^^^^ coroutine finished or errored

WHY: The coroutine's function has returned or raised an error.

FIX: Check status before resuming:
   if coroutine.status(co) ~= "dead" then
       coroutine.resume(co)
   end
```

---

## Appendix (non-normative)

### Rationale

**C5 (arena-allocated):** Coroutine state could theoretically be heap-allocated separately from the arena. But that would require individual allocation/deallocation tracking — effectively reintroducing a memory manager for coroutines. Arena allocation means coroutine creation is a bump, and cleanup is free (arena reset).

**W1 (built-in wait):** `wait()` could be a stdlib function, but it's so fundamental to game AI that it deserves first-class treatment. Every game scripting system needs it. Making it built-in means it's always available and well-optimized.

### Patterns

**Behavior trees via coroutines:**
```raido
func enemy_brain(h)
    while true do
        if h.health < 20 then
            flee(h)
        elseif can_see_player(h) then
            chase(h)
            attack(h)
        else
            patrol(h)
        end
        yield()
    end
end

func flee(h)
    const escape = away_from_player(h)
    while h.health < 50 and not safe(h) do
        move_toward(h, escape)
        yield()
    end
end

func chase(h)
    while can_see_player(h) and not in_range(h) do
        move_toward(h, player_pos())
        yield()
    end
end
```

**Timed sequences (cutscenes, tutorials):**
```raido
func tutorial()
    show_text("Welcome!")
    wait(2.0)
    show_text("Click to move")
    while not clicked() do yield() end
    show_text("Great!")
    wait(1.0)
    hide_text()
end
```

### See Also

- `raido.vm` — Arena allocation and instruction budgets
- `raido.interop` — How the host creates and resumes coroutines
- `raido.syntax` — `yield` keyword
