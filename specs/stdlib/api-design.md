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
| **SD6: Weakest bound** | A generic function asks for the least trait that lets one body serve every `T`. Generic when the algorithm doesn't care which type it got; concrete when a type parameter would only absorb a conversion. A conversion belongs at the call site, written, with its policy visible |
| **SD7: Canonical protocols** | The stdlib speaks a closed set of protocols: `Sequence`, `Comparable`, `Equal`, `Hashable`, `Reader`/`Writer`, `Encode`/`Decode`, and the operator traits once `type.operator-resolution` lands. No module invents a parallel interface for something this set covers. Growing the set is a change to this spec, not a module-level decision |
| **SD8: Laws, not just signatures** | Every canonical protocol states its contract in its spec (`Equal` is reflexive and symmetric, `Comparable` is a total order, `Sequence` yields each element once). Conforming means meeting the laws. A signature match without the laws is how independently-written pieces compose into bugs |

## Why SD1 is the load-bearing rule

The day-to-day cost of a big stdlib isn't learning it — it's *re-scanning* it. Every "which function do I want" pause is a trip to the docs, and a module with 60 entries makes that trip mandatory; a module with 15 makes it skippable, because the answer is visible in one `rask api` call or one autocomplete popup. Go's stdlib is loved for exactly this: each package holds a dozen things you can keep in your head. Batteries included means every battery *slot* is filled — not that every slot holds six batteries.

SD2 is how SD1 stays possible: surface grows by parameter, not by name. A parameter is discoverable at the one function you already found; a sibling function is another entry you had to know existed.

## The guess test, operationally (SD3)

When speccing a module, write the *call sites first* — a dozen lines of realistic use, before any signature exists — and have someone (or a second pass, cold) guess what each call does and what its variants would be called. Three outcomes:

- Guess matches design → done.
- Guess is wrong because the operation is genuinely subtle → keep the design, and the doc comment leads with the distinction.
- Guess is reasonable and differs → **rename toward the guess.** The guess is data about every future user's first attempt.

This is `CLAUDE.md`'s "sketch how the call site reads first" made into a gate rather than advice.

## Composability (SD6–SD8)

The goal is Julia's property: two pieces of code that have never heard of each other work together, because the algorithm asked for the least it needed and the type answered. A user's number type flows through generic stdlib math; a user's container flows through everything written against `Sequence`. Rask gets this statically: the "multiple dispatch" question was already settled in `type.operator-resolution`'s rationale — choosing a method from several argument types and third-party conformances (#312) are compile-time features Rask takes; the runtime open-set version is the part rejected. Blocked today on generic trait parameters (#1164) and associated types (#1165).

SD6 delivers the generics half, SD7 the conventions half. They only work together: a weakest-bound function over a protocol nobody shares composes with nothing.

### The litmus: Raido's fixed-point

Raido's 32.32 fixed-point number is the in-house test that the property exists. When it conforms to the operator traits and `Comparable`, this must work with **zero stdlib changes**:

<!-- test: skip -->
```rask
let readings: Vec<Fixed> = sensor.window()
let smallest = min(readings[0], readings[1])
let total = readings.sum()
let mid = clamp(estimate, low, high)
```

If any of those needs a stdlib edit, a cast, or a `Fixed`-specific sibling function, SD6 or SD7 was violated somewhere. Re-run this check whenever a numeric or container API lands.

### What SD8 buys

Julia composes so well partly because nothing can reject you — and it pays in combinations that run and are wrong (the ecosystem-wide breakage when arrays stopped being 1-based is the canonical case). Checked conformance plus written laws is the version of composability where the combinations that compile are the ones that work. That bill is worth paying; `type.operator-resolution`'s rationale records the same trade from the operator side.

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Module genuinely needs > one screen (e.g. `math`) | SD1 | Split into submodules with one-screen surfaces, or document the exception in the module spec's rationale |
| Fallibility pair (`push`/`try_push`) | SD2 | Allowed — the pair is the pattern (`std.collections/C2`), not surface growth |
| Cost-family conversions (`as_`/`to_`/`into_`) | SD2 | Allowed — the prefixes are one concept, learned once ([canonical-patterns](../canonical-patterns.md)) |
| Callers would routinely discard the error | SD3 | The API is absence-shaped — return `T?`, not `T or E`. A probe's failure is a non-answer, and an error branch nobody reads is ceremony at every call site ([canonical-patterns](../canonical-patterns.md)) |
| A Rust name really is the right one | SD4 | Fine — justified from the Rask side in the module spec's rationale, not from precedent |
| A module needs an interface the canonical set almost covers | SD7 | Extend the set here (a change to this spec, argued in its rationale) — never a one-module parallel protocol |
| A generic bound would hide a fallible conversion (e.g. integers of different width) | SD6 | Stay concrete. The seam is fixed by choosing types, not by a type parameter that swallows the policy |
| Deprecating toward one spelling | SD5 | The loser gets a lint pointing at the winner for one release, then removal (pre-1.0: immediate removal) |

---

## Appendix (non-normative)

### Rationale

**SD1 (one screen):** The alternative — "add whatever's useful" — is how every stdlib grows into a place where finding the function costs more than writing it yourself. A hard budget forces the merge/remove conversation at design time, when it's cheap, instead of at deprecation time, when it breaks people.

**SD3 (guess test):** Guessability compounds: a stdlib where the first guess works teaches users to guess, which makes every module cheaper to use than its docs. A stdlib that punishes guessing teaches doc-checking, and then the size of the docs *is* the size of the language. This is the API-level version of the reading-set budget ([DAY_ONE.md](../DAY_ONE.md), `spec.metrics` RS).

**SD4 (Rust legacy):** Rask's early stdlib sketches leaned on Rust names because that's what the hands knew. Some survived scrutiny (`Vec`, `Map`), most didn't (`Result` → `T or E`). The rule exists so the scrutiny happens per-name instead of per-habit.

**SD6/SD7 (composability):** I want Julia's composability, by generics and by conventions. The generics half without the conventions half is Rust: everything is generic and nothing agrees on which abstraction to be generic *over*, so flexibility lands at parameter positions (`impl AsRef`, `Into`) where it hides conversions and turns errors into trait-bound walls. The conventions half without the generics half is Go: everyone agrees, and you write the same loop per type. Both halves, statically checked, is the target. `min<T: Comparable>` with no `math.min` and no `.min()` method is the existing model case.

**SD8 (laws):** Composability means combinations nobody tested. The only way those are correct is if each side conforms to a stated contract rather than a shape. Laws live in the protocol's own spec and are cited from conformance docs; a comptime-checkable subset can come later without changing what the rule asks.

### See Also

- [canonical-patterns.md](../canonical-patterns.md) — the naming vocabulary (`is_*`, `to_*`, `with_*`, `try_*`)
- [DAY_ONE.md](../DAY_ONE.md) — the language-level reading-set budget this mirrors
- [README.md](README.md) — module inventory these rules govern
