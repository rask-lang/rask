# Solution: Gradual Constraints

## The Question

Must every function fully specify its parameter types, return types, and trait bounds? Or can the compiler infer missing pieces from the function body?

## Decision

Non-public functions may omit parameter types, return types, and generic trait bounds. The compiler infers them from the function body using constraint solving. Public functions must have full explicit signatures — types and bounds are part of the API contract. Explicit annotations are additive: they merge with inferred constraints and never conflict.

This enables a frictionless prototype-to-production pipeline:
1. **Prototype:** `func process(data, handler) { ... }` — all inferred
2. **Solidify:** `func process(data: Vec<Record>, handler) { ... }` — partial types
3. **Publish:** `public func process<T: Ord>(data: Vec<T>, handler: Handler<T>) -> T` — fully explicit

The program is fully statically checked at every stage. This is not dynamic typing.

## Rationale

**Prototyping friction.** When sketching an algorithm, the programmer's focus is logic, not types. Requiring explicit signatures on every private helper adds ceremony that slows exploration without improving safety — the compiler checks inferred types identically to explicit ones.

**Ergonomic Delta.** Without gradual constraints, Rask private code requires more annotation than Go or Kotlin for equivalent functionality. With gradual constraints, private Rask code matches or beats the ceremony level of dynamically-typed languages while retaining full static checking.

**Body-local inference is local analysis.** The compiler examines one function body at a time, collects constraints, and solves them. It never looks at callers, never performs whole-program analysis, and never crosses module boundaries. This preserves compilation speed (CS ≥ 5× Rust target).

**Public boundary as contract.** The `public` keyword already means "this is visible to external consumers." Requiring explicit types at this boundary is natural — API contracts should be spelled out. Private functions are implementation details where inference reduces noise.

## Specification

### Core Principles

| Principle | Rule |
|-----------|------|
| Public = explicit | `public` functions MUST have full type annotations and trait bounds |
| Private = flexible | Non-public functions MAY omit parameter types, return types, and/or bounds |
| Inference is body-local | Compiler infers from function body only, never from callers |
| Annotations are additive | Explicit types/bounds merge with inferred ones; conflict is a compile error |
| Structural traits apply | Inferred bounds use structural matching, same as explicit bounds |
| IDE shows all | Ghost text displays inferred types, bounds, and return types (Principle 7) |

### Inference Levels

**Level 1 — Fully inferred (prototyping):**

```rask
func find_best(items, score_fn) {
    let best = items[0]
    for i in 1..items.len() {
        if score_fn(items[i]) > score_fn(best) {
            best = items[i]
        }
    }
    best
}
```

The compiler infers:
- `items`: `Vec<T>` (indexed, has `.len()`)
- `score_fn`: `func(T) -> U` (called with element, result compared with `>`)
- Return type: `T`
- Bounds: `T: Copy` (assigned in loop), `U: Comparable` (compared with `>`)

**Level 2 — Partially annotated (solidifying):**

```rask
func find_best(items: Vec<Record>, score_fn) -> Record {
    let best = items[0]
    for i in 1..items.len() {
        if score_fn(items[i]) > score_fn(best) {
            best = items[i]
        }
    }
    best
}
```

`items` and return type are explicit. `score_fn` type is still inferred from usage.

**Level 3 — Fully explicit (publishing):**

```rask
public func find_best<T: Copy, U: Comparable>(items: Vec<T>, score_fn: func(T) -> U) -> T {
    let best = items[0]
    for i in 1..items.len() {
        if score_fn(items[i]) > score_fn(best) {
            best = items[i]
        }
    }
    best
}
```

All types, bounds, and return type explicit. Required for `public`. The function body is unchanged across all three levels.

### Concrete vs Generic Inference

When inferring types from a function body, the compiler prefers **the most general type that satisfies all constraints:**

| Example | Inferred As | Why |
|---------|-------------|-----|
| `func double(x) { x * 2 }` | `<T: Numeric>(x: T) -> T` | `*` desugars to trait method `.mul()` |
| `func get_port() { 8080 }` | `() -> i32` | Literal with default type, no trait-method usage |
| `func add(a, b) { a + b }` | `<T: Numeric>(a: T, b: T) -> T` | `+` desugars to `.add()` |
| `func greet(name) { println("Hi, {name}") }` | `(name: string)` | String interpolation constrains to `string` |
| `func len(items) { items.len() }` | `<T>(items: Vec<T>) -> usize` | `.len()` doesn't constrain `T` |

**Rule:** If the only type information comes from a literal's default type with no trait-method usage on parameters, infer concrete. If constraints come from trait methods, operators, or function calls that require generic bounds, infer generic.

### Inference Rules

**GC1: Parameter type inference.** When a parameter has no type annotation, the compiler examines all uses of that parameter in the body. Each use constrains the type:
- `param.len()` — param has a `.len()` method
- `param + 1` — param satisfies `Numeric` (via operator desugaring to `.add()`)
- Passing `param` to `known_func(x: Vec<i32>)` — param is `Vec<i32>`

If uses produce a single concrete type, that is the inferred type. If uses produce only trait constraints, the parameter becomes generic with those bounds.

**GC2: Return type inference.** The compiler examines all `return` expressions. The return type is the unified type of all return statements. If return statements produce incompatible types, this is a compile error.

```rask
// OK: both branches return the same type
func abs(x) { if x < 0: -x else: x }

// ERROR: branches return incompatible types
func bad(x) { if x > 0: x else: "negative" }
```

**GC3: Bound inference.** When a type parameter is used with methods or operators, the compiler collects the required methods into structural trait constraints:

```rask
func sort_and_print(items) {
    items.sort()                 // requires Comparable on element type
    for i in items {
        println(items[i].display())   // requires Display on element type
    }
}
// Compiler infers: items: Vec<T> where T: Comparable + Display
```

**GC4: Additive annotations.** Explicit annotations are additional constraints merged with inferred ones. The compiler checks that explicit types satisfy inferred requirements:

```rask
// OK: Vec<Record> satisfies all inferred constraints
func process(data: Vec<Record>, handler) {
    handler(data)
}

// ERROR: explicit type conflicts with body usage
func transform(x: i32) {
    x.display()        // i32 does not have method 'display'
}
```

**GC5: Public enforcement.** Adding `public` to a function without full annotations is a compile error. The compiler suggests the inferred signature:

```
error: public function 'process' requires explicit type annotations
  --> utils.rask:1
  | public func process(data, handler) {
  |                     ^^^^  ^^^^^^^ add type annotations

  Inferred signature:
    public func process<T: Validatable>(data: Vec<T>, handler: func(Vec<T>) -> T) -> T

  hint: apply suggested signature? (IDE quick action)
```

**GC6: Module-local scope.** Inference examines only the function body. It does NOT examine callers, does NOT perform whole-program analysis, and does NOT cross module boundaries. The algorithm:
1. Walk the function body AST
2. Collect type constraints from each expression
3. Solve constraints via unification
4. Produce concrete types or generic bounds
5. Check explicit annotations against inferred constraints

### Smart Error Messages

When inferred bounds change due to body edits, the compiler reports what changed, which line caused it, and which callers break.

**Error 1: New bound required**

```
error: function 'helper' now requires Display on its parameter

  6 | func helper(item) {
  7 |     println(item.display())    // <-- requires Display (NEW)
                  ^^^^^^^^^^^^^^^
  8 |     item.hash()                // <-- requires Hashable (existing)
  9 | }

  Previous bounds: Hashable
  Current bounds:  Hashable + Display

  Callers that no longer match:
    utils.rask:20  helper(my_key)    // Key has Hashable but not Display
```

**Error 2: Inferred type changed**

```
error: inferred return type of 'compute' changed

  Before: func compute(data: Vec<i32>) -> i32
  After:  func compute(data: Vec<i32>) -> f64

  Caused by:
  12 | data.sum() / 2.5
                    ^^^ f64 literal changed inferred return type

  Callers that break:
    main.rask:45  const result: i32 = compute(items)
                        ^^^^^^^^^^^ expected i32, got f64
```

**Error 3: Additive conflict**

```
error: explicit annotation conflicts with body usage

  3 | func transform(x: i32) {
                        ^^^   annotated as i32
  5 |     x.display()
           ^^^^^^^^^ i32 does not have method 'display'

  hint: change parameter type, or remove the .display() call
```

### Interaction with Implicit PascalCase Generics

SYNTAX.md defines that unknown PascalCase names in type position become type parameters. Gradual constraints extend this — when a parameter has no type at all, the compiler infers whether it is concrete or generic.

```rask
// Existing: T is auto-generic because it is PascalCase and unknown
func identity(x: T) -> T { x }

// New with gradual constraints: type entirely omitted
func identity(x) { x }
// Compiler infers: func identity<T>(x: T) -> T
```

Both forms coexist. With PascalCase generics, you explicitly name a type parameter. With gradual constraints, you omit the type entirely and let the compiler decide.

### Interaction with Structural Traits

Inferred bounds use structural matching — exactly the same mechanism as explicit bounds. If the body calls `.hash()` and `.eq()`, the compiler infers a bound requiring those methods. The IDE maps structural bounds to named traits for display:

```rask
func lookup(table, key) {
    const idx = key.hash() % table.len()
    table[idx]
}
// Compiler infers structural bounds:
//   key needs: hash(self) -> u64
//   table needs: indexed access, .len()
// IDE displays: func lookup<K: Hashable>(table: Vec<K>, key: K) -> K
```

### Edge Cases

| Case | Handling |
|------|----------|
| Recursive functions | Inferred from base case + recursive structure. If ambiguous, compiler requires annotation on at least the return type |
| Mutual recursion | Both functions analyzed together (SCC). If cycle cannot resolve, compiler error requiring annotation on at least one function |
| Closures | Closure parameter types already inferred from context (unchanged). Gradual constraints apply to the enclosing function's signature |
| `any Trait` | Cannot be inferred from structural usage — if dynamic dispatch is needed, it must be explicit |
| `comptime` parameters | Must be explicit — compilation requires knowing them upfront |
| Empty function body | No constraints inferred. Parameters are unconstrained generics. Return type is `()` |
| Multiple return types | If different branches return incompatible types, compile error (same as with explicit types) |
| `extern` functions | Must have full explicit signatures (C ABI requires it) |

### Monomorphization

Inference does not change monomorphization. The compiler infers the bounds, then monomorphization proceeds exactly as with explicit bounds: each call site with concrete types generates specialized code, and trait satisfaction is checked at the monomorphization site.

**Guarantee:** An inferred signature is semantically identical to the equivalent explicit signature. Changing from inferred to explicit (with the same types) produces identical compiled output.

### IDE Integration (Principle 7)

The IDE displays inferred information as ghost text:

```rask
func process(data, handler) {           // ghost: <T: Validatable>(data: Vec<T>, handler: func(Vec<T>) -> T) -> T
    const result = handler(data)
    result.validate()
    result
}
```

Quick actions:
- **"Make signature explicit"** — fills in all inferred types and bounds
- **"Make public"** — adds `public` and fills in the full explicit signature

Hover on a parameter shows its full inferred type. Hover on the function name shows the complete inferred signature.

### Metrics Impact

| Metric | Impact | Notes |
|--------|--------|-------|
| ED (Ergonomic Delta) | Improved | Private code matches Python/Go ceremony level |
| CS (Compilation Speed) | Slight decrease | Per-function constraint solving adds work, but no whole-program analysis |
| SN (Syntactic Noise) | Reduced | Fewer ceremony tokens in private code |
| PI (Predictability Index) | Maintained | IDE ghost text ensures inferred info is always visible |
| TC (Transparency Coefficient) | Unchanged | Cost transparency is about operations, not type annotations |

## Examples

### Data Pipeline Prototyping

```rask
func extract(source) {
    source.read_all()
}

func transform(data) {
    data.lines().filter(|line| line.len() > 0).collect()
}

func load(dest, records) {
    for r in records {
        dest.write(r)
    }
}

// Later: solidify the public interface
public func run_pipeline(source: FileReader, dest: FileWriter) -> () or IoError {
    const raw = try extract(source)
    const records = transform(raw)
    load(dest, records)
}
```

### Private Helpers

```rask
// Bounds inferred — compiler derives: <T: Comparable>(val: T, min_val: T, max_val: T) -> T
func clamp(val, min_val, max_val) {
    if val < min_val: min_val
    else if val > max_val: max_val
    else: val
}

// Bounds inferred — compiler derives: <T>(items: Vec<T>, predicate: func(T) -> bool) -> Option<usize>
func find_first(items, predicate) {
    for i in 0..items.len() {
        if predicate(items[i]): return Some(i)
    }
    None
}
```

### Partial Annotations

```rask
func format_table(headers: Vec<string>, rows) -> string {
    // headers is explicit, rows is inferred from usage
    const widths = headers.iter().map(|h| h.len()).collect()
    let output = string.new()
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            output.push_str(cell.display().pad_right(widths[i]))
        }
        output.push('\n')
    }
    output
}
// Compiler infers: rows: Vec<Vec<T>> where T: Display
```

### Gradual Solidification

```rask
// Step 1: Prototyping — logic first, types later
func parse(input) {
    const parts = input.split(",")
    Record.new(parts[0], parts[1].parse_i32())
}

// Step 2: Add the type you know, leave the rest
func parse(input: string) {
    const parts = input.split(",")
    Record.new(parts[0], parts[1].parse_i32())
}

// Step 3: Ready for public API
public func parse(input: string) -> Record or ParseError {
    const parts = input.split(",")
    Record.new(parts[0], try parts[1].parse_i32())
}
```

## Integration Notes

- **Memory model:** Inference does not change ownership semantics. Parameter modes (borrow, read, take) are inferred from body usage, same as with explicit types.
- **Type system:** Inferred bounds are structural (same as explicit structural bounds). If an `explicit trait` is required, the programmer must annotate it.
- **Concurrency:** Closures captured for `spawn` must own their data — inference cannot relax this requirement.
- **Compiler:** Constraint solving is per-function, per-module. No cross-module inference. Incremental compilation preserved.
- **C interop:** `extern` functions always require full signatures (C ABI demands it).
- **Error handling:** `try` propagation in inferred functions works normally — the error type is inferred as part of the return type (`Result<T, E>` where `E` is the union of propagated errors).
- **Generics spec:** The rule "ALL generic functions MUST declare trait constraints" is softened to "ALL *public* generic functions MUST declare trait constraints" with a reference to this spec.
