# Passing Values

Every parameter answers one question: **does the caller still have this afterwards?**
There are three answers, and the code says which one at both ends.

## Borrow: the default

```rask
{{#include ../../../../examples/parameter_modes.rk:borrow}}
```

No marker, no ceremony. The callee reads; the caller keeps the value and carries on using it. This
is most parameters in most programs, which is why it's the default.

A borrow isn't yours to give away. Try to pass it on to something that takes ownership and the
compiler stops you:

```text
{{#include ../../errors/passing-values/consume_borrowed.out}}
```

The caller never marked this as given, so they're still using it. Consuming it here would leave
them holding something that's gone. For a file handle or a transaction, that's a second close of a
real resource.

## Mutate: write through, caller keeps it

```rask
{{#include ../../../../examples/parameter_modes.rk:mutate}}
```

The call site marks it too, and that's not optional:

```rask
{{#include ../../../../examples/parameter_modes.rk:mutatecall}}
```

Leave the marker off and you get the one-token fix, plus the reason:

```text
{{#include ../../errors/passing-values/missing_marker.out}}
```

Here's the thinking behind that. If you misread a *move*, the compiler catches it for you: use the
value again and you get an error naming where it went. If you misread a *mutation*, nothing catches
it. Both readings are legal code, and the value looks the same afterwards, just different. So the
case nobody can catch for you is the one you write down.

Because the rule is syntactic it has no exceptions to memorise: the marker is required exactly when
the parameter says `mutate`, whatever the argument's type or size. An `i64` writes it too.

One thing catches everyone once. `let` is deep:

```text
{{#include ../../errors/passing-values/let_as_mutate.out}}
```

`let` doesn't mean "this name won't be reassigned." It means nothing changes through this name,
including through a mutating method, an index, or a field assignment.

## Take: the callee keeps it

```rask
{{#include ../../../../examples/parameter_modes.rk:take}}
```

No marker needed, though `own account` is available when you want the call site to shout. After the
call the name is gone:

```text
{{#include ../../errors/passing-values/use_after_take.out}}
```

That note is the whole reason `take` needs no marker: the compiler will tell you exactly where the
value went, the moment you reach for it again.

## Receivers are never marked

```rask
{{#include ../../../../examples/parameter_modes.rk:receiver}}
```

```rask
{{#include ../../../../examples/parameter_modes.rk:receivercall}}
```

`charge_fee` takes `mutate self` and the call site still says nothing. The receiver is the thing
being operated on, which is what the dot means. Marking it would put noise on every mutating method
in the language.

## Putting it together

```rask
{{#include ../../../../examples/parameter_modes.rk:callsite}}
```

Read the markers and you know the shape of that block without opening a single signature: two
borrows, one mutation, one method on the receiver, one hand-off. That's what making them visible
buys. Running it prints:

```text
{{#include ../../../../tests/golden/parameter_modes.out}}
```

## Small values are copied, not given

```rask
{{#include ../../../../examples/parameter_modes.rk:copyfn}}
```

```rask
{{#include ../../../../examples/parameter_modes.rk:copy}}
```

`take` means "I'm keeping this", but you can't take away what the caller never gave up. Values of
16 bytes or less whose fields are all Copy get copied on the way in, so `fee` is untouched. Past 16
bytes it's a real move and the name dies, which is what the `Account` error above shows, sizes and
all.

The threshold is fixed at 16 bytes and isn't configurable. Moving it would change what existing
programs mean, so it's a semantic boundary rather than a tuning knob.

## Rules behind this page

- [Parameter modes](https://github.com/rask-lang/rask/blob/main/specs/memory/parameters.md): the normative version
- [Value semantics](https://github.com/rask-lang/rask/blob/main/specs/memory/value-semantics.md): the copy threshold
- [Linearity](https://github.com/rask-lang/rask/blob/main/specs/memory/linear.md): why a borrow can't be consumed
