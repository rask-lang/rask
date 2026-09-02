<!-- id: type.gradual -->
<!-- status: decided -->
<!-- summary: Non-public functions may omit types and bounds while sketching; compiler infers from body, explicit signatures are the steady state -->
<!-- depends: types/traits.md, types/generics.md -->

# Gradual Constraints

Non-public functions may omit parameter types, return types, and bounds. Compiler infers from body using constraint solving. Public functions require full explicit signatures.

**This is a prototyping ergonomic, not a pillar.** Written-out signatures are the steady state; omitting them is for code you're still sketching. The three-level ladder below runs *toward* explicit, and the destination is Level 3 — a module you consider finished has its signatures written down, even the private ones.

The reason is the tradeoff inference actually makes. A private function's signature is derived from its body, so editing the body can change the signature, which can break callers — a break at a distance from the line you touched. That's a fine price while you're exploring, when the "callers" are three functions you wrote ten minutes ago and the compiler tells you exactly which ones shifted. It's a bad price in code you're maintaining, which is why ML-family languages ended up requiring signatures at module boundaries even though they can infer everything. Rask takes both positions, at different times in a module's life: infer while sketching, write it down to harden.

Two things bound the damage in the meantime. Inference never crosses a package (GC12), and it never looks at callers (GC6) — so the blast radius of an inferred signature is the package it lives in, and the compiler can name every caller that breaks. `duck trait` (`type.generics/DT1–DT4`) is the same story with the same enforcement line: a hard error where the looseness could reach someone else's code (`public duck trait`), a warning where it can only affect you (GC11, DT2, DT3).

## Core Rules

| Rule | Description |
|------|-------------|
| **GC1: Parameter inference** | Compiler examines all parameter uses; single concrete type inferred as concrete, only trait constraints inferred as generic with bounds |
| **GC2: Return inference** | Return type is unified type of all return expressions; incompatible types are a compile error |
| **GC3: Bound inference (mixed)** | Inferred bounds are the union of (a) nominal bounds propagated from called functions and (b) structural method-requirements from direct method/operator use. A nominal bound subsumes the methods it provides |
| **GC4: Additive annotations** | Explicit types/bounds merge with inferred; conflict is a compile error |
| **GC5: Public enforcement** | `public` functions must have full type annotations and trait bounds |
| **GC6: Module-local scope** | Inference examines only function body — no callers, no cross-module analysis |
| **GC11: Explicit is the steady state** | Inferred signatures are a sketching affordance. `rask lint` flags them in a package that carries publish metadata (`tool.lint/I4`) and `rask publish` reports a count (`struct.build/PB9`). Never a hard error — the code is fully checked either way, and a package may legitimately ship with inferred internals |
| **GC12: Invalidation stops at the package** | A shifted inferred signature can only break callers in the same package — non-public items don't cross package boundaries (`struct.modules/CM3`) — and propagates further only while each hop's own signature keeps shifting. No external consumer can be affected, ever |

| Principle | Rule |
|-----------|------|
| Public = explicit | `public` functions MUST have full type annotations and trait bounds |
| Private = flexible *while sketching* | Non-public functions MAY omit parameter types, return types, and/or bounds. Writing them out is what "done" looks like (GC11) |
| Annotations are additive | Explicit types/bounds merge with inferred ones |
| Inferred bounds are mixed | Direct method calls produce shape requirements; calls into bounded functions propagate their nominal bounds outward (GC3), so errors land at the outermost call site naming the real requirement. Named bounds appear when the signature is written out — see below |

## Inference Levels

Level 3 is where a module ends up. Levels 1 and 2 are the road there — deliberately temporary, and useful exactly as long as the code is still moving.

**Level 1 — Fully inferred (sketching; temporary by design):**

<!-- test: skip -->
```rask
func find_best(items, score_fn) {
    mut best = items[0]
    for i in 1..items.len() {
        if score_fn(items[i]) > score_fn(best) {
            best = items[i]
        }
    }
    return best
}
// Inferred: <T: Copy, U: Comparable>(items: Vec<T>, score_fn: |T| -> U) -> T
```

**Level 2 — Partially annotated (solidifying; still in motion):**

<!-- test: skip -->
```rask
func find_best(items: Vec<Record>, score_fn) -> Record {
    mut best = items[0]
    for i in 1..items.len() {
        if score_fn(items[i]) > score_fn(best) {
            best = items[i]
        }
    }
    return best
}
```

**Level 3 — Fully explicit (the steady state):**

<!-- test: parse -->
```rask
public func find_best<T: Copy, U: Comparable>(items: Vec<T>, score_fn: |T| -> U) -> T {
    mut best = items[0]
    for i in 1..items.len() {
        if score_fn(items[i]) > score_fn(best) {
            best = items[i]
        }
    }
    return best
}
```

`public` forces Level 3 (GC5). Nothing forces it on a private function — but that's where a hardened module lands, and the "Make signature explicit" quick action gets you there in one keystroke per function. Once the signature is written, editing the body can no longer move it: a body change that contradicts the signature is an error at the function, not a break at some caller.

## Hardening a Module

Nothing about hardening is manual archaeology — the compiler already knows every inferred signature, so writing them down is a tooling operation.

| Step | What it does |
|------|--------------|
| "Make signature explicit" (per function) | Fills in inferred parameter types, return type, bounds, error union, and self mode. Named traits appear where a bound is nominal; residual shape requirements go through the promotion rules (IS2) |
| `rask lint --rule idiom/inferred-signature` | Lists every non-public function still relying on inference |
| `rask publish` | Reports the remaining count (`struct.build/PB9`) — informational, never blocking |

Promotion is where the sketch actually gets pinned down, and it can surface a decision you'd been deferring: a shape requirement (`T: {frobnicate}`) becomes a named trait only when exactly one visible trait covers it, otherwise you pick or define one (IS2). That's the point. The trait name is the contract; inference was carrying a shape instead.

### Blast radius (GC12)

An inferred private signature is not an unbounded hazard, and the spec shouldn't be read as claiming otherwise:

- **Never crosses a package.** Non-public items aren't visible externally, so no external consumer can be affected by a body edit — full stop. A private body change recompiles the package and nothing beyond it (`struct.modules/CM3`).
- **Never walks the call graph blindly.** Inference reads one body (GC6). Invalidation propagates from a body edit to direct callers whose own inferred signatures shift, and transitively only while each hop keeps shifting. Most edits stop at the first caller.
- **Always named.** When a shift does break callers, the diagnostic lists them with the line that caused the change (see the GC2 message below). Nothing fails silently.
- **Bounded by choice.** Write the signatures out and the propagation stops at that function permanently.

`CORE_DESIGN.md` principle 5 states the same bound from the language side — local checking plus *bounded* propagation, not "one function = one invalidation."

## Concrete vs Generic Inference

| Rule | Description |
|------|-------------|
| **IN1: Generic preference** | Compiler infers most general type satisfying constraints |
| **IN2: Literal default** | If only info is literal default with no trait-method usage, infer concrete |
| **IN3: Trait triggers generic** | Constraints from trait methods, operators, or calls needing bounds produce generic |

| Example | Inferred As | Why |
|---------|-------------|-----|
| `func double(x) { x * 2 }` | `<T: Numeric>(x: T) -> T` | `*` desugars to `.mul()` |
| `func get_port() { 8080 }` | `() -> i32` | Literal default, no trait usage |
| `func greet(name) { println("Hi, {name}") }` | `(name: string)` | String interpolation constrains type |
| `func len(items) { items.len() }` | `<T>(items: Vec<T>) -> usize` | `.len()` doesn't constrain T |

## Auto-Generics: Single Letters Only

| Rule | Description |
|------|-------------|
| **PC1: Single letters are type params** | A single uppercase letter in a signature type position (`T`, `U`, `K`, `V`, …) is always a type parameter — resolved without scope lookup, so imports can never change a signature's meaning. Explicit `<T>` stays optional |
| **PC2: Other names must resolve** | Any name longer than one letter in a signature must name a declared type. Unknown name is an immediate error with a "did you mean" suggestion — a typo never silently becomes a generic, and never silently becomes nothing |
| **PC3: Single letters reserved** | Declaring a struct, enum, trait, union, or type alias with a single-letter name is a compile error |

Signature positions: function parameters, return types, struct fields, enum payloads.

<!-- test: skip -->
```rask
// Single letter: type parameter, no <T> declaration needed
func swap(a: T, b: T) -> (T, T) { return (b, a) }

// Longer unknown names error where the typo is...
func load(c: Confg) -> i32 { }
// ERROR [type.gradual/PC2]: unknown type `Confg` — did you mean `Config`?

// ...whatever their case. `str` isn't a Rask type; the string type is `string`.
func label(s: str) -> i32 { }
// ERROR [type.gradual/PC2]: unknown type `str` — did you mean `string`?

// ...descriptive type parameters use an explicit list
func map<Item, Output>(items: Vec<Item>, f: |Item| -> Output) -> Vec<Output> { }

// Gradual: omit the type entirely, let the compiler decide
func identity(x) { return x }
// Inferred: func identity<T>(x: T) -> T
```

## Error Messages

```
ERROR [type.gradual/GC5]: public function requires explicit type annotations
   |
1  |  public func process(data, handler) {
   |                       ^^^^  ^^^^^^^ add type annotations

   Inferred signature:
     public func process<T: Validatable>(data: Vec<T>, handler: |Vec<T>| -> T) -> T

   hint: apply suggested signature? (IDE quick action)
```

```
ERROR [type.gradual/GC4]: explicit annotation conflicts with body usage
   |
3  |  func transform(x: i32) {
   |                     ^^^ annotated as i32
5  |      x.display()
   |        ^^^^^^^^^ i32 does not have method 'display'

FIX: Change parameter type, or remove the .display() call.
```

```
ERROR [type.gradual/GC2]: inferred return type changed
   |
   Before: func compute(data: Vec<i32>) -> i32
   After:  func compute(data: Vec<i32>) -> f64

   Caused by:
12 |  data.sum() / 2.5
                    ^^^ f64 literal changed inferred return type

   Callers that break:
     main.rk:45  let result: i32 = compute(items)

   note: `compute` is not public, so this is contained to this package (GC12).
         Writing the return type out pins it: the error would then land on
         line 12 instead of at the caller.
```

```
WARNING [tool.lint/I4]: `scan` relies on an inferred signature
   |
34 |  func scan(input, pos) {
   |            ^^^^^  ^^^ inferred: (input: string, pos: usize) -> Token?
   |
WHY: this package declares publish metadata, so it's past the sketching
     phase. Inference is fine while code is moving; a body edit here can
     shift the signature and break callers elsewhere in the package.

FIX: "Make signature explicit", or @allow(idiom/inferred-signature).
```

## Error Union Inference

Error return types are inferred like any other return type — the compiler collects error types from all `try` calls and bare error-typed returns in the body. Three annotation levels from least to most explicit:

| Rule | Description |
|------|-------------|
| **GC7: Error union from body** | `try` calls and bare error-typed return expressions contribute to the inferred error union |
| **GC8: Public errors explicit** | Public functions must declare error types explicitly (same as GC5) |

<!-- test: skip -->
```rask
// Fully omitted — both success and error types inferred
func load_config(path: string) {
    let text = try read_file(path)     // contributes IoError
    let config = try parse(text)       // contributes ParseError
    return config
}
// Inferred: -> Config or (IoError | ParseError)

// Partial: `or _` — success type explicit, error union inferred
func load_config(path: string) -> Config or _ {
    let text = try read_file(path)     // contributes IoError
    let config = try parse(text)       // contributes ParseError
    return config
}
// LSP ghost text: -> Config or (IoError | ParseError)

// Public: must be fully explicit
public func load_config(path: string) -> Config or (IoError | ParseError) {
    let text = try read_file(path)
    return try parse(text)
}
```

See `type.errors/ER23–ER25` for full rules.

## Self Mode Inference

Private methods can omit the `self` mode modifier. The compiler infers it from how `self` is used in the body.

| Rule | Description |
|------|-------------|
| **GC9: Self mode from body** | `self` only read → borrow (default). `self` fields written → `mutate self`. `self` moved/consumed → `take self` |
| **GC10: Public self explicit** | Public methods must declare self mode explicitly (API contract) |

<!-- test: skip -->
```rask
extend Player {
    // Private: self mode inferred as mutate (writes to self.health)
    func damage(self, amount: i32) {
        self.health -= amount
    }
    // IDE ghost text: func damage(mutate self, amount: i32)

    // Private: self mode inferred as take (self consumed)
    func into_stats(self) -> Stats {
        Stats { health: self.health, name: self.name }
    }
    // IDE ghost text: func into_stats(take self) -> Stats

    // Public: must be explicit
    public func heal(mutate self, amount: i32) {
        self.health += amount
    }
}
```

Inference rules:
- Read-only access to any `self` field → borrow (unchanged from default)
- Write to any `self` field → `mutate self`
- Move of `self` or field that triggers ownership transfer → `take self`
- Conflict (read in one branch, write in another) → `mutate self` (conservative)

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Recursive functions | GC2 | Inferred from base case + recursive structure; ambiguous requires return type annotation |
| Mutual recursion | GC6 | Both analyzed together (SCC); unresolvable requires annotation on at least one |
| Closures | GC1 | Closure params already inferred from context; gradual applies to enclosing function |
| `any Trait` | GC3 | Cannot be inferred — dynamic dispatch must be explicit |
| `comptime` parameters | GC5 | Must be explicit — compilation requires them upfront |
| Empty function body | GC1 | Parameters are unconstrained generics, return type is `()` |
| Multiple return types | GC2 | Incompatible branch types produce compile error |
| `extern` functions | GC5 | Must have full explicit signatures (C ABI requires it) |
| Private function with `try` | GC7 | Error union inferred from body |
| Private method writing to `self` | GC9 | `mutate self` inferred |
| Private method consuming `self` | GC9 | `take self` inferred |
| Public method omitting self mode | GC10 | Compile error — must be explicit |
| Inferred signature in a package with publish metadata | GC11 | Lint warning (`tool.lint/I4`), reported again by `rask publish` — never blocking |
| Body edit shifts an inferred signature | GC12 | Callers in the same package are re-checked and named in the diagnostic; nothing outside the package can be affected |
| Inferred helper called by another inferred helper | GC12 | Invalidation follows the chain only while each hop's signature keeps shifting; stops at the first hop that holds still |

---

## Appendix (non-normative)

### Rationale

**GC1 (parameter inference):** When sketching, focus is logic, not types. Requiring explicit signatures on private helpers adds ceremony that slows exploration without improving safety — compiler checks inferred types identically.

**GC5 (public enforcement):** `public` means "visible to external consumers." Explicit types at this boundary are natural — API contracts should be spelled out. Private functions are implementation details where inference reduces noise.

**GC6 (module-local scope):** Compiler examines one function at a time, collects constraints, solves them. Never looks at callers, never does whole-program analysis, never crosses modules. Preserves compilation speed.

**GC11 (why a lint and not a rule):** The honest objection to gradual constraints is that a private function's signature living in its body means a body edit is an API edit — action at a distance, in a language whose fifth principle is local analysis. The answer isn't that the objection is wrong; it's that inference is scoped to where the tradeoff is worth taking. While you're sketching, "the signature follows the code" is the feature. Once the code stops moving, it's a liability, so hardening means writing the signatures down, and the ladder's endpoint (Level 3) is the framing rather than a footnote on a progression.

Why a warning rather than a gate: an inferred private signature cannot break anyone outside the package (GC12), so there's nothing for a publish check to protect. Gating it would be ceremony without a victim, and would forbid a legitimate shape — a small published package whose internals are genuinely still in flux. The same test applied to `duck trait` puts the hard error at `public` (`type.generics/DT1`), where a stranger's code is genuinely at risk, and leaves package-internal duck traits reported rather than blocked (`type.generics/DT2`). One rule for both: enforce where it protects someone else, inform where it doesn't.

The trigger is publish metadata (`description` + `license`, which `struct.build/PB2` requires to publish at all) rather than a new "is this a scratchpad" manifest key. Those fields are already the signal that you consider the package something other people will use, and reusing them means one less knob.

**GC12 (bounded invalidation):** Named as its own rule because "inference is local" (GC6) and "invalidation is local" are different claims, and only the first is unconditionally true. Inference reads one body; invalidation follows shifted signatures to direct callers and stops as soon as a hop's signature holds still. The package boundary is a hard ceiling on it. Stating the bound explicitly beats implying a stronger claim and being caught out by a chain of three inferred helpers.

**PC1–PC3 (single letters only):** The original rule made *any* unknown PascalCase name a type parameter. Two failure modes: a typo'd type name (`Confg`) silently became a generic and surfaced later as a confusing constraint failure at some call site, and adding or importing a type could silently flip an existing signature from generic to concrete — action at a distance from an import. Single letters close both. Typos are multi-letter, so they error early with a suggestion; single letters never consult scope, so a signature means the same thing no matter what's imported. The `swap(a: T, b: T)` idiom — linking two parameters without ceremony — survives, and descriptive names are one `<Item, Output>` away. Gradual constraints already cover the "just let me sketch" case by omitting types entirely.

**PC2 covers lowercase names too.** It used to say "PascalCase", because the rule
was written against the generics confusion, where case is exactly what separates a
type parameter from a type. But a lowercase name that names nothing isn't a
generics question at all — it was simply unchecked, so `func f(x: uszie)`
type-checked and failed later at the use site, on a type that was never real.
`str` got into two proposed specs that way (#966). The rule's own reason — a typo
never silently becoming something else — applies whatever the case, so the case
requirement is gone. Length is still the test: a single uppercase letter is a
type parameter (PC1), anything longer must resolve.

**Ergonomic Delta:** Without gradual constraints, Rask private code needs more annotation than Go or Kotlin. With them, private code matches or beats ceremony of dynamically-typed languages while keeping full static checking.

### Patterns & Guidance

**Prototype-to-production pipeline:**

1. **Sketch:** `func process(data, handler) { ... }` — all inferred
2. **Solidify:** `func process(data: Vec<Record>, handler) { ... }` — partial
3. **Harden:** `public func process<T: Comparable>(data: Vec<T>, handler: Handler<T>) -> T` — explicit

Fully statically checked at every stage — not dynamic typing. Step 3 isn't only about going public; it's what a private function in finished code looks like too.

**Interaction with nominal traits:** Direct method calls infer shape requirements — deliberately looser than nominal conformance: private-only sketching glue, invisible in any API. The moment the signature is written out (and always at `public`), bounds are named traits and nominal conformance applies (`type.generics/G1`). The seam has three rules:

| Rule | Description |
|------|-------------|
| **IS1: Mixed inference** | Per GC3, nominal bounds propagate up from callees; only direct method use stays shape-based |
| **IS2: Promotion is exact** | "Make explicit"/"make public" fills in a named trait only when exactly one visible trait covers the residual method-requirements. Zero matches: report the methods and offer to generate a trait definition plus conformance declarations. Two or more: list candidates, the user picks — never auto-pick a semantic claim |
| **IS3: Honest ghost text** | Display distinguishes propagated nominal bounds from raw shape requirements: `T: Comparable` vs `T: {frobnicate}`. Never show a trait name that was merely guessed from shape |

**Gotcha (by design):** annotating a working private function can make a working call fail — the bound's meaning flips from shape to declaration when written down. A callee type that had the methods but never declared conformance passes inference and fails the explicit bound. This is the publish step doing its job: naming the contract.

**Prototyping with traits:** traits belong to the hardening phase; the sketching phase needs none (inference carries shapes). When a trait is wanted while sketching, `duck trait` is the scratchpad form — no conformance declarations, methods move freely. Harden by deleting the `duck` keyword: the compiler lists every shape-matching type and quick-fixes insert the declarations (`type.generics/DT4`). The one place duck traits are gated harder than inferred signatures: a duck trait may never be `public` (`type.generics/DT1`), because shape-matching that crosses a package boundary can break code its author never sees. Neither is gated inside a package — GC12 and DT1 both mean the looseness stays with the author who wrote it.

**Monomorphization:** Inference doesn't change monomorphization. Compiler infers bounds, then monomorphization proceeds as with explicit: each call site generates specialized code. Inferred signature is semantically identical to equivalent explicit.

### IDE Integration

Ghost text displays inferred types, bounds, and return types:

<!-- test: skip -->
```rask
func process(data, handler) {           // ghost: <T: Validatable>(data: Vec<T>, handler: |Vec<T>| -> T) -> T
    let result = handler(data)
    result.validate()
    return result
}
```

Quick actions:
- **"Make signature explicit"** — fills in all inferred types, bounds, error unions, and self modes
- **"Make error type explicit"** — fills in only the inferred error union
- **"Make public"** — adds `public` and fills in the full explicit signature

Hover on a parameter shows its full inferred type. Hover on the function name shows the complete inferred signature.

### See Also

- `type.traits` — Trait definitions and structural matching
- `type.generics` — Generic type parameters and bounds
- `ctrl.comptime` — Compile-time parameters
