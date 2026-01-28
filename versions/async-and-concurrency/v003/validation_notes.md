# Validation Notes: Async Function Syntax

## Conflicts with CORE Design?
**NO** — Preserves core principles:
- **Transparent costs:** `async` keyword, `.await` operator, `block_on` all explicit
- **Local analysis:** Function color visible in signature, no inference across function boundaries
- **No annotations:** AsyncTask<T> is compiler-internal, users write simple return types
- **Ergonomics:** Postfix await allows chaining, minimal ceremony

## Internal Consistency?
**YES** — Checked:
- `async fn` prefix consistent with Rust/JS/Python
- `.await` postfix enables chaining (`.await?.method().await`)
- `block_on` explicit boundary marker (consistent with transparency)
- Async blocks/closures follow same pattern as sync
- `?` propagation works identically in async

## Conflicts with Other Specs?
**NO** — Cross-checked:
- **Error handling:** `?` works in async (elaboration confirms)
- **Linear types:** Can be async parameters, must consume before suspend (consistent with ownership)
- **Closures:** Async closures capture by move (matches existing closure spec)
- **Ensure cleanup:** Works with await (ensure fires on scope exit)

## Complete Enough to Implement?
**YES** — Provides:
- Syntax for async fn, blocks, closures
- `.await` operator semantics
- `block_on` boundary function
- Return type inference rules
- AsyncTask<T> internal representation
- Function color boundary rules
- Edge cases table

## Concise?
**YES** — ~150 lines with tables. Focused on syntax and semantics.

## Design Decisions Made

**Decision 1: Prefix `async fn` (not postfix)**
- Rationale: Industry standard (Rust, JS, Python), visibility at glance
- Tradeoff: None

**Decision 2: Postfix `.await` (not prefix)**
- Rationale: Enables chaining, reads naturally left-to-right
- Tradeoff: None (Rust proved this pattern)

**Decision 3: Implicit AsyncTask<T> (not explicit)**
- Rationale: Reduces ceremony, `async` keyword is sufficient marker
- Tradeoff: Less explicit about task-ness (acceptable, follows principle 7)

**Decision 4: No async-from-sync (compile error)**
- Rationale: Prevents accidental blocking, forces explicit block_on
- Tradeoff: Requires boundary function (acceptable, cost visible)

**Decision 5: Sync-from-async allowed**
- Rationale: FFI and legacy code need escape hatch
- Tradeoff: Blocks runtime (programmer responsibility, IDE warns)

All decisions align with CORE_DESIGN.md and existing concurrency spec.

## Syntax Choice Rationale

Compared alternatives:

| Syntax | Chosen? | Rationale |
|--------|---------|-----------|
| `async fn foo()` | YES | Industry standard, visible prefix |
| `fn foo() async` | NO | Non-standard, less visible |
| `.await` | YES | Postfix enables chaining |
| `await expr` | NO | Breaks chaining |
| Explicit `AsyncTask<T>` | NO | Ceremony, violates principle 7 |
| Implicit via `async` | YES | Minimal annotation |

Final syntax maximizes ergonomics while maintaining cost visibility.
