# manifest loader

The `rask.toml` loader from [struct.manifest](../../../../specs/structure/manifest.md), written in Rask. The compiler embeds these files (`../src/lib.rs`) and runs them on its interpreter, so this is the only implementation.

To add a release of the `rask-package` schema: change the shape in `package_schema.rk` and append a `Release` with the matching changes to its history. `cargo test -p rask-manifest` replays the history and fails if the two disagree.

```
rask test compiler/crates/rask-manifest/loader     # the Rask tests, native
cargo test -p rask-manifest                        # the same tests on the interpreter, plus the Rust side
```

The tests build a made-up release 2 (a rename, a changed default, an added field) to exercise forward loading, lossless and refused downgrades, and a rewritten history.

| File | Holds |
|------|-------|
| `toml.rk` | TOML subset reader and writer, well-formedness checks |
| `schema.rk` | Shapes, changes, replaying the log on shapes, the history check |
| `check.rk` | Checking a tree against a shape, with suggestions |
| `replay.rk` | Load and migrate: the log run over a file, either direction |
| `package_schema.rk` | `rask-package`: the shape and its history |
| `package.rk` | The checked tree onto Rask types |
| `jsonschema.rk` | JSON Schema export for editors |
| `toolchain.rk` | What the compiler calls; results as JSON |
