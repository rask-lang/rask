<!-- id: raido.stdlib -->
<!-- status: proposed -->
<!-- summary: Core functions, math, bit and built-in methods, all always present, with exact conversion rules -->
<!-- depends: raido/language/types.md, raido/language/syntax.md -->

# Standard Library

Everything here is always present. I dropped the opt-in tiers: there's no reason for a host to deny a script `sqrt`, and every example reached for it. A host adds capabilities through its API module, not by switching stdlib parts on.

## Core Functions

`tostring(v: T) -> string` -- the text form of any value (rules below).

`int(x: float) -> int` -- truncate toward zero; out of the `int` range is `IntegerOverflow`. `int(s: string) -> int?` -- an optional `-` then decimal digits; anything else, or a value out of range, is `None`.

`float(x: int) -> float` -- nearest float, ties to even. `float(s: string) -> float?` -- an optional `-`, one or more digits, optionally `.` and one or more digits, optionally `e`, an optional sign and one or more digits (`"1.5"`, `"-2"`, `"3e-4"`; not `".5"` or `"5."`). The result is the correctly rounded nearest float, so every machine reads the same string to the same bits. A value too small rounds as usual, down to `0.0`; `"-0"` is `-0.0`. No `inf`, `nan` or hex; a value too large is `None`.

`char(n: int) -> string` -- the one-character string for an ASCII code `n` in 0..127; anything else is `ArgumentError`, so strings stay valid UTF-8.

`len(v: T) -> int` -- string byte length, array length, or map entry count.

`spawn(f, args...)` -- start a new task running `f(args...)`, handed to the host if the current run commits. See [pausing.md](pausing.md#starting-a-task).

`error(msg: string)` -- aborts the current call; its changes roll back and the host receives a ScriptError.

`assert(v: bool, msg: string?)` -- if `v` is false, raises ScriptError with `msg` (default: "assertion failed").

`print(values...)` -- any number of values of any type, converted with `tostring` and joined by single spaces, as one line to the host's print handler: `print("hp", hp, pos)` prints `hp 90 Vec2 { x: 1.0, y: 2.0 }`. It runs immediately and is a log, not an effect: a call that later fails has still printed. Default handler: no-op.


### tostring

| Value | Text |
|-------|------|
| `int` | decimal, `-` for negatives |
| `float` | the shortest decimal that reads back to the same bits, always with a `.` or an exponent: `0.1`, `3.0`, `-0.0`, `1e+21`, `5e-324`. Zero is `0.0` or `-0.0`. Otherwise plain digits when `1e-7 <= abs(x) < 1e21`, and `d.ddde±n` outside that. When two shortest decimals both read back, the one closer to the exact value wins, then the one ending in an even digit |
| `bool` | `true`, `false` |
| `string` | itself; inside a struct, tuple, array or map, quoted and escaped as a literal: `["a\n"]` |
| enum | `Order.Attack(3)`, `Stance.Aggressive` |
| `T?` | `Some(...)`, `None` |
| struct, tuple, array, map | `Ship { id: 1, health: 90 }`, `(1, 2)`, `[1, 2]`, `{"a": 1}`, with each part converted by these rules. A struct already being printed further up shows as `Ship { ... }` |
| function, extern struct | `<func>`, `<Enemy>` |

A function prints as `<func>` with no name, because names live in debug info, which isn't part of a chunk's identity: the same chunk has to print the same text with and without it.

### Formatting

String interpolation `"{x}"` uses `tostring`. A format spec after a colon controls the form: `"{hp:.1}"`, `"{n:>5}"`, `"{id:x}"`. The grammar is Rask's (`std.fmt/S1`), less what needs Unicode tables:

`{expr:[[fill]align][0][width][.precision][type]}`

- **align** is `<`, `>` or `^`, with an optional fill character before it. A `0` before the width pads numbers with zeros after the sign. With no align, numbers go right and everything else left.
- **width** is a minimum width in bytes, so it lines up ASCII text and numbers.
- **precision** is the number of decimal places for a `float`: the exact binary value rounded to that many places, ties to even, so `{2.675:.2}` is `2.67` everywhere (the double nearest 2.675 is just below it). On a string it keeps at most that many bytes, without cutting a character.
- **type** is `x` or `X` (hex), `b` (binary) or `e` (scientific, as in `tostring`) for numbers. Hex and binary show an `int`'s 64-bit pattern: `{-1:x}` is `ffffffffffffffff`.

```raido
print("{name:<12}{hp:>5}  {hp * 100.0 / max_hp:.1}%")   // "Guard          90  90.0%"
```

The spec starts at the first `:` outside brackets, parentheses and strings, so `{m["a:b"]}` has no spec. A spec that doesn't fit the value's type (`.2` on a bool) is a compile error.

## math

All correctly rounded, so the same on every machine. Called by bare name.

`floor(x: float) -> int`, `ceil(x: float) -> int`, `round(x: float) -> int` -- `round` takes ties away from zero (`round(2.5) == 3`, `round(-2.5) == -3`). Out of the `int` range is `IntegerOverflow`.

`sqrt(x: float) -> float` -- the IEEE operation.

`min(a, b)`, `max(a, b)` -- on two `int`s or two `float`s. For floats, `-0.0` counts as less than `0.0`, so `min(0.0, -0.0)` is `-0.0` whichever order the arguments come in.

`clamp(x, lo, hi)` -- `min(max(x, lo), hi)`, on `int`s or `float`s. `lerp(a: float, b: float, t: float) -> float` -- `a + (b - a) * t`, rounded at each step.

`sin`, `cos`, `atan2(y, x)`, `exp`, `log` -- correctly rounded, the same functions Rask uses.

`random() -> float` -- in [0, 1). `random(n: int) -> int` -- in [0, n); `n <= 0` is `ArgumentError`. Both draw from the VM's PRNG; [vm/architecture.md](../vm/architecture.md#prng) has the exact bit recipes.

`pi: float` -- the double nearest to pi.

## bit

Bitwise operations on `int`, as a 64-bit two's complement pattern.

`bit.and(a, b)`, `bit.or(a, b)`, `bit.xor(a, b)`, `bit.not(a)`

`bit.lshift(a, n)`, `bit.rshift(a, n)` -- logical shifts; `n` outside 0..63 is `ArgumentError`.

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
