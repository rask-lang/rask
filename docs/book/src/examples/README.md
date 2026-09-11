# Examples

Complete programs, not fragments. Each one lives in
[examples/](https://github.com/rask-lang/rask/tree/main/examples) with a recorded output in
`tests/golden/`, which enrolls it in the example gate: every CI run compiles it on both
backends and diffs what it prints. A program listed here works, or the build is red.

| Program | What it exercises |
|---|---|
| [grep_clone.rk](https://github.com/rask-lang/rask/blob/main/examples/grep_clone.rk) | CLI flags, file reads, `catch` on the error branch, string scanning |
| [file_copy.rk](https://github.com/rask-lang/rask/blob/main/examples/file_copy.rk) | Error enums with `message()`, optionals, `own` at a call site |
| [game_loop.rk](https://github.com/rask-lang/rask/blob/main/examples/game_loop.rk) | Frame update, traits, worker threads. Read it for the loop, not the storage — see below |
| [text_editor.rk](https://github.com/rask-lang/rask/blob/main/examples/text_editor.rk) | Undo stack, `ensure` cleanup, linear resources. Same storage caveat |
| [parameter_modes.rk](https://github.com/rask-lang/rask/blob/main/examples/parameter_modes.rk) | Borrow, `mutate`, `take` — the subject of [Passing Values](../guide/passing-values.md) |

Run one:

```bash
rask run examples/grep_clone.rk -- -n pattern file.txt
```

Two of these store their entities in a `Pool` with `Handle`s, which racks and links have
replaced. That's sequencing rather than neglect: migrating the examples is step 5 of
[#908](https://github.com/rask-lang/rask/issues/908), and it waits on an answer for
serialization — a handle is an integer that survives a round trip, and a link is an address,
so there's no link analogue yet. Copy the loop and the undo stack from these; take the
storage pattern from [racks.md](https://github.com/rask-lang/rask/blob/main/specs/memory/racks.md).

There were walkthrough chapters here. They quoted code by hand, were checked only for
parsing, and drifted: the game-loop page taught `Pool<T>`, which the design replaced with
racks and links. Guide chapters now pull their code out of these programs
([how this book is built](https://github.com/rask-lang/rask/blob/main/docs/book/README.md)),
so a walkthrough can't say something the program doesn't do.
