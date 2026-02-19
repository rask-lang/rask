<!-- id: struct.modules -->
<!-- status: decided -->
<!-- summary: Package-visible default, fixed built-ins, path-based imports, export re-exports -->

# Module System

Package-visible default with explicit `public`, fixed built-in types, simple path-based imports, `export` for library facades, transparent re-exports with origin-based identity.

## Visibility

| Rule | Description |
|------|-------------|
| **V1: Package default** | Items visible to all files in same package (no keyword) |
| **V2: Public** | `public` exposes to external packages |
| **V3: Tests access all** | Test files (`*_test.rk`) access all package items |
| **V4: No file-private** | Same package = same team — no file-level visibility |

| Level | Scope | Declaration | Default |
|-------|-------|-------------|---------|
| pkg | All files in package | (no keyword) | Yes |
| public | External packages | `public` | No |

## Built-in Types

| Rule | Description |
|------|-------------|
| **BI1: Always available** | Primitives, `string`, `Vec`, `Map`, `Set`, `Result`, `Option`, `Error`, `Channel`, `Ok`, `Err`, `Some`, `None` |
| **BI2: Fixed set** | Can't be extended by users or projects |
| **BI3: No shadowing** | Defining local type with built-in name is a compile error |
| **BI4: Qualified fallback** | `core.Result` always works regardless of local names |

## Package Organization

| Rule | Description |
|------|-------------|
| **PO1: Directory = package** | All `.rk` files in directory form one package |
| **PO2: Name from path** | Package name derived from directory name |
| **PO3: Nested packages** | `pkg/sub/` is package `pkg.sub` (separate compilation unit) |

## Imports

| Rule | Description |
|------|-------------|
| **IM1: Qualified** | `import pkg` → access as `pkg.Name` |
| **IM2: Last-segment qualifier** | `import myapp.net.http` → access as `http.get()` |
| **IM3: Alias** | `import pkg as p` → access as `p.Name` |
| **IM4: Unqualified** | `import pkg.Name` → `Name` directly |
| **IM5: Grouped** | `import pkg.{Name, Other}` for multiple unqualified |
| **IM6: Glob** | `import pkg.*` — allowed but emits compiler warning |
| **IM7: Unused** | Unused imports are compile errors |
| **IM8: No shadowing** | Shadowing imported name with local definition is compile error |

<!-- test: skip -->
```rask
import http
import myapp.utils as u
import json.{parse, stringify}

func main() {
    const req = http.new("GET", "/")
    const body = parse(read())
}
```

## Lazy Imports

| Rule | Description |
|------|-------------|
| **LZ1: Deferred init** | `import lazy pkg` defers package initialization until first runtime use |
| **LZ2: Once** | Init runs exactly once, synchronized across threads |
| **LZ3: Eager wins** | If any importer uses eager import, package initializes eagerly |
| **LZ4: Error propagation** | Init errors propagate to call site via `try` |

| Access type | Triggers init? |
|-------------|----------------|
| Function call | Yes |
| Constructor | Yes |
| Const access | No (compile-time) |
| Type reference | No (compile-time) |

## Re-exports

| Rule | Description |
|------|-------------|
| **RE1: Export syntax** | `export internal.Name` re-exports as `mylib.Name` |
| **RE2: Origin identity** | Type identity = (origin_package, origin_name) — re-exports preserve type equality |
| **RE3: Public only** | Can't export non-public items |
| **RE4: No cycles** | Export cycles detected at import graph construction |

<!-- test: parse -->
```rask
export internal.parser.Parser
export internal.lexer.Lexer
```

## Struct Visibility

| Rule | Description |
|------|-------------|
| **SV1: All-public fields** | External literal construction allowed |
| **SV2: Any non-public field** | External literal construction forbidden — must provide factory |
| **SV3: Pattern matching** | External code sees `public` fields only; non-public fields automatically ignored |

## Trait Implementation Visibility

| Rule | Description |
|------|-------------|
| **TV1: Inferred** | `extend` visibility = min(trait visibility, type visibility) |
| **TV2: No keyword** | No `public extend` needed — compiler infers, IDE shows ghost annotation |

## Package-Level State

| Rule | Description |
|------|-------------|
| **PS1: Const only** | Package-level declarations must be `const` |
| **PS2: Sync required** | Mutable state requires `Atomic`, `Mutex`, or `Shared` (via interior mutability) |
| **PS3: No mutable globals** | `let` at package level is a compile error |

## Package Initialization

| Rule | Description |
|------|-------------|
| **IN1: init function** | `init() -> () or Error { }` runs once before main |
| **IN2: One per file** | At most one `init()` per file |
| **IN3: Parallel topo sort** | Intra-package init order: parallel topological sort of file import DAG |
| **IN4: Deps first** | Inter-package: dependencies fully initialize before dependents |
| **IN5: Fail-fast** | Init returning `Err` cancels dependent inits immediately |

## Compilation Model

| Rule | Description |
|------|-------------|
| **CM1: Package = unit** | All files in package compiled together |
| **CM2: Public change** | `public` signature change recompiles importers |
| **CM3: Private change** | Non-public change recompiles package only |
| **CM4: Generic recompilation** | Generic body change recompiles instantiation sites (mitigated by semantic hash caching) |

## Error Messages

```
ERROR [struct.modules/BI3]: built-in type shadowing
   |
3  |  struct Vec { }
   |  ^^^^^^^^^^ cannot define type with built-in name `Vec`
```

```
ERROR [struct.modules/IM7]: unused import
   |
1  |  import json
   |  ^^^^^^^^^^^ `json` imported but never used
```

```
ERROR [struct.modules/PS3]: mutable global
   |
5  |  let counter: i32 = 0
   |  ^^^^^^^^^^^^^^^^^^^^^ package-level `let` not allowed; use Atomic or Shared
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Built-in type shadowing | BI3 | Compile error |
| Diamond re-exports | RE2 | Same identity (origin-based) |
| Export of pkg item | RE3 | Compile error |
| Circular imports | IM1 | Compile error at import graph construction |
| Circular lazy init | LZ1 | Runtime error |
| Lazy + eager same package | LZ3 | Eager wins |
| Multiple `init()` in same file | IN2 | Compile error |
| Nested brace imports | IM5 | Compile error: `import a.{b.{C}}` not allowed |
| Glob import | IM6 | Warning emitted |

---

## Appendix (non-normative)

### Rationale

**V1 (package default):** Packages are compilation units — default package visibility keeps related code accessible without ceremony. Same package = same team.

**BI2 (fixed built-in set):** Predictability. Reading any Rask file, you always know what's in scope. Go has no extension mechanism either — IDE auto-import handles repetition.

**RE2 (origin-based identity):** `std.Vec` (re-export of `core.Vec`) and `collections.Vec` (also `core.Vec`) are the same type. Preserves composability across re-export chains.

### Patterns

**Factory-first struct:**
<!-- test: skip -->
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

**Trait-based cycle breaking:**
<!-- test: skip -->
```rask
// pkg: ast
public trait Visitor {
    func visit(self: mut, node: Node)
}

// pkg: printer — no import of ast needed beyond the trait
extend Printer with ast.Visitor {
    func visit(self: mut, node: ast.Node) { ... }
}
```

**Self-referential types with handles (not trait objects):**
<!-- test: parse -->
```rask
struct Node {
    value: i32
    parent: Option<Handle<Node>>
    children: Vec<Handle<Node>>
}
```

### Open Questions

**Package granularity (deferred):** Current design is directory = package (Go-style). File = package (Zig-style) is an alternative. Deferring until validation programs exist.

### Implementation Status

| Feature | Status |
|---------|--------|
| Import syntax parsing | Implemented |
| Export syntax parsing | Implemented |
| Package discovery | Implemented |
| Built-in type tracking | Implemented |
| Built-in shadowing detection | Implemented |
| Cross-package symbol lookup | Implemented |
| Visibility checking (public/package) | Implemented |
| Circular dependency detection | Implemented |
| Semver constraint parsing | Implemented |
| Feature resolution (additive + exclusive) | Implemented |
| Lock file with capability tracking | Implemented |

### See Also

- `struct.build` — build system, dependencies
- `struct.packages` — versioning, dependency resolution
- `struct.c-interop` — C imports, `extern "C"`
- `struct.targets` — libraries vs executables, entry points
