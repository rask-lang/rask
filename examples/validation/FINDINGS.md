# Validation findings — HTTP JSON API server

Program: `examples/validation/` — an in-memory work-item tracker (15 files,
~1560 lines), written to the **spec**. This pass was redone against `main`
after the codegen work landed.

**The big change since the last pass:** it builds and runs — every path. Last
time the program type-checked but the MIR→Cranelift layer couldn't lower most
of it. Now `rask build` produces a native binary that serves real traffic:
full CRUD, transactions, the seed pipeline, metrics, filtering, pagination, and
the whole error surface.

## Status

| Stage | Result |
|-------|--------|
| `rask parse` / `rask build` type-check | clean — 0 errors |
| `rask build` → native binary | **builds** |
| `rask test` | **8/8 pass** |
| Native server, CRUD + errors + filtering | **works** |
| Native server, batch transaction under load | **flaky crash — [#577]** |

Four native/runtime codegen bugs turned up mid-pass (§B). All four were filed
with minimal repros **and fixed on `main` within this pass**, so the program
runs them the idiomatic way — the workarounds are gone. A fifth, harder one is
still open: a heap-state-dependent flaky crash during the batch transaction
([#577], §B4). None is a language problem; all are backend.

## How to verify

```
rask test examples/validation                       # 8/8
rask build examples/validation --release            # builds
./examples/validation/build/release/tracker &       # serves on :8080
curl -H 'Authorization: Bearer dev-secret' localhost:8080/tasks
curl -X POST -H 'Authorization: Bearer dev-secret' \
     -d '{"title":"x","project_id":1,"priority":"high"}' localhost:8080/tasks
```

---

## §A What works, natively

Everything the design leans on now compiles and runs:

- **Full CRUD** — create/get/list/update/delete for tasks, plus users and
  projects. POST returns 201 with the created view; the writes stick.
- **`PATCH` actually mutates** — status/priority/tags all persist through a
  plain `with self.tasks[h] as t { t.status = s }` block.
- **Pools + handles** — `Pool<Task>`, `Handle<Task>`, `with pool[h] as t`,
  `using [frozen] Pool<T>` context clauses, and handle-field auto-deref
  (`h.priority`, `dep.status`). The dependency graph is `Vec<Handle<Task>>`;
  `blocked` walks it. Reads beautifully.
- **`@resource` + `ensure`** — the batch transaction stages ops, validates,
  commits, and `ensure tx.rollback()` covers every early exit (C3–C5). It
  type-checks and runs, but flaky-crashes under a mixed load (B4/#577).
- **`spawn(own || …)` + channel `send`/`receive` + `join`** — the startup seed
  streams specs over a channel into the store.
- **`Mutex` / `Shared` via `with`** and inline `.lock()`/`.read()` — the store
  is a module-level `Mutex`, config a `Shared`.
- **Nominal newtypes** — `type TaskId = u64 with (Equal, Hashable, Comparable,
  Debug)`, constructed `TaskId(n)`, unwrapped `.value`, stepped via an `extend`
  method. Distinct types: a `TaskId` can't be passed for a `ProjectId`.
- **Struct field defaults** — `Config {}` is the fully-defaulted value; `Task`,
  `TaskPatch`, `ListFilter` name only the fields that vary (FD1/FD3).
- **`T or E` + `try` + auto-wrap + `try…else` + `??` + guards** — the whole
  error surface, no `if err != nil`. `try` places a domain error into the
  boundary enum by itself (see §C).
- **Encoding** — request DTOs auto-derive Decode, response views auto-derive
  Encode, no declarations.
- **Duck trait + `as any Trait`** — the dev-only `/debug/inspect` uses a
  `duck trait Inspectable` and heterogeneous `as any Inspectable` dispatch.
- **Full error responses** — 401 (auth), 400 (bad JSON), 422 (validation), 404
  (store not-found), 503 (capacity) all render the right message + code +
  status. The store-originated ones ride back through the mutex; that used to
  crash (B/#569) and doesn't now.
- **Filtering + pagination** — `GET /tasks?status=open&priority=high&limit=10`
  reads its filter through `query_param` and pages with `.skip().take()`.

---

## §B Bugs found this pass — native codegen / runtime (all fixed on `main`)

Restoring the spec-idiomatic forms surfaced four backend bugs. Each got a
minimal repro and an issue, and each was fixed on `main` during this pass — so
the program now runs the direct form with no workaround. The value here is the
repros; they're the record of what a program-scale test caught that the unit
tests didn't.

### B1 — a value returned from a cross-module `Mutex.lock().method()` was corrupt — [#566] (Ok side), [#569] (Err side), fixed in #575
The big one. When a handler called `try store.lock().op()` and `op` lived in
`store.rk` while the caller was in `handlers.rk`, the returned value lost its
type identity across the module boundary:

- **Ok side (#566):** reading a returned newtype's `.value` read a wrong offset
  — MIR printed `unresolved field 'value', defaulting to I64` and the deref
  segfaulted (`POST /projects`).
- **Err side (#569):** the returned error's inner payload was garbage. The outer
  tag survived — a shallow `match` was fine — but `e.message()` or a `match` on
  the inner error trapped (SIGILL). Every store error crosses the mutex, so a
  plain `GET /tasks/<nonexistent>` (→ 404) crashed the server.

Both vanished in a single file or with a local (non-mutex) receiver — same
mechanism as [#545] (MIR carrying the type as a string). Now the handler builds
`ProjectView` inline with `id.value`, and 404s return clean JSON.

### B2 — a scalar field write in `with self.pool[h] as t { t.f = x }` was lost — [#567], fixed
Native codegen didn't flush a **scalar** field write back to the pool slot when
the `Pool` was reached through a struct field (`self.tasks[h]`). Inside the block
it read back fine; after, the slot was unchanged. A Vec push in the same block
*did* persist (shared buffer, not the slot), which is what made `PATCH` look
half-broken — tags stuck, status didn't. The interpreter got it right. Now
`update_task` writes `t.status = s` inline and it sticks.

### B3 — `Request.query_param` segfaulted on any query string — [#568], fixed
`req.query_param(key)` crashed the native server the moment a request carried a
query string (no query → `none`, so it looked healthy). Now the filter/pagination
endpoint (`GET /tasks?status=open&limit=10`) serves.

### B4 — flaky crash in the batch transaction under a mixed load — [#577], open
The one still open. Once the deterministic mutex bugs (B1) were fixed, a flaky
one surfaced underneath: run a realistic sequence (create a user/project/task,
PATCH it, hit a 404) and then `POST /tasks/batch`, and the server dies during
the batch ~40% of the time — SIGSEGV or SIGILL (a Cranelift `unreachable`, i.e.
a match on a corrupted tag). It's heap-state-dependent: batch alone is fine,
PATCH-then-batch alone is fine, the seed pipeline alone is fine — it takes the
accumulated allocation history to trip it. The batch path is the linear
`@resource` commit writing through pool handles with a `using Pool<Task>`
context; the PATCH path mutates the same pool via a `with` block. Couldn't
reduce below "a realistic mix, then batch" — it needs an ASAN build of the
runtime to pin. Pre-existing on `main`, independent of the example's code shape.

---

## §C Spec forms that were workarounds two passes ago — now idiomatic

Each of these was a documented workaround before. They now work, so the program
uses the real form:

- **Nominal `extend` on newtypes** ([#445] fixed) — ids are `type X = u64 with
  (...)` again, not struct wrappers.
- **`try` → boundary enum auto-wrap** ([#341] fixed, ER31a) — `try store.lock().op()`
  places a `StoreError` into `ApiError.Store` by itself. Deleted ~20 `else |e|`
  maps and 3 hand-written `to_api()` lifters; the 6 `else |e|` left each carry
  real information (a JSON error becoming a `BadRequest` message).
- **Struct field defaults** (FD1/FD3) — see §A.
- **`Ordering` is nameable** ([#406] fixed) — `list_views` tie-breaks with
  `by_priority == Ordering.Equal`; the priority test asserts it directly.
- **`@message` auto-delegation** (ER37) — `ApiError`'s wrapper variants
  (`Store(StoreError)`, …) delegate `message()` to the inner error with no
  hand-written match; prose variants carry their own `@message` template. The
  hand-written `extend ApiError with ErrorMessage` is gone.
- **`with <module-level Mutex> as v`** (was #448) — now unwraps to the inner
  type. The program keeps inline `.lock()` in handlers (both are valid — MX1 /
  MX3 — and inline is terser), but the `with` form is no longer blocked.
- **Field annotations parse** (`@rename`/`@skip`/`@default`) — no longer a parse
  error. Not exercised in the DTOs yet (would need a decode round-trip check),
  so the wire names still match the field names; noted for completeness.

---

## §D Gaps that persist

Still worked around, still worth fixing:

- **ErrorMessage pure auto-derive (ER6)** ([#378]) — a bare error enum with no
  `@message` still gets no `message()` and can't be used as an error type. So
  `AuthError`/`StoreError` carry `@message`; a zero-annotation error enum can't
  be shown.
- **Manual `Comparable` needs all five methods** — a hand-written conformance
  must supply `compare` *and* `lt`/`le`/`gt`/`ge`; the four booleans aren't
  derived from `compare`. `EmailAddress` (custom case-insensitive `Equal`) stays
  non-`Comparable` rather than hand-writing five methods for a type that's never
  sorted. (This is the real reason now, not the old "Ordering unnameable".)
- **`SystemTime` missing** ([#451]) — only `Instant`/`Duration`/`Timer` in the
  stub. Timestamps are monotonic-since-boot. `Duration.as_millis()` returns
  `i64`, not `u64` per time.md.
- **`StringBuilder` unreachable from user code** ([#453]) — `/debug/inspect`
  uses `Vec<string>` + `join`.
- **Manual accept loop absent** ([#452]) — only `http.listen_and_serve` resolves;
  `http.listen` + linear `Responder` + `ensure` (the flagship http.md loop)
  can't be written. The program uses the convenience server.
- **`<T: Encode>` bound rejected at the call site** ([#449]) — the response
  helper takes a `string`; handlers call `json.encode(v)` directly.
- **Adapter closure can't capture a local** ([#450]) — `slow_count` is a plain
  loop; non-capturing chains (`.skip().take().map().collect()`) are fine.

---

## §E Spec-doc findings that persist

- **[#336]** `json.encode` return type — json.md (`-> string`) vs encoding.md
  (`-> string or JsonError`). The infallible signature is the real one.
- **[#337]** `using` clause vs return-type ordering — pools.md and
  canonical-patterns disagree; program uses `-> Ret using Pool<T>`.
- **[#342]** block vs struct-literal ambiguity in `if`/`while` — `if x ==
  Ord.Equal { }` still fails to parse (worked around with a bound const where it
  bit).

---

## Metrics

### Ergonomic delta (Rask vs Go), core handlers, body lines only

| Handler | Rask | Go | ED |
|---------|------|----|----|
| get_task | 2 | ~8 | 0.25 |
| list_tasks | 3 | ~7 | 0.43 |
| create_task | ~9 | ~27 | 0.33 |

Under the 1.2 target. `try store.lock().op()` is one line where Go needs
lock/call/unlock/`if err`.

### Ceremony lines

| Kind | Count |
|------|-------|
| conformance declarations (`extend … with`) | 13 |
| `else \|e\| …` error mapping | 6 |
| `as any Trait` casts | 6 |
| `ensure` | 1 |

Everything the design makes deliberately visible stays cheap. Error mapping was
26 before ER31a; auto-wrap removed the 20 that only restated the enum, and the 6
left each say something the enum doesn't.
