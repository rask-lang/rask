# Learn the language

These chapters build one program: a small text adventure, the kind called a MUD,
where several people can walk the same rooms and talk to each other. It's a good fit
for learning Rask because the game's rules are the language's rules. An item lies in
one room or sits in one pocket, never both, so picking it up is a move. Rooms lead to
rooms. Later, each player is a task of its own.

They're written for someone who has programmed before, in anything. Where a language
you know would lead you to expect the wrong thing, the chapter says so.

## How to follow along

Every chapter works with nothing but the `rask` binary and a text editor. The website
is the same text with Run buttons on top. If you'd rather read offline, clone the
repository and open this file in an editor; the chapters are plain Markdown and
everything they show is next to them in this directory.

<!-- learn: skip -->
```bash
git clone --depth 1 https://github.com/rask-lang/rask
cd rask/docs/book/src/learn
```

Every command in the chapters runs from this directory.

Each chapter from 3 on comes with an exercise file named after it, like
`04-types-of-your-own.rk`. It is the program so far, with the functions that chapter is
about left empty: their bodies say `todo()`, and a comment above each says what it
should do. Run its tests, and they fail where the program reaches something you haven't
written yet:

<!-- learn: skip -->
```bash
rask test 04-types-of-your-own.rk
```

Fill in the functions until everything passes. The finished program is in `early/`
and `mud/`, if you want to compare or you get stuck. Nothing checks that you wrote it the
same way, only that the tests pass.

## The chapters

- [Your first program](03-your-first-program.md): `main`, printing, names
- [Types of your own](04-types-of-your-own.md): rooms, items and directions
- [Branching and looping](05-branching-and-looping.md): answering a command
- [Holding many things](06-holding-many-things.md): the rooms of the world
- [When there might be nothing](07-when-there-might-be-nothing.md): an exit that isn't there
- [Errors are values](08-errors-are-values.md): reading what the player typed
- [Passing values](09-passing-values.md): who has the lamp
