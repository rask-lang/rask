# Solution: Libraries vs Executables

## The Question
How do libraries differ from executables? What constitutes an entry point, can a package be both, and how is this configured?

## Decision
**Package role is determined by presence of `@entry` function.** No manifest, no configuration flags, no dual-purpose packages. Libraries export public API; executables contain an `@entry` function. Testing allows both patterns. Convention is to name the entry function `main`, but any name works.

## Rationale
Maximum simplicity: the compiler determines package role from code structure. No build files needed for basic usage. Follows "package = directory" principle—structure determines behavior. Separates "library with CLI example" from "application" clearly. Testing gets special handling because tests need to import the package they're testing while providing their own entry points.

The `@entry` attribute makes entry points explicit rather than relying on a magic function name. Developers unfamiliar with C conventions can see `@entry` and understand immediately that this is where the program starts.

## Specification

### Package Classification

| Pattern | Classification | Build Output |
|---------|---------------|--------------|
| Package with `@entry` function | Executable | Binary |
| Package without `@entry` | Library | No output (imported only) |
| Package with `*_test.rask` | Library + tests | Test binary (when testing) |

**Rules:**
- Presence of ANY `@entry` function → executable
- `@entry` function MUST be `public` (external tools need to find entry point)
- `@entry` function MUST be in root package directory (not nested packages)
- **Exactly one `@entry` per program** — multiple `@entry` functions is a compile error
- Nested packages (`pkg/sub/`) are ALWAYS libraries (cannot have `@entry`)

### Entry Point Signatures

| Signature | When to Use |
|-----------|-------------|
| `@entry public func main()` | Sync program, infallible |
| `@entry public func main() -> Result<()>` | Sync program, can fail |

The function name `main` is convention, not required. Any name works:
```rask
@entry
public func run() { ... }  // Valid entry point
```

**Error handling:**
- Returning `Err(e)` from `@entry`: process exits with non-zero status, error printed to stderr
- Panic in `@entry`: process exits with non-zero status, panic message printed
- Linear resources in `@entry`: must be consumed before return (same as any function)

### CLI Arguments

**Built-in type:** `Args` (always available, like `string`, `Vec`)

```rask
@entry
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
@entry
public func main() {
    try stdin.read_line()  // stdin: linear resource, can be consumed
    try stdout.write("hello\n")
    try stderr.write("error\n")
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
@entry
public func main() {
    print("done")
    // Exits with status 0 when main returns
}

@entry
public func main() -> Result<()> {
    if error { return Err(e) }  // Exits with status 1
    Ok(())  // Exits with status 0
}
```

**Explicit exit:**
```rask
import sys

@entry
public func main() {
    sys.exit(42)  // Immediate exit with status 42
}
```

**Exit behavior:**
- `@entry` returning → status 0
- `@entry` returning `Ok(())` → status 0
- `@entry` returning `Err(e)` → status 1, error printed to stderr
- `sys.exit(n)` → status n, immediate (no cleanup)
- Panic → status 101, panic message to stderr

**Cleanup on exit:**
- `ensure` blocks run before exit (unless `sys.exit()` used)
- Linear resources must be consumed before `@entry` returns
- Init errors: if package init fails, entry function never runs

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
public func new(method: string, path: string) -> Request { ... }

// NO @entry → this is a library
```

**Usage:**
```rask
// pkg: myapp
import http

@entry
public func main() {
    const req = http.new("GET", "/")
}
```

### Testing Pattern

**Test files can import the package they're testing:**

```rask
// pkg: http
// file: http/request.rask
public func parse(input: string) -> Result<Request> { ... }

// file: http/request_test.rask
import http  // Can import own package in tests

public func test_parse() {
    const req = try http.parse("GET / HTTP/1.1")
    assert(req.method == "GET")
}
```

**Test entry point:**
```rask
@entry
public func main() {
    // Auto-generated test runner (by test framework)
    run_all_tests()
}
```

**OR explicit test main:**
```rask
@entry
public func main(args: Args) {
    if args.len() > 1 && args[1] == "--benchmark" {
        run_benchmarks()
    } else {
        run_tests()
    }
}
```

**Rules:**
- Test files (`*_test.rask`) can have their own `@entry` for custom test runners
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
- Each binary file has its own `@entry` function
- Files NOT in `[[bin]]` list are library code (importable by binaries)
- Without rask.toml: compile one file at a time explicitly

### Examples Directory Pattern

**Common pattern:** Library with example executables

```bash
mylib/
  core.rask         # Library code, public API
  internal.rask     # Library code, pkg-visible
  examples/
    basic.rask      # Example program with @entry
    advanced.rask   # Another example with @entry
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
| `@entry` function not `public` | Compile error: entry point must be public |
| Multiple `@entry` in same package | Compile error: exactly one entry point allowed |
| `@entry` in nested package | Compile error: only root package can have entry point |
| `@entry async func` without async runtime | Runtime initialized automatically for main thread |
| Linear resource leak in `@entry` | Compile error: must consume before return |
| `sys.exit()` with unconsumed linear resource | Resource leaked (exit is unsafe operation) |
| Package has both library code and `@entry` | Legal: can `import myapp` OR `raskc myapp` (but confusing, discouraged) |
| Test with multiple `test_*.rask` files | Each file can have tests, one `@entry` total (framework-generated) |
| `init()` failure before `@entry` | Entry function never runs, process exits with init error |
| Args not used in `@entry` | Legal: `@entry func main()` or `@entry func main(_: Args)` both fine |

## Integration Notes

- **Memory Model**: `Args`, `stdin`, `stdout`, `stderr` are linear resources in `@entry` scope; must be consumed or explicitly leaked
- **Type System**: `@entry` signatures are checked for exact match (no inference of return type)
- **Module System**: Importing a package with `@entry` imports its library API, not its entry point (entry is special, never exported)
- **Error Handling**: `try` propagation works in `@entry func -> Result<()>`; error returned becomes process exit status
- **Concurrency**: `@entry async func` initializes async runtime for main thread; sync threads can be spawned from async entry
- **Compiler Architecture**: Entry point detection happens during package parsing; multiple `@entry` error caught early
- **C Interop**: `extern "C"` functions can be entry points for embedding Rask in C programs, but that's separate from `@entry`

## Comparison to Other Languages

| Language | Library/Executable Distinction | Entry Point |
|----------|-------------------------------|-------------|
| **Rask** | Presence of `@entry` | `@entry public func main()` |
| Rust | `Cargo.toml` `[lib]` vs `[[bin]]` | `fn main()` |
| Go | No distinction (package main) | `func main()` in package main |
| Zig | Build script `exe()` vs `lib()` | `pub fn main()` |
| Odin | Implicit (package main) | `main :: proc()` |
| C | Linker (main.o vs lib.a) | `int main()` |

**Rask approach:**
- No build script needed (simpler than Zig)
- No special package name needed (simpler than Go)
- Explicit attribute makes entry point self-documenting (no magic name knowledge required)
- Structure determines role (like Odin, but with explicit attribute)
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
