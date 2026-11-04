<!-- id: raido.types -->
<!-- status: proposed -->
<!-- summary: Raido type system -- static types, ints and floats, structs, enums, optionals, functions and closures -->
<!-- depends: raido/README.md -->

# Types

Statically typed. Function signatures are fully annotated, local variables are inferred from their initializer. The compiler knows every register's type at every instruction, so nothing dispatches on type at run time.

## Primitives

| Type | Size | Description |
|------|------|-------------|
| `int` | i64 | Counters, IDs, indices, bitfields. |
| `float` | f64 | Physics, coordinates, fractions. Deterministic on every machine. |
| `bool` | i64 (0 or 1) | `true`/`false`. |
| `string` | heap reference | Immutable UTF-8. Dialogue, identifiers, messages. |

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

`func(int, int) -> bool` -- the type of a function value: a named function or a closure. See [Functions as Values](#functions-as-values).

## Float

A 64-bit IEEE 754 double, the number type JS, Python, Lua and C all have.

- **Same bits on every machine.** `+ - * /` and `sqrt` are correctly rounded by IEEE 754, so they agree everywhere. The VM never fuses a multiply and an add, and `sin`, `cos`, `exp` and the rest are correctly rounded, so they agree too. The full profile is rask-lang/rask#1683, the same one Rask's own floats follow.
- **No NaN, no infinity.** An operation that would produce either (`0.0 / 0.0`, `sqrt(-1.0)`, an overflow) is a runtime error, like integer overflow. So a bad value fails where it's made instead of spreading, and no script ever sees NaN's platform-dependent bits.
- **Literal rule:** a decimal point means `float`, no decimal means `int`. `42` is int, `42.0` is float, `0xff` is int.
- **Use `int` for money and anything counted or conserved.** Floats round; store cents, not dollars. A script that uses only `int` can be proven with zero knowledge ([vm/architecture.md](../vm/architecture.md#provable-profile)).

## Struct

User-defined named records. Fields are fixed at compile time. Fields are numbered in declaration order.

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

Structs are heap objects; a struct value is a reference. Field access compiles to `GET_STRUCT_FIELD` / `SET_STRUCT_FIELD` with the field index known at compile time.

**Pass by reference.** Multiple bindings can point to the same struct. A change made through any binding or a `mutate` parameter is visible through all of them. See [Variables](#variables).

**Struct update syntax** copies all fields, overriding specific ones:

```raido
let damaged = Ship { health: ship.health - dmg, ..ship }
```

**Field shorthand** -- omit the value when the variable name matches the field name:

```raido
return Star { id: index, x, y, z, spectral, planet_count, luminosity }
```

## Extend

Add methods to any struct or enum. Methods take `self` as the first parameter, read-only unless written `mutate self`. The compiler desugars `x.method(args)` into a normal function call with `x` as the first argument. No new opcodes, no vtable, no dynamic dispatch.

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

    func deposit(mutate self, amount: Money) {
        self.cents += amount.cents
    }

    func to_number(self) -> float {
        return float(self.cents) / 100.0
    }

    func to_string(self) -> string {
        return "{self.cents / 100}.{self.cents % 100}"
    }
}

let price = Money.from_dollars(10)
let tax = price.percent(8)
let total = price.add(tax)

let wallet = Money.from_dollars(5)
wallet.deposit(total)       // `mutate self` changes wallet
```

**Rules:**

- `self` is read-only by default. `mutate self` lets the method change the value.
- Methods without `self` are static: `Money.from_dollars(10)`. Called via `Type.method()`.
- Methods with `self` are instance: `price.add(tax)`. Called via `value.method()`.
- Multiple `extend` blocks on the same type are allowed (useful for imported modules adding methods).
- Extend works on `struct` and `enum` types. Not on primitives — `int`, `float`, `bool`, `string` have compiler-known built-in methods only.
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
    Allocate { system: int, power: float }
}
```

Variants are accessed with dot syntax: `Order.Attack(target_id)`, `Stance.Aggressive`.

Simple enums (no payloads) are a discriminant in the register, with no heap object. Enums with payloads are heap objects holding the discriminant and the payload fields.

## Optionals

`T?` is the optional type. Either `Some(value)` or `None`. Compiler-enforced -- you can't use an optional without handling the `None` case.

```raido
let shield: Shield? = find_shield(inventory)

// Null coalescing
let defense = shield ?? default_shield

// Force unwrap -- panics on None
let order = orders.get(ship.id)!

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
func bounds(arr: array<float>) -> (float, float) {
    // ...
    return (lo, hi)
}

let (lo, hi) = bounds(values)
```

Tuples are anonymous structs. `(float, float)` is a 2-field struct. The compiler generates internal type entries for tuple types -- no user-visible struct name.

## Functions as Values

A value of a function type (`func(Ship, Ship) -> bool`) is either a named function or a closure. Both are called the same way and can be stored, passed and returned.

A **closure** captures the variables it uses from around it. The rules:

- **A captured variable can't be assigned again**, inside the closure or after it in the enclosing function. Because of that rule, capturing the variable and capturing its value can't be told apart, so there's nothing about capture modes to learn: a closure sees exactly what was there when it was made.
- **Structs, arrays and maps are shared**, as everywhere in Raido. A closure that captured `ship` sees `ship.health` change, and can change it.
- **Read-only stays read-only.** A closure that captures a parameter not marked `mutate` can't change it either, so passing a struct into a function still can't change it behind your back.
- **Each loop iteration has its own variable**, so `for i in 0..3 { fs.push(|| i) }` makes closures that return 0, 1 and 2.
- **A closure can't use itself.** `let f = || f()` is an error, since `f` isn't declared until the closure is made. Recursion uses a named function.
- **Closures aren't comparable.** There's no `==` on function values.

A closure that captures nothing costs nothing: it's a prototype index in a register, the same as a named function. One that captures is a heap object holding the prototype and the captured values ([vm/architecture.md](../vm/architecture.md#heap)), and lives as long as anything refers to it: a coroutine, a struct field, a host root.

## Extern Types

The host's published API module declares the shapes it provides; scripts use them without declaring anything (see [syntax.md](syntax.md#the-hosts-api)). The compiler type-checks all access against the module.

```raido
extern struct Enemy {
    health: int
    x: float
    y: float
    readonly name: string
}

extern func move_to(entity: Enemy, target: Vec2)
extern func noise(quality: float, id: int, index: int) -> float
```

See [vm/interop.md](../vm/interop.md) for host-side binding.

## Variables

`let` declares a variable. Every variable can be reassigned and its fields changed; there's no immutable binding to learn. Assigning to a name that was never declared is a compile error.

```raido
let ship = Ship { id: 1, health: 100, x: 0.0, y: 0.0, shield: None }
ship.health -= 10   // OK
ship = other_ship   // OK
shipp = other_ship  // ERROR: `shipp` isn't declared; did you mean `ship`?
```

**Parameters are read-only unless marked `mutate`.** Same rule for `self` and for any other parameter. A `mutate` parameter changes the struct the caller passed, and the signature says so. Structs are references, so every binding to the same struct sees the change, as in Lua or Python:

```raido
func apply_damage(mutate ship: Ship, amount: int) {
    ship.health -= amount
}

let ship = spawn_ship()
apply_damage(ship, 10)      // ship.health is now 10 lower

func damaged(ship: Ship, amount: int) -> Ship {
    return Ship { health: ship.health - amount, ..ship }   // or return a copy
}
```

For host entity mutation, use extern struct fields -- the host controls the data:

```raido
extern struct Enemy { health: int, x: float, y: float }
func damage_enemy(enemy: Enemy, amount: int) {
    enemy.health -= amount  // OK: extern struct field write goes through host setter
}
```

## Arithmetic Rules

**Same-type arithmetic:**

| Expression | Result | Notes |
|-----------|--------|-------|
| `int + int` | `int` | Also `-`, `*`, `%` |
| `int / int` | `float` | Division always returns float |
| `float + float` | `float` | All ops |

**Mixed arithmetic -- int promotes to float:**

| Expression | Result | Notes |
|-----------|--------|-------|
| `int + float` | `float` | int operand converted to float |
| `float + int` | `float` | Same |
| `int CMP float` | `bool` | Promotion for comparison too |

Conversion is exact for ints within +/-2^53 (about 9 * 10^15). Beyond that the int rounds to the nearest float, as in every language with doubles.

**Narrowing requires explicit conversion:** `float -> int` always requires `int(x)`, which truncates toward zero. No implicit narrowing.

**Integer overflow:** panics by default. Use built-in wrapping methods on `int` for explicit wrapping (see [Built-in Methods](#built-in-methods)).

**Float overflow:** producing an infinity is a runtime error (see [Float](#float)).

**Modulo:** follows the sign of the dividend. `7 % 3 == 1`, `-7 % 3 == -1`.

**String comparison:** lexicographic by bytes (UTF-8 byte order). `"a" < "b"` is `true`.

## Type Rules

- Function signatures are fully typed (parameters + return type)
- Local variables inferred from initializer: `let x = 42` -> `x` is `int`
- No generics beyond built-in `array<T>`, `map<K, V>`, `T?`, tuples, and function types
- No interfaces or interfaces
- Exhaustive `match` on enums -- compiler error if a variant is missing
- `??` supplies the other branch on an optional: `value ?? fallback` (both sides same type)
- Compound assignment: `+=`, `-=`, `*=`, `/=`, `%=` on any variable, including chained access (`ships[i].health -= damage`)
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
- `substring(start: int, end: int) -> string` -- extract by byte offset, allocates
- `find(pattern: string) -> int?` -- literal substring search
- `upper() -> string`, `lower() -> string` -- ASCII only
- `split(sep: string) -> array<string>`, `trim() -> string`
- `starts_with(prefix: string) -> bool`, `ends_with(suffix: string) -> bool`
- `rep(n: int) -> string`, `byte(i: int) -> int`, `char(n: int) -> string`

## Maps

Insertion-ordered. Deterministic iteration order. Keys restricted to `string` or `int` -- types with stable, deterministic equality and hashing.

```raido
let lookup: map<string, int> = {"iron": 26, "gold": 79}
let entry = lookup.get("iron")  // returns int?
```
