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
collection path is **pre-existing-broken** — independent of `Wide`. This is
filed as **rask-lang/rask#441**.

### The blocker — `Vec.map(closure)` segfaults natively (#441)

```rask
func main() {
    mut xs = Vec.new()
    xs.push(10)
    xs.push(20)
    const ys = xs.map(|x| x + 1)   // native binary segfaults (exit 139)
    print("len={ys.len()}\n")
}
```
`rask compile` succeeds; the produced binary segfaults; the interpreter is
correct. `--dump-mir` shows why:
```
_4 = closure[stack](main__closure_0, [])
_5 = Vec_map(_1, _4)
func main__closure_0(__env: ptr, x: i64) -> i64 { ... }
```
`Vec_map` is handed a **closure object** (`{fn_ptr, env}`), but `rask_vec_map`
(runtime/vec.c) casts its arg straight to a bare `int64_t(*)(int64_t)` and calls
`func(elem)` — wrong on two counts: it treats the object as a code pointer, and
the real fn is `(__env: ptr, x: i64)` (env-first, two params). Elements are also
read as raw `int64` and passed in an integer register, so `f64` closures would
additionally hit a float-register ABI mismatch. `rask_vec_map` is marked a
"Stub." Full detail and fix sketch in #441.

Not to be confused with **#414** (`Vec<i64>.join` heap corruption): that crash
is in `rask_vec_join_i64`, not in `Vec.from`. `Vec.from([...])` and
`Vec.new()`+`push` both work natively — only `join` on a non-string Vec
corrupts. (The tests avoid `join` on native paths for this reason.)

## Consequence for `Wide`

Until #441 is fixed, `Wide.map`/`zip_with` stay interpreter-only. The native
dispatch entries deliberately cover only the closure-free ops. When the native
closure-callback path works (correct object/env calling convention + float-aware
element ABI), `Wide_map`/`Wide_zip_with` can route to `rask_wide_map`/
`rask_wide_zip_with` runtime functions mirroring `rask_vec_map`, and the native
tests can be extended to the full algebra.
