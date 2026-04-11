<!-- id: raido.stdlib -->
<!-- status: proposed -->
<!-- summary: Configurable stdlib modules -- host opts in to what scripts can access -->
<!-- depends: raido/language/types.md, raido/language/syntax.md -->

# Standard Library

Two tiers: **core** (always present) and **modules** (host opts in).

```rask
const vm = raido.Vm.new(raido.Config {
    stdlib: [raido.Stdlib.math, raido.Stdlib.string],
})
```

## core (always available)

These are part of the language, not an opt-in module. A Raido VM without these isn't a Raido VM.

`tostring(v: T) -> string` -- convert any value to string representation.

`int(s: string) -> int?` -- parse string to int. Returns `None` on failure.

`number(s: string) -> number?` -- parse string to number. Returns `None` on failure.

`len(v: T) -> int` -- string byte length, array length, or map entry count. `T` must be `string`, `array`, or `map` (compiler-enforced).

`error(msg: string)` -- raises a ScriptError. `msg` must be a string.

`assert(v: bool, msg: string?)` -- if `v` is false, raises ScriptError with `msg` (default: "assertion failed").

`print(v: string)` -- calls the host's print handler. Default: no-op. Host can override via `vm.set_print(handler)`.

Error catching uses `try`/`else` syntax, not a stdlib function.

## math

Opt-in. All deterministic (fixed-point).

`abs(x: number) -> number`, `floor(x: number) -> int`, `ceil(x: number) -> int`, `round(x: number) -> int`

`sqrt(x: number) -> number` -- Newton's method.

`min(a: number, b: number) -> number`, `max(a: number, b: number) -> number`

`clamp(x: number, lo: number, hi: number) -> number`, `lerp(a: number, b: number, t: number) -> number`

`sin(x: number) -> number`, `cos(x: number) -> number`, `atan2(y: number, x: number) -> number` -- CORDIC-based fixed-point approximations. ~10-bit accuracy. Not scientific precision -- good enough for game math.

`random() -> number` -- number in [0, 1). `random(n: int) -> int` -- int in [0, n). Uses VM's xoshiro128++ PRNG.

`pi: number` -- 3.14159265 as 32.32 fixed-point.

## string

Opt-in. Methods on string values.

`sub(s: string, start: int, end: int?) -> string` -- substring by byte offset (0-indexed).

`find(s: string, pattern: string) -> int?` -- returns index or `None`. No regex -- literal substring search only.

`upper(s: string) -> string`, `lower(s: string) -> string` -- ASCII only. No Unicode case mapping.

`split(s: string, sep: string) -> array<string>`. `trim(s: string) -> string` -- strip ASCII whitespace.

`starts_with(s: string, prefix: string) -> bool`, `ends_with(s: string, suffix: string) -> bool`.

`rep(s: string, n: int) -> string` -- repeat string n times.

`byte(s: string, i: int) -> int` -- byte value at index. `char(n: int) -> string` -- single byte.

## array

Opt-in. Methods on array values.

`push(v: T)`, `pop() -> T?`, `insert(i: int, v: T)`, `remove(i: int) -> T`

`sort(cmp: func(T, T) -> bool)` -- stable sort. Takes a function reference as comparator.

`contains(v: T) -> bool`, `join(sep: string) -> string`, `reverse()`

`get(i: int) -> T?` -- safe access, returns `None` on out-of-bounds.

## map

Opt-in. Methods on map values.

`keys() -> array<K>`, `values() -> array<V>`

`contains(k: K) -> bool`, `remove(k: K)`

`get(k: K) -> V?` -- safe access, returns `None` on missing key.

## bit

Opt-in. Bitwise operations on `int` values.

`bit.and(a: int, b: int) -> int`, `bit.or(a: int, b: int) -> int`, `bit.xor(a: int, b: int) -> int`, `bit.not(a: int) -> int`

`bit.lshift(a: int, n: int) -> int`, `bit.rshift(a: int, n: int) -> int` -- logical shift (not arithmetic).

## What Hosts Add

Domain-specific functions via `extern func` declarations in scripts:

```raido
extern func spawn_enemy(kind: string, pos: Vec2) -> Enemy
extern func play_sound(name: string)
```

The host binds these at load time. See [vm/interop.md](../vm/interop.md) for host-side registration.
