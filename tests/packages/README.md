# Multi-package fixtures

Nothing in the repo declared a dependency until these existed. `grep -rn 'dep "'`
across `projects/`, `examples/` and `tests/` found no hits, so every multi-package
build was untested — which is how #1112 reached the tree: a library that exports a
function returning its own struct can't be consumed at all, and *any* consumer
fails, whether or not it calls that function.

Each fixture is a `libpkg/` and an `app/` that depends on it by path. The gate
(`tests/packages_gate.sh`) builds `app/`, runs it, and diffs its output against
`expected.txt`. It deletes `rask.lock` first — the dependency is a relative path
in this same tree, so the lockfile pins nothing and its checksum goes stale the
moment anyone edits the library, reporting an edit you just made on purpose as
"dependency 'libpkg' has changed".

A fixture that isn't expected to work yet goes in `tests/known_fail_packages.txt`
with its tracking issue. Either kind of failure counts: `const-export/` builds and
links and then prints the wrong number.

| fixture | shape | state |
|---|---|---|
| `basic/` | a scalar function, a struct the *caller* constructs, functions that *take* one | green |
| `constructor/` | a function that constructs the library's own struct and hands it back | #1112 |
| `const-export/` | reading an exported `const` | #1123 — builds, prints `0` |
| `method-export/` | calling a method on an exported struct | #1124 |
| `vec-return/` | a function returning `Vec<T>` | #1125 |
| `enum-export/` | `import libpkg.SomeEnum` | #1126 |

Five of the six are the same root: `check_package` flattens a dependency's public
declarations into the root's and renames each through `prefix_decl`, which renames
the declaration's own name and nothing that refers to it — then pushes a *second*,
unprefixed copy when the consumer imported the name, giving the type two
declarations and two TypeIds. Whoever fixes it will likely close all five at once.

`const-export/` is the one to look at first anyway: it is the only one that
doesn't fail loudly.
