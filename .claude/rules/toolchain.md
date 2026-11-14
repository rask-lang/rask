---
paths:
  - "**/*.rk"
  - "compiler/**"
  - "runtime/**"
  - "stdlib/**"
  - "tests/**"
  - "examples/**"
---

# Toolchain

Binary: `compiler/target/release/rask`. Build: `cd compiler && cargo build --release -p rask-cli`.
Releases: https://github.com/rask-lang/rask/releases

| Command | Use |
|---------|-----|
| `rask lint <file>` | Naming, style and idiom issues in a .rk file |
| `rask test-specs <path>` | Spec code blocks parse; staleness warnings |
| `rask api <file>` | A module's public API |
| `rask fmt <file>` | Format .rk source |
| `rask check <file>` | Type-check a .rk file |
| `rask check --claims <dir>` | Also build and run the package's `claims.rk` ([type.claims](../../specs/types/claims.md)); `build`, `run` and `test` run them anyway |
| `rask claims <dir>` | A package's claims and verdicts; `--since <rev>` and `--mutate` for review |
| `rask help agents` | The language card for a coding agent; `rask init --agents` points AGENTS.md at it |
| `rask mcp` | check, claims, explain, api and the briefing over MCP (opt-in) |
| `rask run <file>` / `--interp` | Run natively / on the interpreter |
| `rask test <file>` / `--interp` | Run `test` blocks natively / on the interpreter |
| `rask compile --dump-mir <file>` | MIR before codegen |
| `rask bindc <header.h> [-o f.rk]` | The Rask a C header translates to: what `import c` compiles |

**Native is the default everywhere.** `--interp` opts out. `rask test --native`
is accepted and does nothing, so a bug "reproduced on the interpreter" with it
was native all along. When the backends disagree, the interpreter says what the
answer should be; it isn't what ships. A native crash, SIGILL, flake or
divergence: the `debug-native` skill.

Hooks run `rask lint` and `rask check` after editing `.rk` files (the skip list
is in `.claude/hooks/post-edit.sh`) and `rask test-specs` after editing `specs/*.md`.

## Things that waste time

- In a cloud session the binary comes from the environment snapshot and can be
  days older than the clone, while `runtime/*.c` is current. A stale binary gives
  raw `ld` undefined-symbol errors for `rask_*` internals that read like a codegen
  bug (#1041). A SessionStart hook rebuilds it in the background (log in
  `compiler/target/session-start-build.log`); run `cargo build --release -p
  rask-cli` before trusting it. [docs/cloud-cache.md](../../docs/cloud-cache.md)
  covers the snapshot.
- `stdlib/*.rk` is `include_str!`'d into the compiler, so an edit does nothing
  until you rebuild. `runtime/*.c` is compiled by the linker itself and cached by
  size+mtime in `$XDG_CACHE_HOME/rask/runtime` (or `RASK_RUNTIME_CACHE`), so an
  edit takes effect on the next compile. `librask_runtime.a` isn't linked by
  anything.
- A **failed** `rask build` exits 1 and leaves the previous binary in
  `build/debug/`. Check the exit code; don't pipe the build through `tail`.
- `rask build` caches objects in `build/.cache/`, keyed on source, profile,
  target and the compiler binary, so rebuilding `rask` invalidates it. `--force`
  or `--no-cache` bypasses. `rask run` and `rask compile` don't cache.
- `println` is fully buffered to a pipe, so output before a crash is lost.
  Always `stdbuf -o0 -e0 ./binary > log 2>&1`.
- **When a reduction won't reproduce, stop writing reductions.** Copy the real
  program, instrument it (print each field, each hop) and let it tell you which
  one is wrong.
