# Raido conformance claims

Claims that any Raido implementation has to satisfy, written against the spec and the host API contract in [../vm/interop.md](../vm/interop.md#the-contract), not against any one implementation.

An implementation is a Rask package named `raido` that exposes that contract. It applies the claims from its `rask.toml`:

```toml
[claims]
conformance = { path = "path/to/projects/raido/conformance" }
```

`rask claims .` then checks every claim against that package, and `rask build` and `rask test` check them too.

Each claim is a property over generated inputs: integer arithmetic for every pair of `i64`s, float text for every finite `f64`, sort order for every `Vec<i64>`, rollback for every sequence of writes. Where the spec fixes an exact answer, the claim compares against a reference written straight from the spec, in `helpers.rk`: integer division and remainder, the merge sort and its comparator calls, and the PRNG. The PRNG reference was cross-checked against a separate Python version of the same spec text.

| File | What it holds |
|------|---------------|
| `numbers.rk` | integer arithmetic, conversions, `round`, float and int text, host values of the wrong type |
| `collections.rk` | the merge sort, map insertion order |
| `state.rk` | commit or rollback as a whole, deferred calls, snapshots, closures sharing captures, garbage and the memory limit |
| `tasks.rk` | pausing and resuming, `spawn`, the PRNG |
| `helpers.rk` | building a VM from Raido source, results as text, the reference models |

The claims need an implementation with a compiler, since each one compiles Raido source. They type-check against the contract today; no implementation runs them yet.
