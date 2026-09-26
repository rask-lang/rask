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

## `!` stops the program

The last thing the error arm can do is give up. `!` takes the value out, and panics if the
error came back instead:

```rask
{{#include ../../panics/errors-are-values/force_on_an_error.rk:body}}
```

```text
{{#include ../../panics/errors-are-values/force_on_an_error.out}}
```

A panic is not an error value. It has no type, it never appears in a return type, and a caller
can't handle it. The task dies, its `ensure` cleanups run on the way out, and any locks it held
are released.

Use `!` where there is no sensible way to carry on — a config the program can't start without.
For anything the caller could deal with, hand it back with `try`.

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

- [Error types](https://github.com/rask-lang/rask/blob/main/specs/types/error-types.md): `T or E`, `try`, `catch`, and the `Error` interface
- [Panics](https://github.com/rask-lang/rask/blob/main/specs/control/panics.md): the other failure channel, for bugs rather than expected outcomes
