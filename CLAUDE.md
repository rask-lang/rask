Keep docs short.

## Talk to me in plain language

I know the compiler pipeline — MIR, lowering, codegen, and the rest aren't the problem. What loses me is dense, abstract phrasing: spec-speak, piled-up noun phrases, and internal rule IDs dropped into a sentence as if they explained something. Explain your work so a colleague could follow it at a whiteboard — in chat, and in commit/PR descriptions.

- Lead with the plain-English point: what broke, what you changed, why. Save the fine detail for after.
- Say it like a person, not the spec. "Both variables were pointing at the same struct, so editing one edited the other" beats "the aggregate aliases the source cell."
- Don't cite rule IDs (VS1, PM2, …) as if they carry meaning — say what the rule actually requires, in words.
- Show a concrete example when you can: this input → got X → now gives Y.
- Don't over-explain things I clearly know. The goal is clarity, not a glossary.
- Short and clear wins. Three sentences I understand beat a precise paragraph I don't.

(This is about explaining to me. Code comments still follow the style guide below.)

## I work from my phone

Chat is my interface. Reading diffs and files on a phone is painful — never point me at a diff as the explanation. The explanation in chat IS the deliverable.

- Bring code to me, don't send me to it: quote the 10–20 relevant lines in chat with your commentary when a decision needs my eyes.
- End every substantial piece of work with a **judgment digest**: the 2–4 most questionable calls you made, each with the few lines of code that embody it, framed so I can react in one word ("keep / wrong / show me more"). Put decisions you need from me first, not buried after the summary.
- The durable record lives in **commit messages and issues** — write those to stand alone, they're what future sessions read. PR bodies get one sentence plus the closing keywords (`Closes #N`); nobody reads more than that there.

Prefer long term proper fixes over quick-fixes.

Choose simple over easy.

## Design is mostly settled

The big decisions are made (see **Decided** table below). Don't re-derive them and don't open design debates inside implementation tasks. Spend energy on implementation, rough edges, and bugs.

If something genuinely seems wrong, flag it once with a concrete reason — then drop it unless I bite. Keep critique pointed; no broad "have you considered" rounds on settled areas.

### Nothing is stable — settled is not frozen

Settled means "don't reopen it for fun". It does not mean "can't be changed".
Nobody is using this language. There are no downstream users, no released API, no
migration to plan for. **Backward compatibility is never a reason for anything.**

So when a design turns out to be wrong, change it — all of it, everywhere, in one
go. I would rather have a large destructive change that leaves the design better
than a careful patch that keeps a bad shape alive. Renaming a method with different
semantics, deleting an operator, rewriting a decided spec, breaking every call site
in the repo: all fine, all cheap, do it properly.

What this rules out:

- Hedging a change to avoid breaking existing code. There is no existing code
  worth protecting — the repo is ours and the compiler will find every call site.
- Leaving a stub that wears a real name. A method that silently does nothing is
  worse than a missing one: implement it or delete it.
- "Keep these two copies in step" comments. Duplication a human has to maintain is
  rot with a delay fuse — generate it from one source or collapse it.
- Deprecation periods, aliases, compatibility shims. Delete the old spelling.

If you catch yourself weighing "how much would this break", you are weighing the
wrong thing. Weigh whether the result is better.

### Don't re-litigate

- **Clone cost is intentional.** Types >16 bytes require explicit `.clone()` even when all fields are Copy. This is the transparency principle — the cost is visible. Don't suggest raising the Copy threshold, making clones implicit, or treating this as a problem to solve. It's a deliberate tradeoff.
- **The box family is closed.** `Shared` (with its `Local`/`Readers`/`Mutex` strategies), `Rack`+`Link`, `Heap`, `Pool` (deprecated), `Atomic*`, `string` are compiler types; users can't build equivalents, and there's no unsafe hatch for it. Argued in `specs/memory/boxes.md` (BX1–BX4). Don't propose one.

# Working relationship

- No sycophancy.
- Be direct, matter-of-fact, and concise.
- Be critical; challenge my reasoning.
- Don’t include timeline estimates in plans.
- Don’t add yourself as a co-author to git commits.
- When creating a PR, tag the issues it resolves in the body with closing keywords (`Closes #N` / `Fixes #N`) so GitHub auto-closes them on merge. Issues it only relates to get a plain `#N` reference.

## Keep going

**Don't hand back unless there's a decision only I can make.**

Bug crunching has no decisions in it — work the whole list. A judgment call with a
defensible answer isn't a decision for me: pick it, say which way you went, carry
on. When the two backends disagree, the interpreter is the reference — read it
instead of asking. "I've been at this a while" is not a reason to stop.

Do stop for: a change that would make the flagship example worse, a spec question
with no answer in `specs/`, or anything needing credentials or an outward-facing
action.

Messy is fine. A branch with eight fixes and two written-up dead ends beats three
fixes and a status report. If a fix trades one failure for a worse one, revert it
and file what you learned — that's a finished piece of work, not a blocker.

## Debugging discipline

- Understand before changing. If you can't explain why something is broken, you're not ready to fix it.
- Fix causes, not symptoms. State the root cause in one sentence before writing the fix; if the diff doesn't touch the thing that sentence names, it's a patch, not a fix — trace deeper. Workarounds are allowed only when labeled as workarounds (in the code comment and the commit) with the real cause filed as an issue.
- **When a reduction won't reproduce, stop writing reductions.** Guessing at what's
  essential can cost hours and still miss. Copy the real program, instrument it —
  print each field, each hop — and let it tell you which one is wrong. Two of this
  round's bugs were found in one pass that way after several failed guesses.
- **Pre-existing errors that surface during unrelated work get filed, not ignored.** If a test fails, the compiler panics, or a spec breaks for reasons unrelated to your current change, search `rask-lang/rask` issues first; if it's not tracked, open one with a minimal repro before moving on. Don't paper over it, don't only mention it in chat, don't bundle it into the current commit silently.

**Tool usage:**
- Use `Write` tool for creating test files, not `Bash` with cat/heredocs
- Avoid pipes (`|`), redirects (`2>&1`), and command chaining (`&&`) in Bash commands - they break permission matching
- Run commands separately instead of chaining them
- Create test scripts in `/tmp`, not the main project folder

**CLI tools** (binary at `compiler/target/release/rask`):

| Command | Use |
|---------|-----|
| `rask lint <file>` | Check .rk files for naming/style/idiom issues |
| `rask test-specs <path>` | Verify spec code blocks parse + show staleness warnings |
| `rask api <file>` | Show a module's public API (structs, funcs, enums) |
| `rask fmt <file>` | Format .rk source files |
| `rask check <file>` | Type-check a .rk file |
| `rask run <file>` | Execute a .rk program (native) |
| `rask run --interp <file>` | Execute via interpreter (no codegen) |
| `rask test <file>` | Run `test` blocks — **native**, like `run` |
| `rask test --interp <file>` | Run `test` blocks on the interpreter |
| `rask compile --dump-mir <file>` | Print MIR before codegen (debug codegen issues) |

**Native is the default everywhere.** `rask run` and `rask test` both compile and
run natively; `--interp` opts out. `rask test --native` is accepted and does
nothing, so a bug you "reproduced on the interpreter" with it was native all
along. When the two disagree the interpreter is the reference for *what the answer
should be* — it isn't what anyone ships.

Binary: `compiler/target/release/rask` (build: `cd compiler && cargo build --release -p rask-cli`)
Releases: https://github.com/rask-lang/rask/releases

**Debugging codegen:** If a compiled binary segfaults, use `--dump-mir` to inspect the MIR and `RASK_RUNTIME_CHECKS=1 ./binary` to turn null-deref segfaults into panics with messages. Compile the C runtime with `-DRASK_DEBUG` for unconditional checks.

`RASK_POISON_STACK=1 ./binary` fills the stack with `0xAA` before `main` and before each worker thread's tasks. A slot codegen forgot to write reads as zero on a fresh stack and looks fine, so those bugs only appear once a program has run a while — and vanish the moment you reduce them. Poisoning makes them fire on the first call instead. That's what turned #577 from 40% flaky into 10/10.

If the compiler panics saying a name "belongs to `Vec`" but nothing declares it, MIR has minted an internal spelling nobody accounted for. `INTERNAL_SPELLINGS` in `rask-stdlib/src/mir_metadata.rs` says what each one stands for, and the panic is deliberate — the alternative answer, "no declaration, so the caller owns what came back", frees a string the container still holds. `RASK_LIST_UNMAPPED_SPELLINGS=1` reports each one and carries on instead of stopping at the first, so one sweep over the corpus lists them all.

SIGILL means a Cranelift trap — an `unreachable` was reached, usually a match on an out-of-range tag. `gdb -batch -ex run -ex 'bt 25' ./binary` gets the frame.

**Three things that will waste your time:**

- `stdlib/*.rk` is `include_str!`'d into the compiler (`rask-stdlib/src/stubs.rs`), so editing it does nothing until `cargo build --release -p rask-cli`. `runtime/*.c` is different: the linker compiles those sources itself and caches the objects keyed by size+mtime, so an edit takes effect on the next compile with no rebuild and no `make`. (`librask_runtime.a` isn't linked by anything — `make` in `compiler/runtime` builds it for its own sake.) Cached objects live in `$XDG_CACHE_HOME/rask/runtime`, or `RASK_RUNTIME_CACHE` if set.
- A **failed** `rask build` exits 1 and leaves the previous binary in `build/debug/`. Run it without checking and you're testing old code — which reads exactly like "my fix didn't work". Don't pipe the build through `tail`; check the exit code.
- `rask build` caches Rask object files in `build/.cache/*.o` (separate from the runtime object cache above). The key covers source, profile, target **and the compiler binary** (path + size + mtime), so rebuilding `rask` invalidates it on its own — no `rm -rf build/.cache` needed. Two compilers keep separate entries rather than evicting each other, so alternating between builds still hits. `rask build --verbose` prints the compiler fingerprint on a cache hit; `--force` or `--no-cache` bypasses. (`rask run` / `rask compile` on a single file don't cache at all.)
- `println` is fully buffered to a pipe, so output before a crash is lost. Always `stdbuf -o0 -e0 ./binary > log 2>&1`, or you'll place the crash earlier than it is.

Hooks auto-run `rask lint` after editing `.rk` files and `rask test-specs` after editing `specs/*.md`.

# Rask Writing Style Guide

**Core principle:** Sound like a developer with a vision, not a committee or AI. Natural flow over perfect grammar.

Add `// SPDX-License-Identifier: (MIT OR Apache-2.0)` to the top of source code files (.rs, .rk), not docs (.md)

## Documentation (Markdown)
Dont be TOO consistent.

**Use "I" for design choices:**
- ✅ "I chose handles over pointers—indirection cost is explicit"
- ❌ "It was decided that handles should be used"

**Keep technical sections neutral:**
- ✅ "References cannot outlive their lexical scope"
- ❌ "I make sure references cannot outlive scope"

**Be direct about tradeoffs:**
- ✅ "This means more `.clone()` calls. I think that's better than lifetime annotations"
- ❌ "While this may result in additional clones, it provides benefits..."

**Remove filler:** "It should be noted", "In order to", "With regard to"

**Natural language OK:** Contractions, slight grammar quirks, Scandinavian English flow

## Code Comments (Rust)

**Neutral and direct - no "I":**
- ✅ `// Skip to next declaration after error`
- ❌ `// I skip to the next declaration`

**Remove:**
- Obvious docs: `/// Get current token`
- Restating code: `// Check for X` when obvious
- Statement markers: `// While loop`
- AI explanations

**Keep:**
- Section headers
- Non-obvious algorithms
- Important constraints (tightened)
- "Why" not "what"

**Tighten everything:**
- ✅ `/// Record error, return if should continue`
- ❌ `/// Record an error and return a boolean indicating whether we should continue`

## Summary

**Docs:** "I" for design, neutral for tech specs, be direct, natural flow over grammar
**Code:** Neutral/direct, remove obvious, tighten verbose, no "I"
**Overall:** Sound like a developer with vision, own tradeoffs, no corporate speak


## Rask Syntax

**Claude: Use Rask syntax, not Rust.** Full reference: [specs/SYNTAX.md](specs/SYNTAX.md)

Key differences from Rust: `let`/`mut` (never `let mut`; `const` is module-level constants only), `func` (not `fn`), `extend` (not `impl`), `public` (not `pub`), `string` (lowercase), `Token.Plus` (not `::`), `try expr` (not `?`), `T or E` (not `Result<T,E>`), explicit `return` in functions, newlines as terminators.


## Compiler

Pipeline: `.rk → Lexer → Parser → Desugar → Resolve → TypeCheck → Comptime → Ownership → MIR → Codegen/Interp`

For detailed per-crate file maps: [compiler/CLAUDE.md](compiler/CLAUDE.md)

| Task | Start here |
|------|-----------|
| Parse error / new syntax | `rask-parser/src/parser.rs` |
| AST node types | `rask-ast/src/{decl,expr,stmt}.rs` |
| Operator desugaring | `rask-desugar/src/lib.rs` |
| Name resolution | `rask-resolve/src/resolver.rs`, `scope.rs` |
| Type error / inference | `rask-types/src/checker/{check_expr,check_stmt,inference,unify}.rs` |
| Trait / generics | `rask-types/src/checker/{generics,resolve}.rs` |
| Borrow checking | `rask-types/src/checker/borrow.rs`, `rask-ownership/` |
| Monomorphization | `rask-mono/src/{reachability,instantiate,layout}.rs` |
| MIR lowering | `rask-mir/src/lower/{mod,expr,stmt}.rs` |
| MIR codegen (Cranelift) | `rask-codegen/src/{builder,module}.rs` |
| Interpreter bugs | `rask-interp/src/interp/`, `rask-interp/src/stdlib/` |
| Stdlib types/stubs | `rask-stdlib/src/{stubs,types,builtins}.rs` |
| Error formatting | `rask-diagnostics/src/{formatter,convert}.rs` |
| CLI commands | `rask-cli/src/commands/`, `main.rs` |
| Formatter | `rask-fmt/src/printer.rs` |

## Stdlib design

The stdlib should feel Rask, not Rust-with-different-keywords. Don't lift names, shapes, or layering from `std::*` just because they're familiar.

- Pick names from how Rask programs read, not from Rust precedent. `Vec`/`Map` survive because they fit; `Result`, `Option`, `Box`, `Rc`, `RefCell`, `Arc<Mutex<T>>` do not — Rask has `T or E`, `T?`, `Owned`, `Shared`, `Cell`, `Mutex`.
- Method names follow Rask conventions, not Rust's (`unwrap`, `expect`, `ok_or`, `and_then` are Rust idioms; design from the actual operation, not the cheat sheet).
- Layering should reflect Rask's box family and linearity rules — don't import Rust's trait hierarchy (`Deref`, `Borrow`, `AsRef`, `Iterator` adapters) by reflex.
- When in doubt, sketch how the call site reads in a real Rask program first, then pick the name.

If a Rust name genuinely is the right one, fine — but justify it from Rask's side, not from "that's what `std` calls it."

Formal rules (one screen per module, the guess test, SD1–SD5): [specs/stdlib/api-design.md](specs/stdlib/api-design.md).

## Error messages

Diagnostics are a first-class feature, not an afterthought. A confusing error is a bug.

- All user-facing compiler errors go through `rask-diagnostics` (`compiler/crates/rask-diagnostics/src/`). Don't `eprintln!` or `panic!` your way out of an error path.
- Every diagnostic must explain **what's wrong, where, and what to do next** — not just restate the failed check. If the message is "expected `T`, found `U`", that's a starting point, not the finished message.
- Prefer suggestions (`suggestions.rs`) over prose when there's an obvious fix. Show the fix as code, not as a sentence.
- Use stable error codes (`codes.rs`) so messages can be looked up and improved over time.
- When you add a new error path, write the message before writing the check — if you can't phrase it clearly, the check probably isn't the right shape.

## Goal

Systems language where **safety is invisible**. Eliminate abstraction tax, cover 80%+ of real use cases.

**Non-negotiable:** Feel simple, not safe. Safety is a property, not an experience.

## Core Principles

Unifying thread: **safety through visibility.** Safety mechanisms are visible in source (explicit `ensure`, `mutate`, `take`, `own`, scoped `with`) rather than hidden in destructors, lifetime annotations, or effect types. The compiler guarantees invariants; the source shows the mechanism.

1. **Transparency of Cost** — Major costs visible in code (allocations, locks, I/O). Small costs (bounds checks) can be implicit.
2. **Mechanical Safety** — Safety by structure. Data races, null derefs, and dangling pointers impossible by construction; use-after-free through a stale handle is caught at the access, never silent.
3. **Practical Coverage** — Handle web services, CLI, data processing, embedded. Not limited to fixed-size programs.
4. **Ergonomic Simplicity** — Low ceremony. If Rask needs 3+ lines where Go needs 1, question the design.
5. **Information Without Enforcement** — Track effects, captures, and modes as metadata surfaced via tooling (IDE ghosts, lints) instead of type-system constraints. No function coloring, no effect polymorphism.

Full nine-principle set: [specs/CORE_DESIGN.md](specs/CORE_DESIGN.md). Scoring methodology: [METRICS.md](specs/METRICS.md).

---

## Design Status

Start with [CORE_DESIGN.md](specs/CORE_DESIGN.md). For specs: [specs/README.md](specs/README.md). For spec ID conventions and citation format: [specs/CONVENTIONS.md](specs/CONVENTIONS.md).

**Citing spec rules:** `spec-id/rule-id` — e.g., `mem.ownership/O1`, `type.structs/M3`. See CONVENTIONS.md for the full ID scheme.

### Decided

| Area | Decision | Spec |
|------|----------|------|
| Ownership | Single owner, move semantics, 16-byte copy threshold | [memory/](specs/memory/) |
| Borrowing | Block-scoped (fixed sources), inline + `with` (growable sources) | [borrowing.md](specs/memory/borrowing.md) |
| Linearity | Consume exactly once (L1–L6) — shared by `@resource`, `Owned<T>`, `Pool<Linear>` | [linear.md](specs/memory/linear.md) |
| Boxes | Container family with `with`-scoped access — `Shared<T, S>`, Rack+Link, Heap. `Cell`/`Mutex` are strategies, not types | [boxes.md](specs/memory/boxes.md) |
| Collections | Vec, Map, Rack+Link for graphs | [collections.md](specs/stdlib/collections.md), [racks.md](specs/memory/racks.md) |
| Resource types | `@resource` annotation for I/O handles, transactions; `ensure` cleanup | [resource-types.md](specs/memory/resource-types.md) |
| Types | Primitives, structs, enums, generics, traits, unions, tuples, nominal types, type aliases | [types/](specs/types/) |
| Errors | `T or E` result, `try` propagation, `T?` optionals, `todo()`/`unreachable()` | [error-types.md](specs/types/error-types.md) |
| Panics | Task-kill + unwind, ensures run, locks release without poisoning, opt-in `staged()` | [panics.md](specs/control/panics.md) |
| Concurrency | spawn(\|\| {})/join/detach (functions), channels, no function coloring | [concurrency/](specs/concurrency/) |
| Comptime | Compile-time execution | [comptime.md](specs/control/comptime.md) |
| C interop | Unsafe blocks, raw pointers | [unsafe.md](specs/memory/unsafe.md) |
| Rust interop | compile_rust() in build scripts, C ABI, cbindgen | [build.md](specs/structure/build.md) |
| Encoding | `comptime for` + field access, auto-derived Encode/Decode, field annotations | [encoding.md](specs/stdlib/encoding.md) |
| Networking | TCP, UDP, DNS resolution | [net.md](specs/stdlib/net.md) |
| HTTP | Client + server, linear Responder, HttpClient | [http.md](specs/stdlib/http.md) |
| Time | Duration, Instant, SystemTime, Duration scaling | [time.md](specs/stdlib/time.md) |
| OS | Env, args, subprocess spawning, signal handling | [os.md](specs/stdlib/os.md) |
| Compiler architecture | IR layers, SSA pipeline, analysis framework, pass manager, CTFE, debug info | [architecture.md](specs/compiler/architecture.md) |
| Code generation | MIR-based pipeline, Cranelift backend, runtime library | [codegen.md](specs/compiler/codegen.md) |
| Raido | Deterministic scripting VM — 32.32 fixed-point, serializable state, content-addressed bytecode. Independent project, also serves as verification engine for Allgard's verifiable transforms | [raido/](projects/raido/) |
| Leden | Capability-based networking protocol — sessions, capabilities, object references, gossip discovery | [leden/](projects/leden/) |
| Allgard | Federation model — primitives, conservation laws, domain sovereignty, bilateral trust, owner presence, distributed beacon | [allgard/](projects/allgard/) |
| GDL | Gard Description Language — content schema for describing gards over Leden. Regions, entities, affordances, appearance, style system, spatial protocol | [gdl/](projects/gdl/) |
| Midgard | Virtual world example — uses Raido, Allgard, Leden, GDL together | [midgard/](projects/midgard/) |
| Apeiron | Federated space game — seed-generated galaxy, player-hosted star systems, ships as domains. Sub-specs: combat, economy, elements, exploration, factions, physics, transformation, salvage, reputation, contracts, knowledge, sensors, social, navigation, market | [apeiron/](projects/apeiron/) |

### Open

| Area | Status |
|------|--------|
| Build system | Working, including cross-package symbol export |
| Macros/attributes | No macro system (rejected). User annotations built — `annotation @name { … }`, attachment checking, `has<A>()`/`get<A>().field` at comptime on both backends, across packages ([annotations.md](specs/types/annotations.md), spec still proposed). Call information is spec only ([call-info.md](specs/control/call-info.md)); gap analysis in [macro-story.md](specs/analysis/macro-story.md) |
| Frontend caching | LSP works, incremental check caching not yet implemented |
| Parallel compilation | Semantic hashing done, rayon parallelism not yet implemented |
| Phase B fiber implementation | Decided: stackful fibers with mmap'd virtual stacks, pluggable reactor (io_uring/epoll/kqueue/IOCP), signal-based preemption. Still to prototype: `fiber_switch` assembly, safe-point instrumentation, reactor backends |

See [TODO.md](TODO.md) for full list.

---

## Validation

Test programs that must work naturally:
1. HTTP JSON API server
2. grep clone
3. Text editor with undo
4. Game loop with entities
5. Embedded sensor processor

**Litmus test:** If Rask is longer/noisier than Go for core loops, fix the design.

---
