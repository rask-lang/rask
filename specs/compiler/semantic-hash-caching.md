# Solution: Semantic Hash Caching

## The Question
How does the compiler avoid recompiling generic instantiations that haven't meaningfully changed? What constitutes a "meaningful change"? How do changes propagate across package boundaries, and what exactly is cached?

## Decision
The compiler computes a structural hash of each function's desugared AST, normalizing away cosmetic differences (comments, whitespace, local variable names). Each function's hash incorporates its direct callees' hashes, forming a Merkle tree where changes propagate upward through the call graph. Cached monomorphized code is keyed by `(function_identity, type_arguments, semantic_hash)`. Two-tier caching: per-file parse cache (keyed by file content) and per-instantiation monomorphization cache (keyed by the full composite key).

## Rationale
Monomorphization produces fast runtime code but creates an incremental compilation problem: changing a generic function's body forces recompilation of ALL call sites across ALL packages. Without mitigation, this makes monomorphization a compile-time bottleneck.

Semantic hashing solves this by detecting when a function's body hasn't *meaningfully* changed — renaming a local variable or adding a comment shouldn't force downstream recompilation. The Merkle tree structure handles transitive dependencies naturally: if `sort` calls `swap` and `swap` changes, `sort`'s hash changes automatically because it incorporates `swap`'s hash.

**Why hash the desugared AST?** The desugared AST is the first representation that is semantically stable — operators have been normalized to method calls (`a + b` → `a.add(b)`), but type information hasn't been injected yet. Hashing post-typecheck would be fragile to type inference implementation changes. Hashing pre-desugar would make `a + b` and `a.add(b)` produce different hashes for identical semantics.

**Why cache monomorphized AST, not machine code?** Machine code depends on optimization level and target architecture. Caching the monomorphized, type-checked, ownership-verified AST means the cache is valid across debug/release builds and across different targets. Codegen is fast; type checking and ownership verification are the expensive phases.

## Specification

### What Is Hashed

The semantic hash operates on the **desugared AST** — after operator desugaring but before type checking.

**Included in hash:**

| Element | What contributes | Example |
|---------|-----------------|---------|
| AST node kind | Discriminant tag | `Call` vs `If` vs `Match` |
| Control flow structure | Nesting and ordering | `if/else` arm count, `match` arm count |
| Literal values | Exact value | `42`, `"hello"`, `true` |
| Callee identity | Resolved name + package | `std.io.print` |
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

Local bindings are replaced with positional indices based on their introduction order within each scope. This ensures renaming a variable produces the same hash.

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

Each scope tracks a counter. When a binding is introduced (`const x = ...` or `let y = ...`), it gets the next index. References to that binding use the same index. Nested scopes start a new counter but include the enclosing scope's depth.

### Merkle Tree: Transitive Dependency Hashing

When a function calls another function, the callee's semantic hash is incorporated into the caller's hash. Since the callee's hash already incorporates *its* callees' hashes, this forms a Merkle tree: a change at any depth propagates upward automatically.

```
sort<T> ──hash includes──→ swap() ──hash includes──→ compare()
```

If `compare()` changes:
1. `compare()`'s hash changes
2. `swap()`'s hash changes (it includes `compare()`'s hash)
3. `sort<T>`'s hash changes (it includes `swap()`'s hash)
4. All cached instantiations of `sort<T>` are invalidated

**Computation order:** Within a package, hashes are computed in reverse topological order of the call graph (leaf functions first, then their callers). This ensures each function's callees are already hashed when it's processed.

#### Mutually Recursive Functions

Mutually recursive functions form a strongly connected component (SCC) in the call graph. These are hashed as a group:

1. Identify all SCCs in the intra-package call graph
2. For each SCC, hash all member functions together into a single combined hash
3. All members of the SCC share this combined hash
4. Any change to any member invalidates all members of the SCC

This is conservative but correct — mutual recursion means the functions' behaviors are intertwined.

### Cache Key Structure

**Non-generic functions (package-tier caching):**

```
(package_id, function_id, semantic_hash)
```

**Generic instantiations (instantiation-tier caching):**

```
(source_package_id, function_id, [type_arguments], body_semantic_hash, [type_definition_hashes])
```

The `type_definition_hashes` are necessary because changes to a type's definition affect monomorphized code even when the generic function's body is unchanged. If `struct Point` gains a field, `sort<Point>` must be recompiled.

### What Is Cached (Two Tiers)

| Tier | Cached artifact | Cache key | When invalidated |
|------|----------------|-----------|-----------------|
| **Package** | Parsed + resolved AST per file | File content hash | Source file bytes change |
| **Instantiation** | Monomorphized, type-checked, ownership-verified AST | Full composite key | Any component of cache key changes |

**Package tier:** If a source file's content hash hasn't changed, skip parsing and name resolution entirely. This is a simple byte-level check — no semantic analysis needed.

**Instantiation tier:** The monomorphized AST is the result of substituting concrete types into a generic function, type-checking the result, and verifying ownership. This is the expensive work that caching avoids.

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

Cached artifacts live in `.rask-cache/` at the project root (gitignored).

**Requirements:**
- Compiler version stamp — mismatch discards entire cache
- Per-package organization for locality
- `rask cache clean` removes all cached artifacts
- `rask cache stats` shows cache size and hit/miss rates

**Implementation note:** Binary serialization format and directory structure are implementation details, not part of this specification. A fast binary format (e.g., bincode) and per-package subdirectories are recommended.

### Quick Path: No Changes

Before computing any semantic hashes, the compiler checks whether any source file in the package has changed (by content hash). If no files have changed and all upstream package metadata is unchanged, the entire package compilation is skipped. This makes the common case (no changes) essentially free.

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
- **Generics:** Semantic hash caching is the primary mitigation for monomorphization's incremental build cost. Without it, any change to a generic function body forces recompilation of ALL call sites across ALL packages. See [generics.md](../types/generics.md).
- **Comptime:** Comptime memoization uses the same semantic hash infrastructure. Pure comptime functions can be cached by `(function, arguments, body_hash)`. This resolves the memoization question from [comptime.md](../control/comptime.md) Remaining Issues.
- **Type System:** Type definition hashes are part of the instantiation cache key. Struct layout changes invalidate affected monomorphizations even when the generic function body is unchanged.
- **Local Analysis:** Hash computation is function-local plus callee identities/hashes. No whole-program analysis. Packages export their hashes as metadata, maintaining the compilation boundary.
- **Concurrency:** Hash computation is embarrassingly parallel per-function within a package (after topological sort). Cross-package hashing follows the existing parallel compilation model (independent packages in parallel).
- **Tooling:** `rask build --cache-stats` shows hit/miss rates. IDEs MAY show "cached" annotations on functions that will be skipped in the next build.

## Remaining Issues

### Medium Priority
1. **Shared instantiation deduplication** — If packages A and B both instantiate `sort<i32>`, should the linker deduplicate them? This affects binary size, not compilation speed.
2. **Distributed cache** — Could teams share a cache server (like Bazel's remote cache)? Requires deterministic hashing across machines.

### Low Priority
3. **Profiling-guided cache eviction** — Should rarely-used cache entries be evicted to save disk space? Probably not worth the complexity.
