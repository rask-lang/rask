# Language coverage map

One suite file per area of the language, across three horizons: what someone hits
in the **first hour**, in the **first week**, and in the **first month**. Every
file runs on both backends through `tests/differential.sh`.

Counts below are `tests passed / total`, per backend. A **BUILD-FAIL** means the
file doesn't compile on that backend at all — which for a probe file is the point.

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
| println, interpolation, escapes | `t_day_println.rk` | 10/10 | 10/10 | #897 #898 |
| let, mut, shadowing, compound assign | `t_day_bindings.rk` | 10/10 | 10/10 | |
| integer arithmetic at every width | `t_day_int_math.rk` | 11/11 | 11/11 | |
| float arithmetic, mixed int/float rules | `t_day_float_math.rk` | 11/11 | 11/11 | |
| string methods | `t_day_strings.rk` | 13/13 | 13/13 | #900 |
| bool, if/else, logical operators | `t_day_conditionals.rk` | 11/11 | 11/11 | |
| while, for, loop, break, continue | `t_day_loops.rk` | 12/12 | 12/12 | |
| Vec basics | `t_day_vec.rk` | 13/13 | 13/13 | |
| Map basics | `t_day_map.rk` | 11/11 | 11/11 | |
| fixed arrays `[T; N]` | `t_day_arrays.rk` | 7/7 | 7/7 | |
| functions, returns, default/named args | `t_day_functions.rk` | 12/12 | 12/12 | |
| structs, literals, field access, defaults | `t_day_structs.rk` | 12/12 | 12/12 | |
| module-level const | `t_day_const.rk` | 9/9 | 9/9 | |
| numeric conversions and casts | `t_day_casts.rk` | 11/11 | 11/11 | |
| **probe** — `[T; N]` writes and growth | `t_day_array_writes.rk` | 5/7 | 1/6 | #902 #901 |
| **probe** — `Map.insert`'s displaced value | `t_day_map_insert_displaced.rk` | 5/5 | 0/3 | #903 |
| **probe** — `[T; const]` length | `t_day_const_array.rk` | 5/5 | 0/3 | #906 |
| **probe** — `unsigned as f64` | `t_day_unsigned_to_float.rk` | 6/6 | 1/6 | #907 |

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
| **probe** — named-payload enum variants | `t_week_enum_named_payloads.rk` | 2/7 | 3/7 | #910 #911 |
| **probe** — `Vec<T?>` literal elements | `t_week_optional_vec_literal.rk` | 2/5 | 5/5 | #909 |
| **probe** — `?.` onto an optional field | `t_week_optional_field_chains.rk` | 6/6 | 0/2 | #917 |
| **probe** — inferred signatures | `t_week_gradual_generics.rk` | BUILD-FAIL | BUILD-FAIL | #904 #905 |
| **probe** — implicit-param generic structs | `t_week_generic_struct_naming.rk` | BUILD-FAIL | BUILD-FAIL | #913 |
| **probe** — type param vs stdlib name | `t_week_generic_param_shadowing.rk` | BUILD-FAIL | BUILD-FAIL | #915 |
| **probe** — generic method return type | `t_week_generic_method_return.rk` | 7/7 | BUILD-FAIL | #916 |
| **probe** — `parse<T>` target range | `t_week_parse_range.rk` | 2/6 | 2/6 | #919 |
| **probe** — `import X as Y` | `t_week_import_alias.rk` | BUILD-FAIL | BUILD-FAIL | #923 |
| **probe** — opaque handles as struct fields | `t_week_opaque_struct_fields.rk` | 8/8 | 0/0 | #924 |
| **pending** — declared-but-unbuilt collection API | `t_week_collection_stubs.rk` | BUILD-FAIL | BUILD-FAIL | #912 |
| **pending** — range terminals and adapters | `t_week_range_adapters.rk` | BUILD-FAIL | BUILD-FAIL | #920 |

## Month one

| Area | File | interp | native | Issues |
|---|---|---|---|---|
| ownership, moves, 16-byte copy threshold | `t_month_ownership.rk` | 12/12 | 12/12 | |
| borrowing, disjoint fields, `with` | `t_month_borrowing.rk` | 13/13 | 13/13 | |
| box family — Cell, Mutex, Shared, Owned | `t_month_boxes.rk` | 13/13 | 13/13 | |
| `@resource` and `ensure` | `t_month_resource_ensure.rk` | 11/11 | 11/11 | |
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
| **probe** — `ensure` block scoping | `t_month_ensure_block_scope.rk` | 5/5 | 0/5 | #929 |
| **probe** — CT49 field access by literal | `t_month_comptime_field_literal.rk` | 4/4 | BUILD-FAIL | #930 |
| **probe** — comptime `FieldInfo.name` | `t_month_reflect_field_strings.rk` | 6/6 | BUILD-FAIL | #931 |
| **probe** — `try` in a test block | `t_month_try_in_test.rk` | 5/5 | BUILD-FAIL | #932 |
| **probe** — i128 in aggregates and conversions | `t_month_i128_aggregates.rk` | 10/10 | BUILD-FAIL | #933 |
| **probe** — `u64 as u128` | `t_month_u128_widening.rk` | 4/6 | 6/6 | #934 |
| **pending** — atomics | `t_month_atomics.rk` | BUILD-FAIL | BUILD-FAIL | #927 |

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

**`unsafe` and C interop.** Not covered here. It needs a C toolchain and a
companion object file to be meaningful, which is a build-system shape rather than
a single-file one. `specs/memory/unsafe.md` is the spec; nothing in
`tests/suite/` exercises it today, and it's the largest genuinely uncovered area
this sweep found.

**The build system and multi-package projects.** `tests/projects_gate.sh` and
`tests/examples_gate.sh` are the right harnesses for this — a suite file can't
have a second package.

**Linearity's rejections.** `t_month_linearity.rk` pins the positive side — every
shape where consuming exactly once is legal — because a test can't assert on a
compile error. The rejections (forgot to close, closed twice, consumed on one arm
only) belong in `tests/compile_errors/`.

**Panics and unwinding.** A test that panics fails, so a suite file can't assert
on a panic's behaviour without failing. `specs/control/panics.md` describes
task-kill plus unwind with ensures running; the `ensure`-ordering half of that is
covered in `t_month_resource_ensure.rk` and `t_month_ensure_block_scope.rk`, and
the panic half wants a harness that runs a program expecting a non-zero exit.
`tests/compile_errors/` is the nearest existing pattern.

**`select`.** Covered by `t_select.rk` and `p06_select.rk`.

**SIMD, `@binary`, the sequence protocol.** All pending features with their own
probe files already: `p09_simd.rk`, `p10_binary.rk`, `p08_sequence.rk`.

---

## Two spec questions raised, not answered

**Nested optionals through `?.`.** What `a.inner?.v` gives when `inner` is present
and `v` is `none` — `Some(none)` (layers stay distinct, per the nesting section) or
`none` (the chain short-circuits, per how `user?.name ?? "guest"` reads). OPT10's
"when present" refers to the receiver, which *is* present. The interpreter
short-circuits. Not asserted either way; raised on #917.

**`mutate` on a Copy type.** `mem.parameters` says two different things — PM2's
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
