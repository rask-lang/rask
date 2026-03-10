// SPDX-License-Identifier: (MIT OR Apache-2.0)

<!-- id: comp.clone-elision -->
<!-- status: proposed -->
<!-- summary: Compiler eliminates unnecessary .clone() calls when original value is unused after the clone -->
<!-- depends: memory/ownership.md, memory/value-semantics.md -->

# Last-Use Clone Elision

When a `.clone()` call is the last use of a value, the compiler replaces the clone with a move. Pure optimization — no semantic change, no user annotation required.

## The Problem

Rask's "no storable references" design means more `.clone()` calls than Rust. The spec acknowledges this (~5% of lines in string-heavy code). But many clones are unnecessary:

```rask
const path = config.path.clone()
do_something(path)
// config.path is never used again — the clone was pointless
```

The compiler can see that `config.path` has no subsequent uses. The clone allocates and copies data that was about to be dropped anyway.

## Rules

| Rule | Description |
|------|-------------|
| **CE1: Last-use detection** | If `x.clone()` is the last use of `x` in all control flow paths, replace the clone with a move of `x` |
| **CE2: Local analysis** | Analysis is per-function. No cross-function last-use tracking |
| **CE3: No semantic change** | Elision doesn't change observable behavior — same result, less allocation |
| **CE4: Control flow aware** | All branches from the clone point must not use `x` again |
| **CE5: IDE annotation** | IDE shows `[clone elided → move]` ghost text when optimization applies |

## Examples

### Simple case — clone then no further use

```rask
func process(config: Config) {
    const name = config.name.clone()   // [clone elided → move]
    send(name)
    // config.name never used again → clone becomes move
}
```

### Branch-aware — all paths must be last-use

```rask
func example(data: Data) {
    const copy = data.items.clone()
    if condition {
        use(copy)
        // data.items NOT used here
    } else {
        use(copy)
        // data.items NOT used here
    }
    // data.items not used after either branch → clone elided
}
```

### NOT elided — subsequent use exists

```rask
func example(data: Data) {
    const copy = data.items.clone()
    use(copy)
    log(data.items)   // data.items used again → clone NOT elided
}
```

### NOT elided — used in one branch

```rask
func example(data: Data, flag: bool) {
    const copy = data.items.clone()
    use(copy)
    if flag {
        log(data.items)   // used in this branch → clone NOT elided
    }
}
```

## Interaction with Other Features

| Feature | Interaction |
|---------|-------------|
| `@unique` types | Clone elision does NOT apply — `@unique` clone has semantic meaning (explicit duplication intent) |
| `@resource` types | Not applicable — resource types aren't Clone |
| Inline closures | Clone inside inline closure eligible if outer scope proves last-use |
| `with` blocks | Clone of collection element inside `with` block — source is frozen, so last-use analysis unaffected |
| Field access | `x.field.clone()` where `x` is moved — elides to partial move if field is independently movable |

## Edge Cases

| Case | Handling |
|------|----------|
| Clone in loop body | Conservative — NOT elided (value used in next iteration) |
| Clone followed by `discard` of original | Elided — `discard` confirms last use |
| Clone of function parameter | Elided if parameter not used after clone |
| Clone where original is shadowed | Elided — shadowing proves no further access to original |
| Clone of Copy type | Clone is already a bitwise copy — for `string`, refcount ops may also be elided (`comp.string-refcount-elision`) |
| Nested clone (`x.clone().clone()`) | Outer clone checked independently |

## Implementation

MIR-level optimization pass:
1. For each `clone()` call on value `x`, find all subsequent uses of `x` in the CFG
2. If no subsequent uses exist on any path from the clone to function exit, replace clone with move
3. Mark `x` as moved at the clone point (invalidates `x` binding)
4. Run after MIR construction, before codegen

Compile-time cost: O(n) per function — single backward pass from clone sites. Negligible.

---

## Appendix (non-normative)

### Rationale

**CE1 (last-use):** This addresses `.clone()` calls from the no-storable-references design. With strings now Copy (immutable, refcounted), remaining clones concentrate on collections (`Vec`, `Map`). Clone elision further reduces those — users still write `.clone()` for clarity, but the compiler eliminates the allocation when it's provably unnecessary.

**CE5 (IDE annotation):** Showing `[clone elided]` helps users learn which clones matter and which don't. Over time, users write fewer unnecessary clones.

### Comparison

Rust is adding similar optimizations (RFC 3680 / "clone ergonomics"). Rask benefits more because the no-storable-references design produces more clone sites.

### See Also

- [Ownership](../memory/ownership.md) — Move semantics (`mem.ownership`)
- [Value Semantics](../memory/value-semantics.md) — Copy vs move threshold (`mem.value`)
- [String Refcount Elision](string-refcount-elision.md) — Atomic op elision for string copies (`comp.string-refcount-elision`)
