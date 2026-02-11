<!-- id: struct.build -->
<!-- status: decided -->
<!-- summary: rask.build is manifest and build script; package block for deps, features, profiles -->
<!-- depends: structure/modules.md -->

# Build System

`rask.build` is the single source of truth — both manifest and build script. A `package` block declares metadata, dependencies, features, and profiles. An optional `build()` function handles code generation and native compilation.

## Package Block

| Rule | Description |
|------|-------------|
| **PK1: Single file** | `rask.build` in package root; one `package` block per file |
| **PK2: Declarative** | Package block is purely declarative — no `comptime if`, no code execution |
| **PK3: Independent parsing** | Parser extracts `package` block independently of build logic — dep resolution works even if `func build()` has syntax errors |
| **PK4: Optional** | No `rask.build` needed for zero-dependency packages — name from directory, version 0.0.0 |
| **PK5: Auto-import** | Build module auto-imported — `BuildContext`, `CompileOptions`, etc. available without `import` |

```rask
package "my-app" "1.0.0" {
    dep "http" "^2.0"
    dep "json" "^1.5"
}
```

### Keywords

All keywords inside `package` follow: `keyword "name" ["version"] [{ key: value }]`

| Keyword | Purpose | Example |
|---------|---------|---------|
| `dep` | Dependency | `dep "http" "^2.0"` |
| `feature` | Feature group/flag | `feature "ssl" { dep "openssl" "^3.0" }` |
| `scope` | Dependency scope | `scope "dev" { dep "mock" "^2.0" }` |
| `profile` | Custom build profile | `profile "embedded" { opt_level: "z" }` |

### Metadata Keys

| Key | Type | Purpose |
|-----|------|---------|
| `description` | string | Package description (for publishing) |
| `license` | string | SPDX license identifier |
| `repository` | string | Source repository URL |
| `members` | list | Workspace member directories |

## Dependencies

| Rule | Description |
|------|-------------|
| **D1: Semver ranges** | Version strings use semver: `"^2.0"` (compatible), `">=1.5"` (minimum), `"=1.0.0"` (exact) |
| **D2: Sub-block properties** | Extended config via sub-block: `dep "name" { target: "linux" }` |
| **D3: No duplicates** | Same dep declared twice at same level is a compile error |
| **D4: Scopes** | `scope "dev"` for test deps, `scope "build"` for build script deps |

```rask
dep "http" "^2.0"
dep "epoll" "^1.0" { target: "linux" }
dep "shared" { path: "../shared" }
dep "tokio" "^1.0" { with: ["rt-multi-thread", "net"] }
```

| Key | Type | Purpose |
|-----|------|---------|
| `target` | string | Platform filter |
| `path` | string | Local path dependency |
| `git` | string | Git repository URL |
| `branch` | string | Git branch (requires `git`) |
| `with` | list | Enable features of the dependency |
| `scope` | string | Scope override inside a `feature` block |
| `feature` | string | Additional feature gate |

## Features

| Rule | Description |
|------|-------------|
| **F1: Optional deps** | Deps inside a feature block are gated by that feature |
| **F2: Code-only features** | Features without deps are flags for `comptime if` |
| **F3: Default features** | `default: true` — users disable with `--no-default-features` |
| **F4: No cross-nesting** | `scope` inside `feature` or vice versa is a compile error — use sub-block tags |
| **F5: No duplicates** | Same dep in multiple feature blocks is an error — use `{ feature: "other" }` tag |
| **F6: No circular enables** | Circular `enables` references are a compile error |

```rask
feature "ssl" {
    dep "openssl" "^3.0"
    dep "ring" "^0.17" { feature: "crypto" }
}
feature "full" { enables: ["ssl", "logging"] }
feature "verbose"  // code-only flag
```

## Profiles

| Rule | Description |
|------|-------------|
| **PR1: Built-in** | `debug` and `release` are built-in profiles |
| **PR2: Inheritance** | Custom profiles inherit from another via `inherits` |

```rask
profile "embedded" {
    inherits: "release"
    opt_level: "z"
    panic: "abort"
}
```

| Key | Type | Values |
|-----|------|--------|
| `inherits` | string | Profile name |
| `opt_level` | int/string | 0-3, `"s"`, `"z"` |
| `debug_info` | bool | Include debug symbols |
| `overflow_checks` | bool | Runtime integer overflow checks |
| `lto` | bool/string | false, true, `"thin"`, `"fat"` |
| `codegen_units` | int | 1-256 |
| `strip` | bool | Strip debug symbols |
| `panic` | string | `"unwind"`, `"abort"` |

## Build Logic

| Rule | Description |
|------|-------------|
| **BL1: Separate unit** | `rask.build` is compiled before the main package |
| **BL2: Full language** | Build scripts have full Rask available (I/O, pools, concurrency, C interop) |
| **BL3: No self-import** | Can't import the package being built — circular dependency error |

```rask
func build(ctx: BuildContext) -> () or Error {
    try ctx.declare_dependency("schema.json")
    const schema = try fs.read_file("schema.json")
    try ctx.write_source("types.rk", generate_types(schema))
}
```

### BuildContext API

<!-- test: skip -->
```rask
struct BuildContext {
    public package_name: string
    public package_version: string
    public package_dir: Path
    public profile: ProfileInfo
    public target: Target
    public features: Set<string>
    public gen_dir: Path
    public out_dir: Path
}
```

### Code Generation

| Method | Description |
|--------|-------------|
| `write_source(name, code)` | Write `.rk` file to `.rk-gen/` (auto-included) |
| `write_file(name, data)` | Write arbitrary file to out_dir |
| `declare_dependency(path)` | Mark file as input (triggers rebuild on change) |

### Native Compilation

| Method | Description |
|--------|-------------|
| `compile_c(opts)` | Compile C sources to object files |
| `compile_rust(opts)` | Compile Rust crate via cargo (C ABI, cbindgen) |
| `link_library(name)` | Link with system library (-l) |
| `link_search_path(path)` | Add library search path (-L) |
| `pkg_config(name)` | Use pkg-config for flags |

## Build Lifecycle

| Rule | Description |
|------|-------------|
| **LC1: Order** | Parse package → resolve deps → compile build deps → run `func build()` → compile main package → link |
| **LC2: Caching** | Build script only re-runs when `rask.build` or declared dependencies change |

| Trigger | Runs? |
|---------|-------|
| First build | Yes |
| No changes | No (cached) |
| `rask.build` modified | Yes |
| Declared dependency modified | Yes |
| `rask build --force` | Yes |

## Conditional Compilation

| Rule | Description |
|------|-------------|
| **CC1: comptime if** | Use `comptime if cfg.field` in regular code (not package block) |
| **CC2: cfg fields** | `os`, `arch`, `env`, `profile`, `features` |

```rask
func get_backend() -> Backend {
    comptime if cfg.os == "linux" {
        return LinuxBackend.new()
    } else {
        return GenericBackend.new()
    }
}
```

## Error Messages

```
ERROR [struct.build/D3]: duplicate dependency
   |
5  |  dep "http" "^2.0"
   |  ^^^^^^^^^^ "http" already declared at line 3
```

```
ERROR [struct.build/BL3]: circular dependency
   |
8  |  import myapp
   |  ^^^^^^^^^^^^ build script cannot import the package being built
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| No `rask.build` | PK4 | Name from directory, version 0.0.0, no deps |
| Empty package block | PK1 | Valid — declares identity only |
| `package` in regular `.rk` file | PK1 | Compile error |
| Build script imports main package | BL3 | Compile error |
| Generated file conflicts with source | BL1 | Compile error: duplicate file |
| Dep with both `path` and `git` | D2 | Compile error: conflicting sources |
| Circular `enables` | F6 | Compile error |

---

## Appendix (non-normative)

### Rationale

**PK2 (declarative package block):** I chose Rask-native syntax over TOML because it eliminates a second language, enables type-checked metadata, and keeps everything in one file. The `package` block is purely declarative — no code execution. This lets the parser extract dependency information independently of build logic.

**BL2 (full language):** Build scripts need I/O, external tool execution, and sometimes concurrency. Restricting them to a subset creates artificial limitations. Full language access makes complex build scenarios (protoc, cbindgen, code generation) straightforward.

### Comptime vs Build Scripts

| Task | Use | Why |
|------|-----|-----|
| Compute lookup tables | Comptime | Pure computation |
| Embed file contents | Comptime (`@embed_file`) | Safe, read-only |
| Generate code from schema | Build script | Needs parsing libraries |
| Call external tools (protoc) | Build script | Subprocess execution |
| Compile C sources | Build script | C compiler invocation |
| Feature-based code paths | `comptime if cfg` | Compile-time branching |

### Rust Crate Integration

Rust crates exporting C ABI functions can be compiled and linked via `compile_rust`. The Rust crate produces a static library; an optional cbindgen step generates a C header that Rask imports with `import c`.

<!-- test: skip -->
```rask
func build(ctx: BuildContext) -> () or Error {
    try ctx.compile_rust(RustCrateOptions {
        path: "vendor/fast-hash",
        cbindgen: true,
    })
}
```

### Testing Build Scripts

```rask
@test
func roundtrip_codegen() -> bool {
    const output = generate_types('{"name": "string"}')
    assert output.contains("name: string")
    return true
}
```

Run with: `rask test rask.build`

### Full Example

```rask
package "my-api" "1.0.0" {
    description: "REST API server"
    license: "MIT"

    dep "http" "^2.0"
    dep "json" "^1.5"
    dep "epoll" "^1.0" { target: "linux" }

    scope "dev" { dep "mock-server" "^2.0" }
    scope "build" { dep "protobuf-gen" "^2.0" }

    feature "ssl" {
        dep "openssl" "^3.0" { target: "linux" }
        dep "security-framework" "^2.0" { target: "macos" }
    }
    feature "full" { enables: ["ssl", "logging"] }

    profile "staging" { inherits: "release", debug_info: true }
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
```

### See Also

- `struct.modules` — module system, imports, visibility
- `struct.packages` — versioning, dependency resolution, lock files
- `struct.c-interop` — C interop, `import c`, `extern "C"`
- `ctrl.comptime` — compile-time execution
