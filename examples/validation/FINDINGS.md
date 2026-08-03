# Validation findings — HTTP JSON API server

Program: `examples/validation/` — an in-memory work-item tracker (16 files,
~1600 lines), written to the **spec**. This pass was redone against `main`
after ~112 compiler commits landed.

**Status on current `main`:**

| Stage | Result |
|-------|--------|
| `rask parse` (every file) | **clean — 0 errors** |
| `rask build` type-check phase | **clean — 0 type errors** |
| `rask build` MIR/codegen phase | fails — native codegen still broadly incomplete (see §D) |

So the program is **spec-valid and fully type-checks**. It does not yet produce
a native binary, because the MIR→Cranelift layer can't lower large parts of it
— including stdlib functions the program only touches indirectly (`IoError.message`,
`http.listen_and_serve`). That's a compiler-maturity axis separate from the
language, and it's where the remaining work is.

The parser is the big win since last pass: three of the five constructs that
didn't parse before now do. The friction moved down a layer — into the type
checker (worked around, §B), the stdlib surface (§C), and codegen (§D).

---

## How to verify

```
rask parse examples/validation/<file>.rk     # 0 errors, every file
rask build examples/validation               # 0 error[E…] type errors; MIR errors remain
```

`rask build` runs type-check then MIR lowering. Grep its output: there are no
`error[E####]` (type) diagnostics; every `error:` is a `MIR lowering` / `codegen`
line (§D).

---

## §A Parser — now essentially up to spec

Last pass, five spec constructs didn't parse. Now:

| Construct | Before | Now |
|-----------|--------|-----|
| `duck trait` | ✗ | **✓ parses + checks** |
| `scoped extend` | ✗ | **✓ parses + checks** |
| comma-list `extend T with A, B` | ✗ | **✓ parses + checks** |
| struct field defaults `f: T = expr` | ✗ | ✗ still unimplemented (#311) |
| field annotations `@rename/@skip/@default` | ✗ | ✗ still unimplemented |

Nominal trait conformance is now **enforced** (G1) — a good change; the program
relies on it. The two remaining parser gaps (field defaults, field annotations)
forced the only spec-shaped code I had to abandon:

- **Field defaults** → explicit `.new()` constructors carry the defaults, and
  every field is written at construction. `Config {}`-as-default-value (the
  "No Default trait" design) can't be shown.
- **Field annotations** → DTOs use plain field names; "optional on the wire" is
  a `T?` field plus a handler fallback. No `@rename`/`@skip`/`@default` demo.

---

## §B Type-checker gaps (worked around to keep it checking)

Each of these type-checks fine in the spec form I *wanted* to write, but the
checker rejects it, so the program uses the noted workaround. These are the
highest-value findings — they're where writing spec-correct Rask hits a wall.

### B1 — `extend` on a nominal newtype resolves no methods — [#445]
`type TaskId = u64 with (...)` then `extend TaskId { func next(...) }` →
`no method next found for type TaskId`, even for a plain method. `type.aliases/T13`
says extend works on nominal types; it doesn't. **Workaround:** ids are
`struct TaskId { public value: u64 }` (the "full struct wrapper" option). This
also mooted the previous OC1-on-nominal question (#340) — EmailAddress is a struct.

### B2 — `Ordering` can't be named in user code — [#406]
`func compare(self, o) -> Ordering` and `x == Ordering.Equal` both fail with
`unknown type Ordering` — the checker carries it as an unresolved placeholder.
**Workaround:** never name it. Comparators return `a.compare(b)` directly and
tie-break by falling through; tests use `<`/`>`. Consequence: a hand-written
`Comparable` conformance is impossible (its signature must name `Ordering`), so
the EmailAddress `Comparable` override from the OC1 demo had to be dropped —
only `Equal` + `Hashable` remain.

### B3 — `ErrorMessage` auto-derive (ER6) is unimplemented — [#378]
A bare error enum gets no `message()` and can't be used as an error type
(`does not implement ErrorMessage`). **Workaround:** every error enum needs
`@message` or a manual `extend … with ErrorMessage`. AuthError became `@message`
(losing the pure-auto-derive demo).

### B4 — `@message` auto-delegation (ER37) is unimplemented — [#446]
A wrapper variant `Store(StoreError)` with no template →
`has no message template and cannot auto-delegate`. **Workaround:** ApiError
drops `@message` and hand-writes `message()`, delegating to `e.message()` per arm.

### B5 — `try` union widening (ER31) fails for explicit unions — [#447]
`func f() -> T or (JsonError | ValidationError)` with `try json.decode(...)`
inside → `try propagates JsonError, but function returns … JsonError | ValidationError`,
even though JsonError ∈ the union. **Workaround:** no union-returning helper;
handlers decode + validate inline with explicit `else |e|` maps.

### B6 — `with <module-level const sync-box> as v` doesn't unwrap — [#448] (follow-up to #268)
`const store = Mutex.new(...)` then `with store as s { s.method() }` →
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
  (`const (tx, rx) = …` at file scope) is rejected as a "top-level statement",
  so the channel is created inside `main`.

---

## §D Codegen — native build does not complete — [#454] (umbrella #203)

Type-check passes; MIR lowering then fails across the board. Representative:

```
error: MIR lowering 'handle_create_task': InvalidConstruct("method `create_task`
  on receiver of unresolved type — dispatch could not determine a stdlib type prefix")
error: MIR lowering 'Store_update_task': InvalidConstruct("method `has_tag` …")
error: MIR lowering 'task_is_blocked': InvalidConstruct("method `is_terminal` …")
error: MIR lowering 'http_listen_and_serve': InvalidConstruct("method `close` …")
error: MIR lowering 'IoError_message': UnresolvedVariable("msg")
```

Three distinct codegen problems, none fixable from the program:

- **D1** Method dispatch can't resolve the receiver type for calls on `.lock()`
  results, pool-element bindings, `Handle` context derefs, and even `self`-typed
  receivers inside methods (`t.has_tag`). This is the bulk of the errors.
- **D2** A module-level `Mutex<UserStruct>` + a `mutate self` method also
  produces a Cranelift `mismatched argument count` verifier error in isolation.
- **D3** Passing a function by name as a value (`listen_and_serve(addr, route)`)
  → `UnresolvedVariable("route")`. Fixed in-program with a closure
  (`|req| { route(req) }`), but `http.listen_and_serve`'s own stub still fails
  to lower (`.close()` on unresolved type), as does `IoError.message`.

Net: the codegen layer is where "up to spec" is still furthest off. The language
as the type checker sees it is in good shape; the backend can't yet emit it.

---

## §E Spec findings that persist (filed last pass)

Still valid against `main`:

- **#336** `json.encode` return type — json.md (`-> string`, infallible) vs
  encoding.md (`-> string or JsonError`). The infallible signature is the real
  one (used it directly); encoding.md remains wrong.
- **#337** `using` clause vs return-type ordering — pools.md and
  canonical-patterns still disagree; program uses `-> Ret using Pool<T>`.
- **#338** LANGUAGE_GUIDE omits `with (...)` on nominal newtypes. Compounded now
  that nominal `extend` is broken (B1): the card points at a form that neither
  carries traits nor takes methods.
- **#340** OC1 × nominal `with (...)` delegation — unspecified. Sidestepped by
  using a struct (B1), but the ambiguity stands.
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
| conformance declarations (`extend … with`) | 11 |
| `else \|e\| …` error mapping | 6 |
| `as any Trait` casts | 6 |
| `ensure` | 1 |

Everything the design makes deliberately visible (conformances, casts, `ensure`)
stays cheap. Error mapping used to dominate at 26; ER31a took out the 20 that
only restated the enum declaration, and the 6 that are left each carry real
information.

---

## What worked well on `main`

- **Nominal conformance enforcement + comma-lists + `duck trait`** all parse and
  check — the trait surface is solid now.
- **Pools**: `Pool<T>` + `Handle<T>`, `with pool[h] as e`, `using [frozen] Pool<T>`
  context clauses, and handle-field auto-deref (`h.priority`, `h.deps`,
  `dep.status`) all type-check. The store reads beautifully.
- **`@resource` + `ensure`** with the commit/rollback transaction pattern
  type-checks (C3–C5 shape).
- **`spawn(own || …)` + channel `send`/`receive` + `join`** type-check (startup
  seed pipeline).
- **Inline sync access** (`store.lock().op()`, `config.read().field`) type-checks
  and is terse.
- **`T or E` + `try` + `try…else` + `??` + guards** cover the whole error surface
  with no `if err != nil` equivalents.
