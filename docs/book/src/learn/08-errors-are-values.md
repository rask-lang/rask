# Errors are values

A player can type anything. Most of it is a command, and the rest needs an answer that
says what went wrong. When a call can fail, the failure comes back out of it, as an
ordinary value.

## A type that says both

Reading what the player typed gives back a command, or a reason it isn't one:

<!-- from: mud/command.rk -->
```rask
public func parse(line: string) -> Command or ParseError {
```

`Command or ParseError` is the whole story in the signature. The reasons are an enum of
the game's own:

<!-- from: mud/command.rk -->
```rask
public enum ParseError {
    Empty
    UnknownVerb(string)
    MissingObject(string)
    NoSuchDirection(string)
}
```

Every error type has a `message()`, the sentence a person reads. An enum gets one
written for it, and `ParseError` writes its own, worded for a player:

<!-- from: mud/command.rk -->
```rask
extend ParseError {
    public func message(self) -> string {
        return match self {
            ParseError.Empty => "Say something.",
            ParseError.UnknownVerb(v) => "I don't know how to '{v}'. Try 'help'.",
            ParseError.MissingObject(v) => "{v} what?",
            ParseError.NoSuchDirection(w) => "'{w}' isn't a direction.",
        }
    }
}
```

Returning one is plain `return`. `return ParseError.Empty` from a function that returns
`Command or ParseError` is the error side, and `return Command.Look` is the other.

## `catch`: handle it here

`match` can take a result apart by type, the way chapter 7 took an optional apart. The
answer to "the player typed nonsense" is nearly always the same, though: say why, and
wait for the next line. `catch` is that arm on its own:

<!-- from: early/play.rk -->
```rask
public func respond(mutate world: World, line: string) -> string {
    let cmd = parse(line) catch e => return e.message()
    return apply(mutate world, cmd)
}
```

`catch e =>` names the error and says what to do with it. Here that's leaving with its
message. On the success path, `cmd` is a plain `Command`.

The name is required. `catch e =>` uses the error, `catch _ =>` throws it away on
purpose, and there's no third form where it disappears unwritten:

<!-- from: ../../errors/08-errors-are-values/bare_catch.out -->
```text
error[E0100]: `catch` needs a binder
  --> docs/book/errors/08-errors-are-values/bare_catch.rk:20:31
    |
 20 |     let verb = parse("dance") catch "look"
    |                               ^^^^^ here
    = fix: write `catch e => value` to use the error, or `catch _ => value` to
          drop it — there is no bare-value form, so an error is never swallowed
          silently
    = why: a fallback is the one form here that can destroy information. `??`
          discards a `none`, which carried nothing; `catch` discards or
          transforms an error, which carried a payload someone should look at.
          So the binder is what makes the second one legible — `catch _ =>` is
          the spelling for "an error dies here", and it greps [type.errors/ER14]
```

## `try`: hand it up

Parsing `go west` means parsing `west`, and that can fail too. When it does, `parse`
doesn't want to handle it, only to pass it on:

<!-- from: mud/command.rk -->
```rask
        "go" => Command.Go(try direction(rest)),
```

`try` takes the value out of a result, or returns the error to the caller unchanged. It's
`direction(rest) catch e => return e`, written once. That only works in a function with
somewhere to put the error, an `or` in its return type:

<!-- from: ../../errors/08-errors-are-values/try_without_a_carrier.out -->
```text
error[E0316]: `try` requires function returning Result or Option, found `void`
  --> docs/book/errors/08-errors-are-values/try_without_a_carrier.rk:20:16
    |
 20 |     let verb = try parse(line)
    |                ^^^^^^^^^^^^^^^ try used here
    = fix: change the function return type to `T or E`
    = why: `try` propagates errors upward — the enclosing function must declare
          an error type in its return
```

## `!`: stop

The last thing to do with an error is give up. `!` takes the value out, and stops the
program if it's an error:

<!-- from: ../../panics/08-errors-are-values/force_on_an_error.out -->
```text
panic at docs/book/panics/08-errors-are-values/force_on_an_error.rk:22: ! on a value that was an error: I don't know how to 'dance'.
```

That's a panic, which is not an error value. It has no type, no caller can handle it, and
it stops the task it happens in. The tests use `!` because a test that gets an error where
it expected a command has already failed:

<!-- from: mud/command.rk -->
```rask
test "a direction alone is a move" {
    assert parse("n")! == Command.Go(Direction.North)
    assert parse("  go   west ")! == Command.Go(Direction.West)
}
```

For anything a caller could deal with, hand it back with `try`.

## Pieces of a string

`parse` splits the line at its first space:

<!-- from: mud/command.rk -->
```rask
    mut verb = text.to_lowercase()
    mut rest = ""
    if text.index_of(" ")? as i {
        verb = text[0..i].to_lowercase()
        rest = text[i + 1..].trim().to_string()
    }
```

`text[0..i]` is the part of `text` from position 0 up to `i`, without copying anything. It
can't outlive the line it's on, because it points into `text`, so a piece that's kept goes
through `.to_string()` (or another method that makes a new string, like `to_lowercase`).
`index_of` answers a `usize?`, since there might not be a space.

## Exercise

`08-errors-are-values.rk` has the two helpers `parse` calls left empty:

<!-- from: 08-errors-are-values.rk -->
```rask
// exercise 08: The direction `word` names, or why it doesn't name one.
func direction(word: string) -> Direction or ParseError {
    todo()
}

// exercise 08: What `verb` acts on, or an error when nothing follows it.
func object(verb: string, rest: string) -> string or ParseError {
    todo()
}
```

`go` with nothing after it is `ParseError.MissingObject("go")`, and a word that isn't a
direction is `ParseError.NoSuchDirection` with the word. `Direction.parse(word)` answers a
`Direction?`. `take`, `drop` and `say` with nothing after the verb are `MissingObject`
with the verb.

```bash
rask test 08-errors-are-values.rk
```

<!-- from: output/08-exercise.out -->
```text
=== Testing 08-errors-are-values.rk ===

  ✓ every direction leads back the way it came (0ms)
  ✓ finding an item by name (0ms)
  ✗ a direction alone is a move
      08-errors-are-values.rk:167: not yet implemented
  ✗ the object keeps its case and inner spaces
      08-errors-are-values.rk:172: not yet implemented
  ✗ what can go wrong is a value
      08-errors-are-values.rk:172: not yet implemented
  ✓ the game starts at the gate (0ms)
  ✓ walking to the hall (0ms)
  ✓ a dead end and a typo (0ms)
  ✗ picking something up moves it
      08-errors-are-values.rk:172: not yet implemented
  ✗ putting it down somewhere else
      08-errors-are-values.rk:172: not yet implemented

──────────────────────────────────────────────────
10 tests, 5 passed, 5 failed (0ms)
```
