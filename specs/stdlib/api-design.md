<!-- id: std.api -->
<!-- status: decided -->
<!-- summary: Stdlib API rules — small surface, guessable names, one powerful function over many specific, no Rust legacy by reflex -->
<!-- depends: canonical-patterns.md -->

# Stdlib API Design

The stdlib is where language size actually hits people. Nobody reads the grammar; everybody asks "what function do I call for this" fifty times a day. These rules keep that question cheap to answer — and usually unnecessary to ask.

## Rules

| Rule | Description |
|------|-------------|
| **SD1: One screen per module** | A module's public surface fits on one screen — roughly 20 items. That's a budget, not a guideline: adding past it means removing or merging something, or arguing why this module is the exception. `rask api <module>` is the measuring stick |
| **SD2: Powerful over specific** | One function with orthogonal parameters beats a family of names. `read(path)` with options beats `read_text`/`read_lines`/`read_binary` siblings. Name-variants are reserved for exactly two axes: fallibility pairs (`push`/`try_push` — `std.collections/C2`) and cost pairs per the naming table (`as_*`/`to_*`/`into_*`) |
| **SD3: The guess test** | Before designing a function, write the call site you'd *guess* — the line you'd type before opening any docs. If the guess is reasonable and the stdlib differs, the stdlib is wrong, not the guess. Names come from [canonical-patterns.md](../canonical-patterns.md)'s vocabulary so guesses transfer between modules |
| **SD4: No Rust legacy by reflex** | Every name and shape is justified from how the Rask call site reads, never from what `std` calls it. Rask has `T or E`, `T?`, `Owned`, `Shared`, `Cell`, `Mutex` — so `Result`, `Option`, `Box`, `Rc`, `RefCell`, `Arc<Mutex<T>>` never appear, and neither do their method idioms (`unwrap`, `expect`, `ok_or`, `and_then`). `Vec`/`Map` survive because they read right in Rask, not because Rust has them |
| **SD5: One way** | No convenience aliases, no two spellings for one operation (`mem.atomics/GA1` is the precedent). If two functions do the same thing, one of them is deprecated the day the second lands |

## Why SD1 is the load-bearing rule

The day-to-day cost of a big stdlib isn't learning it — it's *re-scanning* it. Every "which function do I want" pause is a trip to the docs, and a module with 60 entries makes that trip mandatory; a module with 15 makes it skippable, because the answer is visible in one `rask api` call or one autocomplete popup. Go's stdlib is loved for exactly this: each package holds a dozen things you can keep in your head. Batteries included means every battery *slot* is filled — not that every slot holds six batteries.

SD2 is how SD1 stays possible: surface grows by parameter, not by name. A parameter is discoverable at the one function you already found; a sibling function is another entry you had to know existed.

## The guess test, operationally (SD3)

When speccing a module, write the *call sites first* — a dozen lines of realistic use, before any signature exists — and have someone (or a second pass, cold) guess what each call does and what its variants would be called. Three outcomes:

- Guess matches design → done.
- Guess is wrong because the operation is genuinely subtle → keep the design, and the doc comment leads with the distinction.
- Guess is reasonable and differs → **rename toward the guess.** The guess is data about every future user's first attempt.

This is `CLAUDE.md`'s "sketch how the call site reads first" made into a gate rather than advice.

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Module genuinely needs > one screen (e.g. `math`) | SD1 | Split into submodules with one-screen surfaces, or document the exception in the module spec's rationale |
| Fallibility pair (`push`/`try_push`) | SD2 | Allowed — the pair is the pattern (`std.collections/C2`), not surface growth |
| Cost-family conversions (`as_`/`to_`/`into_`) | SD2 | Allowed — the prefixes are one concept, learned once ([canonical-patterns](../canonical-patterns.md)) |
| Callers would routinely discard the error | SD3 | The API is absence-shaped — return `T?`, not `T or E`. A probe's failure is a non-answer, and an error branch nobody reads is ceremony at every call site ([canonical-patterns](../canonical-patterns.md)) |
| A Rust name really is the right one | SD4 | Fine — justified from the Rask side in the module spec's rationale, not from precedent |
| Deprecating toward one spelling | SD5 | The loser gets a lint pointing at the winner for one release, then removal (pre-1.0: immediate removal) |

---

## Appendix (non-normative)

### Rationale

**SD1 (one screen):** The alternative — "add whatever's useful" — is how every stdlib grows into a place where finding the function costs more than writing it yourself. A hard budget forces the merge/remove conversation at design time, when it's cheap, instead of at deprecation time, when it breaks people.

**SD3 (guess test):** Guessability compounds: a stdlib where the first guess works teaches users to guess, which makes every module cheaper to use than its docs. A stdlib that punishes guessing teaches doc-checking, and then the size of the docs *is* the size of the language. This is the API-level version of the reading-set budget ([DAY_ONE.md](../DAY_ONE.md), `spec.metrics` RS).

**SD4 (Rust legacy):** Rask's early stdlib sketches leaned on Rust names because that's what the hands knew. Some survived scrutiny (`Vec`, `Map`), most didn't (`Result` → `T or E`). The rule exists so the scrutiny happens per-name instead of per-habit.

### See Also

- [canonical-patterns.md](../canonical-patterns.md) — the naming vocabulary (`is_*`, `to_*`, `with_*`, `try_*`)
- [DAY_ONE.md](../DAY_ONE.md) — the language-level reading-set budget this mirrors
- [README.md](README.md) — module inventory these rules govern
