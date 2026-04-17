<!-- id: compiler.sequence-implementation -->
<!-- status: in-progress -->
<!-- summary: Staged implementation plan for the Sequence<T> push-iteration protocol -->
<!-- depends: types/sequence-protocol.md, memory/closures.md, control/loops.md -->

# Sequence Protocol Implementation Plan

## Context

The spec [types/sequence-protocol.md](../types/sequence-protocol.md) retires `Iterator<Item>` in favor of `Sequence<T>` — a function-type alias: `func(yield: |T| -> bool)`. For-loops over custom types desugar to yield-closure calls. No stored references, no state machines, no trait. Zero-cost enforced by closure inlining.

Most infrastructure is already present: `Type::Fn` exists, closures lower fine, `ClosureCall` MIR stmt exists, generic substitution handles function types. The work is (1) one parser extension, (2) new for-loop lowering branch, (3) stdlib migration from trait methods to closure-returning functions, (4) retire the hardcoded Iterator trait, (5) update tests.

## Scope

**Unchanged**: built-in `for x in vec` (inline-alias desugar, `ctrl.loops/LP17`); `Type::Fn`; `ClosureCall` MIR stmt; closure lowering; generic substitution; existing iterator-chain fusion for built-in collections (`try_parse_iter_chain`).

**Changed**: closure parser gains `|mutate x: T|`; new for-loop path for Sequence values; stdlib collection methods return `Sequence<T>` instead of `Iterator<T>`; adapters become extension methods on `Sequence<T>`; `Iterator<Item>` trait removed; tests updated.

## Status

| Stage | Status | Commit |
|-------|--------|--------|
| 1 — Parser `\|mutate x: T\|` | ✓ done | `9f2f831` |
| 2 — Stdlib `Sequence<T>` / `SequenceMut<T>` aliases | ✓ done | `4d1eaa0` |
| 3 — MIR for-loop lowering for Sequence | pending | — |
| 4 — Interpreter for-loop over callable | pending | — |
| 5 — Stdlib adapters as extension methods | pending | — |
| 6 — Migrate collection iteration methods | pending | — |
| 7 — Channel `stream()` method | pending | — |
| 8 — Retire `Iterator<Item>` trait | pending | — |
| 9 — Test suite migration | pending | — |
| 10 — Zero-cost fusion test | pending | — |

## Stages

Each stage is independently shippable and testable.

### Stage 1 — Parser: `|mutate x: T|` closure param ✓

- **File**: `compiler/crates/rask-parser/src/parser.rs`, `parse_closure()` at line 3360
- Accept `mutate` keyword before the parameter name
- When `is_mutate` is true, require an explicit type annotation
- `ClosureParam.is_mutate` field already exists in `rask-ast/src/expr.rs:260`
- **Tests**: `|mutate x: T|` parses; `|mutate x|` without type errors; untyped `|x|` unchanged

### Stage 2 — Stdlib: declare `Sequence<T>` / `SequenceMut<T>` ✓ (partial)

- **File**: `stdlib/sequence.rk` — currently contains:
  ```rask
  public type alias Sequence<T> = func(|T| -> bool)
  ```
- **File**: `compiler/crates/rask-stdlib/src/stubs.rs` — forwards `DeclKind::TypeAlias` from stdlib stubs (previously filtered out)
- **Known gap**: `SequenceMut<T>` is commented out. The type-parser path for function types (`parse_fn_type`) doesn't accept named parameters (`yield: |T|`) or `mutate` in closure-type parameter position. Stage 3 or a separate small stage should extend `compiler/crates/rask-types/src/checker/parse_type.rs` (`parse_fn_type` around line 247) to accept both, then restore the `SequenceMut` and named-`yield` forms.
- **Follow-up**: verify the alias resolves at use sites. `const s: Sequence<i32> = |x| { x > 0 }` should type-check. If resolution fails, inspect `rask-resolve/src/resolver.rs` and `rask-types/src/checker/declarations.rs`.

### Stage 3 — MIR: for-loop lowering over `Sequence<T>`

- **File**: `compiler/crates/rask-mir/src/lower/stmt.rs`, `lower_for()` at line 914
- Add a new branch **before** the generic index fallback: if `iter_expr`'s type resolves to a function type matching the `Sequence<T>` or `SequenceMut<T>` shape, dispatch to new `lower_for_sequence()`
- Implement `lower_for_sequence()`:
  - Lower `iter_expr` to a callable value
  - Synthesize a yield closure with one parameter matching the for-binding; body is the loop body with:
    - `break` → `return false`
    - `continue` → `return true`
    - `return expr` → set a non-local-return flag in the enclosing frame, then `return false`
    - fallthrough → `return true`
  - Emit `ClosureCall` (already in `rask-mir/src/stmt.rs:59`) invoking the sequence with the yield closure
  - After the call: check the non-local-return flag, propagate if set
- Reuse: `rask-mir/src/lower/closures.rs` `lower_closure()` at line 18
- **Test**: `tests/suite/t26_custom_sequence.rk` — custom sequence, for-loop, break/continue/return

### Stage 4 — Interpreter: for-loop over callable values

- **File**: `compiler/crates/rask-interp/src/interp/exec_stmt.rs`, `StmtKind::For` handler at line ~175
- Add a branch: if the iter value is `Value::Closure` or `Value::Function`, treat as Sequence
- Build a yield closure that runs the body and returns `bool` using the translation above
- Invoke via `call_value` (already handles closures — `interp/dispatch.rs:11-93`)
- Use existing `ControlFlow` / break / continue / return propagation infrastructure
- **Test**: same `.rk` tests as Stage 3, run under interpreter

### Stage 5 — Stdlib adapters as extension methods on `Sequence<T>`

- **File**: extend `stdlib/sequence.rk` with adapter and terminal definitions
- Verify Rask allows `extend Sequence<T> { ... }` on a type alias. If not, either add support or make adapters free functions with method-call sugar
- Adapters: `filter`, `map`, `take`, `skip`, `take_while`, `skip_while`, `chain`, `enumerate`, `flatten`, `flat_map`
- Terminals: `collect`, `fold`, `reduce`, `sum`, `product`, `count`, `min`, `max`, `min_by`, `max_by`, `min_by_key`, `max_by_key`, `any`, `all`, `find`, `for_each`
- Each adapter is closure-returning — example:
  ```rask
  extend Sequence<T> {
      public func filter(self, pred: |T| -> bool) -> Sequence<T> {
          return |yield| {
              self(|item| {
                  if pred(item): return yield(item)
                  return true
              })
          }
      }
  }
  ```
- Per `type.sequence/SEQ13a`: adapters must return `false` from their own yield when the downstream yield returns `false`
- Interpreter side: pure-Rask adapters first. If performance requires it, add `rask-interp/src/builtins/sequence.rs`

### Stage 6 — Migrate collection iteration methods to `Sequence<T>`

- **`stdlib/collections.rk`** — `Vec::iter()`, `Vec::take_all()` return `Sequence<T>` (lines 80, 83)
- **`stdlib/memory.rk`** — `Pool::iter()`, `Pool::handles()`, `Pool::values()`, `Pool::take_all()` (lines 61, 64, 67)
- **`stdlib/string.rk`** — `chars()`, `bytes()`, `char_indices()`, `split()`, `split_whitespace()`, `lines()` (lines 102–117)
- **Runtime** (`compiler/crates/rask-interp/src/builtins/collections.rs`): rewrite `iter()`, `take_all()`, `handles()`, `keys()`, `values()` to return `Value::Closure` driving the underlying data
- **Type checker** (`compiler/crates/rask-types/src/checker/resolve.rs`): remove the Iterator-return references for `drain()`/`take_all()` (lines 1365, 1746–1748)
- The existing iterator-chain fusion (`rask-mir/src/lower/iterators.rs`) for `vec.iter().filter(...).map(...)` remains — it operates on the AST chain pattern, not on the runtime Iterator trait

### Stage 7 — Channel `stream()` method

- **File**: `stdlib/async.rk`
  ```rask
  extend Receiver<T> {
      public func stream(take self) -> Sequence<T> {
          return |yield| {
              loop {
                  const msg = match self.recv() {
                      Ok(m) => m,
                      Err(_) => break,
                  }
                  if not yield(msg): break
              }
          }
      }
  }
  ```
- `take self` is required: the returned closure calls `recv()` after `stream()` has returned, so the Receiver must be owned by the closure. A borrowing `self` produces an expression-scoped Sequence (`mem.closures/SL2`) — not storable.
- **Test 1**: `for msg in rx.stream().take(10) { ... }` — channel close terminates the sequence
- **Test 2**: build a channel, call `rx.stream()`, drop the Sequence without iterating. Verify the Receiver drops with it and senders see the channel-closed path.

### Stage 8 — Retire `Iterator<Item>` trait

- **File**: `compiler/crates/rask-types/src/traits.rs` — delete the `"Iterator" =>` arm at lines 330–336
- **File**: `rask-interp/src/builtins/iterators.rs` — the pull-based `IteratorState` and `iter_next()` may stay as internal-only or be removed. Remove if Stage 6 rewrote all builtins
- `Value::Iterator` variant in `rask-interp/src/value.rs:396` — remove if no runtime uses it
- Update remaining error messages mentioning `Iterator<T>` to reference `Sequence<T>`
- **Test**: `git grep -n "Iterator<"` returns zero hits in `compiler/` and `stdlib/`

### Stage 9 — Test suite migration

- **File**: `tests/suite/t16_iterators.rk` — update comment `Spec: type.iterator-protocol` → `type.sequence`; verify tests still pass
- **File**: `tests/suite/t25_iterator_adapters.rk` — same; confirm chains use the new API path
- **New**: `tests/suite/t26_custom_sequence.rk` — custom Sequence authoring, break/continue translation, non-local return, `SequenceMut` with `for mutate`, channel `.stream()`, dropped Sequence closes Receiver

### Stage 10 — Zero-cost fusion test (`type.sequence/SEQ19` contract)

- **New file**: `compiler/crates/rask-mir/tests/sequence_fusion.rs`
- Parse, type-check, and MIR-lower canonical chains:
  - `seq.filter(p).map(f).take(n).collect()`
  - Custom Sequence via explicit closure
- Assert MIR output equivalent to the hand-written loop (block count, no extra function calls per item beyond closure inlining)
- Reuse the existing compiler test harness in `compiler/crates/rask-mir/tests/`

## Critical file map

| Area | Path | Role |
|---|---|---|
| Closure parser | `compiler/crates/rask-parser/src/parser.rs:3360` | `parse_closure()` |
| Closure AST | `compiler/crates/rask-ast/src/expr.rs:260` | `ClosureParam` |
| For-loop lowering | `compiler/crates/rask-mir/src/lower/stmt.rs:914` | `lower_for()` |
| Closure lowering | `compiler/crates/rask-mir/src/lower/closures.rs:18` | `lower_closure()` |
| ClosureCall MIR | `compiler/crates/rask-mir/src/stmt.rs:59` | MIR opcode |
| Iterator-chain fusion | `compiler/crates/rask-mir/src/lower/iterators.rs` | built-in fusion |
| Iterator trait | `compiler/crates/rask-types/src/traits.rs:330-336` | delete in Stage 8 |
| Function type parsing | `compiler/crates/rask-types/src/checker/parse_type.rs:247` | `parse_fn_type()` |
| Generic substitution | `compiler/crates/rask-mono/src/instantiate.rs:70` | `substitute_type_string()` |
| Stdlib Vec methods | `stdlib/collections.rk:80,83` | return types |
| Stdlib Pool methods | `stdlib/memory.rk:61,64,67` | return types |
| Stdlib string methods | `stdlib/string.rk:102-117` | return types |
| Stdlib Channel | `stdlib/async.rk` | add `stream()` |
| Interp for-loop | `compiler/crates/rask-interp/src/interp/exec_stmt.rs:175` | `StmtKind::For` |
| Interp closure call | `compiler/crates/rask-interp/src/interp/dispatch.rs:11` | `call_value()` |
| Interp iterator builtins | `compiler/crates/rask-interp/src/builtins/iterators.rs` | migrate/retire |
| Interp collection builtins | `compiler/crates/rask-interp/src/builtins/collections.rs` | rewrite iter methods |
| Tests | `tests/suite/t16_iterators.rk`, `t25_iterator_adapters.rk` | update |

## Reuse opportunities

- `ClosureCall` MIR statement handles function-value invocation — no new MIR opcode
- `lower_closure()` synthesizes closure functions with environment — reuse for yield closures
- `parse_fn_type()` parses `func(...) -> ...` — verify closure-type params work
- `substitute_type_string()` substitutes generics — verify closure params substitute
- Extension model resolves `seq.filter(...)` to method-call sugar — reuse for adapter dispatch
- Existing iterator-chain fusion for built-in collections keeps working unchanged

## Verification

**Per-stage (CI):**
- Stage 1: `cargo test -p rask-parser`
- Stage 2–3: `cargo test -p rask-types -p rask-mir`
- Stage 4: `cargo test -p rask-interp`
- Stages 5–7: `compiler/target/release/rask run tests/suite/t26_custom_sequence.rk`
- Stage 8: `git grep -n "Iterator<"` returns zero hits
- Stage 9: `compiler/target/release/rask test-project tests/suite/`
- Stage 10: `cargo test -p rask-mir --test sequence_fusion`

**End-to-end:**
1. `cd compiler && cargo build --release -p rask-cli`
2. Author a custom tree with a `Sequence` method (in-order traversal)
3. `rask run tree_example.rk` prints nodes in order
4. `tree.in_order().filter(|n| n.value > 10).take(5).collect()` works
5. `for mutate node in tree.in_order_mut() { node.value += 1 }` works
6. `for msg in rx.stream() { handle(msg) }` works with a spawned sender
7. `rask test-project tests/suite` passes

**Performance sanity check:**
- `vec.iter().sum()` on 1M i32s matches hand-written `for i in 0..vec.len() { sum += vec[i] }` within ±5%
- `cargo test -p rask-mir --test sequence_fusion` confirms fusion MIR shape

## Open questions

1. **Extending a type alias**: can Rask `extend Sequence<T> { ... }` where `Sequence<T>` is a function-type alias? If not, adapters must be free functions. Verify in Stage 5.
2. **Non-local return mechanism**: MIR and interp need a frame-scoped flag to propagate `return expr` out of the yield closure. Check existing `ControlFlow`/`ReturnError` infrastructure before adding a one-off flag.
3. **Generic function-type aliases**: confirm `type alias Sequence<T> = func(...)` with generic `T` expands correctly through monomorphization.
4. **Iterator runtime tear-down**: in Stage 8, decide whether to remove `Value::Iterator` / `IteratorState` outright or keep as internal-only.

## Out of scope

- Compiler-wide performance tuning beyond fusion correctness
- Async iteration / `for await` — explicitly rejected in the spec
- `zip` adapter — explicitly rejected; use indices or explicit buffer
- New error-reporting rules beyond the ones already in `type.sequence`
