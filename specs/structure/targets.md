# Solution: Libraries vs Executables

## The Question
How do libraries differ from executables? What constitutes an entry point, can a package be both, and how is this configured?

## Decision
**Package role is determined by presence of `func main()`.** No manifest, no configuration flags, no dual-purpose packages. Libraries export public API; executables contain a `main` function. Testing allows both patterns.

`@entry` is optional—only needed to mark a non-main function as the entry point.

## Rationale
Maximum simplicity: compiler determines package role from code structure. No build files needed for basic usage. Follows "package = directory" principle—structure determines behavior. Separates "library with CLI example" from "application" clearly. Testing gets special handling because tests need to import the package they're testing while providing their own entry points.

`func main()` is a universal convention (C, Go, Rust, Java). Rask uses the same convention instead of requiring an attribute on every program. `@entry` exists for the rare case where you want a different entry point name.

## Specification

### Package Classification

| Pattern | Classification | Build Output |
|---------|---------------|--------------|
| Package with `func main()` or `@entry` | Executable | Binary |
| Package without entry point | Library | No output (imported only) |
| Package with `*_test.rk` | Library + tests | Test binary (when testing) |

**Rules:**
- `func main()` is the entry point by convention—no annotation needed
- `@entry` can mark a non-main function as entry point instead
- Entry function must be `public` (external tools need to find entry point)
- Entry function must be in root package directory (not nested packages)
- **Exactly one entry point per program** — multiple entry points is a compile error
- Nested packages (`pkg/sub/`) are always libraries (can't have entry points)

### Entry Point Signatures

| Signature | When to Use |
|-----------|-------------|
| `public func main()` | Sync program, infallible |
| `public func main() -> () or Error` | Sync program, can fail |

The function name `main` is convention. `@entry` can mark a different name:
```rask
@entry
public func run() { ... }  // Non-main entry point
```

**Error handling:**
- Returning `Err(e)` from entry: process exits with non-zero status, error printed to stderr
- Panic in entry: process exits with non-zero status, panic message printed
- Linear resources in entry: must be consumed before return (same as any function)

### CLI Arguments

**Built-in type:** `Args` (always available, like `string`, `Vec`)

```rask
public func main(args: Args) {
    for arg in args {
        print(arg)  // arg is string
    }
}
```

**API:**
```rask
struct Args { ... }  // opaque built-in

extend Args {
    func len(self) -> usize
    func get(self, i: usize) -> Option<string>
    func iter(self) -> ArgsIter
}

// Implements Iterate trait
for arg in args { ... }  // yields string
```

**Behavior:**
- `args[0]` is program name (like C, unlike Rust)
- `args.len()` includes program name
- Empty args (no CLI input) has length 1 (just program name)
- Arguments are always valid UTF-8 (platform-specific encoding handled by runtime)

### Standard Streams

**Built-in handles:** `stdin`, `stdout`, `stderr` (always available)

```rask
public func main() {
    try stdin.read_line()  // stdin: linear resource, can be consumed
    try stdout.write("hello\n")
    try stderr.write("error\n")
}
```

**Properties:**
- Linear resources (must be consumed exactly once)
- Available in `main()` scope without import
- Not available in other functions (pass as parameters if you need them)
- Can use `ensure` for cleanup (e.g., flush on exit)

### Process Exit

**Implicit exit:**
```rask
public func main() {
    print("done")
    // Exits with status 0 when main returns
}

public func main() -> () or Error {
    if error { return Err(e) }  // Exits with status 1
    Ok(())  // Exits with status 0
}
```

**Explicit exit:**
```rask
import sys

public func main() {
    sys.exit(42)  // Immediate exit with status 42
}
```

**Exit behavior:**
- `main` returning → status 0
- `main` returning `Ok(())` → status 0
- `main` returning `Err(e)` → status 1, error printed to stderr
- `sys.exit(n)` → status n, immediate (no cleanup)
- Panic → status 101, panic message to stderr

**Cleanup on exit:**
- `ensure` blocks run before exit (unless `sys.exit()` used)
- Linear resources must be consumed before entry returns
- If package init fails, entry function never runs

### Libraries (No main())

**Library packages:**
- Export `public` functions, types, traits
- Can't be executed directly
- Must be imported by executables or other libraries
- Can have `init()` for package initialization

**Example:**
```rask
// pkg: http
// file: http/request.rk
public struct Request { ... }
public func new(method: string, path: string) -> Request { ... }

// No func main() → this is a library
```

**Usage:**
```rask
// pkg: myapp
import http

public func main() {
    const req = http.new("GET", "/")
}
```

### Testing Pattern

**Test files can import the package they're testing:**

```rask
// pkg: http
// file: http/request.rk
public func parse(input: string) -> Request or Error { ... }

// file: http/request_test.rk
import http  // Can import own package in tests

public func test_parse() {
    const req = try http.parse("GET / HTTP/1.1")
    assert(req.method == "GET")
}
```

**Test entry point:**
```rask
public func main() {
    // Auto-generated test runner (by test framework)
    run_all_tests()
}
```

**OR explicit test main:**
```rask
public func main(args: Args) {
    if args.len() > 1 && args[1] == "--benchmark" {
        run_benchmarks()
    } else {
        run_tests()
    }
}
```

**Rules:**
- Test files (`*_test.rk`) can have their own entry point for custom test runners
- Tests access all package items (public and non-public)
- Test binaries are separate from package binary

### Build Configuration (Minimal)

**No configuration needed for basic cases:**
```rask
raskc myapp          # Builds myapp/main.rk → myapp binary
raskc mylib          # Error: no main() found
raskc --lib mylib    # Success: builds library (for checking only, no output)
```

**Optional build file:** `rask.build` (for dependencies, build logic, and complex cases)
```rask
package "myapp" "0.1.0" {
    dep "http" { path: "../http" }
    dep "json" "^1.0"
}

func build(ctx: BuildContext) -> () or Error {
    try ctx.compile_c(CSource {
        files: ["vendor/ssl.c"],
        include_paths: ["/usr/include"],
    })
}
```

**When you need rask.build:**
- External dependencies
- C library linking
- Build logic (code generation, etc.)
- Multi-binary projects (see below)

**When you DON'T need rask.build:**
- Single executable with no external dependencies
- Library with no C dependencies
- Local imports only

### Multi-Binary Projects

**Problem:** Some projects want multiple executables in one package.

**Solution:** Explicit file selection via CLI
```rask
raskc myapp/cli.rk → cli binary
raskc myapp/server.rk → server binary
```

**OR:** Define multiple binaries in `rask.build`:
```rask
package "myapp" "1.0.0" {
    bin: ["cli.rk", "server.rk"]
}
```

**Rules:**
- Each binary file has its own entry function (`func main()` or `@entry`)
- Files not in `bin` list are library code (importable by binaries)
- Without rask.build: compile one file at a time explicitly

### Examples Directory Pattern

**Common pattern:** Library with example executables

```bash
mylib/
  core.rk         # Library code, public API
  internal.rk     # Library code, pkg-visible
  examples/
    basic.rk      # Example program with func main()
    advanced.rk   # Another example with func main()
```

**Without rask.build:**
```bash
raskc mylib/examples/basic.rk → basic binary
```

**With rask.build:**
```rask
package "mylib" "1.0.0" {
    examples: ["examples/basic.rk", "examples/advanced.rk"]
}
```

**Behavior:**
- `raskc --examples mylib` builds all examples
- Examples can `import mylib` (import parent package)
- Examples aren't built by default (explicit opt-in)

### Edge Cases

| Case | Handling |
|------|----------|
| Entry function not `public` | Compile error: entry point must be public |
| Multiple entry points in same package | Compile error: exactly one entry point allowed |
| Entry point in nested package | Compile error: only root package can have entry point |
| `async func main()` without async runtime | Runtime initialized automatically for main thread |
| Linear resource leak in entry | Compile error: must consume before return |
| `sys.exit()` with unconsumed linear resource | Resource leaked (exit is unsafe operation) |
| Package has both library code and `main()` | Legal: can `import myapp` OR `raskc myapp` (but confusing, discouraged) |
| Test with multiple `test_*.rk` files | Each file can have tests, one entry point total (framework-generated) |
| `init()` failure before `main()` | Entry function never runs, process exits with init error |
| Args not used in entry | Legal: `func main()` or `func main(_: Args)` both fine |
| Both `func main()` and `@entry func run()` | Compile error: ambiguous entry point |

## Integration Notes

- **Memory Model**: `Args`, `stdin`, `stdout`, `stderr` are linear resources in entry scope; must be consumed or explicitly leaked
- **Type System**: Entry point signatures are checked for exact match (no inference of return type)
- **Module System**: Importing a package with `func main()` imports its library API, not its entry point (entry is special, never exported)
- **Error Handling**: `try` propagation works in `func main() -> () or Error`; error returned becomes process exit status
- **Concurrency**: `async func main()` initializes async runtime for main thread; sync threads can be spawned from async entry
- **Compiler Architecture**: Entry point detection happens during package parsing; `func main()` auto-detected, `@entry` on non-main caught early
- **C Interop**: `extern "C"` functions can be entry points for embedding Rask in C programs, separate from `func main()`

## Comparison to Other Languages

| Language | Library/Executable Distinction | Entry Point |
|----------|-------------------------------|-------------|
| **Rask** | Presence of `func main()` | `public func main()` |
| Rust | `Cargo.toml` `[lib]` vs `[[bin]]` | `fn main()` |
| Go | No distinction (package main) | `func main()` in package main |
| Zig | Build script `exe()` vs `lib()` | `pub fn main()` |
| Odin | Implicit (package main) | `main :: proc()` |
| C | Linker (main.o vs lib.a) | `int main()` |

**Rask approach:**
- No build script needed (simpler than Zig)
- No special package name needed (simpler than Go)
- `func main()` is universal convention—zero ceremony
- `@entry` available for non-main entry points (rare)
- Optional manifest only for complex cases (like Cargo, but not required)

## Migration Path

**Phase 1 (current):** No build system, manual compilation
- Use `raskc file.rk` for executables
- Libraries are checked but not built

**Phase 2:** Add rask.build support
- Dependencies, C linking
- Multi-binary projects
- Examples/tests configuration

**Phase 3:** Package manager
- Dependency resolution
- Lock files
- Registry integration

**This spec covers Phase 1 and Phase 2.** Package manager is out of scope.

## Remaining Issues

**RESOLVED:**
- ✅ Library vs executable distinction
- ✅ Entry point signature
- ✅ CLI arguments access
- ✅ Standard streams availability
- ✅ Test pattern
- ✅ Multi-binary projects

**DEFERRED:**
- Package manager design
- Dependency resolution algorithm
- Lock file format
- Version constraints
- Registry API

These are intentionally out of scope—focus is on language semantics first.
