# Semantic Hash Caching

## The Question
How does compiler avoid recompiling generic instantiations that haven't meaningfully changed? What constitutes "meaningful change"? How do changes propagate across package boundaries? What gets cached?

## Decision
Compiler computes structural hash of each function's desugared AST, normalizing away cosmetic differences (comments, whitespace, local variable names). Each function's hash incorporates direct callees' hashes, forming Merkle tree where changes propagate upward through call graph. Monomorphized code keyed by `(function_identity, type_arguments, semantic_hash)`. Two-tier caching: per-file parse cache (keyed by file content) and per-instantiation monomorphization cache (keyed by full composite key).

## Rationale
Monomorphization produces fast runtime code but creates incremental compilation problem: changing generic function's body forces recompilation of ALL call sites across ALL packages. Without mitigation, monomorphization becomes compile-time bottleneck.

Semantic hashing solves this by detecting when function body hasn't *meaningfully* changed—renaming local variable or adding comment shouldn't force downstream recompilation. Merkle tree structure handles transitive dependencies naturally: if `sort` calls `swap` and `swap` changes, `sort`'s hash changes automatically because it incorporates `swap`'s hash.

**Why hash desugared AST?** Desugared AST is first semantically stable representation—operators normalized to method calls (`a + b` → `a.add(b)`), but type information not injected yet. Hashing post-typecheck would be fragile to type inference implementation changes. Hashing pre-desugar would make `a + b` and `a.add(b)` produce different hashes for identical semantics.

**Why cache monomorphized AST, not machine code?** Machine code depends on optimization level and target architecture. Caching monomorphized, type-checked, ownership-verified AST means cache valid across debug/release builds and different targets. Codegen is fast. Type checking and ownership verification are expensive.

## Specification

### What Is Hashed

Semantic hash operates on **desugared AST**—after operator desugaring but before type checking.

**Included in hash:**

| Element | What contributes | Example |
|---------|-----------------|---------|
| AST node kind | Discriminant tag | `Call` vs `If` vs `Match` |
| Control flow structure | Nesting and ordering | `if/else` arm count, `match` arm count |
| Literal values | Exact value | `42`, `"hello"`, `true` |
| Callee identity | Resolved name + package | `io.print` |
| Callee hash | Callee's own semantic hash | See Merkle Tree section |
| Type annotations | Explicit type in source | `: i32`, `-> string` |
| Parameter modes | Borrow vs take | `take x: File` |
| Field names | String identity | `.health`, `.position` |
| Pattern structure | Kind + nesting | `Some(x)`, `Point { x, y }` |
| Attributes | Attribute identity | `@inline`, `@unsafe` |

**Excluded from hash:**

| Element | Why excluded |
|---------|-------------|
| Comments | Not semantically meaningful |
| Whitespace/formatting | Not semantically meaningful |
| Local variable names | Normalized to positional scope indices |
| Source spans (line/column) | Location is cosmetic |
| Internal compiler IDs (NodeId) | Bookkeeping, not semantics |
| Import syntax style | `import pkg.Foo` and `import pkg; pkg.Foo` resolve identically |

### Variable Normalization

Local bindings replaced with positional indices based on introduction order within each scope. Renaming variable produces same hash.

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

Each scope tracks counter. When binding introduced (`const x = ...` or `let y = ...`), gets next index. References to that binding use same index. Nested scopes start new counter but include enclosing scope depth.

### Merkle Tree: Transitive Dependency Hashing

When function calls another function, callee's semantic hash incorporated into caller's hash. Since callee's hash already incorporates *its* callees' hashes, this forms Merkle tree: change at any depth propagates upward automatically.

```
sort<T> ──hash includes──→ swap() ──hash includes──→ compare()
```

If `compare()` changes:
1. `compare()`'s hash changes
2. `swap()`'s hash changes (it includes `compare()`'s hash)
3. `sort<T>`'s hash changes (it includes `swap()`'s hash)
4. All cached instantiations of `sort<T>` are invalidated

**Computation order:** Within package, hashes computed in reverse topological order of call graph (leaf functions first, then callers). Each function's callees already hashed when processed.

#### Mutually Recursive Functions

Mutually recursive functions form strongly connected component (SCC) in call graph. Hashed as group:

1. Identify all SCCs in intra-package call graph
2. For each SCC, hash all member functions together into single combined hash
3. All members share this combined hash
4. Any change to any member invalidates all members

Conservative but correct—mutual recursion means behaviors are intertwined.

### Cache Key Structure

**Non-generic functions (package-tier caching):**

```
(package_id, function_id, semantic_hash)
```

**Generic instantiations (instantiation-tier caching):**

```
(source_package_id, function_id, [type_arguments], body_semantic_hash, [type_definition_hashes])
```

`type_definition_hashes` necessary because changes to type definition affect monomorphized code even when generic function body unchanged. If `struct Point` gains field, `sort<Point>` must recompile.

### What Is Cached (Two Tiers)

| Tier | Cached artifact | Cache key | When invalidated |
|------|----------------|-----------|-----------------|
| **Package** | Parsed + resolved AST per file | File content hash | Source file bytes change |
| **Instantiation** | Monomorphized, type-checked, ownership-verified AST | Full composite key | Any component of cache key changes |

**Package tier:** If source file content hash unchanged, skip parsing and name resolution entirely. Simple byte-level check—no semantic analysis needed.

**Instantiation tier:** Monomorphized AST is result of substituting concrete types into generic function, type-checking result, verifying ownership. Expensive work caching avoids.

### Invalidation Rules

| What changed | What's invalidated |
|-------------|-------------------|
| Whitespace/comment only | Nothing (normalized out of hash) |
| Local variable renamed | Nothing (positional indices) |
| Function body logic | That function's hash + all callers via Merkle propagation |
| Public function signature | All downstream packages (forced recompile) |
| Private function body | Same-package callers (if inferred signature changes, their callers too) |
| Struct field added/removed | Type definition hash changes → all instantiations using that type |
| Trait method added/changed | Trait hash changes → all generic functions bounded by that trait |
| Compiler version | Entire cache (version stamp mismatch) |
| Build profile (debug/release) | Nothing at instantiation tier (profile-independent) |

### Cross-Package Protocol

Each compiled package produces metadata containing its public function and type hashes.

**Package metadata contents:**

| Field | Purpose |
|-------|---------|
| `package_id` | Identifies the package |
| `compiler_version` | For cache version stamping |
| `public_functions` | Map of function ID → (signature_hash, body_hash, is_generic, type_params) |
| `public_types` | Map of type ID → definition_hash |

**Build flow:**

1. Packages compile in dependency order (A before B if B depends on A)
2. A produces metadata with its public function/type hashes
3. B reads A's metadata during compilation
4. B incorporates A's function hashes when computing hashes for B's functions that call into A
5. B stores its monomorphization cache entries keyed partly by A's function hashes

**Incremental rebuild:**

1. A recompiles (if sources changed), producing new metadata
2. If A's metadata is **byte-identical** to previous build: B's cache is fully valid — skip B entirely
3. If A's metadata changed: B compares per-function hashes. Only functions whose callee hashes changed need recompilation. Unchanged instantiations are served from cache.

**Example:**

```rask
// Package: collections
public func sort<T: Comparable>(items: Vec<T>) { ... }

// Package: myapp
import collections

func process() {
    const data = Vec<i32>.new()
    // ...
    collections.sort(data)  // monomorphizes sort<i32>
}
```

Build 1: `collections` exports `sort` body hash = `0xABCD`. `myapp` monomorphizes `sort<i32>`, caches with key including `0xABCD`.

Build 2: Developer changes `sort`'s algorithm. `collections` exports hash = `0xEF01`. `myapp` cache miss on `sort<i32>` — recompile.

Build 3: Developer renames a local variable inside `sort`. `collections` exports hash = `0xEF01` (unchanged — variable names are normalized). `myapp` cache hit — skip monomorphization.

### Comptime Memoization

Comptime functions are pure (no I/O, no side effects). Their results can be cached:

```
ComptimeCacheKey = (function_id, arguments_hash, body_semantic_hash)
```

Identical inputs with identical body hash always produce identical outputs. The cache stores the computed result (the frozen value). If a comptime function's body changes, its hash changes and all cached results are invalidated.

Comptime results feed into the semantic hash of the enclosing function. If a comptime result changes, the enclosing function's hash changes too.

### Cache Storage

Cached artifacts live in `.rk-cache/` at the project root (gitignored).

**Requirements:**
- Compiler version stamp — mismatch discards entire cache
- Per-package organization for locality
- `rask cache clean` removes all cached artifacts
- `rask cache stats` shows cache size and hit/miss rates

**Implementation note:** Binary serialization format and directory structure are implementation details, not part of specification. Fast binary format (e.g., bincode) and per-package subdirectories recommended.

### Quick Path: No Changes

Before computing semantic hashes, compiler checks whether any source file in package has changed (by content hash). If no files changed and all upstream package metadata unchanged, entire package compilation skipped. Common case (no changes) essentially free.

## Edge Cases

| Case | Handling |
|------|---------|
| Mutually recursive functions | Hash as SCC group; any change invalidates all members |
| Generic function calls another generic | Hash includes callee's generic body hash (not an instantiated hash) |
| Closure captures | Capture list is part of AST structure, included in hash |
| Default parameter values | Default expression hashed as part of function signature |
| Trait default methods | Hashed separately; an override produces a different hash than the default |
| `any Trait` dispatch | Not monomorphized, not cached at instantiation tier |
| `comptime if` branches | Both branches hashed (dead branch elimination is a codegen concern) |
| `unsafe` blocks | Hashed normally (`unsafe` is semantically meaningful) |
| Cross-package private function | Cannot be called cross-package; not exported in metadata |
| Build-script generated code | Treated as normal source files; content hash of generated file is the cache key |
| `@embed_file` content changes | File content hash is part of the comptime evaluation cache key |
| No source files changed | Quick path: skip entire package compilation |
| New package (no cache) | Full compilation; cache populated for next build |
| Cache corruption | Detect via checksums; discard and recompile (cache is always reconstructible) |

## Integration Notes

- **Module System:** Package metadata files are produced alongside compiled output. Import resolution reads metadata to get function/type hashes for cross-package Merkle tree computation.
- **Generics:** Semantic hash caching is primary mitigation for monomorphization's incremental build cost. Without it, any change to generic function body forces recompilation of ALL call sites across ALL packages. See [generics.md](../types/generics.md).
- **Comptime:** Comptime memoization uses same semantic hash infrastructure. Pure comptime functions cached by `(function, arguments, body_hash)`. Resolves memoization question from [comptime.md](../control/comptime.md) Remaining Issues.
- **Type System:** Type definition hashes are part of instantiation cache key. Struct layout changes invalidate affected monomorphizations even when generic function body unchanged.
- **Local Analysis:** Hash computation is function-local plus callee identities/hashes. No whole-program analysis. Packages export hashes as metadata, maintaining compilation boundary.
- **Concurrency:** Hash computation embarrassingly parallel per-function within package (after topological sort). Cross-package hashing follows existing parallel compilation model (independent packages in parallel).
- **Tooling:** `rask build --cache-stats` shows hit/miss rates. IDEs MAY show "cached" annotations on functions skipped in next build.

## Remaining Issues

### Medium Priority
1. **Shared instantiation deduplication** — If packages A and B both instantiate `sort<i32>`, should the linker deduplicate them? This affects binary size, not compilation speed.
2. **Distributed cache** — Could teams share a cache server (like Bazel's remote cache)? Requires deterministic hashing across machines.

### Low Priority
3. **Profiling-guided cache eviction** — Should rarely-used cache entries be evicted to save disk space? Probably not worth the complexity.
