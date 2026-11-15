# Writing it once for many types

Looking around a room lists two kinds of thing: the items on the floor (`You see lamp
and key.`) and the ways out (`Exits: north and south (blocked).`). The lists are built
the same way, with commas and an `and` before the last one, and differ only in how each
entry describes itself. This chapter writes that list once, for both.

## An interface says what a type can do

An interface is a list of methods, with no bodies:

<!-- from: mud/game.rk -->
```rask
// Something a player can look at.
public interface Describe {
    func describe(self) -> string
}
```

A type has it when it says so, in a block that holds the methods the interface asks for:

<!-- from: mud/game.rk -->
```rask
Item implements Describe {
    public func describe(self) -> string {
        return self.name
    }
}
```

`Exit` gets one too, which says `north`, or `north (blocked)` when the room it led to has
collapsed. From the outside, these are ordinary methods. `lamp.describe()` works whether
or not anything else knows about `Describe`.

The block has to match the interface exactly. A `describe` that takes an extra argument
is a different method, and the compiler shows you both:

<!-- from: ../../errors/12-writing-it-once-for-many-types/wrong_signature.out -->
```text
error[E0888]: `Item.describe` doesn't match what `Describe` requires
  --> docs/book/errors/12-writing-it-once-for-many-types/wrong_signature.rk:12:12
    |
 12 |     public func describe(self, closely: bool) -> string {
    |            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ this is `func describe(self, bool) -> string`
    = fix: the interface asks for `func describe(self) -> string`
    = why: the header is the claim and the block is the evidence — a signature
          that differs answers a different question than the one the interface
          asked [type.generics/G1]
```

## A generic function works on any type that has it

The listing is written once, with a type parameter in angle brackets:

<!-- from: mud/game.rk -->
```rask
public func listing<T: Describe>(things: Vec<T>, empty: string) -> string {
```

`T` stands for whichever type the caller has, and `T: Describe` is the condition on it,
which the compiler calls a **bound**. Inside the function, the only thing known about a
`T` is the bound, so `describe()` is the one method it may call on one. The game calls it
with both lists:

<!-- from: mud/game.rk -->
```rask
    let items = listing(room.items, "nothing")
    let exits = listing(room.exits, "none")
```

Each call is checked where it's made: `room.items` is a `Vec<Item>`, so `T` is `Item`
there, and `Item` implements `Describe`. The compiler then builds a separate copy of
`listing` for each type it's used with, so a call through `T` costs the same as calling
`Item`'s method by name.

## Having the method isn't enough

The rat has a `describe` of its own, in a plain `extend` block. Pass a list of monsters
to `listing`, and it's refused anyway:

<!-- from: ../../errors/12-writing-it-once-for-many-types/never_says_it.out -->
```text
error[E0333]: `Monster` does not implement `Describe`
  --> docs/book/errors/12-writing-it-once-for-many-types/never_says_it.rk:26:13
    |
 26 |     println(listing(rats))
    |             ^^^^^^^^^^^^^ `Monster` has the methods `Describe` asks for, but never says it implements it
    = fix: declare it; the methods can stay where they are:
          Monster implements Describe {}
    = why: a type has an interface only where it says so, because a method with
          the right name and shape can still mean something else
          [type.generics/G1]
```

A method that happens to be called `describe` and return a `string` might be meant for
something else entirely, and a list of what's in the room is a promise about what each
entry says. So an interface counts only where the type declares it. The declaration can be
empty, as the fix shows, when the methods are already written somewhere else.

## Exercise

`12-writing-it-once-for-many-types.rk` is the same program as the last chapter's, with
the listing and an exit's description left empty:

<!-- from: 12-writing-it-once-for-many-types.rk -->
```rask
Exit implements Describe {
    // exercise 12: An exit describes itself by its direction, with
    // ` (blocked)` after it when the room it led to is gone.
    public func describe(self) -> string {
        todo()
    }
}

// exercise 12: Describe each thing and list them: `lamp`, `lamp and key`,
// `lamp, key and rope`. When there's nothing, answer `empty`.
public func listing<T: Describe>(things: Vec<T>, empty: string) -> string {
    todo()
}
```

`self.to?` asks whether the exit still leads anywhere, and `self.dir.name()` is the
direction as a word. For the list, the separator before each entry after the first is
`, ` except before the last, where it's ` and `.

```bash
rask test 12-writing-it-once-for-many-types.rk
```

<!-- from: output/12-exercise.out -->
```text
=== Testing 12-writing-it-once-for-many-types.rk ===

  ✓ every direction leads back the way it came (0ms)
  ✓ finding an item by name (0ms)
  ✓ a direction alone is a move (0ms)
  ✓ the object keeps its case and inner spaces (0ms)
  ✓ what can go wrong is a value (0ms)
  ✗ walking to the hall
      12-writing-it-once-for-many-types.rk:345: not yet implemented
  ✗ picking something up moves it
      12-writing-it-once-for-many-types.rk:345: not yet implemented
  ✗ a dead end and a typo
      12-writing-it-once-for-many-types.rk:345: not yet implemented
  ✗ two players in one room hear each other
      12-writing-it-once-for-many-types.rk:345: not yet implemented
  ✗ a collapsed tunnel leaves a blocked exit
      12-writing-it-once-for-many-types.rk:345: not yet implemented
  ✗ leaving drops what you carried
      12-writing-it-once-for-many-types.rk:345: not yet implemented
  ✓ the rat wanders (0ms)
  ✗ a name already in the game is refused
      12-writing-it-once-for-many-types.rk:345: not yet implemented
  ✗ a list reads like a sentence
      12-writing-it-once-for-many-types.rk:345: not yet implemented

──────────────────────────────────────────────────
14 tests, 6 passed, 8 failed (0ms)
```
