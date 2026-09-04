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
| 3 — MIR for-loop lowering for Sequence | ✓ done | `81c546d`, tuple binding after |
| 4 — Interpreter for-loop over a Sequence | ✓ done | `0209bbb` |
| 5 — Adapters + terminals as `extend Sequence<T>` | ✓ 14 adapters + `sum`/`product`/`join`/`min`/`max`/`to_map` | `t31`, `t37` |
| 6 — Migrate collection iteration; delete eager Vec adapters (`SEQ41`) | ✓ `.iter()` deleted (SEQ48); `filter`/`map`/`skip`/`take`/`enumerate`/`flat_map` on a collection are lazy. `zip`/`chunks` stay eager by SEQ39; `flatten` is the one exception left | `t38` |
| — #1045: a returned closure's environment | ✓ dangles no more; captured containers still leak | `3417ccc`, `09c6b4a`, `c947684` |
| — **#1047: a Vec passed by value to a function is never freed** | **the real next thing** | — |
| 7 — `Range<T>` as one nominal type yielding a `Sequence<T>` (#920) | pending | — |
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

### The protocol's foundation was unsound on native (#1045) — fixed

A function that *returned* a capturing closure put the environment in a stack
slot of its own frame, and the closure read it after that frame was gone. A
captured scalar came back as a silently wrong answer (`sum=6` where the
interpreter said 33, a `max` of 1194732450 read out of the popped frame); a
captured `Vec` segfaulted. Since every adapter and every source returns a
closure, that sat under the whole protocol.

Three things were wrong, and none of them turned out to need a decision.

*Allocation.* Lowering picked heap-vs-stack from `own`, and the escape pass only
ever downgraded, so a scope-limited closure that escaped anyway kept its stack
environment. It's heap exactly when it escapes now, in both directions. The pass
also read only the `ClosureCreate` destination, and lowering copies that on
before returning it, so the return didn't look like an escape; it follows plain
copies now.

*Writes.* An `own` closure loaded its captures out of the environment at the top
of every call and never wrote back, so `counter()` answered 1 forever. `own`
moves the variable *into* the environment, which makes the environment its home,
so the body works through the slot's address for its whole life.

*Ownership.* I'd written this up as needing a choice between refcounting,
leaking and an owned box. It doesn't: a `func` value is an owned value, so single
owner, and the frame still holding it when it ends frees it
(`mem.ownership/O1`). What was missing is that the drop pass only knew about
closures *built* in a frame, not ones taken back from a call — and
`let tick = counter()` is the caller receiving a block nobody else will free.
The block carries its size in a header word, because the frame that frees a
closure usually isn't the one that built it and has no idea what the capture
layout was.

`t26_custom_sequence.rk` used to pass by luck — everything in it is small enough
for the inliner to move the `ClosureCreate` into a live frame.
`t28_escaping_closure.rk` is deliberately too big for that, which is what makes
it a regression test.

Still open on #1045: freeing the block doesn't release what it captured, so an
`own` closure holding a `Vec` leaks the vector. The block would need drop glue
next to its size.

### The terminals, and which ones landed

Fourteen adapters are written, in Rask, in the protocol they define: each
adapter is a closure over `self` that re-yields, each terminal is a `for` loop
over `self`.

The six terminals that need to know something about the element — `sum`,
`product`, `min`, `max`, `join`, `to_map` — needed a bound, and Rask turned out
to have one. `extend Sequence<T> where T: Numeric` gives `sum` and `product` a
`+` and a `*`; `extend Sequence<string>` gives `join` its `push`. Three landed
that way.

The last two took longer, and neither was a bounds problem:

- **`min`/`max`** are in, written in Rask under `T: Comparable`. They used to
  collide with the free `min(a, b)`/`max(a, b)` in `builtins.rk` — a method and
  a free function share one name table, and whichever file loads later wins the
  bare name, so `min(5.0, 3.0)` started resolving to the method and reported
  "expected 1 argument, found 2". `Vec.min` never had the problem only because
  `collections.rk` already loaded first. `sequence.rk` loads ahead of builtins
  now and both spellings work.
- **`to_map`** is in, and what stopped it was the `extend` header, not the
  bound. `extend Sequence<(K, V)>` names two parameters under the one parameter
  `Sequence` declares, and three places rebuilt the receiver from the
  declaration's own names instead: the method's signature counted `K` and `V` as
  its *own* type parameters, `Self` came out as `Sequence<T>` so `self` had `T`
  for an element type, and monomorphization zipped the two names against the
  receiver's one argument. Carrying the header through all three fixed it (see
  **Where an extend method's type parameters come from** below).

### `.iter()` is gone (SEQ48)

A collection is its own chain head: `v.filter(p)`, `for x in v`. The call bought
nothing — `for x in v` and `for x in v.iter()` compiled to the identical fused
index loop — and the borrow-vs-move distinction it draws in Rust is spelled
`take_all()` and `for mutate x in v` here.

Deleting it took the compiler's `iter` special-cases with it: the chain-head
match in `try_parse_iter_chain`, the bare-`.iter()` unwrap in `lower_for`, the
`Vec.iter()`/`Sequence.iter()` arms in the checker, and the interpreter
builtins. A user method named `iter` that returns a sequence is now an ordinary
method the compiler has no opinion about — `t26_custom_sequence.rk` holds that
line.

**What it cost.** `Vec` reaches the Sequence terminals through fusion, so the
chain spellings all still work — except `to_map`, which the checker only knew
on a sequence. It's on `Vec<(K, V)>` now, same pair-or-error rule, on both
backends.

### The lazy switch (SEQ41)

`v.filter(p)` hands back a `Sequence<T>` now, not a second `Vec`. The type
changed; the AST didn't, and that distinction is the whole implementation.

**Fusion is untouched.** `try_lower_iter_terminal` matches the terminal first
and consumes the whole chain, so `v.filter(p).to_vec()` and
`for x in v.filter(p)` are the same index loop with no closure they always were.
Measured: zero heap closures in the enclosing frame either way. That mattered
enough to check, because routing the chain through `as_sequence()` in the AST
costs three heap closures per stage — `v.as_sequence().filter(p).to_vec()` is
26ms where the fused form is a few.

**What changed is the un-terminated case.** A bare adapter used to be lowered as
"the chain, with an implicit `.to_vec()`" — arms in `iterators.rs` for `map`,
`filter`, `enumerate` and `flat_map`. Those are gone. `let odd = v.filter(p)`
now builds the closure chain, and `odd` is re-runnable like any sequence.

Leaving one of those arms in place while the type said `Sequence` is worth
knowing about: the frame handed the caller a Vec pointer where its type said
closure, and `Sequence_count` jumped through it. A segfault, not a type error.

**Which adapters moved, and why the rest didn't:**

| Method | State | Why |
|--------|-------|-----|
| `filter`, `skip`, `take`, `enumerate` | lazy, forwards to `self.as_sequence().<same>(…)` | nothing to say |
| `map`, `flat_map` | lazy, body written out | forwarding to a callee that declares its own type parameter loses the argument and the call mangles bare — #1065 |
| `zip`, `chunks` | eager, on the collection | SEQ14/SEQ39: lockstep and position need two positions at once, and a push source can't hold one |
| `flatten` | eager | `Sequence.flatten` has no body either; a lazy type with nothing behind it would be a name wearing no implementation. Nothing in the corpus calls it |

**Indexing a sequence** is a type error now (`E0819`, SEQ39) rather than
"Function not found: Sequence_index" out of codegen. `.len()` is likewise a
"no method" error pointing at `count()`.

### Where an extend method's type parameters come from

A block header can name the receiver's parameters differently from the
declaration, and can name *more* of them:

```
struct Sequence<T>          // one parameter
extend Sequence<(K, V)>     // two names, under that one parameter
```

`K` and `V` are the receiver's — the halves of its element type. Nothing
recorded that, so three separate places each rebuilt the receiver from
`Sequence<T>` and lost the header:

| Place | What it did instead | Symptom |
|-------|--------------------|---------|
| `method_signature` | counted `K`, `V` as the method's own parameters | one fresh inference variable per call, nothing to bind it: `to_map`'s `Map<K, V>` came back "type is still open" |
| `resolve_impl_self_type` | built `Self` as `Sequence<T>` | `self`'s element type was `T`, which no call in such a block binds — so the copy dropped the record and `for (k, v) in self` fell through to the index loop and walked a closure as if it were a `Vec` |
| `instantiation_params` | zipped the header's names against the receiver's arguments one for one | `K` took the whole `(i64, i64)`, `V` bound to nothing: `Sequence_to_map$___` |

`MethodSig` carries the header's arguments as written now (one per parameter the
type declares), and a call binds them against the receiver member-wise, so a
parameter nested in a tuple gets the matching half. A header that just repeats
the declaration's names is the same thing it always was.

Two related routes collapsed into one on the way:

- **A call's type arguments are named.** `TypedProgram.call_type_args` holds
  `(parameter, type)` pairs rather than a positional list. Monomorphization used
  to receive the receiver's arguments and the method's own merged into one flat
  list and split it at a seam it had to *count* — the count that `(K, V)` breaks.
  Both producers had the names in hand already and threw them away.
- **A call's receiver is read from the dispatch record.** `call_targets` is the
  checker's answer to "what does this call dispatch on", and it was recorded
  before inference settled and never re-applied — so `wrap(3.14).get()` carried
  `Box<?2133>`. Applying the substitution on the way out (which `node_types` and
  `span_types` already got) makes it usable, and a static method's receiver is
  recorded instantiated — `Box.new("hei")` gives the type name a fresh variable
  per declared parameter and lets `-> Box<T>` bind it. That deleted the guess
  that used to read a constructor's instantiation back out of the call's result
  type.

**What it exposed.** `take(n)` was lowered as a loop bound — stop at source
index n — which is only "n elements out" while everything upstream survives.
`v.filter(p).take(3)` was stopping after three *candidates*:
`[0..9].filter(%3 == 0).take(3)` gave one element instead of three. The old
suite spelling hid it, because `.iter()` in the middle of the chain split it in
two and `take` only ever saw a dense Vec. A `skip`/`take` downstream of a filter
now carries a runtime counter instead of a bound (`t32_counted_skip_take.rk`).

### The ordering that has to hold

`for x in v` fuses into an index loop with no closure at all. `lower_for` tries
chain fusion *before* asking SEQ6, because a collection returning a `Sequence<T>`
would otherwise let the SEQ6 branch claim the language's most common loop and
put a yield closure call on every element. Fusion wins the tie; SEQ6 catches
what fusion declines, which is what `for x in bag` needs.

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

- **`stdlib/collections.rk`** — `Vec::take_all()` returns `Sequence<T>` (line 83)
- **`stdlib/memory.rk`** — `Pool::handles()`, `Pool::values()`, `Pool::take_all()` (lines 61, 64, 67)
- **`stdlib/string.rk`** — `chars()`, `bytes()`, `char_indices()`, `split()`, `split_whitespace()`, `lines()` (lines 102–117)
- **Runtime** (`compiler/crates/rask-interp/src/builtins/collections.rs`): rewrite `take_all()`, `handles()`, `keys()`, `values()` to return `Value::Closure` driving the underlying data
- **Type checker** (`compiler/crates/rask-types/src/checker/resolve.rs`): remove the Iterator-return references for `drain()`/`take_all()` (lines 1365, 1746–1748)
- The existing chain fusion (`rask-mir/src/lower/iterators.rs`) for `vec.filter(...).map(...)` remains — it operates on the AST chain pattern, not on the runtime Iterator trait

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
- `tests/suite/t26_custom_sequence.rk` exists and is green on both backends: authoring a Sequence, break/continue/non-local return, a tuple binding, a hand-written adapter that composes and forwards, driving one sequence twice, and nested loops. Still to add once their stages land: `SequenceMut` with `for mutate`, channel `.stream()`, and a dropped Sequence closing its Receiver

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
- `vec.sum()` on 1M i32s matches hand-written `for i in 0..vec.len() { sum += vec[i] }` within ±5%
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
