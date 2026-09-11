# Examples

Complete programs, not fragments. Each one lives in
[examples/](https://github.com/rask-lang/rask/tree/main/examples) with a recorded output in
`tests/golden/`, which enrolls it in the example gate: every CI run compiles it on both
backends and diffs what it prints. A program listed here works, or the build is red.

| Program | What it exercises |
|---|---|
| [grep_clone.rk](https://github.com/rask-lang/rask/blob/main/examples/grep_clone.rk) | CLI flags, file reads, `catch` on the error branch, string scanning |
| [file_copy.rk](https://github.com/rask-lang/rask/blob/main/examples/file_copy.rk) | Error enums with `message()`, optionals, `own` at a call site |
| [game_loop.rk](https://github.com/rask-lang/rask/blob/main/examples/game_loop.rk) | Frame update, traits, worker threads. Still built on `Pool` + `Handle`, which racks and links replaced — read it for the loop, not for the storage |
| [parameter_modes.rk](https://github.com/rask-lang/rask/blob/main/examples/parameter_modes.rk) | Borrow, `mutate`, `take` — the subject of [Passing Values](../guide/passing-values.md) |

Run one:

```bash
rask run examples/grep_clone.rk -- -n pattern file.txt
```

There were walkthrough chapters here. They quoted code by hand, were checked only for
parsing, and drifted: the game-loop page taught `Pool<T>`, which the design replaced with
racks and links. Guide chapters now pull their code out of these programs
([how this book is built](https://github.com/rask-lang/rask/blob/main/docs/book/README.md)),
so a walkthrough can't say something the program doesn't do.
