# When one owner isn't enough

So far every room has sat in a `Vec`, and an exit has named the room it leads to by its
place in that list. That works as long as the list never loses anything. This chapter
lets a tunnel collapse, and that one change is enough to break it.

## Places in a list move

Take a room out of the middle of a `Vec`, and everything after it shifts down one. A
number saved before the removal still compiles, still runs, and now means a different
room:

<!-- test: run | The way down leads to the Hall. -->
```rask
func main() {
    mut rooms: Vec<string> = ["Gate", "Courtyard", "Tunnel", "Hall"]
    let tunnel = 2
    rooms.remove(0)
    println("The way down leads to the {rooms[tunnel]}.")
}
```

Nothing went wrong that anything could notice. In the game this is an exit quietly
leading into the wrong room, and the same goes for anything else that remembers a room
by number: where each player is standing, where the rat is.

The rooms aren't really a list anyway. They point at each other, and some of them go
away while others still point at them. Rask has one type for exactly that.

## `Rack` and `Link`

A `Rack<Room>` holds rooms the way a `Vec` does, with two differences. A room in a rack
never moves, whatever else is added or deleted. And adding one hands back a
`Link<Room>`, which is how anything else refers to it. From here on the program is the
one in `mud/`, which keeps everything in racks:

<!-- from: mud/world.rk -->
```rask
public struct World {
    public rooms: Rack<Room>
    public players: Rack<Player>
    public monsters: Rack<Monster>
}
```

A link reads like the room it names. `a.exits` is the room's exit list, reached through
the link, with no lookup and no copy. It's a machine word (an address), so it copies
like a number, and handing one around never moves the room out of the rack. The rack
owns the rooms; links only name them.

An exit holds its link in a field:

<!-- from: mud/world.rk -->
```rask
// An exit stays when the room it led to is gone: `to` reads `none` from then on.
public struct Exit {
    public dir: Direction
    public to: Link<Room>?
}
```

## Deleting a room

That comment is the part that matters. `rack.delete(room)` frees the room, and before it
returns it finds every `Link<Room>?` field that pointed at it and sets it to `none`. No
field anywhere is left aiming at memory that's gone, and nothing has to remember to
clean up:

<!-- test: run | North of the cellar: Tunnel\nNorth of the cellar: nothing -->
```rask
import memory.Rack
import memory.Link

struct Room {
    name: string
    north: Link<Room>?
}

func main() {
    mut rooms: Rack<Room> = Rack.new()
    let cellar = rooms.insert(Room(name: "Cellar", north: none))
    let tunnel = rooms.insert(Room(name: "Tunnel", north: none))
    cellar.north = tunnel
    if cellar.north? as n {
        println("North of the cellar: {n.name}")
    }
    rooms.delete(tunnel)
    if cellar.north? as n {
        println("North of the cellar: {n.name}")
    } else {
        println("North of the cellar: nothing")
    }
}
```

The `?` on the field type is what makes this work, and the game reads it the way chapter
7 read any value that might be missing. Walking through an exit whose room is gone hits
the `none` and says so:

<!-- from: mud/game.rk -->
```rask
    let exit = here.exit(dir) ?? return reply("You can't go {dir.name()}.")
    let there = exit.to ?? return reply("Rubble blocks the way {dir.name()}.")
```

A link in a local name is different, because the rack can't reach a local to clear it.
So the compiler does that job instead, and refuses to let you follow one after its room
is deleted:

<!-- from: ../../errors/11-when-one-owner-isnt-enough/link_after_delete.out -->
```text
error[E0328]: `tunnel` names a deleted node — this is a use after free
  --> docs/book/errors/11-when-one-owner-isnt-enough/link_after_delete.rk:16:13
    |
 15 |     rooms.delete(tunnel)
    |     -------------------- the node `tunnel` names was deleted here
 16 |     println(tunnel.name)
    |             ^^^^^^ `tunnel` points at freed memory from here on
    = fix: move the reads above the delete, or keep the link in a `Link<T>?`
          field
    = why: a `Link<T>` is a pointer to a node, and `delete` frees the node — so
          every name for it dies at once. A field can survive, because the rack
          nulls it and the `?` makes you check; a local can't be reached by the
          rack, so the compiler proves here that you never follow one
```

Between them, those two rules leave no way to reach a room that's gone: a field reads
`none`, and a local is an error. That's also why following a link costs nothing at run
time. There's no check to make, because the state it would check for can't happen.

## `deleting`

Collapsing a tunnel means finding the room by name and deleting it. A function that does
that picks for itself which room goes, and the caller can't see which one from outside.
So it says so in its signature, with `deleting` where `mutate` would be:

<!-- from: mud/world.rk -->
```rask
    public func collapse(deleting self, name: string) -> bool {
```

Leave it at `mutate` and the compiler names the word:

<!-- from: ../../errors/11-when-one-owner-isnt-enough/delete_without_deleting.out -->
```text
error[E0329]: this can delete nodes the caller never named — declare `deleting self`
  --> docs/book/errors/11-when-one-owner-isnt-enough/delete_without_deleting.rk:18:17
    |
 18 |                 self.rooms.delete(r)
    |                 ^^^^^^^^^^^^^^^^^^^^ `delete` here chooses which nodes die
    = fix: deleting self
    = why: `delete(take link)` is safe for the caller because the link is
          consumed at the call site — they can see the name die. A delete that
          chooses its own victim can't be seen from outside, so the caller's
          links are revoked at the call instead, and `deleting` is what tells
          them to expect it. `mutate` doesn't imply it: inserting and writing
          can't invalidate a link, deleting can
```

The word matters to the caller. After a call that may delete any room, no link the
caller holds in a local can be trusted, even to a room that's still standing, so they
all stop working there:

<!-- from: ../../errors/11-when-one-owner-isnt-enough/link_after_collapse.out -->
```text
error[E0328]: `cellar` may name a deleted node — possible use after free
  --> docs/book/errors/11-when-one-owner-isnt-enough/link_after_collapse.rk:32:13
    |
 31 |     w.collapse("Tunnel")
    |     -------------------- this can delete the node `cellar` names, so `cellar` dies here
 32 |     println(cellar.name)
    |             ^^^^^^ `cellar` may point at freed memory from here on
    = fix: find the node again after this line, or keep the link in a `Link<T>?`
          field
    = why: a delete that can't say which nodes went leaves no local link into
          that rack trustworthy after it. A field can survive, because the rack
          nulls it if its node went and the `?` makes you check
```

The fix is the one it gives: look the room up again by name afterwards. The game never
has to, because it holds no links in locals across commands: each command finds the
player by name and the room through the player.

## What else changed in `mud/`

Moving to racks is the reason for the switch, but the program in `mud/` is also the game
for more than one player, which the last chapters build on. Players sit in a rack of their
own, each with a `Link<Room>?` for where they stand, so a player in a collapsing room ends
up nowhere rather than somewhere wrong. And a command now answers an `Outcome`: the reply
for whoever typed it, plus a list of lines for the other players who should hear about
it. You'll also see an `interface` and a function with a type in angle brackets, which is
the next chapter.

## Exercise

`11-when-one-owner-isnt-enough.rk` is the game from `mud/`, with joining two rooms and
collapsing one left empty:

<!-- from: 11-when-one-owner-isnt-enough.rk -->
```rask
    // exercise 11: Join `a` to `b` both ways: going `dir` from `a` reaches `b`,
    // and the opposite way from `b` leads back to `a`.
    public func connect(mutate self, a: Link<Room>, dir: Direction, b: Link<Room>) -> void {
        todo()
    }
```

<!-- from: 11-when-one-owner-isnt-enough.rk -->
```rask
    // exercise 11: Delete the room called `name`, and answer whether there was
    // one. Every exit into it reads `none` afterwards. Anyone standing in it is
    // nowhere, so callers move them out first.
    public func collapse(deleting self, name: string) -> bool {
        todo()
    }
```

An exit is `Exit(dir, to: b)`, and it goes on the room's `exits` through the link.
`dir.opposite()` is the way back. `self.room(name)` finds a room by name, and
`self.rooms.delete(r)` deletes one.

```bash
rask test 11-when-one-owner-isnt-enough.rk
```

<!-- from: output/11-exercise.out -->
```text
=== Testing 11-when-one-owner-isnt-enough.rk ===

  ✓ every direction leads back the way it came (0ms)
  ✓ finding an item by name (0ms)
  ✓ a direction alone is a move (0ms)
  ✓ the object keeps its case and inner spaces (0ms)
  ✓ what can go wrong is a value (0ms)
  ✗ walking to the hall
      11-when-one-owner-isnt-enough.rk:250: not yet implemented
  ✗ picking something up moves it
      11-when-one-owner-isnt-enough.rk:250: not yet implemented
  ✗ a dead end and a typo
      11-when-one-owner-isnt-enough.rk:250: not yet implemented
  ✗ two players in one room hear each other
      11-when-one-owner-isnt-enough.rk:250: not yet implemented
  ✗ a collapsed tunnel leaves a blocked exit
      11-when-one-owner-isnt-enough.rk:250: not yet implemented
  ✗ leaving drops what you carried
      11-when-one-owner-isnt-enough.rk:250: not yet implemented
  ✗ the rat wanders
      11-when-one-owner-isnt-enough.rk:250: not yet implemented
  ✗ a name already in the game is refused
      11-when-one-owner-isnt-enough.rk:250: not yet implemented
  ✓ a list reads like a sentence (0ms)

──────────────────────────────────────────────────
14 tests, 6 passed, 8 failed (0ms)
```
