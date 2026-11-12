# Holding many things

A world is many rooms, a room holds items, and a player carries some. Two collections
cover all of it: `Vec`, a list in order, and `Map`, a lookup from keys to values.

## `Vec`

The world keeps its rooms in a `Vec`:

<!-- from: early/world.rk -->
```rask
public struct World {
    public rooms: Vec<Room>
    public here: u64
    public carried: Vec<Item>
}
```

`Vec<Room>` is a list of rooms. `rooms[0]` is the first one, `rooms.len()` how many there
are, and `rooms.push(room)` adds one at the end. A room's place in the list doesn't
change once it's there, so the game names a room by its place: `here` is where the player
stands, and an exit stores the place of the room it leads to.

Positions and lengths are `u64`. There are no negative positions, and `len()` never
needs checking for below zero.

Building the world starts from an empty one:

<!-- from: early/world.rk -->
```rask
        return World(rooms: Vec.new(), here: 0, carried: Vec.new())
```

`Vec.new()` makes an empty list, and its element type comes from where it goes. A list
written out in full uses brackets, `["n", "e"]`.

## `Map`

A room's exits are a `Map` from a direction to the place of the room that way:

<!-- from: early/world.rk -->
```rask
    public exits: Map<Direction, u64>
```

Joining two rooms writes an entry into each:

<!-- from: early/world.rk -->
```rask
    // Both ways: going `dir` from `a` reaches `b`, and the opposite way leads back.
    public func connect(mutate self, a: u64, dir: Direction, b: u64) -> void {
        self.rooms[a].exits[dir] = b
        self.rooms[b].exits[dir.opposite()] = a
    }
```

`exits[dir] = b` adds the entry, or replaces the one already there. `exits.contains(dir)`
asks whether there is one. Reading `exits[dir]` for a key that isn't there stops the
program, so a lookup that might miss asks with `exits.get(dir)`, which is chapter 7.

A `Map` gives its entries back in no order you can rely on. That's why describing a room
goes through `DIRECTIONS`, a `Vec` that's always north first, and asks the room about
each, instead of walking the map and printing the exits in whatever order they come.

## Working on one room in place

Copying a room out of the list isn't allowed:

<!-- from: ../../errors/06-holding-many-things/room_out_of_a_vec.out -->
```text
error[E0871]: `Room` isn't Copy, so `room` would be a second name for the same element
  --> docs/book/errors/06-holding-many-things/room_out_of_a_vec.rk:10:16
    |
 10 |     let room = rooms[0]
    |                ^^^^^^^^ this element stays owned by `rooms`
    = fix: scope the access — `with rooms[…] as room { … }` — or ask for a
          separate value with `.clone()`
    = why: indexing hands the element back where it lives rather than copying it
          out, so a binding of a non-Copy element is a second name for storage
          the collection still owns — a write through either one is a write
          through both. A Copy element is copied and has no such tie
          [mem.borrowing/E4]
```

A room is bigger than the small values that copy for free (chapter 9 has the exact
line), so `rooms[0]` is the room where it sits in the list, not a copy of it. `let room =
rooms[0]` would make `room` a second name for that same room, and a change through
either one would show up through the other.

A single expression can reach into the list and act on what it finds:
`world.rooms[gate].items.push(lamp)` adds to the gate's items where they are. For more
than one line of work on the same room, a `with` block names it for the length of the
block:

<!-- from: early/game.rk -->
```rask
public func describe(world: World) -> string {
    with world.rooms[world.here] as room {
        mut text = "{room.name}\n{room.description}"
```

`room` is the room where it sits, for as long as the block runs. What the block can't do
is add to the list or take from it, because that could move the room it's holding:

<!-- from: ../../errors/06-holding-many-things/push_inside_with.out -->
```text
error[E0808]: cannot push `rooms` inside `with` block
  --> docs/book/errors/06-holding-many-things/push_inside_with.rk:11:9
    |
 10 |     with rooms[0] as room {
    |     ------------- element borrowed here
 11 |         rooms.push(Room(name: "Courtyard", items: Vec.new()))
    |         ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ push not allowed inside with block
    = fix: move the structural mutation outside the with block
    = why: rooms can reallocate, invalidating the borrowed element. If the
          elements reference each other and get deleted individually, that is
          what a `Rack` is for — a node never moves, so nothing invalidates
          [mem.borrowing/W2]
```

## Exercise

`06-holding-many-things.rk` has two functions to write. `add_room` puts a new room in the
world and answers where it went:

<!-- from: 06-holding-many-things.rk -->
```rask
    // exercise 06: Add a room with nothing in it and no way out, and answer
    // its place in `rooms`, which is how an exit will name it.
    public func add_room(mutate self, name: string, description: string) -> u64 {
        todo()
    }
```

A new room's items and exits both start empty: `Vec.new()` and `Map.new()`.

`inventory` answers the `i` command:

<!-- from: 06-holding-many-things.rk -->
```rask
// exercise 06: List what the player carries, or say there's nothing.
public func inventory(world: World) -> string {
    todo()
}
```

`You carry nothing.` when the list is empty, and `You carry lamp, key.` when it isn't. A
`Vec<string>` joins into one string with `names.join(", ")`.

```bash
rask test 06-holding-many-things.rk
```

<!-- from: output/06-exercise.out -->
```text
=== Testing 06-holding-many-things.rk ===

  ✓ every direction leads back the way it came (0ms)
  ✓ finding an item by name (0ms)
  ✓ a direction alone is a move (0ms)
  ✓ the object keeps its case and inner spaces (0ms)
  ✓ what can go wrong is a value (0ms)
  ✗ the game starts at the gate
      06-holding-many-things.rk:226: not yet implemented
  ✗ walking to the hall
      06-holding-many-things.rk:226: not yet implemented
  ✗ a dead end and a typo
      06-holding-many-things.rk:226: not yet implemented
  ✗ picking something up moves it
      06-holding-many-things.rk:226: not yet implemented
  ✗ putting it down somewhere else
      06-holding-many-things.rk:226: not yet implemented

──────────────────────────────────────────────────
10 tests, 5 passed, 5 failed (0ms)
```
