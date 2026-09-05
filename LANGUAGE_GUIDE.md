# Rask Language Card

The whole language, compressed and normative. This is the reference that travels with a prompt when a model writes Rask, and the lookup page when a human forgets a rule. Every claim matches the specs; citations like `mem.borrowing/W2` point at the deep file (see [specs/CONVENTIONS.md](specs/CONVENTIONS.md)). If this card and a spec disagree, the spec wins — file the discrepancy.

Rask is a compiled systems language: value semantics, single ownership, no GC, no lifetime annotations, no function coloring. Safety mechanisms are visible in source (`with`, `mutate`, `take`, `ensure`) and failures are compile errors or deterministic panics — never UB. Direction: [NORTH_STAR.md](NORTH_STAR.md).

## Syntax core

<!-- test: parse -->
```rask
func add(a: i32, b: i32) -> i32 {
    return a + b             // functions need explicit return
}

func main() {
    let x = 42               // permanent binding, deep: no reassign, no mutation through it
    mut counter = 0          // rebindable binding (never "let mut" — one keyword each)
    counter = counter + 1

    let sign = if x > 0: "+" else: "-"    // inline block: `: expr`
    let label = match x {
        0 => "zero",
        n if n > 0 => "positive",         // guard arms use => like all arms
        _ => "negative",
    }
}
```

- Newlines terminate statements; `;` only for multiple per line. Braces for multi-statement blocks, `: expr` for single-expression blocks.
- Blocks in expression context produce their last expression; functions always `return` (`ctrl.flow`).
- Static access uses `.`: `Token.Plus`, `Point.origin()`, `Vec.new()` — there is no `::`.
- Loops: `for x in xs`, `for i in 0..n` (half-open) / `0..=n` (inclusive), `while cond`, `loop` (only `loop` supports `break value`). Labels: `outer: loop { break outer }`.
- Comments `//`. String interpolation `"{x}"`, debug format `"{x:debug}"`.
- `void` is the unit type (not `()`); `none` is the absent value.

## Ownership and values

Everything is a value with exactly one owner (`mem.ownership`). Assignment copies types ≤16 bytes whose fields are all Copy; larger types **move**, and the source binding becomes invalid. The threshold is fixed.

<!-- test: parse -->
```rask
let names = Vec.from(["a", "b"])
let other = names            // moves — names is now invalid
let backup = other.clone()   // explicit copy; allocation is visible

func show(v: Vec<i32>) { }         // borrow (default): read-only, caller keeps it
func grow(mutate v: Vec<i32>) { }  // exclusive mutable access, caller keeps it
func eat(take v: Vec<i32>) { }     // ownership transfer
eat(own v)                         // caller marks the transfer with `own`
```

- `string` is Copy (16 bytes, immutable, refcounted) — pass it freely, never `.clone()` it.
- Handles (12 bytes) are Copy. Explicit `.clone()` for everything bigger is deliberate design — don't work around it (`mem.value-semantics`).
- `discard x` drops early and invalidates the binding. `@unique` forces move-only.

## Borrowing and access

No references can be stored in structs, returned, or sent cross-task — this is the keystone rule; it's why there are no lifetimes (`mem.borrowing`). What you get instead depends on one question: **can the source change size?**

- **Fixed layout** (struct fields, arrays): views last until end of block.
- **Growable** (Vec, Map, Pool, string): access is per-expression, or a `with` block for multiple statements.

<!-- test: parse -->
```rask
let hp = pool[h].health          // Copy types: copy out
pool[h].health -= damage           // in-place expression access
let e = vec[i]                   // ERROR if element isn't Copy — use with or .clone()

with pool[h] as entity {           // multi-statement access; writes back at block exit
    entity.health -= damage
    if entity.health <= 0 { entity.status = Status.Dead }
    try log_hit(entity.id)         // return/try/break/continue work — real block, not a closure
}
with pool[h] as e: e.health -= 10  // one-liner form
```

- Inside `with` on Vec/Map: no structural mutation (push/insert/remove/clear) — compile error (`mem.borrowing/W2`). Pool allows `insert` and `remove(other)`; removing the bound handle is a compile error (W2a–W2d).
- Aliasing: many readers XOR one mutator, per field — disjoint fields of the same struct can be borrowed simultaneously (F2).
- String slices `s[i..j]` are expression-scoped views; store `s[i..j].to_string()` or `Span` indices instead.
- All checks are function-local. Errors point at the function you're editing.

## Pools, handles, context clauses

Graphs, trees, entity systems — anything that needs stored identity — use `Pool<T>` + `Handle<T>` (a pool_id/index/generation triple, like a database primary key) instead of pointers (`mem.pools`).

<!-- test: parse -->
```rask
import memory.{Pool, Handle}

struct Node { parent: Handle<Node>, children: Vec<Handle<Node>> }  // handles, not pointers

func spawn_and_hurt() {
    mut entities = Pool.new()
    let h = entities.insert(Entity { health: 100 })  // panics on alloc failure (no try)
    entities[h].health -= 10                         // validated: stale handle = panic
    entities.get(h)                                  // T? — non-panicking (Copy types)
    entities.remove(h)
}
```

`using` threads a pool as a hidden parameter (`mem.context-clauses`):

<!-- test: parse -->
```rask
import memory.{Pool, Handle}

func damage(h: Handle<Player>, n: i32) using Pool<Player> {
    h.health -= n                     // auto-resolves through the context pool
}
func kill(h: Handle<Player>) using players: Pool<Player> {
    players.remove(h)                 // named form for structural ops
}
// callers just call damage(h, 5) — compiler finds the pool in scope; two same-typed pools = error
```

Public functions must declare their `using` clauses; `frozen` marks read-only contexts.

## Collections and iteration

`Vec<T>`, `Map<K,V>` and `Set<T>` (`std.collections`). Growth ops (`push`, `insert`) **panic** on allocation failure; `try_push`/`try_insert` return the rejected value for OOM-aware code. `Set` is `insert`/`contains`/`remove`/`len`/`is_empty`/`clear`/`to_vec`, where `insert` and `remove` answer `bool` — whether the set changed. Membership is `contains`, not `contains_key`: a set has no keys. **Map iteration order is unspecified and seeded per process — never depend on it**; sort explicitly (`determinism/D7`).

There are no stored iterator objects. Collection methods return `Sequence<T>` — a push-based protocol where the source drives the loop and hands you each item (`type.sequence`). A chain is lazy: building it runs nothing, and the work happens at the terminal in a single pass.

**There is no `collect()`** — the terminal says what it builds: `to_vec()`, `to_map()` for a sequence of pairs, `join(sep)` for a string (`type.sequence/SEQ31`):

<!-- test: parse -->
```rask
let active = users.iter().filter(|u| u.active).map(|u| u.name).to_vec()

for x in vec { }               // borrowed elements
for mutate x in vec { }        // in-place mutation
for n in rack.nodes() { }      // racks yield links; write through them directly
```

A chain can be split across statements inside one block — `let s = v.iter().filter(p)` is fine, and `s.to_vec()` later in the same block is fine. What it can't do is outlive what it borrows: a sequence over `v` can't be returned, stored in a struct, or sent to another task (`type.sequence/SEQ26`), exactly like any closure that captures a borrow.

**Lazy, not resumable.** There is no `next()`, no `peek`, no `zip`. A push source keeps its position on the call stack, so it can't hand one back or hold two at once — anything needing that uses indices (`type.sequence/SEQ38`, `SEQ39`).

`Vec<Linear>` is illegal — lists of must-consume values go in `Rack<T>` or are consumed via `take_all()`.

## Strings

`string`: UTF-8, immutable, refcounted, Copy (`std.strings`). Interpolation is the one way to combine strings — no `+`, no concat function. `StringBuilder` for loops (`push`, `push_char`, zero-copy `build()`), `join` for lists. `s[i..j]` slices are expression-scoped; `.to_string()` copies out.

## Method surface

The names below are the ones that exist. Guessing from Rust or Python is the
most common way to lose a first attempt here — `to_lower`, `collect`, `find` on
a string and `entries` on a map are all things that look right and aren't.

**`string`** — `len` `is_empty` `contains` `starts_with` `ends_with` `trim`
`trim_start` `trim_end` `to_uppercase` `to_lowercase` `replace` `replacen`
`repeat` `reverse` `substring` `char_count` `is_ascii` `to_string`
`split` `split_whitespace` `lines` `chars` `char_indices` `bytes`
`parse_int` `parse_float` `parse`
`index_of` / `last_index_of` (→ `u64?`, not a plain index) · `char_at` (→ `char?`)

**`Vec<T>`** — `push` `pop` `len` `is_empty` `get` `first` `last` `contains`
`insert` `remove` `clear` `reverse` `swap` `sort` `sort_by` `chunks` `clone`
`join`

**`Map<K,V>`** — `insert` `get` `remove` `keys` `values` `len` `is_empty`
`clear` `iter` `clone`, and **`contains_key`** for presence — `Vec` and `string`
say `contains`, a map does not. `m[k] = v` inserts or replaces; `m[k]` reads and
panics when the key is absent, so test with `contains_key` or use `get`.

**Sequence adapters** (off `.iter()`, or a `string` splitter) — `map` `filter`
`take` `skip` `take_while` `skip_while` `chain` `enumerate` `flatten`
`flat_map`. No `zip` (needs two positions), no `sort` (sorting a lazy chain
means buffering it — `to_vec()` then `sort()`), and it's `take`, not `limit`.
**Terminals** — `to_vec` `to_map` `join` `fold` `reduce` `sum` `product`
`count` `min` `max` `min_by` `max_by` `min_by_key` `max_by_key` `any` `all`
`find` `for_each`. A chain ends in one of these or in a `for` loop.

Adapters live on `Sequence`, not on the collection: `v.map(f)` doesn't exist,
`v.iter().map(f).to_vec()` does. The old `Vec.map` allocated a second Vec;
the chain allocates once, at the terminal (`type.sequence/SEQ41`).

`.len()` answers `u64` everywhere. `as` won't narrow it to `i64` — that's
`.to<i64>()!`, or keep the count unsigned.

## Types

<!-- test: parse -->
```rask
struct Point { x: f64, y: f64 }              // value type; fields package-visible by default
struct User {
    public name: string                      // exported
    email: string                            // package-visible (default)
    private hash: u64                        // extend blocks only
    retries: i32 = 3                         // declared field default
}
let u = User { name: "bo", email: "e", hash: h() }   // retries filled from default
// All fields defaulted → `Config {}` is the empty construction. There is NO Default trait.

extend Point {                               // methods live in extend blocks
    func length(self) -> f64 { return (self.x * self.x + self.y * self.y).sqrt() }
    func scale(mutate self, k: f64) { self.x *= k; self.y *= k }
    func origin() -> Point { return Point { x: 0.0, y: 0.0 } }   // static: Point.origin()
}

enum Shape {                                 // tagged union; exhaustive match required
    Circle(radius: f64)
    Rect { width: f64, height: f64 }
    Dot
}

type UserId = u64                            // NOMINAL — a distinct type, not an alias
type TaskId = u64 with (Equal, Hashable)     // …and it inherits no traits; opt in with `with`
type alias Bytes = Vec<u8>                   // transparent alias
```

- Tuples: `(a, b)`, arity ≥ 2. Unions `A | B` appear only in error position.
- Conversions: `as` is lossless only — including int→float, so `i64 as f32` is an error. Anything that can lose something names a policy: `x.to<u8>()!` (the common one — exact or it fails, `!` panics), `x.wrap<u8>()` (low bits), `x.clamp<u8>()` (pin to range) — both integers-only — and `x.round<T>()`. `to` yields `T or ConvertError`, so `try` and `catch` work on it too. Float→int is `to` (no fraction allowed), `round`, `floor` and `ceil` — all fallible, no toward-zero mode (`type.primitives`).
- Integer overflow **panics in all builds**. Opt out per-value with `Wrapping<T>`/`Saturating<T>` from `num` (`type.integer-overflow`).
- Floats are not Hashable/Comparable — structs containing `f64` can't be Map keys or `sort()`ed without a custom conformance (HA4/CO4).

## Optionals and errors

`T?` is sugar for `T or none`; `T or E` is the builtin result sum. **There are no `Ok`/`Err`/`Some`/`None` wrappers anywhere.** Branches are picked by type (`type.errors`, `type.optionals`).

<!-- test: parse -->
```rask
import fs
import io.IoError

func find(id: i32) -> User? {
    if id == 0 { return none }
    return load(id)                          // bare value widens for optionals
}

func read_config(path: string) -> Config or (IoError | ParseError) {
    let text = try fs.read_text(path)     // try: extract or propagate (prefix, not `?` suffix)
    return try parse(text)                   // error unions compose with |
}
```

Operator surface — three words, three jobs: `try` means something **leaves**, a `?` means something is **missing**, `catch` means something **failed**:

| Form | Meaning |
|---|---|
| `x?` | `bool` — present test on an optional. A plain boolean; touching the payload is the bind below |
| `x? as v` | test and bind `v` — works on any scrutinee, no restrictions |
| `x?.field` | optional chain — projects when present, `none` otherwise. Optionals only |
| `x ?? <expr>` | **absence fallback:** the payload, or the right side — a value, or any written-out exit (`?? return E`, `?? break`) |
| `r catch e => <expr>` | **failure fallback:** the payload, or the body with the error bound — a value, or a written-out exit |
| `r catch _ => <expr>` | same, dropping the error *visibly*. There is no bare-value `catch` — an error never dies silently |
| `try x` | **leaves:** propagate the bad branch to the caller (error widened, or `none` in a `T?` fn) |
| `try f() ?? <expr>` | the flat-shape composite (`T? or E` callee): error up, absence here |
| `r!` / `r! "msg"` | extract or panic |
| `if r is IoError as e { }` | error-side type test and bind |
| `let v = x is Pattern else { return }` | pattern guard, enum patterns only |

**Every exit is written where it happens.** Bare `try` leaves to the caller — one keyword, one meaning. Everything else that leaves does so with a visible `return`/`break`/`continue`/`panic` on the fallback's right side. And a *discarded error* is always visible too: `catch _ =>` is the loud, greppable spelling of "an error dies here" — the fallbacks are split per shape precisely because dropping a `none` loses nothing while dropping an `E` loses information.

The error-conversion rules — widening into a union, wrapping into a boundary enum, boxing into `any Error` — are attached to `try`, so bare `try r` does work no other form does. `try` also places itself in a postfix chain: `try read_file(p).len()` needs no parens (`type.errors/ER16a`).

**The line shows which shape.** Nothing with a `?` applies to a result — no `r?` test, no `r ?? v`, no `r?.field` chain; failures use `catch`, tests use `is`, and projection is `try r.field` (`try` attaches to the fallible step). `catch` never appears on an optional — absence has nothing to catch. `try`, `!` and `match` work on both.

- Auto-wrap for `T or E` fires **only at `return`**; optionals widen at any position (ER9–ER11).
- Every error type satisfies `Error` (`func message(self) -> string`) — **auto-derived for enums**, overridable. Primitives can't be error types; `void or string` is illegal. `SysError` covers rare platform failures.
- Errors are for what callers can handle; panics are for bugs (bounds, overflow, stale handles). Panics kill the task, run `ensure` blocks, and are deterministic (`ctrl.panic`).

## Traits and generics

Conformance is **nominal — declared, not shape-matched** (`type.generics/G1`):

<!-- test: parse -->
```rask
trait Comparable { func compare(self, other: Self) -> Ordering }

extend Score with Comparable {
    func compare(self, other: Score) -> Ordering { return self.value.compare(other.value) }
}
extend Ring<T> with Countable, Sizable {}    // comma-list; empty block = methods already exist
public extend Point with Displayable { }     // public conformance is declared, never inferred

duck trait Sketchy { func poke(self) }       // opt-in shape-matching — prototyping only;
                                             // harden by deleting `duck` + accepting generated declarations
```

- Auto-derived (no declaration needed): **Equal, Hashable, Comparable, Cloneable** for eligible field types, `Debug` for all types, `Encode`/`Decode` markers, `Error` for enums. Structs and enums only — a nominal newtype (`type TaskId = u64`) inherits nothing from what it wraps and opts in with `with (…)` (`type.aliases/T10`, T11). Overriding `Equal` cancels auto-derived `Hashable`/`Comparable` — redeclare them consistently (OC1).
- Method-name collision between two conformances: mark the second `scoped extend`; call it as `Trait.method(value, args)` (MN3–MN5).
- Generics monomorphize (`func max<T: Comparable>(a: T, b: T) -> T`). Public functions declare bounds; private functions may omit types and bounds entirely — inferred from the body, still fully static (`type.gradual`). Error unions infer too: `-> Config or _`.
- Operators are authored sugar on concrete types: `a + b` calls `a.add(b)` — write the method, get the operator. Arithmetic operators require Copy types (no allocating `+`); `+=` has no such limit. Generic operator use goes through nominal bounds (OP1). There is no `From`/`Into` — `try` widens error unions structurally, and there's one string type.
- Runtime polymorphism: `any Trait` boxes the value (heap allocation + vtable). Conversion is always explicit — `button as any Widget` — including in collections and arguments (TR5). Methods returning `Self` or generic methods can't be called through `any`.

## Resources and cleanup

`@resource` types are linear: must be consumed exactly once — the compiler rejects a path where a file might not close (`mem.resource-types`, `mem.linear`).

<!-- test: parse -->
```rask
func process(path: string) -> Data or Error {
    let file = try fs.open(path)
    ensure file.close()                 // runs on every exit: return, try, panic. LIFO order.
    let text = try file.read_text()
    return parse(text)
}

let tx = try db.begin()
ensure tx.rollback()                    // safety net
try tx.insert(a)
tx.commit()                             // consuming tx cancels the ensure
```

Consumption must be statically definite at every exit — consuming in one branch and merging is a compile error; restructure so the consuming branch exits (`ctrl.ensure/C3–C5`). Channel send counts as consumption.

## Concurrency

Uncolored: no `async`/`await`; I/O pauses the task automatically. Runtimes are opt-in blocks, typically in `main` (`conc.async`):

<!-- test: parse -->
```rask
import net

func main() -> void or Error {
    using Multitasking {                          // green-task runtime; block drains on exit
        let listener = try net.tcp_listen("0.0.0.0:8080")
        loop {
            let conn = try listener.accept()
            spawn(own || { handle(conn) }).detach()  // handles MUST be joined or detached
        }
    }
}
```

- `spawn(|| {})` → green task (needs `using Multitasking`); `ThreadPool.spawn` → CPU work (needs `using ThreadPool`; combine: `using Multitasking, ThreadPool`); `Thread.spawn` → raw OS thread.
- A spawned closure that captures anything needs `own`: `spawn(own || { … })`. The task outlives the block that made it, so captures move in rather than borrow. Two tasks that both need the same `Shared` each get a `.clone()` of it.
- `h.join()` → `T or JoinError`; `h.cancel()` requests cooperative cancellation (tasks poll `cancelled()`; I/O returns `Cancelled`); dropping a handle unconsumed is a compile error.
- Channels transfer ownership: `mut (tx, rx) = Channel<Msg>.buffered(100)`; `try tx.send(m)`, `rx.receive()` (not `recv`), non-blocking `try_send`/`try_receive`.
- `select { rx1 -> v: handle(v), tx <- msg: sent(), _: fallback() }` — random among ready arms; `select_priority` for ordered; `Timer.after(d)` for timeouts.
- Shared state crosses tasks only through sync boxes — there is no shared mutable memory otherwise. One box, `Shared<T>`, with the synchronisation as a strategy (`mem.boxes/BX1`): `Shared<T>` is readers-XOR-writer (the default, built with `Shared.new`), `Shared<T, Mutex>` a plain lock (`Shared.mutex`), `Shared<T, Local>` no lock at all (`Shared.local`). `Cell<T>` and `Mutex<T>` are not types.
- Access is scoped, never a guard you can store: `config.read().timeout` and `queue.write().push(x)` inline, `with config.write() as c { … }` for several statements, `get`/`set` for a Copy value. `return`, `try`, `break` and `continue` all work through a `with` block, which is why it's a block and not a closure. `Atomic*` for counters and flags. Locks release at expression or block end; they cannot leak out.
- Closures capture by value at field granularity; a closure capturing borrowed data can't leave its scope.

## Comptime

Two stages, syntactically marked (`ctrl.comptime`). Comptime code computes constants and decides what runtime code exists; any *pure* function is callable at comptime — no marking needed (CT6).

<!-- test: parse -->
```rask
const PRIMES = comptime {
    mut v = Vec.new()
    for n in 2..100 { if is_prime(n) { v.push(n) } }
    v.freeze()                                    // collections must freeze to cross into runtime
}

func encode<T: Encode>(value: T, mutate w: Writer) -> void or Error {
    comptime for field in reflect.fields<T>() {   // unrolls per field at compile time
        comptime if !field.is_skipped {
            try w.write_key(field.serial_name)
            try encode(value.(field.name), mutate w)  // body is runtime code (residue)
        }
    }
}
```

No I/O (`@embed_file` excepted), no pools/concurrency/`any Trait` at comptime. `comptime if cfg.os == "linux"` for conditional compilation — discarded branches are only syntax-checked. Serialization is built on this plus `@rename`/`@no_serialize`/`@default` field annotations (`std.encoding`) — there are no macros. Auto-derive covers every non-`private` field; `private` means off the wire.

## Modules, build, unsafe

- Package = directory; all files in it share visibility. Default is package-visible; `public` exports; `private` (fields/methods) restricts to `extend` blocks. Struct with any private field → construct via factory function.
- `import http` then `http.get(...)` — qualified by default; `import mylib.{Parser, Lexer}` for unqualified names.
- **Nothing comes pre-imported.** Only primitives, `string`, `Vec`, `Map`, `Set`, `Error`, `Channel` and `none` are in scope on their own. Everything else in the stdlib needs asking for: `import string.StringBuilder`, `import sync.{Shared, Mutex}`, `import memory.{Rack, Link, Pool, Handle, Heap}`, `import fs.File`, `import time.Duration`, `import thread.Thread`. There is no prelude — a name you didn't import is a name the compiler leaves free for you to declare yourself.
- `build.rk` declares the package in Rask syntax (deps, features, profiles) and can contain a `func build(ctx)` script. Capabilities (fs/net/ffi) are inferred from imports and gated by `allow:` (`struct.build`).
- C interop and raw pointers live in `unsafe` blocks only (`mem.unsafe`). Cross-platform: `comptime if cfg.os`.
- Testing: `test "name" { assert x == y }` blocks; `rask test`.

## Common mistakes (especially if you know Rust)

1. **`type X = Y` creates a distinct nominal type.** For a transparent alias write `type alias X = Y` — the opposite of Rust/TypeScript.
2. **No `Ok`/`Err`/`Some`/`None`.** Return bare values or the error value. On results: handle with `catch`, test with `is` (`IoError as e`), never `is Ok` — and never `r?`; `?` is absence-only and doesn't apply to results.
3. **Bindings are `let`/`mut`** — never `let mut`. `mut`, not `let`, is the rebindable one; `const` exists only at module level.
4. **`try` is a prefix keyword**, not a `?` suffix: `let x = try f()`. The `?` suffix means absence, on optionals only — and **`?`-tests don't narrow; there is no flow typing.** `if x? { use(x) }` doesn't unwrap `x`; Kotlin/TypeScript smart-cast instincts fail here. Bind instead (`if x? as v { use(v) }`) or exit with the fallback (`let v = x ?? return`).
5. **Methods live in `extend Point { }` blocks**, not in the struct body; trait conformance is `extend Point with Trait { }` — and it's required (nominal), methods matching by shape is not enough.
6. **Boxing is explicit**: `render(button as any Widget)` — no implicit conversion to `any Trait`, even when the target type is known.
7. **No `&`, `&mut`, lifetimes, or storable references.** Pass values (borrow is the default mode); store `Handle<T>`, indices, or `Span`s instead of references; use `with` for multi-statement element access.
8. **Explicit `return` in functions.** Only block *expressions* (if/match arms, `with`) use last-expression value.
9. **No string `+` or concat** — interpolation `"{a}{b}"`, `StringBuilder`, or `join`. Strings are Copy: never `.clone()` a string, never try to mutate one.
10. **`.clone()` on collections/large types is required and intentional** — don't add reference workarounds; the visible cost is the design.
11. **`|` has three meanings**: closure params `|x| x + 1`, error unions `IoError | ParseError` (error position only), bitwise-or. Match alternatives also use `|`.
12. **Map iteration order is seeded-random** — sort before iterating if order matters. `push`/`insert` panic on OOM; `try_push` for fallible.
13. **Static paths use `.`**: `Shape.Circle(2.0)`, `Vec.new()`, `Token.Plus` — never `::`.
14. **`void` and `none`**, not `()` and `null`: `func f() -> void or Error`, `return none`.
15. **There is no prelude.** Rust hands you `Vec`, `String` and `Option` for free; Rask hands you primitives, `string`, `Vec`, `Map`, `Set`, `Error`, `Channel` and `none`, and nothing else. `StringBuilder`, `Shared`, `Mutex`, `Rack`, `Link`, `Pool`, `Handle`, `File`, `Duration`, `Thread` all need an `import` line. Reaching for one without it is the single easiest way to not compile.

## Spec vs compiler (temporary)

`Error` is auto-derived for enums a signature names as an error type — the `E`
of a `T or E` return. That covers how error types are written, but it is
narrower than ER6, which says every enum. An enum reached as an error only
through inference (a private function with no declared error type, or `or _`)
still needs a hand-written `message()`.

`Timer.after` and `Timer.interval` are declared but have no native entry point,
so a program using either fails at codegen with "Function not found" (#1066).
`os.Command` is in the same state. An `Atomic*` can't be reached from more than
one task yet — `spawn` needs `own`, which moves it, and there's no way to hand a
second task its own handle (#1068), so use `Shared<i64>` for a counter until
that lands.

The spec is normative; the compiler lags it in places. Nominal conformance is now enforced (#283 landed). Still lagging: implicit `any Trait` coercion is still accepted (#284), seeded Map order (#285), and some codegen paths fail on valid programs (see the open codegen issues). Write to the spec; don't infer rules from what today's binary accepts.
