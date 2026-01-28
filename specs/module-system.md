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

**Design rationale:**
- Qualified access is the default (shows provenance)
- `using` makes selective import a conscious choice
- No braces needed—reads as natural English: "import http using Request"

**Constraint rules:**
- NO wildcard imports (no `import pkg using *`)
- Unused imports: compile error
- Shadowing imported name with local definition: compile error

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

### Package Initialization

**Syntax:**
```
init() -> Result<(), Error> {
    // Runs once per package before main
}
```

**Order:**
- Inter-package: topological by import graph
- Intra-package: **UNSPECIFIED** (may run in parallel, do NOT depend on file order)
- Create ordering: file A imports item from file B → A's init runs after B's

**Failure:**
- Init returning `Err`: dependent packages do NOT run, independent packages continue
- First error reported immediately (fail-fast per dependency chain)
- Already-initialized packages remain initialized (no automatic rollback)
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

### C Interop (Zig-style)

**Importing C headers:**

| Syntax | Effect |
|--------|--------|
| `import c "header.h"` | Parse header, expose as `c.symbol` |
| `import c "header.h" as name` | Parse header, expose as `name.symbol` |
| `import c { "a.h", "b.h" }` | Multiple headers, unified namespace |

**How it works:**
- Compiler includes C parser (libclang or similar)
- Header parsed at compile time; C types/functions available immediately
- C types mapped to Rask equivalents (`int` → `c_int`, `char*` → `*u8`, etc.)
- Macros: function-like macros become inline functions; constant macros become constants
- Calling C functions requires `unsafe` context (C cannot guarantee Rask's safety invariants)

**Example:**
```
import c "stdio.h"
import c "mylib.h" as mylib

fn main() {
    unsafe {
        c.printf("Hello %s\n".ptr, name.ptr)
        mylib.process(data.ptr, data.len)
    }
}
```

**Exporting to C:**

| Feature | Mechanism |
|---------|-----------|
| Export function | `pub extern "C" fn name()` |
| Export type | `pub extern "C" struct Name { ... }` (must be C-compatible layout) |
| Header generation | `raskc --emit-header pkg` produces `pkg.h` |
| ABI | `extern "C"` uses C ABI; `pub` alone uses Rask ABI |

**C-compatible types:**
- Primitives: `i8`-`i64`, `u8`-`u64`, `f32`, `f64`, `bool`
- C-specific: `c_int`, `c_long`, `c_size`, `c_char` (platform-dependent sizes)
- Pointers: `*T`, `*mut T`
- `extern "C" struct` with only C-compatible fields

**NOT C-compatible:**
- `String`, `Vec`, `Pool` (internal layout not stable)
- Handles (generational references have no C equivalent)
- Closures, trait objects

**Build integration:**
```
// rask.build or CLI
c_include_paths: ["/usr/include", "vendor/"]
c_link_libs: ["ssl", "crypto"]
```

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
| C header parse failure | Compile error with location in header; suggest `-I` path or missing dependency |
| C macro with side effects | Not imported; warning emitted; use wrapper function |
| C variadic functions | Callable from unsafe; Rask cannot export variadic |
| C opaque struct | Becomes opaque type in Rask; only pointer operations allowed |

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

### C Interop
```
// file: sqlite_wrapper.rask
import c "sqlite3.h" as sql

pub struct Database {
    handle: *sql.sqlite3  // C opaque pointer
}

pub fn open(path: String) -> Result<Database, Error> {
    let db: *sql.sqlite3 = null
    unsafe {
        let rc = sql.sqlite3_open(path.cstr(), &db)
        if rc != sql.SQLITE_OK {
            return Err(Error::new("sqlite open failed"))
        }
    }
    Ok(Database { handle: db })
}

pub fn close(db: Database) {
    unsafe { sql.sqlite3_close(db.handle) }
}

// Exporting to C:
pub extern "C" fn rask_process(data: *const u8, len: c_size) -> c_int {
    unsafe {
        let slice = slice_from_raw(data, len)
        match process(slice) {
            Ok(_) => 0,
            Err(_) => -1,
        }
    }
}
```

## Integration Notes

- **Memory Model**: Package boundaries do NOT affect ownership—values passed across packages transfer ownership identically to intra-package calls
- **Type System**: Trait structural matching works across packages; impl visibility inferred from trait+type (pub+pub=pub)
- **Concurrency**: Channels can send types across package boundaries; type identity preserved (origin-based)
- **Error Handling**: `Result` as built-in eliminates ceremony; propagation (`?`) works uniformly regardless of import style
- **Compiler Architecture**: Import graph construction precedes type checking; cycle detection happens once per incremental build; C header parsing cached per header+flags; generic instantiations cached by semantic hash
- **C Interop**: Compiler bundles C parser; `unsafe` required for all C calls (C cannot provide Rask safety guarantees); raw pointers exist only in unsafe blocks
- **Tooling Contract**: IDEs MUST show ghost `pub` on impl when inferred (both trait and type are pub); SHOULD show ghost annotations for qualified names when selective imports used, and monomorphization locations