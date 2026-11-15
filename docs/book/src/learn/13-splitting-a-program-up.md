# Splitting a program up

The exercise files so far have been the whole game in one file, a few hundred lines
long. The game itself lives in `mud/`, as a dozen files, each about one thing: directions,
items, the world, the rules, saving, the server. This chapter is about how those files
make one program, what the rest of the world gets to see of it, and a way to check
something about the whole game that no single test can.

## A directory is a package

Every `.rk` file in a directory belongs to one package, and the files see each other's
names without importing anything. `game.rk` uses `World` from `world.rk` and `Item` from
`item.rk` as if they were in the same file. The `rask.toml` beside them names the package:

<!-- from: mud/rask.toml -->
```toml
[package]
name = "mud"
version = "0.1.0"
description = "A small multiplayer text adventure: the program the book's tutorial builds"
license = "MIT OR Apache-2.0"
```

So splitting a program into files costs nothing and hides nothing. There's no privacy
between the files of a package, because the people who write one package are the same
people who can read all of it. Files are for keeping each concern in one place.

The commands that took a file take the directory instead. Testing it runs every test in
every file:

```bash
rask test mud
```

<!-- from: output/13-test.out -->
```text
Claims mud/claims.rk
  no_item_in_two_places    held for 1,000 inputs (sampled, not proven)
=== Testing mud ===

  ✓ a direction alone is a move (0ms)
  ✓ the object keeps its case and inner spaces (0ms)
  ✓ what can go wrong is a value (0ms)
  ✓ every direction leads back the way it came (0ms)
  ✓ finding an item by name (0ms)
  ✓ every room of the maze can be reached from its entrance (0ms)
  ✓ walking to the hall (0ms)
  ✓ picking something up moves it (0ms)
  ✓ a dead end and a typo (0ms)
  ✓ two players in one room hear each other (0ms)
  ✓ a collapsed tunnel leaves a blocked exit (0ms)
  ✓ leaving drops what you carried (0ms)
  ✓ the rat wanders (0ms)
  ✓ a name already in the game is refused (0ms)
  ✓ a list reads like a sentence (0ms)
  ✓ a saved world loads back the same (0ms)
  ✓ a broken save says what's wrong (0ms)
  ✓ two players meet and chat (0ms)
  SKIP a dropped connection leaves the room (sim-only: needs `rask test --sim` for fault injection)

──────────────────────────────────────────────────
19 tests, 18 passed, 0 failed, 1 skipped (0ms)
```

The first two lines are the claim this chapter ends on. The skipped test belongs to the
chapter on doing several things at once.

## `public` is for other packages

Code outside the package sees only what's marked `public`. That's a function, a type, a
field or a method with the word in front of it, and the package's interface to the world
is exactly that list. `rask api` prints it, file by file:

```bash
rask api mud/play.rk
```

<!-- from: output/13-api.out -->
```text
play (mud/play.rk)

  public func respond(mutate world: World, name: string, line: string) -> Outcome

  public func script(mutate world: World, name: string, lines: Vec<string>) -> Vec<string>
```

Another package that imports this one writes `import mud` and then `mud.respond(...)`,
with the package name in front. A function without `public`, like `tell_room` in
`game.rk`, can't be named from there at all, so the files are free to change it without
breaking anyone.

## A claim is a test that picks its own inputs

A test checks one case: this player, these commands, this answer. Some rules of the game
are about every case, and the most important one is the rule chapter 9 built the game
around: an item is in exactly one place. Whatever anyone types, the lamp is never on the
floor and in a pocket at once, and it never disappears.

`claims.rk` says that once, for all inputs:

<!-- from: mud/claims.rk -->
```rask
// Every item is in exactly one place: on the floor of one room or in one
// player's pack. Never two, never none.
claim no_item_in_two_places(moves: Vec<(bool, u8)>) {
    mut w = mud.default_world()
    mud.enter(mutate w, "ada", mud.START)!
    mud.enter(mutate w, "bo", mud.START)!
    let names = where_items_are(w)
    for (second, pick) in moves {
        let who = if second: "bo" else: "ada"
        let k = pick.to<u64>()! % LINES.len()
        let line = LINES[k]
        let _ = mud.respond(mutate w, who, line)
        let now = where_items_are(w)
        assert now.len() == names.len()
        for name in names {
            assert count(now, name) == 1
        }
    }
}
```

It reads like a test with parameters. The difference is who supplies them: the tool
generates a thousand lists of moves, each a list of (which player, which command) pairs,
plays every one, and checks the assertions after each step. Two players take turns
picking things up, dropping them and walking about, in orders nobody would think to write
down.

`claims.rk` sits in the directory, but it's compiled as a separate package that imports
`mud`, which is why everything it uses has `mud.` in front and is `public`. A claim checks
the package from the outside, the way anything else would use it.

```bash
rask claims mud
```

<!-- from: output/13-claims.out -->
```text
Claims mud/claims.rk
  no_item_in_two_places    held for 1,000 inputs (sampled, not proven)
```

"Sampled, not proven" is meant literally. A thousand games are a lot of evidence and no
guarantee. When a claim fails, the tool shrinks the input to the shortest list of moves
that still breaks it, and that list is your bug report. `rask test` and `rask build` run
the claims too, which is why they were the first lines of the test output.

## Building and playing it

`rask build` compiles the package into a program of its own, in `mud/build/debug/`:

```bash
rask build mud
```

`main.rk` plays alone in the terminal, the way chapter 10's game did, and saves the
world to `mud.save` whenever it ends:

```bash
printf 'take lamp\nn\ndrop lamp\nquit\n' | mud/build/debug/mud
```

<!-- from: output/13-play.out -->
```text
Gate
A rusted iron gate stands open to the north.
You see lamp.
Exits: north and south.
You take the lamp.
Courtyard
Weeds push up between the flagstones.
You see nothing.
Exits: south, east and up.
You drop the lamp.
```

Or run it with nothing piped in and play:

<!-- learn: skip -->
```bash
mud/build/debug/mud
```

This chapter has no exercise file. Instead, break the rule on purpose and watch the claim
notice. In `take_item` in `mud/game.rk`, these two lines move the item:

<!-- from: mud/game.rk -->
```rask
    let item = here.items.remove(i)
    p.inventory.push(item)
```

Replace them with `p.inventory.push(here.items[i].clone())`, which puts a copy in the
pack and leaves the original on the floor.

Then run `rask claims mud`. The claim fails, and the input it shows is the shortest game it
could find that breaks the rule: one move, a player taking the lamp. The test called
"picking something up moves it" catches this too, because someone thought to write it. The
claim would have caught it without anyone thinking of it. Put the two lines back
afterwards.
