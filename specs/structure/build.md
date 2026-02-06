# Build Scripts

## The Question
How do build scripts work? What is the file format, entry point, and API? How does code generation integrate with compilation?

## Decision
`rask.build` is a Rask source file with a `build(ctx: BuildContext)` entry point. The compiler provides a `build` module with types for declaring generated code, assets, and build configuration. Generated files go to `.rask-gen/` and are automatically included in compilation.

## Rationale
Full Rask in build scripts (unlike comptime's restricted subset) enables I/O-heavy tasks: reading schemas, calling external tools, generating code from templates. A structured API (not stdout directives like Cargo) makes build scripts type-checked and IDE-friendly. Automatic inclusion of generated files removes manual wiring. The `.rask-gen/` directory is gitignored but inspectable for debugging.

## Specification

### Build Script Location and Structure

**File:** `rask.build` in package root (alongside `rask.toml` if present).

**Entry point:**
```rask
import build using BuildContext

public func build(ctx: BuildContext) -> () or Error {
    // Build logic here
    Ok(())
}
```

**Rules:**
- `rask.build` is a separate compilation unit (compiled before the main package)
- Full Rask language available (I/O, pools, concurrency, C interop)
- Build scripts CANNOT import the package they're building (circular dependency)
- Build scripts CAN import external packages (declared in `[build-dependencies]`)

### BuildContext API

**Provided by compiler via `import build`:**

```rask
struct BuildContext {
    // Package information
    public package_name: string
    public package_version: string
    public package_dir: Path

    // Build configuration
    public profile: Profile           // debug | release | custom
    public target: Target             // host or cross-compilation target
    public features: Set<string>      // enabled feature flags

    // Output directories
    public gen_dir: Path              // .rask-gen/ for generated code
    public out_dir: Path              // build artifacts
}

enum Profile {
    Debug,
    Release,
    Custom(string),
}

struct Target {
    public arch: string       // x86_64, aarch64, etc.
    public os: string         // linux, macos, windows, etc.
    public env: string        // gnu, musl, msvc, etc.
}
```

### Code Generation

**Writing generated Rask code:**

```rask
public func build(ctx: BuildContext) -> () or Error {
    // Read schema file
    const schema = try fs.read_file("schema.json")

    // Generate code
    const code = generate_types(schema)

    // Write to gen_dir (automatically included in compilation)
    try ctx.write_source("generated_types.rask", code)

    Ok(())
}
```rask

**BuildContext methods for code generation:**

| Method | Description |
|--------|-------------|
| `write_source(name: string, code: string)` | Write `.rask` file to gen_dir |
| `write_file(name: string, data: []u8)` | Write arbitrary file to out_dir |
| `declare_dependency(path: Path)` | Mark file as input (triggers rebuild on change) |

**Generated file location:**
- `ctx.write_source("foo.rask")` → `.rask-gen/foo.rask`
- `.rask-gen/` is automatically added to package source set
- Generated files have package visibility (not `public` unless explicitly written as such)

### Build Dependencies

**Declared in `rask.toml`:**

```toml
[package]
name = "myapp"
version = "1.0.0"

[dependencies]
http = "2.1"

[build-dependencies]
# Only available to rask.build, not main package
json = "1.3"
codegen-utils = "0.5"
```

**Rules:**
- `[build-dependencies]` are separate from `[dependencies]`
- Build dependencies compiled first, used by build script
- Build dependencies NOT available to main package code
- Version conflicts between build-deps and deps: allowed (separate compilation)

### Build Lifecycle

**Execution order:**

```
1. Resolve dependencies (rask.toml + rask.lock)
2. Fetch/compile build-dependencies
3. Compile rask.build (if exists)
4. Execute rask.build → generates files to .rask-gen/
5. Compile main package (includes .rask-gen/*.rask)
6. Link
```

**When build script runs:**

| Trigger | Build script runs? |
|---------|-------------------|
| `rask build` (first time) | Yes |
| `rask build` (no changes) | No (cached) |
| `rask.build` modified | Yes |
| Declared dependency modified | Yes |
| Source files modified | No (unless declared) |
| `rask build --force` | Yes |

### Declaring Input Dependencies

**For incremental builds:**

```rask
public func build(ctx: BuildContext) -> () or Error {
    // Declare input files (rebuild if these change)
    try ctx.declare_dependency("schema.json")
    try ctx.declare_dependency("templates/*.tmpl")  // glob supported

    const schema = try fs.read_file("schema.json")
    // ... generate code

    Ok(())
}
```

**Automatic dependencies:**
- `rask.build` itself
- All `[build-dependencies]`
- Files read via `ctx.read_file()` (convenience wrapper)

### C Build Integration

**Compiling C code:**

```rask
public func build(ctx: BuildContext) -> () or Error {
    // Compile C sources
    try ctx.compile_c(CompileOptions {
        sources: ["vendor/sqlite3.c"],
        include_dirs: ["vendor/"],
        flags: ["-O2", "-DSQLITE_OMIT_LOAD_EXTENSION"],
    })

    // Link with system library
    try ctx.link_library("pthread")

    Ok(())
}
```

**CompileOptions:**

```rask
struct CompileOptions {
    sources: []string,           // C source files
    include_dirs: []string,      // -I paths
    flags: []string,             // Additional compiler flags
    define: Map<string, string>, // -D macros
}
```

**Methods:**

| Method | Description |
|--------|-------------|
| `compile_c(opts: CompileOptions)` | Compile C sources to object files |
| `compile_rust(opts: RustCrateOptions)` | Compile Rust crate via cargo, optionally generate header via cbindgen |
| `link_library(name: string)` | Link with system library (-l) |
| `link_search_path(path: string)` | Add library search path (-L) |
| `pkg_config(name: string)` | Use pkg-config for flags |

### Rust Crate Integration

Rust crates that export C ABI functions (`#[no_mangle] pub extern "C" fn`) can be compiled and linked via `compile_rust`. The Rust crate produces a static library; an optional cbindgen step generates a C header that Rask imports with the standard `import c` mechanism.

**No new import syntax.** Rust is just another source of C-ABI libraries.

**RustCrateOptions:**

```rask
struct RustCrateOptions {
    path: string                     // Local path or crates.io crate name
    version: string                  // Semver for crates.io (empty for local)
    lib_name: string                 // Override library name (default: from Cargo.toml)
    crate_type: string               // "staticlib" (default) or "cdylib"
    cbindgen: bool                   // Generate C header via cbindgen (default: false)
    cbindgen_config: string          // Path to cbindgen.toml (default: cbindgen defaults)
    header_name: string              // Output header filename (default: "{lib_name}.h")
    features: []string               // Cargo features to enable
    no_default_features: bool        // Disable default Cargo features
    target: string                   // Rust target triple (default: from ctx.target)
    flags: []string                  // Additional cargo build flags
    env: Map<string, string>         // Environment variables for cargo
}
```

**What `compile_rust` does:**

1. Resolve crate — local path (contains `/`) or fetch from crates.io (bare name + `version`)
2. Run `cargo build --lib --message-format=json` with profile/target/features
3. If `cbindgen: true`, run cbindgen → header written to `.rask-gen/{header_name}`
4. Auto-link the resulting static library
5. Auto-link Rust runtime system dependencies (detected from cargo JSON output)
6. Call `declare_dependency` on local crate sources for incremental rebuilds

**Example — local crate with cbindgen (common case):**

```rask
// rask.build
import build using BuildContext

public func build(ctx: BuildContext) -> () or Error {
    try ctx.compile_rust(RustCrateOptions {
        path: "vendor/fast-hash",
        cbindgen: true,
    })
    Ok(())
}
```

```rask
// main.rask
import c ".rask-gen/fast_hash.h" as hash

func main() {
    const data = "hello world"
    const h = unsafe { hash.fast_hash_compute(data.ptr, data.len) }
    println("Hash: {h}")
}
```

**Example — crates.io dependency:**

```rask
// rask.build
import build using BuildContext

public func build(ctx: BuildContext) -> () or Error {
    try ctx.compile_rust(RustCrateOptions {
        path: "blake3",
        version: "1.5",
        cbindgen: true,
        features: ["std"],
    })
    Ok(())
}
```

**Example — no cbindgen (manual bindings):**

```rask
// rask.build
import build using BuildContext

public func build(ctx: BuildContext) -> () or Error {
    try ctx.compile_rust(RustCrateOptions {
        path: "vendor/mylib",
    })
    Ok(())
}
```

```rask
// main.rask — explicit bindings instead of import c
extern "C" {
    func my_init() -> c_int
    func my_process(data: *u8, len: c_size) -> c_int
    func my_shutdown()
}
```

**Linking:**

Static linking is the default (`crate_type: "staticlib"`). This produces a self-contained archive with no runtime library path issues. Platform-specific Rust runtime dependencies are auto-detected from cargo's `--message-format=json` output:

| Platform | Typical dependencies |
|----------|---------------------|
| Linux (glibc) | `pthread`, `dl`, `m` |
| macOS | `System` |
| Windows (msvc) | `advapi32`, `ws2_32`, `userenv` |

Dynamic linking (`crate_type: "cdylib"`) is available for shared library use cases. The `.so`/`.dylib`/`.dll` must be distributed alongside the binary.

**Incremental rebuilds:**

For local crates, `compile_rust` automatically declares dependencies on `{path}/src/**/*.rs` and `{path}/Cargo.toml`. Cargo handles its own incremental compilation internally.

**Error cases:**

| Case | Handling |
|------|----------|
| `cargo` not found | Build error: "cargo not found. Install Rust toolchain: https://rustup.rs" |
| `cbindgen` not found | Build error: "cbindgen not found. Install: cargo install cbindgen" |
| Cargo compilation failure | Build error with cargo stderr |
| Crate not found on crates.io | Build error: "crate not found" |
| Missing `crate-type = ["staticlib"]` | Build error with fix instructions |
| No C-ABI exports | Warning: "no C-ABI symbols found" |
| Rust panic across FFI | Undefined behavior — Rust code must use `catch_unwind` at `extern "C"` boundaries |

### Running External Tools

```rask
public func build(ctx: BuildContext) -> () or Error {
    // Run protoc
    const result = try ctx.run(Command {
        program: "protoc",
        args: ["--rask_out=.rask-gen/", "schema.proto"],
        env: [],
    })

    if result.status != 0 {
        return Err(Error.new("protoc failed: {}", result.stderr))
    }

}
```

### Conditional Compilation

**Using features:**

```rask
public func build(ctx: BuildContext) -> () or Error {
    if ctx.features.contains("ssl") {
        try ctx.link_library("ssl")
        try ctx.link_library("crypto")

        // Generate feature flag for main code
        try ctx.write_source("features.rask", "public const SSL_ENABLED: bool = true")
    } else {
        try ctx.write_source("features.rask", "public const SSL_ENABLED: bool = false")
    }

}
```

**Feature declaration in rask.toml:**

```toml
[features]
default = ["json"]
ssl = []
json = []
full = ["ssl", "json"]
```

### Error Handling

**Build script failures:**

| Scenario | Behavior |
|----------|----------|
| Build script compile error | Compilation stops, error shown |
| Build script returns `Err` | Compilation stops, error message shown |
| Build script panics | Compilation stops, panic shown with backtrace |
| Generated code has errors | Compilation stops, error points to generated file |

**Error context:**
```
error: Build script failed
  --> rask.build:15:5
   |
15 |     try ctx.write_source("bad.rask", invalid_code)
   |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
   |
   = note: Generated file has syntax errors
   = note: See .rask-gen/bad.rask for generated content
```

### Edge Cases

| Case | Handling |
|------|----------|
| No `rask.build` file | Skip build script phase (most packages) |
| `rask.build` without entry point | Compile error: "Missing `public func build(ctx: BuildContext)`" |
| Build script imports main package | Compile error: "Circular dependency" |
| Generated file conflicts with source | Compile error: "Duplicate file" |
| Build script modifies source files | Allowed but discouraged (declare dependency) |
| Parallel builds | Build script output dir isolated per build |
| Cross-compilation | `ctx.target` reflects target, not host |

## Comptime vs Build Scripts

**Clear separation of concerns:**

| Task | Use | Why |
|------|-----|-----|
| Compute lookup tables | Comptime | Pure computation |
| Embed file contents | Comptime (`@embed_file`) | Safe, read-only, compile-time path |
| Generate code from schema | Build script | Needs parsing libraries, complex logic |
| Call external tools (protoc) | Build script | Subprocess execution |
| Compile C sources | Build script | Needs C compiler invocation |
| Conditional feature flags | Either | Simple flags → comptime; complex logic → build script |

**`@embed_file` in comptime:**

```rask
// Comptime file embedding (specified in comptime.md)
const SCHEMA: []u8 = comptime @embed_file("schema.json")
const VERSION: string = comptime @embed_file("VERSION")
```

**Constraints:**
- Path MUST be a string literal (no runtime values)
- Path is relative to package root
- Read-only (no side effects)
- File read at compile time, contents embedded in binary

This handles the common "embed this file" case. Build scripts are for **transforms**: reading input, processing it, writing generated code.

## Integration Notes

- **Comptime:** Build scripts are separate from comptime. Comptime runs in-compiler (restricted subset, plus `@embed_file`); build scripts run as separate programs (full language). Use comptime for constants/embedding; build scripts for I/O-heavy codegen.
- **Package System:** Build scripts are compiled like any package. Build-dependencies resolved via same MVS algorithm. Generated code has package visibility.
- **Module System:** `.rask-gen/` files are part of the package. `import` works normally. No special syntax for generated code.
- **Incremental Compilation:** Build script re-runs only when inputs change. Generated files cached. Main package recompiles if generated files change.

## Remaining Issues

### Medium Priority
1. **Build script debugging** — How to debug build scripts? Printf? Attach debugger?
2. **Build profiles** — How do custom profiles work beyond debug/release?
3. **Workspace build scripts** — Can workspaces have shared build logic?

### Low Priority
4. **Build script testing** — How to test build scripts?
