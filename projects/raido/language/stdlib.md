<!-- id: raido.stdlib -->
<!-- status: proposed -->
<!-- summary: Core functions, built-in methods, and opt-in modules -->
<!-- depends: raido/language/types.md, raido/language/syntax.md -->

# Standard Library

Three tiers: **core functions** (always present), **built-in methods** (always present, compiler-known), and **opt-in modules** (host enables).

```rask
const vm = raido.Vm.new(raido.Config {
    stdlib: [raido.Stdlib.math, raido.Stdlib.bit],
})
```

## Core Functions

Always available. A Raido VM without these isn't a Raido VM.

`tostring(v: T) -> string` -- convert any value to string representation.

`int(x: number) -> int` -- truncate number toward zero. `int(s: string) -> int?` -- parse string to int, `None` on failure.

`number(x: int) -> number` -- promote int to fixed-point (panics if int exceeds +/-2.1B). `number(s: string) -> number?` -- parse string to number, `None` on failure.

`len(v: T) -> int` -- string byte length, array length, or map entry count. `T` must be `string`, `array`, or `map` (compiler-enforced).

`error(msg: string)` -- raises a ScriptError.

`assert(v: bool, msg: string?)` -- if `v` is false, raises ScriptError with `msg` (default: "assertion failed").

`print(v: string)` -- calls the host's print handler. Default: no-op. Host can override via `vm.set_print(handler)`.

Error catching uses `try`/`else` syntax, not a stdlib function.

## Built-in Methods

Compiler-known methods on primitive and collection types. Always available, not opt-in. See [types.md](types.md#built-in-methods) for the complete list.

User-defined structs get methods via `extend` blocks — same dot syntax, same calling convention. Libraries can provide the same ergonomics as the stdlib.

**`int`:** `wrapping_add`, `wrapping_sub`, `wrapping_mul`, `abs`

**`string`:** `len`, `sub`, `find`, `upper`, `lower`, `split`, `trim`, `starts_with`, `ends_with`, `rep`, `byte`, `char`

**`array<T>`:** `len`, `get`, `push`, `pop`, `insert`, `remove`, `sort`, `contains`, `join`, `reverse`

**`map<K, V>`:** `len`, `get`, `keys`, `values`, `contains`, `remove`

## math (opt-in)

All deterministic (fixed-point). Host enables via config.

`abs(x: number) -> number`, `floor(x: number) -> int`, `ceil(x: number) -> int`, `round(x: number) -> int`

`sqrt(x: number) -> number` -- Newton's method.

`min(a: number, b: number) -> number`, `max(a: number, b: number) -> number`

`clamp(x: number, lo: number, hi: number) -> number`, `lerp(a: number, b: number, t: number) -> number`

`sin(x: number) -> number`, `cos(x: number) -> number`, `atan2(y: number, x: number) -> number` -- CORDIC-based fixed-point approximations. ~10-bit accuracy. Good enough for game math.

`random() -> number` -- number in [0, 1). `random(n: int) -> int` -- int in [0, n). Uses VM's xoshiro128++ PRNG.

`pi: number` -- 3.14159265 as 32.32 fixed-point.

## bit (opt-in)

Bitwise operations on `int` values. Host enables via config.

`bit.and(a: int, b: int) -> int`, `bit.or(a: int, b: int) -> int`, `bit.xor(a: int, b: int) -> int`, `bit.not(a: int) -> int`

`bit.lshift(a: int, n: int) -> int`, `bit.rshift(a: int, n: int) -> int` -- logical shift (not arithmetic).

## What Hosts Add

Domain-specific functions via `extern func` declarations in scripts:

```raido
extern func spawn_enemy(kind: string, pos: Vec2) -> Enemy
extern func play_sound(name: string)
```

The host binds these at load time. See [vm/interop.md](../vm/interop.md) for host-side registration.
