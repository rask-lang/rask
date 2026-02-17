<!-- id: comp.semantic-hash -->
<!-- status: decided -->
<!-- summary: Fingerprinting functions for incremental compilation — skip recompiling code that hasn't meaningfully changed -->
<!-- depends: compiler/codegen.md, types/generics.md, control/comptime.md -->

# Semantic Hash Caching

The compiler creates a fingerprint (hash) of each function's simplified code tree — after expanding shortcuts like `a + b` → `a.add(b)` (desugaring), but before type checking. Variable renames, comments, and whitespace are stripped out so cosmetic changes don't trigger recompilation. Each function's fingerprint includes the fingerprints of every function it calls (forming a Merkle tree — any change propagates upward). Specialized generic code is keyed by `(function_identity, type_arguments, semantic_hash)`. Two tiers: per-file parse cache and per-specialization code cache.

## Hash Inputs

| Rule | Description |
|------|-------------|
| **H1: Simplified AST** | Hash operates on the simplified code tree — after expanding operators to method calls, before type checking |
| **H2: Structural content** | AST node kind, control flow structure, literal values, callee identity, type annotations, parameter modes, field names, pattern structure, attributes |
| **H3: Cosmetic exclusion** | Comments, whitespace, local variable names, source spans, internal compiler IDs, import syntax style excluded |
| **H4: Variable normalization** | Local bindings replaced with positional scope indices; renaming produces same hash |

| Included | Example |
|----------|---------|
| AST node kind | `Call` vs `If` vs `Match` |
| Control flow structure | if/else arm count, match arm count |
| Literal values | `42`, `"hello"`, `true` |
| Callee identity | Resolved name + package (`io.print`) |
| Callee hash | Callee's own semantic hash (Merkle tree) |
| Type annotations | `: i32`, `-> string` |
| Parameter modes | `take x: File` |
| Field names | `.health`, `.position` |
| Pattern structure | `Some(x)`, `Point { x, y }` |
| Attributes | `@inline`, `@unsafe` |

| Excluded | Why |
|----------|-----|
| Comments | Not semantically meaningful |
| Whitespace/formatting | Not semantically meaningful |
| Local variable names | Normalized to positional scope indices |
| Source spans | Location is cosmetic |
| Internal compiler IDs | Bookkeeping, not semantics |
| Import syntax style | Resolves identically |

<!-- test: parse -->
```rask
// These two functions have IDENTICAL semantic hashes:
func compute(data: Vec<i32>) -> i32 {
    let total = 0
    for item in data { total += item }
    return total
}

func compute(items: Vec<i32>) -> i32 {
    let sum = 0
    for element in items { sum += element }
    return sum
}
```

## Merkle Tree

| Rule | Description |
|------|-------------|
| **MK1: Callee incorporation** | Each function's hash incorporates its direct callees' hashes |
| **MK2: Transitive propagation** | A change anywhere propagates upward automatically — if a helper function changes, everything calling it gets a new fingerprint |
| **MK3: Topological order** | Hashes computed in reverse topological order (leaves first) |
| **MK4: SCC grouping** | Mutually recursive functions hashed as single group; any change invalidates all members |

```
sort<T> ──hash includes──→ swap() ──hash includes──→ compare()
```

If `compare()` changes: `compare()` hash changes, `swap()` hash changes, `sort<T>` hash changes, all cached `sort<T>` instantiations invalidated.

## Cache Keys

| Rule | Description |
|------|-------------|
| **CK1: Non-generic key** | `(package_id, function_id, semantic_hash)` |
| **CK2: Generic key** | `(source_package_id, function_id, [type_arguments], body_semantic_hash, [type_definition_hashes])` |
| **CK3: Type definition hashes** | Changes to type definition affect monomorphized code even when generic body unchanged |

## Cache Tiers

| Rule | Description |
|------|-------------|
| **CT1: Package tier** | Parsed + resolved AST cached per file; keyed by file content hash |
| **CT2: Specialization tier** | Type-checked, ownership-verified specialized AST cached; keyed by full composite key |
| **CT3: Quick path** | If no source files changed and upstream metadata unchanged, skip entire package |

| Tier | Cached artifact | Cache key | When invalidated |
|------|----------------|-----------|-----------------|
| Package | Parsed + resolved AST per file | File content hash | Source file bytes change |
| Specialization | Type-checked, ownership-verified specialized AST | Full composite key (CK2) | Any component of cache key changes |

## Invalidation

| Rule | Description |
|------|-------------|
| **IV1: Cosmetic changes** | Whitespace, comments, variable renames do not invalidate |
| **IV2: Body logic change** | Invalidates that function's fingerprint + all callers (propagates up the tree) |
| **IV3: Signature change** | Public signature change forces downstream recompile |
| **IV4: Type definition change** | Struct field added/removed invalidates all specializations using that type |
| **IV5: Trait change** | Trait method added/changed invalidates all generic functions bounded by that trait |
| **IV6: Compiler version** | Entire cache invalidated on version mismatch |
| **IV7: Build profile** | Debug/release does not invalidate instantiation tier (profile-independent) |

## Cross-Package Protocol

| Rule | Description |
|------|-------------|
| **CP1: Metadata export** | Each compiled package produces metadata with public function and type hashes |
| **CP2: Dependency order** | Packages compile in dependency order; downstream reads upstream metadata |
| **CP3: Metadata diff** | If upstream metadata byte-identical to previous build, downstream cache fully valid |
| **CP4: Per-function comparison** | If metadata changed, compare per-function hashes; only recompile affected functions |

**Package metadata contents:**

| Field | Purpose |
|-------|---------|
| `package_id` | Package identity |
| `compiler_version` | Cache version stamp |
| `public_functions` | Map of function ID to (signature_hash, body_hash, is_generic, type_params) |
| `public_types` | Map of type ID to definition_hash |

## Comptime Memoization

| Rule | Description |
|------|-------------|
| **CM1: Pure caching** | Comptime functions are pure; results cached by `(function_id, arguments_hash, body_semantic_hash)` |
| **CM2: Result propagation** | Comptime results feed into enclosing function's semantic hash |

## Cache Storage

| Rule | Description |
|------|-------------|
| **CS1: Location** | `.rk-cache/` at project root (gitignored) |
| **CS2: Version stamp** | Compiler version mismatch discards entire cache |
| **CS3: Commands** | `rask cache clean` removes all; `rask cache stats` shows size and hit/miss rates |

## Edge Cases

| Case | Handling | Rule |
|------|---------|------|
| Mutually recursive functions | Hash as SCC group; any change invalidates all | MK4 |
| Generic calls another generic | Hash includes callee's generic body hash (not instantiated hash) | MK1 |
| Closure captures | Capture list part of AST structure, included in hash | H2 |
| Default parameter values | Default expression hashed as part of function signature | H2 |
| Trait default methods | Hashed separately; override produces different hash than default | IV5 |
| `any Trait` dispatch | Not specialized per type, not cached at specialization tier | CK2 |
| `comptime if` branches | Both branches hashed (dead branch elimination is codegen concern) | H2 |
| `unsafe` blocks | Hashed normally (semantically meaningful) | H2 |
| Cross-package private function | Not exported in metadata | CP1 |
| Build-script generated code | Treated as normal source; content hash is cache key | CT1 |
| `@embed_file` content changes | File content hash is part of comptime evaluation cache key | CM1 |
| No source files changed | Quick path: skip entire package | CT3 |
| New package (no cache) | Full compilation; cache populated for next build | CT1 |
| Cache corruption | Detect via checksums; discard and recompile | CS2 |

## Error Messages

```
ERROR [comp.semantic-hash/CK2]: cache key mismatch for `sort<i32>`
   |
   type definition hash changed for `Point` (field added)
   |

WHY: Type layout changes affect monomorphized code even when the generic body is unchanged.

FIX: This is expected. Recompilation happens automatically.
```

---

## Appendix (non-normative)

### Rationale

**H1 (simplified AST):** The simplified (desugared) AST is the first semantically stable representation. Operators are normalized to method calls (`a + b` becomes `a.add(b)`), but type information isn't injected yet. Hashing post-typecheck would be fragile to type inference implementation changes. Hashing pre-simplification would make `a + b` and `a.add(b)` produce different hashes for identical semantics.

**CT2 (cache specialized AST, not machine code):** Machine code depends on optimization level and target architecture. Type-checked, ownership-verified specialized AST is valid across debug/release and different targets. Codegen is fast. Type checking and ownership verification are expensive.

**MK4 (SCC grouping):** Mutual recursion means behaviors are intertwined. Conservative but correct to invalidate all members on any change.

### Patterns & Guidance

**Cross-package rebuild example:**

<!-- test: skip -->
```rask
// Package: collections
public func sort<T: Comparable>(items: Vec<T>) { ... }

// Package: myapp
import collections
func process() {
    const data = Vec<i32>.new()
    collections.sort(data)  // monomorphizes sort<i32>
}
```

Build 1: `collections` exports `sort` body hash = `0xABCD`. `myapp` caches `sort<i32>` with that key.

Build 2: Developer changes `sort`'s algorithm. Fingerprint changes to `0xEF01`. Cache miss -- recompile.

Build 3: Developer renames a local inside `sort`. Fingerprint stays `0xEF01` (variable names normalized). Cache hit -- skip.

### See Also

- `comp.codegen` — Pipeline where code specialization and caching integrate
- `type.generics` — Code specialization (monomorphization) that semantic hashing optimizes
- `ctrl.comptime` — Comptime memoization uses same hash infrastructure
