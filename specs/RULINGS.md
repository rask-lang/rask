<!-- id: rulings -->
<!-- status: decided -->
<!-- summary: The six tests that decide a question the specs don't answer yet -->
<!-- depends: CORE_DESIGN.md -->

# How to Rule on a New Question

[CORE_DESIGN.md](CORE_DESIGN.md) says what Rask values. This page is the working version: six tests
that decide a case nobody has written a spec line for. Most existing rules fall
out of them, which is the evidence they're the real ones — they were extracted
from rulings already made, not invented up front.

Use them when someone proposes a feature, or when two specs seem to disagree.

## 1. Cost is visible in source; only the compiler may hide it

Generates: the 16-byte copy threshold (`mem.value/VS6`), `.clone()` being
explicit, mandatory `value as any Trait` because it allocates
(`type.traits/TR5`), `with` blocks making lock duration visible,
`Wrapping<T>` living in `num` rather than the prelude (`type.overflow/W1`).

The sharp corner is who's allowed to hide a cost. `string`'s refcount bump is
hidden and fine, because the compiler emits it, knows what it is, and deletes it
when it's provably unnecessary (`comp.string-refcount-elision`). User code in
that same slot is opaque — never elidable, never auditable. That one distinction
is the whole of `mem.boxes/BX2`.

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

Generates: an unensured linear value leaking on panic, admitted outright
(`ctrl.panic/U5`), unwind releasing access but never rolling back data (`U2`),
torn application invariants being yours while language-level ones always hold
(`LK3`).

Linearity is a claim about normal exit, `return`, and `try` propagation. A panic
is task death, not a path. Anything stronger needs destructors, which is
principle 1 again.

**Applying it:** separate "the compiler promised this" from "the process stayed
alive". The second was never on offer.

## 6. Machinery that only type-checks code the canon says not to write gets deleted

Generates: the removal of scrutinee narrowing (`type.errors/ER21`, `ER24`,
`ER25`). It existed solely so non-canonical error handling would compile. With it
gone the bad shape simply fails to type-check and the compiler suggests the
guard — no lint needed, because the shape routes itself.

**Applying it:** prefer deleting the support to adding the warning. See
[canonical-patterns.md](canonical-patterns.md) for what counts as canonical.

## Two habits that go with them

**Check the premise is constructible.** Before arguing about a proposed
operation, check whether the thing it operates on can exist. "Should
`Vec<T>.drain_into` handle linear `T`?" dissolves once you notice `Vec` can never
hold a linear value at all (`mem.linear`, containers) — dropping it would have to
consume each element, and there's no destructor to do that in.

**Nothing here is frozen.** These are the tests the rulings so far imply. A good
argument that one of them is wrong changes the test, and then the rules it
generated get revisited — all of them, in one go. What the tests rule out is
hedging: keeping a bad shape alive because changing it would break call sites.
