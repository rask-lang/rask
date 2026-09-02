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
| 0 — Infer mutable captures (`mem.closures/MC1`, #1038) | ✓ done | `60f338b` interp, `6fe0847` native |
| 1 — Parser `\|mutate x: T\|` | ✓ done | `9f2f831` |
| 2 — Stdlib **nominal** `Sequence<T>` / `SequenceMut<T>` | ✓ done | `1ebca5d` |
| 3 — MIR for-loop lowering for Sequence | pending | — |
| 4 — Interpreter for-loop over a Sequence | pending | — |
| 5 — Adapters + terminals as `extend Sequence<T>` | pending | — |
| 6 — Migrate collection iteration; delete eager Vec adapters (`SEQ41`) | pending | — |
| 7 — `Range<T>` as one nominal type with `iter()` (#920) | pending | — |
| 8 — Channel `stream()` method | pending | — |
| 9 — Retire `Iterator<Item>` trait | pending | — |
| 10 — Test suite migration | pending | — |
| 11 — Closure devirtualization pass (`SEQ17`–`SEQ19`) | pending | — |
| 12 — Zero-cost fusion test | pending | — |

**Stage 0 was the gate, and it shrank.** Every `for` body that writes an enclosing local — the common case — desugars into a closure that captures it. Two drafts tried to make that spellable: first an exemption from `MC1` for generated closures, then an emitted capture list. Both are gone. `MC1` infers a closure's captures from its body, the way read captures already worked, so the desugar emits a plain `|v|` and does nothing special.

What was left was a bug rather than a design task: writing an inferred mutable capture compiled and silently dropped the write (#1038, both backends). Under the old rule the fix was "reject it"; under inference the fix was "make it work". Both backends now do:

- The interpreter binds a name to a *slot* (`Arc<Mutex<Value>>`) shared by everything bound to it, so a scope-limited closure's capture reaches the definer's storage.
- Native holds each capture's *address* in the closure environment. Scalars needed a local category Cranelift doesn't have — an SSA value has no address — so `transform::addr_taken` rewrites an address-taken scalar's reads into loads and its writes into stores before SSA runs.

`tests/suite/p20_closure_mutable_capture.rk` gates it, 11/11 on both.

The same lie — `Ref` on a scalar spilling a copy — was also #899, so `mutate` on a primitive parameter reaches the caller now.

### Prerequisites — all cleared

| What | Outcome |
|---|---|
| `not` is not a lexer keyword (#1040) | Not a bug. `!` stays; the eight spec examples were wrong and are fixed |
| A write to an implicitly-captured local is silently dropped (#1038) | Fixed on both backends, stage 0 above |
| `\|mutate x\|` is rejected by the parser (#1039) | Not a bug. Captures are inferred, so there is no capture syntax to parse — everything in the pipes is a parameter (`CP3`) |
| A stale `rask` binary fails to link (#1041) | Not a compiler bug. The runtime source list is a compile-time constant, so a `.c` file added since the binary was built isn't linked. `cargo build --release -p rask-cli` |

One thing stages 3–7 have to work around rather than rely on: a closure that *escapes* its frame still copies its captures, and a returned one is stack-allocated in a frame that has already been popped (#1045). Adapters must not depend on writing through a capture of a returned closure.

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
- **Follow-up**: verify the alias resolves at use sites. `let s: Sequence<i32> = |x| { x > 0 }` should type-check. If resolution fails, inspect `rask-resolve/src/resolver.rs` and `rask-types/src/checker/declarations.rs`.

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
- Terminals: `to_vec`, `to_map`, `join`, `fold`, `reduce`, `sum`, `product`, `count`, `min`, `max`, `min_by`, `max_by`, `min_by_key`, `max_by_key`, `any`, `all`, `find`, `for_each` (`type.sequence/SEQ28-SEQ33` — there is no `collect`)
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
                  let r = self.receive()
                  if r? as msg { if not yield(msg): break } else { break }
              }
          }
      }
  }
  ```
- `take self` is required: the returned closure calls `receive()` after `stream()` has returned, so the Receiver must be owned by the closure. A borrowing `self` produces an expression-scoped Sequence (`mem.closures/SL2`) — not storable.
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
  - `seq.filter(p).map(f).take(n).to_vec()`
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
4. `tree.in_order().filter(|n| n.value > 10).take(5).to_vec()` works
5. `for mutate node in tree.in_order_mut() { node.value += 1 }` works
6. `for msg in rx.stream() { handle(msg) }` works with a spawned sender
7. `rask test-project tests/suite` passes

**Performance sanity check:**
- `vec.iter().sum()` on 1M i32s matches hand-written `for i in 0..vec.len() { sum += vec[i] }` within ±5%
- `cargo test -p rask-mir --test sequence_fusion` confirms fusion MIR shape

## Resolved questions

1. **Extending a type alias** — you can't, and that's why `Sequence<T>` is nominal now (`SEQ1`). `extend` attaches methods to a name; an alias has dissolved into `func(func(T) -> bool)` before method resolution runs, so `seq.filter(p)` would have nothing to find. This was filed as "verify in Stage 5" and was actually a design hole under the whole adapter surface.
2. **Non-local return** — the yield closure writes the return value to a slot in the enclosing frame, sets a flag beside it, and returns `false`; the frame tests the flag after the call (`SEQ8`). On the interpreter, `RuntimeError::Return` already unwinds — it needs a variant the intermediate adapter frames pass through and only the originating frame catches, or the innermost adapter swallows it. Note this makes `SEQ13a` load-bearing for correctness, not just for `.take(n)`: an adapter that drops the `false` drops the `return` with it.
3. **Generics through a nominal wrapper** — `Sequence<T>` is now an ordinary generic type, so it monomorphizes like `Vec<T>`. The alias-expansion question doesn't arise.
4. **Iterator runtime tear-down** — remove outright. `Iterator<Item>`, `Value::Iterator` and `IteratorState` all go in Stage 9; nothing keeps a compatibility path, and the eager `Vec` adapters go with them (`SEQ41`).

## Design decisions this plan now assumes

Recorded here because they changed after the first draft and the stages depend on them:

- **`Sequence<T>` is nominal**, not a `type alias` (`SEQ1`). A closure literal fills a Sequence-typed slot without a constructor (`SEQ36`).
- **Captures are inferred** (`mem.closures/MC1`). A closure that writes an enclosing local borrows it mutably with no annotation, as read captures already did; `own` stays explicit because it moves or clones. This deletes the whole capture-spelling problem and the desugar's special case with it.
- **Yields lend, except to a terminal** (`SEQ34`). A terminal may move a value the chain owns, because nothing observes it afterwards; ownership is read off the chain's shape (`map` makes values, `filter`/`take`/`skip` pass on what they were lent), so `Sequence<T>` stays one type. `take_all()` returns the drained `Vec<T>`, not a Sequence (`SEQ35`).
- **`to_vec` never deep-clones for you** (`SEQ47`). It copies `Copy` elements and moves chain-owned ones; a chain that only lends non-`Copy` items is a compile error naming the `map(|u| u.clone())` you meant. An earlier draft had it clone silently and justified that with the `to_` prefix — wrong, because `to_vec()` on `Sequence<i32>` and on `Sequence<User>` would then be the same spelling for a memcpy and for N allocations.
- **Lazy but not resumable** (`SEQ37`, `SEQ38`). No `next()`, no peek, no zip — all one restriction, and indices are the answer (`SEQ39`).
- **Collections carry no eager adapters** (`SEQ41`). `Vec.map`/`filter`/`take`/`skip`/`fold`/`sum`/… are deleted; the chain is the spelling.
- **One `Range<T>`** with step and inclusivity as fields (`ctrl.ranges/R6`), not seven types.
- **Fusion is a target, not a fact.** `SEQ17`–`SEQ19` need Stage 11's devirtualization pass; the general inliner only inlines direct calls, and an adapter chain is indirect calls all the way down.

## Out of scope

- Compiler-wide performance tuning beyond fusion correctness
- Async iteration / `for await` — explicitly rejected in the spec
- `zip` adapter — explicitly rejected; use indices or explicit buffer
- New error-reporting rules beyond the ones already in `type.sequence`
