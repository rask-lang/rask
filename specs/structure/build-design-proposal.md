<!-- id: struct.build-v2 -->
<!-- status: proposed -->
<!-- summary: Comprehensive build system: incremental steps, cross-compilation, caching, CLI, security -->
<!-- depends: structure/build.md, structure/packages.md, structure/targets.md -->

# Build System Design — Proposal

Assessment of the current `struct.build` and `struct.packages` specs, plus proposed
additions. The goal: a build system that scores well on METRICS.md and is genuinely
better than Cargo, Go, and Zig for the common case.

## Assessment of Current State

**What's solid:**

The `rask.build` single-file design (PK1–PK5) is good. One file, Rask syntax, declarative
package block parseable independently of build logic (PK3). This beats Cargo's TOML + build.rs
split and avoids the "two languages" problem. MVS resolution (MV1–MV4) from `struct.packages`
is deterministic and simple. Lock files, registry basics, and workspaces are specified.

**What's missing or underspecified:**

| Gap | Impact | Priority |
|-----|--------|----------|
| No incremental build steps | Build scripts re-run entirely when any input changes | High |
| No cross-compilation | Can't target other platforms from `rask build` | High |
| No `rask add` / `rask remove` CLI | Manual editing of rask.build for every dep | High |
| No watch mode | Developers restart build manually after every change | High |
| No compilation cache | Switching git branches recompiles everything | Medium |
| No output directory spec | Where do binaries go? | Medium |
| No vendoring | Can't build offline | Medium |
| No `rask publish` workflow | Registry endpoint exists but no CLI flow | Medium |
| No dependency auditing | No way to check for known vulnerabilities | Medium |
| No build script sandboxing | Third-party build scripts have full system access | Deferred |
| No artifact naming rules | Binary names unspecified | Low |
| External tool versions untracked | `protoc` version drift breaks reproducibility | Low |

---

## 1. Build CLI

All commands live under the `rask` binary. No separate tools.

### Commands

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
| `rask add <pkg> [version]` | Add dependency to rask.build |
| `rask remove <pkg>` | Remove dependency from rask.build |
| `rask fetch` | Download dependencies |
| `rask update [pkg]` | Update to latest compatible versions |
| `rask publish` | Publish to registry |
| `rask vendor` | Copy deps to `vendor/` for offline builds |
| `rask audit` | Check deps for known vulnerabilities |
| `rask clean` | Remove build artifacts |
| `rask clean --all` | Remove build artifacts + cache |

### Rules

| Rule | Description |
|------|-------------|
| **CL1: Zero-config default** | `rask build` works with no flags, no rask.build, no config |
| **CL2: Consistent flags** | `--release`, `--target`, `--verbose` work on all build commands |
| **CL3: Machine-readable output** | `--format json` on all commands for CI integration |
| **CL4: Exit codes** | 0 = success, 1 = build error, 2 = usage error |

### `rask add`

Edits `rask.build` programmatically:

```
$ rask add http
  Added dep "http" "^2.1.0" (latest: 2.1.0)

$ rask add openssl --feature ssl
  Added dep "openssl" "^3.1.0" to feature "ssl"

$ rask add mock-server --dev
  Added dep "mock-server" "^2.0.0" to scope "dev"

$ rask add mylib --path ../mylib
  Added dep "mylib" { path: "../mylib" }
```

| Rule | Description |
|------|-------------|
| **AD1: Registry lookup** | Fetches latest compatible version from registry |
| **AD2: Preserves formatting** | Inserts dep line without reformatting existing content |
| **AD3: Dedup check** | Error if dep already exists (suggest `rask update <pkg>`) |
| **AD4: Lock update** | Runs resolution and updates `rask.lock` after adding |

### `rask remove`

```
$ rask remove http
  Removed dep "http" from package block
```

| Rule | Description |
|------|-------------|
| **RM1: Unused check** | Warns if removing a dep that other deps depend on transitively |
| **RM2: Lock update** | Updates `rask.lock` after removing |

---

## 2. Incremental Build Steps

The current `func build()` runs as a monolith — if any declared dependency changes,
everything re-runs. This is Cargo's `build.rs` problem. For packages with multiple
code generation steps (protobuf + schema + C compilation), it's wasteful.

### Step API

```rask
func build(ctx: BuildContext) -> () or Error {
    // Step 1: only re-runs when schema.json changes
    try ctx.step("codegen", inputs: ["schema.json"], || {
        const schema = try fs.read_file("schema.json")
        try ctx.write_source("types.rk", generate_types(schema))
    })

    // Step 2: only re-runs when .proto files change
    try ctx.step("protobuf", inputs: ["api.proto", "models.proto"], || {
        try ctx.exec("protoc", ["--rask_out=.rk-gen/", "api.proto", "models.proto"])
    })

    // Step 3: C compilation — re-runs when C sources change
    try ctx.step("native", inputs: ["vendor/zlib/*.c"], || {
        try ctx.compile_c(CompileOptions {
            sources: ["vendor/zlib/*.c"],
            include: ["vendor/zlib/"],
            flags: ["-O2"],
        })
    })
}
```

### Rules

| Rule | Description |
|------|-------------|
| **ST1: Input hashing** | Step inputs are content-hashed. Step skipped if all hashes match previous run |
| **ST2: Glob inputs** | Input paths can use globs: `"src/proto/*.proto"` |
| **ST3: Isolation** | Steps run sequentially in declaration order (no implicit parallelism) |
| **ST4: Failure stops** | If a step fails, subsequent steps don't run |
| **ST5: Cache location** | Step hashes stored in `build/.build-cache/steps/` |
| **ST6: Backwards compatible** | `declare_dependency()` still works — it's a single implicit step covering the whole `build()` function |

### Step vs declare_dependency

| Pattern | Use when |
|---------|----------|
| `declare_dependency()` | Simple build scripts with one logical operation |
| `ctx.step()` | Multiple independent operations that should cache separately |

Both are valid. `declare_dependency()` is sugar for a single unnamed step.

### Tool version tracking

```rask
try ctx.step("protobuf", inputs: ["api.proto"], tool: "protoc", || {
    try ctx.exec("protoc", ["--rask_out=.rk-gen/", "api.proto"])
})
```

| Rule | Description |
|------|-------------|
| **TV1: Tool fingerprint** | When `tool` is specified, step also hashes the tool binary's version/path |
| **TV2: Version command** | `ctx.tool_version("protoc", "--version")` — records version string in cache |

---

## 3. Output Directory Structure

```
myproject/
  rask.build
  main.rk
  lib.rk
  .rk-gen/                      # Generated source files (build script output)
  build/
    debug/
      myproject                  # Debug binary
    release/
      myproject                  # Release binary
    aarch64-linux/
      release/
        myproject                # Cross-compiled binary
    .cache/                      # Compilation cache (content-addressed)
    .build-cache/
      steps/                     # Build step hashes
      lock                       # Build lock file (prevents concurrent builds)
```

### Rules

| Rule | Description |
|------|-------------|
| **OD1: Default location** | `build/` in project root. Override with `RASK_BUILD_DIR` |
| **OD2: Profile directories** | `build/<profile>/` — `debug`, `release`, or custom profile name |
| **OD3: Target directories** | `build/<target-triple>/<profile>/` for cross-compilation |
| **OD4: Binary naming** | Binary name = package name from `rask.build` (or directory name if no rask.build) |
| **OD5: .gitignore** | `rask build` auto-creates `build/.gitignore` with `*` on first run |
| **OD6: Clean** | `rask clean` removes `build/` entirely. `rask clean --all` also removes `~/.rask/cache/` entries for this project |

---

## 4. Cross-Compilation

Cross-compilation for pure Rask code should work without installing external toolchains.
C interop cross-compilation requires a cross-linker (the compiler tells you what you need).

### Target Triples

Format: `<arch>-<os>` or `<arch>-<os>-<env>`

| Component | Values |
|-----------|--------|
| `arch` | `x86_64`, `aarch64`, `arm`, `riscv64`, `wasm32` |
| `os` | `linux`, `macos`, `windows`, `freebsd`, `none` (bare metal) |
| `env` | `gnu`, `musl`, `msvc` (optional, has sane defaults) |

Examples: `aarch64-linux`, `x86_64-windows-msvc`, `wasm32-none`, `x86_64-linux-musl`

### Rules

| Rule | Description |
|------|-------------|
| **XT1: Host default** | No `--target` flag → build for host platform |
| **XT2: Pure Rask** | Cross-compiling pure Rask code requires only the compiler. No external toolchains |
| **XT3: C interop** | Cross-compiling with C deps requires a cross-linker. Compiler reports what's missing |
| **XT4: cfg access** | `comptime if cfg.os`, `comptime if cfg.arch` resolve to target, not host |
| **XT5: Build script runs on host** | `func build()` always executes on the host machine |
| **XT6: Target in BuildContext** | `ctx.target.arch`, `ctx.target.os`, `ctx.target.env` available in build scripts |
| **XT7: Platform-specific deps** | `dep "epoll" "^1.0" { target: "linux" }` — only included when targeting linux |

### Built-in Targets

The compiler ships with target descriptions for common platforms. No downloading required.

**Tier 1 (tested, guaranteed to work):**
- `x86_64-linux`
- `aarch64-linux`
- `x86_64-macos`
- `aarch64-macos`

**Tier 2 (builds, best-effort testing):**
- `x86_64-windows-msvc`
- `aarch64-windows-msvc`
- `wasm32-none`
- `x86_64-linux-musl`
- `aarch64-linux-musl`

**Tier 3 (community-maintained):**
- `riscv64-linux`
- `x86_64-freebsd`
- `arm-none` (bare metal)

### CLI

```
$ rask build --target aarch64-linux
  Compiling myapp for aarch64-linux (release)
  Finished: build/aarch64-linux/release/myapp

$ rask build --target aarch64-linux --target x86_64-linux
  Compiling myapp for aarch64-linux (release)
  Compiling myapp for x86_64-linux (release)
  Finished: 2 targets
```

| Rule | Description |
|------|-------------|
| **XT8: Multi-target** | Multiple `--target` flags build for all specified targets |
| **XT9: Target list** | `rask targets` lists all available targets with tier info |

---

## 5. Watch Mode

`rask watch` monitors source files and re-runs the build pipeline when they change.
Because of Rask's compilation model (package = compilation unit, no whole-program analysis),
changing one file recompiles only its package and dependents.

### Rules

| Rule | Description |
|------|-------------|
| **WA1: Default command** | `rask watch` → runs `rask check` on change (type-check only, no codegen — fastest feedback) |
| **WA2: Custom command** | `rask watch build`, `rask watch test`, `rask watch run` — any rask subcommand |
| **WA3: Debounce** | 100ms debounce — multiple rapid saves trigger one rebuild |
| **WA4: Scope** | Watches `.rk` files, `rask.build`, and declared build step inputs |
| **WA5: Clear output** | Clears terminal on each rebuild (disable with `--no-clear`) |
| **WA6: Error persistence** | Errors stay on screen until fixed (no scrolling away) |

```
$ rask watch
  Watching 23 files in 4 packages...
  [12:34:56] Change: src/parser.rk → checking...
  [12:34:56] OK (2 packages rebuilt)

  [12:35:02] Change: src/lexer.rk → checking...
  [12:35:02] ERROR [type.structs/S1]: ...
```

```
$ rask watch run -- --port 8080
  Watching 23 files...
  [12:34:56] Change detected → building + running...
  Server listening on :8080

  [12:35:10] Change detected → restarting...
  Server listening on :8080
```

| Rule | Description |
|------|-------------|
| **WA7: Process management** | `rask watch run` kills the previous process before starting new one |
| **WA8: Signal forwarding** | Ctrl+C stops watch mode, sends SIGTERM to child process |

---

## 6. Compilation Cache

Content-addressed compilation cache. The same source + deps + compiler + target + profile
produces the same output. This means:

- Switching git branches and back doesn't recompile unchanged packages
- CI machines can share a remote cache
- Rebuilds after `rask clean` are fast if the cache is populated

### Cache Key

```
cache_key = hash(
    source_content_hash,       # hash of all .rk files in the package
    dependency_signatures,     # public API hashes of all dependencies
    compiler_version,          # rask compiler version
    target_triple,             # e.g., x86_64-linux
    profile_settings,          # opt_level, debug_info, etc.
)
```

### Rules

| Rule | Description |
|------|-------------|
| **CC1: Local cache** | `~/.rask/cache/compiled/` stores compiled artifacts by cache key |
| **CC2: Hit = skip** | If cache key matches, skip compilation and use cached artifact |
| **CC3: Signature-based invalidation** | Dependency change only invalidates if its public API signature changes (not internal changes) |
| **CC4: Cache size limit** | Default 2 GB. Configurable via `RASK_CACHE_SIZE`. LRU eviction |
| **CC5: No cache flag** | `--no-cache` forces full recompilation |

### Remote Cache (future, v2)

| Rule | Description |
|------|-------------|
| **RC1: Protocol** | HTTP GET/PUT with cache key as path |
| **RC2: Config** | `RASK_REMOTE_CACHE=https://cache.example.com` |
| **RC3: Read-only option** | `RASK_REMOTE_CACHE_READONLY=true` for CI pull requests |

---

## 7. Publishing

### Workflow

```
$ rask publish --dry-run
  Package: my-api 1.0.0
  Files: 12 (.rk) + rask.build
  Size: 45 KB
  Dependencies: http ^2.0, json ^1.5
  Checks:
    ✓ No path dependencies (RG3)
    ✓ Version not already published
    ✓ rask check passes
    ✓ rask test passes
    ✗ Missing: description, license

$ rask publish
  Publishing my-api 1.0.0 to packages.rk-lang.org...
  Published: https://packages.rk-lang.org/pkg/my-api/1.0.0
```

### Rules

| Rule | Description |
|------|-------------|
| **PB1: Pre-checks** | `rask publish` runs check + test before uploading |
| **PB2: Required metadata** | `description` and `license` required for publishing |
| **PB3: Dry run** | `--dry-run` shows what would be published without uploading |
| **PB4: Authentication** | API token stored in `~/.rask/credentials` or `RASK_REGISTRY_TOKEN` |
| **PB5: No path deps** | Publish fails if package has path dependencies (struct.packages/RG3) |
| **PB6: Reproducible tarball** | Deterministic file ordering, no timestamps in archive |
| **PB7: Size limit** | 10 MB max package size. Error with breakdown if exceeded |

### Yanking

```
$ rask yank my-api 1.0.0 --reason "security vulnerability in auth module"
  Yanked my-api 1.0.0
  Existing lock files still resolve. New resolution skips this version.
```

| Rule | Description |
|------|-------------|
| **YK1: Soft delete** | Yanked versions aren't selected by new resolution but existing lock files still work |
| **YK2: Reason required** | Must provide a reason string |
| **YK3: Reversible** | `rask yank --undo` un-yanks within 72 hours |

---

## 8. Vendoring

```
$ rask vendor
  Vendored 15 packages to vendor/
  Add to rask.build: vendor_dir: "vendor"
```

### Rules

| Rule | Description |
|------|-------------|
| **VD1: Copy** | `rask vendor` copies all resolved dependencies to `vendor/` |
| **VD2: Checksum preserved** | Vendored packages include their checksums for integrity |
| **VD3: vendor_dir config** | `vendor_dir: "vendor"` in package block enables vendor resolution |
| **VD4: Priority** | Vendor dir takes priority over registry. Lock file still required |
| **VD5: Offline** | With vendored deps, `rask build` works without network access |

---

## 9. Dependency Auditing

```
$ rask audit
  Checking 23 dependencies against advisory database...

  VULNERABILITY: openssl 3.0.2
    CVE-2024-1234: Buffer overflow in TLS handshake
    Severity: HIGH
    Fixed in: 3.0.8
    Fix: rask update openssl

  1 vulnerability found (1 high, 0 medium, 0 low)
```

### Rules

| Rule | Description |
|------|-------------|
| **AU1: Advisory database** | Fetches from `https://advisories.rk-lang.org` |
| **AU2: Lock file based** | Checks exact versions from `rask.lock`, not constraints from `rask.build` |
| **AU3: Exit code** | Returns non-zero if vulnerabilities found (for CI gates) |
| **AU4: Ignore list** | `rask audit --ignore CVE-2024-1234` for acknowledged risks |
| **AU5: Offline mode** | `rask audit --db ./advisory-db.json` for air-gapped environments |

---

## 10. Build Script Security (Deferred)

Build scripts (`func build()`) have full system access (BL2). This is necessary for
calling external tools, but it's a risk for third-party packages. Nobody does this
well today — Cargo's build.rs and npm's postinstall are known attack vectors.

Deferred to v2. Keeping these rules as a future direction — the UX needs real-world
usage before deciding between prompt-based, sandbox-based, or capability-based approaches.

### Future Direction (not for v1)

| Rule | Description |
|------|-------------|
| **BS1: First-party unrestricted** | Your own package's build script runs without restrictions |
| **BS2: Dependency prompt** | First build with a new dependency that has a build script shows a prompt: "package X wants to run a build script. Allow? [y/N]" |
| **BS3: Allowlist** | Allowed build scripts recorded in `rask.lock` (hash of the build function) |
| **BS4: Hash change** | If a dependency's build script changes on update, re-prompt |
| **BS5: CI mode** | `--trust-build-scripts` flag for CI (no prompts) |
| **BS6: Audit trail** | `rask.lock` records which packages have build scripts and their hashes |

---

## 11. Expanded BuildContext API

Adding to the existing API from `struct.build`:

```rask
struct BuildContext {
    // Existing fields
    public package_name: string
    public package_version: string
    public package_dir: Path
    public profile: ProfileInfo
    public target: Target
    public features: Set<string>
    public gen_dir: Path
    public out_dir: Path

    // New: host vs target distinction
    public host: Target
}
```

### New Methods

| Method | Description |
|--------|-------------|
| `step(name, inputs, body)` | Declare an incremental build step (ST1-ST6) |
| `exec(program, args) -> ExecResult or Error` | Run external command (replaces ad-hoc Command usage) |
| `exec_output(program, args) -> string or Error` | Run command, capture stdout |
| `tool_version(program, version_flag) -> string` | Record tool version for cache invalidation |
| `env(name) -> string?` | Read environment variable |
| `warning(msg)` | Emit build warning (shown to user) |
| `is_cross_compiling() -> bool` | `target != host` |
| `find_program(name) -> Path?` | Search PATH for executable |

### exec vs run

The current spec shows `ctx.run(Command { ... })`. I'd replace this with `ctx.exec()` for
consistency with the rest of the API. `exec` is a method, not a manual struct construction:

```rask
// Current (verbose)
const result = try ctx.run(Command {
    program: "protoc",
    args: ["--rask_out=.rk-gen/", "api.proto"],
})
if result.status != 0 {
    return Err(Error.new("protoc failed: {}", result.stderr))
}

// Proposed (concise)
try ctx.exec("protoc", ["--rask_out=.rk-gen/", "api.proto"])
// exec() returns Error if non-zero exit, includes stderr in error message
```

---

## 12. Compilation Pipeline

Expanding the lifecycle from `struct.build/LC1`:

```
rask build
  │
  ├─ 1. Find rask.build (or use defaults)
  ├─ 2. Parse package block (PK3: independent of build logic)
  ├─ 3. Resolve dependencies (MVS)
  ├─ 4. Check rask.lock (error if out of sync)
  ├─ 5. Download missing deps (from cache or registry)
  │
  ├─ 6. Run build steps (if build() exists)
  │     ├─ Check step cache (ST1)
  │     ├─ Skip cached steps
  │     └─ Execute changed steps
  │
  ├─ 7. Compile packages (dependency order, parallel where possible)
  │     ├─ Check compilation cache (CC1-CC3)
  │     ├─ Skip cached packages
  │     └─ Compile changed packages
  │         ├─ Parse → Resolve → Type-check → Ownership-check
  │         ├─ Monomorphize → MIR → Cranelift/LLVM
  │         └─ Emit object file
  │
  ├─ 8. Link
  │     ├─ Object files + runtime library + system libs
  │     └─ Emit binary to build/<profile>/<name>
  │
  └─ 9. Done
       └─ Report: "Built myapp (debug) in build/debug/myapp"
```

### Parallelism

| Rule | Description |
|------|-------------|
| **PP1: Package parallelism** | Independent packages compile in parallel (up to CPU count) |
| **PP2: Pipeline parallelism** | Package B's parsing can start while package A is still in codegen |
| **PP3: Jobs flag** | `--jobs N` or `-j N` controls parallelism. Default: CPU count |

---

## 13. Remaining Issues from struct.packages

These items from the `struct.packages` remaining issues list need specs:

### Private Registry Authentication

| Rule | Description |
|------|-------------|
| **PA1: Token auth** | Bearer token in `Authorization` header |
| **PA2: Config location** | `~/.rask/registries/<host>/token` or `RASK_REGISTRY_TOKEN_<HOST>` |
| **PA3: Per-package registry** | `dep "internal" "^1.0" { registry: "https://pkgs.corp.com" }` |

### Patch Overrides

```rask
package "my-app" "1.0.0" {
    dep "http" "^2.0"

    // Override transitive dep version for debugging/patching
    patch "http-parser" { path: "../http-parser-fork" }
}
```

| Rule | Description |
|------|-------------|
| **PO1: Scope** | Patches apply to the entire dependency graph |
| **PO2: Local only** | Patches cannot be published (like path deps) |
| **PO3: Version constraint** | Patch must satisfy the original version constraint |

---

## Metrics Validation

How this design scores against METRICS.md:

### TC (Transparency Coefficient, target ≥ 0.90)

Build steps declare their inputs explicitly. No hidden magic. The compilation
pipeline is deterministic and inspectable (`--verbose` shows every step). Cross-compilation
target is explicit in the command. Score: **high**.

### ED (Ergonomic Delta, target ≤ 1.2)

| Task | Best-in-class | Rask |
|------|--------------|------|
| Add dependency | `cargo add serde` | `rask add json` |
| Cross-compile | `GOOS=linux go build` | `rask build --target x86_64-linux` |
| Watch + rebuild | `cargo watch` (external) | `rask watch` (built-in) |
| Build script step | Zig `b.addStep()` (8 lines) | `ctx.step(name, inputs, closure)` |
| Publish package | `cargo publish` | `rask publish` |
| Offline build | `cargo vendor` + config | `rask vendor` |

ED for build tasks: ~1.0 vs best-in-class for each. The all-in-one tooling (watch, vendor,
audit built-in) is better than languages that require external tools.

### SN (Syntactic Noise, target ≤ 0.3)

The `rask.build` format is minimal:
```rask
package "my-api" "1.0.0" {
    dep "http" "^2.0"
    dep "json" "^1.5"
}
```

Compare Cargo.toml (4 lines for the same), go.mod (similar), package.json (more verbose).
Rask is competitive. Score: **low noise**.

### CS (Compilation Speed, target ≥ 5× Rust)

The design supports fast builds through:
- Package-level parallelism (PP1)
- Content-addressed compilation cache (CC1–CC3)
- Signature-based invalidation (CC3: internal changes don't cascade)
- Cranelift for dev builds (already decided)
- Watch mode avoids cold-start overhead (WA1)

These are architectural enablers. Actual speed depends on implementation.

### IF (Innovation Factor)

| Feature | Novel? |
|---------|--------|
| Incremental build steps with auto-caching | Yes — nobody does this cleanly |
| Build script security prompts | Yes — first systems language to do this |
| All-in-one tooling | Not novel but still rare for systems languages |
| Cross-compilation without toolchains | Zig does this, but doing it well is still differentiating |
| Content-addressed compilation cache built-in | Go has this; novel for Rust-class languages |

---

## Open Questions

These are decisions I want your input on before formalizing into spec rules:

### Q1: MVS vs Maximal Version Selection

The current `struct.packages` spec uses Go's MVS (minimum version selection) — always
pick the oldest compatible version. This is deterministic and stable, but it means
you never get bug fixes unless you explicitly `rask update`.

Cargo uses maximal selection (newest compatible). More bug fixes by default, but less
stable — a new dependency version could break things.

My take: MVS is the right call for Rask. Determinism matters more than auto-updates.
`rask update` is easy to run when you want newer versions. But this is a genuine tradeoff.

### Q2: Remote compilation cache

RC1–RC3 sketches a remote cache. High value for teams but adds significant
complexity (authentication, cache poisoning, storage costs). Defer to v2?

### Q3: WASM target

`wasm32-none` is listed as Tier 2. WASM has unique constraints (no filesystem, no threads,
different memory model). Should this be a first-class target with its own documentation,
or is Tier 2 + community effort sufficient initially?

### Q4: Multi-binary output

`struct.targets/MB2` specifies `bin: ["cli.rk", "server.rk"]` for multi-binary projects.
How should `rask build` handle this? Build all binaries? Build the first one? Require
`rask build --bin cli`?

Proposal: `rask build` builds all binaries. `rask run` requires specifying which one
if there are multiple: `rask run --bin server -- --port 8080`.

---

## What's NOT in This Proposal

These are explicitly deferred:

- **Macro system** — Language feature, not build system
- **Plugin/extension system** — Build steps cover most use cases
- **Distributed builds** — Remote cache covers the common case
- **Hermetic builds** — Nix-level hermeticity is out of scope
- **Custom build backends** — Cranelift for dev, LLVM for release. No pluggable backends
- **IDE build integration** — The compiler's JSON output mode (CL3) is sufficient for IDE integration
- **Bazel/Buck2 compatibility** — Rask is opinionated. If you need Bazel, use Bazel

---

## Implementation Priority

For the compiler's current state (end-to-end pipeline being wired), the implementation
order should be:

**Needed now (for `rask build` to work):**
1. Output directory structure (OD1–OD6)
2. Basic compilation pipeline (section 12)
3. Binary naming (OD4)

**Needed for usability (before first users):**
4. `rask add` / `rask remove` (AD1–AD4, RM1–RM2)
5. Watch mode (WA1–WA8)
6. Cross-compilation basics (XT1–XT6, Tier 1 targets)

**Needed for ecosystem (before public registry):**
7. Publishing workflow (PB1–PB7)
8. Vendoring (VD1–VD5)
9. Dependency auditing (AU1–AU5)

**Needed for scale (v2):**
10. Incremental build steps (ST1–ST6)
11. Compilation cache (CC1–CC5)
12. Remote cache (RC1–RC3)
13. Build script security (BS1–BS6)
