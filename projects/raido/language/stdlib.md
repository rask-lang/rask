<!-- id: raido.stdlib -->
<!-- status: proposed -->
<!-- summary: Core functions, built-in methods, and opt-in modules -->
<!-- depends: raido/language/types.md, raido/language/syntax.md -->

# Standard Library

Three tiers: **core functions** (always present), **built-in methods** (always present, compiler-known), and **opt-in modules** (host enables).

```rask
let vm = raido.Vm.new(raido.Config {
    stdlib: [raido.Stdlib.math, raido.Stdlib.bit],
})
```

## Core Functions

Always available. A Raido VM without these isn't a Raido VM.

`tostring(v: T) -> string` -- convert any value to string representation.

`int(x: float) -> int` -- truncate float toward zero. `int(s: string) -> int?` -- parse string to int, `None` on failure.

`float(x: int) -> float` -- convert int to float. `float(s: string) -> float?` -- parse string to float, `None` on failure.

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

All deterministic: correctly rounded, so the same on every machine. Host enables via config.

`abs(x: float) -> float`, `floor(x: float) -> int`, `ceil(x: float) -> int`, `round(x: float) -> int`

`sqrt(x: float) -> float` -- the IEEE operation, correctly rounded.

`min(a: float, b: float) -> float`, `max(a: float, b: float) -> float`

`clamp(x: float, lo: float, hi: float) -> float`, `lerp(a: float, b: float, t: float) -> float`

`sin(x: float) -> float`, `cos(x: float) -> float`, `atan2(y: float, x: float) -> float` -- correctly rounded, the same functions Rask uses.

`random() -> float` -- float in [0, 1). `random(n: int) -> int` -- int in [0, n). Uses VM's xoshiro128++ PRNG.

`pi: float` -- the double nearest to pi.

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
