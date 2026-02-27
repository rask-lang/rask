# Memory Model Validation

Audit of the 5 validation programs plus 3 targeted string stress tests. This document records what works, what breaks, and what needs fixing.

## TL;DR

The memory model is **coherent and mostly works**. The foundation (ownership, block/statement-scoped borrowing, pools+handles) is sound. Three categories of issues emerged:

1. **Spec inconsistencies** — for-loop desugaring contradicts borrowing rules; examples violate their own spec
2. **Unnecessary ceremony** — many `.clone()` calls in examples were avoidable; authors didn't trust the borrow model
3. **String tax is real but manageable** — 4-6 allocations per parsed line; acceptable for most code, painful for hot-path parsers

No fundamental redesign needed. Targeted fixes below.

---

## Clone Census

### Existing Validation Programs

| Program | Total `.clone()` | Required | Avoidable | Notes |
|---------|-----------------|----------|-----------|-------|
| grep_clone.rk | 7 | 3 | 4 | Hot-path clones (line 106) were avoidable — **fixed** |
| http_api_server.rk | 7 | 7 | 0 | All clones cross closure/thread boundaries |
| text_editor.rk | 7+ | 7+ | 0 | Undo stack recording requires owned copies |
| game_loop.rk | 0 | 0 | 0 | All-numeric, no strings |
| sensor_processor.rk | 0 | 0 | 0 | All-numeric, no strings |

### String Stress Tests

| Test | Allocs/unit | Comparable Rust | Comparable Go | Verdict |
|------|-------------|-----------------|---------------|---------|
| HTTP header parser (Variant A) | ~8 per request | 0 | 0 | Acceptable for most servers |
| Tokenizer (Variant A) | 1 per payload token | 0 | 0 | Fine for small inputs, use StringPool for compilers |
| Log analyzer | 6 per log line | 0-2 | 0-1 | Map.get needing owned key is the real problem |

### Clone Categories

**Required by design (no fix possible without storable references):**
- Returning owned data from functions (`get_line() -> string`)
- Undo/history stacks (need independent copies)
- Crossing closure boundaries (`Shared.read`, `Shared.write`)
- Crossing thread boundaries (`spawn(|| { ... })`)
- Pushing non-Copy values into Vec

**Avoidable (authors were overly conservative):**
- Passing borrowed values to functions that take borrows (`line_matches(line.clone(), ...)`)
- Iterating a Vec that doesn't need to be consumed (`opts.files.clone()` → just `opts.files`)
- Passing struct fields as borrows (`opts.clone()` → just `opts`)

**Spec gap (Map.get should accept borrows):**
- `stats.by_level.get(level_key.clone())` — Map.get takes an owned key, but only needs a borrow for lookup. This forces an allocation on every Map lookup with string keys. Rust's `HashMap::get(&str)` avoids this. **This is the single highest-impact fix for string-heavy code.**

---

## Spec Violations Found in Examples

### 1. game_loop.rk:133-134 — Storing statement-scoped Pool view

**Before (violation of `mem.borrowing/V2`):**
```rask
const e1 = state.entities[h1]   // Entity is ~38 bytes, !Copy
const e2 = state.entities[h2]   // Statement-scoped view stored in const
```

**After (copy out small sub-structs):**
```rask
const p1 = state.entities[h1].position   // Position is 8 bytes, Copy
const p2 = state.entities[h2].position
const c1 = state.entities[h1].collider   // Collider is 4 bytes, Copy
const c2 = state.entities[h2].collider
```

**Verdict:** Example was wrong, not the spec. Fixed.

### 2. sensor_processor.rk:258 — Data race with bare Vec

**Before (violates value semantics, data race):**
```rask
const readings = Vec.new()
// ...
let isr_handle = Thread.spawn(|| {
    interrupt_handler(readings, ...)  // Vec moved or... shared?
})
// Main thread also reads `readings` — data race
```

**After (channel-based, ownership transfer):**
```rask
const (tx, rx) = Channel<SensorReading>.bounded(1024)
let isr_handle = Thread.spawn(|| {
    interrupt_handler(tx, ...)       // tx moved into closure
})
// Main thread: rx.try_recv() — no shared state
```

**Verdict:** Example was fundamentally wrong. Vec has value semantics — sharing it between threads without `Shared<>` is a data race. The comment claiming "Vec handles are heap-allocated — both threads see the same data" contradicts the core design principle of value semantics. Fixed with channels.

### 3. grep_clone.rk:106 — Unnecessary clones on hot path

**Before:**
```rask
const matches = line_matches(line.clone(), opts.pattern.clone(), opts.ignore_case)
```

**After:**
```rask
const matches = line_matches(line, opts.pattern, opts.ignore_case)
```

**Verdict:** Both parameters are borrows by default (B3). Clones were unnecessary. For a grep tool processing 10K-line files, this eliminates 20K string allocations per file. Fixed.

### 4. game_loop.rk:213-237 — Closure capturing mutable borrow across threads

**Before:**
```rask
func parallel_movement_system(mutate entities: ...) using ThreadPool {
    // ...
    let task = ThreadPool.spawn(|| {
        // Closure captures `entities` (mutable borrow) — can't escape scope (S3)
        entities[h].position.x += ...
    })
}
```

**After (copy-out, compute, write-back):**
```rask
// 1. Copy out positions and velocities (small Copy types)
// 2. Compute new positions in parallel (owned data, no borrows)
// 3. Write back results (single-threaded)
```

**Verdict:** Mutable borrows can't escape their scope (S3). Closures sent to other threads must capture by value. Fixed with batch-read/compute/write pattern.

---

## Spec Inconsistencies

### For-loop desugaring contradicts borrowing rules

**The problem:** `ctrl.loops` desugars value iteration as:
```rask
const item = vec[_pos]   // "Expression-scoped borrow"
body                     // But body uses `item` across multiple statements
```

But `const item = vec[_pos]` stores a statement-scoped view in a `const`, which violates `mem.borrowing/V2`. And `std.iteration/A4` says `collection[i]` where T: !Copy is a compile error.

**The intent is clear:** for-loop bindings have special "loop-body-scoped" semantics. `item` is valid for the entire loop body, not just the statement.

**Recommendation:** Add a rule to `ctrl.loops` or `mem.borrowing`:

> **LP11: Loop binding scope.** In value iteration (`for item in collection`), the binding `item` is a borrow that extends to the end of the loop body. This is a special scope distinct from both statement-scoped and block-scoped borrowing. The collection is read-locked for the loop duration (no structural mutation per LP8).

### `std.iteration/A4` conflicts with value iteration

**A4** says `collection[i]` where T: !Copy is a compile error. But value iteration yields borrowed non-Copy elements. These rules apply to different contexts (index access vs iteration binding) but the spec doesn't make this distinction clear.

**Recommendation:** Clarify that A4 applies to standalone index expressions (`vec[i]` in index mode), not to the implicit binding in value iteration.

### `let x = y` moves even for Copy types

The compiler treats `let level_end = level_start` as a move for integers. Per `mem.value-semantics/VS6`, types ≤16 bytes with all-Copy fields copy implicitly. Integers should be Copy.

**Recommendation:** Compiler bug. `let` assignment from a Copy type should copy, not move.

---

## Pattern Catalog

### Patterns That Work Well

| Pattern | Example | Ergonomics |
|---------|---------|------------|
| Pool+Handle for entities | game_loop.rk | Natural. Zero clones for numeric data. |
| Inline statement-scoped access | `pool[h].health -= 10` | Clean one-liner. |
| Copy-out small sub-structs | `const pos = entity.position` | Works for any Copy field. |
| `with pool[h] as entity { }` | text_editor.rk | Good sugar for multi-statement mutation. |
| Value iteration (read-only) | `for item in vec { ... }` | Natural, matches Go/Python. |
| Field projections | `GameState.{entities}` | Enables disjoint borrows. |
| Channel-based thread comm | sensor_processor.rk (fixed) | Ownership transfer, zero shared state. |
| `ensure` for cleanup | text_editor.rk, http_api_server.rk | Visible RAII. |

### Patterns That Need Workarounds

| Pattern | Workaround | Pain Level | Notes |
|---------|------------|------------|-------|
| Return substring from function | `.to_string()` (allocate) or `string_view` (fragile) | Medium | Fundamental cost of no storable refs |
| Store parsed substrings | Variant A (clone) or Variant C (StringPool) | Medium | StringPool adds ceremony but works |
| Map lookup with string key | Clone key for lookup | High | **Fix: Map.get should accept borrows** |
| Multi-step string processing | Chain of `.to_string()` calls | Medium | ~4-8 allocs per parse |
| Parallel mutation of Pool | Copy-out / compute / write-back | Medium | More code but correct |
| Mutable closure escaping scope | Restructure to use channels or Shared | Low | This is the right constraint |

### Missing Patterns (Not Tested)

| Pattern | Status |
|---------|--------|
| Regex-based string matching | Not specified in stdlib |
| Streaming/incremental parsing | Would need StringPool or cursor-based API |
| Observer/event pattern with callbacks | Partially tested (http_api_server) |
| Long-lived cache returning references | Would need Pool-based approach |
| Work-stealing parallelism | ThreadPool exists but no steal-from-neighbor |

---

## String Story Assessment

### The Three Tiers

| Tier | Type | When to Use | Allocations | Safety |
|------|------|-------------|-------------|--------|
| **Simple** | `string` + `.to_string()` | Most code, API boundaries | 1 per extraction | Compile-time (ownership) |
| **Indexed** | `string_view` | Internal optimizations | 0 | **None** (user responsibility) |
| **Validated** | `StringPool` + `StringSlice` | Parsers, compilers, tokenizers | 1 (pool insert) | Runtime (generation checks) |

### Comparison

| Operation | Rask | Rust | Go |
|-----------|------|------|-----|
| Parse header `"Key: Value"` | 2 allocs (key + value) | 0 (two `&str`) | 0 (string slice) |
| Tokenize identifier | 1 alloc (`.to_string()`) | 0 (`&str` slice) | 0 (string slice) |
| Map lookup by string key | 1 alloc (clone for lookup) | 0 (`&str` borrow) | 0 |
| Store substring in struct | 1 alloc or StringPool | 0 (`&'a str`) | 0 |

### Verdict

The string tax is **real but not fatal**. For most application code (web servers, CLI tools), the clone overhead is negligible — a few microseconds per request. For hot-path parsers (compiler lexer, protocol decoder), StringPool is the answer but adds ~15% more code.

The biggest single improvement would be **Map.get accepting borrows** — this eliminates one clone per map lookup, which compounds across log analyzers, routers, configuration parsers, and any code that maps string keys.

### Potential Spec Improvements

1. **Map.get/contains_key should accept borrowed keys** — Lookup doesn't need ownership. Only insert needs an owned key. This is purely a collections API change, not a language change. Impact: eliminates 1-2 clones per Map operation in string-heavy code.

2. **`split_once` method on string** — Returns two expression-scoped slices for `"key: value"` parsing. Currently requires `find` + manual index arithmetic. Zero allocations, purely ergonomic.

3. **Clarify `string_view` safety expectations** — Currently described as "user ensures source string validity." Should explicitly warn this is equivalent to unchecked index access — no compile-time protection. Consider whether `string_view` should be a debug-mode-checked type (like pointer validity checks in debug builds).

---

## Recommendations

### Must Fix (Spec Inconsistencies)

1. Add `ctrl.loops/LP11` rule for loop binding scope
2. Clarify `std.iteration/A4` vs value iteration
3. Fix compiler: `let x = y` should copy for Copy types

### Should Fix (High Impact, Low Risk)

4. `Map.get()` accepts borrowed keys — biggest single improvement for string-heavy code
5. Add `string.split_once()` — ergonomic improvement for key-value parsing

### Consider (Medium Impact)

6. Document the copy-out pattern explicitly in `mem.borrowing` appendix — "when you need multiple fields from a Pool element, copy out small sub-structs"
7. Add a note to `mem.pools` about the parallel mutation pattern — "copy-out, compute, write-back"
8. `string_view` safety documentation — explicit warning about fragility

### Don't Change

- The core borrowing model (block-scoped vs statement-scoped) — it works
- The pool+handle pattern — it's proven
- The "no storable references" principle — every alternative is worse for Rask's goals
- The clone-based approach to returning data — it's the right tradeoff

---

## Appendix: Full Clone Inventory

### grep_clone.rk (After Fixes)

| Line | Expression | Category | Notes |
|------|-----------|----------|-------|
| 57 | `arg.clone()` | Required | Push borrowed iterator element into Vec |
| 62 | `positional[0].clone()` | Required | Statement-scoped Vec access → struct field |
| 68 | `positional[i].clone()` | Required | Statement-scoped Vec access → Vec push |

**Removed:** Lines 97, 106 (×2), 144, 145 — all were avoidable borrows.

### http_api_server.rk

| Line | Expression | Category | Notes |
|------|-----------|----------|-------|
| 98 | `u.name.clone()` | Required | Cross closure boundary (Shared.read) |
| 99 | `u.email.clone()` | Required | Same |
| 108 | `u.name.clone()` | Required | Same (get_user) |
| 109 | `u.email.clone()` | Required | Same |
| 130 | `user.clone()` | Required | Need two copies (Map + response) |
| 174 | `db.clone()` | Required | Shared handle for spawned task |
| 175 | `log_tx.clone()` | Required | Channel sender for spawned task |

### text_editor.rk

| Line | Expression | Category | Notes |
|------|-----------|----------|-------|
| 71 | `self.lines[h].text.clone()` | Required | Return owned string from Pool element |
| 76 | `text.clone()` | Required | Used in both Line creation and undo stack |
| 105 | `self.lines[h].text.clone()` | Required | Record old text for undo |
| 106 | `new_text.clone()` | Required | Used in both assignment and undo |
| 135 | `cmd.clone()` | Required | Used in both apply and redo stack |
| 196-197 | `part.to_owned()` | Required | Expression-scoped iterator → owned string |
| 218 | `parts[1].clone()` | Required | Statement-scoped Vec access → enum variant |

### game_loop.rk

No clones. All types are numeric/Copy.

### sensor_processor.rk

No clones. All types are numeric/Copy.
