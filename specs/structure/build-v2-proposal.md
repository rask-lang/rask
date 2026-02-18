<!-- id: struct.build-v2 -->
<!-- status: proposed -->
<!-- summary: Build system redesign — permissions, declarative native compilation, exclusive features, content-addressed caching -->
<!-- depends: structure/build.md, structure/packages.md -->

# Build System v2 — Design Proposal

The current build spec is Cargo with Rask syntax. Same semver, same additive features, same build scripts, same lock file. If I'm building a new language in 2026, I should learn from Cargo's decade of real-world pain instead of copying it.

This proposal covers seven areas where we can do genuinely better. Each is evaluated against METRICS.md. I'm not proposing all seven — some may not survive scrutiny. That's the point.

---

## 1. Dependency Permissions

**Problem:** Build scripts and dependencies run with full OS access. A malicious `build.rk` or transitive dep can exfiltrate data, write to arbitrary paths, or spawn network connections. This is Cargo's #1 supply chain attack vector.

**What Deno does:** Process-level permission flags (`--allow-net`, `--allow-read`). Works, but permissions are per-process, not per-package. A malicious dep inside your process gets whatever you granted.

**What I propose:** Compile-time capability inference from imports, with consumer consent at `rask add` time.

### How it works

Capabilities aren't declared by package authors — they're **inferred by the compiler** from the import graph. Stdlib modules are the capability roots:

| Stdlib module | Capability |
|---------------|------------|
| `io.net` | `net` |
| `io.fs` | `read`, `write` |
| `os.exec` | `exec` |
| `os.env` | `env` |
| `unsafe` blocks | `ffi` |

If a package imports `io.net`, it requires `net`. If it depends on a package that imports `io.net`, it transitively requires `net`. The compiler traces this statically — no runtime checks, no author annotations.

The **consumer** (you) consents when adding the dep:

```
$ rask add http
  Resolving http ^2.1.0...

  Capabilities required:
    net(*)    — expected for an HTTP library

  Allow? [y/N] y
  Added dep "http" "^2.1.0" { allow: [net] }
```

```
$ rask add sketchy-json-parser
  Resolving sketchy-json-parser ^1.2.0...

  Capabilities required:
    net(*)    — src/telemetry.rk imports io.net
    read(./)  — src/cache.rk imports io.fs

  Allow these capabilities? [y/N] n
  Aborted.
```

Packages with `unsafe` / FFI get a louder warning:

```
$ rask add fast-simd
  Resolving fast-simd ^0.3.0...

  ⚠ Uses unsafe code (src/avx2.rk, src/neon.rk)
  Capabilities required:
    ffi    — C interop via unsafe blocks

  This package can execute arbitrary native code.
  Allow? [y/N]
```

### Rules

| Rule | Description |
|------|-------------|
| **PM1: Default deny** | Dependencies get no I/O, no network, no subprocess, no env access unless consented to |
| **PM2: Import-inferred capabilities** | Compiler infers required capabilities from a package's import graph. No author annotations needed |
| **PM3: Consumer consent** | `allow:` block on deps records consent. `rask add` prompts interactively |
| **PM4: Transitive visibility** | Capabilities visible in `rask.lock` — audit what every transitive dep can do |
| **PM5: Build script sandbox** | `func build()` runs sandboxed. Capabilities declared via `scope "build" { allow: [...] }` |
| **PM6: Escape hatch** | `allow: [all]` for trusted deps. Linter warns on `all` |
| **PM7: Update detection** | `rask update` flags capability changes in new versions. A patch release adding `net` is suspicious |
| **PM8: Root unrestricted** | Your own code (root package) is unrestricted. Permissions only gate dependencies |

### Prompt tiers

| Situation | UX |
|-----------|-----|
| Dep needs no capabilities | `Added dep "json" "^1.5"` — no prompt |
| Dep needs standard capabilities (`net`, `read`, `exec`) | List capabilities + `Allow? [y/N]` |
| Dep uses `unsafe` / `ffi` | Warning banner + explicit acknowledgment |
| Update adds new capability | `⚠ http 2.2.0 added new capability: exec("curl")` + `Accept? [y/N]` |

### Capability categories

| Capability | Scope | Inferred from |
|------------|-------|---------------|
| `read` | Filesystem read | `import io.fs` (read operations) |
| `write` | Filesystem write | `import io.fs` (write operations) |
| `net` | Network access | `import io.net` |
| `exec` | Subprocess | `import os.exec` |
| `env` | Env variables | `import os.env` |
| `ffi` | Foreign function interface | `unsafe` blocks, `extern` declarations |

### Enforcement mechanism

**Compile time (primary):** The compiler traces imports. If `dep "foo"` transitively imports `io.net` but `build.rk` doesn't have `allow: [net]` on foo, compilation fails:

```
ERROR [struct.build/PM1]: undeclared capability
  sketchy-json-parser requires net
    └─ src/telemetry.rk:3 imports io.net

FIX: dep "sketchy-json-parser" "^1.0" { allow: [net] }
```

**Build scripts (interpreter):** Already controlled — add permission checks before `ctx.exec()`, `ctx.env()`, etc. in the interpreter.

**The FFI hole:** A package with `allow: [ffi]` can do anything through C calls. That's inherent — you can't sandbox native code without OS-level enforcement. But `ffi` is a loud, visible capability. If a JSON parser needs `ffi`, that's a red flag and the prompt makes sure you see it.

### Metrics evaluation

| Metric | Impact | Score |
|--------|--------|-------|
| **TC (Transparency)** | Capabilities visible in build.rk and rask.lock | +0.05 |
| **MC (Correctness)** | Doesn't prevent bugs, prevents supply chain attacks | neutral |
| **PI (Predictability)** | You know what deps can do before they run | +0.10 |
| **ED (Ergonomic Delta)** | Interactive prompt handles it — no manual annotation burden | neutral |
| **IF (Innovation)** | Compile-time capability inference + consumer consent in a compiled language. Nothing like this exists | HIGH |
| **SN (Syntactic Noise)** | `allow:` only in build.rk, auto-generated by `rask add` | neutral |

**Verdict:** High value, moderate effort. The compiler already resolves imports — tracing which stdlib modules are reachable is straightforward. The `rask add` prompt and `rask update` diff are the killer UX features. The `rask.lock` audit trail answers "which of my 47 transitive deps can access the network?" — a question no compiled language can answer today.

**Risk:** False positives. If a package imports `io.fs` but only uses it in a test, it still gets flagged as needing `read`/`write`. Mitigation: scope analysis could distinguish test-only imports, but that's a refinement, not a blocker. Start coarse, refine later.

**Why this is better than Deno:** Deno checks at runtime — you find out in production. This checks at compile time and at `rask add` time — you find out before the code ever runs.

---

## ~~2. Declarative Native Compilation~~ — Cut

More surface area to learn for marginal benefit. `func build(ctx)` with `ctx.step()` already gives structured, cacheable, parallelizable build steps. Adding a declarative DSL (`native {}`, `codegen {}`) duplicates that functionality with less flexibility. If the step DAG needs to be more static, improve `ctx.step()` — don't invent a second language.

---

## 2. Exclusive Feature Groups

**Problem:** Cargo features are additive-only. You can't express "pick exactly one of these backends." This causes real production bugs — sqlx with both tokio and async-std enabled, openssl with conflicting vendoring options in workspaces.

The root cause: Cargo uses set-union at resolution time with no way to express negation, exclusion, or conditionality. This works for 80% of features (enabling optional integrations) but fails when features represent choices between implementations.

**What I propose:** One keyword, two modes. `feature` stays for additive flags. `feature ... exclusive` adds mutually exclusive groups.

```rask
package "my-db" "1.0.0" {
    // Additive: can enable any combination
    feature "logging"
    feature "metrics"

    // Exclusive: must pick exactly one
    feature "runtime" exclusive {
        option "tokio" { dep "tokio" "^1.0" }
        option "async-std" { dep "async-std" "^1.0" }
        default: "tokio"
    }

    feature "tls" exclusive {
        option "openssl" { dep "openssl" "^3.0" }
        option "rustls" { dep "rustls" "^0.23" }
        option "none"
        default: "rustls"
    }
}
```

### Rules

| Rule | Description |
|------|-------------|
| **FG1: Additive features** | `feature "name"` — standard additive flag, set-union resolution |
| **FG2: Exclusive groups** | `feature "name" exclusive { ... }` — exactly one option active. Compile error if multiple selected |
| **FG3: Default required** | Every exclusive feature must have a `default:`. No "no choice" state |
| **FG4: Transitive exclusion** | If dep A selects `runtime = "tokio"` and dep B selects `runtime = "async-std"`, resolution fails with a clear error explaining the conflict and both sources |
| **FG5: Override** | Root package can override any dep's exclusive selection: `dep "my-db" "^1.0" { runtime: "async-std" }` |
| **FG6: Code access** | `comptime if cfg.runtime == "tokio"` works for exclusive groups |

### Resolution rules

Additive features: set union (same as Cargo). Feature enabled anywhere → enabled everywhere.

Exclusive groups: all selectors must agree. If they don't, the resolver reports a conflict with full dependency path showing which dep selected which option. Root package override always wins.

```
ERROR [struct.build/FG4]: exclusive feature conflict
  "runtime" for my-db:
    dep-a selects "tokio"   (via dep "my-db" { runtime: "tokio" })
    dep-b selects "async-std" (via dep "my-db" { runtime: "async-std" })

FIX: Override in root build.rk:
  dep "my-db" "^1.0" { runtime: "tokio" }
```

### Metrics evaluation

| Metric | Impact | Score |
|--------|--------|-------|
| **MC (Correctness)** | Prevents impossible feature combinations at compile time | +0.05 |
| **PI (Predictability)** | Clear error instead of silent runtime breakage | +0.10 |
| **ED (Ergonomic Delta)** | Reuses `feature` keyword — one concept, two modes | neutral |
| **IF (Innovation)** | Cargo has had an open issue for this since 2016 (#2980). Actually solving it is meaningful | HIGH |

**Verdict:** High value, low cost. Reuses the `feature` keyword with an `exclusive` modifier. No new concepts to learn — just "some features are pick-one." The resolution rules are clear and the error messages write themselves.

**Risk:** Diamond resolution. Dep A wants `runtime = "tokio"`, dep B wants `runtime = "async-std"`. Resolution fails — the error message needs to show the full path and suggest the root override. The UX of the error is what makes or breaks this.

---

## 3. Content-Addressed Remote Cache

**Problem:** Every developer and CI machine rebuilds from scratch. Compilation is the #1 time sink in development workflows.

**What Bazel does:** Every build action is content-hashed. A remote cache stores results by hash. If the cache has a hit, no local work needed. But Bazel's setup cost is enormous.

**What Turborepo does:** Package-task-level caching. Much simpler, but coarser granularity.

**What I propose:** Package-level content-addressed caching with optional remote storage. Simpler than Bazel, finer than Turborepo.

```
RASK_CACHE_URL=https://cache.myteam.com rask build
  → hash(source files + deps + profile + target + compiler version) = abc123
  → GET /abc123.o → 200 OK → skip compilation
  → or: compile locally, PUT /abc123.o → cache for next build
```

### Rules

| Rule | Description |
|------|-------------|
| **RC1: Content key** | Cache key = hash of (source content + dep signatures + profile + target + compiler version) |
| **RC2: Local cache** | `build/.cache/` — same as current XC1-XC5 |
| **RC3: Remote cache** | `RASK_CACHE_URL` or `cache_url` in workspace config. HTTP GET/PUT protocol |
| **RC4: Read-only mode** | `RASK_CACHE_MODE=readonly` — CI machines push, dev machines pull |
| **RC5: No auth by default** | Anonymous read, bearer token for write. Keep it simple |
| **RC6: Cache protocol** | GET `/<key>` returns artifact. PUT `/<key>` stores it. HEAD `/<key>` checks existence. That's it |

### Protocol

Intentionally minimal — a static file server works. S3, R2, a reverse proxy to disk. No custom server needed.

```
GET  /v1/artifacts/<hash>         → 200 + artifact | 404
PUT  /v1/artifacts/<hash>         → 201 | 409 (exists)
HEAD /v1/artifacts/<hash>         → 200 | 404
```

### Metrics evaluation

| Metric | Impact | Score |
|--------|--------|-------|
| **CS (Compilation Speed)** | Team of 10 shares cache → ~5x fewer compilations | +very significant |
| **ED (Ergonomic Delta)** | Zero config for local. One env var for remote | neutral |
| **PI (Predictability)** | Cache hits are deterministic (content-addressed) | +0.05 |
| **IF (Innovation)** | Bazel does this but needs an army. Built-in and simple is new for a compiled language | MEDIUM |

**Verdict:** High practical value, moderate implementation effort. The local cache already exists (Phase 4). Remote is just HTTP GET/PUT on the same keys.

**Risk:** Cache poisoning. If someone pushes a bad artifact, everyone gets it. Mitigations: signed artifacts, build reproducibility checks, read-only mode for untrusted sources.

**Counter-argument:** Is this really innovation? sccache exists for Rust/C. The difference is being built-in and zero-config instead of a separate tool.

---

## 4. Hermetic Builds

**Problem:** "Works on my machine." Different system libraries, different tool versions, different env vars → different build results. Nix solves this completely but at enormous complexity cost.

**What I propose:** Partial hermeticity. Not Nix-level purity, but enough to catch the common problems.

### Rules

| Rule | Description |
|------|-------------|
| **HB1: Tool pinning** | `codegen {}` blocks pin tool versions. If `protoc --version` changes, the step re-runs |
| **HB2: Env isolation** | Build scripts see only declared env variables + a minimal set (HOME, PATH, TMPDIR) |
| **HB3: Reproducibility report** | `rask build --check-repro` builds twice and diffs outputs. Reports non-deterministic steps |
| **HB4: Lock includes tool versions** | `rask.lock` records tool versions used during resolution. `rask build` warns if they differ |

### What I'm NOT proposing

Full Nix-style sandboxing (Linux namespaces, no network, pure store paths). The complexity cost is too high, and it doesn't work well on macOS/Windows. Partial hermeticity — pinning tool versions, isolating env vars, reproducibility checks — catches 80% of "works on my machine" for 20% of the effort.

### Metrics evaluation

| Metric | Impact | Score |
|--------|--------|-------|
| **PI (Predictability)** | Builds are more reproducible across machines | +0.10 |
| **TC (Transparency)** | Tool versions visible in lock file | +0.02 |
| **ED (Ergonomic Delta)** | Env isolation may break some build scripts | -0.05 |
| **IF (Innovation)** | Nix did it fully. Partial hermeticity is practical, not novel | LOW |

**Verdict:** Medium value. HB1 and HB2 are cheap. HB3 is a nice diagnostic. HB4 is informational. None of this is groundbreaking, but it's table stakes for 2026.

**Risk:** Env isolation (HB2) will break real build scripts that read undeclared env vars. The error message needs to be crystal clear about which variable was denied and how to declare it.

---

## 5. Smart Lock File

**Problem:** Cargo.lock is platform-specific. Build on Linux, commit the lock file, CI builds on macOS — different results. uv solved this with universal lockfiles that encode platform forks instead of flattening to one platform's resolution.

The current Rask implementation uses `DefaultHasher` (non-cryptographic, output changes between Rust versions). That's a time bomb.

**What uv does:** The lock file contains *resolution markers* — the platform/version conditions under which each dependency fork was taken. One lock file, all platforms. The resolver runs once and encodes all conditional branches.

**What I propose:** Same idea, kept simple. Platform-conditional deps get `when:` annotations. Capabilities (from proposal 1) and exclusive feature selections (from proposal 2) are recorded per-package.

```
# rask.lock — auto-generated, do not edit
# rask 0.1.0

[[package]]
name = "http"
version = "2.1.0"
source = "registry"
checksum = "sha256:a1b2c3d4..."
capabilities = [net]
features = [ssl]
runtime = "tokio"

[[package]]
name = "http"
version = "2.1.0"
deps = ["json ^1.5", "tcp ^0.9"]

    [[package.when]]
    target = "linux"
    deps = ["epoll ^1.0"]

    [[package.when]]
    target = "macos"
    deps = ["kqueue ^1.0"]

[[package]]
name = "json"
version = "1.5.2"
source = "registry"
checksum = "sha256:e5f6a7b8..."
capabilities = []
```

### Rules

| Rule | Description |
|------|-------------|
| **LF1: Universal** | One lock file works across all targets. Platform-conditional deps encoded with `when:` blocks, not flattened |
| **LF2: Feature + exclusive resolution** | Records which additive features are enabled and which exclusive option was selected |
| **LF3: Capabilities** | Records inferred capabilities per package — the audit trail for dependency permissions |
| **LF4: SHA-256** | All checksums use SHA-256 |
| **LF5: Human-scannable** | Simple line-based format. No nested TOML tables. `rask.lock` should be reviewable in a PR diff |

### What I'm NOT including

- **Reason tracking** ("why was this version chosen") — useful for debugging but adds noise to every entry. Better as `rask why <package>` command output than lock file bloat.
- **Tool versions** — belongs in build cache metadata, not the lock file. The lock file pins *package* versions, not *tool* versions.

### Metrics evaluation

| Metric | Impact | Score |
|--------|--------|-------|
| **PI (Predictability)** | Lock file works across platforms, CI matches local | +0.10 |
| **TC (Transparency)** | Capabilities and feature resolution visible in PR diffs | +0.05 |
| **ED (Ergonomic Delta)** | No user-facing ceremony change | neutral |
| **IF (Innovation)** | uv pioneered universal lockfiles. Capability recording is new | MEDIUM |

**Verdict:** High value, low cost. LF1 (universal) prevents "works on my machine." LF3 (capabilities) makes the permission model auditable in version control. LF4 (SHA-256) is a mandatory fix.

**Immediate action:** Switch from `DefaultHasher` to SHA-256 in the current implementation before anyone depends on the hash format.

---

## 6. Workspace-First Design

**Problem:** Cargo workspaces feel bolted on. Feature unification across workspace members causes real issues. Monorepo workflows (shared deps, coordinated releases) are clunky.

**What I propose:** Workspaces as the default project structure, not an add-on.

```rask
// workspace root build.rk
package "my-project" "1.0.0" {
    members: ["app", "lib-core", "lib-http"]

    // Shared deps — all members can use these
    dep "json" "^1.5"

    // Per-member overrides
    member "app" {
        dep "cli" "^3.0"
    }
}
```

### Rules

| Rule | Description |
|------|-------------|
| **WK1: Single source** | One `build.rk` at workspace root. Members don't need their own |
| **WK2: Shared lock** | Single `rask.lock` at root |
| **WK3: Independent features** | Feature unification is per-member, not per-workspace. Member A's features don't infect member B |
| **WK4: Targeted builds** | `rask build app` builds one member. `rask build` builds all |
| **WK5: Cross-member deps** | Members reference each other by name, not path |

### WK3 is the important one

Cargo's feature unification across workspace members is a known footgun. If `crate-server` needs `openssl/vendored` and `crate-wasm` can't use it, you can't build both in one workspace.

I propose per-member resolution: each member gets its own resolved feature set. Shared deps are resolved once with the intersection of compatible features. If members need incompatible features, they get separate resolution (at the cost of longer compile times from duplication).

### Metrics evaluation

| Metric | Impact | Score |
|--------|--------|-------|
| **UCC (Use Case Coverage)** | Better monorepo support for web services (30%) and CLI tools (20%) | +0.05 |
| **ED (Ergonomic Delta)** | Less boilerplate for multi-package projects | +0.05 |
| **CS (Compilation Speed)** | Per-member resolution may slow builds (duplication) | -small |
| **IF (Innovation)** | Nx/Turborepo have good monorepo stories. This is catching up, not innovating | LOW |

**Verdict:** Medium value. WK3 (independent features) is the important fix. The rest is polish.

**Risk:** Per-member feature resolution means the same dep might be compiled twice with different features. This hurts compilation speed. The tradeoff is: correct builds that are slower vs. fast builds that are broken.

---

## Decision

Three proposals accepted:

| # | Proposal | What changes |
|---|----------|-------------|
| 1 | **Dependency permissions** | PM1-PM8 rules. Compile-time capability inference from imports. `rask add` prompts. Capabilities in `rask.lock` |
| 2 | **Exclusive feature groups** | FG1-FG6 rules. `feature "name" exclusive { ... }` syntax in `build.md` |
| 5 | **Smart lock file** | LF1-LF5 rules. Universal format, SHA-256, capabilities + feature resolution recorded |

### Cut

- **Declarative native compilation** — More surface area than value. `func build(ctx)` with `ctx.step()` is sufficient.
- **Hermetic builds** — Tool pinning is nice but not differentiating. Nix-level sandboxing is too costly.
- **Workspace-first** — WK3 (independent features) ships as part of exclusive feature groups. The rest is polish for later.
- **Remote cache** — High practical value but not a design differentiator. Can be added later as implementation work without spec changes.

### What this means for the spec

`build.md` needs: `feature ... exclusive` section, `allow:` block on deps, PM1-PM8 rules.

`packages.md` needs: LF1-LF5 rules replacing current LK1-LK4, universal lock file format.

The `feature` keyword gains an `exclusive` modifier. No new keywords needed.

---

## Appendix (non-normative)

### What makes this genuinely different from Cargo

1. **Permission model** — No compiled language has compile-time capability inference with consumer consent and lockfile audit trail
2. **Exclusive features** — Solves a 10-year-old Cargo issue (#2980) that causes real production bugs
3. **Universal lockfile** — One lockfile across all platforms, with capabilities and feature resolution visible in PR diffs

Three concrete improvements over the state of the art, not syntax reshuffling.

### What this doesn't solve

- Registry infrastructure (hosting, authentication, yanking)
- Build script testing and debugging
- Incremental compilation (semantic hashing)
- IDE integration with build system

These are real gaps but they're orthogonal to this proposal.
