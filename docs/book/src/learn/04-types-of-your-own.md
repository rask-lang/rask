# Types of your own

A game needs things to talk about: rooms, the items lying in them, the ways out. Each
of those is a type the program declares.

## Structs

An item has a name and a description:

<!-- from: mud/item.rk -->
```rask
public struct Item {
    public name: string
    public description: string
}
```

A struct is a value made of named fields. `public` lets code outside this package
use the type and its fields; within one program you can ignore it until chapter 13.

Building one names every field, in any order:

<!-- from: early/start.rk -->
```rask
    w.rooms[gate].items.push(Item(name: "lamp", description: "A brass lamp, still half full of oil."))
```

`Item(...)` looks like a call because it is one, of the type's own constructor.
Arguments go by name, so `Item("lamp", "A brass lamp")` is an error: with two strings in
a row, nothing would say which is which. When a variable already has the field's name,
the name alone will do. `Room(name, description, …)` passes the variables `name` and
`description` to the fields of the same names.

A field is read with a dot:

<!-- test: run | lamp: A brass lamp. -->
```rask
struct Item {
    name: string
    description: string
}

func main() {
    let lamp = Item(name: "lamp", description: "A brass lamp.")
    println("{lamp.name}: {lamp.description}")
}
```

A room holds more: a list of the items lying in it, and its exits, each a direction and
the room that way. The next chapters are about those two fields. Here they're only types:

<!-- from: early/world.rk -->
```rask
public struct Room {
    public name: string
    public description: string
    public items: Vec<Item>
    public exits: Map<Direction, usize>
}
```

## Enums

There are six ways out of a room, and a direction is always exactly one of them:

<!-- from: mud/direction.rk -->
```rask
public enum Direction {
    North
    South
    East
    West
    Up
    Down
}
```

An enum lists what a value can be. A `Direction` is `Direction.North` or one of the five
others, and never anything else, so code that handles all six has handled every
direction there is.

A variant can carry a value of its own. The game's commands are an enum where some
variants carry what they act on:

<!-- from: mud/command.rk -->
```rask
public enum Command {
    Look
    Go(Direction)
    Take(string)
    Drop(string)
    Inventory
    Say(string)
    Help
    Quit
}
```

`Command.Go(Direction.North)` is one command, and `Command.Take("lamp")` is another. A
`Command.Look` carries nothing, because looking needs nothing more said.

## Methods

Functions that belong to a type go in an `extend` block for it:

<!-- from: early/world.rk -->
```rask
extend World {
    public func new() -> World {
        return World(rooms: Vec.new(), here: 0, carried: Vec.new())
    }
```

A function whose first parameter is `self` is a method, called on a value with a dot:
`dir.name()`. One without `self`, like `World.new` here, is called on the type itself:
`World.new()`. There's no separate kind of constructor. `new` is a function that
happens to return a `World`.

The method behind `dir.name()` has to say something different for each direction, and
`match` is how it asks which one `self` is:

<!-- from: mud/direction.rk -->
```rask
    public func name(self) -> string {
        return match self {
            Direction.North => "north",
            Direction.South => "south",
            Direction.East => "east",
            Direction.West => "west",
            Direction.Up => "up",
            Direction.Down => "down",
        }
    }
```

Each arm is a variant, an arrow, and the value for it. The arms have to cover every
variant. Leave one out and the program doesn't compile:

<!-- from: ../../errors/04-types-of-your-own/match_misses_a_variant.out -->
```text
error[E0340]: non-exhaustive match: missing Up
  --> docs/book/errors/04-types-of-your-own/match_misses_a_variant.rk:9:12
    |
  9 |     return match dir {
    |            ^^^^^^^^^^^ missing variants: Up
    = fix: add the missing variants or a wildcard `_` arm
    = why: match expressions must cover all possible values
```

That's what makes adding a seventh direction safe: every `match` that would have to
answer for it stops compiling until it does. The next chapter has more of `match`.

## Exercise

Rooms are joined both ways: going north from the gate reaches the courtyard, and going
south from the courtyard leads back. To make the second exit from the first, the world
needs each direction's opposite. In `04-types-of-your-own.rk`, write
`Direction.opposite`:

<!-- from: 04-types-of-your-own.rk -->
```rask
    // exercise 04: The way back: north for south, up for down, and so on.
    public func opposite(self) -> Direction {
        todo()
    }
```

As it stands, most of the tests fail, because building the world joins rooms with it:

```bash
rask test 04-types-of-your-own.rk
```

<!-- from: output/04-exercise.out -->
```text
=== Testing 04-types-of-your-own.rk ===

  ✗ every direction leads back the way it came
      04-types-of-your-own.rk:44: not yet implemented
  ✓ finding an item by name (0ms)
  ✓ a direction alone is a move (0ms)
  ✓ the object keeps its case and inner spaces (0ms)
  ✓ what can go wrong is a value (0ms)
  ✗ the game starts at the gate
      04-types-of-your-own.rk:44: not yet implemented
  ✗ walking to the hall
      04-types-of-your-own.rk:44: not yet implemented
  ✗ a dead end and a typo
      04-types-of-your-own.rk:44: not yet implemented
  ✗ picking something up moves it
      04-types-of-your-own.rk:44: not yet implemented
  ✗ putting it down somewhere else
      04-types-of-your-own.rk:44: not yet implemented

──────────────────────────────────────────────────
10 tests, 4 passed, 6 failed (0ms)
```

Once `opposite` is written, they all pass.
