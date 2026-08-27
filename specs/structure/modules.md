<!-- id: struct.modules -->
<!-- status: decided -->
<!-- summary: Package-visible default, `private` keyword for encapsulation, fixed built-ins, path-based imports, export re-exports -->

# Module System

Package-visible default for items and fields, `private` keyword for extend-only access, explicit `public` modifier, fixed built-in types, simple path-based imports, `export` for library facades, transparent re-exports with origin-based identity.

## Visibility

| Rule | Description |
|------|-------------|
| **V1: Package default** | Items visible to all files in same package (no keyword) |
| **V2: Public** | `public` exposes to external packages |
| **V3: Tests access all** | Test files (`*_test.rk`) access all package items |
| **V4: No file-private** | Same package = same team — no file-level visibility |
| **V5: Private keyword** | `private` restricts fields and methods to `extend` blocks only. Invalid on free functions or types |

| Level | Scope | Declaration | Applies to |
|-------|-------|-------------|------------|
| private | `extend` blocks only | `private` | Fields, methods in `extend` blocks |
| package | All files in package | (no keyword) | Items and fields (default) |
| public | External packages | `public` | Items and fields |

## Built-in Types

| Rule | Description |
|------|-------------|
| **BI1: Always available** | Primitives, `string`, `Vec`, `Map`, `Set`, `Error`, `Channel`, `none`. Status types `T?` and `T or E` are builtin sugar — no nominal name |
| **BI2: Fixed set** | Can't be extended by users or projects |
| **BI3: No shadowing** | Defining local type with built-in name is a compile error |
| **BI4: Qualified fallback** | `core.Error` always works regardless of local names |

## Built-in Functions

| Rule | Description |
|------|-------------|
| **BF1: Always available** | `println`, `print`, `format`, `panic`, `todo`, `unreachable`, `spawn`, `transmute` |
| **BF2: Compiler-known** | Not regular functions. The compiler knows their signatures, validates arguments, and generates specialized code per call site |
| **BF3: No shadowing** | Defining a function with a built-in name is a compile error |
| **BF4: No variadics** | `format`, `println`, `print` accept variable arguments through compiler support, not through a general variadic mechanism. The compiler parses template strings at compile time and type-checks each argument against its placeholder |

## Package Organization

| Rule | Description |
|------|-------------|
| **PO1: Directory = package** | All `.rk` files in directory form one package |
| **PO2: Name from path** | Package name derived from directory name |
| **PO3: Nested packages** | `pkg/sub/` is package `pkg.sub` (separate compilation unit) |

## Imports

| Rule | Description |
|------|-------------|
| **IM1: Qualified** | `import pkg` → access as `pkg.Name`. Every stdlib module works this way — there is no set that comes pre-imported, so a module name is an ordinary identifier until a program imports it, and a program that hasn't may bind a local called `net` or `json` |
| **IM2: Last-segment qualifier** | `import myapp.net.http` → access as `http.get()` |
| **IM3: Alias** | `import pkg as p` → access as `p.Name` |
| **IM4: Unqualified** | `import pkg.Name` → `Name` directly |
| **IM5: Grouped** | `import pkg.{Name, Other}` for multiple unqualified |
| **IM6: Glob** | `import pkg.*` — allowed but emits compiler warning |
| **IM7: Unused** | Unused imports warn (`tool.warnings/W1`, `unused_import`) — promote to error via `@deny` or `--deny-warnings` for CI/publish |
| **IM8: No shadowing** | Shadowing imported name with local definition is compile error |

<!-- test: skip -->
```rask
import http
import myapp.utils as u
import json.{decode, encode}

func main() {
    let req = http.new("GET", "/")
    let body = decode<JsonValue>(read())
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

## Struct Field Visibility

| Rule | Description |
|------|-------------|
| **SV1: All-public fields** | Literal construction allowed by anyone |
| **SV2: No private fields** | Literal construction allowed within same package |
| **SV3: Any private field** | Literal construction only in `extend` blocks — must provide factory for outside use |
| **SV4: Pattern matching** | Only visible fields are bindable; `private` fields require `..` |

## Trait Implementation Visibility

| Rule | Description |
|------|-------------|
| **TV1: Declared** | Trait conformance is package-internal by default; `public extend Type with Trait` exports it |
| **TV2: Capped by parts** | `public extend` requires both trait and type to be public — conformance can't be more visible than what it connects |

## Package-Level State

| Rule | Description |
|------|-------------|
| **PS1: Const only** | Package-level declarations must be `const` |
| **PS2: Sync required** | Mutable state requires `Atomic`, `Mutex`, or `Shared` (via interior mutability) |
| **PS3: No mutable globals** | `let` and `mut` at package level are compile errors (outside script mode) — bindings are local, `const` is the only package-level declaration |
| **PS4: Script mode** | Files without `main()` may have interleaved `const` declarations and statements. All top-level code runs in source order within a synthetic entry point. Declarations (func, struct, enum, import) are hoisted; `const` is not. |

## Package Initialization

| Rule | Description |
|------|-------------|
| **IN1: init function** | `init() -> void or Error { }` runs once before main |
| **IN2: One per file** | At most one `init()` per file |
| **IN3: Parallel topo sort** | Intra-package init order: parallel topological sort of file import DAG |
| **IN4: Deps first** | Inter-package: dependencies fully initialize before dependents |
| **IN5: Fail-fast** | Init returning `Err` cancels dependent inits immediately |

## Compilation Model

| Rule | Description |
|------|-------------|
| **CM1: Package = unit** | All files in package compiled together |
| **CM2: Public change** | `public` signature change recompiles importers |
| **CM3: Private change** | Non-public (package-visible or `private`) change recompiles package only |
| **CM4: Generic recompilation** | Generic body change recompiles instantiation sites (mitigated by semantic hash caching) |

## Error Messages

```
error[E0210]: `Duration` is not in scope
  --> app.rk:2:13
    |
  2 |     let d = Duration.seconds(1)
    |             ^^^^^^^^ needs an import
    = fix: import time.Duration
    = why: nothing comes pre-imported. A stdlib name is in scope where the program asked for it and nowhere else, which is also what leaves the name free for a program that wants it for something of its own [structure.modules/IM1]
```

`import time.Duration` rather than `import time`, because the code is already
written against the bare name — IM4 keeps it working where the whole-module form
would mean rewriting the use.

```
error[E0208]: `Duration` is already in scope from `time`
  --> app.rk:3:1
    |
  3 | struct Duration { a: i64 }
    | ^^^^^^^^^^^^^^^^^^^^^^^^^^ a second `Duration` in the same scope
    = fix: import time.Duration as StdDuration
    = why: one name, one meaning: with both in scope nothing in the source says which `Duration` a use means, and the compiler would be picking for you [struct.modules/IM8]
```

```
error[E0209]: `Vec` is a built-in type
  --> app.rk:1:1
    |
  1 | struct Vec { a: i64 }
    | ^^^^^^^^^^^^^^^^^^^^^ this name is already taken
    = fix: give it a name of its own, not `Vec`
    = why: the built-in set is fixed — a program can't add to it or replace a member — so a second type under this name would have no way to be referred to [struct.modules/BI2, BI3]
```

A built-in *function* gets a different reason, because it's worse: the compiler
generates code for `println` at each call site (BF2), so a program's own would be
compiled and then never called.

```
error[E0209]: `println` is a built-in function
  --> app.rk:1:1
    |
  1 | func println(x: i64) { return }
    | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ this name is already taken
    = fix: give it a name of its own, not `println`
```

```
ERROR [struct.modules/PS3]: mutable global
   |
5  |  mut counter: i32 = 0
   |  ^^^^^^^^^^^^^^^^^^^^ package-level `mut` not allowed; use Atomic or Shared
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

**V1 (package default):** Everything defaults to package-visible — functions, types, and fields. Same package = same team. No asymmetry to learn. I chose this over struct-private default because data-oriented design (plain structs accessed by functions) is as common as encapsulated types, and shouldn't require annotation tax. When you need encapsulation, `private` is explicit and signals "this field has invariants."

**BI2 (fixed built-in set):** Predictability. Reading any Rask file, you always know what's in scope. Go has no extension mechanism either — IDE auto-import handles repetition.

**IM7 (unused imports warn, not error):** Originally a Go-style hard error, which contradicted `tool.warnings/W1` and the severity boundary (SB1: errors are for broken code). An unused import is untidy, not unsafe — and a hard error bites exactly when you comment out a call to isolate a bug. The warning is default-on; teams that want Go's strictness get it back with `@deny(unused_import)` in `build.rk` or `--deny-warnings` in CI.

**RE2 (origin-based identity):** `std.Vec` (re-export of `core.Vec`) and `collections.Vec` (also `core.Vec`) are the same type. Preserves composability across re-export chains.

### Patterns

**Factory-first struct:**
<!-- test: skip -->
```rask
public struct Request {
    public method: string
    public path: string
    private id: u64             // private → factory required
}

extend Request {
    public func new(method: string, path: string) -> Request {
        Request { method, path, id: next_id() }   // OK: inside extend block
    }
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
    parent: Handle<Node>?
    children: Vec<Handle<Node>>
}
```

### Rejected: Workspace Visibility

**Workspace-level visibility (rejected):** Considered adding a 4th visibility level (`internal`) for items shared across workspace member packages but not exposed in the public API. Rejected because:

1. The primary use case (unpublished internal packages) is already handled — path deps can't be published (struct.packages/RG3), so `public` items in internal packages don't leak to external consumers.
2. Each visibility level must earn its existence by preventing real mistakes. `internal` would prevent nothing — it's a semantic distinction with no technical consequence for unpublished packages.
3. Three visibility levels (private, package-default, public) is already on the edge for a simplicity-focused language. Go and Zig thrive with two.

If multi-package published libraries become common, revisit with a package-level `internal: true` flag in `build.rk` (import restriction, not item-level visibility). The *package* is internal, not individual items.

### Rejected: File-Level Package Granularity

**File = package granularity (rejected):** Considered making each `.rk` file its own module/namespace (Zig-style) instead of directory = package (Go-style). Rejected because:

1. File = module adds 2-6 internal imports per file plus `public` on every cross-file function — strictly noisier than Go, violating the ergonomic litmus test.
2. Tightly-coupled code (editor commands + buffer, game systems + entities) ends up marking everything `public` anyway, making file-level encapsulation counterproductive.
3. Intra-package circular references are natural in systems code. File = module turns these into circular import errors, forcing architectural changes for module-system reasons rather than design reasons.
4. The existing "package = team" trust model (package-default visibility) already assumes files within a package cooperate freely.

Tradeoff acknowledged: directory = package makes intra-package dependencies invisible at the file level. This is a tooling problem (LSP go-to-definition), not a language problem. Scaling to large packages is handled by splitting into subdirectories — the same mechanism Go uses at Google scale.

### Implementation Status

| Feature | Status |
|---------|--------|
| Import syntax parsing | Implemented |
| Export syntax parsing | Implemented |
| Package discovery | Implemented |
| IM1 — nothing pre-imported (modules and types) | Implemented |
| IM3 — aliased import carries the type | Implemented |
| IM8 — no shadowing an import | Implemented |
| BI1/BI3 — built-in set, no shadowing | Implemented |
| BF1/BF3 — built-in functions, no shadowing | Implemented |
| Cross-package symbol lookup | Implemented |
| Visibility checking (public/package/private) | Implemented |
| Circular dependency detection | Implemented |
| Semver constraint parsing | Implemented |
| Feature resolution (additive + exclusive) | Implemented |
| Lock file with capability tracking | Implemented |

**Open, under IM1:** `import time` binds `Duration` as a bare name as well as
`time.Duration`. IM1 says how to reach a name through a whole-module import and
IM4 gives the unqualified form for a selective one, but neither says the bare
name is *unavailable* after `import time` — and the resolver registers a
module's types unqualified on purpose ("module functions are accessed qualified,
types are used unqualified per convention"). Reading IM1 as closed would mean
34 corpus files gain a selective import, `http.Response` and `http.Request`
being most of them. Worth settling one way or the other; I haven't — #999.

### See Also

- `struct.build` — build system, dependencies
- `struct.packages` — versioning, dependency resolution
- `struct.c-interop` — C imports, `extern "C"`
- `struct.targets` — libraries vs executables, entry points
