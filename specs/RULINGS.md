<!-- id: rulings -->
<!-- status: decided -->
<!-- summary: The eight tests that decide a question the specs don't answer yet -->
<!-- depends: CORE_DESIGN.md -->

# How to Rule on a New Question

[CORE_DESIGN.md](CORE_DESIGN.md) says what Rask values. This page is the working version: eight tests
that decide a case nobody has written a spec line for. Most existing rules fall
out of them, which is the evidence they're the real ones — they were extracted
from rulings already made, not invented up front.

Use them when someone proposes a feature, or when two specs seem to disagree.

## 1. Cost is visible in source; only the compiler may hide it

Generates: the 16-byte copy threshold (`mem.value/VS6`), `.clone()` being
explicit, mandatory `value as any Interface` because it allocates
(`type.interfaces/TR5`), `with` blocks making lock duration visible,
`Wrapping<T>` living in `num` rather than the prelude (`type.overflow/W1`).

The sharp corner is who's allowed to hide a cost. `string`'s refcount bump is
hidden and fine, because the compiler emits it, knows what it is, and deletes it
when it's provably unnecessary (`comp.string-refcount-elision`). User code in
that same slot is opaque — never elidable, never auditable. That one distinction
is the whole of `mem.shared-rack-heap/BX2`.

**Applying it:** ask who emits the hidden work, not how expensive it is.

## 2. The shape of the source decides, not the type of the value

The rules are syntactic on purpose, so reading the call site is enough.

Generates: `mutate` required at the call site whether or not the argument is Copy
(`mem.parameters/PM5`), `ensure` being block-scoped regardless of what it cleans
up (`ctrl.ensure/EN1`, `EN7`), `try` attaching to the one fallible step in a
chain (`type.errors/ER16a`), `read()` vs `write()` naming the discipline you took.

**Applying it:** if a proposal makes the meaning of a line depend on something
the reader can't see in the line, it's out. A rule that inspects the argument's
type to decide what the caller must write is the same error wearing a type.

The same goes for the order a line is read in. Between two equally expressive
spellings, pick the one that reads left to right without holding anything in
your head. That's why `!x?` is an error (`type.optionals/OPT16`), why sequence
adapters are methods that chain (`type.sequence`), and why terminal styling is a
`Style` value rather than nested calls (`std.terminal/T1`).

## 3. Anything statically decidable is decided statically — never a runtime flag

Generates: which cleanups run being fixed at compile time (`ctrl.ensure/C3`,
`C4`), a condition the compiler can decide from source being a compile error
rather than a compiled-in panic (`ctrl.panic/S7`), and the prettiest one —
`rack.delete(n)` nulls every incoming edge before returning, so the invalid state
doesn't exist, which is what earns unchecked link reads
(`mem.racks/RK3`, `RK4`).

**Applying it:** the strong form of a guarantee is "the bad state isn't
representable", not "the bad state is caught cheaply". A proposal that trades the
first for the second usually pays a check everywhere to save work somewhere.

## 4. Losing information must be visible; losing nothing needs no ceremony

Generates: the mandatory binder on `catch`, including `_ =>` to discard visibly
(`type.errors/ER14`), against bare `x ?? v` needing nothing because absence
carries no information (`type.optionals/OPT11`).

`ensure`'s silent error drop (`ctrl.ensure/ER1`) is the one exception, and note
how it's justified: not "errors don't matter here" but "scope exit has nowhere to
send an error, so the drop lives in `ensure`'s documented semantics rather than
hidden in an innocent-looking expression." The principle is about where a reader
learns of the loss.

**Applying it:** if a proposal makes an existing zero-ceremony operator start
losing information, that operator has to grow ceremony — and the cost is paid
language-wide, by every use site, not by the feature. That's the real price tag.

## 5. Guarantees are about the paths a program takes, not about surviving

Generates: unwind releasing access but never rolling back data
(`ctrl.panic/U2`), torn application invariants being yours while language-level
ones always hold (`LK3`).

Linearity is a claim about normal exit, `return`, and `try` propagation. A panic
is task death, not a path.

This test used to also generate "an unensured linear value leaks on panic,
admitted outright" (`ctrl.panic/U5`), on the reasoning that anything stronger
needs destructors — principle 1 again. That reasoning was wrong, and the
counterexample is worth keeping: `mem.linear/L7` made the acquisition-to-commitment
window empty, so there is never an unensured linear value for a panic to lose,
and the ensure that is always scheduled runs during unwind (`U1`). No
destructors. The third option was to make the bad state unrepresentable rather
than to catch it or to admit it, which is principle 3 reaching into this one.

What survives is narrower and still true: the panic *inside* an acquisition —
after the syscall, before the value is anyone's — is nobody's to clean up, and
the process staying alive is not a guarantee on offer.

**Applying it:** separate "the compiler promised this" from "the process stayed
alive". Before admitting a path can't be guaranteed, check whether the state it
loses can be made not to exist.

## 6. Machinery that only type-checks code the canon says not to write gets deleted

Generates: the removal of scrutinee narrowing (`type.errors/ER21`, `ER24`,
`ER25`). It existed solely so non-canonical error handling would compile. With it
gone the bad shape simply fails to type-check and the compiler suggests the
guard — no lint needed, because the shape routes itself.

**Applying it:** prefer deleting the support to adding the warning. See
[canonical-patterns.md](canonical-patterns.md) for what counts as canonical.

## 7. A feature pays rent in the language, not in the compiler

Compiler work is free: it's written once, and every program gets it. What isn't
free is what a feature adds to the language: one more thing every reader has to
know, every other feature has to interact with, and every tool (formatter, LSP,
`rask api`, both backends) has to understand. A feature is judged by that, not
by whether it's good in isolation.

Generates: no macro system, because DSLs only work in tools that expand macros
(`rejected-features.md`). No user-declarable Functor/Monad: `try` is the one
monad that earned a keyword, and the next gets a specialisation only once it
pays (`rejected-features.md`, Higher-Kinded Types). `Ok`/`Err`/`Some`/`None`
deleted, which took the whole family of rebind forms with them.

**Applying it:** count compiler effort as zero and language surface at full
price. A proposal that saves the compiler work by adding a keyword or an
annotation has the trade backwards. [DAY_ONE.md](DAY_ONE.md) is the budget for
what a reader has to carry, and a feature that lands on it pays the most.

## 8. The default is what most code does, so it has to be the right thing

Most code never overrides a default, which makes a default a policy decision.
The signature of a bad one is convenience where you write it and cost everywhere
else. Null is the canonical example: one less thing to write at the
declaration, paid for at every use.

Generates: `let` and `mut` being the same length, because `const` against `mut`
made the immutable binding the one that cost more to type
(`rejected-features.md`, `const` for Local Bindings). The `Default` interface's
removal: universal zeros let any type be built without anyone choosing its
values, and declared field defaults replaced them (`type.structs/FD1`).
Absence living in the type as `T?`, not in every reference.

**Applying it:** ask who pays when the default is wrong, and how they find out.
The lazy path and the correct path should be the same path. If the cheapest way
to satisfy the checker is an escape hatch (a cast, a `todo()`, a discard), the
checker is generating escape hatches, and the rule is what needs fixing.

## Habits that go with them

**Check the premise is constructible.** Before arguing about a proposed
operation, check whether the thing it operates on can exist. "Should
`Vec<T>.drain_into` handle linear `T`?" dissolves once you notice `Vec` can never
hold a linear value at all (`mem.linear`, containers) — dropping it would have to
consume each element, and there's no destructor to do that in.

**Argue from consequences.** A ruling names a program it makes shorter, longer,
or wrong, preferably one of the five validation programs in
[METRICS.md](METRICS.md). "It's cleaner" is a preference and can't be
settled. "This loop in the grep clone grows two lines" can.

**The semantics can't wait on an analysis.** Compiler work that doesn't exist
yet is fine to plan on; it's free once written. What a rule can't do is depend
on an analysis that might not succeed. A program's meaning has to hold with every
optimisation switched off, and an optimisation only makes it cheaper. String
refcount elision is the model: the refcount is correct without it.

**Keep the counterexample.** When a test turns out wrong, write down the case
that broke it next to the test, the way test 5 keeps `L7`. A model of
consequences is built from decisions whose outcome is known, and the reversed
ones teach the most.

**Nothing here is frozen.** These are the tests the rulings so far imply. A good
argument that one of them is wrong changes the test, and then the rules it
generated get revisited — all of them, in one go. What the tests rule out is
hedging: keeping a bad shape alive because changing it would break call sites.
