<!-- id: raido.stdlib -->
<!-- status: proposed -->
<!-- summary: Minimal built-in library — math, string, array, map, bit. No I/O. -->
<!-- depends: raido/values.md, raido/syntax.md -->

# Standard Library

No I/O, no filesystem, no networking, no modules, no `loadstring`. Host provides capabilities.

## Core

`print(...)`, `type(v)`, `tostring(v)`, `tonumber(v)`, `toint(v)`, `error(msg)`, `pcall(f, ...)`, `assert(v, msg?)`, `valid(h)`, `remove(h)`, `handles(pool_name)`

## math

`abs`, `floor`, `ceil`, `round`, `sqrt`, `sin`, `cos`, `atan2`, `min`, `max`, `clamp`, `lerp`, `random`, `pi`, `huge`

```raido
const speed = math.clamp(raw_speed, 0, max_speed)
const smooth = math.lerp(old_pos, new_pos, 0.1)
```

`clamp` and `lerp` included because every game scripting setup needs them.

## string

`len`, `sub`, `find`, `upper`, `lower`, `split`, `trim`, `starts_with`, `ends_with`, `rep`, `byte`, `char`

No regex. No `string.format` — string interpolation covers it.

## array

Methods on array values: `push`, `pop`, `insert`, `remove`, `sort`, `contains`, `join`, `reverse`

```raido
let enemies = []
enemies.push(h1)
enemies.sort(|a, b| a.health < b.health)
```

## map

Methods on map values: `keys`, `values`, `contains`, `remove`

## bit

`bit.and`, `bit.or`, `bit.xor`, `bit.not`, `bit.lshift`, `bit.rshift`
