# manifest loader

The `rask-package` schema for `rask.toml` ([struct.manifest](../../../../specs/structure/manifest.md)), the Rask types it maps onto, and the `toolchain_*` functions the compiler calls. Everything general (the TOML reader, schemas with history, checking, migration) is the [`schema` library](../../../../libs/schema/), a dependency of this package.

The compiler embeds both packages (`../src/lib.rs`) and runs them on its interpreter, so this is the only implementation.

To add a release of `rask-package`: change the shape in `package_schema.rk` and append a `Release` with the matching changes to its history. `cargo test -p rask-manifest` replays the history and fails if the two disagree.

```
rask test compiler/crates/rask-manifest/loader     # these tests and the library's, native
cargo test -p rask-manifest                        # the same on the interpreter, plus the Rust side
```

| File | Holds |
|------|-------|
| `package_schema.rk` | `rask-package`: the shape and its history |
| `package.rk` | The checked tree onto Rask types |
| `toolchain.rk` | What the compiler calls; results as JSON |
