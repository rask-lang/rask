<!-- id: raido.syntax -->
<!-- status: proposed -->
<!-- summary: Raido language syntax — Lua-inspired with Rask-flavored adjustments -->
<!-- depends: raido/values.md -->

# Syntax

Lua-inspired with targeted adjustments: `func` not `function`, 0-indexed arrays, newline-terminated statements.

## Lexical Structure

| Rule | Description |
|------|-------------|
| **L1: Newline-terminated** | Statements end at newlines. No semicolons. Semicolons allowed but optional. |
| **L2: Comments** | `--` for line comments. `--[[ ]]--` for block comments. |
| **L3: Identifiers** | `[a-zA-Z_][a-zA-Z0-9_]*`. Case-sensitive. |
| **L4: Keywords** | `and`, `break`, `case`, `const`, `do`, `else`, `elseif`, `end`, `false`, `for`, `func`, `if`, `in`, `local`, `match`, `nil`, `not`, `or`, `repeat`, `return`, `then`, `true`, `until`, `while`, `yield` |
| **L5: Strings** | Double-quoted `"hello"`, single-quoted `'hello'`, long strings `[[ ... ]]`. Escape sequences: `\n`, `\t`, `\\`, `\"`, `\'`. |
| **L6: Numbers** | `42` (int), `3.14` (number), `0xff` (hex int), `0b1010` (binary int), `1e6` (number). Underscores allowed: `1_000_000`. |

```raido
-- This is a comment
--[[ This is a
     block comment ]]--

name = "Raido"
count = 1_000
mask = 0xff
```

## Variables

| Rule | Description |
|------|-------------|
| **V1: Global by default** | Bare assignment creates/updates a global: `x = 42`. |
| **V2: Local** | `local x = 42` creates a block-scoped local. |
| **V3: Const local** | `const x = 42` creates an immutable block-scoped local. |
| **V4: Multiple assignment** | `a, b = 1, 2` and `local a, b = foo()` for multiple returns. |
| **V5: Nil default** | Uninitialized locals are `nil`. |

```raido
x = 42             -- global
local y = 10       -- mutable local
const z = 100      -- immutable local

a, b, c = 1, 2, 3  -- multiple assignment
local x, y = position()  -- capture multiple returns
```

`const` is borrowed from Rask. Lua doesn't have `const` locals (until 5.4's `<const>` attribute, which is clunky). Game config values and cached references should be immutable — `const` makes that explicit without ceremony.

## Functions

| Rule | Description |
|------|-------------|
| **F1: Declaration** | `func name(params) ... end`. No `function` keyword. |
| **F2: Anonymous** | `func(params) ... end` as expression. |
| **F3: Short form** | `func(x) return x * 2 end` for single-expression lambdas (no special syntax, just a short body). |
| **F4: Multiple returns** | Functions can return multiple values: `return a, b`. |
| **F5: Variadic** | `func f(...)` accepts variable arguments. `...` expands them. |
| **F6: First-class** | Functions are values. Can be assigned, passed, returned, stored in tables. |
| **F7: Closures** | Functions capture enclosing locals by reference (like Lua upvalues). |

```raido
func greet(name)
    return "Hello, " .. name
end

-- Anonymous
const double = func(x) return x * 2 end

-- Multiple returns
func minmax(a, b)
    if a < b then
        return a, b
    end
    return b, a
end

local lo, hi = minmax(5, 3)

-- Variadic
func sum(...)
    local total = 0
    for _, v in ipairs({...}) do
        total = total + v
    end
    return total
end

-- Closure
func counter(start)
    local n = start
    return func()
        n = n + 1
        return n
    end
end
```

## Control Flow

| Rule | Description |
|------|-------------|
| **C1: If/elseif/else** | `if cond then ... elseif cond then ... else ... end` |
| **C2: While** | `while cond do ... end` |
| **C3: Repeat-until** | `repeat ... until cond` (loop body runs at least once) |
| **C4: Numeric for** | `for i = start, stop do ... end` and `for i = start, stop, step do ... end`. Inclusive upper bound. |
| **C5: Generic for** | `for k, v in iter do ... end` with iterator functions. |
| **C6: Break** | `break` exits the innermost loop. |
| **C7: Match** | `match expr case pattern then ... case pattern then ... end` for value-based branching. |

```raido
-- If
if health <= 0 then
    die()
elseif health < 20 then
    warn("low health")
else
    fight()
end

-- While
while queue_size() > 0 do
    process(dequeue())
end

-- Numeric for (inclusive)
for i = 0, 9 do
    print(i)  -- 0 through 9
end

for i = 10, 0, -1 do
    print(i)  -- countdown
end

-- Generic for
for k, v in pairs(config) do
    print(k, v)
end

for i, item in ipairs(inventory) do
    print(i, item)
end

-- Match
match state
    case "idle" then
        wait()
    case "patrol" then
        move_to(next_waypoint())
    case "chase" then
        move_toward(target)
    case _ then
        error("unknown state: " .. state)
end
```

**C4 (inclusive upper bound):** Lua uses inclusive bounds for numeric for — `for i = 1, 10` runs 10 times. Raido keeps this despite 0-indexing. `for i = 0, 9` runs 10 times. The alternative (exclusive upper bound like Python's `range`) would be more consistent with 0-indexing but less familiar to Lua users. Inclusive wins on familiarity — game scripters write `for i = 0, #enemies - 1`.

**C7 (match):** A Rask influence. Game AI is full of state machines — `match state` is cleaner than if/elseif chains. Match only supports value equality (strings, numbers, bools), not pattern destructuring. This isn't a full pattern matching system.

## Operators

| Precedence | Operators | Associativity |
|-----------|-----------|---------------|
| 1 (highest) | `not`, `-` (unary), `#` (length) | Right |
| 2 | `^` (power) | Right |
| 3 | `*`, `/`, `//`, `%` | Left |
| 4 | `+`, `-` | Left |
| 5 | `..` (concat) | Right |
| 6 | `<`, `>`, `<=`, `>=`, `==`, `~=` | Left |
| 7 | `and` | Left |
| 8 (lowest) | `or` | Left |

| Rule | Description |
|------|-------------|
| **O1: String concat** | `..` concatenates strings. Coerces numbers to strings. |
| **O2: Length** | `#t` returns array length of table (highest contiguous 0-based index + 1). `#s` returns string byte length. |
| **O3: Equality** | `==` and `~=`. Tables/functions compared by reference. Strings by value. Int/number by mathematical value. |
| **O4: Logical** | `and`/`or` return operand values (short-circuit), not just bools. `not` always returns bool. |
| **O5: No bitwise operators** | Use `bit.and()`, `bit.or()`, `bit.xor()`, `bit.lshift()`, `bit.rshift()`. Keeps operator count small. |

```raido
-- Concat
name = "player" .. "_" .. tostring(id)

-- Length
items = {10, 20, 30}
print(#items)  -- 3

-- Logical (returns operand, not bool)
default = config.timeout or 30
name = player and player.name or "unknown"
```

**O5 (no bitwise operators):** Lua 5.3 added `&`, `|`, `~`, `<<`, `>>`. I chose stdlib functions instead — bitwise ops are uncommon in game scripts, and adding 5+ operators to the grammar for niche use isn't worth it. The `bit` stdlib module covers the need.

## Table Constructors

| Rule | Description |
|------|-------------|
| **TC1: Array part** | `{a, b, c}` — values get sequential 0-based integer keys. |
| **TC2: Hash part** | `{key = value}` — string keys. |
| **TC3: Computed keys** | `{[expr] = value}` — arbitrary expression as key. |
| **TC4: Mixed** | Array and hash parts can be mixed: `{"warrior", level = 5}`. |
| **TC5: Trailing comma** | Optional trailing comma allowed. |

```raido
-- Array
colors = {"red", "green", "blue"}

-- Hash
point = { x = 10, y = 20 }

-- Computed key
lookup = { [get_key()] = true }

-- Mixed
entity = {
    "enemy",
    type = "goblin",
    health = 50,
    pos = { x = 0, y = 0 },
}
```

## Method-Style Calls

| Rule | Description |
|------|-------------|
| **MC1: Colon syntax** | `obj:method(args)` is sugar for `obj.method(obj, args)`. |

```raido
-- These are equivalent:
player.take_damage(player, 10)
player:take_damage(10)
```

This is pure Lua compatibility. Game scripts use this constantly for entity behaviors defined in tables. It's just syntactic sugar — Raido has no classes or inheritance.

## Edge Cases

| Case | Rule | Behavior |
|------|------|----------|
| Multi-line expression | L1 | Lines ending with binary operator or `,` continue to next line. |
| `return` not at block end | F4 | Runtime error — `return` must be the last statement in a block. |
| `for i = 0, -1 do` | C4 | Loop body doesn't execute (start > stop with positive step). |
| Numeric for with float step | C4 | Allowed: `for x = 0.0, 1.0, 0.1 do`. Beware float precision. |
| Calling non-function | F6 | Runtime error: "attempt to call a nil/number/... value". |
| `#` on hash-only table | O2 | Returns 0 (no array part). |

---

## Appendix (non-normative)

### Rationale

**L1 (newline-terminated):** Matches Rask. Lua uses optional semicolons and newlines aren't significant — but in practice, every Lua statement is one per line anyway. Making newlines significant removes ambiguity.

**F1 (`func` not `function`):** `function` is 8 characters of noise repeated hundreds of times in a game script. `func` is 4. Matches Rask. The Lua community informally abbreviates it already.

**V3 (`const`):** Lua 5.4 added `<const>` attributes: `local x <const> = 42`. That's ugly. Rask uses `const` as a keyword. Raido follows Rask — it's cleaner and game config values should be immutable by default.

### Grammar Sketch (EBNF)

```ebnf
program     = { statement }
statement   = assignment | local_decl | const_decl | func_decl
            | if_stmt | while_stmt | repeat_stmt | for_stmt
            | match_stmt | return_stmt | break_stmt | call_stmt

assignment  = var_list '=' expr_list
local_decl  = 'local' name_list ['=' expr_list]
const_decl  = 'const' name_list '=' expr_list
func_decl   = 'func' name '(' [param_list] ')' block 'end'

if_stmt     = 'if' expr 'then' block {'elseif' expr 'then' block} ['else' block] 'end'
while_stmt  = 'while' expr 'do' block 'end'
repeat_stmt = 'repeat' block 'until' expr
for_stmt    = 'for' name '=' expr ',' expr [',' expr] 'do' block 'end'
            | 'for' name_list 'in' expr_list 'do' block 'end'
match_stmt  = 'match' expr { 'case' pattern 'then' block } 'end'
return_stmt = 'return' [expr_list]
break_stmt  = 'break'
call_stmt   = prefix_expr call_args

block       = { statement }
expr        = unary_op expr | expr binary_op expr | primary
primary     = 'nil' | 'true' | 'false' | number | string
            | name | prefix_expr | table_ctor | func_expr
prefix_expr = name | prefix_expr '.' name | prefix_expr '[' expr ']'
            | prefix_expr call_args | prefix_expr ':' name call_args
call_args   = '(' [expr_list] ')'
table_ctor  = '{' [field_list] '}'
func_expr   = 'func' '(' [param_list] ')' block 'end'
```

### See Also

- `raido.values` — Type system and value representation
- `raido.coroutines` — `yield` keyword semantics
- `raido.stdlib` — Built-in functions (`print`, `type`, `pairs`, `ipairs`)
