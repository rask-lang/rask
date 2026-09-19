# Passing Values

Every parameter answers one question: **does the caller still have this afterwards?**

For a lot of values the answer is yes and there's nothing to think about. For the rest, the code
says which answer applies, at both ends. This page goes from the first kind to the second.

## Small values are copied

Start with the easy half. Pass an `i64` and the callee gets its own copy:

```rask
{{#include ../../../../examples/parameter_modes.rk:copyfn}}
```

```rask
{{#include ../../../../examples/parameter_modes.rk:copy}}
```

No marker at either end, because nothing happened to `fee` that the caller needs to know about.

The line is **16 bytes or less, with every field itself copyable**. That covers the integers, the
floats, `bool`, `char`, and small structs built out of those. It's fixed and not configurable:
moving it would change what existing programs mean, so it's a semantic boundary rather than a
tuning knob.

So for that whole family, ignore the rest of this page. Pass them around and stop worrying.

## Everything else has one owner

Now the other half: a `string`, a `Vec`, a struct bigger than the line. Copying one of those means
copying whatever it owns on the heap, and Rask won't do that behind your back — a copy that costs
an allocation is a copy you should be able to see.

So a value like that has exactly **one owner** at a time. One name is responsible for it, and when
that name goes out of scope the value is released.

Which leaves two ways to hand it to a function:

- **Lend it.** The callee uses it for the length of the call. You still own it afterwards.
- **Give it away.** The callee owns it now. You don't have it any more.

The next three sections are those two options — lending to read, lending to write, and giving.

## Lend it to read: the default

```rask
{{#include ../../../../examples/parameter_modes.rk:borrow}}
```

No marker at either end, and the reason is that nothing happened. `account` is the same after the
call as it was before: same owner, same contents. There's no cost to flag and no surprise to warn
about, so the syntax stays out of the way. This is what most parameters do.

The lend lasts exactly as long as the call. That's what makes it safe without any annotation —
`describe` can't stash the account somewhere and still be holding it after it returns, because
[a reference can't be stored](../index.html).

A borrow isn't yours to give away, either. Try to pass it on to something that takes ownership and
the compiler stops you:

```text
{{#include ../../errors/passing-values/consume_borrowed.out}}
```

The caller never gave this up, so they're still using it. Consuming it here would leave them
holding something that's gone. For a file handle or a transaction, that's a second close of a real
resource.

## Lend it to write: `mutate`

Same deal — the caller still owns it afterwards — except the callee writes through:

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
the parameter says `mutate`, whatever the argument's type or size. An `i64` writes it too — the
copy rule decides what the callee gets, not whether you have to say so.

One thing catches everyone once. `let` is deep:

```text
{{#include ../../errors/passing-values/let_as_mutate.out}}
```

`let` doesn't mean "this name won't be reassigned." It means nothing changes through this name,
including through a mutating method, an index, or a field assignment.

## Give it away: `take`

```rask
{{#include ../../../../examples/parameter_modes.rk:take}}
```

Now the caller doesn't have it. Reach for the name again and it's gone:

```text
{{#include ../../errors/passing-values/use_after_take.out}}
```

That note is the whole reason `take` needs no marker at the call site: the compiler will tell you
exactly where the value went, the moment you reach for it again. `own account` is available when
you want the call site to shout anyway.

### You can't `take` a small value

Back to the easy half for a second, because the two rules meet here. Write `take` on an `i64` and
the compiler stops you:

```text
{{#include ../../errors/passing-values/take_on_copy.out}}
```

`take` is a promise to the caller that they lose the value, and an `i64` is copied — they don't.
A marker that says the wrong thing is worse than no marker, so the signature has to drop it.

That is about what you *write*. Handing a Copy value to a `take` parameter is still fine, and
common — it just happens through a generic, where the declaration couldn't know:

```rask
{{#include ../../../../examples/parameter_modes.rk:copytake}}
```

`Vec.push` is `push(mutate self, take item: T)`. At `T = i64` the push copies, and `fee` is still
there for the next line.

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
lends, one write, one method on the receiver, one hand-off. That's what making them visible buys.
Running it prints:

```text
{{#include ../../../../tests/golden/parameter_modes.out}}
```

## Rules behind this page

- [Parameter modes](https://github.com/rask-lang/rask/blob/main/specs/memory/parameters.md): the normative version
- [Value semantics](https://github.com/rask-lang/rask/blob/main/specs/memory/value-semantics.md): the copy threshold
- [Linearity](https://github.com/rask-lang/rask/blob/main/specs/memory/linear.md): why a borrow can't be consumed
