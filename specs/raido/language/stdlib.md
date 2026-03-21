<!-- id: raido.stdlib -->
<!-- status: proposed -->
<!-- summary: Configurable stdlib modules — host opts in to what scripts can access -->
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

`type(v)` — returns type name as string ("nil", "bool", "int", "number", "string", "array", "map", "function", "host_ref").

`tostring(v)` — converts any value to string.

`tonumber(v)` — string/int → number. Returns nil on failure.

`toint(v)` — string/number → int (truncates). Returns nil on failure.

`len(v)` — string byte length, array length, or map entry count. TypeError on other types.

`error(msg)` — raises a ScriptError. `msg` must be a string.

`assert(v, msg?)` — if `v` is falsy, raises ScriptError with `msg` (default: "assertion failed").

`print(v...)` — calls the host's print handler. Default: no-op. Host can override via `vm.set_print(handler)`.

Error catching uses `try`/`else` syntax, not a stdlib function.

## math

Opt-in. All deterministic (fixed-point).

`abs`, `floor`, `ceil`, `round`, `sqrt`, `min`, `max`, `clamp`, `lerp`

`sin`, `cos`, `atan2` — CORDIC-based fixed-point approximations. ~10-bit accuracy. Not scientific precision — good enough for game math.

`random()` → number in [0, 1). `random(n)` → int in [0, n). Uses VM's xoshiro128++ PRNG.

`pi` — 3.14159265 as 32.32 fixed-point.

## string

Opt-in.

`sub(s, start, end?)` — substring by byte offset (0-indexed). `find(s, pattern)` — returns index or nil. No regex — literal substring search only.

`upper(s)`, `lower(s)` — ASCII only. No Unicode case mapping (keeps implementation tiny).

`split(s, sep)` → array. `trim(s)` — strip ASCII whitespace.

`starts_with(s, prefix)`, `ends_with(s, suffix)` → bool.

`rep(s, n)` — repeat string n times. `byte(s, i)` → int (byte value at index). `char(n)` → string (single byte).

## array

Opt-in. Methods on array values.

`push(v)`, `pop()`, `insert(i, v)`, `remove(i)`

`sort(cmp?)` — insertion sort (stable, simple, fast for small arrays typical in scripts). `cmp` is an optional comparison function.

`contains(v)` → bool. `join(sep)` → string. `reverse()`.

## map

Opt-in. Methods on map values.

`keys()` → array. `values()` → array. `contains(k)` → bool. `remove(k)`.

## bit

Opt-in. Bitwise operations on `int` values.

`bit.and(a, b)`, `bit.or(a, b)`, `bit.xor(a, b)`, `bit.not(a)`

`bit.lshift(a, n)`, `bit.rshift(a, n)` — logical shift (not arithmetic).

## What Hosts Add

Domain-specific functions via `vm.register()`:

```rask
vm.register("spawn_enemy", |ctx| { ... })
vm.register("play_sound", |ctx| { ... })
vm.register("send_email", |ctx| { ... })
```

The VM is a blank slate beyond core. The host shapes the environment.
