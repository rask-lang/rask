# When there might be nothing

A lookup that finds nothing, a field nobody filled in, the last element of an empty list. The
type says so: `string?` is a `string` or nothing at all.

```rask
{{#include ../../../../examples/optionals.rk:type}}
```

`name` is always there. `email` might not be, and the `?` is the whole difference. Nothing else
in the program can forget it — there is no path where an absent `email` arrives looking like a
string.

## Getting at the value

`match` takes both cases:

```rask
{{#include ../../../../examples/optionals.rk:simplest}}
```

For two cases that's more writing than it needs, and `rask lint` will say so. The shorter form
tests with `x?` and names the value with `as`:

```rask
{{#include ../../../../examples/optionals.rk:bind}}
```

`as found` names what the test just proved exists, and `found` is a plain `Profile` inside the
block. The same `as` shows up after every test that proves a value is there, so it reads the
same way each time.

Without `as`, `x?` is just a `bool`:

```rask
{{#include ../../../../examples/optionals.rk:test}}
```

## `??` supplies the other branch

Most of the time you don't want to branch, you want a value either way. `??` gives the right
side when the left is absent:

```rask
{{#include ../../../../examples/optionals.rk:fallback}}
```

The right side can also leave instead of producing a value — `?? return`, `?? break`,
`?? continue`. There's no binder, because there's nothing to bind: absence carries no payload.

That's also why a failure takes a different word. `catch` names or drops an error, and `none`
isn't one:

```text
{{#include ../../errors/when-there-might-be-nothing/catch_on_an_optional.out}}
```

## Reaching through

`?.` reads a field when there's something to read it from, and gives `none` when there isn't:

```rask
{{#include ../../../../examples/optionals.rk:chain}}
```

`p` might be absent and `p.email` might be absent, and `p?.email` is one `string?` rather than
two layers of absence — both mean the same thing to whoever reads it. So a single `??` finishes
the job.

## Running it

The code above is one program, and it prints:

```text
{{#include ../../../../tests/golden/optionals.out}}
```

## Rules behind this page

- [Optionals](https://github.com/rask-lang/rask/blob/main/specs/types/optionals.md): `T?`, `none`, and the `?` family
- [Error types](https://github.com/rask-lang/rask/blob/main/specs/types/error-types.md): the other shape, where the second branch carries a reason
