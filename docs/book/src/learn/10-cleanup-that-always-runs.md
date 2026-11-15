# Cleanup that always runs

Up to now the game has played a list of commands written into the program. This chapter
makes it playable: it reads what you type, and when you come back later it starts where
you left off. That means writing a file, and a file is the first value in the tutorial
that has to be finished with, not just dropped.

## Reading what the player types

`io.read_line()` waits for a line from the keyboard and answers it without the newline.
It answers `string or IoError`, and the error case is mostly the end of input (Ctrl-D in
a terminal, or the end of a file piped in), so the game takes either as the signal to
stop:

<!-- from: early/alone.rk -->
```rask
    loop {
        let line = io.read_line() catch _ => break
        if line == "quit" {
            break
        }
        println(respond(mutate world, line))
        typed.push(line)
    }
```

Every line the player types goes into `typed` too. That list is the save: play the same
lines again on a fresh world and you end up exactly where you were, so the game writes
them to a file at the end and reads them back at the start.

## A file has to be closed

`fs.create_file` opens a file for writing and answers a `File`. Up to now every value
went away by itself when its name did. A `File` can't, because going away isn't free: it
holds a handle the operating system lent the program, and the last of what was written
may still be sitting in a buffer. Something has to close it. Rask has no destructors,
no hidden code at a closing brace, so the program does it, by calling `close()`. The
compiler keeps track of every `File` until that call, and refuses a program where some
way out of the function skips it.

The obvious version closes it at the end:

<!-- from: ../../errors/10-cleanup-that-always-runs/close_at_the_end.rk -->
```rask
func save(lines: Vec<string>, path: string) -> void or IoError {
    let file = try fs.create_file(path)
    for line in lines {
        try file.write_text("{line}\n")
    }
    try file.close()
}
```

and it doesn't build:

<!-- from: ../../errors/10-cleanup-that-always-runs/close_at_the_end.out -->
```text
error[E0881]: `file` would leak if this fails
  --> docs/book/errors/10-cleanup-that-always-runs/close_at_the_end.rk:8:9
    |
  6 |     let file = try fs.create_file(path)
    |     ----------------------------------- `file` was acquired here
   ...
  8 |         try file.write_text("{line}\n")
    |         ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ the error leaves the function here, with `file` still open
    = fix: add `ensure file.close()` on the line after it's acquired
    = why: a resource has to be consumed on every way out of the scope, and `try`
          is one of them
```

A `try` is a way out. If a write fails halfway, the function returns from that line and
the `close()` at the bottom never runs. The compiler calls a value like this a
**resource**: one that has to be consumed on every path, not only the one you were
thinking of when you wrote it.

## `ensure`

`ensure` takes a line of code and runs it when the block it's in ends, however it ends:

<!-- test: run | The torch goes out.\nThe door is locked.\nThe torch goes out.\nYou go in. -->
```rask
func enter(locked: bool) -> string {
    ensure println("The torch goes out.")
    if locked {
        return "The door is locked."
    }
    return "You go in."
}

func main() {
    println(enter(true))
    println(enter(false))
}
```

Both calls put the torch out, before the caller gets the answer. Falling off the end,
`return`, a `try` that fails, `break` out of a loop and a panic all leave the block, and
all of them run what it scheduled.

So the fix the compiler suggested is a line, right after the file is opened:

<!-- from: early/alone.rk -->
```rask
    let file = try fs.create_file(path)
    ensure file.close()
```

Opening and closing now sit next to each other, which is where a reader checks that they
match. Everything after that line can fail however it likes. The compiler wants it on the
very next line for the same reason: anything in between could fail with the file open and
nothing scheduled.

Several `ensure`s in one block run last first, so whatever was set up last is taken down
first:

<!-- test: run | You light the lamp.\nYou open the door.\nYou look around.\nYou close the door.\nYou blow out the lamp. -->
```rask
func main() {
    println("You light the lamp.")
    ensure println("You blow out the lamp.")
    println("You open the door.")
    ensure println("You close the door.")
    println("You look around.")
}
```

## When the cleanup itself fails

Closing a file can fail too. By the time an `ensure` runs, the block is already leaving,
maybe with an error of its own, so there's nowhere sensible to send a second one. An
`ensure` drops its line's error unless you say what to do with it, with the same `catch`
as anywhere else.

The game uses that for the save itself. `main` schedules it once, as soon as it has the
list of lines:

<!-- from: early/alone.rk -->
```rask
    ensure save(typed, LOG) catch e => println("Couldn't save: {e.message()}")
```

It runs when `main` ends, with `typed` as it is then, so it saves everything the player
typed, including the lines after this one. A disk that's full gets a message instead of
silence. And if the game panics halfway through a command, the save still runs, because a
panic leaves `main` like anything else.

## Exercise

`10-cleanup-that-always-runs.rk` is the game with `save` left empty:

<!-- from: 10-cleanup-that-always-runs.rk -->
```rask
// exercise 10: Write each of `lines` to the file at `path`, one per line.
// Open it with `fs.create_file`, and close it whichever way this ends.
func save(lines: Vec<string>, path: string) -> void or IoError {
    todo()
}
```

`file.write_text(text)` writes a string and answers `void or IoError`.

```bash
rask test 10-cleanup-that-always-runs.rk
```

<!-- from: output/10-exercise.out -->
```text
=== Testing 10-cleanup-that-always-runs.rk ===

  ✓ every direction leads back the way it came (0ms)
  ✓ finding an item by name (0ms)
  ✓ a direction alone is a move (0ms)
  ✓ the object keeps its case and inner spaces (0ms)
  ✓ what can go wrong is a value (0ms)
  ✓ the game starts at the gate (0ms)
  ✓ walking to the hall (0ms)
  ✓ a dead end and a typo (0ms)
  ✓ picking something up moves it (0ms)
  ✓ putting it down somewhere else (0ms)
  ✗ what was typed comes back
      10-cleanup-that-always-runs.rk:443: not yet implemented
  ✓ no log is a new game (0ms)

──────────────────────────────────────────────────
12 tests, 11 passed, 1 failed (0ms)
```

Once it passes, play. Type commands, then `quit`, then start it again and look around:

<!-- learn: skip -->
```bash
rask run 10-cleanup-that-always-runs.rk
```

The lines you typed are in `mud.log`, one per line. Delete it to start a new game.
