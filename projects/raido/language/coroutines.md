<!-- id: raido.coroutines -->
<!-- status: proposed -->
<!-- summary: Cooperative multitasking -- coroutine values with methods, try integration, serializable state -->
<!-- depends: raido/language/syntax.md -->

# Coroutines

Cooperative multitasking. Yield mid-function, resume later. Fully serializable -- a coroutine can be suspended, the VM serialized, and the coroutine resumed on a different machine.

## API

```raido
// Create from function reference + args
const co = coroutine(patrol, entity, waypoints)

// Resume -- errors propagate via try
const value = try co.resume()

// Status
const s = co.status  // "suspended", "running", "dead"

// Yield from inside a coroutine
yield(value)
```

`coroutine(func, args...)` creates a suspended coroutine from a function reference and initial arguments. `resume()` starts execution on first call, continues from last `yield` on subsequent calls.

`co.resume(args...)` returns the value passed to `yield()`. If the coroutine errors, the error propagates through `try` like any other call.

```raido
// Catch coroutine errors
const value = try co.resume() else |e| {
    log("coroutine failed: {e}")
    return fallback
}
```

## Why

Coroutines turn state machines into sequential code. The host resumes, the script picks up where it left off.

**Game AI:**
```raido
func patrol(entity: Entity, route: array<Vec2>) {
    loop {
        for wp in route {
            while !near(entity, wp) {
                move_toward(entity, wp)
                yield
            }
            wait(2.0)
        }
    }
}
```

**Workflow steps:**
```raido
func onboarding(user: User) {
    send_welcome_email(user)
    yield  // host resumes when email confirmed
    create_account(user)
    yield  // host resumes when account provisioned
    send_setup_guide(user)
}
```

**Interactive dialogue:**
```raido
func conversation(npc: Entity, player: Entity) {
    say(npc, "Hello, traveler.")
    const choice: string = yield  // host resumes with player's choice
    match choice {
        "quest" => start_quest(npc, player),
        "trade" => open_shop(npc, player),
        _ => say(npc, "Safe travels."),
    }
}
```

## Function References, Not Closures

Coroutines are created from function references -- named top-level functions. No closures, no captured state. Context is passed as arguments.

```raido
// The function and its arguments are stored in the coroutine
func guard_loop(npc: Entity, post: Vec2, radius: number) {
    loop {
        patrol_area(npc, post, radius)
        yield
        if detect_threat(npc, post, radius) {
            alert(npc)
            yield
        }
    }
}

const co = coroutine(guard_loop, guard, tower_pos, 50.0)
```

## Resume with Value

Resumed coroutines can receive values from the host. `yield` returns the value passed by `resume`:

```raido
func combat_tick(fleet: FleetState) {
    loop {
        const orders: Orders = yield  // host passes orders on resume
        execute_orders(fleet, orders)
    }
}

// Host side:
try co.resume(current_orders)
```

## Serialization

Coroutine state is part of the VM's serializable state. A suspended coroutine's registers, call frames, and PC are captured. See [vm/architecture.md](../vm/architecture.md#serialization) for format details.
