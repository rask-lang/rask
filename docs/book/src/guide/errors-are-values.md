# Errors are values

When a call can fail, the failure comes back out of it. Same `return`, same assignment, same
place in the line as the answer. Nothing is thrown, nothing unwinds past you, and there is no
handler wrapped around the call waiting to catch something.

So a function that can fail says both halves in its return type. `parse` says
`i64 or ParseError`: a number, or a reason it isn't one.

```rask
{{#include ../../../../examples/errors.rk:simplest}}
```

`n` holds one of the two. `is` asks which one arrived, `as` names it, and the `else` arm gets
the other. Both branches are reachable and both are yours to write — there is no path where the
failure turns up somewhere you didn't put it.

Two smaller things in there. `e.message()` works without knowing what kind of error arrived,
because answering `message()` is what makes a type usable as the failure half. And the
annotation on `n` is only there to show the shape; you would normally let it be inferred.

## Handing it up

Usually the function that hit the failure isn't the one that knows what to do about it. `try`
takes the value out, and on the bad branch leaves — the failure goes to *your* caller:

```rask
{{#include ../../../../examples/errors.rk:propagate}}
```

`doubled` returns `i64 or ParseError` because that is now true of it: a number, or the failure
`parse` produced, passed along intact.

The error has to land somewhere, so the enclosing signature needs a place to put it. A function
that returns nothing has none:

```rask
{{#include ../../errors/errors-are-values/try_without_a_carrier.rk:body}}
```

```text
{{#include ../../errors/errors-are-values/try_without_a_carrier.out}}
```

Worth reading the other way round: a signature with no error branch is a promise that nothing
escapes this function, and `try` is how you would break it.

## Missing is not failed

Two different things go wrong when you look something up, and they get different words.

A key that isn't in the map is **absent**. Nothing went wrong and there is nothing to report, so
`??` supplies a value and the line carries on:

```rask
{{#include ../../../../examples/errors.rk:absence}}
```

A value that is there but won't parse **failed**. Something did go wrong, and there's a payload
saying what. That takes `catch`:

```rask
{{#include ../../../../examples/errors.rk:drop}}
```

Both lines fall back to a default, and they read differently because they are different. The
first discards a `none`, which carried nothing. The second discards a `ParseError`, which
carried something.

## An error that dies says so

Which is why `catch` takes a binder and has no bare-value form. `catch e =>` uses the error,
`catch _ =>` drops it. Dropping is allowed — plenty of failures deserve it — but it is a thing
you write down:

```text
{{#include ../../errors/errors-are-values/bare_catch.out}}
```

So `catch _ =>` is one grep away when you come back later wondering where a failure went.

## Your own errors

`ParseError` came with `parse`. When the failures belong to your program, declare them: an error
type is any type that answers `message()`.

```rask
{{#include ../../../../examples/errors.rk:owntype}}
```

```rask
{{#include ../../../../examples/errors.rk:ownuse}}
```

Both words from the last section show up one function apart: `??` for the key that might not be
there, `catch` for the value that might not parse. Each turns into the error this program wants
its caller to see.

## Running it

The code above is one program, and it prints:

```text
{{#include ../../../../tests/golden/errors.out}}
```

## Rules behind this page

- [Error types](https://github.com/rask-lang/rask/blob/main/specs/types/error-types.md): `T or E`, `try`, `catch`, and the `Error` trait
- [Optionals](https://github.com/rask-lang/rask/blob/main/specs/types/optionals.md): `T?`, `??`, and what absence means
- [Panics](https://github.com/rask-lang/rask/blob/main/specs/control/panics.md): the other failure channel, for bugs rather than expected outcomes
