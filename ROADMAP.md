# Rask Roadmap

Strategic phases. Open work items are in [TODO.md](TODO.md); bugs are [GitHub issues](https://github.com/rask-lang/rask/issues). For the current spec-vs-compiler gap and its work order, see [PLAN.md](PLAN.md).

## Where things stand

Frontend, ownership, interpreter, monomorphization, MIR lowering, Cranelift backend, build system, package management — all working. 90 decided specs, unchanged from last measure.

Simple programs compile natively (hello world, structs, closures, Vec/Map, threads, channels, file I/O). As of 2026-09-07: 2 tracked bugs (both interpreter-only — native is correct on both) and 7 unbuilt features, each with a probe file in the suite. That's up from "0 bugs, 7 unbuilt" a week ago, but it's not backsliding — both new bugs were caught by a harness audit that got better at looking, not by anything breaking. See [PLAN.md](PLAN.md) for the work order.

## Validation programs

Re-measured 2026-09-07 by running all five. **All five now work, including the HTTP server** — last week's native regression (#1036, every response's first 8 bytes coming back as garbage) is gone. I confirmed it directly: pulled the 12-line repro straight from the issue and ran it — output is now byte-for-byte correct (`HTTP/1.1 200 OK\r\n...`), not just "the gate happens to pass."

| Program | Status | Gate |
|---------|--------|------|
| Sensor processor | **Works** | examples gate, golden |
| grep clone | **Works** | examples gate, golden + argv |
| Game loop with entities | **Works** | examples gate, golden (seeded RNG) |
| Text editor with undo | **Works** | examples gate, golden + stdin |
| HTTP JSON API server | **Works** | `tests/http_api_harness.sh`, both backends |

I couldn't find the exact commit that fixed #1036 — the issue is still open, and the PR that claims to close it (#1054) is an unmerged draft. The likely source is #1056 ("Audit the test harness..."), whose own summary mentions fixing "a use-after-free corrupting every response the native server sent" — a different bug than the one #1036 describes (uninitialized bytes vs. freed memory), but on the same send path, so one commit plausibly fixed both. Worth a comment on #1036 closing it out with this note, since nobody currently gets credit for the fix.

## Stdlib architecture

| Layer | Language | What lives here |
|-------|----------|-----------------|
| **Runtime** | C | OS interface, memory primitives, data structures, concurrency, raw I/O |
| **Stdlib** | Rask | Everything above the OS — HTTP, JSON, CSV, URL, base64, hashing, unicode, terminal |

Dogfooding validates the language. Rask code gets ownership and bounds checking that C doesn't. If the language can't handle an HTTP parser, something's wrong.

C stays for things that must talk to the OS (syscalls, io_uring) or wrap existing C libraries (TLS via OpenSSL/mbedTLS, hardware crypto).

---

## An open draft PR is sitting on ~100 issues

[#1054](https://github.com/rask-lang/rask/pull/1054) is a 148-commit, 26k-line draft branch that claims to close roughly a hundred issues — atomics, three package-system bugs, C-struct interop, `reflect`, several use-after-free classes, closure mutation enforcement (MC2), and more. It's based on the current `main` tip and GitHub reports it as cleanly mergeable. None of it is reflected in the numbers above or in "what comes next" below, because it isn't merged — this roadmap describes `main`, not a branch sitting next to it.

I'm flagging it rather than folding it in because merging (or reviewing, or discarding) a change of that size is a call for a person, not something to wave through as part of a measurement pass. If it's good, most of the "what comes next" section below is already done and just needs a merge.

## What comes next, and why in this order

The sequence protocol used to be one item that unblocked three others. It mostly landed since last measure (PR #1042: `Sequence<T>` is now the real backbone of `Vec.iter()`, adapters are lazy), so what's left below is a set of independent gaps rather than a dependency chain — ordered by how visible each one is to a day-one user, not by what it unblocks.

### 1. Ranges still have no methods (#920)

`Vec` iteration now goes through `Sequence<T>` and works — `v.iter().filter(...).map(...).to_vec()` all run correctly on both backends, confirmed directly against `tests/suite/p21_sequence_adapters.rk`. Ranges didn't get any of it: `(0..10).sum()`, `.map()`, `.filter()`, even `.iter()` still fail with "no method found," confirmed directly against `tests/suite/t_week_range_adapters.rk`. A `for x in range` loop works; anything else on a range doesn't. This is the most-reached-for missing thing now that the collection side works, since a range is the obvious thing to reach for before a `Vec` even exists.

### 2. Eleven declared Vec/Map methods, still unimplemented (#912)

Unchanged since last measure — `remove_where`, `take_where`, `get_clone`, and the capacity-control cluster (`reserve`, `shrink_to_fit`, etc.) all still fail with "declared but not implemented" on both backends. Confirmed directly against `tests/suite/t_week_collection_stubs.rk`. Most of these are thin wrappers over state the runtime already tracks (`capacity()`, `is_bounded()` work today); `remove_where`/`take_where` are the two with real logic.

### 3. Atomics have zero operations on any spelling (#927)

Unchanged. `Atomic<i64>.new(0)` type-checks and then has no `.load`, `.store`, `.add` — confirmed directly against `tests/suite/t_month_atomics.rk`. `Mutex` is the working substitute today, which is exactly what atomics exist to avoid paying for.

### 4. The sequence protocol's remaining edges (#1046)

Two smaller things left in the otherwise-landed protocol: the operator-bound terminals (`sum`, `product`, `min`, `max`, `join`) need a Rust-driven closure call the way `rask_vec_sort_by` already does one for its comparator, and a user-written `Sequence<T>` (not going through `Vec`) works on native but not on the interpreter — `tests/suite/p08_sequence.rk` is now a `known_divergences.txt` entry rather than a `pending_features.txt` one, i.e. it went from "unbuilt" to "built, but the interpreter has a bug." That's forward motion even though it's still red.

### 5. Panics — same one small tracker left as last measure

No change: [#298](https://github.com/rask-lang/rask/issues/298), 10 of [#299](https://github.com/rask-lang/rask/issues/299)'s 11 sub-issues closed. What's left: detached-task panics should print to stderr (silent today), a guard that panics during unwind should report as a secondary panic instead of replacing the original, the task id should prefix the panic line, and a panic reaching an FFI boundary should abort there instead of unwinding into foreign frames.

### 6. Incremental compilation

Unchanged. Function-granularity design exists (spec: [incremental.md](specs/compiler/incremental.md)), semantic hashing is done, but `rask build` itself doesn't cache or patch at function granularity yet. Not re-verified in detail this pass beyond confirming nobody's touched `rask-semantic-hash` or the build cache since last measure.

### 7. Cross-compilation — not re-verified this pass

Last measure's correction stands unchanged: `--target` reaches Cranelift's ISA lookup, `rask targets` lists all three tiers, and what's actually missing is the wider toolchain (cross-linker detection, multi-target builds, XT1–XT8 in `specs/structure/build.md`). Didn't re-run the zig/gcc cross-link check this pass — no reason to think it changed, but flagging that "unchanged" here means "no new evidence," not "re-confirmed."

## On the LLVM backend

Deferring it, unchanged reasoning: the two backends disagreeing is still the largest bug class this project produces, and Cranelift already reaches ARM/WASM, so LLVM's usual "more targets" argument doesn't apply here. Nothing measures generated-code quality yet (`benchmarks/` has one ceremony comparison, no speed benchmark). **Nothing goes to LLVM until something measures slow.**

## The agent benchmark

`agentbench/` — 19 tasks, reference solutions, model adapters. `agentbench_gate.sh` (the free `selftest`) passed this measure: 18 green, 1 quarantined.

The quarantine itself needed a fix this pass: #1002 (native mis-dispatching a method on a union-narrowed error) is fixed on `main` — I confirmed by hand-running the task's own test suite natively, all 5 pass. But `agentbench/quarantine.txt` still listed the task as broken for that reason. Digging in, the task fixture has an unrelated bug: `verify.rk` imports `string.ParseError`, which nothing in the task uses and which collides with the task's own `ParseError` enum (`E0208`, one-name-one-meaning) — a real, if minor, compiler-caught error that was there all along and just never surfaced because #1002 failed first. Filed as [#1133](https://github.com/rask-lang/rask/issues/1133) and re-pointed the quarantine entry at it, so the file says what's actually true again.

This is exactly the failure mode this task exists to catch, just one level down — a tracked-broken row that was half-fixed, with a second, different reason for the row to stay red hiding underneath.

## Stdlib breadth, alongside

| Module | Language | Purpose |
|--------|----------|---------|
| url | Rask | URL parsing (RFC 3986) |
| encoding | Rask | Base64, hex, URL encoding (RFC 4648) |
| csv | Rask | CSV parsing/writing (RFC 4180) |
| unicode | Rask | Properties, normalization, categories |
| terminal | Rask | ANSI colors, terminal detection |
| hash | Rask (or C for HW accel) | SHA-256, MD5, CRC32 |
| tls | C shim + Rask API | TLS/SSL via OpenSSL/mbedTLS |

Unchanged: all seven have specs, none has an implementation file in `stdlib/` yet — confirmed no `stdlib/url.rk` etc. exist. `json.to_value`/`json.from_value` are still `@unimplemented`, waiting on Encode/Decode derivation.

## Post-v1.0

- Platform-specific deps (XT7), multi-target builds (XT8)
- LLVM backend, if the benchmarks ask for it
- Macros / `format!`
- Comptime debugger
- Fuzzing / property-based testing
- Code coverage
- `std.reflect` — comptime reflection
- Inline assembly
- Pointer provenance rules
- `compile_cpp()` build script support
- Auto Rask wrapper generation from cbindgen

## What came off this list since last measure (2026-09-04)

- **The HTTP server's native corruption (#1036)** — was the #1 blocker, now fixed. Verified directly against the issue's own repro, not just by the gate turning green.
- **The sequence protocol's Vec side** — was "unimplemented, leading feature gap." `Sequence<T>` is now the real backbone of `Vec.iter()`; twelve adapters (`filter`, `map`, `take`, `fold`, `enumerate`, `flat_map`, `to_map`, and more) work on both backends. What's left moved down to items #1 and #4 above, and it's a much smaller remaining slice than "the whole protocol."
- **The registered-bug half of the coverage backlog (#1057)** — ten tracked divergences fixed and pruned, one (#904, gradual-inference) correctly reclassified as unbuilt rather than broken.
- **#1002 (union-narrowed error dispatch)** — fixed. See "The agent benchmark" above for the fixture wrinkle this uncovered underneath it.

## New this measure

- **#1093** — the interpreter's `using Multitasking` block doesn't wait for its spawned tasks the way native does, and inside a `test` block the task's output is silently dropped rather than late. Found by the Sept 4–6 harness audit (#1056), not by anything regressing.
- **#1046** — the sequence protocol's remaining edges (see item #4 above): a user-written `Sequence<T>` now works on native but not the interpreter, and five operator-bound terminals need a Rust-driven closure call that doesn't exist yet.
- **#1133** — the agentbench fixture bug uncovered under #1002's fix (see "The agent benchmark" above).
- **The open draft PR #1054** — see its own section above.
