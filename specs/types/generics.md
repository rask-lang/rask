<!-- id: type.generics -->
<!-- status: decided -->
<!-- summary: Nominal trait conformance via extend...with, operator-to-method expansion, verified clone/equal/comparable, code specialization per type -->
<!-- depends: types/structs.md, types/enums.md, types/traits.md -->
<!-- implemented-by: compiler/crates/rask-types/ -->

# Generics and Traits

Trait conformance is declared — `extend Type with Trait` says the type satisfies the trait, and the compiler checks the signatures against the declaration. `duck trait` opts individual traits into shape-matching — the prototyping mode; delete the keyword to harden. Operators like `a + b` expand to method calls. The compiler generates specialized code for each concrete type you use (this is called *monomorphization*). For mixed-type collections, opt into runtime dispatch with `any Trait`.

## Core Principles

| Rule | Description |
|------|-------------|
| **G1: Declared conformance** | A type satisfies a trait through a declared `extend Type with Trait` block, checked against the trait's signatures. `duck trait` opts a trait into shape-matching (no declaration needed). The four core traits (Equal, Hashable, Comparable, Cloneable) are auto-derived for eligible types — compiler-provided conformance, overridable per EQ2/HA2/CO2 and subject to OC1. `Debug` (all types), `Encode`/`Decode` (markers), and `ErrorMessage` (enums, `type.errors/ER6`) are also auto-derived |
| **G2: Checked at use site** | The compiler verifies trait matching when you call a generic function, not when you define it |
| **G3: Body-local inference** | Non-public functions can have bounds inferred from body; see [Gradual Constraints](gradual-constraints.md) |
| **G4: Operator expansion** | `a + b` becomes `a.add(b)` before trait checking |
| **G5: Verified clone** | Compiler ensures clone produces deep copy; types with pointers require unsafe extend |
| **G6: Code specialization** | Each `<T>` usage generates specialized code (monomorphization) — fast calls, but increases binary size |
| **G7: Runtime polymorphism opt-in** | `any Trait` for heterogeneous collections; dispatch through function pointer table (vtable) |

## Trait Definition

| Rule | Description |
|------|-------------|
| **TD1: Module-scoped** | Traits must be module-scoped |
| **TD2: Default methods** | Traits may contain default implementations |
| **TD3: Composition** | Traits may compose using `:` syntax |

| Trait Form | Meaning |
|------------|---------|
| `trait Comparable` | Nominal (default) — types conform via `extend Type with Comparable` |
| `duck trait Frobber` | Shape-matched — any type with the right methods satisfies it, no declaration. Prototyping mode |
| `trait Hashable: Equal` | Composition (requires all methods from Equal plus Hashable's own) |

```rask
trait Name {
    func method_name(self, params...) -> ReturnType
    func another_method(self) -> OtherType

    // Default implementation (optional)
    func helper(self) -> bool {
        self.method_name(...) != null
    }
}
```

Nominal is the default because conformance is a semantic claim, not just a shape: `compare()` existing doesn't make it a total order. The declaration states intent, gives the compiler a place to check signatures, and gives readers and tools a place to look.

`duck trait` is the prototyping dial, not a production feature — the stdlib ships zero duck traits. Declare a trait duck while sketching: no conformance declarations, methods move freely between types. To harden, delete the `duck` keyword — the compiler lists every type currently matching by shape and a quick-fix inserts the `extend Type with Trait {}` declarations. `rask lint` warns on duck traits outside prototype contexts. Declaring conformance to a duck trait is legal and harmless: documentation plus a signature check at the declaration instead of the use site.

## Generic Functions

| Rule | Description |
|------|-------------|
| **GF1: Public bounds explicit** | Public generic functions must declare trait constraints explicitly |
| **GF2: Private bounds inferred** | Non-public functions may omit constraints; compiler infers from body |
| **GF3: Caller constraints** | Calling a constrained function requires same or stronger constraints (explicit or inferred) |

```rask
// Public: bounds MUST be explicit
public func process<T: Hashable>(items: []T) { ... }

// Private: bounds inferred from body
func helper(item) { item.hash() }
// Compiler infers: func helper<T: Hashable>(item: T)
```

See [Gradual Constraints](gradual-constraints.md) for inference rules, smart error messages, and edge cases.

## How Conformance Is Checked

A conformance declaration provides the trait's methods (or inherits them from methods already on the type):

```rask
extend Point with Comparable {
    func compare(self, other: Point) -> Ordering {
        // Custom implementation
    }
}
```

An empty `extend Point with Comparable {}` declares conformance using methods the type already has. Either way, the compiler checks each required method:
1. Method exists on the type (not a free function)
2. Parameter types match exactly
3. Return type matches exactly
4. Self parameter matches (value/mut/none)

| Type has | Trait requires | Satisfied |
|----------|----------------|-----------|
| `func compare(self, other: T) -> Ordering` | `compare(self, other: T) -> Ordering` | Yes |
| `func compare(self, other: T) -> i32` | `compare(self, other: T) -> Ordering` | No (return type mismatch) |
| `func compare(a: T, b: T) -> Ordering` | `compare(self, other: T) -> Ordering` | No (free function, not method) |

For `duck trait`, the same signature check runs at the use site against the type's own methods — no declaration involved. Errors point at the declaration for nominal traits and at the use site for duck ones.

## Conformance Declarations

| Rule | Description |
|------|-------------|
| **CD1: Conformance list** | `extend T with A, B, C { ... }` declares all listed conformances. Each trait's signature check runs independently against the block plus the type's existing methods. Modifiers (`public extend`, `scoped extend`) apply to every listed trait |
| **CD2: Block body unrestricted** | The block may mix methods for any of the listed traits and ordinary non-trait methods. The conformance list is a header on a normal extend block, not a per-trait container |
| **CD3: Composite chain** | Declaring a composite (`extend T with HashKey {}`) checks the full supertrait chain (TD3); auto-derived supertraits satisfy automatically, missing methods error at the declaration |

<!-- test: skip -->
```rask
// The common shape for a trait-rich type: one block, header carries the claims
extend LogSource with Reader, Displayable, ErrorMessage {
    func read(mutate self, buf: Buffer) -> usize or IoError { ... }
    func to_string(self) -> string { ... }
    func message(self) -> string { ... }
    func rewind(mutate self) { ... }            // plain method, same block
}
```

## Method Namespace

One type, one method name, one meaning — with an opt-out scoped to the collision.

| Rule | Description |
|------|-------------|
| **MN1: Single namespace** | Methods defined in `extend T with Trait { }` are ordinary methods of T, same namespace as plain `extend T` blocks |
| **MN2: Shared implementation** | Two conformances requiring the same method name share the one implementation — legal iff both signatures match it |
| **MN3: Conflict needs scoping** | If the signatures disagree, the second conformance declaration is a compile error naming both traits — unless it is declared `scoped` |
| **MN4: Scoped conformance** | `scoped extend T with Trait { ... }` — methods in a scoped conformance do not enter T's inherent namespace. Reachable through trait dispatch (generic bounds, `any Trait`) and trait-qualified calls |
| **MN5: Trait-qualified call** | `Trait.method(value, args)` — mirrors `Type.method()` static-call syntax. Legal for any conformance, needed only for scoped ones |

<!-- test: skip -->
```rask
extend Dog with Greeter {
    func greet(self) -> string { ... }               // ordinary method: dog.greet()
}

scoped extend Dog with Announcer {
    func greet(self, volume: i32) -> string { ... }  // trait-only
}

dog.greet()                 // Greeter's — the inherent one
Announcer.greet(dog, 5)     // Announcer's — qualified
```

## Override Coherence

The core-trait family carries cross-trait contracts (`a == b` implies `hash(a) == hash(b)`; `compare` agrees with `eq`) that data structures physically rely on. Auto-derive keeps them consistent by construction; overrides must not silently break that.

| Rule | Description |
|------|-------------|
| **OC1: Override cancels dependents** | Overriding `Equal` cancels auto-derived `Hashable` and `Comparable` for that type. Overriding `Hashable` alone is safe (hashing fewer fields than eq compares costs collisions, never correctness) and cancels nothing |
| **OC2: Loud, with the fix** | Using a cancelled conformance is a compile error at the use site naming the override and the fix: declare the dependent trait consistent with the new eq |
| **OC3: Canonical order only** | `Comparable` is the type's one canonical order. The OC diagnostics steer one-off orderings ("sort by salary") to `sort_by` |

## Conditional Conformance

| Rule | Description |
|------|-------------|
| **CC1: Conditional conformance** | Conformance on a generic type holds exactly for instantiations satisfying its condition, checked at monomorphization like every other bound (G2/G6) |
| **CC2: Condition inferred** | Package-private conformances omit the clause; the compiler derives it from the conformance body — same machinery and local-only scope as gradual constraints (`type.gradual/GC6`). IDE ghosts the inferred clause |
| **CC3: Public states it** | Public conformances declare the condition explicitly with `where` — same rule as public function signatures (`type.gradual/GC5`) |
| **CC4: One condition per block** | Inferred conditions are computed per listed trait independently; an explicit `where` clause applies to the whole block. Traits needing different explicit conditions split into separate blocks |

<!-- test: skip -->
```rask
// Package-private: zero boilerplate — clause inferred as `where T: Displayable`
extend Ring<T> with Displayable {
    func to_string(self) -> string {
        return self.items.map(|x| x.to_string()).join(", ")
    }
}

// Public library API: the contract is spelled out
public extend Ring<T> with Displayable where T: Displayable { ... }
```

This is the same conditionality auto-derive has always applied implicitly ("Vec of Cloneable is Cloneable" — CL1), with syntax for user traits.

## Operator Expansion

The compiler expands operators into method calls before type checking (G4), then verifies the method exists.

| Operator | Desugars To | Trait Requirement |
|----------|-------------|-------------------|
| `a + b` | `a.add(b)` | `add(self, other: T) -> T` |
| `a - b` | `a.sub(b)` | `sub(self, other: T) -> T` |
| `a * b` | `a.mul(b)` | `mul(self, other: T) -> T` |
| `a / b` | `a.div(b)` | `div(self, other: T) -> T` |
| `a == b` | `a.eq(b)` | `eq(self, other: T) -> bool` |
| `a < b` | `a.compare(b) == Less` | `compare(self, other: T) -> Ordering` |

| Rule | Description |
|------|-------------|
| **OP1: Concrete operators are authored sugar** | Operator expansion on concrete types is method-call sugar — no conformance involved. The method being called was deliberately written on that type; nothing is matched, so the accidental-conformance hazard nominal conformance addresses doesn't exist here. Generic operator use goes through nominal bounds (`Numeric`, `Comparable`) like any other generic call |

## Compiler-Verified Cloneable

The compiler auto-derives Cloneable where all fields implement Cloneable and no raw pointers exist (G5).

| Rule | Description |
|------|-------------|
| **CL1: Auto-derive** | Primitives, structs with all Cloneable fields, arrays/Vec of Cloneable, handles: auto-derived |
| **CL2: Pointer block** | Struct with raw pointer is NOT Cloneable unless `unsafe extend` |

```rask
trait Cloneable {
    func clone(self) -> Self
}
```

| Type | Cloneable Status |
|------|------------------|
| Primitives (i32, bool, f64) | Auto-derived (bitwise copy) |
| Struct with all Cloneable fields | Auto-derived (deep copy) |
| Struct with raw pointer | NOT Cloneable unless `unsafe extend` |
| Array/Vec of Cloneable | Auto-derived (element-wise clone) |
| Handle types | Auto-derived (handle copy, not referent) |

## Compiler-Verified Equal

The compiler auto-derives Equal where all fields implement Equal — same pattern as Cloneable.

| Rule | Description |
|------|-------------|
| **EQ1: Auto-derive** | Primitives, structs with all Equal fields, enums (tag + payload equality): auto-derived |
| **EQ2: Override** | `extend Type with Equal { ... }` overrides the auto-derived version |
| **EQ3: Enum equality** | Variants compared by tag, then field-wise payload equality |

```rask
struct Point {
    x: i32
    y: i32
}

// No extend block needed — Point is Equal because i32 is Equal
const a = Point { x: 1, y: 2 }
const b = Point { x: 1, y: 2 }
// a == b → true (field-wise comparison)
```

| Type | Equal Status |
|------|--------------|
| Primitives (i32, bool, f64, string) | Auto-derived |
| Struct with all Equal fields | Auto-derived (field-wise) |
| Enum with all Equal payloads | Auto-derived (tag + payload) |
| Struct with `any Trait` field | NOT Equal unless manually implemented |
| Struct with closure field | NOT Equal (closures have no equality) |

## Compiler-Verified Hashable

The compiler auto-derives Hashable where all fields implement Hashable. Since Hashable requires Equal (supertrait), auto-derive applies only when both are satisfied.

| Rule | Description |
|------|-------------|
| **HA1: Auto-derive** | Primitives, structs with all Hashable fields, enums (tag + payload hash): auto-derived |
| **HA2: Override** | `extend Type with Hashable { ... }` overrides the auto-derived version |
| **HA3: Hash combine** | Field-wise hash uses deterministic combine (order matches declaration order) |
| **HA4: Float exclusion** | `f32` and `f64` are NOT Hashable (NaN != NaN violates Hashable contract) |

| Type | Hashable Status |
|------|-----------------|
| Integer primitives, bool, string | Auto-derived |
| `f32`, `f64` | NOT Hashable (NaN breaks equality) |
| Struct with all Hashable fields | Auto-derived (field-wise hash combine) |
| Enum with all Hashable payloads | Auto-derived (tag + payload) |
| Handle types | Auto-derived (hash of index + generation) |

## Compiler-Verified Comparable

The compiler auto-derives Comparable where all fields implement Comparable — lexicographic by declaration order. Since Comparable requires Equal (supertrait), auto-derive applies only when both are satisfied.

| Rule | Description |
|------|-------------|
| **CO1: Auto-derive** | Primitives, structs with all Comparable fields, enums (variant order, then payload): auto-derived |
| **CO2: Override** | `extend Type with Comparable { ... }` overrides the auto-derived version |
| **CO3: Lexicographic** | Fields compared in declaration order — first field is most significant |
| **CO4: Float exclusion** | `f32`/`f64` are NOT Comparable (NaN breaks totality); see `type.operators/ORD3` |

```rask
trait Comparable: Equal {
    func compare(self, other: Self) -> Ordering
}

enum Ordering { Less, Equal, Greater }
```

| Type | Comparable Status |
|------|-------------------|
| Integer primitives, bool, char, string | Auto-derived |
| `f32`, `f64` | NOT Comparable (NaN breaks totality) |
| Struct with all Comparable fields | Auto-derived (lexicographic by field order) |
| Enum with all Comparable payloads | Auto-derived (variant order, then payload) |
| Struct with float field | NOT Comparable unless manually implemented with `.total_cmp()` |

<!-- test: skip -->
```rask
struct Version {
    major: u32
    minor: u32
    patch: u32
}

// No extend block needed — Version is Comparable because u32 is Comparable
// Compares major first, then minor, then patch (lexicographic)
const a = Version { major: 1, minor: 2, patch: 0 }
const b = Version { major: 1, minor: 3, patch: 0 }
// a < b → true (minor field differs)
// a.compare(b) → Ordering.Less
```

## No Default Trait

There is no `Default` trait and no `.default()` method. Declared field defaults (`type.structs`) are the mechanism: a field with a declared default may be omitted at construction, and a struct whose fields all have defaults constructs with zero fields — `Config {}` *is* the default value. A struct with any defaultless field has no empty construction; the compiler names the missing field instead of inventing `""`/`0`. Universal zero-defaults were rejected as Go zero-values by another name.

## Comptime Generics

```rask
func dot<comptime N: usize>(a: [f32; N], b: [f32; N]) -> f32
```

Compiler infers `N` from array literals (`N = 2`) or known types (`arr: [f32; 5]`).

Errors if lengths differ, inference ambiguous, or non-literal const without explicit parameter.

## Must-Consume Types in Traits

Must-consume resource types (`@resource`) can be generic parameters. Narrowing over a `Resource?` must bind the value — wildcards are forbidden because that would silently drop the resource.

| Pattern | Resource content | Valid |
|---------|-----------------|-------|
| `if opt? as f` | Binds f | Yes, f must be consumed |
| `if opt?` (no bind, single-payload implicit) | Wildcard | No, compile error |
| `opt == none` branch | No value | Yes, nothing to consume |
| `if r? as f` on `Resource or E` | Binds f | Yes, f must be consumed |

## Trait Composition

Composition via `:` is additive (TD3). `T: HashKey` requires `hash`, `eq`, AND `clone`.

Compiler collects all methods from the full supertrait chain, deduplicates identical requirements, errors on conflicts.

```rask
trait Hashable: Equal {
    func hash(self) -> u64
}

trait HashKey: Hashable + Cloneable {}
// Requires: eq (from Equal), hash (from Hashable), clone (from Cloneable)
```

## Code Specialization (Monomorphization)

When you call `sort<i32>` and `sort<string>`, the compiler generates two separate `sort` functions — one optimized for `i32`, one for `string` (G6). Trait matching is verified at each call site. No whole-program analysis.

| Aspect | Behavior |
|--------|----------|
| Code size | Each type usage generates its own copy of the function |
| Type checking | Performed per usage with concrete types |
| Error location | Reported at the call site |
| Compilation | Incremental per compilation unit |

## Numeric Literals in Generics

Integer literals auto-coerce to T when `T: Numeric`. Compiler inserts `T.from_int()`. IDE shows ghost text.

```rask
trait Numeric {
    func add(self, other: Self) -> Self
    func zero() -> Self
    func one() -> Self
    func from_int(n: i64) -> Self
}

func increment<T: Numeric>(val: T) -> T {
    val + 1  // Compiler inserts: val + T.from_int(1)
}
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Zero usages | G2 | Function body syntax-checked; type errors may be deferred |
| Recursive generics | G6 | `Vec<Vec<T>>` allowed; compiler prevents infinite expansion |
| Trait visibility | TD1 | Package-visible by default, `public trait` exports — same rule as structs and functions (`struct.modules/V1`) |
| Same method required by two traits | MN2/MN3 | Same signature: shared implementation. Different: `scoped` or error |
| Trait evolution | TD2 | Adding a required method with a default body is non-breaking; without one it breaks every conformer (major version) |
| Generic struct fields | G1 | `struct Foo<T: Comparable>` requires T: Comparable at every usage |
| Negative constraints | — | Not in MVP; workaround via naming convention or separate functions |
| Associated types | — | Not in MVP; deferred |
| More than 2 type params | — | Not in MVP; traits limited to 1-2 parameters |
| Omitted bounds (private) | GF2 | Inferred from body; see [Gradual Constraints](gradual-constraints.md) |
| Container method access | GF1 | Methods on containers (like `[]T.len()`) don't require constraints on T |

---

## Appendix (non-normative)

### Rationale

**G1 (declared conformance):** This flipped. The original design matched by shape by default, with `explicit trait` as the opt-out — chosen to avoid global impl tracking. Two things overturned it: accidental conformance is silent-wrong (a `compare()` that isn't a total order satisfies `Comparable` structurally and misbehaves instead of erroring), and the declaration's cost dropped — one line that states intent is cheap, especially when most code is machine-written and human-reviewed. Checking stays local: a declaration is checked where it's written, and bounds are still checked at the use site — no whole-program analysis either way. `duck trait` keeps shape-matching available for prototyping.

**MN1–MN5 (single namespace):** Under shape-matching, one method satisfied every matching trait by construction; nominal conformance created the "which trait owns this method" question. Single namespace matches how people think ("Dog has a greet method") and keeps `dog.greet()` working when `greet` was defined inside a conformance block. The collision case is rare, and `scoped` puts the ceremony exactly on the declaration that collides — no Rust-style qualified-call syntax tax on everyone.

**OC1 (override cancels dependents):** Auto-derive keeps the eq/hash/compare contracts consistent by construction; a declared Equal override paired with an untouched auto-derived hash is the one guaranteed-inconsistent state (Map entries silently vanish). Cancellation plus a loud error removes it. The compiler can't verify a hand-written hash is consistent — no compiler can — but it can refuse to pair your eq with a hash you never looked at.

**OP1 (operators stay authored):** The flip's hazard — being *matched* against a contract you never claimed — requires a bound satisfied by accident. Concrete operator use has no bound; it calls a method someone deliberately wrote. The generic path was already nominal. Watch-item: if public generic code over-constrains through `Numeric` when it only needs `add`, split Numeric from usage evidence — not preemptively.

**Default (removed):** Zero corpus usage, and DF-style universal zeros were Go zero-values by another name — a back door around Rask's all-fields-required construction. Declared field defaults replaced the trait; `Config {}` is the default value when every field declares one. No spec API used `T: Default` as a bound at removal time; a constructible-empty bound can return from usage evidence if ever needed.

**G4 (operator expansion):** Makes numeric code ergonomic — `a + b` reads naturally while the trait system handles dispatch.

**G5 (verified clone):** Compiler-verified Cloneable prevents aliasing bugs. Types with raw pointers can't silently claim to be cloneable.

**G6 (code specialization):** Keeps costs transparent and compilation fast. Each usage generates specialized code — no hidden function-pointer overhead.

**`duck trait`:** The opt-in that replaced `explicit trait` when the default flipped, renamed from `structural` (jargon). The register is deliberate — the keyword reading as unserious *is* the signal that the contract is loose by design. Prototype with it, delete the keyword to harden (the compiler generates the missing declarations). The stdlib ships zero duck traits; docs note the concept is known elsewhere as structural typing.

### Patterns & Guidance

**Generic sorting:**

```rask
public func sort<T: Comparable>(items: []T) {
    for i in 1..items.len() {
        mut j = i
        while j > 0 && items[j] < items[j - 1] {
            swap(mut items[j], mut items[j - 1])
            j = j - 1
        }
    }
}
```

**HashMap with verified clone:**

```rask
trait HashKey: Hashable + Cloneable {}

public struct HashMap<K: HashKey, V> {
    buckets: []Bucket<K, V>
}

public func insert<K: HashKey, V>(map: HashMap<K, V>, key: K, val: V) {
    const idx = key.hash() % map.buckets.len()
    map.buckets[idx].add(key.clone(), val)  // Cloneable is compiler-verified deep copy
}
```

### Integration Notes

- **Memory model**: Generic ownership rules same as non-generic; move/copy determined per concrete type
- **Type system**: Conformance declared and checked locally at the `extend` block; bounds checked at use site — no global tracking. `duck trait` checks shape at use site
- **Concurrency**: Generic tasks can send owned generic values; traits verified per concrete type
- **Compiler**: Specialization happens per compilation unit; no cross-unit analysis
- **C interop**: Generic functions cannot be exported to C (no stable ABI); specialized wrappers required
- **Error handling**: Generic functions with `T or E` work normally; must-consume tracking per concrete type
- **Closures**: Generics in closures capture by value; traits verified at closure usage
- **Runtime polymorphism**: `any Trait` enables heterogeneous collections; see `type.traits`

### Standard Library Traits

| Trait | Methods | Auto-Derived? |
|-------|---------|---------------|
| `Equal` | `eq(self, other: Self) -> bool` | Yes — all Equal fields (EQ1) |
| `Comparable`: Equal | `compare(self, other: Self) -> Ordering` | Yes — all Comparable fields, lexicographic (CO1) |
| `Hashable`: Equal | `hash(self) -> u64` | Yes — all Hashable fields, no floats (HA1) |
| `Cloneable` | `clone(self) -> Self` | Yes — all Cloneable fields, no raw pointers (CL1) |
| `ErrorMessage` | `message(self) -> string` | Yes — enums, from variant names + payloads (`type.errors/ER6`); structs declare |
| `Displayable` | `to_string(self) -> string` | No — opt-in (user-facing output is intentional) |
| `Debug` | `to_debug_string(self) -> string` | Yes — all types |
| `Numeric` | `add, sub, mul, div, neg, zero, one, from_int` | No |
| `Convert<From, To>` | `convert(self: From) -> To` | No |
| `Encode` | Marker — no methods | Yes — all-Encode public fields (`std.encoding/E12`) |
| `Decode` | Marker — no methods | Yes — all-Decode public fields (`std.encoding/E12`) |

The four core traits (Equal, Hashable, Comparable, Cloneable) are the invariant-carrying family: their implementations get baked into data structures that cross package boundaries, so they are auto-derived, owner-overridable, and never third-party (cross-package conformance rules, issue #312).

### See Also

- [Traits](traits.md) — Trait definitions and `any Trait` polymorphism (`type.traits`)
- [Structs](structs.md) — Struct definitions and methods (`type.structs`)
- [Enums](enums.md) — Enum types (`type.enums`)
- [Gradual Constraints](gradual-constraints.md) — Bound inference for private generics
- [Resource Types](../memory/resource-types.md) — Must-consume types (`mem.resource-types`)
