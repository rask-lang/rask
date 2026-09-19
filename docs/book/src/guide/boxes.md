# Boxes

A value in Rask sits in one place and one name is responsible for it. That covers almost
everything you write. A box is what you reach for when it doesn't.

There are four of them and you can go a long way without meeting any. So this page starts with
the code you already know how to write, and only adds a box when the plain version stops working.

## Most of the time: a plain field

```rask
{{#include ../../../../examples/boxes.rk:plain}}
```

```rask
{{#include ../../../../examples/boxes.rk:plainuse}}
```

One owner, fields you read and write directly, a `Vec` inside it that grows. No box anywhere, and
nothing on this page applies. Reach for `Vec` or `Map` when you have many of something and this
still holds: they're ordinary values that happen to own heap storage.

The rest of the page is the three times that isn't enough.

## Several names, one value: `Shared`

Say two parts of your program need to see the same settings, and one of them updates it. You can't
give them each a copy, because then there are two settings. You need one value that both reach.

```rask
{{#include ../../../../examples/boxes.rk:sharedtype}}
```

```rask
{{#include ../../../../examples/boxes.rk:sharedmake}}
```

That's the whole of it for most uses. `Shared.new` is always a correct answer: it takes a
read-write lock, so any number of readers go at once and a writer gets it to itself. It's safe to
send to another task.

What you give up is reaching the value with a dot. Access is scoped, and it says `read` or `write`:

```rask
{{#include ../../../../examples/boxes.rk:sharedread}}
```

```rask
{{#include ../../../../examples/boxes.rk:sharedwrite}}
```

Taking a lock is a real cost, and a real cost is visible in Rask source. Scoping it puts the
unlock where you can see it too: the block ends, the lock releases.

A block is for several statements under one lock. One statement doesn't need it:

```rask
{{#include ../../../../examples/boxes.rk:sharedinline}}
```

```rask
{{#include ../../../../examples/boxes.rk:sharedstore}}
```

`Shared` takes a strategy that decides which lock it uses, and the default is the one above. The
other two are worth reading about the day a profiler points at the lock, and not before.

## Things that point at each other: `Rack` and `Link`

Rooms with exits, nodes with children, anything where one value refers to another and values get
removed while others still refer to them. A `Vec` can't do this: the index you saved means
something different after a removal, and nothing tells you.

```rask
{{#include ../../../../examples/boxes.rk:racktype}}
```

A `Rack` holds the values. A `Link` is how one of them refers to another.

```rask
{{#include ../../../../examples/boxes.rk:rackmake}}
```

```rask
{{#include ../../../../examples/boxes.rk:rackread}}
```

Now delete the room that `hall` points at:

```rask
{{#include ../../../../examples/boxes.rk:rackdelete}}
```

The same `if`, run twice, prints `hall leads to cell` and then `hall leads nowhere`.

That's the part worth keeping. Deleting a node doesn't leave `hall.exit` aiming at a dead value,
and it doesn't leave you an index that now means some other room. Every link into the deleted node
reads as absent, so the `if` above simply takes its other branch.

## Recursive, or big and moved often: `Heap`

A struct can't contain itself by value, because nothing could decide how large it is. `Heap<T>` is
one pointer's worth of indirection, which breaks the cycle. It's also what you use for a large
value you move around a lot, so the moves copy a pointer instead of the whole thing.

```rask
{{#include ../../../../examples/boxes.rk:heaptype}}
```

```rask
{{#include ../../../../examples/boxes.rk:heapuse}}
```

`Heap` is the one box that has to be released by hand, because it's the one that owns an allocation
of its own with no container around it to do the job. Rask has no destructors, so nothing runs
behind your back at the closing brace.

The `ensure` goes immediately after the line that allocates, and the compiler holds you to it:

```text
{{#include ../../errors/boxes/heap_no_cleanup.out}}
```

That program does release the value, on the last line. The complaint is about the lines in
between: a panic anywhere in them leaves with nothing scheduled to clean up. Putting the `ensure`
first costs nothing, because consuming the value later cancels it.

## Choosing

Four questions, asked in order. Stop at the first yes.

1. One value, one owner? A plain field. Done.
2. Many values? `Vec` or `Map`, unless:
3. They refer to each other and can be deleted? `Rack` and `Link`.
4. Several names reach one value that changes? `Shared`.

One more question sits outside that list, and mixing it in is what makes the set feel harder than
it is: does the value need to be on the heap, because it's recursive or large and moved often?
That's `Heap`, and it's independent of all four answers above.

Reading it as one sentence: plain fields until you have many, `Vec` and `Map` until they refer to
each other, `Rack` when they do, and a lock only once a second task exists. Steps 1 and 2 are
most programs.

## One thing to watch

A box is not the only place a value can be held in place while you look at it. Reading an element
of a `Vec` in a `with` block does the same thing, and growing the `Vec` underneath it is refused:

```text
{{#include ../../errors/boxes/push_inside_with.out}}
```

Pushing can move the whole buffer somewhere else, which would leave the binding pointing at
storage that isn't the element any more. Note what's frozen: the collection, not the element. You
can read and write other elements freely. It's growth that's the problem, because growth is what
moves things.

## Running it

The whole program on this page, start to finish:

```text
{{#include ../../../../tests/golden/boxes.out}}
```

## Rules behind this page

- [Boxes](https://github.com/rask-lang/rask/blob/main/specs/memory/boxes.md): the family, and why it's closed
- [Racks](https://github.com/rask-lang/rask/blob/main/specs/memory/racks.md): links, deletion, and what a stale one reads as
- [Synchronization](https://github.com/rask-lang/rask/blob/main/specs/concurrency/sync.md): the three strategies
- [Borrowing](https://github.com/rask-lang/rask/blob/main/specs/memory/borrowing.md): inline access and `with`
- [Linearity](https://github.com/rask-lang/rask/blob/main/specs/memory/linear.md): why `Heap` needs an `ensure`
