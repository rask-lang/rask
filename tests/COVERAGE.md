# Language coverage map

One suite file per area of the language, across three horizons: what someone hits
in the **first hour**, in the **first week**, and in the **first month**. Every
file runs on both backends through `tests/differential.sh`.

Counts below are `tests passed / total`, per backend. A **BUILD-FAIL** means the
file doesn't compile on that backend at all — which for a probe file is the point.

A count marked **crashes** is worse than it looks: the test binary segfaults
part-way through, so the denominator is the tests that got to report, not the
tests in the file. No file is in that state right now — `t_day_const_array.rk`
was, until the element-store width fix took the segfault away and the const
length made the rest pass.

Two kinds of file:

- **Area files** are green on both backends. They're the regression gate: if one
  goes red, something broke.
- **Probe files** are red on purpose, each registered in
  `tests/known_divergences.txt` (a bug) or `tests/pending_features.txt` (unbuilt)
  against an issue. The harness reports them and fails if one silently starts
  passing, so a fix shows up as an UNEXPECTED PASS rather than going unnoticed.

Where an area file avoids a shape because it's broken, the probe next to it
carries that shape and the area file says so in a comment. That way the working
surface stays gated instead of hiding behind a known-fail line.

---

## Day one — the first hour

| Area | File | interp | native | Issues |
|---|---|---|---|---|
| println, interpolation, escapes | `t_day_println.rk` | 10/10 | 10/10 | |
| let, mut, shadowing, compound assign | `t_day_bindings.rk` | 10/10 | 10/10 | |
| integer arithmetic at every width | `t_day_int_math.rk` | 11/11 | 11/11 | |
| float arithmetic, mixed int/float rules | `t_day_float_math.rk` | 11/11 | 11/11 | |
| string methods | `t_day_strings.rk` | 13/13 | 13/13 | |
| bool, if/else, logical operators | `t_day_conditionals.rk` | 11/11 | 11/11 | |
| while, for, loop, break, continue | `t_day_loops.rk` | 12/12 | 12/12 | |
| Vec basics | `t_day_vec.rk` | 13/13 | 13/13 | |
| Map basics | `t_day_map.rk` | 11/11 | 11/11 | |
| fixed arrays `[T; N]` | `t_day_arrays.rk` | 7/7 | 7/7 | |
| functions, returns, default/named args | `t_day_functions.rk` | 12/12 | 12/12 | |
| structs, literals, field access, defaults | `t_day_structs.rk` | 12/12 | 12/12 | |
| module-level const | `t_day_const.rk` | 9/9 | 9/9 | |
| numeric conversions and casts | `t_day_casts.rk` | 11/11 | 11/11 | |
| `[T; N]` element writes | `t_day_array_writes.rk` | 6/6 | 6/6 | |
| `Map.insert`'s displaced value | `t_day_map_insert_displaced.rk` | 8/8 | 8/8 | |
| arrays sized by a named const | `t_day_const_array.rk` | 5/5 | 5/5 | |
| **probe** — `u8`/`u16` `.to<f64>()` | `t_day_unsigned_to_float.rk` | 6/6 | 5/6 | #974 |

## Week one

| Area | File | interp | native | Issues |
|---|---|---|---|---|
| enums, match, guards, arm order | `t_week_enums.rk` | 13/13 | 13/13 | |
| `T?`, `??`, `!`, `is … as`, `?.` | `t_week_optionals.rk` | 14/14 | 14/14 | |
| `T or E`, `try`, `catch` | `t_week_results.rk` | 13/13 | 13/13 | |
| methods via `extend` | `t_week_methods.rk` | 13/13 | 13/13 | |
| traits, `any Trait` dispatch | `t_week_traits.rk` | 11/11 | 11/11 | |
| generic functions and types | `t_week_generics.rk` | 11/11 | 11/11 | |
| closures, higher-order collection methods | `t_week_closures.rk` | 16/16 | 16/16 | |
| tuples | `t_week_tuples.rk` | 14/14 | 14/14 | #914 |
| ranges, iteration adapters | `t_week_ranges.rk` | 14/14 | 14/14 | #886 |
| parse and format | `t_week_parse_format.rk` | 14/14 | 14/14 | |
| sorting | `t_week_sorting.rk` | 14/14 | 14/14 | |
| nested data structures | `t_week_nested_data.rk` | 13/13 | 13/13 | #922 #925 |
| `test` blocks and the assertion forms | `t_week_test_blocks.rk` | 14/14 | 14/14 | |
| imports | `t_week_imports.rk` | 13/13 | 13/13 | |
| named-payload enum variants | `t_week_enum_named_payloads.rk` | 7/7 | 7/7 | |
| `Vec<T?>` literal elements | `t_week_optional_vec_literal.rk` | 5/5 | 5/5 | |
| `?.` onto an optional field | `t_week_optional_field_chains.rk` | 6/6 | 6/6 | |
| `?.` flattening a `T?` field | `t_week_optional_chain_flatten.rk` | 5/5 | 5/5 | |
| **probe** — inferred signatures | `t_week_gradual_generics.rk` | BUILD-FAIL | BUILD-FAIL | #904 #905 |
| implicit-param generic structs | `t_week_generic_struct_naming.rk` | 5/5 | 5/5 | |
| type param vs stdlib name | `t_week_generic_param_shadowing.rk` | 4/4 | 4/4 | |
| a method returning its own type parameter | `t_week_generic_method_return.rk` | 7/7 | 7/7 | |
| **probe** — `parse<T>` target range | `t_week_parse_range.rk` | 2/6 | 2/6 | #919 |
| `import X as Y` | `t_week_import_alias.rk` | 5/5 | 5/5 | |
| opaque handles as struct fields | `t_week_opaque_struct_fields.rk` | 8/8 | 8/8 | |
| **pending** — declared-but-unbuilt collection API | `t_week_collection_stubs.rk` | BUILD-FAIL | BUILD-FAIL | #912 |
| **pending** — range terminals and adapters | `t_week_range_adapters.rk` | BUILD-FAIL | BUILD-FAIL | #920 |

## Month one

| Area | File | interp | native | Issues |
|---|---|---|---|---|
| ownership, moves, 16-byte copy threshold | `t_month_ownership.rk` | 12/12 | 12/12 | |
| borrowing, disjoint fields, `with` | `t_month_borrowing.rk` | 13/13 | 13/13 | |
| box family — Cell, Mutex, Shared, Owned | `t_month_boxes.rk` | 13/13 | 13/13 | |
| `@resource` and `ensure` | `t_month_resource_ensure.rk` | 11/11 | 11/11 | |
| a program type named like a stdlib one | `t_month_stdlib_name_collision.rk` | 5/5 | 5/5 | |
| threads, channels | `t_month_concurrency.rk` | 11/11 | 11/11 | #267 |
| comptime | `t_month_comptime.rk` | 11/11 | 11/11 | |
| JSON encode and decode | `t_month_json.rk` | 13/13 | 13/13 | |
| fs, path, os, time, random | `t_month_stdlib_modules.rk` | 18/18 | 18/18 | |
| linearity — consume exactly once | `t_month_linearity.rk` | 12/12 | 12/12 | |
| file handles, buffered io | `t_month_io.rk` | 12/12 | 12/12 | |
| 128-bit integers | `t_month_i128.rk` | 12/12 | 12/12 | |
| **probe** — parameter modes | `t_month_param_modes.rk` | 6/10 | 6/10 | #899 |
| **probe** — test-block parameter scope | `t_month_borrow_name_shadow.rk` | BUILD-FAIL | BUILD-FAIL | #926 |
| **probe** — `@resource` in a loop | `t_month_resource_loop.rk` | BUILD-FAIL | BUILD-FAIL | #928 |
| `ensure` block scoping | `t_month_ensure_block_scope.rk` | 11/11 | 11/11 | |
| CT49 field access by literal | `t_month_comptime_field_literal.rk` | 11/11 | 11/11 | |
| comptime `FieldInfo.name` | `t_month_reflect_field_strings.rk` | 11/11 | 11/11 | |
| **probe** — `reflect.fields()` as a value | `t_month_reflect_fields_value.rk` | 4/4 | BUILD-FAIL | #997 |
| **probe** — `try` in a test block | `t_month_try_in_test.rk` | 5/5 | BUILD-FAIL | #932 |
| i128 in aggregates and conversions | `t_month_i128_aggregates.rk` | 20/20 | 20/20 | |
| unsigned widening to 128 bits | `t_month_u128_widening.rk` | 6/6 | 6/6 | |
| unsafe blocks and raw pointers | `t_month_unsafe.rk` | 16/16 | 16/16 | |
| **pending** — atomics | `t_month_atomics.rk` | BUILD-FAIL | BUILD-FAIL | #927 |
| floats in word-wide slots | `t_week_float_slots.rk` | 9/9 | 9/9 | |

---

## Holes left on purpose

**Pool and Handle.** Excluded from this sweep by standing request. Existing
coverage: `t51_pools.rk`, `t_pool_capacity.rk`, `t_pool_iter_handles.rk`,
`t_pool_remove.rk`, `t_bounded_pool.rk`, `t_niche_handle_option.rk`.

**Cross-file user imports.** `import mypkg.mymodule.Thing` between two files you
wrote can't live in this suite — the differential harness compiles each file on
its own, and a user module needs a package around it. `tests/projects_gate.sh`
covers it, including cross-package symbol export, and
`t_week_imports.rk` covers the stdlib side (which is the import anyone writes on
day one).

**`net` and `http`.** Both need a listener. `t_net_loopback.rk` and the HTTP
harness (`tests/http_api_harness.sh`) own them; a file the differential harness
runs on every invocation shouldn't be binding ports.

**C interop, but not `unsafe`.** I initially wrote the whole area off as needing a
C toolchain and a companion object file. The raw-pointer half turns out to be
perfectly testable in one file, and doing so found #935 — the interpreter treated a
raw pointer as a plain i64, so `*p` silently yielded 0 while native read the byte.
Fixed; `t_month_unsafe.rk` now covers dereference, `read()`, `write()`, the
arithmetic and alignment methods, pointer identity, `cast`, `null`, Vec pointers,
`string.from_raw`/`from_c`, and the U3 and UF1 forms — 16 tests, green on both
backends. Widening it past the original six found two more native bugs: #985
(pointer stride disagrees with the Vec's slot width for elements under 8 bytes)
and #986 (`cast<U>()` drops the `<U>`, so an inline `*p.cast<u8>()` reads a word).

What can't go in it, for the same reason in both cases — a test that panics
fails — is reading past the end of a buffer and dereferencing null. mem.unsafe
specifies those as a panic with a location in debug and UB in release; the
interpreter always panics, native reads whatever is there. That's the specified
split between the modes, not a divergence.

What genuinely does need a build harness is the C-interop half: `compile_rust()` in
a build script, the C ABI, cbindgen, linking a real object file. That stays
uncovered. `&x` to take a raw pointer to a local also isn't available on either
backend, so `as_ptr()` is the only way to get a pointer today.

**The build system and multi-package projects.** `tests/projects_gate.sh` and
`tests/examples_gate.sh` are the right harnesses for this — a suite file can't
have a second package.

**A fixed array's growth operations.** `push`, `pop` and `clear` on a `[T; N]`
are a compile error (E0843), and a test can't assert on one. They live in
`tests/compile_errors/fixed_array_growth.rk`; the reads and writes an array does
have are in `t_day_arrays.rk` and `t_day_array_writes.rk`.

**Linearity's rejections.** `t_month_linearity.rk` pins the positive side — every
shape where consuming exactly once is legal — because a test can't assert on a
compile error. The rejections (forgot to close, closed twice, consumed on one arm
only) belong in `tests/compile_errors/`.

**A float in a word-wide slot.** `t_week_float_slots.rk` sweeps the nine positions
a float can occupy in one — match-expression result, concrete and generic enum
payload, generic struct field, optional, result, `Vec` element, `Map` value,
array element, tuple element, and through a generic function. Three of them
disagreed with the one convention (#972, #973), and `t_week_enums.rk` stayed
green through all of it because none of its variants carry a float. That is the
lesson the file records: an area file only gates the shapes it happens to use.

**Panics and unwinding.** A test that panics fails, so a suite file can't assert
on a panic's behaviour without failing. `specs/control/panics.md` describes
task-kill plus unwind with ensures running; the `ensure`-ordering half of that is
covered in `t_month_resource_ensure.rk` and `t_month_ensure_block_scope.rk`, and
the panic half wants a harness that runs a program expecting a non-zero exit.
`tests/compile_errors/` is the nearest existing pattern.

**Assertion failure messages — no longer a hole.** #897 and #898 were both
filed as ungateable, on the reasoning that a test can't assert on its own
failure message. That's true from inside Rask and it doesn't apply from outside:
a Rust integration test can run `rask test` on a failing file and read what the
message said. `assertion` in `rask-cli/tests/compile_run.rs` does that on both
backends, and the float case asserts the two render character for character.
Anything about how a diagnostic or a runtime message *reads* belongs there
rather than in a suite file.

**`select`.** Covered by `t_select.rk` and `p06_select.rk`.

**SIMD, `@binary`, the sequence protocol.** All pending features with their own
probe files already: `p09_simd.rk`, `p10_binary.rk`, `p08_sequence.rk`.

---

## Spec questions

**Nested optionals through `?.` — answered, and implemented.** `a.inner?.v` where
`v: string?` is a `string?`, not a `string??`: the chain unwraps, and both
absences mean the same thing to the caller, so there's nothing to keep apart.
The nesting rules still apply to a `T??` that comes from a generic —
`Vec<Config?>.first()`, where the two absences are different facts — and OPT10
says a chain isn't that shape.

Both runtimes always flattened; the checker didn't, because it decided at the
access, where the field's type is still the fresh variable a deferred
`HasField` will fill in. "Is it already an option?" asked of a bare variable
always answers no. It's a deferred constraint now, settled once the field's
type is (#938), and #917 went green with it.

**`mutate` on a Copy type — still open.** `mem.parameters` says two different things — PM2's
prose promises the caller sees the write, the edge-case table says a Copy type's
mutations affect the copy. The implementation does neither cleanly: primitives
drop the write, aggregates keep it at any size. Raised on #899, where
`t_month_param_modes.rk` asserts PM2's reading.

---

## Running it

```
tests/differential.sh          # every suite file, both backends, compared
tests/examples_gate.sh
tests/projects_gate.sh
tests/fmt_roundtrip_gate.sh    # catches `rask fmt` breaking a file — it did, twice
rask test-specs specs/
cd compiler && cargo test --release --workspace
```
