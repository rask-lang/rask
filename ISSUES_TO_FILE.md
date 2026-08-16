# Issues found but not filed

I hit these while working #696 and couldn't file them: the GitHub API is
refused for this session ("GitHub access is not enabled for this session. An
org admin must connect the Claude GitHub App for this organization"). Git
push works, so the branch is up, but REST 403s on every repo-scoped call and
GraphQL only serves a pinned set of PR-review operations. Each entry below is
a ready-to-paste issue. Delete this file once they're filed.

---

## 1. Interpreter hangs on `map.keys()` followed by `map.get()` in a loop

**Backend:** interpreter only. Native is fine.

Blocks forever at 0% CPU — blocked, not spinning.

```rask
func main() {
    mut m: Map<string, i64> = Map.new()
    m.insert("b", 2)
    m.insert("a", 1)
    mut keys: Vec<string> = Vec.new()
    for k in m.keys() {
        keys.push(k)
    }
    println("collected")
    for k in keys {
        if m.get(k)? as v {
            println("{k}={v}")
        }
    }
    println("done")
}
```

Native prints `collected / a=1 / b=2 / done`. The interpreter prints
`collected` and then hangs.

Either half alone is fine, so it's the combination:

- `keys()` loop, then a single `get` **outside** a loop → works.
- `get` in a loop over a Vec built by hand (never calling `keys()`) → works.
- `keys()` loop, then `get` **inside** a loop → hangs.

0% CPU points at the Map's mutex still being held by the `keys()` iterator
(`Value::Map(Arc<Mutex<MapData>>)` in rask-interp/src/value.rs) rather than an
infinite loop.

Found because it took down a workspace test: with the JSON encoder written
the obvious way (sort keys, look each back up), `rask run --interp
compiler/crates/rask-cli/tests/fixtures/json_encode_pretty.rk` hung and took
`cargo test --workspace` with it. Worked around in the encoder by taking key
and value together in one pass — the interpreter bug is untouched.

---

## 2. `compare()` on numbers escapes the Displayable check, and the two backends print different things

`Ordering` doesn't implement `Displayable`, so rendering one should be a
compile error — and for most receivers it is:

```rask
"a".compare("b").to_string()    // error[E0826]: `Ordering` does not implement `Displayable`  ✓
'a'.compare('b').to_string()    // error[E0826]  ✓
true.compare(false).to_string() // error[E0826]  ✓

(1).compare(2).to_string()      // compiles — should not
(1.5).compare(2.5).to_string()  // compiles — should not
```

Both numeric cases then disagree at runtime:

```
        (1).compare(2)     (1.5).compare(2.5)
native  0                  0                    ← raw enum tag
interp  Ordering.Less      Ordering.Less
```

So the integer and float `compare` return types aren't resolving to the real
`Ordering` type, which is what the Displayable check keys on. Only those two
paths leak; `string`/`char`/`bool` resolve it correctly.

There's an existing note about this shape in
`rask-types/src/checker/generics.rs` (the `ordering_type()` doc comment)
describing exactly this symptom, so part of it was known — but the integer
and float paths still slip through.

---

## 3. `f32` arithmetic runs at `f64` precision natively

```rask
func main() {
    let a: f32 = 3.7
    println(a.sqrt().to_string())
}
```

```
native  1.9235384185619262   ← f64 precision
interp  1.9235384            ← f32 precision, correct
```

Note the direction: `tests/known_fail_examples.txt` records the *interpreter*
running f32 at f64 precision for `game_loop`. This is the opposite — native
is the one keeping too many bits — so either that note is stale or these are
two different bugs.

---

## 4. `Vec.new()` + `push` + `as_ptr()` leaves the pointee type open

```rask
func main() {
    mut v = Vec.new()
    v.push(7)
    let b = unsafe v.as_ptr()
    print(unsafe *b)
}
```

```
error[E0361]: couldn't work out the type of `b`
  --> t.rk:4:9
    |
  4 |     let b = unsafe v.as_ptr()
    |         ^ type is still open here
```

`Vec.from([42]).as_ptr()` works (fixed in #696 — `substitute_type_params`
wasn't recursing into `RawPtr`). This variant still fails because the element
type arrives from `push` rather than from a literal the constructor can see,
and it isn't settled when `as_ptr`'s return type is built.

Same family as the `Vec.from` fresh-variable fallback in
`resolve_vec_static_method`: when the argument type isn't known yet it invents
an unconstrained element type instead of deferring. Uses with a defaulting
rule hide it; a pointer has none.
