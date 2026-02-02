# Solution: Libraries vs Executables

## The Question
How do libraries differ from executables? What constitutes an entry point, can a package be both, and how is this configured?

## Decision
**Package role is determined by presence of `main()` function.** No manifest, no configuration flags, no dual-purpose packages. Libraries export public API; executables contain `main()`. Testing allows both patterns.

## Rationale
Maximum simplicity: the compiler determines package role from code structure. No build files needed for basic usage. Follows "package = directory" principle—structure determines behavior. Separates "library with CLI example" from "application" clearly. Testing gets special handling because tests need to import the package they're testing while providing their own entry points.

## Specification

### Package Classification

| Pattern | Classification | Build Output |
|---------|---------------|--------------|
| Package with `main()` | Executable | Binary |
| Package without `main()` | Library | No output (imported only) |
| Package with `*_test.rask` | Library + tests | Test binary (when testing) |

**Rules:**
- Presence of ANY `public func main()` → executable
- `main()` MUST be `public` (external tools need to find entry point)
- `main()` MUST be in root package directory (not nested packages)
- Multiple files can each have `main()`; compiler error if more than one is built
- Nested packages (`pkg/sub/`) are ALWAYS libraries (cannot have `main()`)

### Entry Point Signatures

| Signature | When to Use |
|-----------|-------------|
| `public func main()` | Sync program, infallible |
| `public func main() -> Result<()>` | Sync program, can fail |

**Error handling:**
- Returning `Err(e)` from `main()`: process exits with non-zero status, error printed to stderr
- Panic in `main()`: process exits with non-zero status, panic message printed
- Linear resources in `main()`: must be consumed before return (same as any function)

### CLI Arguments

**Built-in type:** `Args` (always available, like `String`, `Vec`)

```rask
public func main(args: Args) {
    for arg in args {
        print(arg)  // arg is String
    }
}
```

**API:**
```rask
struct Args { ... }  // opaque built-in

extend Args {
    func len(self) -> usize
    func get(self, i: usize) -> Option<String>
    func iter(self) -> ArgsIter
}

// Implements Iterate trait
for arg in args { ... }  // yields String
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
    stdin.read_line()?  // stdin: linear resource, can be consumed
    stdout.write("hello\n")?
    stderr.write("error\n")?
}
```

**Properties:**
- Linear resources (must be consumed exactly once)
- Available in `main()` scope without import
- Not available in other functions (pass as parameters if needed)
- Can use `ensure` for cleanup (e.g., flush on exit)

### Process Exit

**Implicit exit:**
```rask
public func main() {
    print("done")
    // Exits with status 0 when main returns
}

public func main() -> Result<()> {
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
- `main()` returning → status 0
- `main()` returning `Ok(())` → status 0
- `main()` returning `Err(e)` → status 1, error printed to stderr
- `sys.exit(n)` → status n, immediate (no cleanup)
- Panic → status 101, panic message to stderr

**Cleanup on exit:**
- `ensure` blocks run before exit (unless `sys.exit()` used)
- Linear resources must be consumed before `main()` returns
- Init errors: if package init fails, main never runs

### Libraries (No main())

**Library packages:**
- Export `public` functions, types, traits
- Cannot be executed directly
- Must be imported by executables or other libraries
- Can have `init()` for package initialization

**Example:**
```rask
// pkg: http
// file: http/request.rask
public struct Request { ... }
public func new(method: String, path: String) -> Request { ... }

// NO main() → this is a library
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
// file: http/request.rask
public func parse(input: String) -> Result<Request> { ... }

// file: http/request_test.rask
import http  // Can import own package in tests

public func test_parse() {
    const req = http.parse("GET / HTTP/1.1")?
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
    if args.len() > 1 && args[1] == "--bench" {
        run_benchmarks()
    } else {
        run_tests()
    }
}
```

**Rules:**
- Test files (`*_test.rask`) can have their own `main()` for custom test runners
- Tests access all package items (public and non-public)
- Test binaries are separate from package binary

### Build Configuration (Minimal)

**No configuration needed for basic cases:**
```rask
raskc myapp          # Builds myapp/main.rask → myapp binary
raskc mylib          # Error: no main() found
raskc --lib mylib    # Success: builds library (for checking only, no output)
```

**Optional configuration file:** `rask.toml` (for complex cases only)
```
[package]
name = "myapp"
version = "0.1.0"

[dependencies]
http = { path = "../http" }
json = { version = "1.0" }

[build]
c_include_paths = ["/usr/include"]
c_link_libs = ["ssl", "crypto"]
```

**When you need rask.toml:**
- External dependencies
- C library linking
- Compiler flags
- Multi-binary projects (see below)

**When you DON'T need rask.toml:**
- Single executable with no external dependencies
- Library with no C dependencies
- Local imports only

### Multi-Binary Projects

**Problem:** Some projects want multiple executables in one package.

**Solution:** Explicit file selection via CLI
```rask
raskc myapp/cli.rask → cli binary
raskc myapp/server.rask → server binary
```

**OR:** Use rask.toml to define multiple binaries
```
[[bin]]
name = "cli"
path = "cli.rask"

[[bin]]
name = "server"
path = "server.rask"
```

**Rules:**
- Each binary file has its own `main()`
- Files NOT in `[[bin]]` list are library code (importable by binaries)
- Without rask.toml: compile one file at a time explicitly

### Examples Directory Pattern

**Common pattern:** Library with example executables

```bash
mylib/
  core.rask         # Library code, public API
  internal.rask     # Library code, pkg-visible
  examples/
    basic.rask      # Example program with main()
    advanced.rask   # Another example with main()
```

**Without rask.toml:**
```bash
raskc mylib/examples/basic.rask → basic binary
```

**With rask.toml:**
```toml
[[example]]
name = "basic"
path = "examples/basic.rask"

[[example]]
name = "advanced"
path = "examples/advanced.rask"
```

**Behavior:**
- `raskc --examples mylib` builds all examples
- Examples can `import mylib` (import parent package)
- Examples are NOT built by default (explicit opt-in)

### Edge Cases

| Case | Handling |
|------|----------|
| `main()` not `public` | Compile error: entry point must be public |
| Multiple `main()` in same package | Compile error (unless using rask.toml `[[bin]]`) |
| `main()` in nested package | Compile error: only root package can have main |
| `async func main()` without async runtime | Runtime initialized automatically for main thread |
| Linear resource leak in `main()` | Compile error: must consume before return |
| `sys.exit()` with unconsumed linear resource | Resource leaked (exit is unsafe operation) |
| Package has both library code and `main()` | Legal: can `import myapp` OR `raskc myapp` (but confusing, discouraged) |
| Test with multiple `test_*.rask` files | Each file can have tests, one `main()` total (framework-generated) |
| `init()` failure before `main()` | `main()` never runs, process exits with init error |
| Args not used in `main()` | Legal: `public func main()` or `public func main(_: Args)` both fine |

## Integration Notes

- **Memory Model**: `Args`, `stdin`, `stdout`, `stderr` are linear resources in `main()` scope; must be consumed or explicitly leaked
- **Type System**: `main()` signatures are checked for exact match (no inference of return type)
- **Module System**: Importing a package with `main()` imports its library API, not its entry point (main is special, never exported)
- **Error Handling**: `?` propagation works in `main() -> Result<()>`; error returned becomes process exit status
- **Concurrency**: `async func main()` initializes async runtime for main thread; sync threads can be spawned from async main
- **Compiler Architecture**: Entry point detection happens during package parsing; multiple `main()` error caught early
- **C Interop**: `extern "C"` functions can be entry points for embedding Rask in C programs, but that's separate from `main()`

## Comparison to Other Languages

| Language | Library/Executable Distinction | Entry Point |
|----------|-------------------------------|-------------|
| **Rask** | Presence of `main()` | `public func main()` |
| Rust | `Cargo.toml` `[lib]` vs `[[bin]]` | `func main()` |
| Go | No distinction (package main) | `func main()` in package main |
| Zig | Build script `exe()` vs `lib()` | `public func main()` |
| Odin | Implicit (package main) | `main :: proc()` |
| C | Linker (main.o vs lib.a) | `int main()` |

**Rask approach:**
- No build script needed (simpler than Zig)
- No special package name needed (simpler than Go)
- Structure determines role (like Odin, but with explicit public)
- Optional manifest only for complex cases (like Cargo, but not required)

## Migration Path

**Phase 1 (current):** No build system, manual compilation
- Use `raskc file.rask` for executables
- Libraries are checked but not built

**Phase 2:** Add rask.toml support
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
