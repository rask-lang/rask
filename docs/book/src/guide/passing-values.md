# Passing Values

Every parameter answers one question: **does the caller still have this afterwards?**
There are three answers, and the code says which one at both ends.

All the Rask on this page comes from [examples/parameter_modes.rk](https://github.com/rask-lang/rask/blob/main/examples/parameter_modes.rk),
pulled in directly rather than copied. CI runs that file on both backends, so nothing here is a
snippet that used to work. The error messages are real output from `rask check`.

## Borrow — the default

```rask
{{#include ../../../../examples/parameter_modes.rk:borrow}}
```

No marker, no ceremony. The callee reads; the caller keeps the value and carries on using it. This
is most parameters in most programs, which is why it's the mode you write nothing for.

A borrow isn't yours to give away. Try to pass it on to something that takes ownership and the
compiler stops you:

```text
error[E0835]: cannot give away `account` — it's borrowed, not owned
 10 | func describe(account: Account) -> i64 {
    |               ------- `account` is declared as a borrowed parameter
 11 |     return close_out(account)
    |                      ^^^^^^^ `close_out` takes ownership, and `account` isn't yours to give
    = fix: take it: `take account: …` in the signature — then the caller can see it goes
```

The caller never marked this as given, so they're still using it. Consuming it here would leave
them holding something that's gone — and for a file handle or a transaction, that's a second close
of a real resource.

## Mutate — write through, caller keeps it

```rask
{{#include ../../../../examples/parameter_modes.rk:mutate}}
```

The call site marks it too, and that's not optional:

```rask
deposit(mutate account, 50)
```

Leave the marker off and you get the one-token fix, plus the reason:

```text
error[E0373]: `deposit` mutates `account` — mark it at the call site
 12 |     deposit(account, 50)
    |             ^^^^^^^ passed to the `mutate account` parameter
    = fix: deposit(mutate account, …)
```

Here's the thinking behind that. A misread *move* is backstopped by the compiler — use a value
after it's moved and you get an error naming where it went. A misread *mutation* has no backstop:
both readings are legal code, and the value looks identical afterwards, just different. So the case
that can't be caught is the one that gets written down.

Because the rule is syntactic it has no exceptions to memorise: the marker is required exactly when
the parameter says `mutate`, whatever the argument's type or size. An `i64` writes it too.

One thing catches everyone once. `let` is deep:

```text
error[E0302]: cannot mutate `account` — declared `let`
 12 |     deposit(mutate account, 50)
    |                    ^^^^^^^ `account` is a let binding — immutable
    = fix: replace `let account` with `mut account`
```

`let` doesn't mean "this name won't be reassigned." It means nothing changes through this name —
including a mutating method, and including an index or field assignment.

## Take — the callee keeps it

```rask
{{#include ../../../../examples/parameter_modes.rk:take}}
```

No marker needed, though `own account` is available when you want the call site to shout. After the
call the name is gone:

```text
error[E0800]: use of moved value: `account`
 12 |     let n = close_out(account)
    |                       ------- value moved here
 13 |     println("{n} {account.balance}")
    |                   ^^^^^^^ value used here after move
    = note: `Account` is 24 bytes (copy threshold is 16) — assignment moves instead of copying
```

That note is the whole reason `take` needs no marker: the compiler will tell you exactly where the
value went, the moment you reach for it again.

## Receivers are never marked

```rask
{{#include ../../../../examples/parameter_modes.rk:receiver}}
```

```rask
account.charge_fee(5)
```

`charge_fee` takes `mutate self` and the call site still says nothing. The receiver is the thing
being operated on — that's what the dot means. Marking it would put noise on every mutating method
in the language.

## Putting it together

```rask
{{#include ../../../../examples/parameter_modes.rk:callsite}}
```

Read the markers and you know the shape of that block without opening a single signature: two
borrows, one mutation, one method on the receiver, one hand-off. That's what making them visible
buys.

## Small values are copied, not given

```rask
{{#include ../../../../examples/parameter_modes.rk:copyfn}}
```

```rask
{{#include ../../../../examples/parameter_modes.rk:copy}}
```

which prints:

```text
doubled to 10, and fee is still 5
```

`take` means "I'm keeping this", but you can't take away what the caller never gave up. Values of
16 bytes or less whose fields are all Copy get copied on the way in, so `fee` is untouched. Past 16
bytes it's a real move and the name dies — as the `Account` error above shows, with the sizes in
the note.

The threshold is fixed at 16 bytes and isn't configurable. Moving it would change what existing
programs mean, so it's a semantic boundary rather than a tuning knob.

## Rules behind this page

- [Parameter modes](https://github.com/rask-lang/rask/blob/main/specs/memory/parameters.md) — the normative version
- [Value semantics](https://github.com/rask-lang/rask/blob/main/specs/memory/value-semantics.md) — the copy threshold
- [Linearity](https://github.com/rask-lang/rask/blob/main/specs/memory/linear.md) — why a borrow can't be consumed
