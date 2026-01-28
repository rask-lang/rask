# Validation Notes: Async Runtime Initialization

## Conflicts with CORE Design?
**NO** — Preserves core principles:
- **Transparent costs:** `async` keyword marks all async operations, IDE warns on sync I/O blocking
- **Local analysis:** Per-thread runtime, no global state requiring whole-program analysis
- **No annotations:** Runtime initialization implicit (ergonomics), cost visible via async keyword
- **Compilation speed:** Linker-based inclusion, no whole-program analysis

## Internal Consistency?
**YES** — Checked:
- Per-thread runtime eliminates cross-thread sharing issues
- block_on explicit at boundaries (cost visible)
- Async/sync separation clear (compile-time enforced)
- Shutdown automatic (matches ensure/drop patterns)

## Conflicts with Other Specs?
**NO** — Cross-checked:
- **Concurrency spec:** Async nursery follows same structured concurrency (task handles affine)
- **Linear types:** ensure works in async (elaboration confirms)
- **Channels:** Same channel types work sync/async (already specified)
- **Module system:** No impact (runtime is stdlib, auto-linked)

## Complete Enough to Implement?
**YES** — Provides:
- Initialization trigger (first async operation)
- Per-thread model
- Main function signature (`async fn main`)
- Async/sync interaction rules
- Shutdown semantics
- Configuration (or lack thereof)

## Concise?
**YES** — ~180 lines with tables and examples. Could be condensed further but covers essential decision points.

## Design Decisions Made

**Decision 1: Per-thread runtime (not global)**
- Rationale: Eliminates cross-thread state, preserves local analysis, simpler implementation
- Tradeoff: Cannot share tasks across threads (acceptable per spec)

**Decision 2: Implicit initialization (not explicit)**
- Rationale: Ergonomics, matches "cost visible when used" (async keyword is the marker)
- Tradeoff: Less control (acceptable for 80% case)

**Decision 3: No configuration API**
- Rationale: Simplicity, fast compilation, avoid bikeshedding
- Tradeoff: Power users must use native threads (acceptable escape hatch)

**Decision 4: Async-from-sync is compile error**
- Rationale: Prevents accidental blocking, preserves function color clarity
- Tradeoff: Requires block_on at boundaries (acceptable, explicit)

**Decision 5: Sync-in-async allowed but warned**
- Rationale: FFI and legacy I/O need escape hatch
- Tradeoff: Blocks runtime (programmer responsibility, IDE warns)

All decisions align with CORE_DESIGN.md principles.
