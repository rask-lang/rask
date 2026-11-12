# Branching and looping

The game's whole job is a loop: read a command, decide what it does, answer. This
chapter is the deciding and the looping.

## `if`

Describing a room ends one of two ways, depending on whether there's any way out:

<!-- from: early/game.rk -->
```rask
        if exits.is_empty() {
            return "{text}\nThere is no way out."
        }
        return "{text}\nExits: {exits.join(", ")}."
```

The condition is a `bool`, with no parentheses around it, and the braces are always
there. An `if` can also be a value, with a colon where the braces would be:

<!-- test: run | 1 exit -->
```rask
func main() {
    let count = 1
    let noun = if count == 1: "exit" else: "exits"
    println("{count} {noun}")
}
```

## `for`

A `for` loop runs its body once for each thing in a collection. Describing a room lists
what lies in it:

<!-- from: early/game.rk -->
```rask
        mut text = "{room.name}\n{room.description}"
        for item in room.items {
            text = "{text}\nThere is a {item.name} here."
        }
```

`item` is each item in turn. `text` is `mut` because the loop builds it up a line at a
time.

The exits come next. A room keeps them in a `Map`, which hands its entries back in no
particular order, so the loop goes through the directions in a fixed order of its own
and asks the room about each:

<!-- from: early/game.rk -->
```rask
        mut exits: Vec<string> = Vec.new()
        for dir in DIRECTIONS {
            if room.exits.contains(dir) {
                exits.push(dir.name())
            }
        }
```

`DIRECTIONS` is all six, north first. The next chapter has more about `Vec` and `Map`.

`for i in 0..n` counts from 0 up to, and not including, `n`. `while` runs as long as a
condition holds, and `loop` runs until something breaks out of it. A `loop` is also a
value, the one its `break` carries:

<!-- test: run | 3 steps, then 5 -->
```rask
func main() {
    mut steps = 0
    while steps < 3 {
        steps += 1
    }
    let found = loop {
        if steps == 5 {
            break steps
        }
        steps += 1
    }
    println("3 steps, then {found}")
}
```

## `match`

Chapter 4 used `match` to pick a word for each direction. It also takes values apart. A
variant that carries a value names it in the pattern, and the arm can use it:

<!-- test: run | You look around.\nYou go north.\nYou say "hello" -->
```rask
enum Command {
    Look
    Go(string)
    Say(string)
}

func answer(cmd: Command) -> string {
    return match cmd {
        Command.Look => "You look around.",
        Command.Go(way) => "You go {way}.",
        Command.Say(words) => "You say \"{words}\"",
    }
}

func main() {
    println(answer(Command.Look))
    println(answer(Command.Go("north")))
    println(answer(Command.Say("hello")))
}
```

`Command.Go(way)` matches any `Go` and calls what it carries `way`. A `match` can match
plain values too. Here's how the game turns a word into a direction, with `|` for "or"
and `_` for anything else:

<!-- from: mud/direction.rk -->
```rask
    public func parse(word: string) -> Direction? {
        return match word {
            "n" | "north" => Direction.North,
            "s" | "south" => Direction.South,
            "e" | "east" => Direction.East,
            "w" | "west" => Direction.West,
            "u" | "up" => Direction.Up,
            "d" | "down" => Direction.Down,
            _ => none,
        }
    }
```

`Direction?` and `none` mean "a direction, or nothing", for a word that isn't one. That's
chapter 7.

## Exercise

`05-branching-and-looping.rk` has two functions to write. `apply` decides what a command
does:

<!-- from: 05-branching-and-looping.rk -->
```rask
// exercise 05: Answer each command: look describes the room, a direction
// walks, take and drop move an item, and so on down the list.
public func apply(mutate world: World, cmd: Command) -> string {
    todo()
}
```

Every command has a function waiting for it in the same file: `describe`, `go`,
`pick_up`, `put_down` and `inventory`. `apply` is one `match` that calls the right one. For
`Say`, answer `You say "…"` with the words; for `Help`, the `HELP` text; for `Quit`,
`Goodbye.`. Where a call says `mutate world`, write it that way. Chapter 9 is about what
it means.

`script` plays a list of lines against the world, the way a player would type them:

<!-- from: 05-branching-and-looping.rk -->
```rask
// exercise 05: Look around first, then answer each line in turn. Every
// answer goes in the list, in order.
public func script(mutate world: World, lines: Vec<string>) -> Vec<string> {
    todo()
}
```

`respond(mutate world, line)` answers one line, and `describe(world)` is the look around.
Start with `mut replies: Vec<string> = Vec.new()`, add each answer with `replies.push(…)`,
and return it.

```bash
rask test 05-branching-and-looping.rk
```

<!-- from: output/05-exercise.out -->
```text
=== Testing 05-branching-and-looping.rk ===

  ✓ every direction leads back the way it came (0ms)
  ✓ finding an item by name (0ms)
  ✓ a direction alone is a move (0ms)
  ✓ the object keeps its case and inner spaces (0ms)
  ✓ what can go wrong is a value (0ms)
  ✓ the game starts at the gate (0ms)
  ✗ walking to the hall
      05-branching-and-looping.rk:344: not yet implemented
  ✗ a dead end and a typo
      05-branching-and-looping.rk:344: not yet implemented
  ✗ picking something up moves it
      05-branching-and-looping.rk:344: not yet implemented
  ✗ putting it down somewhere else
      05-branching-and-looping.rk:344: not yet implemented

──────────────────────────────────────────────────
10 tests, 6 passed, 4 failed (0ms)
```

Write both until they pass.
