<!-- id: raido.coroutines -->
<!-- status: proposed -->
<!-- summary: Cooperative multitasking for game AI — yield/resume with arena-allocated state -->
<!-- depends: raido/vm.md, raido/syntax.md -->

# Coroutines

Cooperative multitasking for game AI. Yield mid-function, resume next frame. State preserved in the arena.

## The Point

Without coroutines, AI is a state machine:
```raido
func on_update(h, dt) {
    match h.ai_state {
        "patrol" => {
            move_toward(h, h.waypoint)
            if arrived(h) { h.ai_state = "wait"; h.wait_timer = 2.0 }
        },
        "wait" => {
            h.wait_timer = h.wait_timer - dt
            if h.wait_timer <= 0 { h.ai_state = "patrol" }
        },
        _ => {},
    }
}
```

With coroutines, AI reads like a sequence:
```raido
func patrol(h) {
    while true {
        const wp = next_waypoint(h)
        while !near(h, wp) { move_toward(h, wp); yield() }
        wait(2.0)
    }
}
```

## API

- `coroutine.create(func)` — wrap a function in a coroutine.
- `coroutine.resume(co, args...)` — resume. Returns `true, values` or `false, error`.
- `yield(values...)` — suspend, return values to resumer.
- `coroutine.status(co)` — `"suspended"`, `"running"`, or `"dead"`.
- `wait(seconds)` — built-in helper, yields until duration elapsed (uses dt from resume).

## Game Pattern

Host creates a coroutine per entity, resumes each frame:

```rask
try vm.exec_with(|scope| {
    scope.provide_pool("enemies", enemies)
    scope.call("resume_ai", [raido.Value.string(enemy_id), raido.Value.number(dt)])
})
```

```raido
func enemy_brain(h) {
    while true {
        if h.health < 20 {
            flee(h)
        } else if can_see_player(h) {
            chase(h)
            attack(h)
        } else {
            patrol(h)
        }
        yield()
    }
}
```

Coroutine state lives in the arena. `vm.reset()` kills all coroutines. ~200-500 bytes per suspended coroutine.
