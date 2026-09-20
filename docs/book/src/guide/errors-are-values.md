# Errors are values

When a call can fail, the failure comes back out of it.

A function that can fail says both outcomes in its return type. `parse` returns
`i64 or ParseError`: a number, or a reason it isn't one.

```rask
{{#include ../../../../examples/errors.rk:simplest}}
```

`n` is either an `i64` or a `ParseError`. `is` tests which one it is, `as` names it, and
`else as e` binds the other.

Every error type has a `message()`, so `e.message()` works whatever kind of error arrived.

The annotation on `n` is there to show the type. You'd normally leave it off.

## `catch` handles it

Branching on both outcomes is the long way round. When you only want a value out, `catch`
gives one:

```rask
{{#include ../../../../examples/errors.rk:handle}}
```

The binder is required. `catch e =>` uses the error, `catch _ =>` throws it away. There is no
`catch 3`:

```text
{{#include ../../errors/errors-are-values/bare_catch.out}}
```

A `catch` body can also leave instead of producing a value — `catch e => return wrap(e)` turns
the error into your own and hands it to the caller.

## `try` passes it up

`try r` is `r catch e => return e`: take the value out, or return the error to the caller.
That case is common enough to get a word.

```rask
{{#include ../../../../examples/errors.rk:propagate}}
```

`doubled` can't swallow the failure, so its own return type carries it: `i64 or ParseError`.

The error has to go somewhere, so the function needs an error branch to put it in. This one
doesn't have one:

```rask
{{#include ../../errors/errors-are-values/try_without_a_carrier.rk:body}}
```

```text
{{#include ../../errors/errors-are-values/try_without_a_carrier.out}}
```

## `??` is for missing, not failed

A `Map` lookup returns `string?` — the value, or nothing. Nothing isn't a failure and carries
no error to bind, so it gets `??` rather than `catch`:

```rask
{{#include ../../../../examples/errors.rk:absence}}
```

## Your own error type

`ParseError` came with `parse`. For your program's own failures, write an enum and give it a
`message()`:

```rask
{{#include ../../../../examples/errors.rk:owntype}}
```

Then it goes on the right of `or`:

```rask
{{#include ../../../../examples/errors.rk:ownuse}}
```

## Running it

The code above is one program, and it prints:

```text
{{#include ../../../../tests/golden/errors.out}}
```

## Rules behind this page

- [Error types](https://github.com/rask-lang/rask/blob/main/specs/types/error-types.md): `T or E`, `try`, `catch`, and the `Error` trait
- [Optionals](https://github.com/rask-lang/rask/blob/main/specs/types/optionals.md): `T?`, `??`, and what absence means
- [Panics](https://github.com/rask-lang/rask/blob/main/specs/control/panics.md): the other failure channel, for bugs rather than expected outcomes
