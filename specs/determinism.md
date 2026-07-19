<!-- id: determinism -->
<!-- status: proposed -->
<!-- summary: Every source of nondeterminism enumerated and disposed of — sim mode replays from a seed, production runs at full speed -->
<!-- depends: concurrency/async.md, concurrency/runtime.md, stdlib/collections.md, stdlib/time.md, stdlib/random.md, memory/unsafe.md -->

# Determinism Contract

Determinism is a property of an execution mode, not a tax on the language. Production builds run at full speed with no determinism overhead. **Sim mode** (`rask test --sim`) makes execution a pure function of a seed: same binary, same seed, same recorded inputs → identical execution, including failures.

This is the foundation for commitment 3 of [NORTH_STAR.md](../NORTH_STAR.md): no unreproducible failures. Prior art: FoundationDB's simulation, TigerBeetle's VOPR. Rask's design makes this cheaper than it was for them — I/O is stdlib-mediated (no function coloring means every syscall goes through the runtime), tasks run on a runtime-owned scheduler, and user code cannot observe addresses (no storable references).

## The promise

| Rule | Description |
|------|-------------|
| **D1: Seed determinism** | In sim mode, execution is a deterministic function of (binary, seed, recorded external inputs). Every failure reproduces from its seed |
| **D2: Zero production cost** | Sim mode is a runtime mode, not a compilation dialect. Production builds carry no determinism instrumentation. The only always-on choices are semantic ones (D7) |
| **D3: Same code** | Programs are not written differently for sim. The same binary logic runs in both modes; only the runtime beneath it is swapped |

The governing rule for every disposal below: **production may be nondeterministic only in ways sim can explore (interleavings, timings, faults, orders), never in ways sim can't express (divergent semantics).** A behavior difference between the two modes that sim can't reach through its seed space is a hole in the contract.

## Sources of nondeterminism and their disposal

Every source is listed here. A source not listed is a spec bug.

| Rule | Source | Disposal |
|------|--------|----------|
| **D4: Task scheduling** | Green task interleaving | Sim runs the scheduler single-threaded; task switch order drawn from the seed. Adversarial schedules (pathological interleavings) are a seed away, not a fluke |
| **D5: Time** | `Instant`, `SystemTime`, timers, sleep | Virtual clock in sim, advanced by the scheduler. Already runtime-mediated |
| **D6: Randomness** | `random` module | All generators derive from the sim seed |
| **D7: Map iteration order** | Hash order | Seeded hash order, seed owned by the mode: production seeds per process (HashDoS mitigation; accidental order-dependence can't take root), sim derives the seed from the sim seed (replay-exact, and seed search varies the order so order-dependent code fails under test). No mode guarantees an observable order — code that needs one sorts explicitly. Insertion-ordered Map was considered and rejected: order-preserving removal costs O(n) or compaction pauses, and the index layer taxes every lookup |
| **D8: Pool/handle allocation** | Slot and generation assignment | Deterministic function of the operation sequence. Handles are indices, never addresses |
| **D9: I/O** | Network, disk, file system | Sim substitutes simulated implementations behind the same stdlib surface. Fault injection (partitions, slow disks, torn writes) is driven by the seed |
| **D10: External inputs** | Env, args, stdin, wall-clock start | Fixed or recorded as part of the sim scenario |
| **D11: Addresses** | Pointer values leaking into logic | Impossible by construction outside `unsafe` — no storable references, no address-of. Nothing to virtualize |
| **D12: Floats** | FP evaluation | Deterministic within one binary on one platform (fixed evaluation, no contraction variance between runs). Cross-platform bit-exactness is **out of scope** — that's Raido's domain (32.32 fixed point) |
| **D13: OS threads** | `Thread.spawn`, true parallelism | Outside the contract. Sim mode rejects raw thread spawns; `ThreadPool` work is scheduled deterministically like tasks. Production parallelism is nondeterministic by nature — the promise is that sim explores the interleavings, not that production replays them |
| **D14: FFI / unsafe** | C calls, raw pointers | Outside the contract. The capability metadata (`struct.build`) already tracks which code reaches `ffi`/`unsafe`; sim mode reports or rejects it. Recorded shims are future work |

## What sim mode is for

- **Reproducible tests:** a failing test prints its seed; the seed replays the exact execution, including task interleaving and injected faults.
- **Interleaving search:** run the same test across thousands of seeds to explore schedules no wall-clock test would hit.
- **Fault injection:** network partitions, disk errors, and slow peers as seed-driven scenario, not hand-built mocks.

## Open questions

- **Ordered map type:** with D7 guaranteeing no order anywhere, programs that genuinely need insertion or sorted order have no dedicated type — sort-at-use covers most cases; decide whether an explicit `OrderedMap` earns a place in the stdlib when real programs ask for it.
- **Record-replay in production:** capturing real inputs for after-the-fact replay is a heavier feature than sim and is not part of this contract yet.
- **Panic interaction:** replaying a run that panics requires the panic path itself to be deterministic (message, unwind behavior). Depends on the panic-semantics spec (tracked separately).
