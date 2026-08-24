# Plan

**Measured 2026-08-24 at `60678a7`.** Every number below came from running the thing, not
from reading the previous version of this file. Re-measure before planning off it — the last
three times this document went wrong, it was because a state column outlived its evidence.

```
tests/differential.sh      317 green, 31 expected-red, 0 untracked, 0 unexpected-pass
tests/examples_gate.sh     34 ok, 0 failed, 0 pending
tests/projects_gate.sh     21 ok, 0 failed
tests/fmt_roundtrip_gate.sh 430 round-tripped, 566 reformatted, 0 failures
tests/http_api_harness.sh  ok on both backends
cargo test --release --workspace   green
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

**What is left is a backlog, not a frontier.** 31 files in the suite are registered red: 24
tracked bugs and 7 unbuilt features. Nothing is untracked and nothing has silently started
passing. Every red file has a probe and an issue.

## The one thing worth deciding

The day/week/month coverage sweep of 2026-08-19 filed ~40 bugs — one systematic pass over
what a person meets in their first hour, first week, first month. In the five days since,
the work went to Rack/Link native codegen, the `Shared<T, S>` consolidation, and the
annotations + call-information specs. **None of the 40 were fixed.**

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

**A1. Fixed-size arrays `[T; N]` — #895, #901, #902, #906, #946.** The biggest cluster and
the most day-one of them. `[T; N]` isn't in the layout pass, so a struct field holding one is
sized at a pointer:

```
$ rask run arr.rk
warning: unknown type '[i32; 4]' in layout, defaulting to pointer size (8, 8)
error: MIR lowering 'main': field `cells` holds 16 bytes but its slot is 8 — this
       instantiation is using the shared layout of a generic type …
```

`Grid` is not generic. The diagnostic is guessing too, and it sends the reader somewhere
there is nothing to find — fix the message with the layout. `ArrayStore` in
`rask-codegen/src/builder.rs:849` is the same family: it computes the address from
`elem_size` and then stores a full word into it, so writing one element of a sub-word array
clobbers its neighbours.

**A2. Naming and inferring generics — #904, #905, #913, #915, #916, #961, #968, #970.** A
declared type parameter loses to a stdlib type of the same name. A method returning the
receiver's own parameter isn't monomorphized, so floats come back as bit patterns. An
inferred signature is pinned to `i32` by a literal in its body, so the spec's own
`func double(x) { x * 2 }` won't take an `f64`.

**A3. Optional chains — #909, #917, #938, #939.** `?.` onto an optional-typed field: both
runtimes flatten `T?`, the checker says `T??`, so `chain ?? "default"` won't compile. The
E0308 that results suggests `try` and talks about "the error" — there is no error.

**A4. Enum payloads — #910, #911, #922.** A named-payload variant never matches on the
interpreter, and native matches by the arm's position rather than the variant's tag — wrong
arm, or a segfault.

**A5. The singles.** #899 `mutate` on a primitive parameter drops the write (and
`mem.parameters` contradicts itself about whether it should — settle the spec first, #881).
#903, #907, #919, #923, #924, #928, #929, #930, #931, #932, #933, #934, #935.

Exit condition per cluster: the probe file leaves `tests/known_divergences.txt` and rejoins
the green gate.

## Track B — #725, the thing that generates Track A

MIR re-derives types the checker already worked out. When #725 was filed there were 41 sites
that gave up and guessed `i64`; **there are 49 today.** It is drifting the wrong way while
its symptoms get patched one miscompile at a time.

Two of the three mechanisms named in that issue are gone — `ambiguous_method_prefix`, the
name→type table that could return a *wrong* answer, no longer exists, and `node_types` is now
written at a choke point rather than opportunistically. The instrumentation asked for in step 1
is built. Run it on real programs:

```
text_editor        4178 lookups: 96.4% resolved, 0.0% open, 3.6% missing
game_loop          2134 lookups: 92.8% resolved, 0.0% open, 7.2% missing
markdown_renderer  6076 lookups: 97.4% resolved, 0.0% open, 2.6% missing
package_manager   10957 lookups: 96.9% resolved, 0.1% open, 3.0% missing
http_api_server    5477 lookups: 95.6% resolved, 0.3% open, 4.1% missing
```

That settles which of the two failures matters: **missing, not open.** Inference converges;
the checker simply never records a type for some nodes. So the fix is at the checker, not in
the solver.

Next step, and it is small: the miss counter takes a `NodeId` and nothing else, so it can say
*how many* were missing but not *which kinds*. Record the AST kind alongside the miss, rank
them, and the 3–7% becomes a short list of expression kinds to fix at the choke point. Then
delete fallback sites as their inputs become reliable. That kills the class instead of the
instance.

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
