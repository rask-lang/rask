# Plan

**Measured 2026-08-26, on all seven lane branches merged together.** Every number below came
from running the thing, not from reading the previous version of this file. Re-measure before
planning off it — the last three times this document went wrong, it was because a state column
outlived its evidence.

```
tests/differential.sh      342 green, 15 expected-red, 0 untracked, 0 unexpected-pass, 0 misfiled
tests/examples_gate.sh     34 ok, 0 failed, 0 pending
tests/projects_gate.sh     21 ok, 0 failed
tests/prototypes_gate.sh   13 agree, 0 untracked
tests/fmt_roundtrip_gate.sh 438 round-tripped, 577 reformatted, 0 failures
tests/http_api_harness.sh  ok on both backends
tests/agentbench_gate.sh   17 green, 1 quarantined, 0 broken
cargo test --release --workspace   0 failures
```

## The seven lanes, merged

Seven sessions ran in parallel, partitioned by crate: #987 codegen, #988 interpreter, #989
namespace rules, #991 diagnostics, #995 comptime reflection, #1003 agent benchmark, #1004
panics. Each is green against its own base.

Merged together they need seven fixups, every one a lane meeting something another lane wrote.
Three were caught by a guard one lane had written firing on another's change, which is the
argument for writing them:

- **#989's stub-set guard** caught `reflect` in two lists at once. #989 kept it out because
  parsing it broke monomorphizing `reflect.fields<T>()` through a generic; #995 fixed that
  cause and put it in.
- **#991's error-code audit** caught #991 and #1004 both allocating E0844 — main topped out at
  E0843, so both picked the same next number. A HashMap keeps the last, so one of the two
  errors had no explanation. `try`-in-`ensure` is E0845 now.
- **The fmt gate** caught a formatter bug the corpus had never reached: `rask fmt` collapsed
  any one-statement `ensure { … }` to the braceless form, which parses one *expression*, so
  `ensure { let n = try s.close() }` came back out as unparseable source. #1004's E0845
  fixture is the first file with a lone `let` in an ensure body.

The other four nothing caught:

- **#989 was branched from `125d039` and its CI had never seen the files its own rule
  rejects.** A branch's green CI says nothing about a base it never merged.
- **Thirteen files across five lanes needed imports they never got** — suite probes, a
  registered-red file, panic and `staged()` fixtures, a benchmark reference. Not carelessness:
  a rule applied to a snapshot of the tree cannot reach files written after the snapshot, and
  no single branch's CI sees the combination. Three of the thirteen were the nastier kind, where
  the file still failed and so still looked fine: `t_day_const_string_array.rk` is registered
  red, `staged_misuse.rk` is a compile-error fixture, and `torn_lock_update.rk` asserts a
  warning *count* — a check that dies on a missing import emits zero warnings, so that
  assertion failed for a reason unrelated to what it tests. All three were failing on the
  import instead of on the thing they exist to pin.
- **One registered-red file rotted.** `t_day_const_string_array.rk` is red for #1000, so the
  gate expected it to fail — and stopped looking when it started failing at the *check* step
  for a missing import. The bug it documents was not being exercised. Fixed, and the gate now
  holds every red file to a `(backend phase)` claim so it cannot recur (#1005).
- **The benchmark would have measured our own stale documentation.** It hands a model
  LANGUAGE_GUIDE.md as normative and scores whether the reply compiles; the guide never said
  stdlib names need importing. A low solve rate would have read as a language-usability
  result. The guide says it now, in the Modules section and as common mistake 15.

Suggested merge order: #988, #991, #995, #1003, #1004 in any order, then #989, then the
integration branch. #987 is already in main.

**The lanes are still moving.** Between the first integration pass and the second, five of
seven branches gained commits and one opened its PR; between the second and third, five gained
commits again and #987 merged. Two of those rounds changed the answer — the panics lane
replaced the `is_immutable` flag this branch had merged with a `reassigned_names` set, and
fixed the `ensure`-braces formatter bug independently. So this branch is a snapshot, not a
standing result: re-merge at current heads and re-run before trusting the numbers.

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

#915 was name resolution having no notion of a type parameter being in scope.
`parse_type_string` ended at `types.lookup(name)`, so `struct Holder<Output>` and
`func first<Output>(…)` both meant the stdlib's `os.Output`, and every use of the parameter
mismatched against a type nobody wrote. Single letters never reach that lookup (PC1), so this
only ever bit the descriptive names — `Output`, `Item`, `Error` — which are the ones likely to
collide in the first place.

`TypeTable` carries a scoped parameter set now, consulted before the lookup, pushed around a
declaration's field types and around a function's signature and body. The third place was the
one that took two attempts: a callee's signature is parsed at the *call site*, so the scope in
effect is the caller's — the callee's own parameters have to be pushed there too, out of the
registration pass's record.

**#904/#905 are a missing feature, not a bug — attempted and reverted.** The trigger is an
unresolved receiver, not two call sites: one float call is enough, and every neighbouring form
works.

| Program | Result |
|---|---|
| `func double(x) { x * 2 }`, int calls only | 42 ✓ |
| `func annotated(x: f64) -> f64 { x * 2 }` | 3 ✓ |
| `let x: f64 = 1.5` then `x * 2` | 3 ✓ |
| `func double<T: Numeric>(x: T) -> T { x * 2 }`, int *and* float | 42, 3 ✓ |
| `func double(x) { x * 2 }`, one float call | **E0371** |

`resolve.rs` already ties a *literal receiver* to a settled numeric argument, so the obvious
fix is the mirror — open receiver, literal argument, tie them. It compiles, and `double(1.5)`
returns **2**: the literal's `i32` default flows into the parameter rather than the other way,
and the float is truncated at the call. A silent wrong answer where there was a compile error
is strictly worse, so it is reverted.

The fourth row is the tell. The explicitly-generic form works completely, so the fix is to
*produce* it: `func double(x) { x * 2 }` must infer `<T: Numeric>(x: T) -> T`, which
`type.gradual/IN3` already specifies and `gradual-constraints.md` gives as its first example.
Any fix that settles the parameter to one concrete type is either wrong or right for one call
only. That is generalization — a feature, and the same one #905 needs.

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
