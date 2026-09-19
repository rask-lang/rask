# Boxes

A value in Rask sits in one place and one name is responsible for it. That covers almost
everything you write. A box is what you reach for when it doesn't.

There are three you'll meet, and you can go a long way without needing any of them. So this page
starts with the code you already know how to write, and adds a box only where the plain version
stops working.

## Most of the time: a plain field

```rask
{{#include ../../../../examples/boxes.rk:plain}}
```

```rask
{{#include ../../../../examples/boxes.rk:plainuse}}
```

`hero` owns that player. The fields are reached with a dot, the `Vec` inside grows when you push to
it, and when `hero` goes out of scope the whole thing is released. No box in sight.

Having many of something doesn't change that. A `Vec` or a `Map` is an ordinary value that happens
to keep its contents on the heap, so it's owned by one name like anything else.

What breaks this shape is one of three things: another part of the program needs the same value,
your values need to refer to each other, or the value can't sit where it is. A box for each.

## When another part needs the same value: `Shared`

Say your settings are read in one place and updated in another. A copy for each is the wrong shape,
because then there are two settings and an update to one is invisible to the other. What you want
is a single value that both places reach.

```rask
{{#include ../../../../examples/boxes.rk:sharedtype}}
```

```rask
{{#include ../../../../examples/boxes.rk:sharedmake}}
```

That's the whole of it for most uses. `Shared.new` is always a correct answer: it takes a
read-write lock, so any number of readers go at once and a writer gets it to itself. It's safe to
send to another task.

What you give up is reaching the value with a plain dot. Every access says `read` or `write`:

```rask
{{#include ../../../../examples/boxes.rk:sharedget}}
```

```rask
{{#include ../../../../examples/boxes.rk:sharedset}}
```

Each of those takes the lock, does the one thing, and releases it, all inside the expression.

When you need several statements to happen under one lock, `with` holds it open for a block. Here
the read and the write have to be the same lock, or another task could change `retries` in between:

```rask
{{#include ../../../../examples/boxes.rk:sharedblock}}
```

Taking a lock is a real cost, and a real cost is visible in Rask source. Scoping it puts the
unlock where you can see it too: the block ends, the lock releases.

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

A `Heap<T>` puts the value somewhere else and keeps only its address. An address is the same small
size however big the value is, and that one fact is what makes it useful twice.

The first use is recursion. A struct that contains itself has no size: a `Step` holding a `Step`
holds a `Step`, and the number never settles. Hold the address instead and it settles at once,
because an address doesn't grow.

<!-- test: run-interp | wash\nrinse -->
```rask
import memory.Heap

struct Step {
    label: string
    next: Heap<Step>?
}

func main() {
    let second = Heap(Step { label: "rinse", next: none })
    let first = Heap(Step { label: "wash", next: second })
    ensure drop(first)

    println("{(*first).label}")
    if (*first).next? as n {
        println("{(*n).label}")
    }
}
```

`second` is committed by the next line moving it into `first`, and storing it in that field hands
it over for good: the chain belongs to `first` now, so releasing `first` releases all of it.

The second use is size. Moving a large value copies every byte of it, and moving the box copies one
address. This snapshot is six fields wide:

```rask
{{#include ../../../../examples/boxes.rk:heaptype}}
```

```rask
{{#include ../../../../examples/boxes.rk:heapfn}}
```

```rask
{{#include ../../../../examples/boxes.rk:heapuse}}
```

Passing `shot` to `describe` moves the address. The six fields never get copied, and the caller
gives the value up, which is what `take` in that signature says.

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
4. Does another part of the program need the same value, and does it change? `Shared`.

One more question sits outside that list, and mixing it in is what makes the set feel harder than
it is: does the value need to be on the heap, because it's recursive or large and moved often?
That's `Heap`, and it's independent of all four answers above.

Reading it as one sentence: plain fields until you have many, `Vec` and `Map` until they refer to
each other, `Rack` when they do, and a lock only once a second task exists. Steps 1 and 2 are
most programs.

## One thing to watch

A box is not the only place a value can be held in place while you look at it. Reading an element
of a `Vec` in a `with` block does the same thing:

```rask
{{#include ../../errors/boxes/push_inside_with.rk:body}}
```

That doesn't build, and the `push` is why:

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
