# When there might be nothing

Some things might not be there. `string?` is the type that says so: a `string`, or nothing at
all.

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

The shorter form tests with `x?` and names the value with `as`:

```rask
{{#include ../../../../examples/optionals.rk:bind}}
```

`as found` names what the test just proved exists, and `found` is a plain `Profile` inside the
block.

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

## Chaining with `?.`

`p?.email` reads `email` when `p` is there and gives `none` when it isn't — no branch, no
binder:

```rask
{{#include ../../../../examples/optionals.rk:chain}}
```

Two things can be missing here: the profile, and the email on it. `p?.email` is a `string?` all
the same, not two layers of absence — "no profile" and "a profile with no email" are the same
answer to whoever asked for an email. So one `??` finishes the job.

A longer chain works the same way: every link short-circuits, and the end of it is still one
layer.

## Running it

The code above is one program, and it prints:

```text
{{#include ../../../../tests/golden/optionals.out}}
```

## Rules behind this page

- [Optionals](https://github.com/rask-lang/rask/blob/main/specs/types/optionals.md): `T?`, `none`, and the `?` family
- [Error types](https://github.com/rask-lang/rask/blob/main/specs/types/error-types.md): the other shape, where the second branch carries a reason
