<!-- id: type.gradual -->
<!-- status: decided -->
<!-- summary: Non-public functions may omit types and bounds; compiler infers from body -->
<!-- depends: types/traits.md, types/generics.md -->

# Gradual Constraints

Non-public functions may omit parameter types, return types, and bounds. Compiler infers from body using constraint solving. Public functions require full explicit signatures.

## Core Rules

| Rule | Description |
|------|-------------|
| **GC1: Parameter inference** | Compiler examines all parameter uses; single concrete type inferred as concrete, only trait constraints inferred as generic with bounds |
| **GC2: Return inference** | Return type is unified type of all return expressions; incompatible types are a compile error |
| **GC3: Bound inference** | Type parameter used with methods/operators produces structural trait constraints |
| **GC4: Additive annotations** | Explicit types/bounds merge with inferred; conflict is a compile error |
| **GC5: Public enforcement** | `public` functions must have full type annotations and trait bounds |
| **GC6: Module-local scope** | Inference examines only function body — no callers, no cross-module analysis |

| Principle | Rule |
|-----------|------|
| Public = explicit | `public` functions MUST have full type annotations and trait bounds |
| Private = flexible | Non-public functions MAY omit parameter types, return types, and/or bounds |
| Annotations are additive | Explicit types/bounds merge with inferred ones |
| Structural traits apply | Inferred bounds use structural matching, same as explicit bounds |

## Inference Levels

**Level 1 — Fully inferred (prototyping):**

<!-- test: skip -->
```rask
func find_best(items, score_fn) {
    let best = items[0]
    for i in 1..items.len() {
        if score_fn(items[i]) > score_fn(best) {
            best = items[i]
        }
    }
    return best
}
// Inferred: <T: Copy, U: Comparable>(items: Vec<T>, score_fn: |T| -> U) -> T
```

**Level 2 — Partially annotated (solidifying):**

<!-- test: skip -->
```rask
func find_best(items: Vec<Record>, score_fn) -> Record {
    let best = items[0]
    for i in 1..items.len() {
        if score_fn(items[i]) > score_fn(best) {
            best = items[i]
        }
    }
    return best
}
```

**Level 3 — Fully explicit (publishing):**

<!-- test: parse -->
```rask
public func find_best<T: Copy, U: Comparable>(items: Vec<T>, score_fn: |T| -> U) -> T {
    let best = items[0]
    for i in 1..items.len() {
        if score_fn(items[i]) > score_fn(best) {
            best = items[i]
        }
    }
    return best
}
```

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

## Interaction with PascalCase Generics

| Rule | Description |
|------|-------------|
| **PC1: Coexistence** | PascalCase names auto-generic (existing); gradual constraints omit type entirely |

<!-- test: skip -->
```rask
// PascalCase: explicitly name the type parameter
func identity(x: T) -> T { return x }

// Gradual: omit type, let compiler decide
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
     main.rk:45  const result: i32 = compute(items)
```

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

---

## Appendix (non-normative)

### Rationale

**GC1 (parameter inference):** When sketching, focus is logic, not types. Requiring explicit signatures on private helpers adds ceremony that slows exploration without improving safety — compiler checks inferred types identically.

**GC5 (public enforcement):** `public` means "visible to external consumers." Explicit types at this boundary are natural — API contracts should be spelled out. Private functions are implementation details where inference reduces noise.

**GC6 (module-local scope):** Compiler examines one function at a time, collects constraints, solves them. Never looks at callers, never does whole-program analysis, never crosses modules. Preserves compilation speed.

**Ergonomic Delta:** Without gradual constraints, Rask private code needs more annotation than Go or Kotlin. With them, private code matches or beats ceremony of dynamically-typed languages while keeping full static checking.

### Patterns & Guidance

**Prototype-to-production pipeline:**

1. **Prototype:** `func process(data, handler) { ... }` — all inferred
2. **Solidify:** `func process(data: Vec<Record>, handler) { ... }` — partial
3. **Publish:** `public func process<T: Ord>(data: Vec<T>, handler: Handler<T>) -> T` — explicit

Fully statically checked at every stage. Not dynamic typing.

**Interaction with structural traits:** Inferred bounds use structural matching. Body calls `.hash()` and `.eq()` — compiler infers bound requiring those. IDE maps structural bounds to named traits for display.

**Monomorphization:** Inference doesn't change monomorphization. Compiler infers bounds, then monomorphization proceeds as with explicit: each call site generates specialized code. Inferred signature is semantically identical to equivalent explicit.

### IDE Integration

Ghost text displays inferred types, bounds, and return types:

<!-- test: skip -->
```rask
func process(data, handler) {           // ghost: <T: Validatable>(data: Vec<T>, handler: |Vec<T>| -> T) -> T
    const result = handler(data)
    result.validate()
    return result
}
```

Quick actions:
- **"Make signature explicit"** — fills in all inferred types and bounds
- **"Make public"** — adds `public` and fills in the full explicit signature

Hover on a parameter shows its full inferred type. Hover on the function name shows the complete inferred signature.

### See Also

- `type.traits` — Trait definitions and structural matching
- `type.generics` — Generic type parameters and bounds
- `ctrl.comptime` — Compile-time parameters
