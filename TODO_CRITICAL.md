# Rask Critical Improvements TODO

Based on critical review of examples and specs (2026-02-05)

## 1. Make Examples Actually Run ✓

**Priority: CRITICAL** — DONE

- [x] fs, io, cli modules already implemented in interpreter
- [x] Added interpreter features: field assignment, index assignment, closures, cast, string interpolation, Result/Option methods, struct clone
- [x] file_copy.rask runs end-to-end
- [x] grep_clone.rask runs end-to-end (with -i, -n, -c, -v flags)

**Remaining:** cli_calculator.rask (needs test block execution), http_api_server.rask (needs net module)

---

## 2. Fix Comptime Implementation Gaps

**Priority: HIGH**

- [ ] Mutable arrays at comptime
  - [ ] Current issue: `const table = [0u8; 256]` then `table[i] = crc`
  - [ ] Spec says comptime is restricted - does it allow mutation?
  - [ ] Either: implement it, or fix sensor_processor.rask to use immutable approach

- [ ] Document comptime limitations explicitly
  - [ ] Update `specs/control/comptime.md` with what CAN'T be done
  - [ ] Add example that FAILS compilation (hits iteration limit, etc.)
  - [ ] Show error message user sees

- [ ] Implement `.freeze()` for comptime collections
  - [ ] `Vec` at comptime → `.freeze()` → const array
  - [ ] Prove it works with example

- [ ] **Test:** Run sensor_processor.rask's `build_crc8_table()` at comptime
  - [ ] Verify CRC8_TABLE is in binary's data section
  - [ ] Verify no runtime execution of build function

---

## 3. Measure and Document Pool+Handle Overhead

**Priority: HIGH**

- [ ] Implement pool auto-resolution in interpreter
  - [ ] Thread-local registry: `HashMap<u32, &Pool>`
  - [ ] Handle structure: `{ pool_id: u32, index: u32, generation: u32 }`
  - [ ] Index operation: `pool[handle]` → registry lookup + generation check

- [ ] **Benchmark:** Pool+Handle vs direct indexing
  - [ ] Test: 10,000 entity updates (game_loop.rask pattern)
  - [ ] Measure: registry lookup cost, cache misses, memory bandwidth
  - [ ] Document: actual overhead in ns/operation

- [ ] Update docs with honest performance section
  - [ ] Add to `specs/memory/pools.md`: "Performance Characteristics"
  - [ ] State overhead: ~4-5 pointer chases vs 1-2 in Rust
  - [ ] When to use: graph structures where ergonomics > raw speed
  - [ ] When NOT to use: tight loops, cache-sensitive code

- [ ] Optimization ideas (if overhead is bad)
  - [ ] Cache pool pointer after first lookup
  - [ ] Inline generation check
  - [ ] SIMD batch validation

---

## 4. Fix Clone Spam and Ergonomics

**Priority: MEDIUM**

- [ ] Audit examples for unnecessary `.clone()`
  - [ ] grep_clone.rask line 87: `path.clone()` appears 3× in 2 lines
  - [ ] Find pattern: is this inherent or fixable?

- [ ] Compare to Go for each example
  - [ ] **Run litmus test:** Is Rask longer/noisier for core loops?
  - [ ] http_api_server.rask `list_users`: 9 lines vs Go's 7 lines
  - [ ] Identify where Rask is worse, fix syntax/design

- [ ] Possible solutions:
  - [ ] Auto-clone for String in certain contexts? (dangerous)
  - [ ] Better error messages: suggest `.as_ref()` instead of `.clone()`?
  - [ ] String refs that ARE storable for read-only cases?

- [ ] **Decision point:** Is clone spam acceptable tradeoff for no lifetimes?
  - [ ] If yes: document it honestly in CORE_DESIGN.md
  - [ ] If no: rework borrowing model

---

## 5. Fix Syntax Inconsistencies in Examples — Partially Done

**Priority: MEDIUM**

- [x] `String` → `string` (lowercase) in all examples
  - [x] cli_calculator.rask, simple_test.rask, http_api_server.rask, text_editor.rask, file_copy.rask, grep_clone.rask
  - [x] CORE_DESIGN.md built-in types list updated
- [x] `let` → `const` where bindings are not reassigned

- [ ] Document `GameState.{entities}` projection syntax
  - [ ] game_loop.rask:110 uses this, but spec doesn't define it
  - [ ] Either: add to spec, or remove from example

- [ ] Clarify `f32x8` type
  - [ ] sensor_processor.rask:402 shows `let acc: f32x8 = [0.0; 8]`
  - [ ] Is this array syntax or SIMD type?

- [ ] Run syntax checker on ALL examples for remaining inconsistencies

---

## 6. Implement and Validate Concurrency Claims

**Priority: HIGH**

- [ ] Fix game_loop.rask threading issue
  - [ ] Line 218-225: `entities` shared across threads without `Shared<>`
  - [ ] Either: add `Shared<Pool>`, or explain auto-sync
  - [ ] Update spec if compiler does implicit sync

- [ ] Implement "no function coloring" runtime
  - [ ] Async I/O backed by event loop (epoll/kqueue)
  - [ ] Green task scheduler with work-stealing
  - [ ] Blocking calls auto-pause task, not thread
  - [ ] **This is 3,000+ lines of complex code**

- [ ] Validate http_api_server.rask concurrency
  - [ ] Run with 100 concurrent connections
  - [ ] Measure: context switch overhead, latency
  - [ ] Compare to Go goroutines (apples-to-apples)

- [ ] Document runtime architecture
  - [ ] New spec: `specs/concurrency/runtime.md`
  - [ ] Explain: task scheduler, executor, I/O integration
  - [ ] Be honest about overhead vs OS threads

---

## 7. Add Negative Examples (What Doesn't Compile) ✓

**Priority: MEDIUM** — DONE

- [x] `examples/compile_errors/` directory with 4 examples:
  - [x] `borrow_stored.rask` — storing references in structs fails
  - [x] `comptime_loop.rask` — comptime iteration limit hit
  - [x] `error_mismatch.rask` — incompatible error types with `?`
  - [x] `resource_leak.rask` — forgetting to consume `@resource` type
- [x] Each example shows: the error, why it fails, expected error message, and fix
- [x] All use correct Rask syntax

---

## 8. Implement SIMD Support (or Remove Claims)

**Priority: LOW (but decide)**

- [ ] **Option A: Implement SIMD**
  - [ ] Map `f32x8`, `i32x4`, etc. to platform intrinsics
  - [ ] Auto-vectorization for simple loops
  - [ ] Alignment handling (`@align` attribute?)
  - [ ] Fallback for non-SIMD targets
  - [ ] Test sensor_processor.rask SIMD sum

- [ ] **Option B: Remove from examples**
  - [ ] Replace sensor_processor.rask SIMD with scalar code
  - [ ] Move SIMD to "future work"
  - [ ] Don't claim it works until it does

- [ ] **Decision criteria:** Is SIMD core to Rask's value prop?
  - [ ] If yes (embedded/HPC): implement
  - [ ] If no: defer, focus on core language

---

## 9. Complete Stdlib Modules (Incremental)

**Priority: MEDIUM (ongoing)**

Phase 1: Make examples run ✓
- [x] `fs` - file I/O (file_copy.rask, grep_clone.rask)
- [x] `io` - stdin/stdout (cli_calculator.rask)
- [x] `cli` - args (grep_clone.rask)
- [ ] `time` - timing (game_loop.rask, sensor_processor.rask)

Phase 2: Advanced examples
- [ ] `net` - TCP listen/accept (http_api_server.rask)
- [ ] `json` - encode/decode (http_api_server.rask)
- [ ] `regex` - pattern matching (grep_clone.rask)

Phase 3: Nice to have
- [ ] `http` - higher-level HTTP (beyond raw TCP)
- [ ] `csv`, `encoding`, etc. from specs/stdlib/README.md

**Strategy:** Implement just enough to run each example, iterate

---

## 10. Performance Validation

**Priority: MEDIUM**

- [ ] Benchmark suite based on examples
  - [ ] file_copy.rask vs `cp` / Go / Rust
  - [ ] cli_calculator.rask parser vs hand-written C
  - [ ] game_loop.rask entity updates vs Rust ECS

- [ ] Measure hidden costs
  - [ ] Pool handle resolution overhead
  - [ ] Clone cost from no-references design
  - [ ] Task spawn/context-switch vs Go/Rust

- [ ] Document results honestly
  - [ ] Add `PERFORMANCE.md` to root
  - [ ] State where Rask is slower and WHY
  - [ ] State where ergonomics justify cost
  - [ ] State where it's unacceptable

---

## 11. Documentation Honesty Pass

**Priority: HIGH**

- [ ] Update CLAUDE.md
  - [ ] Remove claim "80%+ coverage" until proven
  - [ ] Add "Status: Examples validate syntax, not execution"
  - [ ] Honest about what's implemented vs designed

- [x] Update CORE_DESIGN.md
  - [x] "Design Tradeoffs" section already added (clone ergonomics, pool overhead, no storable references, comptime limits, when to use Rask)
  - [x] "Limitations" section already present
  - [x] Fixed `String` → `string` in built-in types and default arguments examples

- [ ] Add IMPLEMENTATION_STATUS.md
  - [ ] Table: Feature | Spec | Interpreter | Tested | Example
  - [ ] Track progress transparently
  - [ ] Link to working examples

- [ ] Specs: add "Limitations" sections
  - [ ] Every spec should list what WON'T work
  - [ ] Comptime: what's restricted
  - [ ] Pools: when overhead is bad
  - [ ] Borrowing: what you can't store

---

## 12. Fix Specific Example Bugs

- [ ] **http_api_server.rask**
  - [ ] Line 173-174: how does `db.clone()` work if db is Shared?
  - [ ] Shared clone increments refcount - is this in spec?
  - [ ] Does `Shared` use Arc internally? Document.

- [ ] **game_loop.rask**
  - [ ] Line 274: `parallel_movement_system(state, ...)` - state is not Shared
  - [ ] How is `state.entities` accessed across threads?
  - [ ] Fix or explain with projection borrowing

- [ ] **sensor_processor.rask**
  - [ ] Line 280-283: `buffer` passed to thread without `Shared`
  - [ ] Is SpscRingBuffer implicitly sync?
  - [ ] Document ownership transfer to thread

---

## Success Metrics

After completing this TODO, Rask should:

1. ✅ **Run 3+ examples end-to-end** (file_copy, cli_calculator, simple_grep)
2. ✅ **Pass all inline tests** in cli_calculator.rask
3. ✅ **Have honest performance docs** showing real overhead
4. ✅ **Show negative examples** of what doesn't compile
5. ✅ **Pass litmus test** - no longer than Go for core loops
6. ✅ **Transparent status** - clear what's working vs aspirational

---

## Priority Order

**Week 1: Make it real**
1. Implement minimal fs/io/cli modules
2. Run file_copy.rask and cli_calculator.rask
3. Pass inline tests

**Week 2: Measure reality**
4. Implement pool auto-resolution
5. Benchmark overhead
6. Document performance honestly

**Week 3: Fix examples**
7. Fix syntax inconsistencies
8. Add negative examples
9. Compare to Go line-by-line

**Week 4: Concurrency**
10. Implement basic runtime
11. Run http_api_server.rask
12. Validate "no coloring" claim

**Ongoing:**
- Expand stdlib incrementally
- Update docs with learnings
- Be honest about tradeoffs

---

## Open Questions to Resolve

1. **Is clone spam acceptable?** Or does borrowing model need rework?
2. **Is SIMD core to value prop?** Or can it be deferred?
3. **How much pool overhead is OK?** 10% slower? 50%? 2x?
4. **Should examples be simpler?** Or prove advanced features?
5. **What's the REAL target user?** Game devs? Embedded? Web services?

Answer these to prioritize the TODO.
