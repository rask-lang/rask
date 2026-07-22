# Validation findings — HTTP JSON API server

Program: `examples/validation/` — an in-memory work-item tracker (16 files,
~1600 lines). Written to the **spec**, not to what the current binary accepts.
Purpose was to pressure-test the post-flip language at program scale.

Bottom line: the language holds up well at this size. The handler layer is
genuinely terser than Go. The friction is concentrated in three places — a
missing "typed error → boundary enum" propagation form (23 hand-written
mappings), a few **spec-vs-spec contradictions** that leave a writer guessing,
and one **doc gap** in the language card that would actively mislead.

Everything below separates **spec problems** (what this exercise is for) from
**compiler lag** (parser behind the decided spec — real, but a different bug
class).

---

## How to verify

Parsing is the bar (checker is behind the spec). Per file:

```
rask parse examples/validation/<file>.rk
```

8 of 16 files parse fully clean. The other 8 fail **only** on the five
parser-lag constructs catalogued in the Compiler-lag section — every parse
error in the tree is one of those five, none is a code bug. `rask fmt
examples/validation` exits 0. `rask lint` reports no issues.

---

## Spec findings

Ordered by how much they'd hurt someone writing Rask from the spec.

### S1 — The language card omits the `with (...)` clause on nominal newtypes (HIGH) — [#338]

`LANGUAGE_GUIDE.md` (Types) shows:

```rask
type UserId = u64          // NOMINAL — a distinct type, not an alias
```

and separately lists Equal/Hashable/Comparable as "auto-derived … for eligible
types". A reader joins those and writes `type TaskId = u64`, then uses it as a
`Map<TaskId, …>` key. It doesn't work: `type.aliases/T10` says nominal types do
**not** inherit traits — you need `type TaskId = u64 with (Equal, Hashable,
Comparable)` (T11). The card never shows the `with (...)` clause at all, and the
generics "auto-derive" table doesn't mention that newtypes are excluded from it.

This bit immediately — the ids are Map keys and sort keys, so all three had to
become:

```rask
type TaskId = u64 with (Equal, Hashable, Comparable, Debug)
```

Fix is a doc one: the card's `type X = Y` line should carry the `with (...)`
clause, and the auto-derive table should note "structs and enums only; nominal
newtypes opt in via `with`". (spec: LANGUAGE_GUIDE Types / Traits vs
type.aliases/T10–T11)

### S2 — Multi-producer channels aren't in the normative concurrency spec (HIGH) — [#339]

A server fans one channel out to many producers. This program has N request
tasks emitting audit events to one worker, so it needs multiple senders:

```rask
const tx = audit_tx.clone()      // one Sender per connection task
spawn(|| { ... emit(tx, ...) ... }).detach()
```

`concurrency/async.md` documents `Sender`/`Receiver`, `send`/`receive`, close
semantics, and calls them "non-linear" (CH1) — but never says a `Sender` is
`Cloneable`, and there's no example of more than one producer. The only place
`tx.clone()` and "Arc … safe to clone and send" appear is `runtime.md`, which is
the implementation-strategy doc, not the normative surface. Writing a
multi-producer server *from the spec* is not possible today — you have to infer
it or read the runtime internals. The normative spec should state that senders
(and `Shared`/`Mutex` handles) clone to share, with a multi-producer example.
(spec: conc.async/CH1 — missing; runtime.md l.1254/1718 has it)

### S3 — `json.encode` has two contradictory signatures (HIGH) — [#336]

- `std/json.md` (l.77): `json.encode(value: T) -> string` — infallible
- `std/encoding.md` (l.267): `func encode<T: Encode>(value: T) -> string or JsonError`

Same function, different return type. It changes call sites materially: with the
first you write `json.encode(v)`, with the second `try json.encode(v)`. I
followed json.md (the module's own spec) and made `json_response` treat it as
infallible. A reader who trusts encoding.md writes dead `try`/error paths. One of
the two specs needs to change. (spec: std.json l.77 vs std.encoding l.267)

### S4 — No `try` form for "typed domain error → boundary enum"; 23 hand-mappings (MEDIUM) — [#341]

This is the single biggest ceremony source in the program. `try` auto-widens an
error only when the target is a **union that already contains it** (ER31), or
boxes into `any Error` (ER32). A boundary **enum** — the documented "typed domain
errors" pattern (error-types Appendix) — is neither. So every call from a handler
into the store reads:

```rask
const view = with store as s { try s.view_task(id) else |e| e.to_api() }
```

where `to_api()` is a one-liner I had to add to `StoreError`, `ValidationError`,
and `AuthError`:

```rask
extend StoreError { func to_api(self) -> ApiError { return ApiError.Store(self) } }
```

Count: **23** `else |e| …` mappings across the handlers, plus 3 `to_api`
declarations. The `any Error` escape hatch (ER32) removes the ceremony but throws
away the typed `match` at the boundary — the whole reason to have `ApiError`.

The gap is specific: union-widening (ER31) and any-boxing (ER32) both get
implicit `try`, but the third documented shape — an enum with one variant per
wrapped error — gets nothing. Candidate: let `try` auto-wrap when the target
enum has exactly one variant whose single payload is the source error type
(the enum equivalent of ER31's subset check). It's the most repeated line in the
codebase; worth a rule. (spec: type.errors/ER31–ER32 + Appendix "Typed domain
errors")

### S5 — `using` clause vs return-type ordering is unspecified and inconsistent (MEDIUM) — [#337]

Two specs put the pieces in opposite orders, both parse, neither is stated as
canonical:

- `memory/pools.md`: `func get_health(h: Handle<Entity>) using frozen Pool<Entity> -> i32`
- `canonical-patterns.md`: `func process(...) -> ProcessResult or IoError using Pool<Node>`

I picked `-> Ret using Pool<T>` (return type first) for the whole program and had
to guess. `mem.context-clauses` should fix one order and the other spec's example
should follow it. (spec: mem.context vs canonical-patterns "Rich Signatures")

### S6 — OC1's interaction with nominal-newtype `with (...)` traits is undefined (MEDIUM) — [#340]

I wanted a case-insensitive email with consistent hashing. The natural spelling
is a nominal newtype:

```rask
type EmailAddress = string with (Equal, Hashable, Comparable)
extend EmailAddress with Equal { ... case-insensitive ... }   // override
```

OC1 says "overriding Equal cancels the **auto-derived** Hashable and Comparable".
But `with (...)`-clause traits on a nominal type are "delegated" (T12), not
"auto-derived". Does overriding `Equal` cancel the delegated `Hashable`/
`Comparable`? The spec doesn't say. If it doesn't, you silently keep u64/string
hashing that disagrees with your custom eq — exactly the bug OC1 exists to
prevent. I dodged it by making `EmailAddress` a **struct** (where auto-derive and
OC1 are clearly defined), but the nominal case is the one people will reach for.
OC1 should state whether it covers `with`-clause delegation. (spec:
type.generics/OC1 vs type.aliases/T11–T12)

### S7 — Block vs struct-literal ambiguity in `if`/`while` conditions (MEDIUM) — [#342]

```rask
if by_prio == Ordering.Equal { return a.id.compare(b.id) }   // does NOT parse
```

The parser reads `Ordering.Equal { … }` as a struct literal and never sees the
`if` block. `Ordering.Equal` is a **unit** variant — it can't be a struct literal
— so this is disambiguable in principle. Worked around by binding first:

```rask
const tie = by_prio == Ordering.Equal
if tie { return a.id.compare(b.id) }
```

This shows up any time you compare against a qualified enum constant right before
a block. `control-flow.md`/`SYNTAX.md` say nothing about the rule; Rust and Swift
both forbid bare struct literals in condition position for exactly this reason.
Needs a documented rule (and ideally the unit-variant case just working).
(parser + spec silence: control.flow / SYNTAX)

### S8 — `static` vs `const` for module-level shared singletons (LOW)

`concurrency/sync.md` (l.329) declares shared state with `static`:

```rask
static CONFIG: Shared<AppConfig> = Shared.new(AppConfig {})
```

The `examples/http_api_server.rk` in this repo uses `const`:

```rask
const db = Shared.new(Database.new())
```

I used `const` (three singletons: `config`, `metrics`, `store`). The specs never
say which is idiomatic for module-level `Shared`/`Mutex`, or whether `static`
even exists as a binding form (it's not in the SYNTAX card). Pin one down.
(spec: conc.sync l.329 vs repo example)

### S9 — Match arms don't auto-wrap into `T or E`, which shapes routing (LOW)

Auto-wrap fires only at `return` (ER9/ER11), and a `match` arm isn't a return
position. So a router whose arms produce bare `Response` can't have the `match`
value widen to `Response or ApiError`. Every arm had to either call a function
that already returns the union or use an explicit `return`:

```rask
match (req.method, path) {
    (Method.Get, "/health") => return handle_health(),   // explicit return in every arm
    ...
}
```

It's consistent with ER11, and the fix (make every handler return the identical
union) is fine — but it's a small surprise given blocks and match are
expression-valued everywhere else. Worth a one-line note in error-types that
match/if arms don't participate in return-site auto-wrap. (spec: type.errors/ER11)

### S10 — `primitives.md` calls int→float `as` "Lossless" while admitting it rounds (MINOR)

CV1's header lists int→float under "Lossless"; CV4's note says int→float via `as`
"rounds but never wraps or corrupts". `u64 as f64` silently loses precision past
2^53. The conversion being allowed via `as` is a fine choice — the label just
shouldn't say "lossless". (spec: type.primitives/CV1 vs CV4)

---

## Compiler lag (NOT spec defects — parser behind the decided spec)

Every parse failure in the program is one of these. Written to spec anyway; each
is a decided feature the parser doesn't accept yet. Filed as issues.

| # | Construct | Spec | `rask parse` says |
|---|-----------|------|-------------------|
| L1 | Struct field defaults `field: T = expr` | type.structs; LANGUAGE_GUIDE l.133 (tracked: #311) | `Expected name, found '='` |
| L2 | Field annotations `@rename`/`@skip`/`@default` | std.encoding/E18–E20 | `Expected name, found '@'` |
| L3 | `duck trait` | type.generics/G1 | `Expected ';' or newline` |
| L4 | `scoped extend` | type.generics/MN4 | `Expected ';' or newline` |
| L5 | Comma-list conformance `extend T with A, B` | type.generics/CD1; LANGUAGE_GUIDE l.203 | `Expected '{', found ','` |

L5 is worth flagging twice: the verbatim card example `extend Ring<T> with
Countable, Sizable {}` does not parse.

**Doc bug adjacent to lag:** LANGUAGE_GUIDE's "Spec vs compiler (temporary)"
section lists "`duck trait`/`scoped extend`/`public extend` parsing" among things
the compiler *handles*. In fact `public extend` parses but `duck trait` and
`scoped extend` do **not** (L3/L4). The card overstates current parser support.

These lag items are why the program exercises L1–L5 in code but can't be run
through `rask check`/`build`. That's expected — the checker also still
implements pre-flip trait rules.

---

## Metrics

### Ergonomic Delta (ED) — Rask lines vs a Go equivalent, 3 core handlers

Bodies only (signature + braces excluded), counting logical lines.

**`handle_get_task`** — Rask **2**:

```rask
const view = with store as s { try s.view_task(id) else |e| e.to_api() }
return json_response(200, view)
```

Go equivalent — **~8**:

```go
func handleGetTask(w http.ResponseWriter, id uint64) {
    store.mu.Lock()
    t, err := store.viewTask(id)
    store.mu.Unlock()
    if err != nil { writeError(w, err); return }
    writeJSON(w, 200, t)
}
```

**`handle_list_tasks`** — Rask **3** (filter parse + store call + encode); Go
**~7** (query parse loop, call, `if err`, encode). Store-side sort/paginate is
equal in both.

**`handle_create_task`** — Rask **15** (decode/validate union match, priority
parse, create, emit, view, encode); Go **~27** (decode + `if err`, validate +
`if err`, priority switch, lock/create/unlock + `if err`, view + `if err`,
encode). 

| Handler | Rask | Go | ED (Rask/Go) |
|---------|------|----|----|
| get_task | 2 | 8 | 0.25 |
| list_tasks | 3 | 7 | 0.43 |
| create_task | 15 | 27 | 0.56 |

ED well under the 1.2 target. The wins come from `with` (lock scope is one word,
no unlock line, no lock leak), `try … else |e|` (decode-or-map is one line, not
three), and match-based routing. Rask's `T or E` + `try` removes Go's `if err !=
nil { return }` tax, which is most of the delta.

### Ceremony lines

Lines whose only job is satisfying the type system / cleanup discipline, not
domain logic:

| Kind | Count | Notes |
|------|-------|-------|
| Conformance declarations (`extend T with Trait`) | 11 blocks | + 3 nominal `with (...)` clauses |
| `ensure` (linear cleanup) | 3 | server close, responder fallback, tx rollback |
| `as any Trait` casts | 5 | middleware chain (3), inspect (2) |
| `else \|e\| …` error mapping | **23** | the S4 finding, by far the largest |

23 error-mapping lines against 3 `ensure` and 5 casts is the headline: the
conformance/cast/cleanup ceremony the design deliberately makes visible is
*cheap*; the unplanned ceremony is the typed-error→enum plumbing (S4). If S4 gets
a `try` form, ceremony drops by ~half.

---

## What worked (so the report isn't all complaints)

- **`with` for locks and pool elements** carried the whole concurrency story
  with no lock-leak risk and full `return`/`try` inside — the single best
  ergonomic feature at this scale.
- **`using Pool<Task>` auto-resolution** through `self.tasks` (CC4) let the
  ranking/view helpers read `h.priority`, `h.deps`, `dep.status` as if handles
  were values. No pool threading, no `pool[h]` noise. Reads beautifully.
- **`try … else |e|`** is the right shape for boundary error mapping even where
  S4 makes it repetitive — the one-liner is genuinely nice.
- **Auto-derived Comparable on `Priority`** drove the list sort with zero code;
  the OC1 email override + redeclare was mechanical once I moved to a struct.
- **`T or E` + guard/`??`/`try`** covered the entire error surface without a
  single `if err != nil`-style branch.
- **Field defaults** (where the parser will eventually accept them) made
  `Config {}`, `TaskPatch {}`, `ListFilter {}` clean — the "No Default trait"
  design reads well in practice.
