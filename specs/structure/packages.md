# Package Versioning and Dependencies

## The Question
How are external dependencies managed? What versioning scheme is used? How is dependency resolution performed? How are reproducible builds guaranteed?

## Decision
Semantic versioning with minimal version selection (MVS), optional TOML manifest for dependencies, generated lock file for reproducibility, local cache for downloaded packages, zero-config for standalone packages.

## Rationale
Semantic versioning is well-understood and widely adopted. MVS (like Go modules) is simpler than SAT-solving (like npm/Cargo) and produces predictable, deterministic results without exponential search spaces. Optional manifest keeps simple packages simple—no `rask.toml` needed unless you depend on external code. Lock files ensure reproducible builds across machines and time. Local cache eliminates redundant downloads while keeping packages immutable.

## Specification

### Versioning Scheme

**Semantic Versioning (semver):**
- Format: `MAJOR.MINOR.PATCH` (e.g., `1.4.2`)
- Version components MUST be non-negative integers
- MAJOR: breaking changes (incompatible API)
- MINOR: new features (backward-compatible)
- PATCH: bug fixes (backward-compatible)

**Pre-release versions:**
- Format: `MAJOR.MINOR.PATCH-LABEL.N` (e.g., `1.0.0-beta.3`, `2.0.0-rc.1`)
- Labels: `alpha`, `beta`, `rc` (release candidate)
- N: sequential number starting from 1
- Pre-release versions are NOT considered stable
- Pre-release `1.0.0-beta.1` < `1.0.0`

**Version ordering:**
```rask
0.1.0 < 0.1.1 < 0.2.0 < 1.0.0-alpha.1 < 1.0.0-beta.1 < 1.0.0-rc.1 < 1.0.0 < 1.0.1 < 1.1.0 < 2.0.0
```

**Special semantics for 0.x versions:**
- `0.x.y` versions are considered unstable
- MINOR bump in `0.x` MAY be breaking (treat as MAJOR)
- Dependency resolution treats `0.x` versions conservatively

### Package Manifest (`rask.toml`)

**Location:** Root of package directory, alongside `.rask` source files.

**Required only if:**
- Package depends on external packages
- Package needs to specify metadata for publication
- Package has build configuration (C libraries, etc.)

**Minimal example:**
```toml
[package]
name = "myapp"
version = "1.0.0"

[dependencies]
http = "2.1.0"
json = "1.3"
```

**Full example:**
```toml
[package]
name = "mylib"
version = "1.4.2"
authors = ["Alice <alice@example.com>"]
license = "MIT"
repository = "https://github.com/alice/mylib"
description = "A helpful library"

[dependencies]
http = "2.1.0"              # Exact MINOR version: ≥2.1.0, <2.2.0
json = "1"                  # Exact MAJOR version: ≥1.0.0, <2.0.0
crypto = { version = "3.2", source = "https://crypto.example.com/crypto.git" }

[dev-dependencies]
testing = "1.0"             # Only for tests, not transitive

[build]
c_include_paths = ["/usr/include/custom"]
c_link_libs = ["ssl", "crypto"]
c_flags = ["-O3"]
```rask

**Field specifications:**

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `package.name` | Yes (if manifest exists) | String | Package identifier (lowercase, hyphens allowed) |
| `package.version` | Yes (if manifest exists) | String | Semver version |
| `package.authors` | No | Array[String] | Author names and emails |
| `package.license` | No | String | SPDX license identifier |
| `package.repository` | No | String | Git repository URL |
| `package.description` | No | String | One-line description |
| `dependencies.<name>` | No | String or Table | Version constraint or detailed spec |
| `dev-dependencies.<name>` | No | String or Table | Test/development-only dependencies |
| `build.c_include_paths` | No | Array[String] | C header search paths |
| `build.c_link_libs` | No | Array[String] | C libraries to link |
| `build.c_flags` | No | Array[String] | Additional C compiler flags |

**Version constraint syntax:**

| Constraint | Meaning | Example |
|------------|---------|---------|
| `"1.2.3"` | Exact MINOR: `≥1.2.3, <1.3.0` | `http = "2.1.5"` → allows `2.1.5`-`2.1.999` |
| `"1.2"` | Exact MINOR: `≥1.2.0, <1.3.0` | `json = "1.2"` → allows `1.2.0`-`1.2.999` |
| `"1"` | Exact MAJOR: `≥1.0.0, <2.0.0` | `crypto = "3"` → allows `3.0.0`-`3.999.999` |
| `"^1.2.3"` | Compatible: `≥1.2.3, <2.0.0` | Caret allows MINOR+PATCH bumps |
| `"~1.2.3"` | Tilde: `≥1.2.3, <1.3.0` | Tilde allows PATCH bumps only |
| `"1.2.3-beta.1"` | Pre-release: exact version | Pre-release MUST match exactly |

**Default behavior:**
- `"1.2.3"` (no prefix) → `^1.2.3` (allows compatible updates)
- For `0.x` versions: `"0.3.1"` → `~0.3.1` (only patch updates, MINOR may break)

**Advanced dependency specifications:**

```toml
[dependencies]
# Git source
http = { version = "2.1", source = "https://github.com/author/http.git" }

# Git with branch/tag/commit
parser = { version = "1.0", source = "https://github.com/author/parser.git", branch = "main" }
lexer = { version = "0.5", source = "https://github.com/author/lexer.git", tag = "v0.5.3" }
utils = { version = "1.2", source = "https://github.com/author/utils.git", commit = "abc123def" }

# Path dependency (for local development)
mylib = { path = "../mylib" }
```

**Path dependencies:**
- MUST NOT be published (registry rejects packages with path deps)
- Used for local development and monorepos
- Version ignored for path dependencies (always uses source from path)

### Lock File (`rask.lock`)

**Purpose:** Guarantee reproducible builds by recording exact versions of all transitive dependencies.

**Location:** Root of package directory, alongside `rask.toml`.

**Generated by:** `rask build` or `rask fetch` (auto-generated, DO NOT hand-edit).

**Format (TOML):**
```toml
# This file is auto-generated by rask. Do not edit manually.

[[package]]
name = "http"
version = "2.1.5"
source = "https://packages.rask-lang.org/http"
checksum = "sha256:abc123def456..."

[[package]]
name = "json"
version = "1.3.2"
source = "https://packages.rask-lang.org/json"
checksum = "sha256:789xyz123abc..."
dependencies = ["string-utils"]

[[package]]
name = "string-utils"
version = "0.5.1"
source = "https://packages.rask-lang.org/string-utils"
checksum = "sha256:def456abc789..."
```

**Fields:**

| Field | Description |
|-------|-------------|
| `name` | Package name |
| `version` | Exact resolved version |
| `source` | URL where package was fetched |
| `checksum` | SHA-256 hash of package contents |
| `dependencies` | List of direct dependencies (names only) |

**Lock file semantics:**

| Scenario | Behavior |
|----------|----------|
| `rask.lock` exists | Use exact versions from lock file |
| `rask.lock` missing | Resolve dependencies, generate lock file |
| Dependency version mismatch | Error: lock file out of date, run `rask update` |
| Lock file in version control | RECOMMENDED (ensures reproducibility) |
| Library vs application | Applications SHOULD commit lock; libraries MAY omit |

**Updating dependencies:**

| Command | Effect |
|---------|--------|
| `rask build` | Use lock file if exists, generate if missing |
| `rask fetch` | Download dependencies, update lock file |
| `rask update` | Resolve latest compatible versions, update lock |
| `rask update <pkg>` | Update specific package to latest compatible |

### Dependency Resolution (MVS Algorithm)

**Minimal Version Selection (MVS):**
- Select the **minimum** version that satisfies all constraints
- Predictable: same inputs → same outputs (no backtracking)
- Fast: O(dependencies) time, no exponential search
- Upgrade-stable: adding a dependency cannot downgrade existing deps

**Algorithm:**

1. **Build dependency graph:**
   - Start with root package's direct dependencies
   - For each dependency, fetch its `rask.toml` and read its dependencies
   - Recursively build full transitive closure

2. **Select minimum satisfying version:**
   - For each package name, collect all version constraints
   - Find minimum version that satisfies ALL constraints
   - If no such version exists → dependency conflict error

3. **Verify constraints:**
   - Check selected versions against all constraints
   - Detect cycles (error if cycle found)

4. **Generate lock file:**
   - Record exact selected versions
   - Compute checksums for each package
   - Write to `rask.lock`

**Example:**

```rask
Root depends on: http ^2.1.0, json ^1.3.0
http 2.1.5 depends on: string-utils ^0.5.0
json 1.3.2 depends on: string-utils ^0.5.0

Resolution:
- http: minimum of {≥2.1.0, <3.0.0} → select 2.1.5 (latest in registry)
- json: minimum of {≥1.3.0, <2.0.0} → select 1.3.2
- string-utils: minimum of {≥0.5.0 (from http), ≥0.5.0 (from json)} → select 0.5.1

Result: http@2.1.5, json@1.3.2, string-utils@0.5.1
```

**Conflict resolution:**

| Case | Handling |
|------|----------|
| Compatible constraints | Select minimum satisfying version |
| Incompatible constraints | Error: "Cannot resolve: pkg A requires foo ^1.0, pkg B requires foo ^2.0" |
| Diamond dependency (same package, different versions) | Select minimum that satisfies all |
| Circular dependency | Error: "Circular dependency detected: A → B → C → A" |

**0.x version handling:**

For `0.x` versions, MINOR bumps MAY break compatibility:
- `0.3.5` and `0.4.0` are treated as incompatible MAJOR versions
- Constraint `^0.3.1` → `≥0.3.1, <0.4.0` (not `<1.0.0`)
- Once version reaches `1.0.0`, normal semver rules apply

### Package Registry

**Default registry:** `https://packages.rask-lang.org` (official Rask package index).

**Registry protocol:**

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/pkg/<name>` | GET | Get package metadata (all versions) |
| `/pkg/<name>/<version>` | GET | Download specific version (tarball) |
| `/pkg/<name>/versions` | GET | List all available versions |
| `/publish` | POST | Publish new package version |

**Metadata format (JSON):**
```json
{
  "name": "http",
  "versions": [
    {
      "version": "2.1.5",
      "checksum": "sha256:abc123...",
      "dependencies": {
        "string-utils": "^0.5.0"
      },
      "published_at": "2026-01-15T10:30:00Z"
    },
    {
      "version": "2.1.4",
      "checksum": "sha256:def456...",
      "dependencies": {
        "string-utils": "^0.5.0"
      },
      "published_at": "2026-01-10T14:20:00Z"
    }
  ]
}
```

**Package format (tarball):**
- `<name>-<version>.tar.gz`
- Contains package directory with all `.rask` files
- Includes `rask.toml` manifest
- Excludes: tests, examples, `.git`, build artifacts

**Publishing:**

| Rule | Enforcement |
|------|-------------|
| Version immutability | Once published, version CANNOT be changed or deleted |
| Semver compliance | Version MUST follow semver format |
| No path dependencies | Packages with `path = "..."` dependencies CANNOT be published |
| Checksum verification | Registry computes and stores SHA-256 hash |
| Name uniqueness | First publisher owns the name (no takeover) |

**Alternative registries:**

```toml
# In rask.toml
[registry]
default = "https://my-registry.example.com"

[dependencies]
# Per-package registry override
private-lib = { version = "1.0", registry = "https://company.internal/registry" }
```

### Dependency Cache

**Location:**
- Linux/macOS: `~/.rask/cache/deps/`
- Windows: `%USERPROFILE%\.rask\cache\deps\`
- Override: `RASK_CACHE` environment variable

**Structure:**
```rask
~/.rask/cache/deps/
├── http-2.1.5/
│   ├── rask.toml
│   ├── request.rask
│   ├── response.rask
│   └── ...
├── json-1.3.2/
│   ├── rask.toml
│   ├── parser.rask
│   └── ...
└── checksums.db  # SQLite DB mapping name+version → checksum
```

**Cache behavior:**

| Operation | Cache behavior |
|-----------|----------------|
| Fetch dependency | Check cache first; download if missing |
| Checksum mismatch | Error: "Checksum mismatch for pkg@version, cache corrupted" |
| Cache miss | Download from registry, verify checksum, store in cache |
| Cache invalidation | Manual: `rask cache clean` |
| Concurrent builds | Safe: cache is read-only after population |

**Cache integrity:**
- Each package stored with `<name>-<version>/` directory structure
- Checksums verified on every cache read
- Corrupted cache entries automatically re-downloaded

### Build Integration

**Compilation order:**
1. Resolve dependencies (use lock file if exists)
2. Fetch missing packages into cache
3. Build dependency graph (topological sort)
4. Compile packages in dependency order (independent packages in parallel)
5. Link application

**Import resolution with dependencies:**

```rask
// In source code
import http

// Compiler resolution:
1. Check if "http" is local package (in workspace)
2. If not, check dependencies in rask.toml
3. Look up "http" in cache at ~/.rask/cache/deps/http-<resolved-version>/
4. Import http package from cache
```

**Workspace support (monorepos):**

```rask
workspace/
├── rask.toml           # Workspace manifest
├── app/
│   ├── rask.toml       # App package
│   └── main.rask
├── lib1/
│   ├── rask.toml       # Library package
│   └── lib.rask
└── lib2/
    ├── rask.toml       # Library package
    └── lib.rask
```

**Workspace manifest:**
```toml
[workspace]
members = ["app", "lib1", "lib2"]

[workspace.dependencies]
# Shared dependency versions across workspace
http = "2.1.0"
json = "1.3"
```

**Member package:**
```toml
[package]
name = "app"
version = "1.0.0"

[dependencies]
lib1 = { path = "../lib1" }
http = { workspace = true }  # Use version from workspace manifest
```

**Workspace benefits:**
- Shared dependency resolution (single `rask.lock` at workspace root)
- Path dependencies within workspace (no need for publishing)
- Consistent versions across all packages

### Versioning Best Practices

**For library authors:**

| Rule | Rationale |
|------|-----------|
| Start at `0.1.0` | Signals unstable/experimental |
| Bump to `1.0.0` when API is stable | Commits to semver guarantees |
| MAJOR bump for breaking changes | Allows users to stay on compatible versions |
| MINOR bump for new features | Backward-compatible additions |
| PATCH bump for bug fixes | No API changes |

**For application authors:**

| Recommendation | Rationale |
|----------------|-----------|
| Commit `rask.lock` | Ensures reproducible builds |
| Use `^` constraints | Allow compatible updates |
| Review dependency updates | Run tests before accepting updates |
| Pin critical dependencies | Use exact versions (`=1.2.3`) for security-critical libs |

**Deprecation strategy:**

```rask
// In library code
@deprecated(since = "2.1.0", note = "Use new_function instead")
public func old_function() { ... }
```

Compiler emits warning when deprecated items are used. MAJOR version bump can remove deprecated items.

### Edge Cases

| Case | Handling |
|------|----------|
| Missing `rask.toml` | Package has no external dependencies; version = "0.0.0" |
| Lock file out of date | Error: "rask.lock is out of sync with rask.toml, run `rask update`" |
| Network unavailable | Error: "Cannot fetch pkg@version, check network or use cache" |
| Registry returns 404 | Error: "Package pkg@version not found in registry" |
| Checksum mismatch | Error: "Checksum mismatch for pkg@version, possible tampering" |
| Circular dependency | Error: "Circular dependency: A → B → C → A" |
| Version conflict | Error: "Cannot resolve: X requires Y ^1.0, Z requires Y ^2.0" |
| Pre-release in lock file | Lock file stores exact pre-release version |
| Pre-release in constraint | Error: "Pre-release versions MUST be exact: use `=1.0.0-beta.1`" |
| Path dependency in publish | Error: "Cannot publish with path dependencies" |
| Workspace member version conflict | Error: "Workspace member X@1.0 conflicts with dependency X@2.0" |
| Git dependency not found | Error: "Git repository not found: <url>" |
| Git dependency no tags | Use commit hash as version identifier |
| 0.x MAJOR bump | Treat as breaking; `0.3` and `0.4` are incompatible |
| Dev-dependency conflict | Dev-dependencies do NOT affect transitive resolution |
| Multiple registries | Each package resolved from its specified registry |

## Examples

### Simple Application

**Directory structure:**
```rask
myapp/
├── rask.toml
├── main.rask
└── util.rask
```

**rask.toml:**
```toml
[package]
name = "myapp"
version = "1.0.0"

[dependencies]
http = "2.1"
json = "1.3"
```

**main.rask:**
```rask
import http
import json

@entry
func main() {
    const req = http.get("https://api.example.com/data")
    const data = json.parse(req.body)
    print(data)
}
```

**Build process:**
```bash
$ rask build
Resolving dependencies...
  Fetching http@2.1.5
  Fetching json@1.3.2
  Fetching string-utils@0.5.1 (dependency of http, json)
Generating rask.lock
Compiling string-utils@0.5.1
Compiling http@2.1.5
Compiling json@1.3.2
Compiling myapp@1.0.0
  Linking myapp
Build complete: ./myapp
```

### Library with Dev Dependencies

**rask.toml:**
```toml
[package]
name = "mylib"
version = "2.3.1"
license = "MIT"

[dependencies]
string-utils = "0.5"

[dev-dependencies]
testing = "1.0"  # Only used in tests, not transitive
```

**lib.rask:**
```rask
import string_utils

public func process(s: string) -> string {
    string_utils.normalize(s)
}
```

**lib_test.rask:**
```rask
import testing
import mylib  // Import own package for testing

test "process normalizes strings" {
    testing.assert_eq(mylib.process("  hello  "), "hello")
}
```

**Publishing:**
```bash
$ rask publish
Publishing mylib@2.3.1 to https://packages.rask-lang.org
  Verifying dependencies...
  Packaging tarball...
  Uploading (1.2 MB)...
  Success! Published mylib@2.3.1
```

### Monorepo Workspace

**workspace/rask.toml:**
```toml
[workspace]
members = ["server", "client", "shared"]

[workspace.dependencies]
http = "2.1"
json = "1.3"
```

**workspace/server/rask.toml:**
```toml
[package]
name = "server"
version = "1.0.0"

[dependencies]
shared = { path = "../shared" }
http = { workspace = true }
```

**workspace/client/rask.toml:**
```toml
[package]
name = "client"
version = "1.0.0"

[dependencies]
shared = { path = "../shared" }
http = { workspace = true }
```

**workspace/shared/rask.toml:**
```toml
[package]
name = "shared"
version = "0.1.0"

[dependencies]
json = { workspace = true }
```

**Build:**
```bash
$ cd workspace
$ rask build
Resolving workspace dependencies...
  Fetching http@2.1.5
  Fetching json@1.3.2
Building workspace members...
  Compiling shared@0.1.0
  Compiling server@1.0.0
  Compiling client@1.0.0
```

### Custom Registry

**rask.toml:**
```toml
[package]
name = "corporate-app"
version = "1.0.0"

[registry]
default = "https://registry.company.internal"

[dependencies]
internal-auth = "3.2"  # From corporate registry
http = { version = "2.1", registry = "https://packages.rask-lang.org" }  # Override: public registry
```

## Integration Notes

- **Module System**: Dependencies are imported like local packages—`import http` works identically whether `http` is local or external
- **Compilation Model**: Packages compiled in topological order; independent packages compile in parallel (CS ≥ 5× Rust goal)
- **Type System**: Type identity preserved across dependency boundaries (same as local packages)
- **Error Handling**: Dependency resolution errors reported immediately (fail-fast); checksum errors are fatal
- **C Interop**: `build.c_link_libs` in manifest passed to linker; C dependencies NOT managed by Rask (use system package manager)
- **Tooling**: IDEs fetch package metadata on save; auto-import suggests packages from registry; `rask.lock` changes trigger rebuild

## Remaining Issues

### High Priority
None identified.

### Medium Priority
1. **Private registry authentication** — How to handle auth tokens for private registries? Environment variables? Config file?
2. **Vendoring** — Mechanism to bundle dependencies in source tree for offline builds or environments without registry access
3. **Yanking** — Can published versions be "yanked" (hidden from new resolution but still available for existing lock files)?
4. **Feature flags** — Conditional compilation based on feature flags (like Cargo's features). Needed for optional dependencies.

### Low Priority
5. **Mirror registries** — Fallback to mirrors if primary registry unavailable
6. **Build scripts** — Pre-build/post-build hooks for complex C interop scenarios
7. **Patch dependencies** — Override specific dependency versions (for security patches before upstream fixes)
