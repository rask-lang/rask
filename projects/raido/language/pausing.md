<!-- id: raido.pausing -->
<!-- status: proposed -->
<!-- summary: A hook pauses by calling a host function that takes time; the host resumes it with the result -->
<!-- depends: raido/language/syntax.md -->

# Pausing

A hook can call a host function that takes time, like `wait(2.0)` or `ask(player, "Trade?")`, and carry on from the next line when the host is ready. It reads like any other call:

```raido
func on_guard_spawned(g: Unit) {
    loop {
        for wp in g.route {
            while !near(g, wp) {
                move_toward(g, wp)
                wait(0.1)
            }
        }
        if nearest_enemy(g, 8.0) is Some(e) { attack(g, e) }
    }
}

func on_talk(npc: Unit, player: Unit) {
    let choice = ask(player, "Hello, traveler. Quest or trade?")
    if choice == "quest" { start_quest(npc, player) }
}
```

There's no `yield`, no coroutine object and no `async`. A function that pauses looks like one that doesn't, and so does everything that calls it.

## Where Pausing Comes From

The host's API module marks the functions that take time as `pausing`:

```raido
// game_api.rd
extern pausing func wait(seconds: float)
extern pausing func ask(player: Unit, prompt: string) -> string
```

The script can't pause any other way. What a pausing function means is the host's business: `wait` might count ticks, `ask` might open a dialogue box.

## How the Host Drives It

Every call the host makes into a script can pause. When it does, the host's call returns `Paused(task, call)` instead of `Done(result)`. `call` says which pausing function was called and with what arguments. The host keeps the task and later resumes it with that function's result:

```rask
match try vm.call("on_talk", [npc, player]) {
    raido.Outcome.Done(_) => {}
    raido.Outcome.Paused(task, call) => dialogue.open(task, call)
}
// when the player picks:
try vm.resume(task, raido.Value.string("quest"))
```

A task can pause any number of calls deep; the whole stack pauses. It lives as long as the host keeps it. A hook that loops forever is fine, and that's how a mod keeps behaviour going across ticks.

Pausing isn't allowed where nothing could resume it: inside a callback a built-in is running (a `sort` comparator, a `filter` test) or in a module variable's initializer. Both are errors.

## Transactions

Each stretch of a task, from a call or resume to the next pause or the end, is one transaction ([vm/interop.md](../vm/interop.md#calls-are-transactions)). A pause commits what came before it, so the host can act on it while the task waits. A failure rolls back to the last pause and leaves the task paused there; the host usually drops it.

## Serialization

A paused task is saved with the rest of the VM, frames, registers and all, so it can be resumed after a save and load, on another machine. See [vm/architecture.md](../vm/architecture.md#serialization).
