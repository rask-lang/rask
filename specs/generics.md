# Solution: Generics and Traits

## The Question
How do generic types and functions work in Rask? What is the bounds syntax, how do generics interact with traits, and what is the monomorphization strategy?

## Decision
Structural traits with local verification, operator-to-method desugaring, compiler-verified clone semantics, full monomorphization, and opt-in runtime polymorphism via `any Trait`.

## Rationale
Explicit trait bounds catch errors early without whole-program analysis. Structural satisfaction avoids global coherence complexity. Operator desugaring enables ergonomic numeric code. Compiler-verified clone prevents aliasing bugs. Full monomorphization maintains transparent costs and compilation speed. `explicit trait` option provides library stability when needed.

## Specification

### Core Principles

| Principle | Rule |
|-----------|------|
| Traits are structural | Type satisfies trait if it has all required methods with matching signatures |
| Verification is local | Compiler checks satisfaction at each monomorphization site only |
| Operators desugar | `a + b` becomes `a.add(b)` before trait checking |
| Clone is verified | Compiler ensures clone produces deep copy; types with pointers require unsafe impl |
| Monomorphization is full | Each `<T>` instantiation produces specialized code |
| Runtime polymorphism opt-in | `any Trait` for heterogeneous collections; vtable dispatch |

### Trait Definition

```
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
| `explicit trait Serializable<T>` | Requires explicit `impl` (for library stability) |
| `trait HashKey<T>: Hashable<T>` | Composition (requires all methods from Hashable plus HashKey's own) |

**Explicit traits:** When a trait is marked `explicit`, types must provide an explicit `impl` block. This protects library APIs from accidental breakage when method signatures change.

### Generic Functions

```
pub fn max<T: Comparable>(a: T, b: T) -> T {
    if a.compare(b) == Greater { a } else { b }
}
```

ALL generic functions MUST declare trait bounds (public AND private).
This preserves local analysis—no call-graph tracing required.

```
// Both must have explicit bounds
pub fn process<T: Hashable>(items: []T) { ... }
fn helper<T: Hashable>(item: T) { ... }  // Also needs bounds
```

### Structural Satisfaction

| Type has | Trait requires | Satisfied |
|----------|----------------|-----------|
| `fn compare(self, other: T) -> Ordering` | `compare(self, other: T) -> Ordering` | ✅ Yes |
| `fn compare(self, other: T) -> i32` | `compare(self, other: T) -> Ordering` | ❌ No (return type mismatch) |
| `fn compare(a: T, b: T) -> Ordering` | `compare(self, other: T) -> Ordering` | ❌ No (free function, not method) |

Compiler MUST check:
1. Method exists on type (not free function)
2. Parameter types match exactly
3. Return type matches exactly
4. Self parameter matches (value/mut/none)

**Explicit impl:** Types can provide explicit implementations to override defaults or satisfy `explicit trait` requirements:

```
impl Comparable for Point {
    fn compare(self, other: Point) -> Ordering {
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

```
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
| Struct with raw pointer | NOT Clone unless `unsafe impl` |
| Array/Vec of Clone | Auto-derived (element-wise clone) |
| Handle types | Auto-derived (handle copy, not referent) |

Unsafe impl Clone MUST be explicit and marked unsafe.

### Const Generics

```
fn dot<const N: usize>(a: [f32; N], b: [f32; N]) -> f32
```

Compiler MUST infer `N` from:
- Array literal lengths: `dot([1.0, 2.0], [3.0, 4.0])` → `N = 2`
- Known array types: `arr: [f32; 5]` → `N = 5`

Compiler MUST error if:
- Array lengths differ at call site
- Inference is ambiguous
- Non-literal const expression used without explicit parameter

### Extension Methods

```
// Define extension
fn String.to_snake_case(self) -> String { ... }

// Import extension
import string_utils::String.ext.to_snake_case

// Call
"HelloWorld".to_snake_case()
```

| Resolution Order | Rule |
|------------------|------|
| 1. Type's own method | Always preferred |
| 2. Imported extension | If no type method exists |
| 3. Conflict | Error: "ambiguous method call" |

Compiler MUST resolve type methods before extensions.  
Compiler MUST error if multiple imported extensions match.  
Compiler MUST NOT include extensions in wildcard imports.

### Linear Types in Traits

Linear types MAY be generic parameters.
Pattern matching on `Option<Linear>` MUST bind the value (wildcards forbidden).

| Pattern | Linear Content | Valid |
|---------|----------------|-------|
| `Some(f)` | Binds f | ✅ f must be consumed |
| `Some(_)` | Wildcard | ❌ Compile error |
| `None` | No value | ✅ Nothing to consume |
| `Ok(f)` | Binds f | ✅ f must be consumed |

### Trait Composition

```
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
| Negative bounds | NOT in MVP; workaround via naming convention or separate functions |
| Associated types | NOT in MVP; deferred |
| More than 2 type params | NOT in MVP; traits limited to 1-2 parameters |

### Bounds Requirements

All generic functions must declare their bounds explicitly:

| Function | Requirement |
|----------|-------------|
| Public generic | MUST declare trait bounds |
| Private generic | MUST declare trait bounds |
| Calling a bounded function | Caller must have same or stronger bounds |

```
fn helper<T: Hashable>(item: T) { item.hash() }

pub fn process<T: Hashable>(items: []T) {
    for item in items { helper(item) }  // OK: same bound
}

pub fn bad<T>(items: []T) {
    for item in items { helper(item) }  // ERROR: T not bounded
}
```

Note: Methods on containers (like `[]T.len()`) don't require bounds on T.

### Numeric Literals in Generics

Integer literals auto-coerce to T when `T: Numeric`.
Compiler inserts `T.from_int()` automatically.
IDE shows ghost text indicating the conversion (per Principle 7).

```
trait Numeric<T> {
    add(self, other: T) -> T
    zero() -> T
    one() -> T
    from_int(n: i64) -> T
}

fn increment<T: Numeric>(val: T) -> T {
    val + 1  // Compiler inserts: val + T.from_int(1)
}

fn lerp<T: Numeric>(a: T, b: T, t: T) -> T {
    a * (1 - t) + b * t  // Clean, IDE shows conversions
}
```

## Examples

### Generic Sorting

```
trait Comparable<T> {
    compare(self, other: T) -> Ordering
}

pub fn sort<T: Comparable>(items: mut []T) {
    for i in 1..items.len() {
        let mut j = i
        while j > 0 && items[j] < items[j - 1] {
            swap(mut items[j], mut items[j - 1])
            j = j - 1
        }
    }
}
```

### HashMap with Verified Clone

```
trait Hashable<T> {
    hash(self) -> u64
    eq(self, other: T) -> bool
}

trait HashKey<T>: Hashable<T> + Clone<T> {}

pub struct HashMap<K: HashKey, V> {
    buckets: []Bucket<K, V>
}

pub fn insert<K: HashKey, V>(map: mut HashMap<K, V>, key: K, val: V) {
    let idx = key.hash() % map.buckets.len()
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
- **Runtime polymorphism**: `any Trait` enables heterogeneous collections; see runtime-polymorphism.md

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