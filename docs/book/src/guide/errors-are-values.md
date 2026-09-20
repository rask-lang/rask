# Errors are values

When a call can fail, the failure comes back out of it.

A function that can fail says both outcomes in its return type. `parse` returns
`i64 or ParseError`: a number, or a reason it isn't one. `match` takes both:

```rask
{{#include ../../../../examples/errors.rk:simplest}}
```

One arm per outcome, each naming what it got.

Every error type has a `message()`, so `e.message()` works whatever kind of error arrived.

The annotation on `n` is there to show the type. You'd normally leave it off.

## `catch` writes the error arm

Most of the time the error arm is just "use this value instead". `catch` is that arm on its
own, and the success value comes straight out:

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

## `try` writes the commonest one

The error arm people write most is "hand it to my caller unchanged". `try r` does what
`r catch e => return e` does:

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
- [Panics](https://github.com/rask-lang/rask/blob/main/specs/control/panics.md): the other failure channel, for bugs rather than expected outcomes
