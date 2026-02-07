# Solution: Module System

## The Question
How are programs organized? What are the visibility rules, import/export mechanism, and namespace management?

## Decision
Package-visible default with explicit `public`, fixed built-in types, simple path-based imports, `export` for library facades, transparent re-exports with origin-based identity.

## Rationale
Packages are compilation units—default package visibility keeps related code accessible without ceremony. Fixed built-in types eliminate noise for ubiquitous types while preserving predictability (you always know what's in scope). Path-based imports (`import pkg` for qualified, `import pkg.Name` for unqualified) are intuitive—import a package for qualified access, import a symbol for direct access. `export` clearly communicates re-export intent. Transparent re-exports (identity = origin) preserve composability.

## Specification

### Visibility Levels

| Level | Scope | Declaration | Default |
|-------|-------|-------------|---------|
| **pkg** | All files in package | (no keyword) | YES |
| **public** | External packages | `public` | No |

**Rules:**
- Package-visible default: items visible to all files in package
- `public` exposes to external packages
- Test files (`*_test.rk`) access all package items
- No file-private visibility (same package = same team)

### Built-in Types (Always Available)

**These types are available in every file without import:**
- Primitives: `i8`, `i16`, `i32`, `i64`, `u8`, `u16`, `u32`, `u64`, `f32`, `f64`, `bool`, `char`
- Core types: `string`, `Vec`, `Map`, `Set`, `Result`, `Option`, `Error`, `Channel`
- Variants: `Ok`, `Err`, `Some`, `None`

**Rules:**
- **Fixed set:** Can't be extended by users or projects
- Defining local type with built-in name: **compile error** (prevents silent breakage)
- Qualified access always works: `core.Result` even if local `Result` exists
- Not built-in: `File`, `Socket`, `Task`, `Pool`, `Queue` (must import explicitly)

**Why fixed?** Predictability (PI ≥ 0.85). Reading any Rask file, you always know what's in scope. Go has no extension mechanism either—IDE auto-import handles repetition.

### Package Organization

| Rule | Behavior |
|------|----------|
| Package = directory | All `.rk` files in directory form one package |
| Package name = directory name | Derived from path, no declaration |
| Nested packages | `pkg/sub/` is package `pkg.sub` (separate compilation unit) |
| One package per directory | Can't mix packages |

### Import Mechanism

| Syntax | Effect |
|--------|--------|
| `import pkg` | Qualified access: `pkg.Name` |
| `import std.io` | Qualified via last segment: `io.print()` |
| `import pkg as p` | Aliased access: `p.Name` |
| `import pkg.Name` | Unqualified: `Name` directly |
| `import pkg.Name, pkg.Other` | Multiple unqualified |
| `import pkg.{Name, Other}` | Grouped unqualified (equivalent to above) |
| `import pkg.Name as N` | Renamed unqualified: `N` |
| `import lazy pkg` | Lazy init: deferred until first use |
| `import pkg.*` | Glob import (with warning) |

**Last-segment qualifier rule:**
For nested package imports, the **last path segment** becomes the qualifier (like Go):
```rask
import std.io          // use as: io.print()
import std.net.http    // use as: http.get()
import myapp.utils     // use as: utils.helper()
import std.io as sio   // explicit alias overrides: sio.print()
```

**Grouped imports (brace syntax):**
For importing multiple items from nested modules, use braces to avoid path repetition:
```rask
import std.collections.{
    HashMap,
    HashSet,
    Entry,
}

import std.{io, fs, net}  // multiple submodules, qualified
```
Rules: items in braces follow same rules as individual imports. Trailing comma allowed.

**Design rationale:**
- Path determines access: import package → qualified, import symbol → unqualified
- `import http.Request` clearly says `Request` is available directly
- Glob imports emit a compiler warning to discourage overuse

**Disambiguation (package vs symbol):**
- Convention: packages are lowercase, types are PascalCase
- If ambiguous, symbols take precedence over subpackages

**Constraint rules:**
- Wildcard imports (`import pkg.*`) allowed but emit compiler warning
- Unused imports: compile error
- Shadowing imported name with local definition: compile error

### Lazy Imports

`import lazy` defers package initialization until first runtime use:

```rask
import lazy database  // init() deferred until first function call

func main() -> () or Error {
    if args.has("--help") {
        print_help()
        return Ok(())  // Fast exit, database never initialized
    }

    const conn = try database.connect()  // init() runs here, errors propagate via try
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
- If any importer uses eager import, package initializes eagerly (eager wins)

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

**Purpose:** Library authors can expose a clean API without revealing internal structure.

```rask
// mylib/api.rk
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
- Can't export non-public item: compile error
- Export cycles detected at import graph construction: compile error

### Struct Visibility

| Scenario | External Construction |
|----------|----------------------|
| `public struct` + all `public` fields | Literal construction allowed |
| `public struct` + any non-public fields | Literal construction FORBIDDEN |
| Mixed visibility struct | MUST provide factory function |

**Field addition semantics:**
- Adding `public` field to all-public struct: breaking change
- Adding non-public field to all-public struct: breaking change (disables literals)
- Adding any field to mixed struct: non-breaking (already factory-only)

**Factory-first pattern:**
```rask
public struct Request {
    public method: string
    public path: string
    id: u64  // non-public → factory required
}

public func new_request(method: string, path: string) -> Request {
    Request { method, path, id: next_id() }
}
```

**Pattern matching:**
- Same package: all fields accessible
- External: `public` fields only; non-public fields automatically ignored (no `..` required)

### Trait Implementation Visibility

**Inferred from trait and type:**

| Trait | Type | Impl visibility |
|-------|------|-----------------|
| `public` | `public` | `public` (automatic) |
| `public` | pkg | pkg |
| pkg | `public` | pkg |
| pkg | pkg | pkg |

**Rule:** `extend visibility = min(trait visibility, type visibility)`

No `public extend` keyword needed—compiler infers it. IDE shows ghost `public` when inferred.

**Generic bound visibility:**
- `public func foo<T: Trait>`: caller's `T` must have public-visible extend (both trait and type are public)
- `func foo<T: Trait>` (pkg): `T` must have pkg-visible extend
- Monomorphization at call site: caller's visibility context determines bound satisfaction

### Circular Dependencies

| Situation | Handling |
|-----------|----------|
| Direct import cycle | Compile error at import graph construction |
| Export cycle | Compile error |
| Mutual type dependencies | Use trait indirection |

**Trait-based cycle breaking:**
```rask
// pkg: ast
public trait Visitor {
    func visit_node(self: mut, node: Node)
}

public struct Node<V: Visitor> {
    value: i32
    accept: func(Node<V>, V)  // generic, not trait object
}

// pkg: visitor
extend ConcreteVisitor with ast.Visitor { ... }

// Call site:
node.accept(concrete_visitor)  // Monomorphized: zero indirection
```

**When trait objects required:**
- Heterogeneous collections: `[]any Visitor`
- Runtime polymorphism: storing different visitor types
- Not required for compile-time-known cycles: use explicit generics `<T: Trait>`

**Self-referential types (NOT cycles):**

Recursive structures like trees use handles, not trait objects:
```rask
struct Node {
    value: i32
    parent: Handle<Node>    // zero-cost: index + generation
    children: Vec<Handle<Node>>
}
```

| Approach | When to use |
|----------|-------------|
| `Handle<T>` | Self-referential structures (trees, graphs, linked lists) |
| `<T: Trait>` | Cross-package interaction with statically-known types (generics) |
| `any Trait` | Heterogeneous collections, runtime polymorphism |

Handles aren't trait objects—they're indices into a pool. No vtable, no indirection beyond array lookup.

### Package-Level State

**Allowed at package level:**

| Declaration | Example | Notes |
|-------------|---------|-------|
| `const` | `const MAX: i32 = 100` | Immutable, computed at compile time |
| `const` + Atomic | `const COUNTER: Atomic<i32> = Atomic.new(0)` | Thread-safe via interior mutability |
| `const` + Shared | `const CONFIG: Shared<Config> = Shared.new(...)` | Thread-safe via interior mutability |

**Not allowed:**
```rask
let counter: i32 = 0  // ✗ Compile error: no mutable globals (use Atomic or Shared)
```

**Rationale:** Unsynchronized mutable globals are race conditions waiting to happen. Sync primitives make the intent explicit and enable safe parallel initialization.

### Package Initialization

**Syntax:**
```rask
init() -> () or Error {
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
    api.rk imports db.rk, cache.rk
    db.rk, cache.rk, util.rk have no intra-package imports

    ┌────────┐
    │  api   │  ← waits for db, cache
    └────┬───┘
    ┌────┴────┐
    ▼         ▼
  ┌────┐  ┌───────┐  ┌──────┐
  │ db │  │ cache │  │ util │  ← run in PARALLEL (no dependencies)
  └────┘  └───────┘  └──────┘
```

**Why parallel is safe:** Package-level mutable state requires sync primitives, so concurrent init can't race.

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
| Incremental trigger | `public` signature change → recompile importers |
| Incremental trigger | Non-public change → recompile package only |
| Generic instantiation | At call site; body change triggers recompilation of instantiation sites |
| Parallelization | Independent packages compile in parallel |

**Generic recompilation honesty:**
- Changing generic function body does require recompiling call sites
- Mitigation: use `any Trait` (vtable) for stable ABI across changes
- Trade-off: Monomorphization = fast runtime, slower incremental builds
- Mitigation: semantic hash caching skips recompilation when function body hasn't meaningfully changed

See [Semantic Hash Caching](../compiler/semantic-hash-caching.md) for the full specification of incremental compilation and semantic hash caching.

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
| Export of pkg item | Compile error: cannot `export` non-public item |
| Trait bound with pkg type | Generic caller must have visibility to both type and extend |
| Init with linear resource failure | Resource consumed or returned in error per linear semantics |
| Cycle detection timing | Import graph construction (before type checking), O(edges) |
| Field defaults | Must be `const` expressions; no side effects |
| Generic with pkg-visible helper type | Legal within package; cannot expose via `public func` signature |
| Self-referential struct | Use `Handle<Self>` not `any Trait`; pool required |
| Generic body unchanged (hash match) | Skip recompilation of instantiation sites |
| Unsync package-level mutable | Compile error: "must be sync-safe" (use `Atomic`, `Mutex`, `Shared`) |
| Multiple `init()` in same file | Compile error: "at most one init() per file" |
| Circular intra-package init | Compile error: "circular init dependency: a.rk → b.rk → a.rk" |
| Init order dependency | Use explicit imports to establish ordering between files |
| Lazy + eager import same pkg | Eager wins: package initializes before main() |
| Lazy init failure | Error propagates to call site via `?` |
| Circular lazy init | Runtime error: "circular lazy initialization: A → B → A" |
| Lazy import of symbol | `import lazy pkg.Foo` — Foo() call triggers init |
| Glob import | Warning: "glob import imports N symbols, consider specific imports" |
| Nested braces in import | Compile error: `import a.{b.{C}}` not allowed (use separate imports) |
| Empty brace group | Compile error: `import pkg.{}` is invalid |

## Examples

### Basic Package Structure
```rask
// file: http/request.rk
public struct Request {
    public method: string
    public path: string
    id: u64  // pkg-visible
}

public func new(method: string, path: string) -> Request {
    Request { method, path, id: next_id() }
}

func next_id() -> u64 { ... }  // pkg-visible helper

func debug_id(r: Request) -> u64 { r.id }  // pkg-visible

// file: http/handler.rk (same package)
func log(r: Request) {
    print(debug_id(r))  // OK: pkg-visible
}
```

### Import Patterns
```rask
// file: main.rk
import http
import std.net.http as nethttp        // alias for disambiguation
import json.{parse, stringify}        // grouped unqualified

func main() {
    const req = http.new("GET", "/")   // qualified via last segment
    const body = parse(read())         // unqualified from grouped import
    const s: string = ...              // built-in
    const r: Result<i32, Error> = Ok(42) // built-in
}
```

### Trait-Based Cycles
```rask
// pkg: ast
public trait Visitor {
    func visit(self: mut, node: Node)
}

public struct Node {
    children: Vec<Node>
}

public func traverse<V: Visitor>(node: Node, v: V) {
    v.visit(node)  // Monomorphized: zero-cost
}

// pkg: printer
extend Printer with ast.Visitor {
    func visit(self: mut, node: ast.Node) { ... }
}

// Usage:
ast.traverse(tree, Printer{})  // No vtable; compile-time dispatch
```

### Self-Referential Structures
```rask
// Tree with parent pointers—uses handles, not trait objects
struct Node {
    value: i32
    parent: Option<Handle<Node>>
    children: Vec<Handle<Node>>
}

func build_tree(pool: Pool<Node>) -> Handle<Node> {
    const root = pool.insert(Node { value: 0, parent: None, children: [] })
    const child = pool.insert(Node { value: 1, parent: Some(root), children: [] })
    pool[root].children.push(child)
    root
}

func walk_up(pool: Pool<Node>, node: Handle<Node>) {
    let current = node  // let because reassigned in loop
    while pool[current].parent is Some(parent) {
        print(pool[parent].value)  // O(1) lookup, no vtable
        current = parent
    }
}
```

## Integration Notes

- **Memory Model**: Package boundaries do NOT affect ownership—values passed across packages transfer ownership identically to intra-package calls
- **Type System**: Trait structural matching works across packages; extend visibility inferred from trait+type (public+public=public)
- **Concurrency**: Channels can send types across package boundaries; type identity preserved (origin-based)
- **Error Handling**: `Result` as built-in eliminates ceremony; propagation (`try`) works uniformly regardless of import style
- **Compiler Architecture**: Import graph construction precedes type checking; cycle detection happens once per incremental build; generic instantiations cached by semantic hash
- **C Interop**: See [C Interop](c-interop.md) for full specification
- **Tooling Contract**: IDEs MUST show ghost `public` on extend when inferred (both trait and type are public); SHOULD show ghost annotations for qualified names, and monomorphization (code generation) locations

## Implementation Status

| Feature | Status | Notes |
|---------|--------|-------|
| Import syntax parsing | ✅ Implemented | All forms: qualified, symbol, alias, glob, lazy |
| Export syntax parsing | ✅ Implemented | Re-export declarations |
| Package discovery | ✅ Implemented | Recursive directory scanning |
| Built-in type tracking | ✅ Implemented | Vec, Map, Set, string, Error, Channel |
| Built-in shadowing detection | ✅ Implemented | Compile error on shadowing |
| Import resolution | ✅ Implemented | Records imports for later phases |
| Lazy import tracking | ✅ Implemented | Tracked for deferred loading |
| Multi-package builds | ✅ Implemented | `rask build` command |
| Cross-package symbol lookup | 🔲 Planned | Requires full registry integration |
| Visibility checking | 🔲 Planned | public vs pkg enforcement |
| Circular dependency detection | 🔲 Planned | Import graph analysis |