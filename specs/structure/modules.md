# Solution: Module System

## The Question
How are programs organized? What are the visibility rules, import/export mechanism, and namespace management?

## Decision
Package-visible default with explicit `pub`, fixed built-in types, `import`/`using` for selective imports, `export` for library facades, transparent re-exports with origin-based identity.

## Rationale
Packages are compilation units—default package visibility keeps related code accessible without ceremony. Fixed built-in types eliminate noise for ubiquitous types while preserving predictability (you always know what's in scope). Qualified imports are the default (shows provenance); `using` keyword makes selective import a conscious choice with natural English flow. `export` clearly communicates re-export intent for library authors. Transparent re-exports (identity = origin) preserve composability.

## Specification

### Visibility Levels

| Level | Scope | Declaration | Default |
|-------|-------|-------------|---------|
| **pkg** | All files in package | (no keyword) | YES |
| **pub** | External packages | `pub` | No |

**Rules:**
- Package-visible default: items visible to all files in package
- `pub` exposes to external packages
- Test files (`*_test.rask`) access all package items
- No file-private visibility (same package = same team)

### Built-in Types (Always Available)

**These types are available in every file without import:**
- Primitives: `i8`, `i16`, `i32`, `i64`, `u8`, `u16`, `u32`, `u64`, `f32`, `f64`, `bool`, `char`
- Core types: `String`, `Vec`, `Pool`, `Map`, `Result`, `Option`, `Error`
- Variants: `Ok`, `Err`, `Some`, `None`

**Rules:**
- **Fixed set:** Cannot be extended by users or projects
- Defining local type with built-in name: **compile error** (prevents silent breakage)
- Qualified access always works: `core.Result` even if local `Result` exists
- NOT built-in: `File`, `Socket`, `Channel`, `Task`, `Set`, `Queue` (must import explicitly)

**Why fixed?** Predictability (PI ≥ 0.85). Reading any Rask file, you always know what's in scope. Go has no extension mechanism either—IDE auto-import handles repetition.

### Package Organization

| Rule | Behavior |
|------|----------|
| Package = directory | All `.rask` files in directory form one package |
| Package name = directory name | Derived from path, no declaration |
| Nested packages | `pkg/sub/` is package `pkg.sub` (separate compilation unit) |
| One package per directory | MUST NOT mix packages |

### Import Mechanism

| Syntax | Effect |
|--------|--------|
| `import pkg` | Qualified access: `pkg.Name` |
| `import pkg as p` | Aliased access: `p.Name` |
| `import pkg using Name` | Unqualified: `Name` directly |
| `import pkg using Name, Other` | Multiple unqualified |
| `import pkg using Name as N` | Renamed unqualified: `N` |
| `import lazy pkg` | Lazy init: deferred until first use |

**Design rationale:**
- Qualified access is the default (shows provenance)
- `using` makes selective import a conscious choice
- No braces needed—reads as natural English: "import http using Request"

**Constraint rules:**
- NO wildcard imports (no `import pkg using *`)
- Unused imports: compile error
- Shadowing imported name with local definition: compile error

### Lazy Imports

`import lazy` defers package initialization until first runtime use:

```
import lazy database  // init() deferred until first function call

fn main() -> Result<(), Error> {
    if args.has("--help") {
        print_help()
        return Ok(())  // Fast exit, database never initialized
    }

    let conn = database.connect()?  // init() runs here, errors propagate via ?
}
```

**When init runs:**

| Access type | Triggers init? |
|-------------|----------------|
| Function call (`pkg.foo()`) | ✓ Yes |
| Constructor (`pkg.Type { }`) | ✓ Yes |
| Const access (`pkg.CONST`) | ✗ No (compile-time) |
| Type reference (`pkg.Type`) | ✗ No (compile-time) |

**Semantics:**
- First function call to a lazy-imported package triggers its `init()`
- Init runs exactly once, synchronized across threads (like `OnceLock`)
- Init errors propagate to the call site via `?`
- If ANY importer uses eager import, package initializes eagerly (eager wins)

**When to use:**

| Use case | Import style |
|----------|--------------|
| Server (fail-fast) | `import pkg` (eager, default) |
| CLI tool (fast startup) | `import lazy pkg` |
| Rarely-used feature | `import lazy pkg` |
| Hot-path performance | `import pkg` (no init check overhead)

### Re-exports (for library facades)

| Syntax | Effect |
|--------|--------|
| `export internal.Name` | Re-export as `mylib.Name` |
| `export internal.Name as Alias` | Re-export with rename |
| `export internal.Name, internal.Other` | Multiple re-exports |

**Purpose:** Library authors expose a clean API without revealing internal structure.

```
// mylib/api.rask
export internal.parser.Parser
export internal.lexer.Lexer

// Users see:
import mylib
mylib.Parser  // works, don't need to know about internal/parser
```

**Type identity rule:**
- Identity = (origin_package, origin_name)
- `std.Vec` (re-export of `core.Vec`) has identity `(core, Vec)`
- `collections.Vec` (re-export of `core.Vec`) has identity `(core, Vec)` — **same type**
- Re-exports preserve type equality for interoperability

**Constraints:**
- CANNOT export non-pub item: compile error
- Export cycles detected at import graph construction: compile error

### Struct Visibility

| Scenario | External Construction |
|----------|----------------------|
| `pub struct` + all `pub` fields | Literal construction allowed |
| `pub struct` + any non-pub fields | Literal construction FORBIDDEN |
| Mixed visibility struct | MUST provide factory function |

**Field addition semantics:**
- Adding `pub` field to all-pub struct: **breaking change**
- Adding non-pub field to all-pub struct: **breaking change** (disables literals)
- Adding any field to mixed struct: **non-breaking** (already factory-only)

**Factory-first pattern:**
```
pub struct Request {
    pub method: String
    pub path: String
    id: u64  // non-pub → factory required
}

pub fn new_request(method: String, path: String) -> Request {
    Request { method, path, id: next_id() }
}
```

**Pattern matching:**
- Same package: all fields accessible
- External: `pub` fields only; non-pub fields automatically ignored (no `..` required)

### Trait Implementation Visibility

**Inferred from trait and type:**

| Trait | Type | Impl visibility |
|-------|------|-----------------|
| `pub` | `pub` | `pub` (automatic) |
| `pub` | pkg | pkg |
| pkg | `pub` | pkg |
| pkg | pkg | pkg |

**Rule:** `impl visibility = min(trait visibility, type visibility)`

No `pub impl` keyword needed—compiler infers it. IDE shows ghost `pub` when inferred.

**Generic bound visibility:**
- `pub fn foo<T: Trait>`: caller's `T` must have pub-visible impl (both trait and type are pub)
- `fn foo<T: Trait>` (pkg): `T` must have pkg-visible impl
- Monomorphization at call site: caller's visibility context determines bound satisfaction

### Circular Dependencies

| Situation | Handling |
|-----------|----------|
| Direct import cycle | Compile error at import graph construction |
| Export cycle | Compile error |
| Mutual type dependencies | Use trait indirection |

**Trait-based cycle breaking:**
```
// pkg: ast
pub trait Visitor {
    fn visit_node(self: mut, node: Node)
}

pub struct Node {
    value: i32
    accept: fn(Node, impl Visitor)  // generic, not trait object
}

// pkg: visitor
impl ast.Visitor for ConcreteVisitor { ... }

// Call site:
node.accept(concrete_visitor)  // Monomorphized: zero indirection
```

**When trait objects required:**
- Heterogeneous collections: `[]any Visitor`
- Runtime polymorphism: storing different visitor types
- NOT required for compile-time-known cycles: use `impl Trait` parameter

**Self-referential types (NOT cycles):**

Recursive structures like trees use handles, not trait objects:
```
struct Node {
    value: i32
    parent: Handle<Node>    // zero-cost: index + generation
    children: Vec<Handle<Node>>
}
```

| Approach | When to use |
|----------|-------------|
| `Handle<T>` | Self-referential structures (trees, graphs, linked lists) |
| `impl Trait` | Cross-package interaction with statically-known types |
| `any Trait` | Heterogeneous collections, runtime polymorphism |

Handles are **not** trait objects—they're indices into a pool. No vtable, no indirection beyond array lookup.

### Package-Level State

**Allowed at package level:**

| Declaration | Example | Notes |
|-------------|---------|-------|
| `const` | `const MAX: i32 = 100` | Immutable, computed at compile time |
| Sync-wrapped mutable | `var counter: Atomic<i32> = Atomic::new(0)` | Thread-safe by construction |
| Sync-wrapped mutable | `var config: Shared<Config> = Shared::new(...)` | Thread-safe by construction |

**Not allowed:**
```
var counter: i32 = 0  // ✗ Compile error: package-level mutable state must be sync-safe
```

**Rationale:** Unsynchronized mutable globals are race conditions waiting to happen. Requiring sync primitives makes the intent explicit and enables safe parallel initialization.

### Package Initialization

**Syntax:**
```
init() -> Result<(), Error> {
    // Runs once per package before main
}
```

**Constraints:**
- At most ONE `init()` function per file
- Multiple `init()` in same file: compile error

**Order:**

Intra-package init order is a **parallel topological sort** of the file import DAG:

1. Build directed graph: edge B → A if file A imports from file B (same package)
2. Files with in-degree 0 run their inits **in parallel**
3. When init completes, decrement dependents' in-degree
4. Files reaching in-degree 0 start immediately
5. Inter-package: dependencies fully initialize before dependents

```
Example:
    api.rask imports db.rask, cache.rask
    db.rask, cache.rask, util.rask have no intra-package imports

    ┌────────┐
    │  api   │  ← waits for db, cache
    └────┬───┘
    ┌────┴────┐
    ▼         ▼
  ┌────┐  ┌───────┐  ┌──────┐
  │ db │  │ cache │  │ util │  ← run in PARALLEL (no dependencies)
  └────┘  └───────┘  └──────┘
```

**Why parallel is safe:** Package-level mutable state requires sync primitives, so concurrent init cannot race.

**Failure:**
- Init returning `Err`: dependent packages do NOT run, independent packages continue
- First error reported immediately (fail-fast per dependency chain)
- Already-initialized packages remain initialized (no automatic rollback)
- If any init in a package fails, remaining inits in that package are cancelled
- Linear resources in init: consumed or returned in `Err` per normal error handling

### Compilation Model

| Aspect | Behavior |
|--------|----------|
| Compile unit | Package (all files together) |
| Incremental trigger | `pub` signature change → recompile importers |
| Incremental trigger | Non-pub change → recompile package only |
| Generic instantiation | At call site; body change triggers recompilation of instantiation sites |
| Parallelization | Independent packages compile in parallel |

**Generic recompilation honesty:**
- Changing generic function body DOES require recompiling call sites
- Mitigation: use `any Trait` (vtable) for stable ABI across changes
- Trade-off: Monomorphization = fast runtime, slower incremental builds

**Hash-based caching (optimization):**
- Each generic instantiation is keyed by: (function, type arguments, body semantic hash)
- If body semantic hash unchanged, reuse cached monomorphization
- Semantic hash ignores: comments, formatting, local variable names
- Semantic hash includes: control flow, operations, called functions, types
- Result: changing `sort<T>` implementation only recompiles callers if behavior changes

| Change type | Recompile callers? |
|-------------|-------------------|
| Rename local variable | No (same hash) |
| Add comment | No (same hash) |
| Change algorithm | Yes (different hash) |
| Change called function | Yes (different hash) |

### C Interop

See [C Interop](c-interop.md) for full specification. Summary:

| Approach | Use Case |
|----------|----------|
| `import c "header.h"` | Automatic parsing (built-in C parser, like Zig) |
| `extern "C" { }` | Explicit bindings for C++, complex macros |

All C calls require `unsafe` context.

### Edge Cases

| Case | Handling |
|------|----------|
| Built-in type shadowing | Compile error (prevents silent breakage) |
| Same-package qualified access | Optional: `Request` and `request.Request` both valid within `request` package |
| Diamond exports | Same identity (origin-based): `std.Vec` == `collections.Vec` if both export `core.Vec` |
| Export of pkg item | Compile error: cannot `export` non-pub item |
| Trait bound with pkg type | Generic caller must have visibility to both type and impl |
| Init with linear resource failure | Resource consumed or returned in error per linear semantics |
| Cycle detection timing | Import graph construction (before type checking), O(edges) |
| Field defaults | Must be `const` expressions; no side effects |
| Generic with pkg-visible helper type | Legal within package; cannot expose via `pub fn` signature |
| Self-referential struct | Use `Handle<Self>` not `any Trait`; pool required |
| Generic body unchanged (hash match) | Skip recompilation of instantiation sites |
| Unsync package-level mutable | Compile error: "must be sync-safe" (use `Atomic`, `Mutex`, `Shared`) |
| Multiple `init()` in same file | Compile error: "at most one init() per file" |
| Circular intra-package init | Compile error: "circular init dependency: a.rask → b.rask → a.rask" |
| Init order dependency | Use explicit imports to establish ordering between files |
| Lazy + eager import same pkg | Eager wins: package initializes before main() |
| Lazy init failure | Error propagates to call site via `?` |
| Circular lazy init | Runtime error: "circular lazy initialization: A → B → A" |
| Lazy import with `using` | `import lazy pkg using Foo` — Foo() call triggers init |

## Examples

### Basic Package Structure
```
// file: http/request.rask
pub struct Request {
    pub method: String
    pub path: String
    id: u64  // pkg-visible
}

pub fn new(method: String, path: String) -> Request {
    Request { method, path, id: next_id() }
}

fn next_id() -> u64 { ... }  // pkg-visible helper

fn debug_id(r: Request) -> u64 { r.id }  // pkg-visible

// file: http/handler.rask (same package)
fn log(r: Request) {
    print(debug_id(r))  // OK: pkg-visible
}
```

### Import Patterns
```
// file: main.rask
import http
import json using parse, stringify  // selective unqualified

fn main() {
    let req = http.new("GET", "/")     // qualified
    let body = parse(read())           // unqualified
    let s: String = ...                // built-in
    let r: Result<i32, Error> = Ok(42) // built-in
}
```

### Trait-Based Cycles
```
// pkg: ast
pub trait Visitor {
    fn visit(self: mut, node: Node)
}

pub struct Node {
    children: Vec<Node>
}

pub fn traverse(node: Node, v: impl Visitor) {
    v.visit(node)  // Monomorphized: zero-cost
}

// pkg: printer
impl ast.Visitor for Printer {
    fn visit(self: mut, node: ast.Node) { ... }
}

// Usage:
ast.traverse(tree, Printer{})  // No vtable; compile-time dispatch
```

### Self-Referential Structures
```
// Tree with parent pointers—uses handles, not trait objects
struct Node {
    value: i32
    parent: Option<Handle<Node>>
    children: Vec<Handle<Node>>
}

fn build_tree(pool: mut Pool<Node>) -> Handle<Node> {
    let root = pool.insert(Node { value: 0, parent: None, children: [] })
    let child = pool.insert(Node { value: 1, parent: Some(root), children: [] })
    pool[root].children.push(child)
    root
}

fn walk_up(pool: Pool<Node>, node: Handle<Node>) {
    let current = node
    while let Some(parent) = pool[current].parent {
        print(pool[parent].value)  // O(1) lookup, no vtable
        current = parent
    }
}
```

## Integration Notes

- **Memory Model**: Package boundaries do NOT affect ownership—values passed across packages transfer ownership identically to intra-package calls
- **Type System**: Trait structural matching works across packages; impl visibility inferred from trait+type (pub+pub=pub)
- **Concurrency**: Channels can send types across package boundaries; type identity preserved (origin-based)
- **Error Handling**: `Result` as built-in eliminates ceremony; propagation (`?`) works uniformly regardless of import style
- **Compiler Architecture**: Import graph construction precedes type checking; cycle detection happens once per incremental build; generic instantiations cached by semantic hash
- **C Interop**: See [C Interop](c-interop.md) for full specification
- **Tooling Contract**: IDEs MUST show ghost `pub` on impl when inferred (both trait and type are pub); SHOULD show ghost annotations for qualified names when selective imports used, and monomorphization locations