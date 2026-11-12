# When there might be nothing

Some things might not be there. A room may have no exit to the west, and the lamp may
be somewhere else. `T?` is the type for that: a `T`, or nothing at all.

<!-- test: compile -->
```rask
struct Item {
    name: string
    description: string
}

struct Player {
    name: string
    holding: Item?
}
```

`name` is always there. `holding` might not be, and the `?` is the whole difference.
Nothing else in the program can forget it: there is no path where an empty hand arrives
looking like an item. The empty case is written `none`.

## Asking what's there

`match` takes both cases:

<!-- test: run | ada holds a lamp\nbo holds nothing -->
```rask
struct Item {
    name: string
}

struct Player {
    name: string
    holding: Item?
}

func show(p: Player) -> string {
    return match p.holding {
        Item as item => "{p.name} holds a {item.name}",
        none => "{p.name} holds nothing",
    }
}

func main() {
    println(show(Player(name: "ada", holding: Item(name: "lamp"))))
    println(show(Player(name: "bo", holding: none)))
}
```

`Player(name: "ada", holding: Item(name: "lamp"))` passes a plain `Item` where an `Item?`
goes, and it's taken as "there is one".

The shorter form tests with `x?` and names the value with `as`:

<!-- test: run | ada holds a lamp -->
```rask
struct Item {
    name: string
}

func main() {
    let holding: Item? = Item(name: "lamp")
    if holding? as item {
        println("ada holds a {item.name}")
    }
}
```

`as item` names what the test just proved is there, and `item` is a plain `Item` inside
the block. Without `as`, `x?` is only a `bool`, and `x is none` asks the opposite.

## `??` supplies the other case

Most of the time you don't want to branch, you want a value either way. `??` gives its
right side when its left is `none`:

<!-- test: run | holding nothing -->
```rask
struct Item {
    name: string
}

func main() {
    let holding: Item? = none
    let name = holding?.name ?? "nothing"
    println("holding {name}")
}
```

`holding?.name` is the item's name if there is an item, and `none` if there isn't, so
it's a `string?`. The `??` turns that into a `string`. A longer chain, `a?.b?.c`, works
the same way: any missing link makes the whole chain `none`, and it's still one layer of
"might not be there", so one `??` finishes the job.

The right side of `??` can also leave instead of producing a value. Picking up an item
looks for it, and if it isn't on the floor there's nothing more to do:

<!-- from: early/game.rk -->
```rask
        let i = find_item(room.items, thing) ?? return "There's no {thing} here."
```

`find_item` answers `u64?`: the item's place in the list, or `none`. After that line,
`i` is a plain `u64`, because the only way past it is with an item found. `?? break` and
`?? continue` leave a loop the same way.

## Exercise

`07-when-there-might-be-nothing.rk` has two functions to write. `find_item` is the one
the line above calls:

<!-- from: 07-when-there-might-be-nothing.rk -->
```rask
// exercise 07: Where the item called `name` sits in `items`, or none if
// it isn't there.
public func find_item(items: Vec<Item>, name: string) -> u64? {
    todo()
}
```

`return i` hands back a `u64` where a `u64?` goes, the same as passing a plain `Item` did
above, and `return none` says it isn't there.

`go` walks the player through an exit:

<!-- from: 07-when-there-might-be-nothing.rk -->
```rask
// exercise 07: Walk `dir` from here, and describe where that leads. If there's
// no exit that way, say so and stay put.
public func go(mutate world: World, dir: Direction) -> string {
    todo()
}
```

`room.exits.get(dir)` is the place of the room that way, as a `u64?`. When there's no
exit, answer `You can't go west.` with the direction's name. When there is, set
`world.here` to it and answer with `describe(world)`.

```bash
rask test 07-when-there-might-be-nothing.rk
```

<!-- from: output/07-exercise.out -->
```text
=== Testing 07-when-there-might-be-nothing.rk ===

  ✓ every direction leads back the way it came (0ms)
  ✗ finding an item by name
      07-when-there-might-be-nothing.rk:86: not yet implemented
  ✓ a direction alone is a move (0ms)
  ✓ the object keeps its case and inner spaces (0ms)
  ✓ what can go wrong is a value (0ms)
  ✓ the game starts at the gate (0ms)
  ✗ walking to the hall
      07-when-there-might-be-nothing.rk:274: not yet implemented
  ✗ a dead end and a typo
      07-when-there-might-be-nothing.rk:274: not yet implemented
  ✗ picking something up moves it
      07-when-there-might-be-nothing.rk:86: not yet implemented
  ✗ putting it down somewhere else
      07-when-there-might-be-nothing.rk:86: not yet implemented

──────────────────────────────────────────────────
10 tests, 5 passed, 5 failed (0ms)
```
