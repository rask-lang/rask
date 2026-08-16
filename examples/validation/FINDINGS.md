# Validation findings — HTTP JSON API server

Program: `examples/validation/` — an in-memory work-item tracker (16 files,
~1600 lines), written to the **spec**. This pass was redone against `main`
after ~112 compiler commits landed.

> **2026-08 update:** the dir now targets the settled `try`/`??`/`catch` family
> (#565/#573/#574) *and it builds and serves natively.* It also runs the
> spec-idiomatic forms the earlier passes had to work around — nominal id
> newtypes, struct field defaults, `@message` auto-delegation, and a nameable
> `Ordering` — because the compiler fixes landed (§B is now a fixed-list).
> Readability verdict on the `catch`/`??` migration: §F.

**Status on current `main`:**

| Stage | Result |
|-------|--------|
| `rask parse` / `rask build` type-check | **clean — 0 errors** |
| `rask build --release` → native binary | **builds** |
| Native server, every path (CRUD, PATCH, filter, batch, errors) | **works, stable** |
| `rask test examples/validation` | **fails to compile the test binary — [#697]** |

The program is a working HTTP JSON API server: full CRUD, PATCH that persists,
the batch transaction, the spawn/channel seed pipeline, metrics, filtering, and
the whole error surface run natively. The one gap is `rask test`: the test
binary drags in stdlib http-client functions the server never calls, and those
fail to lower uninstantiated (#697) — `rask build` DCEs them and is fine.

---

## How to verify

```
rask build examples/validation --release      # builds a native binary
./examples/validation/build/release/tracker &  # serves on :8080
curl -H 'Authorization: Bearer dev-secret' localhost:8080/tasks
```

---

## §A Parser — now essentially up to spec

Last pass, five spec constructs didn't parse. Now:

| Construct | Before | Now |
|-----------|--------|-----|
| `duck trait` | ✗ | **✓ parses + checks** |
| `scoped extend` | ✗ | **✓ parses + checks** |
| comma-list `extend T with A, B` | ✗ | **✓ parses + checks** |
| struct field defaults `f: T = expr` | ✗ | **✓ parses + runs** |
| field annotations `@rename/@no_serialize/@default` | ✗ | ✗ still unimplemented |

Nominal trait conformance is **enforced** (G1) — a good change; the program
relies on it. Field defaults now land: `Config {}` is the fully-defaulted value
(FD3), and `Task`/`TaskPatch`/`ListFilter` name only the fields that vary. The
one remaining parser gap is field annotations:

- **Field annotations** → DTOs use plain field names; "optional on the wire" is
  a `T?` field plus a handler fallback. No `@rename`/`@no_serialize`/`@default` demo.

---

## §B Type-checker findings — the four that were workarounds are now fixed

Each of these once rejected the spec form the program wanted to write. Three
have since been fixed and the program now uses the real form; one (B3) is still
open. These were the highest-value findings — where spec-correct Rask hit a wall.

### B1 — `extend` on a nominal newtype — [#445], **fixed**
`type TaskId = u64 with (...)` then `extend TaskId { func next(...) }` used to
give `no method next found`. Fixed. The ids are nominal newtypes again —
distinct types that inherit Equal/Hashable/Comparable via `with` and carry
`next()` via `extend`, built `TaskId(n)`, unwrapped `.value`.

### B2 — `Ordering` nameable in user code — [#406], **fixed**
`x == Ordering.Equal` used to fail with `unknown type Ordering`. Fixed. The list
handler tie-breaks with `by_priority == Ordering.Equal`, and the priority test
asserts it directly. (A hand-written `Comparable` still needs all five of
compare/lt/le/gt/ge — no derive-from-compare — so EmailAddress, never sorted,
stays non-Comparable. That's the reason now, not "Ordering unnameable".)

### B3 — `Error` auto-derive (ER6) is unimplemented — [#378], open
A bare error enum gets no `message()` and can't be used as an error type
(`does not implement Error`). **Workaround:** every error enum needs
`@message` or a manual `extend … with Error`. AuthError carries `@message`
(no pure-auto-derive demo).

### B4 — `@message` auto-delegation (ER37) — [#446], **fixed**
A wrapper variant `Store(StoreError)` with no template auto-delegates its
`message()` to the inner error now. `ApiError` is a `@message` enum: its four
wrapper variants auto-delegate, its three prose variants carry templates, and
the hand-written `message()` match is gone.

### B5 — `try` union widening (ER31) fails for explicit unions — [#447]
`func f() -> T or (JsonError | ValidationError)` with `try json.decode(...)`
inside → `try propagates JsonError, but function returns … JsonError | ValidationError`,
even though JsonError ∈ the union. **Workaround:** no union-returning helper;
handlers decode + validate inline with explicit `else |e|` maps.

### B6 — `with <module-level const sync-box> as v` doesn't unwrap — [#448] (follow-up to #268)
`let store = Mutex.new(...)` then `with store as s { s.method() }` →
`no method … for Mutex<Store>`. A **local** `const` unwraps fine; module-level
doesn't. **Workaround:** inline `store.lock().op()` / `config.read().field`
everywhere. (This one also bites at codegen — §D.)

### B7 — generic `<T: Encode>` bound rejected at the call site — [#449]
`json_response<T: Encode>(status, v)` called with a concrete struct →
`_ does not implement Encode`. **Workaround:** response helper takes a `string`;
handlers call `json.encode(value)` directly (it accepts any encodable value).

### B8 — an adapter closure can't capture a local — [#450]
`v.iter().filter(|ms| ms >= budget_ms).count()` → `closure captures scoped
borrow and cannot escape`. With a literal (`ms >= 250.0`) it's fine.
**Workaround:** `slow_count` is a plain `for` loop. (Non-capturing chains —
`.iter().skip(o).take(n).map(|r| r.view.clone()).collect()` — are fine.)

---

## §C Stdlib surface gaps

- **C1 `SystemTime` is missing** ([#451]) — only `Instant`/`Duration`/`Timer` exist in
  the stub (time.md specs `SystemTime` + `unix_millis`). Timestamps use
  `started.elapsed().as_millis()` (monotonic-since-boot).
- **C2 `Duration.as_millis()` returns `i64`** ([#451]), but time.md/D says `u64`. Every
  timestamp field is `i64` to match the stub.
- **C3 HTTP server API absent** ([#452]) — only `http.listen_and_serve` and the client
  functions resolve. `http.listen`, `HttpServer`, `Responder`, `http.serve`
  (http.md S1–S3) don't. So the **linear-Responder + `ensure` accept loop —
  a flagship http.md feature — can't be written**; the program uses the
  convenience server, and that whole demo is gone.
- **C4 `StringBuilder` is unreachable from user code** ([#453]) — it's `public struct` in
  `stdlib/string.rk`, but no import path resolves it and it's not in the prelude.
  canonical-patterns recommends it "for loops or many concatenations".
  **Workaround:** `Vec<string>` + `join`.
- **C5 `Channel.buffered` returns a tuple `(Sender, Receiver)`** ([#359]), not an object
  with `.sender`/`.receiver` — canonical-patterns' `ch.sender.send(...)` is wrong;
  async.md's `mut (tx, rx) = …` is right. Also: module-level tuple-destructure
  (`let (tx, rx) = …` at file scope) is rejected as a "top-level statement",
  so the channel is created inside `main`.

---

## §D Codegen — now completes; the bugs this arc found are all fixed

Codegen was where "up to spec" was furthest off — the program type-checked but
didn't lower. That's closed: it builds a native binary and serves every path.
Getting there, restoring the idiomatic forms surfaced five native/runtime bugs
that the unit tests didn't. Each got a minimal repro and an issue, and each was
fixed on `main`:

- **#566 / #569** — a value returned from a cross-module `Mutex.lock().method()`
  came back corrupt: an Ok newtype's `.value` read a wrong offset, and the error
  payload of a `T or E` was garbage (a 404 crashed the server). Fixed in #575.
- **#567** — a scalar write in `with self.pool[h] as t { t.f = x }` wasn't
  flushed back to the pool slot (a Vec push in the same block was, which is what
  made PATCH look half-broken). Fixed.
- **#568** — `Request.query_param` segfaulted on any query string. Fixed; the
  filter/pagination endpoint serves.
- **#577** — a flaky (~40%) SIGSEGV/SIGILL during the batch transaction after a
  mixed request load: inlining a bare `return` left a `void or E` result slot
  unwritten, so `try` branched on stale stack. Fixed; the batch endpoint runs
  0/30 clean now.

The one open codegen-adjacent gap is #697: `rask test` on this example drags in
uninstantiated stdlib http-client functions that don't lower. `rask build`
DCEs them and serves fine.

---

## §E Spec findings that persist (filed last pass)

Still valid against `main`:

- **#336** `json.encode` return type — json.md (`-> string`, infallible) vs
  encoding.md (`-> string or JsonError`). The infallible signature is the real
  one (used it directly); encoding.md remains wrong.
- **#337** `using` clause vs return-type ordering — pools.md and
  canonical-patterns still disagree; program uses `-> Ret using Pool<T>`.
- **#338** LANGUAGE_GUIDE omits `with (...)` on nominal newtypes — a doc gap.
  Now that nominal `extend` works (B1) and the ids use it, the card is the one
  place a reader wouldn't learn the form the program relies on.
- **#340** OC1 × nominal `with (...)` delegation — unspecified. The ids are
  nominal now (B1 fixed); EmailAddress stays a struct with a custom `Equal`, so
  the interaction still isn't exercised, but the spec ambiguity stands.
- **#341** typed domain error → boundary enum has no `try` sugar — **fixed**
  (ER31a). `try` now places the error in the one ApiError variant that takes it.
  20 maps and 3 `to_api()` lifters deleted; the 6 `else |e|` maps left all say
  something the enum doesn't (a JSON error becomes a `BadRequest` message).
- **#342** block vs struct-literal ambiguity in `if`/`while` — confirmed still
  reproduces (`if x == Ord.Equal { }` fails to parse).

(#339, multi-producer `Sender` clone: not exercised this pass — the seed
pipeline is single-producer — but the async.md doc gap stands.)

---

## Metrics

### Ergonomic Delta (Rask vs Go), core handlers, body lines only

| Handler | Rask | Go | ED |
|---------|------|----|----|
| get_task | 2 | ~8 | 0.25 |
| list_tasks | 3 | ~7 | 0.43 |
| create_task | ~14 | ~27 | 0.52 |

Still well under the 1.2 target. `try store.lock().op()` is one line where Go
needs lock/call/unlock/`if err`. The delta is Go's `if err != nil` tax, removed
by `T or E` + `try`.

### Ceremony lines

| Kind | Count |
|------|-------|
| conformance declarations (`extend … with`) | 13 |
| `catch _ => …` decode guards | 7 |
| `as any Trait` casts | 6 |
| `ensure` | 1 |

Everything the design makes deliberately visible (conformances, casts, `ensure`)
stays cheap. Error mapping used to dominate at 26; the `try` auto-wrap (ER31a)
took out the 20 that only restated the enum, and the `catch _ =>` guards that
remain each write down a discard the old `else |e|` form hid (§F).

---

## What worked well on `main`

- **Nominal conformance enforcement + comma-lists + `duck trait`** — the trait
  surface is solid, and nominal newtype ids now carry traits + methods (B1).
- **Pools**: `Pool<T>` + `Handle<T>`, `with pool[h] as e`, `using [frozen] Pool<T>`
  context clauses, and handle-field auto-deref (`h.priority`, `dep.status`) run
  natively. The store reads beautifully.
- **`@resource` + `ensure`** with the commit/rollback batch transaction runs
  (C3–C5 shape), and after #577 it's stable under load.
- **`spawn(own || …)` + channel `send`/`receive` + `join`** run (startup seed
  pipeline).
- **Inline sync access** (`store.lock().op()`, `config.read().field`) is terse.
- **`T or E` + `try` + `catch e =>` + `??` + guards** cover the whole error
  surface with no `if err != nil` equivalents — and the `catch`/`??` split (§F)
  makes every swallowed error one grep.

---

## §F The try/orelse migration — readability verdict (2026-08-06)

Every old form in the program mapped mechanically; nothing needed a `match` or lost information.
34 sites across 7 files: 8 value fallbacks (`?? v` → `orelse v`), 17 diverging fallbacks
(`?? return X` → `orelse return X`), 6 decode guards (`try … else |e| V` → `… orelse return V`),
and 3 absence-propagations that collapsed into bare `try`.

**Where it got better:**

- The six decode guards dropped a binder nobody used. `try json.decode<T>(req.body) else |e|
  ApiError.BadRequest("invalid JSON")` carried an `|e|` that looked like a closure and bound
  nothing; `json.decode<T>(req.body) orelse return ApiError.BadRequest("invalid JSON")` says
  return where it returns. The old form also *hid* the return entirely — the else-value was
  implicitly returned, which is the invisible exit the redesign exists to kill.
- `parse_port` went from four lines of ceremony to two honest ones:
  `const s = try raw` / `return try s.parse<u16>()`. Same in `opt_id`. Bare `try` on optionals
  is the right tool exactly where predicted (the 7-sites-propagate-none case).
- The store's NotFound guards read as guard-lets:
  `const h = self.by_id.get(id) orelse return StoreError.NotFound(id.value)` — one word instead
  of a symbol, and the exit is in the line.
- `orelse return deny(AuthError.Missing)` in the auth middleware shows the divergence isn't
  error-flavored: `before()` returns a plain `MwOutcome`, and the same operator reads fine.

**Where it costs:**

- **Long lines got one wrap deeper.** The decode guards and TxConflict guards were already at
  ~100 cols; `orelse return` pushed 7 lines over, and the fix is the continuation form
  (`orelse` leading the next line). It reads well — the exit gets its own line — but the
  formatter must handle it, and `rask fmt` has no rule for it yet. Flagging for task #3.
- **The left-margin exit scan is genuinely weaker.** Under the old rule every exit line started
  with `try`; now `to_txop`'s exits sit mid-line after `raw.value`. In *this* program the loss
  is small — the wrapped continuation puts long exits back at the line head, and short ones
  (`orelse return none` class) became bare `try` — but it's real, and it's the cost the spec's
  reversal record already prices in (`type.errors` appendix).
- One TxConflict continuation line still lands at 101 cols. Cosmetic.

**What the program never exercises** — untested by this corpus, still spec-only:
`try f() orelse v` on a flat `T? or E` (no such callee here), `orelse e =>` with the binder
*used* (every error this program replaces is replaced blind), `orelse` chains (no
multi-source fallback), and `take` (the store swaps nothing out of optional slots).
A corpus that exercises those four is worth having before the implementation freezes.

**Verdict:** the flagship reads better, not worse — mostly from deleting the fake binder and
the four-line optional ceremony. No spec change requested from this pass; the two debts
(formatter rule for continuation `orelse`, weaker margin scan) are known and priced.

**Addendum — the four gaps, exercised (`examples/tiered_store.rk`).** A ~260-line LSM-shaped
store (the ER47 rationale's example, made real) hits all four. Verdicts:

- **The composite is the best line in the file.** `const v = try t.lookup(key) orelse continue`
  routes both bad branches visibly — error up, absence to the next tier. The ER16b precedence
  ruling (no parens) carries its weight.
- **`take` composes with `orelse` naturally** — `const staged = take self.pending orelse return`
  reads as one clause for "grab the staged work or go back to sleep". But the grouping
  (`(take place) orelse …`) is a precedence ruling OPT32 never states. Flagged on #586.
- **Both binder flavors earn their keep once context exists to attach**: diverging
  (`orelse e => return StoreError.FlushFailed(n, e)` — `n` is context only this frame has) and
  value (`orelse e => println("stats dropped: {e.message()}")` — best-effort stats).
- **New finding: the flat shape is match-only at infallible boundaries.** In `main`, `try` has
  nowhere to propagate and `orelse` can't consume a three-branch left side (ER14), so
  `string? or StoreError` is consumed by a three-arm `match` — which turns out to be *right*
  (three outcomes genuinely differ) but nothing in the spec says so. Worth one line in
  error-types.md's flat-shape section when it's next touched.

No spec change forced; one grammar ruling owed (take/orelse precedence, #586).

**Addendum 2 — round three: the merged word failed the designer's reading test.** Reading the
migrated flagship at length surfaced the constant unpaid question at every fallback site: *was
that an error just now, and did it survive?* `orelse` couldn't answer it without a signature
lookup — elegant, low-info. Resolution (now spec): fallbacks split by what they destroy. `??` is
back for absence (a miss carries nothing; terse is right), and keeps the diverging right side.
Failures get `catch e =>` / `catch _ =>` — binder **mandatory**, no bare-value form, so a
swallowed error is always written down (`catch _ =>` is one grep). `try` unchanged; the composite
is `try f() ?? continue` and now glosses itself: error up, absence here. Both corpora
re-migrated: the flagship's 28 absence sites wear `??`, its 6 decode guards wear
`catch _ => return …` — the discard the old `|e|` form hid is now the loudest thing on the line.
This closes the "swallowed errors are unmarked" debt at the grammar level; the fmt continuation
rule now covers `catch` bodies too.
