# Plan

**Measured 2026-08-24, at `60678a7` and again after this branch's fixes.** Every number below
came from running the thing, not from reading the previous version of this file. Re-measure
before planning off it — the last three times this document went wrong, it was because a
state column outlived its evidence.

```
tests/differential.sh      326 green, 23 expected-red, 0 untracked, 0 unexpected-pass
tests/examples_gate.sh     34 ok, 0 failed, 0 pending
tests/projects_gate.sh     21 ok, 0 failed
tests/fmt_roundtrip_gate.sh 431 round-tripped, 567 reformatted, 0 failures
tests/http_api_harness.sh  ok on both backends
cargo test --release --workspace   52 binaries, 0 failures
```

## Where things stand

**The five validation programs all pass.** Sensor processor, grep clone, game loop and text
editor are enrolled in the examples gate with goldens; the HTTP JSON API server serves a CRUD
sequence on both backends through its own harness. ROADMAP's "two of five" table was from
2026-08-08 and is now fixed. That milestone is met — it is no longer the thing to steer by.

**The soundness track is closed.** Spot-checked today, all four rejecting correctly with the
fix shown as code: consuming a value on one branch and using it after the join (E0813),
consuming twice (E0800), `vec["a"]` (E0819), `i64 as u8` (E0817, offering `to`/`wrap`/`clamp`),
and `i32::MAX + 1` panicking at runtime instead of wrapping.

**What is left is a backlog, not a frontier.** 23 files in the suite are registered red: 16
tracked bugs and 7 unbuilt features — down from 24 bugs when this was written, as A1, A3, half
of A4, and two of A2 came off. Nothing is untracked and nothing has silently started passing. Every red file
has a probe and an issue.

## The one thing worth deciding

The day/week/month coverage sweep of 2026-08-19 filed ~40 bugs — one systematic pass over
what a person meets in their first hour, first week, first month. In the five days since,
the work went to Rack/Link native codegen, the `Shared<T, S>` consolidation, and the
annotations + call-information specs. **None of the 40 were fixed** — the first fifteen came
off in this branch.

Those bugs are the first hour of the language. A fixed-size array as a struct field doesn't
compile. `Map.insert` hands back a flag instead of the displaced value. A named-payload enum
variant never matches. A struct with implicit type parameters can't be named at an
instantiation — SYNTAX.md's own `Pair` example doesn't compile. NORTH_STAR says the
instrument is models writing Rask against the compiler and measuring convergence; a model
writing Rask reaches these inside ten minutes, and the examples that pass the gate pass
because they happen to route around them.

So: **stop adding surface until the backlog shrinks.** Every new shape is another place a
bug in these clusters can hide.

---

## Track A — burn down the coverage backlog

Fix by cluster, not by issue number. Several issues share a root cause, and the clusters are
cheaper together than the issues are apart.

**A1. Fixed-size arrays `[T; N]` — #895, #901, #902, #906, #946. Done.** All five were one
half-built feature, and the shape of it is the argument for Track B: the type-string parser
exists three times — in the checker, in mono's layout pass, and again in MIR — and each copy
had its own hole.

- mono's copy had no bracket case at all, so a `[T; N]` struct field became an unknown name
  and was sized at a pointer (#895). The error then blamed a generic instantiation on a
  struct with no type parameters; it says that now only when the field's slot really did
  come from a substituted parameter.
- `ArrayStore` computed the address from `elem_size` and then stored a full word into it, so
  `a[1] = 9` on a `[i32; 4]` blanked `a[2]` (#902). The struct-field path had already solved
  this for #548 — both share one helper now.
- The array literal asked `stored_inline_in_array` — a question about whether the element is
  copied by address — and used the answer for the store's *width*. Integer elements got away
  with it because writing in ascending order paints over each spill; an f32 promoted to f64
  does not, so `[1.5, 2.5, 3.5]` read back as zeroes.
- `push`/`pop`/`clear` resolved through the `Vec` method table and are E0843 now, with the
  rejections in `tests/compile_errors/fixed_array_growth.rk`.
- `[T; W]` for a named const resolved to `[T; 0]` in the checker *and* again in MIR (#906).
  Both read the value from one table on the checker's side now.
- `as_ptr()` on an array dispatched to `rask_vec_as_ptr`, which reads a `RaskVec` header an
  array doesn't have (#946, native). An array local is its own buffer, so its address is the
  answer. The interpreter has no raw-pointer surface at all — that half is #935.

**A2. Naming and inferring generics — #913 done; #904, #905, #915, #916, #961, #968, #970
open.**

#913 was PC1 half-built. `gradual-constraints` says the signature positions are "function
parameters, return types, struct fields, enum payloads" — and only the function half was ever
wired up. So `struct Pair { first: T  second: U }` registered with no type parameters, and
four separate places asked the same wrong question: the checker's `register_struct`/
`register_enum`, mono's `type_param_names` and `instantiate_function_from`, the layout pass's
`build_subst`, and the `generic_decls` filter that decides which declarations are generic at
all. Each read the explicit `<T>` list; each now asks PC1.

The last one was the interesting failure. Without an entry in `generic_decls` the type never
got a per-instantiation layout, so `Pair<i32, string>` kept the *shared* one — where every
parameter is a single word — and its 16-byte string field was written into an 8-byte slot.
`Pair<i32, i64>` worked, which is what made it look like a string bug.

#916 was the same rule missing one layer up. `extend Wrapper<T>` writes its parameters in the
header; `extend Wrapper` doesn't, and the owning declaration is where they live. Reading only
the header left such a method with nothing to bind, so it never got a per-receiver body: one
shared `Wrapper_get` returning a word served `Wrapper<f64>` (which read the double's bits as
an integer) and `Wrapper<string>` (which segfaulted on a 16-byte value in a one-word return
slot) alike. It falls back to the type's own parameter list now, and each instantiation gets
`Wrapper_get$f64`, `Wrapper_get$string`.

That left the probe 6/7, and the last one was a different defect: an `f32` in a generic field
read back as 0. The struct arm's `is_type_param` branch kept whatever type the caller asked
for, which is right for an integer and wrong for a float — a float in a word-wide slot lives
there as an `f64` (the convention #629 settled), so honouring an `F32` request loaded the
double's zero low half. It reads at the slot's width and lets the existing narrowing tail
demote (#972). Probe is 7/7 on both now.

Then a sweep of every position a float can occupy in a word slot, because the same question —
promoted or demoted? — had now been answered wrongly at three separate sites. Eight of nine
agree: generic struct field, optional, result, `Vec` element, `Map` value, fixed-array
element, tuple element, and a value through a generic function. The ninth was worse than the one
that started it, and wasn't what it looked like. Filed as #973, then found and fixed:

A `match` used as an expression allocates its result local *before* any arm has been lowered,
at a placeholder word, and nothing retypes it once the arms report. Assigning a float arm into
an `i64` local converts rather than reinterprets, so `match n { 1 => 2.5, _ => 0.0 }` was `2` —
a rounding bug to look at, a miscompile in fact. Six sites in `match_lower.rs`. No enum
involved: `if/else` was right all along, which is exactly why it read as an enum-payload
problem, since every repro reached its payload through a `match`. Using the binding inside the
arm was always correct, and that was the clue to follow first.

Underneath it, the smaller one the issue had guessed at: the enum arm of `lower_field_access`
never set the load width, so an f32 payload was read four bytes wide out of a slot holding a
promoted double. f64 was right by coincidence.

`tests/suite/t_week_float_slots.rk` gates all nine positions on both backends now.
`t_week_enums.rk` stayed green through the whole thing because none of its variants carry a
float — an area file only gates the shapes it happens to use.

Still open: a declared type parameter loses to a stdlib type of the same name (#915). An
inferred signature is pinned to `i32` by a literal in its body, so the spec's own
`func double(x) { x * 2 }` won't take an `f64` (#904, #905).

**A3. Optional chains — #917, #938, #939 done; #909 open.** All three were one decision made
too early and one type shared by two shapes.

`?.` onto an optional-typed field: both runtimes flatten, the checker said `T??`. The flatten
test existed and read the field's type at the access — where it is still the fresh variable a
deferred `HasField` will fill in, so "is it already an option?" always answered no. It is a
deferred constraint now, `OptionalChain`, settled once the field's type is; same shape as the
`Index` constraint beside it and for the same reason (#938). #917 went green with it.

The E0308 that used to result suggested `try` and talked about "the error" — on an optional,
where there is no error (#939). An optional *is* `Result { ok: T, err: None }` underneath, so
the Result branch of the diagnostic caught it too. It now says what an absent value can do,
as code:

```
= fix: say what an absent value should do:
      x ?? default      // supply one
      x!                // assert it's there, panic if not
      if x? as v { … }  // handle both
```

The corrected flattening turned one probe's assertions into compile errors — correctly.
`a.inner?.v! ?? "X"` was written against the buggy `T??`, where `!` peeled one of two layers
and left something `??` could still take. E0831 rejects it now, and says why. The test asserted
the bug; it asserts the shape instead.

#909 closed A3. `auto_wrap_for_annotation` gave a binding's value the shape its annotation
asked for, but only ever looked at the outer type — so `let xs: Vec<i64?> = [1, none, 3]`
bound the 1 and the 3 bare, and `xs[0]!` failed with "! requires Option or Result, got i64"
while native handled it. It descends into a sequence annotation now (`Vec<T>`, `[T; N]`,
`[]T`); the depth count already there stops a `none` element double-wrapping. A `Map`'s values
want the same and are their own case — two type arguments, and the value isn't a `Value::Vec`.

**A4. Enum payloads — #910, #911 done; #922 open.** Named-payload variants were unreliable
on both backends for two unrelated reasons, and both were one missing case.

- Native built the switch case from the arm's *position* whenever it couldn't resolve a tag,
  and `resolve_pattern_tag` only answers for a qualified `Enum.Variant` — which is never how
  a match arm is written. The positional-payload arm right above it has had the
  scrutinee-based fallback all along, which is exactly why `N(i64)` worked and `N { v }` did
  not (#911).
- The interpreter built `A4.N { v: 5 }` as a *struct* named `A4.N`, because a named payload
  shares the struct literal's syntax. Every `N { v }` arm then compared a struct name against
  a variant name and fell through to the wildcard (#910). It builds a `Value::Enum` with the
  payload in declaration order now, so the value is the right kind everywhere and not just
  inside `match`.

#922 — an enum payload isn't a C4 slot, so `Node.Branch([1, 2, 3])` is rejected and the error
has expected/found swapped — is a different defect in the same area and still open.

**A5. The singles.** #899 `mutate` on a primitive parameter drops the write (and
`mem.parameters` contradicts itself about whether it should — settle the spec first, #881).
#903, #907, #919, #923, #924, #928, #929, #930, #931, #932, #933, #934, #935.

Exit condition per cluster: the probe file leaves `tests/known_divergences.txt` and rejoins
the green gate.

## Track B — #725, measured, and smaller than it looked

This track was ordered second on the strength of two numbers in #725: 41 `i64_fallback` sites
that "give up and guess" (49 today, drifting the wrong way), and 3–7% of MIR's type lookups
coming back empty. Both have now been measured, and neither means what it was taken to mean.

**B1. The missing lookups are all auto-derived `compare` bodies.** Stamping the expression
form during lowering, then the identifier's name, then the enclosing function, walks it down
to one answer:

```
×89: <outside> (no name)
×6:  Ident `JsonParser_compare :: other`     ×6: Ident `JsonParser_compare :: self`
×4:  Ident `Pt_compare :: other`             ×4: Ident `Pt_compare :: self`
×4:  Field `Path_compare :: value`
```

`self` and `other` in equal pairs is a binary method's shape, and every named function is a
`<Type>_compare`. `auto_derive_traits` (`declarations.rs:830`) registers a `MethodSig` and no
body; the body is synthesized after checking, so the checker never visits it and none of its
nodes get an entry. A user's own derived compare misses exactly like the stdlib's.

It is fixed overhead per derived type, not a scaling gap. A ten-line program with one struct
reports 1105 lookups and 161 missing — **14.6%**, worse than any real program, because the
derived bodies are a constant and there is no user code to dilute them. The checker's coverage
of ordinary expressions is fine.

**B2. Not one of the 49 fallback sites is reachable.** They are fatal by default, so a program
that reaches one fails to compile. Run with `RASK_ALLOW_TYPE_FALLBACK=1` and
`RASK_TRACE_TYPE_FALLBACK=1` so a hit is recorded instead:

```
36 examples          0 hits
328 suite files      0 hits   (the 21 that fail are the registered expected-red ones)
```

Zero. `fallback.rs`'s own doc comment predicted this — "sites that no program reaches don't
need a fallback at all; this module is how you find out which those are" — and the answer is
all of them. The growth from 41 to 49 is untidy, not dangerous: it is dead scaffolding
accumulating, not guessing spreading.

**So #725's three mechanisms are all resolved or inert.** `ambiguous_method_prefix`, the one
that could return a *wrong* answer rather than none, was deleted some time ago. The fallback
sites are unreachable. The missing lookups are one category the checker never visits by design.
Nothing here is causing a miscompile today.

**What is left, and it is cleanup, not a track:**

- Delete the unreachable fallback sites and make the unknown-type case say plainly that the
  type is unknown, per that doc comment.
- Give derive bodies node types — synthesize them before checking, or record as they are
  built — so the coverage number measures something.
- **B3. One place decides how wide a slot is — done.** Four sites in this branch disagreed
  with the rule #629 settled: the array element store (#902), the generic struct field read
  (#972), the `match` result local and the enum payload read (both #973). All four were quiet
  wrong answers rather than crashes, which is what let them accumulate.

  `rask_mono::abi` already held the wrapper-payload half of the rule and said why — *"MIR
  widens a value to this on the way in, codegen takes it apart by this on the way out, and
  neither gets its own opinion."* It just didn't cover the other slot kinds. `slot_scalar_bytes`
  generalizes it — a float occupies its slot whole (`f64` in a word, `f32` in four bytes), an
  integer takes the slot it is given — with unit tests on the rule itself and the four-site
  table in its doc comment. The narrow store, the struct field read and the enum payload read
  consult it instead of deciding locally.

  It is not yet a chokepoint in the strong sense: a new site can still call `load`/`store`
  directly without asking. Making that impossible means routing emission through the helper
  too, which is a larger change than this one. `t_week_float_slots.rk` is the backstop in the
  meantime — nine positions, both backends.

**Reorder.** Track B was placed above the rest of Track A because it was believed to generate
those bugs. The evidence for that is gone — the fallback machinery generates nothing, because
nothing reaches it. B3 stands on its own evidence; the rest of Track B is tidying. Track A's
remaining singles come first.

## Track C — hold the new surface

Annotations and call information are proposed specs with fresh implementations. Rack/Link
just landed natively and #908 sequences retiring Pool/Handle behind it. None of that is wrong
work; it is the wrong week for it. Revisit when Track A is under ten files.

## Track D — what is genuinely not built

The 7 pending-feature probes, in the order they block real programs:

| Probe | Feature | Issue |
|---|---|---|
| `t_week_collection_stubs.rk` | 11 declared Vec/Map methods with no implementation on either backend | #912 |
| `t_week_range_adapters.rk` | a range has no terminals or adapters — no `to_vec`, `sum`, `map` | #920 |
| `p08_sequence.rk` | the sequence protocol; user types can't be iterated at all | — |
| `t_month_atomics.rk` | `Atomic<T>` has no operations on any spelling | #927 |
| `p10_binary.rk` | `@binary` parse/build — native has none of it | — |
| `p09_simd.rk` | vector types are six names and no instructions | — |
| `p12_rack_link_churn.rk` | not a rack gap: native `filter` on a Vec reached through a field | #866 |

Ranges and the collection stubs are declared API that doesn't exist, which is worse than a
missing feature — the signature promises and the call fails. Those two first. The sequence
protocol is the architectural one and range adapters route through it, so it is the real
prerequisite rather than a parallel track.

## Beyond this

ROADMAP Phase 2 (stdlib breadth), Phase 3 (runtime trait dispatch, cross-compilation) and
Phase 4 (incremental compilation) stand as written. They were waiting on the validation
programs, which no longer block them — but Track A does.
