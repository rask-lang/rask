<!-- id: type.generics -->
<!-- status: decided -->
<!-- summary: Trait matching by shape, operator-to-method expansion, verified clone/equal/comparable, code specialization per type -->
<!-- depends: types/structs.md, types/enums.md, types/traits.md -->
<!-- implemented-by: compiler/crates/rask-types/ -->

# Generics and Traits

Traits match by shape — if your type has the right methods, it satisfies the trait. Operators like `a + b` expand to method calls. The compiler generates specialized code for each concrete type you use (this is called *monomorphization*). For mixed-type collections, opt into runtime dispatch with `any Trait`.

## Core Principles

| Rule | Description |
|------|-------------|
| **G1: Trait matching** | A type satisfies a trait if it has all the required methods with matching signatures — no explicit `extend` needed |
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
| `trait Comparable` | Structural matching allowed |
| `explicit trait Serializable` | Requires explicit `extend` (for library stability) |
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

When a trait is marked `explicit`, types must provide an explicit `extend` block. This protects library APIs from accidental breakage when method signatures change.

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

## How Trait Matching Works

The compiler checks (G1) whether a type has all the methods a trait requires:
1. Method exists on the type (not a free function)
2. Parameter types match exactly
3. Return type matches exactly
4. Self parameter matches (value/mut/none)

| Type has | Trait requires | Satisfied |
|----------|----------------|-----------|
| `func compare(self, other: T) -> Ordering` | `compare(self, other: T) -> Ordering` | Yes |
| `func compare(self, other: T) -> i32` | `compare(self, other: T) -> Ordering` | No (return type mismatch) |
| `func compare(a: T, b: T) -> Ordering` | `compare(self, other: T) -> Ordering` | No (free function, not method) |

Types can also provide explicit implementations to override defaults or satisfy `explicit trait`:

```rask
extend Point with Comparable {
    func compare(self, other: Point) -> Ordering {
        // Custom implementation
    }
}
```

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

## Compiler-Verified Default

The compiler auto-derives Default where all fields have a known default value.

| Rule | Description |
|------|-------------|
| **DF1: Auto-derive for structs** | Struct is Default if every field's type is Default |
| **DF2: No enum default** | Enums do NOT auto-derive Default (which variant?) — requires manual implementation |
| **DF3: Override** | `extend Type with Default { ... }` overrides the auto-derived version |
| **DF4: Primitive defaults** | `0` for integers, `0.0` for floats, `false` for bool, `""` for string |

| Type | Default Value |
|------|--------------|
| Integer types | `0` |
| Float types | `0.0` |
| `bool` | `false` |
| `string` | `""` (empty string) |
| `Vec<T>` | Empty vec |
| `Map<K, V>` | Empty map |
| `T?` (optional) | `none` |
| Struct with all Default fields | Field-wise default |
| Enum | NOT auto-derived |

<!-- test: skip -->
```rask
struct Config {
    timeout: i32          // default: 0
    retries: i32          // default: 0
    verbose: bool         // default: false
}

const c = Config.default()  // Config { timeout: 0, retries: 0, verbose: false }
```

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
| Trait visibility | TD1 | Public by default; `priv trait` for module-private |
| Generic struct fields | G1 | `struct Foo<T: Comparable>` requires T: Comparable at every usage |
| Negative constraints | — | Not in MVP; workaround via naming convention or separate functions |
| Associated types | — | Not in MVP; deferred |
| More than 2 type params | — | Not in MVP; traits limited to 1-2 parameters |
| Omitted bounds (private) | GF2 | Inferred from body; see [Gradual Constraints](gradual-constraints.md) |
| Container method access | GF1 | Methods on containers (like `[]T.len()`) don't require constraints on T |

---

## Appendix (non-normative)

### Rationale

**G1 (trait matching):** Explicit constraints catch errors early without whole-program analysis. Matching by shape avoids needing to track trait implementations globally across the entire program.

**G4 (operator expansion):** Makes numeric code ergonomic — `a + b` reads naturally while the trait system handles dispatch.

**G5 (verified clone):** Compiler-verified Cloneable prevents aliasing bugs. Types with raw pointers can't silently claim to be cloneable.

**G6 (code specialization):** Keeps costs transparent and compilation fast. Each usage generates specialized code — no hidden function-pointer overhead.

**`explicit trait`:** Provides library stability when needed. Prevents accidental shape matches from breaking when method signatures evolve.

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
- **Type system**: Traits checked by shape at use site; no global tracking required (unless `explicit trait`)
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
| `Default` | `default() -> Self` | Yes — all Default fields, structs only (DF1) |
| `Displayable` | `to_string(self) -> string` | No — opt-in (user-facing output is intentional) |
| `Debug` | `debug_string(self) -> string` | Yes — all types |
| `Numeric` | `add, sub, mul, div, neg, zero, one, from_int` | No |
| `Convert<From, To>` | `convert(self: From) -> To` | No |
| `Encode` | Marker — no methods | Yes — all-Encode public fields (`std.encoding/E12`) |
| `Decode` | Marker — no methods | Yes — all-Decode public fields (`std.encoding/E12`) |

### See Also

- [Traits](traits.md) — Trait definitions and `any Trait` polymorphism (`type.traits`)
- [Structs](structs.md) — Struct definitions and methods (`type.structs`)
- [Enums](enums.md) — Enum types (`type.enums`)
- [Gradual Constraints](gradual-constraints.md) — Bound inference for private generics
- [Resource Types](../memory/resource-types.md) — Must-consume types (`mem.resource-types`)
