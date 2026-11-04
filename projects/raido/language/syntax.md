<!-- id: raido.syntax -->
<!-- status: proposed -->
<!-- summary: Raido syntax -- typed Rask subset with structs, enums, extern declarations, module imports -->
<!-- depends: raido/language/types.md -->

# Syntax

Typed subset of Rask. Same `{}` blocks, `match`/`=>`, `if`/`else if`, `for`/`in`. Function signatures carry type annotations, locals are inferred.

## Lexical

- Newline-terminated statements. Semicolons optional.
- `//` line comments, `/* */` block comments.
- Numbers: `42` (int), `3.14` (float), `0xff` (int), `0b1010` (int), `1_000_000` (int). Decimal point means `float`, no decimal means `int`.

### Strings

Strings are a first-class type. Immutable, UTF-8.

```raido
"hello"                     // basic string
"hello {name}"              // interpolation -- any expression in {}
"value: {x + 1}"            // expressions in interpolation
"a literal \{ brace"        // \{ escapes interpolation
```

Escape sequences: `\n`, `\t`, `\\`, `\"`, `\{` (literal brace), `\0`. Single quotes aren't string delimiters; there's no character type.

## Variables

```raido
let x = 42                     // type inferred: int
x = 50                         // any variable can be reassigned
let name: string = get_name()  // explicit type annotation (optional)
```

`let` declares a variable, as in JS. Assigning to a name that was never declared is an error, so a typo can't quietly make a new variable. Unlike Rask, there's no `mut`: every variable can be reassigned and its fields changed. See [types.md](types.md#variables).

### Module State

A `let` at the top level of a script is a module variable. It's set when the host loads the script and keeps its value between host calls, so a death counter or a "notified once" flag lives there:

```raido
let deaths = 0
let notified = false
let spawned: array<Unit> = []

func on_unit_died(u: Unit) {
    deaths += 1
}
```

Module variables are part of the VM's state: serialized with it, and rolled back with everything else when a call fails. Each loaded chunk has its own; an imported module's variables belong to that module, read as `combat.hits`. Its initializer runs once, in declaration order, when the chunk is loaded; if one fails, the load fails.

`const` names a value that never changes. Its initializer is a constant expression: literals, arithmetic and other constants.

```raido
const MAX_WAVES = 10
const SPAWN_RADIUS = 12.5
```

## Functions

```raido
func greet(name: string) -> string {
    return "Hello, {name}"
}

func damage(weapon: Weapon, target: Ship, beacon: int) -> float {
    let base = weapon.power * weapon.efficiency
    let scatter = noise(weapon.quality, weapon.id, beacon)
    return clamp(base + scatter, 0.0, base * 2.0)
}
```

Functions require explicit `return` (matches Rask). Signatures are fully typed -- parameters and return type annotated. Local variables inferred from initializer.

### Named Arguments

Order-fixed, same as Rask. Optional (positional calls still work). Compiler checks names match declaration.

```raido
func transfer(source: Ship, target: Ship, amount: int) { ... }
transfer(source: attacker, target: cargo, amount: 50)
```

### Functions as Values

Named functions can be used as values, and so can closures: inline functions written `|params| body`.

```raido
func by_health(a: Ship, b: Ship) -> bool { return a.health < b.health }

ships.sort(by_health)
ships.sort(|a, b| a.health < b.health)          // the same, inline
ships.sort(|a: Ship, b: Ship| -> bool {         // block form
    return a.health < b.health
})
```

Parameter and return types come from where the closure is used, or are written out.

A closure can use variables from around it:

```raido
let threshold = 10
let weak = ships.filter(|s| s.health < threshold)
let nearest = |u| distance(u, home)
```

One rule: a variable a closure uses is never assigned after its `let`, anywhere in the function, inside the closure or outside it. To share a changing value, put it in a struct, which is shared by reference like everything else, or in a module variable:

```raido
let count = 0
let bump = || count += 1          // ERROR: `count` is used by a closure, so it can't change
let tally = Tally { n: 0 }
let bump = || tally.n += 1        // OK: the closure and the caller share `tally`

let x = 0
loop { x += 1; fs.push(|| x) }    // ERROR: `x` is assigned after its `let`
```

See [types.md](types.md#functions-as-values) for the full rules.

## Declarations

### Struct

```raido
struct Vec2 { x: float, y: float }

struct Ship {
    id: int
    health: int
    x: float
    y: float
    shield: Shield?
}
```

### Enum

```raido
enum Stance {
    Aggressive
    Defensive
    Evasive
    HoldPosition
}

enum Order {
    Attack(int)
    Retreat
    HoldPosition
    Allocate { system: int, power: float }
}
```

Variants accessed with dot syntax: `Order.Attack(target_id)`, `Stance.Aggressive`.

### The Host's API

Scripts don't declare what the host provides. The host publishes its API as a module of `extern` declarations, and every script sees it already imported: a mod for a game writes `move_to(enemy, home)` and `enemy.health` with nothing to set up. The compiler type-checks every use against the published module.

`extern` is how that module is written, by whoever writes the host:

```raido
extern struct Enemy {
    health: int
    x: float
    y: float
    readonly name: string
}

extern func send_message(target: string, body: string)
extern func noise(quality: float, id: int, index: int) -> float
extern func move_to(entity: Enemy, target: Vec2)
```

A host function that changes the world and returns nothing can be marked `deferred`: its calls are queued and run only when the script's call succeeds (see [vm/interop.md](../vm/interop.md#calls-are-transactions)).

```raido
extern deferred func send_message(target: string, body: string)
```

A host function that takes time is marked `pausing`: calling it pauses the hook until the host resumes it with the result (see [pausing.md](pausing.md)).

```raido
extern pausing func wait(seconds: float)
extern pausing func ask(player: Unit, prompt: string) -> string
```

The module may also contain ordinary Raido types and functions every script gets.

A script may not contain `extern` declarations of its own. See [vm/interop.md](../vm/interop.md#the-published-api) for how the module is published and checked.

### Extend

Add methods to structs and enums. A method can change `self`, like any function can change what it's given.

```raido
extend Vec2 {
    func distance(self, other: Vec2) -> float {
        let dx = self.x - other.x
        let dy = self.y - other.y
        return sqrt(dx * dx + dy * dy)
    }

    func zero() -> Vec2 {
        return Vec2 { x: 0.0, y: 0.0 }
    }
}

let d = pos.distance(target)
let origin = Vec2.zero()
```

See [types.md](types.md#extend) for rules.

### Module Imports

Scripts can import other content-addressed chunks:

```raido
import "combat_utils" as combat
import "physics_common" as phys

func resolve_tick(state: FleetState, orders: Orders) -> CombatResult {
    let damage = combat.calculate_damage(state, orders)
    ...
}
```

The import graph is part of the chunk's content hash. The host resolves import names to chunks.

## Control Flow

```raido
if health <= 0 {
    die()
} else if health < 20 {
    warn("low")
} else {
    fight()
}

// Colon inline syntax -- single expression after colon
if health < 30: return Animation.Hurt
for star in stars: generate_planets(star)

while queue_size() > 0 {
    process(dequeue())
}

for item in inventory { print(item) }
for i in 0..10 { print(i) }           // 0 through 9 (exclusive)
for i in 0..=10 { print(i) }          // 0 through 10 (inclusive)
for name, score in leaderboard { print("{name}: {score}") }

// loop -- runs until `break` or `return`
loop {
    let msg = receive() is Some else { break }
    handle(msg)
}

// continue -- skip to next iteration
for ship in fleet.ships {
    if ship.health <= 0: continue
    apply_orders(ship, orders)
}
```

### Match

Exhaustive on enums -- compiler error if a variant is missing.

```raido
match order {
    Order.Attack(target) => engage(fleet, target),
    Order.Retreat => disengage(fleet),
    Order.HoldPosition => hold(fleet),
    Order.Allocate { system, power } => reallocate(fleet, system, power),
}

// Pattern guards
match order {
    Order.Attack(target) if target.health > 0 => engage(target),
    Order.Attack(_) => find_new_target(),
    Order.Retreat if fleet.can_retreat() => disengage(),
    Order.Retreat => last_stand(),
    _ => hold(),
}

// Expression context
let sign = if x > 0 { "+" } else { "-" }
let color = match status { Status.Active => "green", _ => "gray" }
```

### `is` Pattern Matching

Match a single pattern in conditions. Same as Rask.

```raido
if opt is Some(v): use(v)
if entity.shield is Some(s): apply_damage(s)
```

### `is ... else` Guard

Bind-and-unwrap with early exit. The `else` block must diverge.

```raido
let target = find_ship(ships, target_id) is Some else { continue }
let item = queue.pop() is Some else { break }
```

## Optionals

```raido
// Null coalescing
let defense = shield ?? default_shield

// Force unwrap -- panics on None
let order = orders.get(ship.id)!

// Safe access
let item = inventory.get(3)      // returns T?
let entry = lookup.get("iron")   // returns V?

// Match on optionals
match shield {
    Some(s) => apply_to(s),
    None => take_full_damage(),
}
```

## Mutation

### Compound Assignment

`+=`, `-=`, `*=`, `/=`, `%=` on any l-value.

```raido
result.density += element.density * fraction
count *= 2
```

### Chained L-value Mutation

Mutate a struct field through an array index.

```raido
ships[i].health -= int(damage)
ships[i].shield = None
fleet.ships[idx].engine.fuel -= cost
```

### Struct Update

Copy all fields, override specific ones.

```raido
let damaged = Ship { health: ship.health - dmg, ..ship }
let next_state = FleetState { ships: new_ships, tick: state.tick + 1, ..state }
```

## Operators

| Precedence | Operators |
|-----------|-----------|
| 1 (highest) | `!`, `-` (unary) |
| 2 | `*`, `/`, `//`, `%` |
| 3 | `+`, `-` |
| 4 | `<`, `>`, `<=`, `>=`, `==`, `!=` |
| 5 | `&&` |
| 6 | `\|\|` |
| 7 (lowest) | `??` |

`&&`/`||` short-circuit and return `bool`. `??` unwraps optionals with a default. `/` always gives a `float`; `//` divides two ints and truncates toward zero, matching `%` (see [types.md](types.md#arithmetic-rules)).

No `#` length operator -- use `len()` from core. No `..` concat operator -- string interpolation covers it.

## Errors

There are no error types to handle. A script that hits something it can't go on from calls `error`, and the call the host made into the script fails as a whole: every change it made is thrown away (see [vm/interop.md](../vm/interop.md#calls-are-transactions)), and the host logs the message with a stack trace.

```raido
error("no target in range")
assert(x > 0, "x must be positive")
```

Something that may legitimately be missing returns `T?`, and the caller decides with `??`, `is Some` or `match`:

```raido
func find_target(ships: array<Ship>, id: int) -> Ship? { ... }

let target = find_target(ships, id) ?? return        // nothing to do this tick
```

The right side of `??` has the optional's inner type, or leaves the block: `return`, `break`, `continue` or `error(...)`.

## Collections

```raido
let colors: array<string> = ["red", "green", "blue"]
let point: map<string, int> = {"x": 10, "y": 20}

colors[0] = "crimson"                  // replace; out of range is an error
point["z"] = 5                         // insert, or replace if the key exists
let x = point["x"]                     // read; a missing key is an error
let w = point.get("w") ?? 0            // read that may miss
point["x"] += 1                        // read then write; the key must exist
point.remove("y")
```

Array literal type is inferred from elements. Map literal type is inferred from entries. Empty collections need type annotation or context.

During `for item in arr`, the loop reads `arr[0]`, `arr[1]`, ... while the index is below the array's current length, so pushes inside the loop are visited and removals shift what's left. During `for k, v in m`, adding or removing a key is an error; assigning to an existing key is fine.

## Wrapping Arithmetic

Integer overflow panics by default. For explicit wrapping (seed hashing, PRNGs):

```raido
let h = seed.wrapping_mul(6364136223846793005).wrapping_add(index)
```

## Keywords

`break`, `const`, `continue`, `deferred`, `else`, `enum`, `extend`, `extern`, `false`, `for`, `func`, `if`, `import`, `in`, `let`, `loop`, `match`, `pausing`, `return`, `struct`, `true`, `while`

`error()` and `assert()` are built-in functions (core), not keywords.

## Deliberate Divergences from Rask

- **No `public`/package visibility** -- Raido scripts are single-file. All declarations are visible within the script and importable by other chunks.
- **Module-level `let`** -- Rask has no mutable globals; a Raido script's top-level `let` is state that lives between host calls.
- **No ownership** -- No moves, no borrowing, no `take`, no `.clone()`. Structs, arrays and maps are references to a garbage-collected heap; primitives are values. There are no parameter modes: a function can change what it's given, as in JS, Python and Lua.
- **No `mut`** -- `let` declares, and every variable can be reassigned, as in JS, Lua and C. Rask code that uses `let` still compiles here; Raido code moved to Rask gets a clear "declare it `mut`" error.
- **Closures can't reassign what they capture** -- a Rask closure that doesn't outlive its frame can change a captured `mut` variable; Raido has no `mut`, and forbids reassigning a captured variable, so there's no hidden shared state.
- **No character type** -- Strings only, so single quotes have no meaning.
- **No `T or E`, `try` or `catch`** -- a failure aborts the host's call and rolls it back; absence is `T?`.
- **No `using` context clauses** -- Host data access is through `extern struct`, not context parameters.
