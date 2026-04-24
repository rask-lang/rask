Keep docs short. In chat, explain things to me—I'm not a language architect expert.

Be critical, test all assumptions, scrutinize the design choices.

Prefer long term proper fixes over quick-fixes. 

Choose simple over easy

## Settled design decisions — don't re-litigate

- **Clone cost is intentional.** Types >16 bytes require explicit `.clone()` even when all fields are Copy. This is the transparency principle — the cost is visible. Don't suggest raising the Copy threshold, making clones implicit, or treating this as a problem to solve. It's a deliberate tradeoff.

- **Zero-copy work uses Arena scopes, not storable references.** Rask's model for zero-copy byte/binary work: allocate into an Arena via `using Arena.scoped(size) { ... }`, work with the data inside the scope (zero-copy, no generation checks, no unsafe), copy results out at the boundary. This is the same tradeoff as Rust's lifetime-bounded returns, expressed as "work inside scope, copy out at end." Don't suggest adding storable references, ByteBuffer types, or borrowed return values. The pieces already exist: Arena + `using` scopes + offset-based indexing. See `mem.alloc` spec.

# Working relationship

- No sycophancy.
- Be direct, matter-of-fact, and concise.
- Be critical; challenge my reasoning.
- Don’t include timeline estimates in plans.
- Don’t add yourself as a co-author to git commits.

## Debugging discipline
                                                                   
- Understand before changing. If you can't explain why something is broken, you're not ready to fix it.
- Fix causes, not symptoms.
- When you find a bug, check existing issues on `rask-lang/rask` first, then file a new one with a repro if it's not already tracked. Don't let bugs live only in chat.  

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
| `rask run <file>` | Execute a .rk program |
| `rask run --interp <file>` | Execute via interpreter (no codegen) |
| `rask compile --dump-mir <file>` | Print MIR before codegen (debug codegen issues) |

Binary: `compiler/target/release/rask` (build: `cd compiler && cargo build --release -p rask-cli`)
Releases: https://github.com/rask-lang/rask/releases

**Debugging codegen:** If a compiled binary segfaults, use `--dump-mir` to inspect the MIR and `RASK_RUNTIME_CHECKS=1 ./binary` to turn null-deref segfaults into panics with messages. Compile the C runtime with `-DRASK_DEBUG` for unconditional checks.

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

Key differences from Rust: `const`/`mut` (not `let`/`let mut`), `func` (not `fn`), `extend` (not `impl`), `public` (not `pub`), `string` (lowercase), `void` (not `()`), `Token.Plus` (not `::`), `try expr` (not `?`), `T or E` (not `Result<T,E>`), explicit `return` in functions, newlines as terminators.


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

## Goal

Systems language where **safety is invisible**. Eliminate abstraction tax, cover 80%+ of real use cases.

**Non-negotiable:** Feel simple, not safe. Safety is a property, not an experience.

## Core Principles

Unifying thread: **safety through visibility.** Safety mechanisms are visible in source (explicit `ensure`, `mutate`, `take`, `own`, scoped `with`) rather than hidden in destructors, lifetime annotations, or effect types. The compiler guarantees invariants; the source shows the mechanism.

1. **Transparency of Cost** — Major costs visible in code (allocations, locks, I/O). Small costs (bounds checks) can be implicit.
2. **Mechanical Safety** — Safety by structure. Use-after-free, data races, null derefs impossible by construction.
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
| Boxes | Container family with `with`-scoped access — Cell, Pool, Shared, Mutex, Owned | [boxes.md](specs/memory/boxes.md) |
| Collections | Vec, Map, Pool+Handle for graphs | [collections.md](specs/stdlib/collections.md), [pools.md](specs/memory/pools.md) |
| Resource types | `@resource` annotation for I/O handles, transactions; `ensure` cleanup | [resource-types.md](specs/memory/resource-types.md) |
| Types | Primitives, structs, enums, generics, traits, unions, tuples, nominal types, type aliases | [types/](specs/types/) |
| Errors | `T or E` result, `try` propagation, `T?` optionals, `todo()`/`unreachable()` | [error-types.md](specs/types/error-types.md) |
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
| Macros/attributes | Not specified |
| Frontend caching | LSP works, incremental check caching not yet implemented |
| Parallel compilation | Semantic hashing done, rayon parallelism not yet implemented |

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
