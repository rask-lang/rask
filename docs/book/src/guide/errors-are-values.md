# Errors are values

A function that can fail says so in its return type, and the failure comes back as an ordinary
value. Nothing is thrown, nothing unwinds past you, and there is no handler wrapped around the
call site waiting to catch something.

So the question at every call is the one you can already answer from the signature: what do I do
with the bad half?

## Saying it can fail

`i64 or ConfigError` is a return type. It means this call gives back a number, or it gives back a
`ConfigError` — and a caller reading the line knows that before reading the body.

```rask
{{#include ../../../../examples/errors.rk:errtype}}
```

An error type answers one question: `message()`. That's the whole obligation, and declaring it is
what lets a type stand on the right of `or`. Leave it out on an enum and you get one built from
the variant names, which is enough while a program is young.

```rask
{{#include ../../../../examples/errors.rk:fallible}}
```

There is no `Ok` or `Err` to wrap anything in. `return n` returns the number and
`return ConfigError.Missing("port")` returns the error; the compiler picks the branch from the
type of what you handed it.

## Handling it here

```rask
{{#include ../../../../examples/errors.rk:handle}}
```

`is` asks which of the two came back, `as` names it, and the `else` arm gets the other one. Both
branches are reachable and both are yours to write — there is no path where the error arrives
somewhere you didn't put it.

## Handing it up

Usually the function holding the failure isn't the one that knows what to do about it. `try`
gives it to the caller:

```rask
{{#include ../../../../examples/errors.rk:propagate}}
```

One word, and the bad half leaves. `banner` returns `string or ConfigError` because that is now
true of it: a line, or the error `port_of` produced, passed along intact.

That is also the constraint. The error has to land somewhere, so the enclosing signature needs a
place to put it. A function that returns nothing has none:

```rask
{{#include ../../errors/errors-are-values/try_without_a_carrier.rk:body}}
```

```text
{{#include ../../errors/errors-are-values/try_without_a_carrier.out}}
```

Give `report` a return type with an error branch and the same body is fine. The rule is worth
reading the other way round: a signature without an error branch is a promise that nothing
escapes this function, and `try` is how you'd break it.

## Missing is not failed

Two different things go wrong when you look something up, and Rask spells them differently.

A key that isn't in the map is **absent**. Nothing went wrong and there is nothing to report, so
`??` supplies a value and the line moves on:

```rask
{{#include ../../../../examples/errors.rk:absence}}
```

A value that is there but won't parse is a **failure**. Something did go wrong and there's a
payload saying what. That one takes `catch`:

```rask
{{#include ../../../../examples/errors.rk:drop}}
```

Both lines fall back to a default, and they read differently because they are different. The
first discards a `none`, which carried nothing. The second discards a `ParseError`, which carried
something.

## An error that dies says so

That is why `catch` has a binder and no bare-value form. `catch e =>` uses the error;
`catch _ =>` drops it. Dropping is allowed — plenty of errors deserve it — but it is a thing you
write down:

```text
{{#include ../../errors/errors-are-values/bare_catch.out}}
```

`catch _ =>` is one grep away when you come back wondering where a failure went. That's the whole
reason the underscore is mandatory.

## Running it

The code above is one program, and it prints:

```text
{{#include ../../../../tests/golden/errors.out}}
```

## Rules behind this page

- [Error types](https://github.com/rask-lang/rask/blob/main/specs/types/error-types.md): `T or E`, `try`, `catch`, and the `Error` trait
- [Optionals](https://github.com/rask-lang/rask/blob/main/specs/types/optionals.md): `T?`, `??`, and what absence means
- [Panics](https://github.com/rask-lang/rask/blob/main/specs/control/panics.md): the other failure channel, for bugs rather than expected outcomes
