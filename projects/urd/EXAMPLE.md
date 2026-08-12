<!-- id: urd.example -->
<!-- status: draft -->
<!-- summary: Worked example -- a todo app as an Urd machine, end to end -->

# Urd by example: a todo app

The smallest program that exercises the whole surface: ops with arguments, rejection, recorded effects, reads, forking, and both kinds of machine upgrade. Written before any code exists, to pressure-test the API. Friction found while writing is collected at the [end](#what-writing-this-surfaced).

## The machine

`todo.rd` — a Raido script. Mutators and reads are registered with `@op`/`@read` annotations — the annotation's string is the stable wire name, decoupled from the function name so refactors don't break clients. State goes in, state comes out; rejection is an ordinary `error()`.

```raido
struct Todo {
    id: int
    title: string
    done: bool
    created_at: int      // ms, from fx -- the VM has no clock
}

struct State {
    items: array<Todo>
}

// Urd fills this in at append time. The only door effects enter through.
struct Fx {
    seq: int             // this op's log position -- doubles as an id source
    time: int            // server clock at append, ms
}

func init() -> State {
    return State { items: [] }
}

@op("add")
func add_todo(s: State, title: string, fx: Fx) -> State or string {
    if title.is_empty(): error("title can't be empty")
    s.items.push(Todo { id: fx.seq, title: title, done: false, created_at: fx.time })
    return s
}

@op("toggle")
func toggle_todo(s: State, id: int, fx: Fx) -> State or string {
    for item in s.items {
        if item.id == id {
            item.done = !item.done
            return s
        }
    }
    error("no todo with id {id}")
}

@op("clear_done")
func clear_done(s: State, fx: Fx) -> State or string {
    s.items = s.items.filter(|t| !t.done)
    return s
}

@read("active")
func active_todos(s: State) -> array<Todo> {
    return s.items.filter(|t| !t.done)
}

@read("count")
func todo_count(s: State) -> int {
    return s.items.len()
}
```

Note what's absent: no persistence, no networking, no ids invented inside the machine, no clock. `fx.seq` and `fx.time` are recorded in the log entry, so replay sees exactly what the original run saw.

## Running it

```
$ urd init todo.urd --machine todo.rd
$ urd serve todo.urd --listen :4444        # or embed the library
```

A client appends ops; a rejected op (empty title, unknown id) returns the error and never enters the log.

```
$ urd append todo.urd main add '{"title": "buy milk"}'
seq 1  state a3f2c9…
$ urd append todo.urd main add '{"title": "write spec"}'
seq 2  state 77b01d…
$ urd append todo.urd main toggle '{"id": 1}'
seq 3  state c04e11…
```

The log now reads like a git history of state:

```
$ urd log todo.urd main
seq 3  c04e11…  toggle {id: 1}            machine 9d41aa…
seq 2  77b01d…  add {title: "write spec"}
seq 1  a3f2c9…  add {title: "buy milk"}
```

History is data:

```
$ urd fork todo.urd main@2 experiment      # branch before the toggle
$ urd diff todo.urd main experiment        # structural diff of the two states
$ urd verify todo.urd                      # re-run everything, check every hash
```

## A client (TypeScript, hand-written)

```ts
const db = await urd.connect("ws://localhost:4444/todo");
const main = db.branch("main");

main.subscribe((state) => render(state));          // hello -> snapshot, then op-stream
await main.append("add", { title: "buy milk" });   // rejection -> thrown error
```

The client speaks the three-message protocol and holds a local replica for instant reads. It does not run the machine in v1 — writes go to the server (README: server-authoritative).

## Evolution, episode 1: compatible upgrade

Add a `@read("stats")` function and rename `add_todo` to `create_todo` — the wire name `add` is pinned by the annotation, so clients don't notice. State shape unchanged.

```
$ urd upgrade todo.urd todo.rd
replaying 3 ops against machine 5c77e0… : 3/3 state hashes match
machine ref advanced 9d41aa… -> 5c77e0…
```

Proven behavior-identical, no ceremony. New entries pin the new hash; old entries keep theirs.

## Evolution, episode 2: migration

Add a `due: int?` field to `Todo`. Replay with the new bytecode now produces different state encodings — the compatible path refuses. So the new machine exports a migration:

```raido
func migrate(old: OldState) -> State {
    mut items: array<Todo> = []
    for t in old.items {
        items.push(Todo { id: t.id, title: t.title, done: t.done,
                          created_at: t.created_at, due: None })
    }
    return State { items: items }
}
```

```
$ urd upgrade todo.urd todo.rd
state shape changed -- compatible path refused
migration found: migrate(OldState) -> State
seq 4  b91f37…  MIGRATE 9d41aa… -> 21c50c…
```

The migration is a log entry like any other: replayable, hash-checked, pinned to both machine versions. History before seq 4 replays with the old bytecode; after, with the new.

## What writing this surfaced

Friction found by writing the example — this is the point of the exercise:

1. **Clients need the op schema.** `add` takes `{title: string}` — the TS client has to know that. Raido chunks already carry typed exports, so `urd schema todo.urd` can emit the op/read signatures mechanically. A typed-client generator can sit on top as a separate tool. Without this, every client hand-mirrors the machine and drifts.
2. **Rejection-as-error settles the open question for v1 — and it's the reversible choice.** A mutator's `error()` means the op is refused and never appended — no tombstones in the chain. Recording rejections later is purely additive (a side-log; the hash chain never changes), whereas starting with in-chain rejections could never be undone. Recorded rejections only become interesting with offline queues (v1.5). Revisit then, not now.
3. **State-in/state-out has a copy question.** Functional signatures read well and suit verification, but naive copy-per-op is quadratic pain on big states. The VM arena needs either in-place mutation with snapshot-on-demand or structural sharing. This is the biggest open implementation risk found here — filed as an open question in [RESEARCH.md](RESEARCH.md).
4. **The `Fx` struct is the entire determinism boundary** and deserves its own spec rule: what's in it (seq, time, later: append-time randomness seed), what can never be in it (anything the server can't record).
5. **`urd append` from the CLI wants JSON args**, which means op arguments need a canonical JSON encoding anyway — the same encoding the wire protocol and the log can use. One encoding, three consumers.
6. **`@op`/`@read` registration needs a small Raido addition**: attributes on exported functions, carried in the chunk format. Worth it — the annotation is a fixed registry (unregistered functions aren't reachable, a typo'd name is a load error, and the wire name survives refactors, which keeps renames on the compatible-upgrade path). Fallback if Raido shouldn't grow attributes: a manifest constant mapping wire names to export names, validated against the export table at load. Same guarantees, two sources of truth.
