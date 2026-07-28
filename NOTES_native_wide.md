# Native `Wide<T>` — status and the two pre-existing bugs that block it

Working notes from implementing `Wide<T>` (conc.data-parallel). Not a spec.

## What works

- **Interpreter — full.** `Vec.wide()`, `map`, `zip_with`, `sum`, `read` all
  run correctly under `rask run --interp`. Lazy plan (`WidePlan`), executed by
  a terminal. Test: `wide_basic_interp`.
- **Native — closure-free subset.** `v.wide().sum()` (and `.read()`) compile
  and run natively, and the output matches the interpreter (the W3 reference-
  semantics oracle). Test: `wide_native_sum_matches_interp`. `sum` folds int64
  lanes in `rask_wide_sum` (runtime/vec.c); `wide`/`read` reuse `rask_vec_clone`
  since a `Wide<T>` is a `RaskVec*` at runtime.

## What's blocked, and why

`Wide.map` / `Wide.zip_with` take a **closure**, and the native closure-into-
collection path is **pre-existing-broken** — independent of `Wide`. Two distinct
bugs, both reproduced on a clean tree (only interpreter + stub touched):

### Bug 1 — `Vec.map(closure)` segfaults natively

```rask
func main() {
    mut xs = Vec.new()
    xs.push(10)
    xs.push(20)
    const ys = xs.map(|x| x + 1)   // segfault at runtime (exit 139)
    print("len={ys.len()}\n")
}
```
`rask compile` succeeds; the produced binary segfaults. Likely causes, both
plausible and possibly compounding:
- **Closure ABI.** `rask_vec_map(src, fn_ptr)` (runtime/vec.c:284) casts
  `fn_ptr` to a bare `int64_t(*)(int64_t)` and calls `func(elem)` with no
  captured-environment argument. If closures are lowered to a (fn-ptr + env)
  pair (rask-mir `lower/closures.rs`), passing/calling as a bare fn-ptr is
  wrong.
- **Float register ABI.** Elements are read as raw `int64_t` and passed in an
  integer register. For `f64` lanes/closures the value belongs in an xmm
  register, so `func` receives garbage. `rask_vec_map`'s own comment calls it a
  "Stub."

### Bug 2 — `Vec.from([literal])` corrupts the heap natively

```rask
func main() {
    const xs = Vec.from([1, 2, 3, 4])
    print("{xs.join(\", \")}\n")   // free(): unaligned chunk detected
}
```
`Vec_from` is mapped to `rask_vec_clone` (rask-codegen/src/dispatch.rs), which
expects a `RaskVec*` but is handed the array literal — so it reads a bogus
`{data,len,cap,elem_size}` and corrupts the allocator. (`Vec.new()` + `push`
works fine, which is why the tests build input that way.)

## Consequence for `Wide`

Until Bug 1 is fixed, `Wide.map`/`zip_with` stay interpreter-only. The native
dispatch entries deliberately cover only the closure-free ops. When the native
closure-callback path works (correct env passing + float-aware element ABI),
`Wide_map`/`Wide_zip_with` can route to `rask_wide_map`/`rask_wide_zip_with`
runtime functions mirroring `rask_vec_map`, and the native tests can be
extended to the full algebra.

These are pre-existing native-backend bugs, not `Wide` bugs — they should be
filed against `rask-lang/rask` with the repros above.
