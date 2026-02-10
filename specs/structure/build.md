# Build System

## The Question
How do packages declare dependencies, features, and build configuration? How do build scripts work?

## Decision
`rask.build` is the single source of truth—both manifest and build script. A `package` block declares metadata, dependencies, features, and profiles using keyword syntax. An optional `build()` function handles code generation and native compilation. No TOML, no separate manifest.

## Rationale
Every Rask project needs dependencies. I chose Rask-native syntax over TOML because it eliminates a second language, enables type-checked metadata (typos in field names are compile errors), and keeps everything in one file. The `package` block is purely declarative—no code execution, just data. This lets the parser extract dependency information independently of build logic, so dep resolution works even if `func build()` has syntax errors.

I considered three alternatives: TOML manifest (Cargo-style), struct literals (verbose), and versioned imports (Zig-like, scatters deps across files). The keyword approach won because it's consistent with how Rask already works—`dep`, `feature`, `scope`, and `profile` follow the same grammar as `import`, `test`, and `struct`: keyword-first declarations, no parentheses.

---

## Package Block

**File:** `rask.build` in package root.

```rask
package "my-app" "1.0.0" {
    dep "http" "^2.0"
    dep "json" "^1.5"
}
```

**No `rask.build` needed** for zero-dependency packages. Package name inferred from directory, version 0.0.0.

### Keyword Grammar

All keywords inside `package` follow the same pattern:

```
keyword "name" ["version"] [{ key: value }]
```

Sub-blocks use map syntax (`key: value`, newline-separated)—consistent with struct field declarations.

| Keyword | Purpose | Example |
|---------|---------|---------|
| `dep` | Dependency | `dep "http" "^2.0"` |
| `feature` | Feature group/flag | `feature "ssl" { dep "openssl" "^3.0" }` |
| `scope` | Dependency scope | `scope "dev" { dep "mock" "^2.0" }` |
| `profile` | Custom build profile | `profile "embedded" { opt_level: "z" }` |

Package-level metadata uses map syntax directly:

| Key | Type | Purpose |
|-----|------|---------|
| `description` | string | Package description (for publishing) |
| `license` | string | SPDX license identifier |
| `repository` | string | Source repository URL |
| `members` | list | Workspace member directories |

```rask
package "json-parser" "2.1.0" {
    description: "Fast JSON parser for Rask"
    license: "MIT"
    repository: "https://github.com/alice/json-parser"

    dep "unicode" "^1.0"
}
```

### Structural Rules

1. **Purely declarative.** No `comptime if` inside the package block. Platform-specific deps use `{ target: "linux" }`.
2. **No group nesting.** `scope` inside `feature` or vice versa is a compile error. Use sub-block tags for cross-cutting.
3. **Forward references valid.** The block is declarative, not sequential—order doesn't matter semantically.
4. **Free-form ordering.** Convention: metadata → deps → scopes → features → profiles. Not enforced.
5. **One `package` block per file.** Duplicate is a compile error. Only valid in `.build` files.
6. **Build module auto-imported.** No `import build using ...` needed.
7. **Parser extracts `package` block independently.** Dep resolution works even if build logic has syntax errors.

---

## Dependencies

### Basic Dependencies

```rask
dep "http" "^2.0"
dep "json" "^1.5"
dep "sqlite" "^3.0"
```

Version strings use semver ranges: `"^2.0"` (compatible with 2.x), `">=1.5"` (minimum), `"=1.0.0"` (exact).

### Dep Sub-Block Properties

When a dep needs more than name + version, use a sub-block:

```rask
dep "epoll" "^1.0" { target: "linux" }

dep "my-fork" {
    git: "https://github.com/me/lib"
    branch: "fix-bug"
}

dep "shared" { path: "../shared" }

dep "shared" "^1.0" { path: "../shared" }  // path for dev, version for publishing

dep "tokio" "^1.0" { with: ["rt-multi-thread", "net"] }
```

| Key | Type | Purpose |
|-----|------|---------|
| `target` | string | Platform filter (`"linux"`, `"macos"`, `"windows"`) |
| `path` | string | Local path dependency |
| `git` | string | Git repository URL |
| `branch` | string | Git branch (requires `git`) |
| `with` | list | Enable features OF the dependency |
| `scope` | string | Scope override when inside a `feature` block |
| `feature` | string | Additional feature gate (multi-feature deps) |

**Version is optional** for path and git deps: `dep "shared" { path: "../shared" }`.

### Dependency Scopes

Group dev and build dependencies with `scope`:

```rask
scope "dev" {
    dep "mock-server" "^2.0"
    dep "test-utils" "^1.0"
    dep "bench-runner" "^1.0"
}

scope "build" {
    dep "codegen" "^1.0"
    dep "protobuf-gen" "^2.0"
}
```

- `scope "dev"` — test dependencies, not included in release builds
- `scope "build"` — build script dependencies, compiled before build script runs
- Scope blocks only contain `dep` statements

### Edge Cases

| Scenario | Behavior |
|----------|----------|
| Dep with both `path` and `git` | Error: conflicting sources |
| Same dep in `scope "dev"` and `scope "build"` | Valid—compiler merges (same version required) |
| Same dep declared twice at same level | Error: duplicate dependency |
| Dep outside `package` block | Error: dep must be inside package block |

---

## Features

Features declare optional functionality. Deps inside a feature block are automatically optional and gated by that feature.

### Basic Features

```rask
feature "ssl" {
    dep "openssl" "^3.0"
    dep "rustls" "^0.21"
}

feature "logging" {
    dep "zap" "^2.0"
}

feature "full" { enables: ["ssl", "logging"] }
```

When a user enables `--features ssl`, the openssl and rustls deps are included. Without it, they're excluded.

### Code-Only Features

Features without deps are flags for `comptime if` in regular code:

```rask
feature "verbose"
```

No block needed. Used in code as `comptime if cfg.features.contains("verbose")`.

### Default Features

All features are optional by default. Mark with `default: true`:

```rask
feature "json" {
    default: true
    dep "serde-json" "^1.0"
}
```

Users can disable defaults: `rask build --no-default-features`.

### Multi-Feature Dependencies

When a dep belongs to multiple features, tag it inside the primary feature block:

```rask
feature "ssl" {
    dep "openssl" "^3.0"
    dep "ring" "^0.17" { feature: "crypto" }  // also activated by "crypto"
}

feature "crypto" {
    dep "libsodium" "^1.0"
    // ring is also here via the tag above
}
```

`ring` is activated when EITHER ssl or crypto is enabled.

### Feature Sub-Block Properties

| Key | Type | Purpose |
|-----|------|---------|
| `enables` | list | Other features this activates (feature names only) |
| `default` | bool | Enabled by default (default: false) |

### Feature + Scope Cross-Cutting

A dep inside a feature block that's also dev-only:

```rask
feature "ssl" {
    dep "openssl" "^3.0"
    dep "ssl-test-helpers" "^1.0" { scope: "dev" }
}
```

### Feature Rules

- `enables` only references feature names—not dep names.
- Optional deps MUST live inside feature blocks. No top-level `{ optional: true }`.
- Same dep in multiple feature blocks is an error. Use `{ feature: "other" }` tag.
- Circular `enables` is a compile error.
- `enables` referencing unknown feature is a compile error.
- Redundant `{ feature: "ssl" }` inside `feature "ssl"` block is a warning.

---

## Custom Profiles

Built-in profiles: `debug` and `release`. Custom profiles use inheritance:

```rask
profile "embedded" {
    inherits: "release"
    opt_level: "z"
    panic: "abort"
}

profile "bench" {
    inherits: "release"
    lto: "fat"
    codegen_units: 1
}

profile "staging" {
    inherits: "release"
    debug_info: true
}
```

| Key | Type | Values | Effect |
|-----|------|--------|--------|
| `inherits` | string | Profile name | Base settings on another profile |
| `opt_level` | int/string | 0-3, `"s"`, `"z"` | Optimization level |
| `debug_info` | bool | true/false | Include debug symbols |
| `overflow_checks` | bool | true/false | Runtime integer overflow checks |
| `lto` | bool/string | false, true, `"thin"`, `"fat"` | Link-time optimization |
| `codegen_units` | int | 1-256 | Parallel codegen units |
| `strip` | bool | true/false | Strip debug symbols from binary |
| `panic` | string | `"unwind"`, `"abort"` | Panic strategy |

**CLI:**
```bash
rask build --profile embedded
rask build --release          # shorthand for --profile release
```

**Build script access:**
```rask
func build(ctx: BuildContext) -> () or Error {
    if ctx.profile.name == "embedded" {
        try ctx.compile_c(CompileOptions {
            sources: ["embedded_runtime.c"],
            flags: ["-Os", "-nostdlib"],
        })
    }
}
```

---

## Build Logic

The optional `func build()` runs after the package block is parsed and deps resolved. It handles code generation, native compilation, and external tools.

### Entry Point

```rask
func build(ctx: BuildContext) -> () or Error {
    const schema = try fs.read_file("schema.json")
    const code = generate_types(schema)
    try ctx.write_source("generated_types.rk", code)
}
```

**Rules:**
- `rask.build` is a separate compilation unit (compiled before the main package)
- Full Rask language available (I/O, pools, concurrency, C interop)
- Can't import the package being built (circular dependency)
- Build module auto-imported—`BuildContext`, `CompileOptions`, etc. available without `import`

### BuildContext API

```rask
struct BuildContext {
    public package_name: string
    public package_version: string
    public package_dir: Path

    public profile: ProfileInfo
    public target: Target
    public features: Set<string>    // enabled feature flags

    public gen_dir: Path            // .rk-gen/ for generated code
    public out_dir: Path            // build artifacts
}

struct ProfileInfo {
    public name: string             // "debug", "release", or custom
    public opt_level: i32
    public debug_info: bool
}

struct Target {
    public arch: string             // x86_64, aarch64, etc.
    public os: string               // linux, macos, windows, etc.
    public env: string              // gnu, musl, msvc, etc.
}
```

### Code Generation

| Method | Description |
|--------|-------------|
| `write_source(name: string, code: string)` | Write `.rk` file to `.rk-gen/` (auto-included in compilation) |
| `write_file(name: string, data: []u8)` | Write arbitrary file to out_dir |
| `declare_dependency(path: Path)` | Mark file as input (triggers rebuild on change) |

```rask
func build(ctx: BuildContext) -> () or Error {
    try ctx.declare_dependency("schema.json")
    try ctx.declare_dependency("templates/*.tmpl")  // glob supported

    const schema = try fs.read_file("schema.json")
    try ctx.write_source("types.rk", generate_types(schema))
}
```

**Generated file location:**
- `ctx.write_source("foo.rk")` → `.rk-gen/foo.rk`
- `.rk-gen/` automatically added to package source set
- Generated files have package visibility (not `public` unless written as such)

### C Build Integration

```rask
func build(ctx: BuildContext) -> () or Error {
    try ctx.compile_c(CompileOptions {
        sources: ["vendor/sqlite3.c"],
        include_dirs: ["vendor/"],
        flags: ["-O2", "-DSQLITE_OMIT_LOAD_EXTENSION"],
    })

    try ctx.link_library("pthread")
}
```

**CompileOptions:**

```rask
struct CompileOptions {
    sources: []string
    include_dirs: []string
    flags: []string
    define: Map<string, string>
}
```

| Method | Description |
|--------|-------------|
| `compile_c(opts: CompileOptions)` | Compile C sources to object files |
| `compile_rust(opts: RustCrateOptions)` | Compile Rust crate via cargo |
| `link_library(name: string)` | Link with system library (-l) |
| `link_search_path(path: string)` | Add library search path (-L) |
| `pkg_config(name: string)` | Use pkg-config for flags |

### Rust Crate Integration

Rust crates exporting C ABI functions can be compiled and linked via `compile_rust`. The Rust crate produces a static library; an optional cbindgen step generates a C header that Rask imports with the standard `import c` mechanism.

No new import syntax. Rust is just another source of C-ABI libraries.

**RustCrateOptions:**

```rask
struct RustCrateOptions {
    path: string
    version: string
    lib_name: string
    crate_type: string              // "staticlib" (default) or "cdylib"
    cbindgen: bool
    cbindgen_config: string
    header_name: string
    features: []string
    no_default_features: bool
    target: string
    flags: []string
    env: Map<string, string>
}
```

**Example — local crate with cbindgen:**

```rask
func build(ctx: BuildContext) -> () or Error {
    try ctx.compile_rust(RustCrateOptions {
        path: "vendor/fast-hash",
        cbindgen: true,
    })
}
```

```rask
// main.rk
import c ".rk-gen/fast_hash.h" as hash

func main() {
    const data = "hello world"
    const h = unsafe { hash.fast_hash_compute(data.ptr, data.len) }
    println("Hash: {h}")
}
```

**Linking:** Static linking default (`crate_type: "staticlib"`). Platform-specific Rust runtime deps auto-detected from cargo JSON output.

**Incremental rebuilds:** For local crates, `compile_rust` automatically declares dependencies on `{path}/src/**/*.rs` and `{path}/Cargo.toml`.

### Running External Tools

```rask
func build(ctx: BuildContext) -> () or Error {
    const result = try ctx.run(Command {
        program: "protoc",
        args: ["--rask_out=.rk-gen/", "schema.proto"],
        env: [],
    })

    if result.status != 0 {
        return Err(Error.new("protoc failed: {}", result.stderr))
    }
}
```

---

## Build Lifecycle

```
1. Parse rask.build — extract package block (independently of build logic)
2. Resolve dependencies (package block + rask.lock)
3. Fetch/compile scope "build" dependencies
4. Compile build logic in rask.build (if func build exists)
5. Execute func build() → generates files to .rk-gen/
6. Compile main package (includes .rk-gen/*.rk)
7. Link
```

**When build script runs:**

| Trigger | Runs? |
|---------|-------|
| First build | Yes |
| No changes | No (cached) |
| `rask.build` modified | Yes |
| Declared dependency modified | Yes |
| Source files modified | No (unless declared) |
| `rask build --force` | Yes |

---

## Conditional Compilation

Inside regular code (NOT the package block), use `comptime if` with the compiler-provided `cfg` constant:

```rask
func get_backend() -> Backend {
    comptime if cfg.os == "linux" {
        return LinuxBackend.new()
    } else if cfg.os == "macos" {
        return MacBackend.new()
    } else {
        return GenericBackend.new()
    }
}

func handle_request(req: Request) -> Response {
    comptime if cfg.features.contains("logging") {
        log.info("Request: {}", req.path)
    }
    // process request...
}
```

**`cfg` fields:**

| Field | Type | Source |
|-------|------|--------|
| `os` | string | Target OS (`"linux"`, `"macos"`, `"windows"`) |
| `arch` | string | Target architecture (`"x86_64"`, `"aarch64"`) |
| `env` | string | Target environment (`"gnu"`, `"musl"`, `"msvc"`) |
| `profile` | string | Build profile name (`"debug"`, `"release"`, custom) |
| `features` | Set\<string\> | Enabled feature flags |

**CLI:**
```bash
rask build --features ssl,logging
rask build --features full
rask build                        # default features only
rask build --no-default-features
```

---

## Debugging and Testing Build Scripts

### Testing

Build scripts can include `test` blocks and `@test` functions:

```rask
func generate_types(schema: string) -> string {
    // complex codegen logic...
}

@test
func roundtrip_codegen() -> bool {
    const output = generate_types('{"name": "string"}')
    assert output.contains("name: string")
    return true
}

test "empty schema produces empty output" {
    assert generate_types("{}") == ""
}
```

```bash
rask test rask.build
```

### Debugging

```bash
# Verbose build output
rask build --build-verbose

# Output:
# Parsing package block...
#   ✓ 3 dependencies, 1 feature
# Compiling build script...
#   ✓ Compiled rask.build
# Running build script...
#   [build] Reading schema.json (1.2 KB)
#   [build] Writing .rk-gen/types.rk (4.5 KB)
#   ✓ Build script succeeded (42ms)
```

### Resource Limits

```rask
// In the package block
build_timeout: 300         // seconds, default 300
build_max_memory: "1GB"    // default 1GB
```

Exceeding limits:
```
error: Build script exceeded timeout (300s)
  help: Increase in rask.build: build_timeout: 600
```

---

## Error Handling

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
15 |     try ctx.write_source("bad.rk", invalid_code)
   |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
   |
   = note: Generated file has syntax errors
   = note: See .rk-gen/bad.rk for generated content
```

---

## Edge Cases

| Case | Handling |
|------|----------|
| No `rask.build` | Package name from directory, version 0.0.0, no deps |
| `rask.build` with only `func build()` | Valid—no deps, just build logic |
| Empty package block | Valid—declares identity only |
| `rask.build` without `func build()` | Valid—just metadata, no build logic |
| Build script imports main package | Compile error: circular dependency |
| Generated file conflicts with source | Compile error: duplicate file |
| `package` block in regular `.rk` file | Compile error: only valid in `.build` files |
| Multiple `package` blocks | Compile error: duplicate |
| Cross-compilation | `ctx.target` and `cfg` reflect target, not host |
| Build script modifies source files | Allowed but discouraged |
| Parallel builds | Build script output dir isolated per build |

---

## Comptime vs Build Scripts

| Task | Use | Why |
|------|-----|-----|
| Compute lookup tables | Comptime | Pure computation |
| Embed file contents | Comptime (`@embed_file`) | Safe, read-only |
| Generate code from schema | Build script | Needs parsing libraries |
| Call external tools (protoc) | Build script | Subprocess execution |
| Compile C sources | Build script | C compiler invocation |
| Feature-based code paths | `comptime if cfg` | Compile-time branching |

---

## Full Example

```rask
package "my-api" "1.0.0" {
    description: "REST API server"
    license: "MIT"

    dep "http" "^2.0"
    dep "json" "^1.5"
    dep "config" "^3.0"
    dep "epoll" "^1.0" { target: "linux" }
    dep "kqueue" "^1.0" { target: "macos" }

    scope "dev" {
        dep "mock-server" "^2.0"
        dep "test-utils" "^1.0"
    }

    scope "build" {
        dep "protobuf-gen" "^2.0"
    }

    feature "ssl" {
        dep "openssl" "^3.0" { target: "linux" }
        dep "security-framework" "^2.0" { target: "macos" }
    }

    feature "logging" {
        dep "zap" "^2.0"
    }

    feature "full" { enables: ["ssl", "logging"] }

    profile "staging" {
        inherits: "release"
        debug_info: true
    }
}

func build(ctx: BuildContext) -> () or Error {
    try ctx.declare_dependency("api.proto")

    const result = try ctx.run(Command {
        program: "protoc",
        args: ["--rask_out=.rk-gen/", "api.proto"],
    })

    if result.status != 0 {
        return Err(Error.new("protoc failed: {}", result.stderr))
    }
}

func generate_rpc_stubs(proto: string) -> string {
    // codegen logic...
}

test "protobuf codegen produces valid output" {
    const output = generate_rpc_stubs("service Foo { rpc Bar(); }")
    assert output.contains("func bar")
}
```

## Integration Notes

- **Comptime:** Build scripts are separate from comptime. Comptime runs in-compiler (restricted subset); build scripts run as separate programs (full language). Use comptime for constants/embedding; build scripts for I/O-heavy codegen.
- **Package System:** Build scripts compiled like any package. Build-deps resolved via same MVS algorithm. Generated code has package visibility.
- **Module System:** `.rk-gen/` files are part of the package. `import` works normally. No special syntax for generated code.
- **Incremental Compilation:** Build script re-runs only when inputs change. Generated files cached. Main package recompiles if generated files change.
