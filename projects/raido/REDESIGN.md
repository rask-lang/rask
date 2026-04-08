<!-- id: raido.redesign -->
<!-- status: proposed -->
<!-- summary: Verification-first redesign — static types, cut closures, keep coroutines -->

# Raido Redesign: Verification-First, Scripting-Capable

Raido started as a Lua derivative for MMO user scripting. The scope shifted. The primary consumers — Allgard (verifiable minting), Apeiron (deterministic physics/combat/crafting), GDL (client-side rendering) — all need deterministic verification. But Raido also has a future as a content scripting language: NPC AI, dialogue, game logic, modding.

The current design carries Lua baggage that hurts verification without meaningfully helping scripting: dynamic typing, closures with shared mutable upvalues, nil as a general value, mutable globals. This document proposes a redesign that serves both use cases.

## Identity

**Verification-first, scripting-capable.**

Static types and pure-function defaults serve verification. Coroutines and function references serve game scripting. Features that help neither get cut.

Two use case families, one VM:

- **Verification** (Allgard minting, Apeiron physics/combat/crafting): pure functions, determinism is load-bearing, content-addressed proofs. A trading partner re-executes the same bytecode with the same inputs and checks the output matches.
- **Content scripting** (NPC AI, dialogue, game logic, GDL client scripts): multi-tick behavior, host entity interaction, sequential logic that yields between ticks.

What they share: determinism, bounded resources, host interop, structured data (structs/enums), content addressing.

## What Gets Cut

| Feature | Rationale |
|---------|-----------|
| **Dynamic typing** | Runtime type errors are a divergence vector between implementations. Structured inputs have known shapes. Static types catch bugs before deployment — critical when scripts are tradeable economic assets. |
| **Closures** | Shared mutable upvalues are a verification hazard. NPC state lives in host entities, not captured variables. Function references and `bind()` cover the composition need without the complexity. |
| **`nil` as a general value** | Replaced by `T?` optionals. Eliminates null-related runtime errors by construction. |
| **`global` keyword** | Mutable global state undermines statelessness between host calls. Script state should live in coroutine locals or host entities. |
| **`host_ref` as distinct concept** | Replaced by `extern struct` — same capability, compile-time type checking. |
| **Triple-quoted strings** | Keep double-quoted (with interpolation) and single-quoted (raw). Triple-quoted adds parser complexity for minimal benefit. |
| **Rest parameters** | No consumer has variadic functions. Fixed arity with typed signatures. |
| **`type()` core function** | Types known at compile time. |
| **`&&`/`\|\|` returning operand values** | With static types, these return `bool`. Use `??` or `match` for defaults. |

## What Gets Added

| Feature | Rationale |
|---------|-----------|
| **Static type system** | Catches errors at compile time. Eliminates runtime type checking. Smaller values (8 bytes vs 16). |
| **Type inference for locals** | Keeps ceremony low. Only function signatures need annotations. |
| **User-defined `struct`** | Structured data is the primary data model for all consumers. |
| **User-defined `enum`** (with payloads) | Combat orders, element types, transform types, animation states. Exhaustive `match`. |
| **`T?` optionals** | Replace nil. Compiler-enforced null safety. |
| **`extern struct`** | Script declares the shape it expects from the host. Type mismatch = load error, not runtime error. |
| **`extern func`** | Host functions with typed signatures. Same load-time checking. |
| **Function references** | References to named top-level functions. No captures, no arena allocation. Enables `coroutine(patrol)`, `sort(by_distance)`, behavior composition. |
| **`bind()`** | Partial application. Freezes arguments — returns a function reference with fewer params. 80% of closure utility, zero verification cost. Deterministic, serializable. |
| **`??` null coalescing** | `value ?? default` — sugar for the common optional-with-default pattern. |
| **Tuples** | Lightweight multi-return without defining a struct. `func bounds(arr: array<number>) -> (number, number)`. |
| **Module imports** | `import "combat_utils"` — content-addressed composition. Import graph is part of chunk identity. Essential for non-trivial scripts. |

## What Stays

| Feature | Why |
|---------|-----|
| **Coroutines** | NPC AI, patrol loops, dialogue trees, multi-tick behavior. Sequential code that yields between ticks is dramatically simpler than manual state machines. Deterministic — same resume sequence = same result. Serializable. |
| **Strings** | Dialogue, identifiers, error messages, content scripting. Double-quoted with interpolation, single-quoted raw. |
| **Maps** (restricted keys: `string`, `int`) | Asset-by-name lookup, entity-by-tag lookup. Structs cover structured data; maps cover ad-hoc association. |
| **`match`** | Exhaustive enum matching (compiler error if variant missing). Nice-to-have for literals. |
| **`try`/`error()`** | Error propagation for host function failures. Errors are exceptional, not normal control flow. |
| **Arena + bump allocator** | Deterministic allocation. No GC. Frame-based lifetime management. |
| **Fuel metering** | Bounded execution. Non-catchable on exhaustion. |
| **PRNG** (xoshiro128++) | Seeded, deterministic, serializable. Part of VM state. |

## Type System

### Primitives

| Type | Description |
|------|-------------|
| `int` | i64. Counters, IDs, indices. |
| `number` | 32.32 fixed-point. Physics, coordinates, economics. |
| `bool` | `true`/`false`. |
| `string` | Immutable UTF-8. Dialogue, identifiers, messages. |

### Composite Types

| Type | Description |
|------|-------------|
| `struct` | User-defined named record. Fixed fields, known at compile time. |
| `enum` | Tagged union with optional payloads. Exhaustive `match`. |
| `array<T>` | Homogeneous growable sequence. 0-indexed. |
| `map<K, V>` | Key-value store. `K` restricted to `string` or `int`. Insertion-ordered. |
| `T?` | Optional. Either `Some(value)` or `None`. Same as Rask. |
| `(T, U, ...)` | Tuple. Lightweight grouping for multi-return. |

### Function Types

`func(int, int) -> bool` — describes the signature of a function reference. No closures. The value is a pointer to a named top-level function (or a `bind()` result with frozen arguments).

### Rules

- Function signatures are fully typed (parameters + return type)
- Local variables inferred from initializer: `const x = 42` → `x` is `int`
- No generics beyond built-in `array<T>`, `map<K, V>`, `T?`, tuples, and function types
- No traits or interfaces
- Exhaustive `match` on enums — compiler error if a variant is missing
- `int` and `number` are separate types — no implicit coercion. Use `number(x)` or `int(x)` for explicit conversion
- `??` unwraps optionals with a default: `value ?? fallback` where both sides must be the same type

### Struct Declaration

```raido
struct Element {
    name: string
    density: number
    hardness: number
    conductivity: number
    reactivity: number
    stability: number
    radiance: number
}

struct FleetState {
    ships: array<Ship>
    formation: Formation
    orders: Orders
}
```

### Enum Declaration

```raido
enum Stance {
    Aggressive
    Defensive
    Evasive
    HoldPosition
}

enum Order {
    Attack(int)                              // positional payload
    Retreat
    HoldPosition
    Allocate { system: int, power: number }  // named payload
}

// Usage — dot access for variants, same as Rask
match order {
    Order.Attack(target) => engage(fleet, target),
    Order.Retreat => disengage(fleet),
    Order.HoldPosition => hold(fleet),
    Order.Allocate { system, power } => reallocate(fleet, system, power),
}
```

### Extern Declarations

Scripts declare the shapes they expect from the host. The compiler checks field access against the declaration. The host binds fields at `vm.load()` — type mismatch is a load error, not a runtime error.

```raido
extern struct Enemy {
    health: int
    x: number
    y: number
    readonly name: string    // compiler rejects writes
}

extern func send_message(target: string, body: string)
extern func noise(quality: number, id: int, index: int) -> number
extern func move_to(entity: Enemy, target: Vec2)
```

### Function References and bind()

Function references point to named top-level functions. No captured state. `bind()` freezes leading arguments — returns a new reference with fewer parameters.

```raido
func apply_damage(amount: int, target: Ship) { ... }
func by_health(a: Ship, b: Ship) -> bool { return a.health < b.health }

// Function reference
const comparator = by_health
ships.sort(comparator)

// Partial application
const hit_hard = bind(apply_damage, 50)
hit_hard(enemy)    // same as apply_damage(50, enemy)

// Coroutine creation
func patrol(npc: Entity, route: array<Vec2>) { ... }
const co = coroutine(patrol, guard, waypoints)
```

`bind()` is deterministic and serializable — the frozen arguments are immutable values stored with the reference.

### Module Imports

Scripts can import other content-addressed chunks:

```raido
import "combat_utils" as combat
import "physics_common" as phys

func resolve_tick(state: FleetState, orders: Orders) -> CombatResult {
    const damage = combat.calculate_damage(state, orders)
    ...
}
```

The import graph is part of the chunk's content hash. Verifying a script means verifying it and all its dependencies. The host resolves import names to chunks — the script doesn't know where the bytecode comes from.

## Syntax Delta

### Unchanged

`if`/`else`, `for`/`while`, `match`, `const`/`let`, arithmetic/comparison/logic operators, `try`/`error()`, `assert()`, double-quoted strings with interpolation, `0..10` and `0..=10` ranges, `//` and `/* */` comments, `break`, `return`, `yield`.

**Adopted from Rask (new to Raido):**

- **Colon inline syntax:** `if x > 0: return x` — single expression after colon, braces for multi-statement
- **`is` pattern matching:** `if opt is Some(v): use(v)` — match a single pattern in conditions
- **`Some`/`None` for optionals** — capitalized variants, same as Rask's `Option` enum

**Deliberate divergences from Rask:**

- **Single-quoted strings** — Rask uses `'a'` for character literals. Raido has no character type; single quotes are raw strings (`'no {interpolation}'`). Different use case, intentional.
- **No `public`/package visibility** — Raido scripts are single-file. All declarations are visible within the script and importable by other chunks.
- **No `extend` blocks (deferred)** — Methods on user-defined structs are deferred. Built-in methods on `array`, `map`, `string` are compiler-known.
- **No parameter modes** — No `mutate`/`take`. Raido values are arena-managed, not ownership-tracked. Structs pass by reference (arena offset), primitives by value.
- **No `using` context clauses** — Host data access is through `extern struct`, not context parameters.

### Changed

```raido
// Function signatures now typed
func damage(weapon: Weapon, target: Ship, beacon: int) -> number {
    const base = weapon.power * weapon.efficiency
    const scatter = noise(weapon.quality, weapon.id, beacon)
    return clamp(base + scatter, 0.0, base * 2.0)
}

// struct and enum declarations (new)
struct Vec2 { x: number, y: number }
enum State { Idle, Patrol, Combat(target: int) }

// extern declarations replace host_ref (new)
extern struct Entity { health: int, x: number, y: number }
extern func spawn(kind: string, pos: Vec2) -> Entity

// Coroutine creation via function reference (changed)
const co = coroutine(patrol, guard, waypoints)

// Tuples for multi-return (new)
func minmax(arr: array<number>) -> (number, number) { ... }
const (lo, hi) = minmax(values)

// Module imports (new)
import "physics" as phys

// Optional handling (changed — no nil, capitalized variants like Rask)
const shield: Shield? = entity.shield
const defense = shield ?? default_shield

// Match on optionals — Some/None capitalized, same as Rask
match shield {
    Some(s) => apply_to(s),
    None => take_full_damage(),
}

// `is` pattern matching — same as Rask
if entity.shield is Some(s): apply_damage(s)

// Safe access (new)
const item = inventory.get(3)      // returns T?
const entry = lookup.get("iron")   // returns V?

// Inline syntax — colon for single expression, same as Rask
if health < 30: return Animation.Hurt
for star in stars: generate_planets(star)

// bind() for partial application (new)
const hit10 = bind(apply_damage, 10)
```

### Removed

- `|x| x * 2` — closure syntax gone
- `global config = {:}` — mutable globals gone
- `nil` — use `None` for optionals (capitalized, like Rask)
- `"""triple-quoted"""` and `'''raw triple'''` — use regular strings
- `func sum(nums...)` — rest parameters gone
- `type(v)` — types known at compile time

## VM Impact

High-level changes. Not a full opcode redesign — that belongs in a revised `vm/architecture.md`.

### Value Representation

**8 bytes per value** (down from 16). With static types, the compiler knows the type at every register position. No runtime type tags needed.

| Type | Payload (8 bytes) |
|------|-------------------|
| `int` | i64 |
| `number` | i64 (32.32 raw bits) |
| `bool` | i64 (0 or 1) |
| `string` | u32 arena offset (padded) |
| `array` | u32 arena offset |
| `map` | u32 arena offset |
| `struct` | u32 arena offset |
| `enum` | u32 discriminant + u32 arena offset (or inline for simple enums) |
| `T?` | u8 tag (0=none, 1=some) + 7 bytes payload |
| `func ref` | u32 prototype index (+ u32 bind offset if bound) |

256 registers × 8 bytes = 2 KB per call frame. Half the current 4 KB.

### Opcode Changes

**Removed (7):**

| Opcode | Why |
|--------|-----|
| `LOAD_NIL` | No nil. |
| `GET_GLOBAL` / `SET_GLOBAL` | No mutable globals. |
| `GET_UPVALUE` / `SET_UPVALUE` / `CLOSE_UPVALUE` | No closures. |
| `CLOSURE` | No closures. |

**Kept from current design:**
All arithmetic, comparison, logic, jump, call, collection, and host field ops. `COROUTINE`, `YIELD`, `RESUME` stay. `TRY` stays.

**Added (~5):**

| Opcode | Purpose |
|--------|---------|
| `NEW_STRUCT A Bx` | Allocate struct of type `Bx` in arena, fields from registers. |
| `GET_STRUCT_FIELD A B C` | `R[A] = R[B].fields[C]`. Field index known at compile time. |
| `SET_STRUCT_FIELD A B C` | `R[A].fields[B] = R[C]`. |
| `ENUM_TAG A B` | `R[A] = discriminant(R[B])`. For `match` dispatch. |
| `FUNC_REF A Bx` | `R[A] = reference to prototype Bx`. For function references and `bind()`. |

**Net: ~35 opcodes** (down from 37). The count reduction is modest. The real win is each opcode being simpler — no runtime type dispatch on arithmetic, no upvalue open/close logic, no type tag checking.

### Arena Changes

- **No per-element type tags** in arrays. An `array<int>` stores raw i64 values. Element type is known from bytecode metadata.
- **Closure and upvalue objects gone.** Coroutines reference prototypes directly.
- **Struct layout is fixed-size**, determined at compile time from the struct declaration.
- **Bind objects** store a prototype index + frozen argument values. Small, fixed layout.
- **Map entries** simpler — keys and values have known types, no tag bytes.

### Compiler Changes

**Two-pass instead of single-pass.**

1. **Declaration pass.** Scan for `struct`, `enum`, `extern struct`, `extern func`, and `func` signatures. Build a type table.
2. **Compile pass.** Recursive descent with type checking. Emit bytecode. Register allocation same as current (linear, locals sequential, temporaries bump).

Still no full AST. The declaration pass records names and types, not structure. Memory is O(declarations), not O(program size).

Type inference for locals uses forward flow: `const x = 42` → x is `int`. No backward inference, no constraint solving. If the initializer's type is known, the local's type is known.

## Host Interop

### Current Design

Host registers vtables by string name. Script accesses fields dynamically. The compiler resolves field names to slot indices, but types aren't checked until runtime.

### New Design

Script declares `extern struct` and `extern func`. The compiler type-checks all access against these declarations. The host binds at `vm.load()` — type mismatch between declaration and binding is a load error.

```rask
// Host side (Rask)
vm.register_extern_struct("Enemy", raido.ExternStruct {
    fields: [
        raido.Field.int("health", get_health, set_health),
        raido.Field.number("x", get_x, set_x),
        raido.Field.number("y", get_y, set_y),
        raido.Field.string("name", get_name, null),  // readonly
    ],
})

vm.register_extern_func("move_to", move_to_handler)

const chunk = try vm.compile("script.raido", source)
try vm.load(chunk)  // fails if extern declarations don't match bindings
```

```raido
// Script side
extern struct Enemy {
    health: int
    x: number
    y: number
    readonly name: string
}

extern func move_to(entity: Enemy, target: Vec2)

func chase(attacker: Enemy, target: Enemy) {
    const dest = Vec2 { x: target.x, y: target.y }
    move_to(attacker, dest)
}
```

The `raido.bind` helper library still works — it generates `register_extern_struct` calls from Rask struct definitions.

### Scoped Bindings

Same pattern as current design. Host borrows data for a scope:

```rask
try vm.with_context(|ctx| {
    ctx.bind("enemies", enemies)
    ctx.call("on_update", [raido.Value.number(dt)])
})
```

### Serialization

Same as current: captures registers, call frames, coroutines, arena, PRNG, fuel. Does not capture host bindings or bytecode.

With static types, serialization is simpler — no type tags to encode per value. The deserializer knows the type of every register from the bytecode metadata.

## Standard Library

### Core (always available)

- `tostring(v: T) -> string` — convert any value to string representation
- `int(s: string) -> int?` — parse string to int, `None` on failure
- `number(s: string) -> number?` — parse string to number, `None` on failure
- `len(v: T) -> int` — string byte length, array length, map entry count (T must be string, array, or map)
- `error(msg: string)` — raise a ScriptError
- `assert(v: bool, msg: string?)` — raise if false
- `print(v: string)` — host-provided print handler
- `bind(f: func, args...) -> func` — partial application, freeze leading arguments

### math (opt-in)

Unchanged: `abs`, `floor`, `ceil`, `round`, `sqrt`, `min`, `max`, `clamp`, `lerp`, `sin`, `cos`, `atan2` (CORDIC), `random()`, `random(n)`, `pi`.

### string (opt-in)

Unchanged: `sub`, `find`, `upper`, `lower`, `split`, `trim`, `starts_with`, `ends_with`, `rep`, `byte`, `char`.

### array (opt-in)

- `push(v: T)`, `pop() -> T?`, `insert(i: int, v: T)`, `remove(i: int) -> T`
- `sort(cmp: func(T, T) -> bool)` — takes a function reference as comparator
- `contains(v: T) -> bool`, `join(sep: string) -> string`, `reverse()`
- `get(i: int) -> T?` — safe access, returns `None` on out-of-bounds
- `each(f: func(T))`, `map(f: func(T) -> U) -> array<U>` — takes function references

### map (opt-in)

- `keys() -> array<K>`, `values() -> array<V>`
- `contains(k: K) -> bool`, `remove(k: K)`
- `get(k: K) -> V?` — safe access, returns `None` on missing key

### bit (opt-in)

Unchanged: `bit.and`, `bit.or`, `bit.xor`, `bit.not`, `bit.lshift`, `bit.rshift`.

## Runtime Error Reduction

Static types eliminate entire error categories. Safe-access patterns reduce the rest.

| Error | Status | Mechanism |
|-------|--------|-----------|
| **TypeError** | **Eliminated** | Static types — impossible by construction. |
| **ReadOnlyField** | **Eliminated** | `extern struct` readonly annotation — compiler rejects writes. |
| **KeyNotFound** | **Reduced** | `map.get(key)` returns `V?`. `map[key]` still asserts (for when you know the key exists). |
| **IndexOutOfBounds** | **Reduced** | `array.get(i)` returns `T?`. `array[i]` still asserts. |
| DivisionByZero | Kept | Logic bug. Making division return `number?` would poison every arithmetic expression. |
| ArenaExhausted | Kept | Resource limit. Non-catchable. |
| FuelExhausted | Kept | Resource limit. Non-catchable. |
| CallOverflow | Kept | Resource limit. Non-catchable. |
| CoroutineDead | Kept | Check `.status` before resume. |
| HostError | Kept | External — host functions can fail. |
| ScriptError | Kept | Deliberate (`error()` calls). |

**Principle:** Make common errors impossible (types), occasional errors opt-in (safe access), rare errors loud (crash).

## Formal Determinism

The determinism contract — what "same execution" means across implementations:

- **Register-level equivalence.** Same bytecode + same inputs + same fuel → identical register state at every instruction boundary.
- **Map iteration order** preserved across serialize/deserialize (insertion-ordered).
- **Coroutine resume sequence** is deterministic — same yields in same order, same values.
- **Error kind and stack trace structure** are part of the contract. Exact error message text is not.
- **Fuel cost** is 1 per instruction, no exceptions. Two implementations counting fuel identically is a hard requirement.
- **Arena exhaustion** is deterministic — same allocation sequence → same failure point.
- **PRNG state evolution** is part of the contract — xoshiro128++, specified seed expansion via SplitMix64.
- **Fixed-point arithmetic** is integer math — bitwise identical on all platforms by construction.
- **Sort stability** is required — `array.sort()` uses a stable sort algorithm.

## What This Enables

- **Multiple conforming implementations.** The formal determinism spec means "any domain can re-execute" is real, not aspirational.
- **Scripts as tradeable economic assets.** A combat doctrine script that type-checks is worth more than one that might crash at runtime. Static types are a quality guarantee.
- **Smaller runtime.** 8-byte values, no type dispatch, no closure machinery. Better cache behavior. Faster per-entity-per-frame GDL scripts.
- **Content-addressed bytecode that's meaningful.** The hash identifies behavior, not just bytes. Two implementations running the same hash produce the same result.
- **NPC AI as sequential code.** Coroutines let modders write patrol loops, dialogue trees, and multi-tick behavior as straightforward sequential logic.
- **Compile-time safety net.** Modders deploying a combat script get a compiler that catches type mismatches, missing enum variants, and readonly field writes before the script ever runs.

## Open Questions

**Coroutine resume-with-value.** Multi-tick combat scripts need per-tick inputs (beacon value, revealed orders). How does a resumed coroutine receive them? Two options: (a) `yield` returns the value passed by the host on resume — `const orders = yield(my_result)`, or (b) the host rebinds extern struct data before each resume and the coroutine reads it. Option (a) is cleaner for combat scripts. Option (b) is simpler to implement and already works. The current VM spec has `RESUME A B` (pass value) and `YIELD A` (return value), so the mechanism exists — it needs to be surfaced in the language syntax.

**Fuel budgets are host-configured, not language-specified.** Galaxy generation (10K stars) might need millions of instructions. GDL client scripts need 10K. Verification re-execution might need 100K. The language doesn't prescribe limits — the host sets fuel per call via `vm.set_fuel()`. A "run to completion" mode (fuel = max) is valid for trusted server-side scripts. This is deployment configuration, not language design.

**GDL predict scripts and cross-frame state.** The `predict` script category in GDL needs velocity integration and interpolation — which require state across frames. Pure functions can't do this. Two solutions: (a) the domain includes previous state in entity properties (host manages the state), or (b) GDL uses coroutines for stateful script categories (the coroutine persists across frames, yielding each frame). This is a GDL design concern, not a Raido concern — the VM supports both patterns.

## What This Defers

| Topic | Status |
|-------|--------|
| `extend` (methods on structs) | Under consideration. Useful ergonomics but adds complexity. Not needed for v1. |
| Generics beyond built-ins | Not planned. `array<T>`, `map<K,V>`, `T?`, function types cover the need. |
| Traits / interfaces | Not planned. Functions, not methods. |
| Operator overloading | Rejected. Hidden dispatch, verification hazard. |
| Comptime evaluation | Not planned. Overkill for a scripting VM. |
| Dynamic eval / loadstring | Rejected. Incompatible with content addressing. |

## Compared to Lua

What you lose:

| Lua feature | Raido equivalent | Gap |
|-------------|-----------------|-----|
| Closures | Function references + `bind()` | Can't create functions at runtime. Can't capture mutable state. |
| Tables (universal) | `struct` + `array<T>` + `map<K,V>` | No heterogeneous collections. More types to learn. |
| Metatables | Nothing | No operator overloading, no prototype OOP, no metaprogramming. |
| `loadstring()` | Nothing | No dynamic code loading. |
| `pcall()` / `xpcall()` | `try`/`else` | Resource errors (fuel, arena) not catchable. |
| Tiny compiler | Two-pass compiler | Larger compiler binary. Still no AST, still O(declarations) memory. |
| Ecosystem | Nothing | No community, no tutorials, no LuaJIT. |

What you gain:

| Raido feature | Lua equivalent | Advantage |
|---------------|---------------|-----------|
| Static types | None | Compile-time bug detection. No runtime type errors. |
| Enums with payloads | Magic strings/ints | Exhaustive match. Compiler-enforced correctness. |
| Typed extern structs | Untyped C API | Load-time type checking. No runtime field errors. |
| `T?` optionals | nil | Null safety by construction. |
| Content-addressed chunks | None | Verifiable identity. Audit trails. |
| Formal determinism spec | Approximate | Multiple conforming implementations possible. |
| `bind()` | Closures | Partial application without shared mutable state. |
| Module imports | `require()` | Content-addressed dependencies. Import graph is part of chunk identity. |

The trade: Raido is a narrower language than Lua. It can't do everything Lua does. The things it loses are exactly what makes Lua unsuitable for deterministic verification. Function references + coroutines + structs/enums + `bind()` cover ~90% of game scripting. The missing 10% (metaprogramming, inline closures, prototype OOP) is what verification can't tolerate.
