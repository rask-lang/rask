<!-- id: raido.stdlib -->
<!-- status: proposed -->
<!-- summary: Core functions, math, bit and built-in methods, all always present, with exact conversion rules -->
<!-- depends: raido/language/types.md, raido/language/syntax.md -->

# Standard Library

Everything here is always present. I dropped the opt-in tiers: there's no reason for a host to deny a script `sqrt`, and every example reached for it. A host adds capabilities through its API module, not by switching stdlib parts on.

## Core Functions

`tostring(v: T) -> string` -- the text form of any value (rules below).

`int(x: float) -> int` -- truncate toward zero; out of the `int` range is `IntegerOverflow`. `int(s: string) -> int?` -- an optional `-` then decimal digits; anything else, or a value out of range, is `None`.

`float(x: int) -> float` -- nearest float, ties to even. `float(s: string) -> float?` -- an optional `-`, digits, an optional `.` and digits, an optional `e` and signed digits. The result is the correctly rounded nearest float, so every machine reads the same string to the same bits. No `inf`, `nan` or hex; a value too large is `None`.

`char(n: int) -> string` -- the one-byte string for `n` in 0..255.

`len(v: T) -> int` -- string byte length, array length, or map entry count.

`error(msg: string)` -- aborts the current call; its changes roll back and the host receives a ScriptError.

`assert(v: bool, msg: string?)` -- if `v` is false, raises ScriptError with `msg` (default: "assertion failed").

`print(v: string)` -- calls the host's print handler, immediately. It's a log, not an effect: a call that later fails has still printed. Default handler: no-op.


### tostring

| Value | Text |
|-------|------|
| `int` | decimal, `-` for negatives |
| `float` | the shortest decimal that reads back to the same bits, always with a `.` or an exponent: `0.1`, `3.0`, `-0.0`, `1e+21`, `5e-324`. Plain digits when `1e-7 <= abs(x) < 1e21`, otherwise `d.ddde±n` |
| `bool` | `true`, `false` |
| `string` | itself |
| enum | `Order.Attack(3)`, `Stance.Aggressive` |
| `T?` | `Some(...)`, `None` |
| struct, tuple, array, map | `Ship { id: 1, health: 90 }`, `(1, 2)`, `[1, 2]`, `{"a": 1}`, with each part converted by these rules. A struct already being printed further up shows as `Ship { ... }` |
| function, extern struct | `<func name>`, `<Enemy>` |

String interpolation `"{x}"` uses `tostring`.

## math

All correctly rounded, so the same on every machine. Called by bare name.

`floor(x: float) -> int`, `ceil(x: float) -> int`, `round(x: float) -> int` -- `round` takes ties away from zero (`round(2.5) == 3`, `round(-2.5) == -3`). Out of the `int` range is `IntegerOverflow`.

`sqrt(x: float) -> float` -- the IEEE operation.

`min(a, b)`, `max(a, b)` -- on two `int`s or two `float`s. For floats, `-0.0` counts as less than `0.0`, so `min(0.0, -0.0)` is `-0.0` whichever order the arguments come in.

`clamp(x, lo, hi)` -- `min(max(x, lo), hi)`, on `int`s or `float`s. `lerp(a: float, b: float, t: float) -> float` -- `a + (b - a) * t`, rounded at each step.

`sin`, `cos`, `atan2(y, x)`, `exp`, `log` -- correctly rounded, the same functions Rask uses.

`random() -> float` -- in [0, 1). `random(n: int) -> int` -- in [0, n); `n <= 0` is an error. Both draw from the VM's PRNG; [vm/architecture.md](../vm/architecture.md#prng) has the exact bit recipes.

`pi: float` -- the double nearest to pi.

## bit

Bitwise operations on `int`, as a 64-bit two's complement pattern.

`bit.and(a, b)`, `bit.or(a, b)`, `bit.xor(a, b)`, `bit.not(a)`

`bit.lshift(a, n)`, `bit.rshift(a, n)` -- logical shifts; `n` outside 0..63 is an error.

## Built-in Methods

Compiler-known methods on primitive and collection types. [types.md](types.md#built-in-methods) has the signatures.

**`int`:** `wrapping_add`, `wrapping_sub`, `wrapping_mul`, `abs`

**`float`:** `abs`

**`string`:** `len`, `substring`, `find`, `upper`, `lower`, `split`, `trim`, `starts_with`, `ends_with`, `rep`, `byte`

**`array<T>`:** `len`, `get`, `push`, `pop`, `insert`, `remove`, `sort`, `filter`, `contains`, `join`, `reverse`

**`map<K, V>`:** `len`, `get`, `keys`, `values`, `contains`, `remove`; writes are `m[k] = v`

User-defined structs get methods via `extend` blocks, same dot syntax.

## What Hosts Add

Domain functions come from the host's published API module, which every script sees already imported:

```raido
// game_api.rd
extern func spawn_enemy(kind: string, pos: Vec2) -> Enemy
extern deferred func play_sound(name: string)
```

See [vm/interop.md](../vm/interop.md#the-published-api).
