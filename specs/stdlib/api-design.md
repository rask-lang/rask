<!-- id: std.api -->
<!-- status: decided -->
<!-- summary: Stdlib API rules — small surface, guessable names, one powerful function over many specific, no Rust legacy by reflex, composability through shared protocols -->
<!-- depends: canonical-patterns.md -->

# Stdlib API Design

The stdlib is where language size actually hits people. Nobody reads the grammar; everybody asks "what function do I call for this" fifty times a day. These rules keep that question cheap to answer — and usually unnecessary to ask.

## Rules

| Rule | Description |
|------|-------------|
| **SD1: One screen per module** | A module's public surface fits on one screen — roughly 20 items. That's a budget, not a guideline: adding past it means removing or merging something, or arguing why this module is the exception. `rask api <module>` is the measuring stick |
| **SD2: Powerful over specific** | One function with orthogonal parameters beats a family of names. `read(path)` with options beats `read_text`/`read_lines`/`read_binary` siblings. Name-variants are reserved for exactly two axes: fallibility pairs (`push`/`try_push` — `std.collections/C2`) and cost pairs per the naming table (`as_*`/`to_*`/`into_*`) |
| **SD3: The guess test** | Before designing a function, write the call site you'd *guess* — the line you'd type before opening any docs. If the guess is reasonable and the stdlib differs, the stdlib is wrong, not the guess. Names come from [canonical-patterns.md](../canonical-patterns.md)'s vocabulary so guesses transfer between modules |
| **SD4: No Rust legacy by reflex** | Every name and shape is justified from how the Rask call site reads, never from what `std` calls it. Rask has `T or E`, `T?`, `Heap`, `Shared<T, S>` — so `Result`, `Option`, `Box`, `Rc`, `RefCell`, `Arc<Mutex<T>>` never appear, and neither do their method idioms (`unwrap`, `expect`, `ok_or`, `and_then`). `Vec`/`Map` survive because they read right in Rask, not because Rust has them |
| **SD5: One way** | No convenience aliases, no two spellings for one operation (`mem.atomics/GA1` is the precedent). If two functions do the same thing, one of them is deprecated the day the second lands |
| **SD7: Weakest bound** | A generic function asks for the least trait that lets one body serve every `T`. Generic when the algorithm doesn't care which type it got; concrete when a type parameter would only absorb a conversion. A conversion belongs at the call site, written, with its policy visible |
| **SD8: Canonical protocols** | The stdlib speaks a closed set of protocols: `Sequence`, `Comparable`, `Equal`, `Hashable`, `Displayable`, `Debug`, `Reader`/`Writer`, `Encode`/`Decode`, and the operator traits (`type.operator-resolution`). No module invents a parallel interface for something this set covers. Growing the set is a change to this spec, not a module-level decision |
| **SD9: Laws, not just signatures** | Every canonical protocol states its contract in its spec (`Equal` is reflexive and symmetric, `Comparable` is a total order, `Sequence` yields each element once). Conforming means meeting the laws. A signature match without the laws is how independently-written pieces compose into bugs |

## One word per question (SD6)

| Rule | Description |
|------|-------------|
| **SD6: One word per question** | A question gets the same word everywhere it's asked. Membership is `contains` on `Vec`, `Map`, `Set`, `string`, `Rack`, `Pool` and `Headers` — not `contains_key` on one of them. Size is `len`. Reading a stored value through a closure is `read`. Consuming a wrapper is `take`. The compounding is the point: the vocabulary is what makes `SD3`'s guess transfer between modules, so a second word for a question already answered costs more than the module it lives in |

A worked pass over the whole stdlib, and what it turned up:

| Was | Is | Why |
|-----|----|-----|
| `Map.contains_key(k)` | `Map.contains(k)` | Rust needs `contains_key` because a Rust map's `contains` would have to say which half it means. Every single-argument method on Rask's `Map` takes a key — `get`, `remove`, `read`, `modify` — so one more that does is no ambiguity. Asking about a value is `m.any(\|e\| e.1 == v)` |
| `Headers.has(name)` | `Headers.contains(name)` | Third spelling of the same question |
| `Wide.read()` | `Wide.to_vec()` | Every other `read` in the stdlib borrows through a closure. This one runs the plan and builds a `Vec`, which is what `type.sequence/SEQ31` says to name it |
| `Shared.into_inner()`, `Atomic.into_inner()` | `take()` | `into_inner` was `Cell`'s name, carried forward from a type Rask no longer has. `take` is the parameter mode the receiver already uses |
| `fs.create(path)` | `fs.create_file(path)` | Bare `create` in a filesystem module doesn't say which of `create_dir` and it you meant |
| `Command.env(k, v)` | `Command.set_env(k, v)` | `os.env(name)` in the same module *reads* one |
| `string.from_utf8_unchecked` | *(deleted)* | Marked `unsafe`, named `_unchecked`, documented "without validation" — and it validated and panicked. `from_utf8(bytes)!` is the same thing, spelled honestly |
| `Duration.as_seconds_f32() -> f64` | *(deleted)* | The name said `f32` and every layer down to codegen returned `f64`. `as_seconds_f64` already covers it |
| `Duration.from_millis`, `from_nanos` | *(deleted)* | Their own doc comments said "(alias)" — of `millis` and `nanos` |
| `http.send_request(m, url, body, hdrs)` | `http.request(Request)` | The four-argument one was `request` with the `Request` spelled out. `Request.with_headers` closes the gap that kept it alive |
| `Pool.with_valid`, `with_valid_mut` | `Pool.read`, `Pool.modify` | Four names for two operations |
| `Vec.count()`, `Map.count()`, `Set.count()` | `len()` | A container knows its length; `count` walks. Offering both made the second a slower spelling of the first — a sequence keeps `count` because it genuinely has to walk, and that is the whole difference between the two words |

## Why SD1 is the load-bearing rule

The day-to-day cost of a big stdlib isn't learning it — it's *re-scanning* it. Every "which function do I want" pause is a trip to the docs, and a module with 60 entries makes that trip mandatory; a module with 15 makes it skippable, because the answer is visible in one `rask api` call or one autocomplete popup. Go's stdlib is loved for exactly this: each package holds a dozen things you can keep in your head. Batteries included means every battery *slot* is filled — not that every slot holds six batteries.

SD2 is how SD1 stays possible: surface grows by parameter, not by name. A parameter is discoverable at the one function you already found; a sibling function is another entry you had to know existed.

### What the budget is actually being spent on

`Vec` was measured against SD1 and came to 60 items — three times the budget.
Applying SD2 to it buys almost nothing, and the reason is worth writing down
before someone else spends an afternoon on it: those 60 are not a pile of
near-synonyms. They are roughly 16 sequence adapters, 12 methods for bounded
capacity, about 20 core operations, and a tail. The name-families SD2 actually
targets — `shrink_to_fit`/`shrink_to`, `sort`/`sort_by`, `min`/`min_by` — are
worth one entry each. A stdlib method can carry a default argument now, which
is what those collapses need; `shrink(to: usize = 0)` is the first.

So the budget is being blown by **structure**, not by naming, and two things
decide whether it can ever be met:

- **The adapters.** `type.sequence/SEQ48` says a collection is its own chain
  head, which is right — it's what removes Rust's `.iter()`. It used to be
  implemented by hand-copying each adapter onto each container, and copies rot:
  `Vec` carried 16 of `Sequence`'s 21 and was missing `take_while` beside a
  `take` that worked. They are generated from `extend Sequence<T>` now — a type
  declares `as_sequence` and gets the rest — so `Vec` declares 14 fewer and the
  gaps are gone. `Map` and `Set` declare one too, so they have the surface for
  the first time — a map's element is the `(key, value)` pair.
- **Whether a container's inherited adapters count against its budget.** If
  they do, no collection can ever meet SD1 while SEQ48 holds, because SEQ48
  requires them. The count that means something is what the container *adds*,
  and generated forwarders are the mechanism, not the surface.

SD2 also needs a mechanism the stdlib doesn't have yet: a defaulted parameter
on a stdlib method is parsed and dropped (rask-lang/rask#1276), so
`sort(by: … ? = none)` doesn't compile. Until that lands, a collapse can only
go as far as a mandatory parameter — which is why `shrink(to:)` takes its
argument.

## The guess test, operationally (SD3)

When speccing a module, write the *call sites first* — a dozen lines of realistic use, before any signature exists — and have someone (or a second pass, cold) guess what each call does and what its variants would be called. Three outcomes:

- Guess matches design → done.
- Guess is wrong because the operation is genuinely subtle → keep the design, and the doc comment leads with the distinction.
- Guess is reasonable and differs → **rename toward the guess.** The guess is data about every future user's first attempt.

This is `CLAUDE.md`'s "sketch how the call site reads first" made into a gate rather than advice.

## Composability (SD7–SD9)

The goal is Julia's property: two pieces of code that have never heard of each other work together, because the algorithm asked for the least it needed and the type answered. A user's number type flows through generic stdlib math; a user's container flows through everything written against `Sequence`. Rask gets this statically: the "multiple dispatch" question was settled in `type.operator-resolution`'s rationale — choosing a method from several argument types and third-party conformances (#312) are compile-time features Rask takes; the runtime open-set version is the part rejected. Both halves are in: generic trait parameters (#1164), associated types (#1165), and the operator resolution built on them.

SD7 delivers the generics half, SD8 the conventions half. They only work together: a weakest-bound function over a protocol nobody shares composes with nothing.

### The litmus: Raido's fixed-point

Raido's 32.32 fixed-point number is the in-house test that the property exists. Conforming to the operator traits and `Comparable`, this works with **zero stdlib changes** — it runs on both backends as `tests/suite/t_fixed_point_litmus.rk`:

<!-- test: skip -->
```rask
let readings: Vec<Fixed> = sensor.window()
let smallest = min(readings[0], readings[1])   // std.math/G1, T: Comparable
mut total = Fixed.zero()
for r in readings {
    total = total + r                          // operator trait, not a Fixed method
    if r > alarm_level { alert(r) }            // Comparable again
}
```

If any line needs a stdlib edit, a cast, or a `Fixed`-specific sibling function, SD7 or SD8 was violated somewhere. Re-run this check whenever a numeric or container API lands, and whenever the stdlib gains a numeric algorithm (a `sum`, a `clamp`): each must be born generic or not at all.

The half that needed operator resolution is `3 * reading` — a scalar on the *left*. `reading * 3` always worked, because the left operand was the one that got to answer.

### What SD9 buys

Julia composes so well partly because nothing can reject you — and it pays in combinations that run and are wrong (the ecosystem-wide breakage when arrays stopped being 1-based is the canonical case). Checked conformance plus written laws is the version of composability where the combinations that compile are the ones that work. That bill is worth paying; `type.operator-resolution`'s rationale records the same trade from the operator side.

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Module genuinely needs > one screen (e.g. `math`) | SD1 | Split into submodules with one-screen surfaces, or document the exception in the module spec's rationale |
| Fallibility pair (`push`/`try_push`) | SD2 | Allowed — the pair is the pattern (`std.collections/C2`), not surface growth |
| Cost-family conversions (`as_`/`to_`/`into_`) | SD2 | Allowed — the prefixes are one concept, learned once ([canonical-patterns](../canonical-patterns.md)) |
| Callers would routinely discard the error | SD3 | The API is absence-shaped — return `T?`, not `T or E`. A probe's failure is a non-answer, and an error branch nobody reads is ceremony at every call site ([canonical-patterns](../canonical-patterns.md)) |
| A Rust name really is the right one | SD4 | Fine — justified from the Rask side in the module spec's rationale, not from precedent |
| A module needs an interface the canonical set almost covers | SD8 | Extend the set here (a change to this spec, argued in its rationale) — never a one-module parallel protocol |
| A generic bound would hide a fallible conversion (e.g. integers of different width) | SD7 | Stay concrete. The seam is fixed by choosing types, not by a type parameter that swallows the policy |
| Deprecating toward one spelling | SD5 | The loser gets a lint pointing at the winner for one release, then removal (pre-1.0: immediate removal) |

---

## Appendix (non-normative)

### Rationale

**SD1 (one screen):** The alternative — "add whatever's useful" — is how every stdlib grows into a place where finding the function costs more than writing it yourself. A hard budget forces the merge/remove conversation at design time, when it's cheap, instead of at deprecation time, when it breaks people.

**SD3 (guess test):** Guessability compounds: a stdlib where the first guess works teaches users to guess, which makes every module cheaper to use than its docs. A stdlib that punishes guessing teaches doc-checking, and then the size of the docs *is* the size of the language. This is the API-level version of the reading-set budget ([DAY_ONE.md](../DAY_ONE.md), `spec.metrics` RS).

**SD4 (Rust legacy):** Rask's early stdlib sketches leaned on Rust names because that's what the hands knew. Some survived scrutiny (`Vec`, `Map`), most didn't (`Result` → `T or E`). The rule exists so the scrutiny happens per-name instead of per-habit.

**SD7/SD8 (composability):** I want Julia's composability, by generics and by conventions. The halves only work together. Generics without agreed protocols puts the flexibility at parameter positions (`impl AsRef`-style bounds), where it hides conversions and turns errors into trait-bound walls. Protocols without generics means writing the same loop per type. Both halves, statically checked, is the target. `min<T: Comparable>` with no `math.min` and no `.min()` method is the existing model case.

**SD9 (laws):** Composability means combinations nobody tested. The only way those are correct is if each side conforms to a stated contract rather than a shape. Laws live in the protocol's own spec and are cited from conformance docs; a comptime-checkable subset can come later without changing what the rule asks.

### See Also

- [canonical-patterns.md](../canonical-patterns.md) — the naming vocabulary (`is_*`, `to_*`, `with_*`, `try_*`)
- [DAY_ONE.md](../DAY_ONE.md) — the language-level reading-set budget this mirrors
- [README.md](README.md) — module inventory these rules govern
