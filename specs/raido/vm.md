<!-- id: raido.vm -->
<!-- status: proposed -->
<!-- summary: Deterministic stack-based VM with fixed-point math, serializable state, configurable arena -->
<!-- depends: raido/values.md -->

# VM Architecture

Stack-based bytecode VM. Deterministic. Serializable. Sandboxed.

## Core Properties

- **Stack-based.** Simple to implement, simple to serialize (stack = value array).
- **Deterministic.** Fixed-point arithmetic, seedable PRNG, insertion-ordered maps. No platform-dependent behavior.
- **Serializable.** Entire state → bytes → restore. No pointers, only arena offsets.
- **Send, not Sync.** Movable between threads, not shareable.
- **`@resource`** in Rask — must be closed.

## Determinism

All `number` arithmetic is 32.32 fixed-point (integer math). No FPU. Bitwise-identical on all platforms.

Add/sub = single i64 op. Mul/div = 128-bit intermediate. Fast.

Determinism enables: lockstep networking, replay, migration, reproducible evaluation, audit trails.

## Arena

All VM allocations (arrays, maps, strings, closures) come from a contiguous arena.

- **Bump allocator.** O(1).
- **Fixed size.** Exceeding raises a runtime error.
- **No GC.** No mark/sweep, no pauses.
- **`frame_end()` (optional).** Resets the arena's frame region, keeping persistent state. For frame-loop embedders (games). Non-loop embedders (rule engines, workflows) can skip this and use `reset()` between evaluations.
- **`reset()`.** Clears everything — globals, coroutines, all state. For hot reload or between independent evaluations.

## Instruction Limits

Per-call instruction budget. Every instruction decrements. Exceeding = runtime error. Prevents runaway scripts.

## Serialization

`vm.serialize()` → bytes. `Vm.deserialize(bytes)` → restored VM.

Captures: value stack, call frames, globals, coroutines, arena contents, PRNG state, instruction counter.
Does not capture: host function closures (by name), host bindings (re-bound), bytecode (re-loaded).

## Instruction Set (sketch)

| Category | Opcodes |
|----------|---------|
| Stack | `PUSH_NIL`, `PUSH_TRUE`, `PUSH_FALSE`, `PUSH_INT`, `PUSH_NUM`, `PUSH_CONST`, `POP`, `DUP` |
| Variables | `GET_LOCAL`, `SET_LOCAL`, `GET_GLOBAL`, `SET_GLOBAL`, `GET_UPVALUE`, `SET_UPVALUE` |
| Arithmetic | `ADD`, `SUB`, `MUL`, `DIV`, `MOD`, `NEG` (fixed-point for numbers) |
| Comparison | `EQ`, `NE`, `LT`, `LE`, `GT`, `GE` |
| Logic | `NOT`, `AND`, `OR` |
| String | `LEN`, `INTERPOLATE` |
| Collection | `NEW_ARRAY`, `NEW_MAP`, `GET_INDEX`, `SET_INDEX`, `GET_FIELD`, `SET_FIELD`, `PUSH_ELEM` |
| Host ref | `GET_REF_FIELD`, `SET_REF_FIELD` |
| Function | `CALL`, `RETURN`, `TAIL_CALL`, `CLOSURE` |
| Jump | `JMP`, `JMP_IF`, `JMP_IF_NOT` |
| Loop | `FOR_ITER`, `FOR_RANGE` |
| Coroutine | `COROUTINE_NEW`, `YIELD`, `RESUME` |
| Error | `TRY`, `TRY_ELSE` |
