<!-- id: raido.coroutines -->
<!-- status: proposed -->
<!-- summary: Cooperative multitasking — yield/resume with serializable state -->
<!-- depends: raido/vm.md, raido/syntax.md -->

# Coroutines

Cooperative multitasking. Yield mid-function, resume later. State preserved in arena and serializable.

## API

- `coroutine.create(func)` — wrap a function.
- `coroutine.resume(co, args...)` — resume. Returns `true, values` or `false, error`.
- `yield(values...)` — suspend, return values to resumer.
- `coroutine.status(co)` — `"suspended"`, `"running"`, `"dead"`.

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

Coroutine state (suspended stack, PC, locals) is part of the VM's serializable state. A workflow can yield, the server serializes the VM, restarts, deserializes, and the coroutine resumes exactly where it left off.

~200-500 bytes per suspended coroutine in the arena.
