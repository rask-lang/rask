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
- **No NaN/infinity.** Division by zero = runtime error. Overflow saturates.
- Scripts write `3.14`, compiler converts to fixed-point.

`int` and `number` are separate types -- no implicit coercion. Use `number(x)` or `int(x)` for explicit conversion.

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

Structs are arena-allocated. A struct value in a register is a u32 arena offset. Field access compiles to `GET_STRUCT_FIELD` / `SET_STRUCT_FIELD` with the field index known at compile time.

**Struct update syntax** copies all fields, overriding specific ones:

```raido
const damaged = Ship { health: ship.health - dmg, ..ship }
```

**Field shorthand** -- omit the value when the variable name matches the field name:

```raido
return Star { id: index, x, y, z, spectral, planet_count, luminosity }
```

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

Simple enums (no payloads) are stored inline as a discriminant. Enums with payloads use a u32 discriminant + u32 arena offset.

## Optionals

`T?` replaces nil. Either `Some(value)` or `None`. Compiler-enforced -- you can't use an optional without handling the `None` case.

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

## Type Rules

- Function signatures are fully typed (parameters + return type)
- Local variables inferred from initializer: `const x = 42` -> `x` is `int`
- No generics beyond built-in `array<T>`, `map<K, V>`, `T?`, tuples, and function types
- No traits or interfaces
- Exhaustive `match` on enums -- compiler error if a variant is missing
- `int` and `number` are separate -- no implicit coercion. `number(x)` or `int(x)` for conversion
- `??` unwraps optionals with a default: `value ?? fallback` (both sides same type)
- Compound assignment: `+=`, `-=`, `*=`, `/=`, `%=` on any l-value including chained access (`ships[i].health -= damage`)
- Force unwrap: `value!` panics if `None`
- Integer overflow: **panic by default**. Use `wrapping_mul()`, `wrapping_add()` for explicit wrapping arithmetic. Part of the determinism contract.
- Sort stability is required -- `array.sort()` uses a stable sort algorithm

## Maps

Insertion-ordered. Deterministic iteration order. Keys restricted to `string` or `int` -- types with stable, deterministic equality and hashing.

```raido
const lookup: map<string, int> = {"iron": 26, "gold": 79}
const entry = lookup.get("iron")  // returns int?
```
