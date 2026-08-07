# Rust corpus census

Taken 2026-08-06, while settling the `try`/`orelse` operator family (#565/#573/#574). Three
corpora, grep-level occurrence counts (`grep -rhoE`), normalized per 10k lines. The question:
which error/optional-handling forms does real code actually use, and does Rask's operator family
cover the ones that matter?

| corpus | lines | provenance |
|---|---|---|
| rask compiler | 147,134 | `compiler/crates`, AI-written, inline tests included |
| tokio | 122,420 | src-only (tests/, examples/, benches/ excluded), expert humans |
| ripgrep | 56,386 | full tree incl. heavy inline tests, expert human (BurntSushi) |

## Error/optional handling, per 10k lines

| form | rask | tokio | ripgrep | Rask spelling |
|---|---|---|---|---|
| `?` propagation | 174 | 108 | 100 | `try` |
| `if/while let Some` | 116 | 27 | 32 | `if x? as v` |
| `.unwrap()`/`.expect()` | 82 | 75 | 210* | `x!` / `x! "msg"` |
| Some/None/Ok/Err match arms | 76 | 63 | 54 | `match` / `is` |
| `.unwrap_or` family (3 methods) | 56 | 6.5 | 7.4 | `orelse v` |
| `is_some/none/ok/err` | 27 | 22 | 61 | `x?`, `x is none`, `r is E` |
| `.as_ref/as_mut/as_deref` | 25 | 35 | 18 | — deleted by construction |
| `.cloned()/.copied()` | 23 | 0.7 | 2.5 | — deleted by construction |
| `.map_err` | 20 | 1.5 | 9 | `orelse e => return f(e)` |
| `.ok_or/_else` | 19 | 0.7 | 0.7 | `orelse return e` |
| `.and_then` | 18 | 1.8 | 2.5 | — cut; `match` serves |
| `Option::take()` | 2.6 | 11 | 1.8 | — hole; `take <place>` proposed |
| `let … else` (diverging) | 4.6 | 0.8 | 13 | `orelse return` |
| `.transpose()` | 0.1 | 1.0 | 0 | — meaningless on flat `T? or E` |

*ripgrep unwrap is test-block-inflated; tokio src-only lands at 75 ≈ rask's 82.

### Readings

1. **The combinator zoo is an AI/compiler dialect.** Experts sit at ≤2/10k on
   `ok_or`/`and_then`/`map_err` where the AI-written compiler sits at 18–20. The forms Rask cut
   are the forms experts already avoid; the cut is validated by expert behavior, not just
   argument. (Domain contributes — a typechecker converts absence to diagnostics constantly —
   but 27× on `ok_or` doesn't come from domain alone.)
2. **The head is invariant.** `try`-shaped propagation runs 100–174/10k across all three corpora;
   bail-or-panic ~75; test-and-bind 27–116. The four short forms (`try`, `!`, `if x? as v`,
   `match`) cover ~90%+ of all handling traffic in every corpus, regardless of author or domain.
3. **The eager/lazy method pairs exist because Rust method args are eager.** `ok_or` 37 vs
   `ok_or_else` 244 in the compiler; `unwrap_or` 642 vs `_else` 116. `orelse`'s right side is
   lazy by construction, so four method names collapse into one operator — removing the reason
   the names multiplied, not just renaming them.
4. **Borrow ceremony costs 18–35/10k everywhere.** `as_ref`/`as_mut`/`cloned` — highest in tokio
   (pin projections, `Option<&mut>`). Rask's inferred binding modes delete the category.
5. **`let-else` is where expert style is heading** — highest in the most modern idiomatic corpus
   (ripgrep, 13/10k). `x orelse return e` is that pattern as one operator.
6. **`Option::take()` is a real hole.** The move-out-and-leave-none idiom runs at 11/10k in tokio
   (wakers, futures, buffers in `mut` slots) — a mutation, so `match` can't serve it, and Rask
   has no spelling. Tracked separately (`take <place>` proposal).

## Wide census, per 10k lines

| metric | rask | tokio | ripgrep | bears on |
|---|---|---|---|---|
| `.clone()` | 211 | 27 | 38 | clone-visibility tradeoff |
| lifetime annotations (non-`'static`) | 21 | 151 | 90 | the tax Rask deletes |
| `'static` | 6 | 31 | 92 | ditto (rg: regex tables) |
| `&mut ` | 124 | 308 | 231 | `mutate` marking rate (PM4) |
| `let mut` / all `let` | 12.5% | 21% | 20.5% | `const` is the right default |
| explicit `return` | 172 | 98 | 119 | explicit-return rule cost |
| `match` | 162 | 58 | 142 | match stays central |
| closures (non-empty `\|x\|`) | 181 | 54 | 118 | closure design load |
| for loops | 136 | 25 | 45 | loops vs adapters |
| `.iter()/.into_iter()` | 119 | 8 | 28 | ditto |
| `.map(` (incl. Option) | 75 | 16 | 26 | adapter demand is modest |
| `.collect` | 58 | 3 | 8 | ditto |
| `.filter(` | 15 | 0.6 | 2 | near-absent in experts |
| `Box<` | 44 | 26 | 9 | deleted (values own heap) |
| `Arc<` | 35 | 36 | 14 | → `Shared` |
| `Mutex<` | 30 | 14 | 0.4 | → `Mutex` (domain-driven) |
| `RefCell` | 2.7 | 2.1 | 3.4 | → `Cell` — niche confirmed |
| `Rc<` | 0 | 2.4 | 0 | Arc won; no Rc analog needed |
| `where` clauses | 11 | 61 | 29 | generics ceremony |
| turbofish `::<` | 7.6 | 26 | 29 | inference gaps |
| `#[derive` | 23 | 34 | 64 | derive culture is universal |
| `unsafe` | 4.6 | 89 | 0.9 | tokio = runtime internals |
| assert family | 126 | 124 | 369* | invariant culture (*tests) |
| panic!/unreachable!/todo! | 15 | 15 | 7 | |
| `.await` / `async fn` / `Pin<` | — | 140 / 102 / 46 | — | coloring Rask deletes |

### Readings

1. **The clone↔lifetime trade, quantified.** Experts pay 90–150 lifetime annotations per 10k
   lines to keep clones at 27–38; the AI pays 211 clones/10k to avoid lifetimes (21). Rask's
   design *is* the second strategy with the lifetime option removed — expect AI-written Rask to
   clone at the high rate, which is what the visibility principle wants seen.
2. **`&mut` at 230–310/10k in expert code** predicts `mutate` call-site marking density around
   one mark per 35 lines. PM4's cost is real but bounded.
3. **Expert systems code barely uses iterator adapters** (tokio: 8 iter/10k, 0.6 filter) —
   plain loops dominate. Rask's loop-first design matches expert practice.
4. **The box family maps 1:1 onto observed usage.** `Arc` common → `Shared`; `RefCell` flat at
   2–3/10k in every corpus → `Cell` deliberately niche; `Rc` ≈ 0 → correctly omitted.
5. **tokio's async coloring** (`await` + `async fn` + `Pin` + `'static` ≈ 320 sites/10k) is the
   ceremony Rask's uncolored concurrency deletes — caveat: tokio is the runtime, not an app.
6. **~20% of expert bindings are `mut`** — `const`-by-default matches practice, not just taste.
7. **`where` at 29–61/10k in experts** — generic-bound ceremony is nontrivial; simpler generics
   have measurable headroom.

## Caveats

Grep-level, unparsed; `.map`/`.filter` conflate Option/Result/Iterator receivers; domain and
authorship are entangled (three data points); ripgrep's inline tests inflate unwrap/assert;
tokio counted src-only. Treat ratios above ~3× as signal, smaller differences as noise.

## Discard vs use at catch-shaped sites

Added 2026-08-07, while pricing the mandatory `catch` binder. Sites where a result's error is
handled in place (the population that becomes `catch <binder> =>` in Rask), split by whether the
handler binds the error or ignores it — measured directly off closure params (`|_|` vs `|e|`)
plus the always-discarding `.ok()`:

| corpus | error used | error discarded | discard share | catch sites /10k |
|---|---|---|---|---|
| rask compiler | 256 (`map_err(\|e\|)` 249, `unwrap_or_else(\|e\|)` 7) | 122 (`\|_\|` 49, `.ok()` 73) | 32% | 26 |
| tokio (src) | 7 | 32 (`\|_\|` 14, `.ok()` 18) | 82% | 3 |
| ripgrep | 26 | 23 (all `.ok()`) | 47% | 9 |

Readings:

1. **The split is domain-shaped, not universal.** A compiler binds the error (it becomes a
   diagnostic); best-effort I/O discards it; a CLI tool does both. Any design that optimizes one
   side flat-out loses somewhere.
2. **Catch-shaped sites are rare everywhere** — 3–26/10k against 100–174/10k for propagation. The
   mandatory binder's ceremony lands on the rarest form in the family; five characters (`_ => `)
   at ≤20 sites per 10k lines.
3. **Acknowledged discard is already expert idiom**: `let _ =` runs at 9/10k in tokio src with no
   ecosystem pushback, while the two ecosystems that shipped *silent* drops retrofitted the check
   (Go's errcheck; Zig's recurring make-swallowing-harder threads).
4. **One-armed narrows bound the residual hole**: `if let Ok(` at 0.5–4.6/10k is the ceiling on
   errors that die structurally (no else arm) rather than in an expression. Lint territory —
   `type.errors` rationale has the boundary statement.
