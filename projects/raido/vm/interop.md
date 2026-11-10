<!-- id: raido.interop -->
<!-- status: proposed -->
<!-- summary: The host API in Rask: compiling, bindings, calls, pausing, roots, snapshots; the contract every implementation exposes -->
<!-- depends: raido/vm/architecture.md, raido/language/types.md -->

# Host API

How a Rask program runs Raido. The signatures in [The Contract](#the-contract) are exact: every implementation exposes them, under a package named `raido`, so one set of conformance claims checks any of them ([../conformance/](../conformance/)). All of it is safe Rask; no `unsafe`.

## A Host in One Page

```rask
import raido

// The published API module, compiled once
let api = try raido.compile_api("game_api.rd", api_source)

// Bindings: a handler for each immediate extern, a getter and setter per field
mut bindings = raido.Bindings.new(api)
bindings.getter("Enemy", "health", |id| raido.Value.from_int(enemies[id].health))
bindings.setter("Enemy", "health", |id, v| { enemies[id].health = v.as_int()! })
bindings.immediate("noise", |args| raido.Value.from_float(noise(args[0].as_float()!, args[1].as_int()!, args[2].as_int()!)))

// A script, compiled against the API
let chunk = try raido.compile("guard.rd", source, api, Vec.new(), raido.CompileOpts.default())

mut vm = raido.Vm.new(raido.Config.default(), bindings)
vm.seed(12345)
try vm.load(chunk, Vec.new())           // checks bindings, runs top-level `let`s as one run

let guard = try vm.extern_value("Enemy", 7)
match try vm.call("on_guard_spawned", [guard]) {
    raido.Outcome.Done(_) => {}
    raido.Outcome.Paused(task, call) => schedule(task, call)   // call.name == "wait"
}
for c in vm.deferred() { apply(c) }      // move_to, attack, ... in call order
for t in vm.spawned() { schedule_new(t) }
for line in vm.printed() { log.info("raido: {line}") }

// later, when the wait is over
match try vm.resume(task, raido.Value.none()) { ... }
```

## The Contract

```rask
// --- Compiling ---

public func compile_api(name: string, source: string) -> Api or CompileError {}

// `library` holds the chunks this one imports, and theirs. The compiled chunk
// names each by hash.
public func compile(name: string, source: string, api: Api, library: Vec<Chunk>, opts: CompileOpts) -> Chunk or CompileError {}

public enum Profile { Normal, Pure, Provable }

public struct CompileOpts {
    public debug_info: bool
    public profile: Profile
}

extend CompileOpts {
    public func default() -> CompileOpts {}          // no debug info, Normal
}

public struct CompileError {
    public file: string
    public line: i64
    public text: string
}

extend CompileError {
    public func message(self) -> string {}
}

extend Api {
    public func hash(self) -> Vec<u8> {}             // 32 bytes
}

extend Chunk {
    public func decode(data: Vec<u8>) -> Chunk or DecodeError {}   // verifies the bytecode
    public func encode(self) -> Vec<u8> {}
    public func hash(self) -> Vec<u8> {}             // 32 bytes, the content identity
    public func profile(self) -> Profile {}
    public func exports(self) -> Vec<Export> {}
}

public struct Export {
    public name: string
    public params: Vec<string>                       // Raido type syntax: "int", "Vec2", "Enemy?"
    public result: string                            // "void" for none
}

public struct DecodeError {
    public text: string
}

extend DecodeError {
    public func message(self) -> string {}
}

// --- Bindings ---
// Built from the API module, which already says every type. The host only
// supplies handlers. Deferred and pausing externs need none: the VM hands
// their calls back.

extend Bindings {
    public func new(api: Api) -> Bindings {}
    public func getter(mutate self, extern_struct: string, field: string, get: func(i64) -> Value) {}
    public func setter(mutate self, extern_struct: string, field: string, set: func(i64, Value) -> void or HostFailure) {}
    public func immediate(mutate self, name: string, handler: func(Vec<Value>) -> Value or HostFailure) {}
    public func undoable(mutate self, name: string, handler: func(Vec<Value>) -> Value or HostFailure, undo: func(Vec<Value>, Value)) {}
}

public struct HostFailure {
    public text: string
}

extend HostFailure {
    public func message(self) -> string {}
}

// --- Values ---
// 16 bytes, Copy. A heap value is only meaningful to the VM it came from.

extend Value {
    public func none() -> Value {}
    public func from_bool(b: bool) -> Value {}
    public func from_int(n: i64) -> Value {}
    public func from_float(x: f64) -> Value {}       // a NaN or infinity is refused at the boundary
    public func as_bool(self) -> bool? {}
    public func as_int(self) -> i64? {}
    public func as_float(self) -> f64? {}
    public func is_none(self) -> bool {}
    public func extern_id(self) -> i64? {}
}

// --- The VM ---

public struct Config {
    public memory_limit: i64
    public fuel: i64
    public max_call_depth: i64
    public profile: Profile
}

extend Config {
    public func default() -> Config {}               // 256 KB, 100_000 fuel, depth 256, Normal
}

public enum Outcome {
    Done(Value)                                      // Value.none() for a function returning nothing
    Paused(Root, HostCall)                           // the task, and the pausing call it's waiting in
}

public struct HostCall {
    public name: string
    public args: Vec<Value>
}

public struct Root {
    public index: i64                                // stable across snapshot and restore
}

extend Vm {
    public func new(config: Config, take bindings: Bindings) -> Vm {}
    public func load(mutate self, chunk: Chunk, library: Vec<Chunk>) -> void or ScriptError {}
    public func seed(mutate self, seed: i64) {}
    public func fuel(self) -> i64 {}
    public func set_fuel(mutate self, fuel: i64) {}

    public func call(mutate self, name: string, args: Vec<Value>) -> Outcome or ScriptError {}
    public func resume(mutate self, task: Root, value: Value) -> Outcome or ScriptError {}

    // Queues, drained in order. Each fills only when a run commits.
    public func deferred(mutate self) -> Vec<HostCall> {}
    public func spawned(mutate self) -> Vec<Root> {}
    public func printed(mutate self) -> Vec<string> {}   // print lines, failed runs included

    // Values the host makes or reads
    public func new_string(mutate self, s: string) -> Value or ScriptError {}
    public func extern_value(self, extern_struct: string, id: i64) -> Value or ScriptError {}
    public func show(self, v: Value) -> string or ScriptError {}   // exactly what `tostring` gives
    public func text(self, v: Value) -> string or ScriptError {}   // a string's contents
    public func len(self, v: Value) -> i64 or ScriptError {}       // string, array or map
    public func item(self, v: Value, i: i64) -> Value or ScriptError {}   // array element
    public func field(self, v: Value, i: i64) -> Value or ScriptError {}  // struct or tuple field

    public func root(mutate self, v: Value) -> Root or ScriptError {}
    public func get(self, r: Root) -> Value or ScriptError {}
    public func unroot(mutate self, r: Root) {}

    public func reset(mutate self) -> void or ScriptError {}
    public func snapshot(self) -> Vec<u8> {}
    public func restore(data: Vec<u8>, chunks: Vec<Chunk>, take bindings: Bindings) -> Vm or SnapshotError {}
}

public struct SnapshotError {
    public text: string
}

extend SnapshotError {
    public func message(self) -> string {}
}

public struct ScriptError {
    public kind: ErrorKind
    public text: string
    public trace: Vec<TraceFrame>
}

extend ScriptError {
    public func message(self) -> string {}
}

public enum ErrorKind {
    DivisionByZero
    IntegerOverflow
    FloatError
    IndexOutOfBounds
    KeyNotFound
    UnwrapNone
    ArgumentError
    Uninitialized
    ModifiedDuringIteration
    PauseNotAllowed
    MemoryExhausted
    FuelExhausted
    CallOverflow
    HostError
    ScriptError
}

extend ErrorKind {
    public func name(self) -> string {}              // "DivisionByZero", as written above
}

public struct TraceFrame {
    public chunk: i64                                // load position
    public function: i64                             // prototype index
    public offset: i64                               // instruction offset
}
```

`ErrorKind` and the trace are part of the determinism contract ([architecture.md](architecture.md#formal-contract)); message text isn't.

Anything not listed here is the implementation's own business. A host that sticks to this contract can swap one implementation for another.

## The Published API

The host publishes its API as one Raido module: `extern` declarations for the structs it exposes and the functions scripts may call, and any ordinary Raido types and functions it wants to give every script (`Vec2`). Every script compiled for that host sees the module already imported. Scripts never write `extern` themselves.

```raido
// game_api.rd -- written by the engine authors
extern struct Enemy {
    health: int
    x: float
    y: float
    readonly name: string
}

struct Vec2 { x: float, y: float }

extern deferred func move_to(entity: Enemy, target: Vec2)
extern func noise(quality: float, id: int, index: int) -> float
extern deferred func send_message(target: string, body: string)
extern pausing func wait(seconds: float)
```

- **One copy of the API.** Adding a field or function to the engine changes the module; scripts don't each carry a stale copy.
- **Checked at load.** `vm.load` fails if the bindings leave out anything the module needs: a getter for every field, a setter for every field that isn't `readonly`, a handler for every immediate extern.
- **Part of identity.** The module's hash is part of every chunk's identity, so a chunk built for an old API refuses to load against a new one instead of misbehaving.

## Extern Structs

Fields hold only `int`, `float`, `bool`, `string`, payload-free enums, other extern structs, or optionals of those ([types.md](../language/types.md#extern-types)). A field holding a struct or a collection would hand the script a value it could change without going through a setter, where rollback can't see it.

An extern struct value is the host's 64-bit id for the object, tagged with the struct's type (`vm.extern_value("Enemy", 7)`). The id is what gets saved, so it has to stay meaningful after a save and load: an entity id or a pool handle, not a pointer.

**Getters and setters are plain field access.** A getter returns the field and changes nothing; a setter stores the value and does nothing else. The VM relies on that to roll a write back: it reads the old value with the getter, stores the new one with the setter, and on failure stores the old ones back, newest first. A setter that wants to do more (play a sound when health drops) belongs in a deferred extern instead.

## Extern Funcs

An extern function is one of three kinds, and the API module says which, because a script can tell them apart:

- **Immediate** (`extern func`): the VM calls the host's handler at the call, and the handler's result is the call's result. It must either change nothing the script could later observe (`noise`, reads), or be registered with `undoable`, whose undo the VM runs if the call fails (`spawn_unit` with a despawn). It sees the script's field writes so far, since those went straight through the setters.
- **Deferred** (`extern deferred func`): returns nothing. Its calls are queued and handed to the host by `vm.deferred()` once the run commits, in call order. A deferred effect isn't visible to the call that made it, and lands after all of that call's field writes. Most world-changing actions are deferred: `move_to`, `attack`, `play_sound`.
- **Pausing** (`extern pausing func`): takes time. Calling it pauses the script, and `vm.call` or `vm.resume` returns `Paused(task, call)` with the function's name and arguments. The host does whatever the function means and later calls `vm.resume(task, result)` ([language/pausing.md](../language/pausing.md)).

A handler, getter, setter or undo can't call back into the VM: a `vm.call` or `vm.resume` while one is running is a `HostError`.

**Values from the host are checked.** Every value that enters the VM is checked against the type the API declares for it before a script sees it: `vm.call` arguments against the export, a handler's result against the extern's return type, a getter's value against the field type, a resume value against the pausing function's return type. A mismatch is a `HostError`. So the script's types hold at the boundary too, whatever the host does.

## Host References

A heap value the host gets back from a call (a struct, array, string or closure) stays alive until the host's next call into the VM starts, and so does one the host makes with `vm.new_string`. Passed into that call, it's in a register from then on. To keep one longer, the host roots it, like Lua's registry:

```rask
let plan = match try vm.call("make_plan", Vec.new()) { raido.Outcome.Done(v) => v, _ => return }
let r = try vm.root(plan)        // stays alive until unrooted
// ... later ticks ...
let again = try vm.get(r)
vm.unroot(r)
```

Roots are what the collector and serialization start from, and they're part of the serialized state, in creation order. A task is a root too. A root's index survives a save and load, which is how a host finds its tasks again.

Whether the host may still use a value is decided by reachability, never by whether the collector got to it: a value passed into a call must be reachable from a root, the last call's result, or something the host made since. Anything else is a `HostError`, collected or not, so the host can't observe the collector. A heap value carries its slot's generation, so a slot reused by a newer object is never mistaken for the old one.

## Modules

A script imports others with `import "name" as alias`. The name is opaque to the VM: the host resolves names to chunks before compiling, and passes them in `library`. The compiled chunk records each import by content hash, and `vm.load` finds them in its `library` argument by hash.

Each chunk is loaded once per VM, keyed by hash: two names that resolve to the same chunk share one instance and one set of module variables. Load order is depth first, imports in the order they're written, each chunk after everything it imports (post-order), so a diamond `A -> B, C -> D` loads `D, B, C, A`. That order is also initializer order and serialization order.

## Loading, Reset and Snapshots

`vm.load` runs every initializer of the chunk and its imports as one run: deferred calls and spawns happen only if all of them succeed, and a failed load leaves the VM as it was.

`vm.reset()` clears the heap, every root (tasks included) and the queues, re-seeds the PRNG with the last seed, and runs the initializers again as one run ([architecture.md](architecture.md#memory-limit)).

`vm.snapshot()` writes the VM's state in canonical form ([architecture.md](architecture.md#serialization)). `Vm.restore(data, chunks, bindings)` rebuilds it, given the chunks it names by hash and fresh bindings. The config comes from the snapshot.

## Calls Are Transactions

Every run of a script, from `vm.call` or `vm.resume` to the next pause or the end, is one transaction: it either commits or changes nothing. A task that pauses three times is four transactions; each pause commits what came before it, so the host can act on it while the task waits.

- **Objects allocated** during the run become unreachable when it fails; the collector reclaims them.
- **Writes to objects that existed before the run** (a struct in a module variable, a map held by a root) go into an undo log. A slot is logged once per run, with the value it had when the run started; a container's first change of shape (push, remove, a new key, sort) saves the whole of it once. Module variable and cell writes are logged the same way. On failure the log is replayed newest first. Objects allocated during the run need no logging; each object records the run it was made in.
- **Tasks.** Resuming a task logs its status and saved frames first. Saved frames are replaced as a whole at each pause, never edited, so rolling back puts the task back where it was paused.
- **The PRNG state** is logged at its first use in the run and restored on failure.
- **Extern field writes** go straight through the setter, after the VM reads the old value with the getter and logs it. On failure the old values are stored back, newest first.
- **Undoable externs** have their undos queued; on failure they run, newest first.
- **Deferred calls and spawned tasks** are queued, and reach `vm.deferred()` and `vm.spawned()` only if the run commits.

A failed run leaves the VM, and a task, exactly as before it started; the host gets the error, logs it, and usually drops the task.

Two things aren't rolled back: fuel spent, and `print`, which is a log (`vm.printed()` includes lines from failed runs). Both are deterministic, so replay still matches.

So a mod that fails halfway through an event never leaves the world half-updated, and the host only has to log the error and carry on.
