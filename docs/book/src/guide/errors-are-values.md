# Errors are values

When a call can fail, the failure comes back out of it. Same `return`, same assignment, same
place in the line as the answer.

A function that can fail says both outcomes in its return type. `parse` returns
`i64 or ParseError`: a number, or a reason it isn't one.

```rask
{{#include ../../../../examples/errors.rk:simplest}}
```

`n` is either an `i64` or a `ParseError`. `is` tests which one it is, `as` names it, and
`else as e` binds the other.

Every error type has a `message()`, so `e.message()` works whatever kind of error arrived.

The annotation on `n` is there to show the type. You'd normally leave it off.

## Passing it up

`try` takes the value out. If the error came back instead, the function returns it, and the
caller deals with it.

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

## `??` and `catch`

`??` supplies a value when something is **missing**. A `Map` lookup returns `string?` — the
value, or nothing:

```rask
{{#include ../../../../examples/errors.rk:absence}}
```

`catch` supplies a value when something **failed**. `parse` hands back an error rather than
nothing, so it takes `catch`:

```rask
{{#include ../../../../examples/errors.rk:drop}}
```

The binder is required. `catch e =>` uses the error, `catch _ =>` throws it away. There is no
`catch 3`:

```text
{{#include ../../errors/errors-are-values/bare_catch.out}}
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
