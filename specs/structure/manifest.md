<!-- id: struct.manifest -->
<!-- status: decided -->
<!-- summary: rask.toml manifest, checked against a versioned schema with an append-only change history -->
<!-- depends: structure/build.md, structure/packages.md -->

# Manifest

A package's metadata and dependencies live in `rask.toml`: plain TOML, checked against a versioned schema. The schema carries its own change history, so a file written against any past version still loads.

I picked TOML over a Rask block because every language already parses it. A registry indexer, Renovate or an editor plugin reads the manifest without knowing Rask, and `rask add` edits it with an ordinary comment-preserving TOML editor. What TOML lacks, the schema puts back: unknown keys are errors, strings are checked by field type, and the history keeps old files loading.

The registry is why the history matters. It holds manifests written against every version of the schema there has ever been, and a resolver today still has to read all of them.

```toml
schema = "rask-package@1"

[package]
name = "my-api"
version = "1.0.0"
license = "MIT"

[dependencies]
http   = { registry = "^2.0", allow = ["net"] }
shared = { path = "../shared" }
tls    = { git = "https://github.com/x/tls", rev = "a1b2c3" }

[dev-dependencies]
mock-server = { registry = "^2.0" }
```

## The File

| Rule | Description |
|------|-------------|
| **MF1: Schema line** | The file starts with `schema = "<name>@<version>"`. The package manifest is `rask-package`. Missing, malformed or naming another schema is an error. `rask init` writes the current version and `rask migrate` changes it |
| **MF2: Newer is rejected** | A version newer than the toolchain's schema is an error that says to update the toolchain. The tool never guesses at a future format |
| **MF3: Unknown keys are errors** | Every key must be in the schema. The error names the nearest valid key when one is close, else lists what belongs there. Silently ignored typos are the bug class this exists to prevent |
| **MF4: Typed strings** | TOML has no unit or version literals, so the field type checks the string: `"^2.0"` for a version range, `"1.0.0"` for a version, `"30s"` for a duration, a fixed set for an enum-like field |
| **MF5: Variant tables** | A table that picks one of several cases names the case by its key: exactly one of the case keys appears. A case may own companion keys (a git dependency's `rev`, `branch`), and shared keys are allowed whatever the case |
| **MF6: Every error at once** | Checking collects every problem in the file, each with its dotted path, instead of stopping at the first |
| **MF7: kebab-case keys** | Keys are kebab-case, the TOML convention and the same shape as package names: `dev-dependencies`, `opt-level`. A key maps onto the Rask field with `-` read as `_` |

A dependency is the clearest variant table. Where it comes from is the case; `target`, `with`, `allow`, `select` and `feature` are shared:

```
dependencies.tls: unknown key `revv`; did you mean `rev`?
dependencies.http: holds `registry`, `path`; pick exactly one of `registry`, `path`, `git`
dependencies.json: "two" is not a version range; write one like "^2.0", ">=1.5" or "=1.0.0"
```

The single-key form alone wasn't enough: a git dependency needs a URL and a revision, which is why cases own companion keys.

## Well-Formedness

Checked on the TOML itself, before any schema sees it.

| Rule | Description |
|------|-------------|
| **WF1: TOML subset** | Comments, bare/quoted/dotted keys, `[table]` headers, basic and literal strings, integers, booleans, arrays, inline tables. Floats, dates, multi-line strings and `[[array of tables]]` are errors that say so. A manifest has no use for them |
| **WF2: NFC keys** | A quoted key must be in NFC form. Two spellings of one name would otherwise be two keys |
| **WF3: No raw bidi controls** | Unescaped bidirectional control characters are rejected anywhere in the file. Text must parse the way it reads |
| **WF4: Duplicates** | A key set twice, a table defined twice, or a key used as both a value and a table is an error naming both |
| **WF5: Limits** | 1 MiB per file, 32 levels of nesting. Fixed numbers with defined errors |

## Schema and History

A schema is the current shape plus a history: one release per version, each a list of changes. Releases are append-only.

| Rule | Description |
|------|-------------|
| **SH1: Change kinds** | `Add` (with a default), `Remove` (records the removed definition), `Rename`, `Retype` (with an explicit table mapping old values to new), `Default` (records the old default) |
| **SH2: Release 1 builds from nothing** | Release 1 adds every field to an empty shape. Only it may add a required field; later a required field would break every older file, so it needs a default |
| **SH3: History is checked** | `check` replays the whole log from nothing and requires the result to equal the current shape, field order included. A shape edited without its change fails, and so does a past release rewritten out from under the current shape |
| **SH4: Paths** | A change targets a field list by path: a record field, every value of a map or list (`*`), or one case's companion keys |

SH3 catches a past release edited on its own. It can't catch one edited together with the current shape so the two still agree. Catching that needs the published schema to compare against, which is the registry's job, not the file's.

## Loading

| Rule | Description |
|------|-------------|
| **LD1: Fixed order** | Parse the TOML with no schema involved; read the declared version (MF1, MF2); replay the history forward from it; check against the current shape; map onto Rask types |
| **LD2: Forward replay** | Renames move old keys (both spellings present is an error). A field left out under an older default gets that old default written in, so it keeps its meaning. Retyped values go through the mapping; a value with no mapping is an error, never a guess. Removed fields are dropped |
| **LD3: Absent tables count** | A `Default` change reaches into optional tables the file left out: leaving out a whole table still meant the old default for each field in it |
| **LD4: Rask types** | The compiler maps the checked tree onto stdlib types (a `Package` struct, a `DepSource` enum), so manifest values are typed at comptime |

## Migrating

`rask migrate --to N` rewrites a manifest for version N, in either direction.

| Rule | Description |
|------|-------------|
| **MG1: Same log, both ways** | Backward undoes each change in reverse order: renames go back, retyped values map back through the same table, a changed default is written out explicitly |
| **MG2: Lossless or refused** | A downgrade that would drop information is refused with the key and value that block it. An added field drops only if the file leaves it at its default or the schema marks it ignorable |
| **MG3: Result is checked** | The rewritten file is checked against the shape as it stood at N before it is written |

## Editors

The current shape exports as JSON Schema (draft-07), which Taplo and the editor plugins built on it use to check `rask.toml` inline. Only the current version is exported: an editor shows what to write now, and the loader handles older files.

The export is weaker than the loader. It flags a typo but suggests nothing, and a bad dependency table comes back as "not valid under any of the schemas listed in the 'oneOf' keyword". `rask check` gives the real message.

## Out of Scope

Includes, variables, interpolation, conditionals and computed values. Each turns configuration into a program. A package that needs code writes `func build()` in `build.rk` (`struct.build/BL1`).

## In the Toolchain

The loader is written in Rask, as two packages. `libs/schema/` is the general part: the TOML reader, schemas with history, checking, migration and the JSON Schema export, usable for any config file. `compiler/crates/rask-manifest/loader/` holds the `rask-package` schema and the functions the compiler calls, and depends on `schema`. The compiler embeds both and runs them on its interpreter, as a package and its dependency, so there is one implementation, and it is Rask. Package discovery calls it through a hook the `rask` binary installs at startup.

That costs a type-check of the loader per process and an interpreter run per manifest. I took that over a second implementation in Rust: the loader is a real Rask program the toolchain depends on, which keeps the language honest, and speed isn't the constraint yet.

| Command | Does |
|---------|------|
| `rask manifest check [dir]` | Load `rask.toml` and print every problem |
| `rask manifest json-schema` | Print the current shape as JSON Schema |
| `rask migrate --to N [dir]` | Rewrite `rask.toml` for schema version N (MG1-MG3) |
| `rask add` / `rask remove` | Edit by line, so comments and layout stay; refused if the result wouldn't load (`struct.build/AD2`) |

## Open Questions

- The schema is Rask data in the loader, and the Rask types it maps onto (`Package`, `Dep`, `DepSource`) restate its shape by hand. Deriving one from the other would remove that.
- Changes name no types, so a change to the dependency shape is repeated at each of the seven places a dependency table appears. Named shapes in the schema would collapse that.
- How the history relates to editions, if editions ever exist: one version number or two axes.
- Whether the registry ever serves old manifests to old toolchains through `migrate --to`, or forward-only is enough.

## Implementation Status

| Feature | Status |
|---------|--------|
| TOML reader, schema, history check, load, migrate, JSON Schema export | Implemented, in Rask (`libs/schema/`, with the `rask-package` schema in `compiler/crates/rask-manifest/loader/`) |
| Toolchain reads `rask.toml` | Implemented |
| `rask migrate`, `rask manifest check`, `rask manifest json-schema` | Implemented |
| Format-preserving `rask add` / `rask remove` | Implemented (line-based) |
| Format-preserving `rask migrate` | Not implemented: a migrated file loses its comments, and the command says so first |

### See Also

- [Build System](build.md) — `build.rk` scripts, permissions, features, profiles (`struct.build`)
- [Packages](packages.md) — Versioning, lock file, registry (`struct.packages`)
