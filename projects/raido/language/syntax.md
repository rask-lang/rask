<!-- id: raido.syntax -->
<!-- status: proposed -->
<!-- summary: Raido syntax -- typed Rask subset with structs, enums, extern declarations, module imports -->
<!-- depends: raido/language/types.md -->

# Syntax

Typed subset of Rask. Same `{}` blocks, `match`/`=>`, `if`/`else if`, `for`/`in`. Function signatures carry type annotations, locals are inferred.

## Lexical

- Newline-terminated statements. Semicolons optional.
- `//` line comments, `/* */` block comments.
- Numbers: `42` (int), `3.14` (number), `0xff`, `0b1010`, `1_000_000`.

### Strings

Strings are a first-class type. Immutable, UTF-8.

```raido
"hello"                     // basic string
"hello {name}"              // interpolation -- any expression in {}
"value: {x + 1}"            // expressions in interpolation
'no interpolation here'     // single-quoted: raw, no escapes, no interpolation
```

Escape sequences (double-quoted only): `\n`, `\t`, `\\`, `\"`, `\{` (literal brace), `\0`.

## Variables

```raido
const x = 42           // immutable local (type inferred: int)
let y = 10             // mutable local (type inferred: int)
const name: string = get_name()  // explicit type annotation (optional)
```

No `global` keyword. Script state lives in coroutine locals or host entities.

## Functions

```raido
func greet(name: string) -> string {
    return "Hello, {name}"
}

func damage(weapon: Weapon, target: Ship, beacon: int) -> number {
    const base = weapon.power * weapon.efficiency
    const scatter = noise(weapon.quality, weapon.id, beacon)
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

### Function References

Named top-level functions can be used as values. No closures, no lambda syntax.

```raido
func by_health(a: Ship, b: Ship) -> bool { return a.health < b.health }

const comparator = by_health
ships.sort(comparator)
```

## Declarations

### Struct

```raido
struct Vec2 { x: number, y: number }

struct Ship {
    id: int
    health: int
    x: number
    y: number
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
    Allocate { system: int, power: number }
}
```

Variants accessed with dot syntax: `Order.Attack(target_id)`, `Stance.Aggressive`.

### Extern Declarations

Scripts declare the shapes they expect from the host. The compiler type-checks all access. The host binds at `vm.load()` -- type mismatch is a load error.

```raido
extern struct Enemy {
    health: int
    x: number
    y: number
    readonly name: string
}

extern func send_message(target: string, body: string)
extern func noise(quality: number, id: int, index: int) -> number
extern func move_to(entity: Enemy, target: Vec2)
```

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

// loop -- infinite loop with break value
loop {
    const msg = try receive() else { break }
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
const sign = if x > 0 { "+" } else { "-" }
const color = match status { Status.Active => "green", _ => "gray" }
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
const defense = shield ?? default_shield

// Force unwrap -- panics on None
const order = orders.get(ship.id)!

// Safe access
const item = inventory.get(3)      // returns T?
const entry = lookup.get("iron")   // returns V?

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
const damaged = Ship { health: ship.health - dmg, ..ship }
const next_state = FleetState { ships: new_ships, tick: state.tick + 1, ..state }
```

## Operators

| Precedence | Operators |
|-----------|-----------|
| 1 (highest) | `!`, `-` (unary) |
| 2 | `*`, `/`, `%` |
| 3 | `+`, `-` |
| 4 | `<`, `>`, `<=`, `>=`, `==`, `!=` |
| 5 | `&&` |
| 6 | `\|\|` |
| 7 (lowest) | `??` |

`&&`/`||` short-circuit and return `bool`. `??` unwraps optionals with a default.

No `#` length operator -- use `len()` from core. No `..` concat operator -- string interpolation covers it.

## Error Handling

`try` propagates errors to the caller, matching Rask's error handling syntax:

```raido
func load_config(path: string) -> Config or string {
    const data = try read_file(path)
    return parse(data)
}

// Catch and handle
const config = try load_config("app.cfg") else |e| {
    log("fallback: {e}")
    return default_config()
}

// Raise an error
error("invalid state")

// Assert
assert(x > 0, "x must be positive")
```

## Collections

```raido
const colors: array<string> = ["red", "green", "blue"]
const point: map<string, int> = {"x": 10, "y": 20}
```

Array literal type is inferred from elements. Map literal type is inferred from entries. Empty collections need type annotation or context.

## Wrapping Arithmetic

Integer overflow panics by default. For explicit wrapping (seed hashing, PRNGs):

```raido
const h = seed.wrapping_mul(6364136223846793005).wrapping_add(index)
```

## Keywords

`break`, `const`, `continue`, `else`, `enum`, `extern`, `false`, `for`, `func`, `if`, `import`, `in`, `let`, `loop`, `match`, `return`, `struct`, `true`, `try`, `while`, `yield`

`coroutine()`, `error()`, and `assert()` are built-in functions (core), not keywords.

## Deliberate Divergences from Rask

- **Single-quoted strings** -- Rask uses `'a'` for character literals. Raido has no character type; single quotes are raw strings (`'no {interpolation}'`).
- **No `public`/package visibility** -- Raido scripts are single-file. All declarations are visible within the script and importable by other chunks.
- **No `extend` blocks (deferred)** -- Methods on user-defined structs are deferred. Built-in methods on `array`, `map`, `string` are compiler-known.
- **No parameter modes** -- No `mutate`/`take`. Raido values are arena-managed, not ownership-tracked. Structs pass by reference (arena offset), primitives by value.
- **No `using` context clauses** -- Host data access is through `extern struct`, not context parameters.
