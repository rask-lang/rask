# Passing values

Every parameter answers one question: does the caller still have this afterwards?

For a lot of values the answer is yes and there's nothing to think about. For the rest,
the code says which answer applies, at both ends.

## Small values are copied

A direction, a room's place in the list, a count: pass one, and the function gets a copy
of its own. `go` takes the direction that way:

<!-- from: early/game.rk -->
```rask
public func go(mutate world: World, dir: Direction) -> string {
```

Nothing is written about `dir` at either end, because nothing about it changes for the
caller.

The line is 16 bytes or less, with every field itself copyable. That covers the numbers,
`bool`, `char`, `string` (a string is never changed in place, so a copy of one shares the
text) and small structs and enums built out of those. It's a fixed line, not a setting.

So for that whole family, pass it around and stop worrying. The rest of the chapter is
about everything else.

## Everything else has one owner

An `Item` is two strings, 32 bytes, so it's over the line, and so is a `Vec` or a `World`.
Copying one means copying everything it holds, and Rask doesn't do that unless the code
says `.clone()`. So a value like that has one owner at a time: one name is responsible for
it, and when that name goes away, so does the value.

That leaves three things a function can do with one: read it, change it, or take it.

## Reading: the default

Describing a room reads the world and changes nothing:

<!-- from: early/game.rk -->
```rask
public func describe(world: World) -> string {
```

No marker at either end, and the reason is that nothing happened. The caller's world is
the same after the call as it was before, and `describe` reads it where it is, without a
copy. That's what most parameters do.

The function can't keep it, either. When the call returns, the reading is over, so there's
nothing left pointing at the caller's world. The compiler calls this borrowing.

## Changing it: `mutate`

Walking changes where the player is, so `go` takes the world to write to, and says
`mutate`. The call site says it too:

<!-- from: early/game.rk -->
```rask
        Command.Go(dir) => go(mutate world, dir),
```

That's required. Leave it off and the compiler names the one-word fix:

<!-- from: ../../errors/09-passing-values/missing_marker.out -->
```text
error[E0373]: `go` mutates `world` — mark it at the call site
  --> docs/book/errors/09-passing-values/missing_marker.rk:18:8
    |
 18 |     go(world, 1)
    |        ^^^^^ passed to the `mutate world` parameter
    = fix: go(mutate world, 1)
    = why: the compiler backstops a misread *move* — using a value after it's
          moved is an error — but nothing backstops a misread mutation: both
          readings are legal code, so the one that can't be caught gets written
          down. The marker follows the signature, not the argument's size, so a
          Copy argument writes it too. A method receiver is exempt —
          `player.take_damage(10)` operates on the receiver by construction
          [mem.parameters/PM4, PM5]
```

A misread move gets caught for you: use a value after it's gone and the compiler says
where it went. A misread change doesn't. The value is still there afterwards, only
different, and both readings are legal code. So the case nothing can catch is the one
the code writes down, and a reader of `apply` sees which commands change the world
without opening a single function.

`mutate` asks for a name made with `mut`. A `let` name promises its value doesn't change
through it at all, so it can't be lent to something that changes it:

<!-- from: ../../errors/09-passing-values/let_as_mutate.out -->
```text
error[E0302]: cannot mutate `world` — declared `let`
  --> docs/book/errors/09-passing-values/let_as_mutate.rk:18:15
    |
 18 |     go(mutate world, 1)
    |               ^^^^^ `world` is a let binding — immutable
    = fix: replace `let world` with `mut world`
    = why: `let` bindings forbid rebinding and mutation. Use `mut` when you need
          to modify the value or call mutating methods.
```

## Taking it: `take`

Picking up the lamp moves it. It leaves the room's items and joins what the player
carries:

<!-- from: early/game.rk -->
```rask
        world.carried.push(room.items.remove(i))
```

`remove(i)` takes the item out of the room's list and hands it over. `push` is declared
`push(mutate self, take item: T)`, and `take` means it keeps what it's given. Once that line
has run, the lamp is in `carried` and nowhere else. There's no second copy anywhere for
the room to still be holding, which is the game's rule exactly: an item is in one place.

`take` needs no marker at the call site, because the compiler catches a misread one. Use
a value after something took it, and the error says where it went:

<!-- from: ../../errors/09-passing-values/use_after_take.out -->
```text
error[E0800]: use of moved value: `lamp`
  --> docs/book/errors/09-passing-values/use_after_take.rk:20:13
    |
 19 |     world.carried.push(lamp)
    |                        ---- value moved here
 20 |     println(lamp.name)
    |             ^^^^ value used here after move
    = note: `Item` is 32 bytes (copy threshold is 16) — assignment moves instead
           of copying
    = help: add `lamp.clone()` if you need an independent copy
```

`take` can't be written on a type that is always copied, like a number or a `string`,
because the caller would keep theirs and the word would be wrong:

<!-- from: ../../errors/09-passing-values/take_on_copy.out -->
```text
error[E0404]: `take` on `u64` takes nothing — a Copy value is copied, not given
  --> docs/book/errors/09-passing-values/take_on_copy.rk:6:40
    |
  6 | func walk_to(mutate world: World, take room: u64) {
    |                                        ^^^^ `room` is a `u64`, and the caller keeps theirs
    = fix: drop the keyword: `room: u64`
    = why: a mode marker is read at the call site to know what happens to the
          argument, so one that says the wrong thing is worse than none
          [mem.parameters/PM6c]
```

Passing a copied value to a `take` parameter is fine, though. `push` on a `Vec<u64>`
copies the number in, and the caller still has it.

## The receiver is never marked

`add_room` changes the world it's called on:

<!-- from: early/world.rk -->
```rask
    public func add_room(mutate self, name: string, description: string) -> u64 {
```

and the call says nothing:

<!-- from: early/start.rk -->
```rask
    let gate = w.add_room("Gate", "A rusted iron gate stands open to the north.")
```

The value before the dot is the thing being worked on, which is what the dot means.
Marking it would put noise on every method that changes anything.

## Exercise

`09-passing-values.rk` has picking up and putting down left empty:

<!-- from: 09-passing-values.rk -->
```rask
// exercise 09: Move the item called `thing` off the floor and into what the
// player carries. It leaves the room: an item is only ever in one place.
public func pick_up(mutate world: World, thing: string) -> string {
    todo()
}

// exercise 09: The other way: from what the player carries onto the floor.
public func put_down(mutate world: World, thing: string) -> string {
    todo()
}
```

`find_item` finds where an item is in a list, `remove` takes it out, and `push` puts it
in another. Answer `You take the lamp.` and `You drop the lamp.`, or
`There's no lamp here.` and `You don't have a lamp.` when it isn't there. Working on the
room means a `with` block, from chapter 6.

```bash
rask test 09-passing-values.rk
```

<!-- from: output/09-exercise.out -->
```text
=== Testing 09-passing-values.rk ===

  ✓ every direction leads back the way it came (0ms)
  ✓ finding an item by name (0ms)
  ✓ a direction alone is a move (0ms)
  ✓ the object keeps its case and inner spaces (0ms)
  ✓ what can go wrong is a value (0ms)
  ✓ the game starts at the gate (0ms)
  ✓ walking to the hall (0ms)
  ✓ a dead end and a typo (0ms)
  ✗ picking something up moves it
      09-passing-values.rk:287: not yet implemented
  ✗ putting it down somewhere else
      09-passing-values.rk:287: not yet implemented

──────────────────────────────────────────────────
10 tests, 8 passed, 2 failed (0ms)
```
