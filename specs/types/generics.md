<!-- id: type.generics -->
<!-- status: decided -->
<!-- summary: Structural traits, operator desugaring, compiler-verified clone, full monomorphization -->
<!-- depends: types/structs.md, types/enums.md, types/traits.md -->
<!-- implemented-by: compiler/crates/rask-types/ -->

# Generics and Traits

Structural traits with local verification, operator-to-method desugaring, compiler-verified clone, full monomorphization, opt-in runtime polymorphism via `any Trait`.

## Core Principles

| Rule | Description |
|------|-------------|
| **G1: Structural satisfaction** | Type satisfies trait if it has all required methods with matching signatures |
| **G2: Local verification** | Compiler checks satisfaction at each monomorphization site only |
| **G3: Body-local inference** | Non-public functions can have bounds inferred from body; see [Gradual Constraints](gradual-constraints.md) |
| **G4: Operator desugaring** | `a + b` becomes `a.add(b)` before trait checking |
| **G5: Verified clone** | Compiler ensures clone produces deep copy; types with pointers require unsafe extend |
| **G6: Full monomorphization** | Each `<T>` instantiation produces specialized code |
| **G7: Runtime polymorphism opt-in** | `any Trait` for heterogeneous collections; vtable dispatch |

## Trait Definition

| Rule | Description |
|------|-------------|
| **TD1: Module-scoped** | Traits must be module-scoped |
| **TD2: Default methods** | Traits may contain default implementations |
| **TD3: Composition** | Traits may compose using `:` syntax |

| Trait Form | Meaning |
|------------|---------|
| `trait Comparable<T>` | Structural matching allowed |
| `explicit trait Serializable<T>` | Requires explicit `extend` (for library stability) |
| `trait HashKey<T>: Hashable<T>` | Composition (requires all methods from Hashable plus HashKey's own) |

```rask
trait Name<T> {
    method_name(self, params...) -> ReturnType
    another_method(self) -> OtherType

    // Default implementation (optional)
    helper(self) -> bool {
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

## Structural Satisfaction

Compiler checks (G1):
1. Method exists on type (not free function)
2. Parameter types match exactly
3. Return type matches exactly
4. Self parameter matches (value/mut/none)

| Type has | Trait requires | Satisfied |
|----------|----------------|-----------|
| `func compare(self, other: T) -> Ordering` | `compare(self, other: T) -> Ordering` | Yes |
| `func compare(self, other: T) -> i32` | `compare(self, other: T) -> Ordering` | No (return type mismatch) |
| `func compare(a: T, b: T) -> Ordering` | `compare(self, other: T) -> Ordering` | No (free function, not method) |

Types can provide explicit implementations to override defaults or satisfy `explicit trait`:

```rask
extend Point with Comparable {
    func compare(self, other: Point) -> Ordering {
        // Custom implementation
    }
}
```

## Operator Desugaring

Compiler desugars operators before type checking (G4), then verifies method exists in trait bound.

| Operator | Desugars To | Trait Requirement |
|----------|-------------|-------------------|
| `a + b` | `a.add(b)` | `add(self, other: T) -> T` |
| `a - b` | `a.sub(b)` | `sub(self, other: T) -> T` |
| `a * b` | `a.mul(b)` | `mul(self, other: T) -> T` |
| `a / b` | `a.div(b)` | `div(self, other: T) -> T` |
| `a == b` | `a.eq(b)` | `eq(self, other: T) -> bool` |
| `a < b` | `a.compare(b) == Less` | `compare(self, other: T) -> Ordering` |

## Compiler-Verified Clone

Compiler auto-derives Clone where all fields implement Clone and no raw pointers exist (G5).

| Rule | Description |
|------|-------------|
| **CL1: Auto-derive** | Primitives, structs with all Clone fields, arrays/Vec of Clone, handles: auto-derived |
| **CL2: Pointer block** | Struct with raw pointer is NOT Clone unless `unsafe extend` |

```rask
trait Clone<T> {
    clone(self) -> T
}
```

| Type | Clone Status |
|------|--------------|
| Primitives (i32, bool, f64) | Auto-derived (bitwise copy) |
| Struct with all Clone fields | Auto-derived (deep copy) |
| Struct with raw pointer | NOT Clone unless `unsafe extend` |
| Array/Vec of Clone | Auto-derived (element-wise clone) |
| Handle types | Auto-derived (handle copy, not referent) |

## Comptime Generics

```rask
func dot<comptime N: usize>(a: [f32; N], b: [f32; N]) -> f32
```

Compiler infers `N` from array literals (`N = 2`) or known types (`arr: [f32; 5]`).

Errors if lengths differ, inference ambiguous, or non-literal const without explicit parameter.

## Linear Types in Traits

Linear resource types may be generic parameters. Pattern matching on `Option<Linear>` must bind the value (wildcards forbidden).

| Pattern | Linear Content | Valid |
|---------|----------------|-------|
| `Some(f)` | Binds f | Yes, f must be consumed |
| `Some(_)` | Wildcard | No, compile error |
| `None` | No value | Yes, nothing to consume |
| `Ok(f)` | Binds f | Yes, f must be consumed |

## Trait Composition

Composition via `:` is additive (TD3). `T: HashKey` requires `hash`, `eq`, AND `clone`.

Compiler collects all methods, deduplicates identical requirements, errors on conflicts.

```rask
trait Hashable<T> {
    hash(self) -> u64
    eq(self, other: T) -> bool
}

trait HashKey<T>: Hashable<T> {
    clone(self) -> T
}
```

## Monomorphization

Each instantiation produces specialized code (G6). Compiler verifies trait satisfaction at instantiation site. No whole-program analysis.

| Aspect | Behavior |
|--------|----------|
| Code size | Each instantiation generates new code (visible cost) |
| Type checking | Performed per instantiation with concrete types |
| Error location | Reported at instantiation site |
| Compilation | Incremental per compilation unit |

## Numeric Literals in Generics

Integer literals auto-coerce to T when `T: Numeric`. Compiler inserts `T.from_int()`. IDE shows ghost text.

```rask
trait Numeric<T> {
    add(self, other: T) -> T
    zero() -> T
    one() -> T
    from_int(n: i64) -> T
}

func increment<T: Numeric>(val: T) -> T {
    val + 1  // Compiler inserts: val + T.from_int(1)
}
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Zero instantiations | G2 | Function body syntax-checked; type errors may be deferred |
| Recursive generics | G6 | `Vec<Vec<T>>` allowed; compiler prevents infinite expansion |
| Trait visibility | TD1 | Public by default; `priv trait` for module-private |
| Generic struct fields | G1 | `struct Foo<T: Comparable>` requires T: Comparable at every instantiation |
| Negative constraints | — | Not in MVP; workaround via naming convention or separate functions |
| Associated types | — | Not in MVP; deferred |
| More than 2 type params | — | Not in MVP; traits limited to 1-2 parameters |
| Omitted bounds (private) | GF2 | Inferred from body; see [Gradual Constraints](gradual-constraints.md) |
| Container method access | GF1 | Methods on containers (like `[]T.len()`) don't require constraints on T |

---

## Appendix (non-normative)

### Rationale

**G1 (structural satisfaction):** Explicit constraints catch errors early without whole-program analysis. Structural satisfaction avoids global coherence complexity.

**G4 (operator desugaring):** Makes numeric code ergonomic — `a + b` reads naturally while the trait system handles dispatch.

**G5 (verified clone):** Compiler-verified clone prevents aliasing bugs. Types with raw pointers can't silently claim to be cloneable.

**G6 (full monomorphization):** Keeps costs transparent and compilation fast. Each instantiation is specialized — no hidden vtable overhead.

**`explicit trait`:** Provides library stability when needed. Prevents accidental structural matches from breaking when method signatures evolve.

### Patterns & Guidance

**Generic sorting:**

```rask
trait Comparable<T> {
    compare(self, other: T) -> Ordering
}

public func sort<T: Comparable>(items: []T) {
    for i in 1..items.len() {
        let j = i
        while j > 0 && items[j] < items[j - 1] {
            swap(mut items[j], mut items[j - 1])
            j = j - 1
        }
    }
}
```

**HashMap with verified clone:**

```rask
trait HashKey<T>: Hashable<T> + Clone<T> {}

public struct HashMap<K: HashKey, V> {
    buckets: []Bucket<K, V>
}

public func insert<K: HashKey, V>(map: HashMap<K, V>, key: K, val: V) {
    const idx = key.hash() % map.buckets.len()
    map.buckets[idx].add(key.clone(), val)  // Clone is compiler-verified deep copy
}
```

### Integration Notes

- **Memory model**: Generic ownership rules same as non-generic; move/copy determined per instantiation
- **Type system**: Traits checked structurally at use site; no global coherence required (unless `explicit trait`)
- **Concurrency**: Generic tasks can send owned generic values; traits verified per instantiation
- **Compiler**: Monomorphization happens per compilation unit; no cross-unit analysis
- **C interop**: Generic functions cannot be exported to C (no stable ABI); monomorphized wrappers required
- **Error handling**: Generic functions with `T or E` work normally; linearity tracked per instantiation
- **Closures**: Generics in closures capture by value; traits verified at closure instantiation
- **Runtime polymorphism**: `any Trait` enables heterogeneous collections; see `type.traits`

### Standard Library Traits

| Trait | Methods |
|-------|---------|
| `Comparable<T>` | `compare(self, other: T) -> Ordering` |
| `Equatable<T>` | `eq(self, other: T) -> bool` |
| `Hashable<T>` | `hash(self) -> u64; eq(self, other: T) -> bool` |
| `Clone<T>` | `clone(self) -> T` (compiler-verified) |
| `Numeric<T>` | `add, sub, mul, div, neg, zero, one, from_int` |
| `Default<T>` | `default() -> T` |
| `Convert<From, Into>` | `convert(self: From) -> Into` |

### See Also

- [Traits](traits.md) — Trait definitions and `any Trait` polymorphism (`type.traits`)
- [Structs](structs.md) — Struct definitions and methods (`type.structs`)
- [Enums](enums.md) — Enum types (`type.enums`)
- [Gradual Constraints](gradual-constraints.md) — Bound inference for private generics
- [Resource Types](../memory/resource-types.md) — Linear types (`mem.resource-types`)
