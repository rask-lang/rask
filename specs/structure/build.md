<!-- id: struct.build -->
<!-- status: decided -->
<!-- summary: build.rk is manifest and build script; CLI, output dirs, cross-compilation, watch mode -->
<!-- depends: structure/modules.md, structure/packages.md -->

# Build System

`build.rk` is the single source of truth — both manifest and build script. A `package` block declares metadata, dependencies, features, and profiles. An optional `build()` function handles code generation and native compilation.

## Package Block

| Rule | Description |
|------|-------------|
| **PK1: Single file** | `build.rk` in package root; one `package` block per file |
| **PK2: Declarative** | Package block is purely declarative — no `comptime if`, no code execution |
| **PK3: Independent parsing** | Parser extracts `package` block independently of build logic — dep resolution works even if `func build()` has syntax errors |
| **PK4: Optional** | No `build.rk` needed for zero-dependency packages — name from directory, version 0.0.0 |
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

### Exclusive Feature Groups

| Rule | Description |
|------|-------------|
| **FG1: Mutual exclusion** | `feature "name" exclusive { ... }` — exactly one option selected |
| **FG2: Required default** | Exclusive features must declare `default: "option_name"` |
| **FG3: Named blocks** | Each string-named block has its own dependency set |
| **FG4: Additive + exclusive** | Additive features use set-union; exclusive groups require all selectors to agree |
| **FG5: Consumer selection** | Consumers select via dep sub-block: `dep "lib" { runtime: "tokio" }` |
| **FG6: Root wins** | Root package's selection overrides transitive selections |

<!-- test: skip -->
```rask
feature "runtime" exclusive {
    "tokio" {
        dep "tokio" "^1.0"
    }
    "async-std" {
        dep "async-std" "^1.12"
    }
    default: "tokio"
}
```

Consumer selects which option:

<!-- test: skip -->
```rask
dep "my-server" "^2.0" {
    runtime: "tokio"
}
```

## Dependency Permissions

I chose compile-time capability inference over author annotations — the compiler scans imports, so capabilities track actual behavior rather than trust.

| Rule | Description |
|------|-------------|
| **PM1: Inferred** | Capabilities are inferred from stdlib imports, not declared by package authors |
| **PM2: Root unrestricted** | Root package has no capability restrictions — only dependencies are gated |
| **PM3: Consumer consent** | `allow: ["net", "read"]` on dep declarations — explicit opt-in |
| **PM4: Build enforcement** | `rask build` errors if a dep uses capabilities not covered by `allow` |
| **PM5: Lock file tracking** | `rask.lock` records inferred capabilities per package |
| **PM6: Update detection** | `rask update` warns if a new version changes capability requirements |
| **PM7: Transitive** | A dep's capabilities include its own transitive deps' capabilities |
| **PM8: Build script sandboxing** | Build scripts run in the interpreter — fs/exec/net are interceptable |

| Import prefix | Capability |
|---------------|------------|
| `io.net`, `http` | `net` |
| `io.fs` | `read`, `write` |
| `os.exec`, `os.process` | `exec` |
| `unsafe`, `extern` | `ffi` |

```rask
dep "http-client" "^2.0" {
    allow: ["net"]
}
dep "parser" "^1.0"          // no capabilities needed — silent
dep "native-lib" "^3.0" {
    allow: ["ffi", "read"]   // uses extern + file reading
}
```

```
ERROR [struct.build/PM4]: capability violation
   dependency 'sketchy-lib' uses network access (io.net, http) but is not allowed
   add `allow: ["net"]` to the dep declaration in build.rk
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
| **BL1: Separate unit** | `build.rk` is compiled before the main package |
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

<!-- test: parse -->
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
| **LC2: Caching** | Build script only re-runs when `build.rk` or declared dependencies change |

| Trigger | Runs? |
|---------|-------|
| First build | Yes |
| No changes | No (cached) |
| `build.rk` modified | Yes |
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
| No `build.rk` | PK4 | Name from directory, version 0.0.0, no deps |
| Empty package block | PK1 | Valid — declares identity only |
| `package` in regular `.rk` file | PK1 | Compile error |
| Build script imports main package | BL3 | Compile error |
| Generated file conflicts with source | BL1 | Compile error: duplicate file |
| Dep with both `path` and `git` | D2 | Compile error: conflicting sources |
| Circular `enables` | F6 | Compile error |

## Build CLI

All commands live under the `rask` binary. No separate tools.

| Command | Description |
|---------|-------------|
| `rask build` | Build current package (debug profile) |
| `rask build --release` | Build with release profile |
| `rask build --profile <name>` | Build with custom profile |
| `rask build --target <triple>` | Cross-compile |
| `rask run [file] [-- args]` | Build and execute |
| `rask test [filter]` | Build and run tests |
| `rask bench [filter]` | Build and run benchmarks |
| `rask check` | Type-check without codegen |
| `rask watch [command]` | File watch + auto-rebuild |
| `rask add <pkg> [version]` | Add dependency to build.rk |
| `rask remove <pkg>` | Remove dependency from build.rk |
| `rask fetch` | Download dependencies |
| `rask update [pkg]` | Update to latest compatible versions |
| `rask publish` | Publish to registry |
| `rask vendor` | Copy deps to `vendor/` for offline builds |
| `rask audit` | Check deps for known vulnerabilities |
| `rask clean` | Remove build artifacts |
| `rask clean --all` | Remove build artifacts + cache |

| Rule | Description |
|------|-------------|
| **CL1: Zero-config default** | `rask build` works with no flags, no build.rk, no config |
| **CL2: Consistent flags** | `--release`, `--target`, `--verbose` work on all build commands |
| **CL3: Machine-readable output** | `--format json` on all commands for CI integration |
| **CL4: Exit codes** | 0 = success, 1 = build error, 2 = usage error |

### rask add

| Rule | Description |
|------|-------------|
| **AD1: Registry lookup** | Fetches latest compatible version from registry |
| **AD2: Preserves formatting** | Inserts dep line without reformatting existing content |
| **AD3: Dedup check** | Error if dep already exists (suggest `rask update <pkg>`) |
| **AD4: Lock update** | Runs resolution and updates `rask.lock` after adding |

```
$ rask add http
  Added dep "http" "^2.1.0" (latest: 2.1.0)

$ rask add openssl --feature ssl
  Added dep "openssl" "^3.1.0" to feature "ssl"

$ rask add mock-server --dev
  Added dep "mock-server" "^2.0.0" to scope "dev"
```

### rask remove

| Rule | Description |
|------|-------------|
| **RM1: Unused check** | Warns if removing a dep that other deps depend on transitively |
| **RM2: Lock update** | Updates `rask.lock` after removing |

## Output Directory

```
myproject/
  build.rk
  main.rk
  .rk-gen/                      # Generated source files (build script output)
  build/
    debug/
      myproject                  # Debug binary
    release/
      myproject                  # Release binary
    aarch64-linux/
      release/
        myproject                # Cross-compiled binary
    .cache/                      # Compilation cache
    .build-cache/
      steps/                     # Build step hashes
```

| Rule | Description |
|------|-------------|
| **OD1: Default location** | `build/` in project root. Override with `RASK_BUILD_DIR` |
| **OD2: Profile directories** | `build/<profile>/` — `debug`, `release`, or custom profile name |
| **OD3: Target directories** | `build/<target-triple>/<profile>/` for cross-compilation |
| **OD4: Binary naming** | Binary name = package name from `build.rk` (or directory name if no build.rk) |
| **OD5: .gitignore** | `rask build` auto-creates `build/.gitignore` with `*` on first run |
| **OD6: Clean** | `rask clean` removes `build/` entirely. `rask clean --all` also removes `~/.rask/cache/` entries for this project |
| **OD7: Multi-binary** | `rask build` builds all binaries. `rask run --bin <name>` selects which to run |

## Cross-Compilation

Cross-compilation for pure Rask code works without installing external toolchains.
C interop cross-compilation requires a cross-linker (the compiler tells you what you need).

### Target Triples

Format: `<arch>-<os>` or `<arch>-<os>-<env>`

| Component | Values |
|-----------|--------|
| `arch` | `x86_64`, `aarch64`, `arm`, `riscv64`, `wasm32` |
| `os` | `linux`, `macos`, `windows`, `freebsd`, `none` (bare metal) |
| `env` | `gnu`, `musl`, `msvc` (optional, has sane defaults) |

| Rule | Description |
|------|-------------|
| **XT1: Host default** | No `--target` flag → build for host platform |
| **XT2: Pure Rask** | Cross-compiling pure Rask code requires only the compiler |
| **XT3: C interop** | Cross-compiling with C deps requires a cross-linker. Compiler reports what's missing |
| **XT4: cfg access** | `comptime if cfg.os`, `comptime if cfg.arch` resolve to target, not host |
| **XT5: Build script runs on host** | `func build()` always executes on the host machine |
| **XT6: Target in BuildContext** | `ctx.target.arch`, `ctx.target.os`, `ctx.target.env` available in build scripts |
| **XT7: Platform-specific deps** | `dep "epoll" "^1.0" { target: "linux" }` — only included when targeting linux |
| **XT8: Multi-target** | Multiple `--target` flags build for all specified targets |
| **XT9: Target list** | `rask targets` lists all available targets with tier info |

### Target Tiers

**Tier 1 (tested, guaranteed):** `x86_64-linux`, `aarch64-linux`, `x86_64-macos`, `aarch64-macos`

**Tier 2 (builds, best-effort):** `x86_64-windows-msvc`, `aarch64-windows-msvc`, `wasm32-none`, `x86_64-linux-musl`, `aarch64-linux-musl`

**Tier 3 (community):** `riscv64-linux`, `x86_64-freebsd`, `arm-none`

## Watch Mode

| Rule | Description |
|------|-------------|
| **WA1: Default command** | `rask watch` → runs `rask check` on change (type-check only, no codegen — fastest feedback) |
| **WA2: Custom command** | `rask watch build`, `rask watch test`, `rask watch run` — any rask subcommand |
| **WA3: Debounce** | 100ms debounce — multiple rapid saves trigger one rebuild |
| **WA4: Scope** | Watches `.rk` files, `build.rk`, and declared build step inputs |
| **WA5: Clear output** | Clears terminal on each rebuild (disable with `--no-clear`) |
| **WA6: Error persistence** | Errors stay on screen until fixed (no scrolling away) |
| **WA7: Process management** | `rask watch run` kills the previous process before starting new one |
| **WA8: Signal forwarding** | Ctrl+C stops watch mode, sends SIGTERM to child process |

## Incremental Build Steps

| Rule | Description |
|------|-------------|
| **ST1: Input hashing** | Step inputs are content-hashed. Step skipped if all hashes match previous run |
| **ST2: Glob inputs** | Input paths can use globs: `"src/proto/*.proto"` |
| **ST3: Isolation** | Steps run sequentially in declaration order |
| **ST4: Failure stops** | If a step fails, subsequent steps don't run |
| **ST5: Cache location** | Step hashes stored in `build/.build-cache/steps/` |
| **ST6: Backwards compatible** | `declare_dependency()` still works — single implicit step covering the whole `build()` function |

```rask
func build(ctx: BuildContext) -> () or Error {
    try ctx.step("codegen", inputs: ["schema.json"], || {
        const schema = try fs.read_file("schema.json")
        try ctx.write_source("types.rk", generate_types(schema))
    })

    try ctx.step("protobuf", inputs: ["api.proto"], || {
        try ctx.exec("protoc", ["--rask_out=.rk-gen/", "api.proto"])
    })
}
```

### Tool version tracking

| Rule | Description |
|------|-------------|
| **TV1: Tool fingerprint** | When `tool` is specified, step also hashes the tool binary's version/path |
| **TV2: Version command** | `ctx.tool_version("protoc", "--version")` — records version string in cache |

## Expanded BuildContext

<!-- test: parse -->
```rask
struct BuildContext {
    public package_name: string
    public package_version: string
    public package_dir: Path
    public profile: ProfileInfo
    public target: Target
    public host: Target
    public features: Set<string>
    public gen_dir: Path
    public out_dir: Path
}
```

### Additional Methods

| Method | Description |
|--------|-------------|
| `step(name, inputs, body)` | Declare an incremental build step (ST1-ST6) |
| `exec(program, args) -> ExecResult or Error` | Run external command (errors on non-zero exit) |
| `exec_output(program, args) -> string or Error` | Run command, capture stdout |
| `tool_version(program, version_flag) -> string` | Record tool version for cache invalidation |
| `env(name) -> string?` | Read environment variable |
| `warning(msg)` | Emit build warning (shown to user) |
| `is_cross_compiling() -> bool` | `target != host` |
| `find_program(name) -> Path?` | Search PATH for executable |

## Compilation Pipeline

| Rule | Description |
|------|-------------|
| **PP1: Package parallelism** | Independent packages compile in parallel (up to CPU count) |
| **PP2: Pipeline parallelism** | Package B's parsing can start while package A is still in codegen |
| **PP3: Jobs flag** | `--jobs N` or `-j N` controls parallelism. Default: CPU count |

```
rask build
  ├─ 1. Find build.rk (or use defaults)
  ├─ 2. Parse package block (PK3)
  ├─ 3. Resolve dependencies (maximal compatible)
  ├─ 4. Check rask.lock (error if out of sync)
  ├─ 5. Download missing deps
  ├─ 6. Run build steps (if build() exists)
  ├─ 7. Compile packages (dependency order, parallel)
  │     ├─ Parse → Resolve → Type-check → Ownership-check
  │     ├─ Monomorphize → MIR → Cranelift/LLVM
  │     └─ Emit object file
  ├─ 8. Link → build/<profile>/<name>
  └─ 9. Report result
```

## Compilation Cache

| Rule | Description |
|------|-------------|
| **XC1: Local cache** | `~/.rask/cache/compiled/` stores compiled artifacts by cache key |
| **XC2: Hit = skip** | If cache key matches, skip compilation and use cached artifact |
| **XC3: Signature-based invalidation** | Dependency change only invalidates if its public API signature changes |
| **XC4: Cache size limit** | Default 2 GB. Configurable via `RASK_CACHE_SIZE`. LRU eviction |
| **XC5: No cache flag** | `--no-cache` forces full recompilation |

## Publishing

| Rule | Description |
|------|-------------|
| **PB1: Pre-checks** | `rask publish` runs check + test before uploading |
| **PB2: Required metadata** | `description` and `license` required for publishing |
| **PB3: Dry run** | `--dry-run` shows what would be published without uploading |
| **PB4: Authentication** | API token stored in `~/.rask/credentials` or `RASK_REGISTRY_TOKEN` |
| **PB5: No path deps** | Publish fails if package has path dependencies (struct.packages/RG3) |
| **PB6: Reproducible tarball** | Deterministic file ordering, no timestamps in archive |
| **PB7: Size limit** | 10 MB max package size. Error with breakdown if exceeded |

## Vendoring

| Rule | Description |
|------|-------------|
| **VD1: Copy** | `rask vendor` copies all resolved dependencies to `vendor/` |
| **VD2: Checksum preserved** | Vendored packages include their checksums for integrity |
| **VD3: vendor_dir config** | `vendor_dir: "vendor"` in package block enables vendor resolution |
| **VD4: Priority** | Vendor dir takes priority over registry. Lock file still required |
| **VD5: Offline** | With vendored deps, `rask build` works without network access |

## Dependency Auditing

| Rule | Description |
|------|-------------|
| **AU1: Advisory database** | Fetches from `https://advisories.rk-lang.org` |
| **AU2: Lock file based** | Checks exact versions from `rask.lock`, not constraints from `build.rk` |
| **AU3: Exit code** | Returns non-zero if vulnerabilities found (for CI gates) |
| **AU4: Ignore list** | `rask audit --ignore CVE-2024-1234` for acknowledged risks |
| **AU5: Offline mode** | `rask audit --db ./advisory-db.json` for air-gapped environments |

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

Run with: `rask test build.rk`

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
