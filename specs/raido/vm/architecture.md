<!-- id: raido.vm -->
<!-- status: proposed -->
<!-- summary: Deterministic register-based VM with fixed-point math, serializable state, configurable arena -->
<!-- depends: raido/language/types.md -->

# VM Architecture

Register-based bytecode VM. Deterministic. Serializable. Sandboxed.

## Core Properties

- **Register-based.** Fewer dispatched instructions than stack-based (~30% fewer in Lua 5's switch). Each call frame has a fixed-size register window. Instructions encode register operands directly — no push/pop overhead.
- **Deterministic.** Fixed-point arithmetic, seedable PRNG, insertion-ordered maps. No platform-dependent behavior.
- **Serializable.** Entire state → bytes → restore. No pointers, only arena offsets. Register windows serialize the same as a stack — just an array of values per frame.
- **Send, not Sync.** Movable between threads, not shareable.
- **`@resource`** in Rask — must be closed.

### Why Register-Based

I considered stack-based (simpler compiler, simpler instruction encoding) but register-based wins on performance and the serialization argument doesn't hold:

1. **Fewer instructions dispatched.** `a + b * c` is `MUL r2, r1, r0; ADD r3, r2, r0` (2 ops) vs stack's `PUSH a; PUSH b; PUSH c; MUL; ADD` (5 ops). For a bytecode interpreter where dispatch is the bottleneck, this matters directly.
2. **Less memory traffic.** Operands addressed by register index, not stack pointer manipulation.
3. **Serialization is equivalent.** A register window is a fixed-size value array per call frame. Same wire format complexity as a stack.
4. **Compiler cost is one-time.** Linear scan register allocation handles Raido's complexity. No SSA needed.

Lua 5's switch from stack-based to register-based is the canonical precedent — same design context (embeddable, serializable, interpreter-only).

## Value Representation

All values are 8 bytes (tagged). Arena-allocated types (strings, arrays, maps, closures) store an arena offset in the value slot. No pointers — only offsets into the contiguous arena.

## Determinism

All `number` arithmetic is 32.32 fixed-point (integer math). No FPU. Bitwise-identical on all platforms.

Add/sub = single i64 op. Mul/div = 128-bit intermediate. Fast.

Determinism enables: lockstep networking, replay, migration, reproducible evaluation, audit trails.

## Arena

All VM allocations (arrays, maps, strings, closures, upvalues) come from a contiguous arena.

- **Bump allocator.** O(1).
- **Fixed size.** Exceeding raises a runtime error. No auto-grow — hides allocation cost.
- **No GC.** No mark/sweep, no pauses.
- **`reset()`.** Clears everything — globals, coroutines, all state. Default strategy for rule engines, workflows, and between independent evaluations.
- **`frame_end()` (opt-in).** Resets the arena's frame region, keeping persistent state. For game-loop embedders that call scripts every frame. Non-loop embedders don't need this.

## Resource Limits

Three runaway vectors, three limits:

### Fuel (instruction budget)

Fuel-based execution control. Every instruction costs 1 fuel. Reaching zero = runtime error.

```rask
const vm = raido.Vm.new(raido.Config {
    arena_size: 256.kilobytes(),
    initial_fuel: 100_000,
    max_call_depth: 256,
})
```

The host can inspect and adjust fuel between calls or from host functions:

```rask
const remaining = vm.fuel()       // check remaining
vm.add_fuel(50_000)               // top up (game loop: add fuel each frame)
vm.set_fuel(100_000)              // reset to specific amount
```

**Determinism constraint.** Fuel is part of VM state and serialized. For deterministic replay, hosts must add fuel at the same points with the same amounts. Fuel operations are not implicit — the host controls them explicitly. If two hosts diverge on fuel, execution diverges. This is the host's responsibility, same as providing identical host function results.

### Call Depth

Maximum call stack depth. Default 256. Every `CALL` / `RESUME` increments, `RETURN` / `YIELD` decrements. Exceeding = runtime error.

Prevents stack overflow from unbounded recursion. Cheap — one comparison per call instruction.

`TAIL_CALL` doesn't increment (it reuses the current frame), so tail-recursive scripts aren't depth-limited.

### Arena Size

Fixed. Exceeding = runtime error. Already covered in the Arena section above.

## Serialization

`vm.serialize()` → bytes. `Vm.deserialize(bytes)` → restored VM. Format is versioned — version header from day one so format changes don't break existing snapshots.

Captures: register windows, call frames (including call depth), globals, coroutines (suspended register windows + PC), arena contents (including upvalues), PRNG state, fuel remaining.
Does not capture: host function closures (by name), host bindings (re-bound), bytecode (re-loaded).

Coroutine state is ~200-500 bytes per suspended coroutine in the arena.

## Closures and Upvalues

Closures capture variables from enclosing scopes. Upvalues live in the arena — closures hold arena offsets to them. This makes closures trivially serializable: a closure is a bytecode prototype index + an array of arena offsets.

When a local variable is captured:
1. The variable is "closed over" — moved from the register window into the arena.
2. The closure stores the arena offset.
3. Multiple closures capturing the same variable share the same arena offset (they see each other's mutations).

This is Lua 5's upvalue model adapted for arena allocation. No heap cells, no GC — upvalues are just arena slots. Serialization captures them as part of the arena contents.

## Instruction Set (sketch)

Instructions are 32 bits. Format: `op(8) + operands(24)`. Register operands are 8-bit indices (256 registers per frame — more than enough).

| Category | Opcodes |
|----------|---------|
| Load | `LOAD_NIL rA`, `LOAD_TRUE rA`, `LOAD_FALSE rA`, `LOAD_INT rA imm16`, `LOAD_CONST rA idx` |
| Move | `MOVE rA rB` |
| Globals | `GET_GLOBAL rA idx`, `SET_GLOBAL idx rA` |
| Upvalues | `GET_UPVALUE rA idx`, `SET_UPVALUE idx rA` |
| Arithmetic | `ADD rA rB rC`, `SUB rA rB rC`, `MUL rA rB rC`, `DIV rA rB rC`, `MOD rA rB rC`, `NEG rA rB` |
| Comparison | `EQ rA rB rC`, `NE rA rB rC`, `LT rA rB rC`, `LE rA rB rC` |
| Logic | `NOT rA rB` |
| String | `LEN rA rB`, `INTERPOLATE rA rB count` |
| Collection | `NEW_ARRAY rA count`, `NEW_MAP rA count`, `GET_INDEX rA rB rC`, `SET_INDEX rA rB rC`, `PUSH_ELEM rA rB` |
| Host ref | `GET_REF_FIELD rA rB field_idx`, `SET_REF_FIELD rA field_idx rB` |
| Function | `CALL rA arg_start arg_count`, `RETURN rA`, `TAIL_CALL rA arg_start arg_count`, `CLOSURE rA proto_idx` |
| Jump | `JMP offset`, `JMP_IF rA offset`, `JMP_IF_NOT rA offset` |
| Loop | `FOR_ITER rA rB offset`, `FOR_RANGE rA rB rC offset` |
| Coroutine | `COROUTINE_NEW rA rB`, `YIELD rA`, `RESUME rA rB` |
| Error | `TRY rA`, `TRY_ELSE rA rB` |
