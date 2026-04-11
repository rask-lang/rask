<!-- id: raido.types -->
<!-- status: proposed -->
<!-- summary: Raido type system -- static types, fixed-point numbers, structs, enums, optionals, function references -->
<!-- depends: raido/README.md -->

# Types

Statically typed. Function signatures are fully annotated, local variables are inferred from their initializer. No runtime type tags -- the compiler knows every register's type at every instruction.

## Primitives

| Type | Size | Description |
|------|------|-------------|
| `int` | i64 | Counters, IDs, indices, bitfields. |
| `number` | i64 (32.32 fixed-point) | Physics, coordinates, economics. Deterministic. |
| `bool` | i64 (0 or 1) | `true`/`false`. |
| `string` | u32 arena offset | Immutable UTF-8. Dialogue, identifiers, messages. |

## Composite Types

| Type | Description |
|------|-------------|
| `struct` | User-defined named record. Fixed fields, known at compile time. |
| `enum` | Tagged union with optional payloads. Exhaustive `match`. |
| `array<T>` | Homogeneous growable sequence. 0-indexed. |
| `map<K, V>` | Key-value store. `K` restricted to `string` or `int`. Insertion-ordered. |
| `T?` | Optional. Either `Some(value)` or `None`. |
| `(T, U, ...)` | Tuple. Lightweight grouping for multi-return. |

## Function Types

`func(int, int) -> bool` -- describes the signature of a function reference. The value is a prototype index pointing to a named top-level function. No closures, no captured state.

## Number (Fixed-Point 32.32)

Stored as i64. 32 integer bits (signed), 32 fractional bits.

- **Range:** +/-2.1 billion. **Precision:** ~2^-32 (~9.6 decimal digits).
- **Deterministic.** Integer math. Same result on every platform.
- **Fast.** Add/sub = single i64 op. Mul/div = 128-bit intermediate.
- **No NaN/infinity.** Division by zero = runtime error. Number overflow saturates. Integer overflow panics (use `wrapping_*` methods for explicit wrapping).
- Scripts write `3.14`, compiler converts to fixed-point.
- **Literal rule:** decimal point means `number`, no decimal means `int`. `42` is int, `42.0` is number, `0xff` is int.

### Why 32.32 over 48.16

I chose 32.32 over 48.16. 48.16 gives more integer range (~140 trillion vs ~2.1 billion) but only ~4.8 decimal digits of fractional precision. Simulation showed that's too coarse for real use cases:

- **Physics accumulation** (10K frames at 1/60): 32.32 error 6e-7, 48.16 error 4e-2
- **Damping chains** (1000 frames x 0.999): 32.32 error 6e-5, 48.16 error 2.6
- **Financial sums** (0.01 x 10K): 32.32 error 2e-6, 48.16 error 5e-2

The 2.1B integer ceiling is real but manageable -- `int` (i64) handles large values. Use `int` for entity IDs, large counters, and scores. Use `number` for positions, velocities, fractions, and math.

## Struct

User-defined named records. Fields are fixed at compile time. Layout is determined by the compiler -- field order in the arena matches declaration order.

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

Structs are arena-allocated. A struct value in a register is a u32 arena offset. All fields occupy 8 bytes each (i64) regardless of logical type -- bools are stored as i64 0/1, arena offsets are zero-extended. Field access compiles to `GET_STRUCT_FIELD` / `SET_STRUCT_FIELD` with the field index known at compile time.

**Pass by reference.** A struct parameter is an arena offset. Multiple bindings can point to the same struct. Mutation through a `let` binding is visible through all bindings to the same offset. See [Const and Let](#const-and-let) for mutation rules.

**Struct update syntax** copies all fields, overriding specific ones:

```raido
const damaged = Ship { health: ship.health - dmg, ..ship }
```

**Field shorthand** -- omit the value when the variable name matches the field name:

```raido
return Star { id: index, x, y, z, spectral, planet_count, luminosity }
```

## Extend

Add methods to any struct or enum. Methods use `self` as the first parameter — always const (no mutation through `self`). The compiler desugars `x.method(args)` into a normal function call with `x` as the first argument. No new opcodes, no vtable, no dynamic dispatch.

```raido
struct Money {
    cents: int
}

extend Money {
    func from_dollars(dollars: int) -> Money {
        return Money { cents: dollars * 100 }
    }

    func add(self, other: Money) -> Money {
        return Money { cents: self.cents + other.cents }
    }

    func percent(self, rate: int) -> Money {
        return Money { cents: self.cents * rate / 100 }
    }

    func to_number(self) -> number {
        return number(self.cents) / 100.0
    }

    func to_string(self) -> string {
        return "{self.cents / 100}.{self.cents % 100}"
    }
}

const price = Money.from_dollars(10)
const tax = price.percent(8)
const total = price.add(tax)
```

**Rules:**

- `self` is always const — methods can read fields but not mutate them. Return a new value instead.
- Methods without `self` are static: `Money.from_dollars(10)`. Called via `Type.method()`.
- Methods with `self` are instance: `price.add(tax)`. Called via `value.method()`.
- Multiple `extend` blocks on the same type are allowed (useful for imported modules adding methods).
- Extend works on `struct` and `enum` types. Not on primitives — `int`, `number`, `bool`, `string` have compiler-known built-in methods only.
- Extend blocks are part of the chunk's type table and content hash.

## Enum

Tagged unions with optional payloads. Exhaustive `match` -- the compiler rejects incomplete matches.

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

Variants are accessed with dot syntax: `Order.Attack(target_id)`, `Stance.Aggressive`.

Simple enums (no payloads) are stored inline as a u32 discriminant in the register -- no arena allocation. Enums with payloads use a u32 discriminant + u32 arena offset in the register. The arena body contains only the payload fields (no redundant discriminant -- the register already has it).

## Optionals

`T?` is the optional type. Either `Some(value)` or `None`. Compiler-enforced -- you can't use an optional without handling the `None` case.

```raido
const shield: Shield? = find_shield(inventory)

// Null coalescing
const defense = shield ?? default_shield

// Force unwrap -- panics on None
const order = orders.get(ship.id)!

// Pattern matching
match shield {
    Some(s) => apply_to(s),
    None => take_full_damage(),
}

// is pattern
if entity.shield is Some(s): apply_damage(s)

// Guard pattern -- bind + early exit
let target = find_ship(ships, target_id) is Some else { continue }
```

Stored as u8 tag (0=None, 1=Some) + 7 bytes payload.

## Tuples

Lightweight multi-return without defining a struct.

```raido
func bounds(arr: array<number>) -> (number, number) {
    // ...
    return (lo, hi)
}

const (lo, hi) = bounds(values)
```

Tuples are anonymous arena-allocated structs. `(number, number)` is a 2-field struct with 8 bytes per field. The compiler generates internal type entries for tuple types -- no user-visible struct name. Arena allocation is cheap (bump pointer) and frames get reset between evaluations.

## Function References

References to named top-level functions. No captured state, no arena allocation. A function reference is a prototype index -- the simplest possible callable value.

```raido
func by_health(a: Ship, b: Ship) -> bool { return a.health < b.health }

const comparator = by_health
ships.sort(comparator)

// Coroutine creation -- function reference + initial arguments
func patrol(npc: Entity, route: array<Vec2>) { ... }
const co = coroutine(patrol, guard, waypoints)
```

No closures, no partial application. If you need to pass context, pass it as an argument.

## Extern Types

Scripts declare the shapes they expect from the host. The compiler type-checks all access. The host binds at `vm.load()` -- type mismatch is a load error.

```raido
extern struct Enemy {
    health: int
    x: number
    y: number
    readonly name: string
}

extern func move_to(entity: Enemy, target: Vec2)
extern func noise(quality: number, id: int, index: int) -> number
```

See [vm/interop.md](../vm/interop.md) for host-side binding.

## Const and Let

`const` and `let` control mutability of the binding **and the data it reaches**:

- `const x = ...` -- cannot reassign `x`, cannot mutate fields through `x`
- `let x = ...` -- can reassign `x`, can mutate fields through `x`

```raido
const ship = Ship { id: 1, health: 100, x: 0.0, y: 0.0, shield: None }
ship.health -= 10  // ERROR: ship is const
ship = other_ship  // ERROR: ship is const

let ship = Ship { id: 1, health: 100, x: 0.0, y: 0.0, shield: None }
ship.health -= 10  // OK: ship is let
ship = other_ship  // OK: ship is let
```

**Function parameters are const.** A function cannot mutate a struct passed to it. Return a modified copy instead:

```raido
func apply_damage(ship: Ship, amount: int) -> Ship {
    return Ship { health: ship.health - amount, ..ship }
}
```

For host entity mutation, use extern struct fields -- the host controls the data:

```raido
extern struct Enemy { health: int, x: number, y: number }
func damage_enemy(enemy: Enemy, amount: int) {
    enemy.health -= amount  // OK: extern struct field write goes through host setter
}
```

## Arithmetic Rules

**Same-type arithmetic:**

| Expression | Result | Notes |
|-----------|--------|-------|
| `int + int` | `int` | Also `-`, `*`, `%` |
| `int / int` | `number` | Division always returns number |
| `number + number` | `number` | All ops |

**Mixed arithmetic -- int promotes to number:**

| Expression | Result | Notes |
|-----------|--------|-------|
| `int + number` | `number` | int operand promoted to 32.32 fixed-point |
| `number + int` | `number` | Same |
| `int CMP number` | `bool` | Promotion for comparison too |

Promotion is widening (lossless for ints within +/-2.1B). If the int exceeds number's 32-bit integer range, the promotion panics at runtime. In practice, ints mixed with numbers are small (counts, indices, loop variables).

**Narrowing requires explicit conversion:** `number -> int` always requires `int(x)`, which truncates toward zero. No implicit narrowing.

**Integer overflow:** panics by default. Use built-in wrapping methods on `int` for explicit wrapping (see [Built-in Methods](#built-in-methods)).

**Number overflow:** saturates to +/-max value.

**Modulo:** follows the sign of the dividend. `7 % 3 == 1`, `-7 % 3 == -1`.

**String comparison:** lexicographic by bytes (UTF-8 byte order). `"a" < "b"` is `true`.

## Type Rules

- Function signatures are fully typed (parameters + return type)
- Local variables inferred from initializer: `const x = 42` -> `x` is `int`
- No generics beyond built-in `array<T>`, `map<K, V>`, `T?`, tuples, and function types
- No traits or interfaces
- Exhaustive `match` on enums -- compiler error if a variant is missing
- `??` unwraps optionals with a default: `value ?? fallback` (both sides same type)
- Compound assignment: `+=`, `-=`, `*=`, `/=`, `%=` on `let` l-values including chained access (`ships[i].health -= damage`)
- Force unwrap: `value!` panics if `None`
- Sort stability is required -- `array.sort()` uses a stable sort algorithm

## Built-in Methods

Compiler-known methods on primitive and collection types. These are not user-extensible — `extend` only works on user-defined structs and enums, not on primitives or built-in collections.

**`int` methods:**

- `wrapping_add(other: int) -> int` -- wrapping addition
- `wrapping_sub(other: int) -> int` -- wrapping subtraction
- `wrapping_mul(other: int) -> int` -- wrapping multiplication
- `abs() -> int` -- absolute value (panics on i64 min)

**`array<T>` methods (always available, not opt-in):**

- `len() -> int`
- `get(i: int) -> T?` -- safe access, returns `None` on out-of-bounds
- `push(v: T)`, `pop() -> T?`, `insert(i: int, v: T)`, `remove(i: int) -> T`
- `sort(cmp: func(T, T) -> bool)` -- stable sort
- `contains(v: T) -> bool`, `join(sep: string) -> string`, `reverse()`

**`map<K, V>` methods (always available, not opt-in):**

- `len() -> int`
- `get(k: K) -> V?` -- safe access, returns `None` on missing key
- `keys() -> array<K>`, `values() -> array<V>`
- `contains(k: K) -> bool`, `remove(k: K)`

**`string` methods (always available, not opt-in):**

- `len() -> int` -- byte length
- `sub(start: int, end: int?) -> string` -- substring by byte offset
- `find(pattern: string) -> int?` -- literal substring search
- `upper() -> string`, `lower() -> string` -- ASCII only
- `split(sep: string) -> array<string>`, `trim() -> string`
- `starts_with(prefix: string) -> bool`, `ends_with(suffix: string) -> bool`
- `rep(n: int) -> string`, `byte(i: int) -> int`, `char(n: int) -> string`

## Maps

Insertion-ordered. Deterministic iteration order. Keys restricted to `string` or `int` -- types with stable, deterministic equality and hashing.

```raido
const lookup: map<string, int> = {"iron": 26, "gold": 79}
const entry = lookup.get("iron")  // returns int?
```
