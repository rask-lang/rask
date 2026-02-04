# Solution: Generics and Traits

## The Question
How do generic types and functions work in Rask? What is the constraint syntax, how do generics interact with traits, and what is the monomorphization strategy?

## Decision
Structural traits with local verification, operator-to-method desugaring, compiler-verified clone semantics, full monomorphization, and opt-in runtime polymorphism via `any Trait`.

## Rationale
Explicit trait constraints catch errors early without whole-program analysis. Structural satisfaction avoids global coherence complexity. Operator desugaring enables ergonomic numeric code. Compiler-verified clone prevents aliasing bugs. Full monomorphization maintains transparent costs and compilation speed. `explicit trait` option provides library stability when needed.

## Specification

### Core Principles

| Principle | Rule |
|-----------|------|
| Traits are structural | Type satisfies trait if it has all required methods with matching signatures |
| Verification is local | Compiler checks satisfaction at each monomorphization (code generation) site only |
| Inference is body-local | Non-public functions can have bounds inferred from body; see [Gradual Constraints](gradual-constraints.md) |
| Operators desugar | `a + b` becomes `a.add(b)` before trait checking |
| Clone is verified | Compiler ensures clone produces deep copy; types with pointers require unsafe extend |
| Monomorphization is full | Each `<T>` instantiation produces specialized code |
| Runtime polymorphism opt-in | `any Trait` for heterogeneous collections; vtable dispatch |

### Trait Definition

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

Traits MUST be module-scoped.
Traits MAY contain default implementations.
Traits MAY compose using `:` syntax.

| Trait Form | Meaning |
|------------|---------|
| `trait Comparable<T>` | Structural matching allowed |
| `explicit trait Serializable<T>` | Requires explicit `extend` (for library stability) |
| `trait HashKey<T>: Hashable<T>` | Composition (requires all methods from Hashable plus HashKey's own) |

**Explicit traits:** When a trait is marked `explicit`, types must provide an explicit `extend` block. This protects library APIs from accidental breakage when method signatures change.

### Generic Functions

```rask
public func max<T: Comparable>(a: T, b: T) -> T {
    if a.compare(b) == Greater { a } else { b }
}
```

ALL **public** generic functions MUST declare trait constraints explicitly.
Non-public generic functions MAY omit trait constraints; the compiler infers them from the function body. See [Gradual Constraints](gradual-constraints.md) for full rules.

```rask
// Public: bounds MUST be explicit
public func process<T: Hashable>(items: []T) { ... }

// Private: bounds inferred from body
func helper(item) { item.hash() }
// Compiler infers: func helper<T: Hashable>(item: T)
```

### Structural Satisfaction

| Type has | Trait requires | Satisfied |
|----------|----------------|-----------|
| `func compare(self, other: T) -> Ordering` | `compare(self, other: T) -> Ordering` | ✅ Yes |
| `func compare(self, other: T) -> i32` | `compare(self, other: T) -> Ordering` | ❌ No (return type mismatch) |
| `func compare(a: T, b: T) -> Ordering` | `compare(self, other: T) -> Ordering` | ❌ No (free function, not method) |

Compiler MUST check:
1. Method exists on type (not free function)
2. Parameter types match exactly
3. Return type matches exactly
4. Self parameter matches (value/mut/none)

**Explicit extend:** Types can provide explicit implementations to override defaults or satisfy `explicit trait` requirements:

```rask
extend Point with Comparable {
    func compare(self, other: Point) -> Ordering {
        // Custom implementation
    }
}
```

### Operator Desugaring

| Operator | Desugars To | Trait Requirement |
|----------|-------------|-------------------|
| `a + b` | `a.add(b)` | `add(self, other: T) -> T` |
| `a - b` | `a.sub(b)` | `sub(self, other: T) -> T` |
| `a * b` | `a.mul(b)` | `mul(self, other: T) -> T` |
| `a / b` | `a.div(b)` | `div(self, other: T) -> T` |
| `a == b` | `a.eq(b)` | `eq(self, other: T) -> bool` |
| `a < b` | `a.compare(b) == Less` | `compare(self, other: T) -> Ordering` |

Compiler MUST desugar operators before type checking.
Compiler MUST verify desugared method exists in trait bound.

### Compiler-Verified Clone

```rask
trait Clone<T> {
    clone(self) -> T
}
```

Compiler MUST auto-derive Clone for types where:
1. All fields implement Clone, AND
2. Type contains no raw pointers

| Type | Clone Status |
|------|--------------|
| Primitives (i32, bool, f64) | Auto-derived (bitwise copy) |
| Struct with all Clone fields | Auto-derived (deep copy) |
| Struct with raw pointer | NOT Clone unless `unsafe extend` |
| Array/Vec of Clone | Auto-derived (element-wise clone) |
| Handle types | Auto-derived (handle copy, not referent) |

Unsafe extend Clone MUST be explicit and marked unsafe.

### Const Generics

```rask
func dot<const N: usize>(a: [f32; N], b: [f32; N]) -> f32
```

Compiler MUST infer `N` from:
- Array literal lengths: `dot([1.0, 2.0], [3.0, 4.0])` → `N = 2`
- Known array types: `arr: [f32; 5]` → `N = 5`

Compiler MUST error if:
- Array lengths differ at call site
- Inference is ambiguous
- Non-literal const expression used without explicit parameter

### Linear Types in Traits

Linear resource types MAY be generic parameters.
Pattern matching on `Option<Linear>` MUST bind the value (wildcards forbidden).

| Pattern | Linear Content | Valid |
|---------|----------------|-------|
| `Some(f)` | Binds f | ✅ f must be consumed |
| `Some(_)` | Wildcard | ❌ Compile error |
| `None` | No value | ✅ Nothing to consume |
| `Ok(f)` | Binds f | ✅ f must be consumed |

### Trait Composition

```rask
trait Hashable<T> {
    hash(self) -> u64
    eq(self, other: T) -> bool
}

trait HashKey<T>: Hashable<T> {
    clone(self) -> T
}
```

Composition via `:` is additive.
`T: HashKey` requires: `hash`, `eq`, AND `clone`.

Compiler MUST collect all methods from composed traits.
Compiler MUST deduplicate identical method requirements.
Compiler MUST error if composed traits have conflicting signatures.

### Monomorphization

Each instantiation `func<ConcreteType>` produces specialized code.
Compiler MUST verify trait satisfaction at instantiation site.
Compiler MUST NOT perform whole-program analysis.

| Aspect | Behavior |
|--------|----------|
| Code size | Each instantiation generates new code (visible cost) |
| Type checking | Performed per instantiation with concrete types |
| Error location | Reported at instantiation site |
| Compilation | Incremental per compilation unit |

### Edge Cases

| Case | Handling |
|------|----------|
| Zero instantiations | Function body MUST be syntax-checked; type errors MAY be deferred |
| Recursive generics | `Vec<Vec<T>>` allowed; compiler MUST prevent infinite expansion |
| Trait visibility | Public by default; `priv trait` for module-private |
| Generic struct fields | `struct Foo<T: Comparable>` requires T: Comparable at every instantiation |
| Negative constraints | NOT in MVP; workaround via naming convention or separate functions |
| Associated types | NOT in MVP; deferred |
| More than 2 type params | NOT in MVP; traits limited to 1-2 parameters |
| Omitted bounds (private) | Inferred from body; see [Gradual Constraints](gradual-constraints.md) |

### Bounds Requirements

| Function | Requirement |
|----------|-------------|
| Public generic | MUST declare trait constraints explicitly |
| Private generic | MAY omit trait constraints (compiler infers from body) |
| Calling a constrained function | Caller must have same or stronger constraints (explicit or inferred) |

See [Gradual Constraints](gradual-constraints.md) for inference rules, smart error messages, and edge cases.

```rask
// Private: bounds inferred from body
func helper(item) { item.hash() }
// Compiler infers: func helper<T: Hashable>(item: T)

// Public: bounds MUST be explicit
public func process<T: Hashable>(items: []T) {
    for item in items { helper(item) }  // OK: T: Hashable satisfies inferred bound
}

// Still an error in public context:
public func bad<T>(items: []T) {
    for item in items { helper(item) }  // ERROR: T not bounded (public requires explicit)
}
```

Note: Methods on containers (like `[]T.len()`) don't require constraints on T.

### Numeric Literals in Generics

Integer literals auto-coerce to T when `T: Numeric`.
Compiler inserts `T.from_int()` automatically.
IDE shows ghost text indicating the conversion (per Principle 7).

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

func lerp<T: Numeric>(a: T, b: T, t: T) -> T {
    a * (1 - t) + b * t  // Clean, IDE shows conversions
}
```

## Examples

### Generic Sorting

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

### HashMap with Verified Clone

```rask
trait Hashable<T> {
    hash(self) -> u64
    eq(self, other: T) -> bool
}

trait HashKey<T>: Hashable<T> + Clone<T> {}

public struct HashMap<K: HashKey, V> {
    buckets: []Bucket<K, V>
}

public func insert<K: HashKey, V>(map: HashMap<K, V>, key: K, val: V) {
    const idx = key.hash() % map.buckets.len()
    map.buckets[idx].add(key.clone(), val)  // Clone is compiler-verified deep copy
}
```

## Integration Notes

- **Memory model**: Generic ownership rules same as non-generic; move/copy determined per instantiation
- **Type system**: Traits checked structurally at use site; no global coherence required (unless `explicit trait`)
- **Concurrency**: Generic tasks can send owned generic values; traits verified per instantiation
- **Compiler**: Monomorphization happens per compilation unit; no cross-unit analysis
- **C interop**: Generic functions cannot be exported to C (no stable ABI); monomorphized wrappers required
- **Error handling**: Generic functions with Result<T, E> work normally; linearity tracked per instantiation
- **Closures**: Generics in closures capture by value; traits verified at closure instantiation
- **Runtime polymorphism**: `any Trait` enables heterogeneous collections; see [traits.md](traits.md)

## Standard Library Traits

| Trait | Methods |
|-------|---------|
| `Comparable<T>` | `compare(self, other: T) -> Ordering` |
| `Equatable<T>` | `eq(self, other: T) -> bool` |
| `Hashable<T>` | `hash(self) -> u64; eq(self, other: T) -> bool` |
| `Clone<T>` | `clone(self) -> T` (compiler-verified) |
| `Numeric<T>` | `add, sub, mul, div, neg, zero, one, from_int` |
| `Default<T>` | `default() -> T` |
| `Convert<From, Into>` | `convert(self: From) -> Into` |