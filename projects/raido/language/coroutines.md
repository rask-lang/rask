<!-- id: raido.coroutines -->
<!-- status: proposed -->
<!-- summary: Cooperative multitasking -- host calls that yield become tasks; scripts make their own coroutines; one yield type per host -->
<!-- depends: raido/language/syntax.md -->

# Coroutines

Cooperative multitasking. A function pauses at `yield` and carries on from there when something resumes it. A paused function is saved with the rest of the VM, so it can be resumed after a save and load, on another machine.

## Hooks Are Tasks

Every call the host makes into a script can yield. When it does, the host's call returns `Yielded(task, value)` instead of `Done(result)`, and the paused call is a **task**: the host keeps the handle and resumes it later with `vm.resume(task, value)`. So a hook can run for as long as it likes:

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
```

`wait` isn't built in. The host's API module defines it on top of `yield` (below), and the host reads the yielded value to decide when to resume. A yield can happen any number of calls deep; the whole call stack pauses, not just the innermost function.

The host owns the task handle. A task the host drops is never resumed, and the collector reclaims it. Each run of a task, from a call or resume to the next yield or the end, is one transaction ([vm/interop.md](../vm/interop.md#calls-are-transactions)).

## The Yield Type

What `yield` passes to the host, and what the host passes back, is declared once in the API module, written like a function:

```raido
// game_api.rd
enum Wait { Seconds(float), NextTick, Event(string) }
extern yield(Wait) -> void

func wait(seconds: float) { yield Wait.Seconds(seconds) }
func next_tick() { yield Wait.NextTick }
```

`yield` then type-checks like a call to that function: `yield Wait.NextTick` is fine, `yield 3` is a compile error, and `let x = yield w` gets the declared result type. A host that declares no yield type gets `extern yield() -> void`, so a bare `yield` pauses and returns nothing.

One type for the whole program means a function that calls `wait` needs no annotation, and neither does anything that calls it: any function may yield. A dialogue host might declare `extern yield(string) -> string`, so a script shows a line and gets the player's choice back:

```raido
func conversation(npc: Unit) {
    let choice = yield "Hello, traveler."
    match choice {
        "quest" => start_quest(npc),
        _ => yield "Safe travels.",
    }
}
```

## Coroutines a Script Makes

A script can make its own paused function with `coroutine(f, args...)`, or `coroutine(|| ...)`. The value has type `coroutine`, and the script drives it:

```raido
let co = coroutine(patrol, guard, route)

if co.resume() is Some(w) {      // ran until it yielded `w`
    ...
}
```

- `co.resume(v)` runs the coroutine until it yields or finishes. It returns `Some(y)` with the yielded value, or `None` when the function finished. `v` becomes the value of the `yield` it was paused at; the first `resume` starts the function and ignores `v`. With the default yield type, `resume()` takes nothing and returns `bool`: `true` if it yielded.
- `co.done` is `true` once the function has finished. Resuming a finished coroutine is a `CoroutineDead` error.
- A `yield` goes to whoever resumed: inside `co`, it goes back to the script's `resume`, not to the host.

A coroutine lives as long as something refers to it. One made in a hook and not stored anywhere is garbage when the hook returns. To keep one going across ticks, put it in a module variable and resume it from a tick hook:

```raido
let spawner: coroutine? = None

func on_wave_start(n: int) { spawner = Some(coroutine(spawn_wave, n)) }

func on_tick() {
    if spawner is Some(co) {
        if co.resume() is None { spawner = None }   // finished
    }
}
```

Most mods don't need this: a hook that loops and waits is already a task the host keeps.

## Failure

A failure inside a coroutine aborts the host's call it ran in, like any other error, and the call's changes roll back, including what happened to the coroutine: it's left paused where it was before the call ([vm/interop.md](../vm/interop.md#calls-are-transactions)). A task whose run fails is left paused at its last `yield`; the host usually drops it.

## Serialization

Coroutines and tasks are heap objects, saved with their frames, registers and program counters. See [vm/architecture.md](../vm/architecture.md#serialization).
