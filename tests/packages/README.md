# Multi-package fixtures

Nothing in the repo declared a dependency until these existed. `grep -rn 'dep "'`
across `projects/`, `examples/` and `tests/` found no hits, so every multi-package
build was untested — which is how #1112 reached the tree: a library that exports a
function returning its own struct can't be consumed at all, and *any* consumer
fails, whether or not it calls that function.

Each fixture is a `libpkg/` and an `app/` that depends on it by path. The gate
(`tests/packages_gate.sh`) builds `app/` and diffs its output against
`expected.txt`. A fixture that isn't expected to build yet goes in
`tests/known_fail_packages.txt` with its tracking issue.

- `basic/` — the three shapes that work: a scalar function, a struct the *caller*
  constructs, and a function that *takes* one.
- `constructor/` — the shape that doesn't: a function that constructs the
  library's own struct and hands it back. #1112.
