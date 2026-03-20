<!-- id: raido.stdlib -->
<!-- status: proposed -->
<!-- summary: Configurable stdlib modules — host opts in to what scripts can access -->
<!-- depends: raido/values.md, raido/syntax.md -->

# Standard Library

Modular. Host chooses which modules to enable. Nothing loaded by default.

```rask
const vm = raido.Vm.new(raido.Config {
    stdlib: [raido.Stdlib.core, raido.Stdlib.math, raido.Stdlib.string],
})
```

## core

Always-available primitives (not opt-in — these are the language):

`type(v)`, `tostring(v)`, `tonumber(v)`, `toint(v)`, `error(msg)`, `pcall(f, ...)`, `assert(v, msg?)`

## math

`abs`, `floor`, `ceil`, `round`, `sqrt`, `sin`, `cos`, `atan2`, `min`, `max`, `clamp`, `lerp`, `random`, `pi`

All deterministic (fixed-point). `random` uses the VM's seedable PRNG.

## string

`len`, `sub`, `find`, `upper`, `lower`, `split`, `trim`, `starts_with`, `ends_with`, `rep`, `byte`, `char`

No regex. No `format` — string interpolation covers it.

## array

Methods on array values: `push`, `pop`, `insert`, `remove`, `sort`, `contains`, `join`, `reverse`

## map

Methods on map values: `keys`, `values`, `contains`, `remove`

## bit

`bit.and`, `bit.or`, `bit.xor`, `bit.not`, `bit.lshift`, `bit.rshift`

## What Hosts Add

Domain-specific functions via `vm.register()`:

```rask
// Game server
vm.register("spawn_enemy", |ctx| { ... })
vm.register("play_sound", |ctx| { ... })

// Workflow engine
vm.register("send_email", |ctx| { ... })
vm.register("wait_for_approval", |ctx| { ... })

// Rule engine
vm.register("lookup_rate", |ctx| { ... })
vm.register("log_decision", |ctx| { ... })
```

The VM is a blank slate. The host shapes the environment.
