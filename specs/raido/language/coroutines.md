<!-- id: raido.coroutines -->
<!-- status: proposed -->
<!-- summary: Cooperative multitasking — coroutine values with methods, try integration, serializable state -->
<!-- depends: raido/language/syntax.md -->

# Coroutines

Cooperative multitasking. Yield mid-function, resume later. Fully serializable — a coroutine can be suspended, the VM serialized, and the coroutine resumed on a different machine.

## API

Method-based, matching Rask's object-oriented style:

```raido
const co = coroutine(patrol, entity)   // create from function + args
const value = try co.resume()          // resume — errors propagate via try
const s = co.status                    // "suspended", "running", "dead"
yield(values...)                       // suspend, return values to resumer
```

`coroutine(func, args...)` creates a suspended coroutine. The function and initial args are captured — `resume()` starts execution on first call, continues from last `yield` on subsequent calls.

`co.resume(args...)` returns the value passed to `yield()`. If the coroutine errors, the error propagates through `try` like any other call. No special `true/false` return convention.

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
func patrol(entity) {
    while true {
        const wp = next_waypoint(entity)
        while !near(entity, wp) {
            move_toward(entity, wp)
            yield()
        }
        wait(2.0)
    }
}
```

**Workflow steps:**
```raido
func onboarding(user) {
    send_welcome_email(user)
    yield()  // host resumes when email confirmed
    create_account(user)
    yield()  // host resumes when account provisioned
    send_setup_guide(user)
}
```

**Interactive dialogue:**
```raido
func conversation(npc, player) {
    say(npc, "Hello, traveler.")
    const choice = yield()  // host resumes with player's choice
    match choice {
        "quest" => start_quest(npc, player),
        "trade" => open_shop(npc, player),
        _ => say(npc, "Safe travels."),
    }
}
```

## Serialization

Coroutine state is part of the VM's serializable state. See [vm/architecture.md](../vm/architecture.md#serialization) for format details.
