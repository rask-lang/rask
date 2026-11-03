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
let x = 42           // immutable binding (type inferred: int)
mut y = 10             // mutable binding (type inferred: int)
let name: string = get_name()  // explicit type annotation (optional)
```

`let` prevents reassignment and mutation through the binding (field writes, indexed writes). `mut` allows both. Same as Rask. See [types.md](types.md#let-and-mut) for full semantics.

No `global` keyword. Script state lives in coroutine locals or host entities.

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

### Function References

Named top-level functions can be used as values, and so can lambdas: inline functions that capture nothing.

```raido
func by_health(a: Ship, b: Ship) -> bool { return a.health < b.health }

ships.sort(by_health)
ships.sort(|a, b| a.health < b.health)          // the same, inline
ships.sort(|a: Ship, b: Ship| -> bool {         // block form
    return a.health < b.health
})
```

Parameter and return types come from where the lambda is used, or are written out. A lambda can't use a local from outside it; pass the value as a parameter instead. It compiles to an unnamed top-level function, so a lambda and a function reference are the same value.

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

### Extern Declarations

Scripts declare the shapes they expect from the host. The compiler type-checks all access. The host binds at `vm.load()` -- type mismatch is a load error.

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

### Extend

Add methods to structs and enums. Same as Rask: `self` is read-only, `mutate self` changes it.

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

// loop -- infinite loop with break value
loop {
    let msg = try receive() else { break }
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
    let data = try read_file(path)
    return parse(data)
}

// Catch and handle
let config = load_config("app.cfg") catch e => {
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
let colors: array<string> = ["red", "green", "blue"]
let point: map<string, int> = {"x": 10, "y": 20}
```

Array literal type is inferred from elements. Map literal type is inferred from entries. Empty collections need type annotation or context.

## Wrapping Arithmetic

Integer overflow panics by default. For explicit wrapping (seed hashing, PRNGs):

```raido
let h = seed.wrapping_mul(6364136223846793005).wrapping_add(index)
```

## Keywords

`break`, `continue`, `else`, `enum`, `extend`, `extern`, `false`, `for`, `func`, `if`, `import`, `in`, `let`, `loop`, `match`, `mut`, `mutate`, `return`, `struct`, `true`, `try`, `while`, `yield`

`coroutine()`, `error()`, and `assert()` are built-in functions (core), not keywords.

## Deliberate Divergences from Rask

- **No `public`/package visibility** -- Raido scripts are single-file. All declarations are visible within the script and importable by other chunks.
- **No ownership** -- No moves, no borrowing, no `take`, no `.clone()`. Structs are arena references, primitives are values, and the arena is reset between evaluations. `mutate` is the only parameter mode, and it means what it means in Rask: the function may change what it's given.
- **No closures** -- Function references only. Captured mutable state is a verification hazard.
- **No character type** -- Strings only, so single quotes have no meaning.
- **No `using` context clauses** -- Host data access is through `extern struct`, not context parameters.
