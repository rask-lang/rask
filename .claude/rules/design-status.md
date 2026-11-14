---
paths:
  - "specs/**"
  - "ROADMAP.md"
  - "TODO.md"
---

# Design status

These decisions are settled. Don't re-derive them, and don't open design debates
inside implementation tasks. If one genuinely seems wrong, flag it once with a
concrete reason.

## Decided

| Area | Decision | Spec |
|------|----------|------|
| Ownership | Single owner, move semantics, 16-byte copy threshold | [memory/](../../specs/memory/) |
| Borrowing | Block-scoped (fixed sources), inline + `with` (growable sources) | [borrowing.md](../../specs/memory/borrowing.md) |
| Closures | Capture mode is inferred — a closure that outlives its frame carries what it captured, one that stays points at it. No `own` keyword | [closures.md](../../specs/memory/closures.md) |
| Linearity | Consume exactly once (L1–L6) — shared by `@resource` and `Heap<T>` | [linear.md](../../specs/memory/linear.md) |
| Scoped access | A value held elsewhere, reached through `with` — `Shared<T, S>`, Rack+Link, Heap. `Cell` and `Mutex<T>` are gone; `Local`/`Readers`/`Mutex` are strategies on `Shared` | [shared-rack-heap.md](../../specs/memory/shared-rack-heap.md) |
| Collections | Vec, Map, Rack+Link for graphs | [collections.md](../../specs/stdlib/collections.md), [racks.md](../../specs/memory/racks.md) |
| Resource types | `@resource` annotation for I/O handles, transactions; `ensure` cleanup | [resource-types.md](../../specs/memory/resource-types.md) |
| Types | Primitives, structs, enums, generics, interfaces, unions, tuples, nominal types, type aliases | [types/](../../specs/types/) |
| Operators | Resolved on the ordered pair of operand types, against the interfaces in `stdlib/ops.rk` | [operator-resolution.md](../../specs/types/operator-resolution.md) |
| Errors | `T or E` result, `try` propagation, `T?` optionals, `todo()`/`unreachable()` | [error-types.md](../../specs/types/error-types.md) |
| Panics | Task-kill + unwind, ensures run, locks release without poisoning, opt-in `staged()` | [panics.md](../../specs/control/panics.md) |
| Concurrency | `spawn { }` task blocks (run once, may consume captures)/join/detach, channels, no function coloring. A task joined where it was spawned borrows from the frame (fork-join without a new construct) | [concurrency/](../../specs/concurrency/) |
| Comptime | Compile-time execution | [comptime.md](../../specs/control/comptime.md) |
| C interop | Unsafe blocks, raw pointers | [unsafe.md](../../specs/memory/unsafe.md) |
| Rust interop | compile_rust() in build scripts, C ABI, cbindgen | [build.md](../../specs/structure/build.md) |
| Manifest | Plain TOML (`rask.toml`) checked against a versioned schema with an append-only change history; `build.rk` is script-only. The toolchain runs the loader, written in Rask, on its interpreter | [manifest.md](../../specs/structure/manifest.md) |
| Encoding | `comptime for` + field access, auto-derived Encode/Decode, field annotations | [encoding.md](../../specs/stdlib/encoding.md) |
| Networking | TCP, UDP, DNS resolution | [net.md](../../specs/stdlib/net.md) |
| HTTP | Client + server, linear Responder, HttpClient | [http.md](../../specs/stdlib/http.md) |
| Time | Duration, Instant, SystemTime, Duration scaling | [time.md](../../specs/stdlib/time.md) |
| OS | Env, args, subprocess spawning, signal handling | [os.md](../../specs/stdlib/os.md) |
| Compiler architecture | IR layers, SSA pipeline, analysis framework, pass manager, CTFE, debug info | [architecture.md](../../specs/compiler/architecture.md) |
| Code generation | MIR-based pipeline, Cranelift backend, runtime library | [codegen.md](../../specs/compiler/codegen.md) |
| Projects | Raido, Leden, Allgard, GDL, Midgard, Apeiron. Decisions in `.claude/rules/projects.md` | [projects/](../../projects/) |

## Open

| Area | Status |
|------|--------|
| Build system | Working, including cross-package symbol export |
| Macros/attributes | No macro system (rejected). User annotations built — `annotation @name { … }`, attachment checking, `has<A>()`/`get<A>().field` at comptime on both backends, across packages ([annotations.md](../../specs/types/annotations.md), spec still proposed). Call information is spec only ([call-info.md](../../specs/control/call-info.md)); gap analysis in [macro-story.md](../../specs/analysis/macro-story.md) |
| Frontend caching | LSP works, incremental check caching not yet implemented |
| Parallel compilation | Semantic hashing done, rayon parallelism not yet implemented |
| Phase B fiber implementation | Decided: stackful fibers with mmap'd virtual stacks, pluggable reactor (io_uring/epoll/kqueue/IOCP), signal-based preemption. Still to prototype: `fiber_switch` assembly, safe-point instrumentation, reactor backends |

See [TODO.md](../../TODO.md) for full list.

## Goal and principles

Systems language where **safety is invisible**. Eliminate abstraction tax, cover
80%+ of real use cases. Non-negotiable: feel simple, not safe. Safety is a
property, not an experience.

Unifying thread: **safety through visibility.** Safety mechanisms are visible in
source (explicit `ensure`, `mutate`, `take`, scoped `with`) rather than hidden in
destructors, lifetime annotations, or effect types.

1. **Transparency of cost.** Major costs visible (allocations, locks, I/O); small
   ones (bounds checks) can be implicit.
2. **Mechanical safety.** Data races, null derefs, dangling pointers and, in safe
   code, leaks impossible by construction; deleting a rack node nulls every link
   to it. `tests/leak_gate.sh` holds the implementation to the leak claim and
   `tests/known_leaks.txt` lists where it doesn't yet.
3. **Practical coverage.** Web services, CLI, data processing, embedded.
4. **Ergonomic simplicity.** If Rask needs 3+ lines where Go needs 1, question
   the design.
5. **Information without enforcement.** Effects, captures and modes are metadata
   for tooling, not type-system constraints. No function colouring.

Full set: [CORE_DESIGN.md](../../specs/CORE_DESIGN.md). Scoring:
[METRICS.md](../../specs/METRICS.md). Spec index: [specs/README.md](../../specs/README.md);
IDs and citation format (`spec-id/rule-id`, e.g. `mem.ownership/O1`):
[CONVENTIONS.md](../../specs/CONVENTIONS.md).

Programs that must work naturally: an HTTP JSON API server, a grep clone, a text
editor with undo, a game loop with entities, an embedded sensor processor. If
Rask is longer or noisier than Go for core loops, fix the design.
