<!-- id: raido.stdlib -->
<!-- status: proposed -->
<!-- summary: Minimal built-in library — math, string, table operations, no I/O -->
<!-- depends: raido/values.md, raido/syntax.md -->

# Standard Library

Minimal. No I/O, no filesystem, no networking. The host provides capabilities. Built-ins cover math, string manipulation, table operations, and type conversion.

## Core

| Function | Signature | Description |
|----------|-----------|-------------|
| `print(...)` | any... → nil | Calls host-registered print handler. No-op if none registered. |
| `type(v)` | any → string | Returns type name: `"nil"`, `"bool"`, `"int"`, `"number"`, `"string"`, `"table"`, `"function"`, `"handle"`, `"userdata"`. |
| `tostring(v)` | any → string | Convert to string representation. |
| `tonumber(v)` | any → number? | Convert to number. Returns nil on failure. |
| `toint(v)` | any → int? | Convert to int. Returns nil on failure or if fractional. |
| `error(msg)` | string → never | Raise a runtime error. |
| `pcall(f, ...)` | func, any... → bool, any | Protected call. Returns `true, result` or `false, error`. |
| `assert(v, msg?)` | any, string? → any | Raises error if v is falsy. Returns v otherwise. |
| `valid(h)` | handle → bool | Check if handle points to a live entity. |
| `remove(h)` | handle → nil | Remove entity from its pool. Handle becomes stale. |
| `handles(pool_name)` | string → iterator | Iterate all live handles in a named pool. |

```raido
print(type(42))        // "int"
print(type(3.14))      // "number"
print(type("hello"))   // "string"

const n = tonumber("42.5")  // 42.5 (number)
const i = toint("42")       // 42 (int)
const bad = toint("42.5")   // nil (fractional)

if valid(h) then
    h.health -= 10
end
```

## math

| Function | Description |
|----------|-------------|
| `math.abs(x)` | Absolute value |
| `math.floor(x)` | Floor to int |
| `math.ceil(x)` | Ceiling to int |
| `math.round(x)` | Round to nearest int |
| `math.sqrt(x)` | Square root |
| `math.sin(x)` | Sine (radians) |
| `math.cos(x)` | Cosine (radians) |
| `math.atan2(y, x)` | Two-argument arctangent |
| `math.min(a, b)` | Minimum |
| `math.max(a, b)` | Maximum |
| `math.clamp(x, lo, hi)` | Clamp x to [lo, hi] |
| `math.lerp(a, b, t)` | Linear interpolation: `a + (b - a) * t` |
| `math.random()` | Random number in [0, 1) |
| `math.random(n)` | Random int in [0, n) |
| `math.random(a, b)` | Random int in [a, b] inclusive |
| `math.pi` | π constant |
| `math.huge` | Infinity |

```raido
// Game math
const angle = math.atan2(dy, dx)
const speed = math.clamp(raw_speed, 0, max_speed)
const smooth = math.lerp(old_pos, new_pos, 0.1)
const spread = math.random() * 0.2 - 0.1
```

`clamp` and `lerp` are not in Lua's math library. They're here because game scripts use them constantly. Every game engine's Lua binding adds them — Raido includes them from the start.

## string

| Function | Description |
|----------|-------------|
| `string.len(s)` | Byte length |
| `string.sub(s, i, j?)` | Substring (0-indexed, inclusive). j defaults to end. |
| `string.find(s, pattern)` | Find first occurrence. Returns start index or nil. |
| `string.upper(s)` | Uppercase |
| `string.lower(s)` | Lowercase |
| `string.format(fmt, ...)` | Printf-style formatting |
| `string.split(s, sep)` | Split into table of substrings |
| `string.trim(s)` | Remove leading/trailing whitespace |
| `string.starts_with(s, prefix)` | Prefix check |
| `string.ends_with(s, suffix)` | Suffix check |
| `string.rep(s, n)` | Repeat string n times |
| `string.byte(s, i?)` | Byte value at position (default 0) |
| `string.char(...)` | Create string from byte values |

```raido
const name = "Goblin_Chief"
print(string.lower(name))             // "goblin_chief"
print(string.sub(name, 0, 5))         // "Goblin"
print(string.starts_with(name, "Gob")) // true

const parts = string.split("a,b,c", ",")
// parts = {"a", "b", "c"}
```

**No regex.** String patterns are plain substring matching. Regex adds complexity and arena bloat. Game scripts rarely need regex — entity names, config keys, and chat messages use simple string ops.

## table

| Function | Description |
|----------|-------------|
| `table.insert(t, v)` | Append to array part |
| `table.insert(t, i, v)` | Insert at position i (shifts elements) |
| `table.remove(t, i?)` | Remove from position i (default: last). Shifts elements. Returns removed value. |
| `table.sort(t, cmp?)` | In-place sort. Optional comparator `func(a, b) -> bool`. |
| `table.len(t)` | Array length (same as `#t`) |
| `table.keys(t)` | Return table of all keys |
| `table.values(t)` | Return table of all values |
| `table.concat(t, sep?)` | Join array elements as string. Optional separator. |
| `table.contains(t, v)` | Check if value exists in array part |

```raido
const enemies = {}
table.insert(enemies, h1)
table.insert(enemies, h2)
table.sort(enemies, func(a, b) return a.health < b.health end)

const nearest = enemies[0]
```

## bit

| Function | Description |
|----------|-------------|
| `bit.and(a, b)` | Bitwise AND |
| `bit.or(a, b)` | Bitwise OR |
| `bit.xor(a, b)` | Bitwise XOR |
| `bit.not(a)` | Bitwise NOT |
| `bit.lshift(a, n)` | Left shift |
| `bit.rshift(a, n)` | Logical right shift |

```raido
// Flag checking
const HAS_ARMOR = 0x01
const HAS_WEAPON = 0x02

if bit.and(h.flags, HAS_ARMOR) != 0 then
    damage = damage // 2
end

h.flags = bit.or(h.flags, HAS_WEAPON)
```

## Iterators

| Function | Description |
|----------|-------------|
| `pairs(t)` | Iterate all key-value pairs (hash + array) |
| `ipairs(t)` | Iterate array part: `(0, v0), (1, v1), ...` |

```raido
for k, v in pairs(config) do
    print(k .. " = " .. tostring(v))
end

for i, item in ipairs(inventory) do
    print(i, item)
end
```

`ipairs` starts at index 0 (consistent with `raido.values/T2`).

## What's NOT Included

| Missing | Why |
|---------|-----|
| File I/O | Host provides if needed. Scripts shouldn't touch the filesystem. |
| Networking | Host provides. Scripts shouldn't open connections. |
| OS access | Sandboxed. No environment variables, no process spawning. |
| Regex | Too complex for entity scripts. Plain string matching suffices. |
| JSON | Host can provide a `json_parse` function if needed. |
| `require`/modules | Host controls code loading. No script-side imports. |
| `dofile`/`loadstring` | No dynamic code execution. All code compiled by host. |
| `setmetatable` | No metatables (`raido.values/T4`). |

The stdlib is deliberately small. Every function in here allocates from the arena and runs under the instruction budget. The host adds domain-specific functions through `vm.register()`.

---

## Appendix (non-normative)

### Rationale

**No I/O:** Entity scripts on a game server are untrusted code. A goblin AI script that can open files or make HTTP requests is a security hole. The host provides exactly the capabilities scripts need and nothing more.

**`clamp`/`lerp` in math:** Lua's standard math library is missing these. Every game that embeds Lua adds them immediately. They're the most-used math operations in game scripting (smooth movement, bounded values). Including them saves every host from re-implementing them.

**`valid()`/`remove()`/`handles()` as builtins:** These are the primary ways scripts interact with the entity system. Making them builtins (rather than host-registered functions) means they're always available and can be optimized in the VM — `handles()` can iterate the pool's internal storage directly rather than going through the host function protocol.

### See Also

- `raido.values` — Type system (what `type()` returns)
- `raido.coroutines` — `wait()` helper built on `yield`
- `raido.interop` — `vm.register()` for host-provided functions
