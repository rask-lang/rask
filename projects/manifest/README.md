# manifest

Prototype of the `rask.toml` loader from [struct.manifest](../../specs/structure/manifest.md), written in Rask: a TOML reader, the `rask-package` schema with its change history, loading, `migrate --to`, the history check, and JSON Schema export.

```
rask build projects/manifest
build/debug/manifest load examples/my-api/rask.toml
build/debug/manifest migrate examples/my-api/rask.toml 1
build/debug/manifest check
build/debug/manifest json-schema > rask-package.schema.json
rask test projects/manifest            # run from the repo root
```

The schema ships at release 1. The tests build a made-up release 2 (a rename, a changed default, an added field) to exercise forward loading, lossless and refused downgrades, and a rewritten history.

What it leaves out: comment-preserving writes (a migrated file loses its comments and layout), and the compiler side, which still reads the `package` block in `build.rk`.

| File | Holds |
|------|-------|
| `toml.rk` | TOML subset reader and writer, well-formedness checks |
| `schema.rk` | Shapes, changes, replaying the log on shapes, the history check |
| `check.rk` | Checking a tree against a shape, with suggestions |
| `replay.rk` | Load and migrate: the log run over a file, either direction |
| `package_schema.rk` | `rask-package` release 1 |
| `package.rk` | The checked tree onto Rask types |
| `jsonschema.rk` | JSON Schema export for editors |
