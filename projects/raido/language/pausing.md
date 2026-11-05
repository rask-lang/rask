<!-- id: raido.pausing -->
<!-- status: proposed -->
<!-- summary: A hook pauses by calling a host function that takes time; the host resumes it with the result -->
<!-- depends: raido/language/syntax.md -->

# Pausing

A hook can call a host function that takes time, like `wait(2.0)` or `ask(player, "Trade?")`, and carry on from the next line when the host is ready. It reads like any other call:

```raido
func on_guard_spawned(g: Unit) {
    let route = patrol_route(g)
    let next = 0
    loop {
        if nearest_enemy(g, 8.0) is Some(e) {
            attack(g, e)
        } else if near(g, route[next]) {
            next = (next + 1) % len(route)
        } else {
            move_toward(g, route[next])
        }
        wait(0.1)
    }
}

func on_talk(npc: Unit, player: Unit) {
    let choice = ask(player, "Hello, traveler. Quest or trade?")
    if choice == "quest" { start_quest(npc, player) }
}
```

There's no `yield`, no coroutine object and no `async`. A function that pauses looks like one that doesn't, and so does everything that calls it.

A hook that loops has to pause on every path round the loop. The guard above waits at the bottom of each pass whatever it did; one that only waited while walking would spin at its post until the fuel ran out.

I left out script-driven coroutines and generators on purpose. They'd buy lazy sequences and a script running its own schedule. Mod data is small enough to return an array, a callback covers "visit everything", and the host already interleaves tasks. What they'd cost is a concept, typing rules for what goes in and out, and a hard question about what `wait` means inside a generator that a `for` loop is draining.

## Where Pausing Comes From

The host's API module marks the functions that take time as `pausing`:

```raido
// game_api.rd
extern pausing func wait(seconds: float)
extern pausing func ask(player: Unit, prompt: string) -> string
extern deferred func attack(attacker: Unit, target: Unit)
extern deferred func move_toward(u: Unit, p: Vec2)
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

Pausing isn't allowed where nothing could resume it: inside a callback a built-in is running (a `sort` comparator, a `filter` test) or in a module variable's initializer. Both are errors. `spawn` is allowed anywhere, since it doesn't pause.

## Starting a Task

A hook that wants something to run alongside it, and to return straight away itself, calls `spawn`:

```raido
func on_wave_start(n: int) {
    for i in 0..n {
        spawn(guard_patrol, make_guard(i))
    }
}

func guard_patrol(g: Unit) {
    loop {
        move_toward(g, next_waypoint(g))
        wait(0.1)
    }
}
```

`spawn(f, args...)` takes a function or closure and its arguments, checked against its signature, and makes a new task that will run `f(args...)`. It returns nothing: a script gets no handle, so it can't wait on, cancel or talk to a task. Tasks share state through module variables and host entities like any other code.

The new task doesn't run yet. The host gets it from `vm.spawned()`, which hands over the tasks spawned since it last asked, in the order they were spawned, and starts each one with `vm.resume(task, none)` whenever it likes. From then on it's a task like any other.

A spawn is an effect of the run it happens in, so it's deferred like a deferred extern: the task exists only if that run commits. A hook that spawns three guards and then fails spawns none.

## Transactions

Each stretch of a task, from a call or resume to the next pause or the end, is one transaction ([vm/interop.md](../vm/interop.md#calls-are-transactions)). A pause commits what came before it, so the host can act on it while the task waits. A failure rolls back to the last pause and leaves the task paused there; the host usually drops it.

## Serialization

A paused task is saved with the rest of the VM, frames, registers and all, so it can be resumed after a save and load, on another machine. See [vm/architecture.md](../vm/architecture.md#serialization).
