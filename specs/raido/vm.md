<!-- id: raido.vm -->
<!-- status: proposed -->
<!-- summary: Deterministic stack-based VM with softfloat, serializable state, frame-wrapping arena -->
<!-- depends: raido/values.md -->

# VM Architecture

Stack-based bytecode VM. Deterministic execution. Serializable state. Arena allocation with per-frame wrapping.

## Core Properties

- **Stack-based.** Simpler to implement, simpler to serialize (stack is just a value array).
- **Deterministic.** Softfloat arithmetic — bitwise-identical results across platforms.
- **Serializable.** Entire VM state (stack, globals, call frames, coroutine positions) can be dumped to bytes and restored.
- **Send, not Sync.** A VM can move between threads but not be shared.
- **`@resource`** in Rask — must be closed via `vm.close()`.

## Determinism

All floating-point arithmetic is software-emulated (softfloat). No hardware FPU instructions. This guarantees bitwise-identical results on x86, ARM, RISC-V, whatever the server runs on.

Cost: ~10x slower float ops. For entity scripts doing `h.x + h.vx * dt` a few hundred times per frame, this is negligible. If a script needs fast math, do it on the host side in Rask (which uses hardware floats) and pass results in.

Determinism enables:
- **Lockstep networking.** Two servers running the same script with the same inputs produce the same outputs.
- **Replay.** Record inputs, replay deterministically.
- **Migration.** Serialize VM, send to another node, resume.

`math.random()` uses a seedable PRNG that's part of the VM state (and therefore serializable/deterministic).

## Serializable State

The entire VM can be serialized to a byte buffer and restored:

```rask
// Snapshot
const snapshot = vm.serialize()

// Restore
const vm2 = raido.Vm.deserialize(snapshot)

// Or persist to disk
try fs.write("save.bin", vm.serialize())
```

What gets serialized:
- Value stack
- Call frame stack (PCs, stack pointers)
- Global table
- All coroutine states (suspended stack, PC)
- Arena contents (arrays, maps, strings, closures)
- PRNG state
- Instruction counter

What does NOT serialize:
- Host function bindings (referenced by name, re-registered on restore)
- Pool references (re-provided via `exec_with` after restore)
- Bytecode (re-compiled or loaded separately)

Host functions are stored as string names in the serialized state. On restore, the host must re-register functions with the same names. Missing a function is an error on first call, not on restore.

## Arena with Frame Wrapping

The arena wraps at frame boundaries. Previous frame's temporaries get overwritten.

- **Per-frame bump allocator.** Allocations within a frame bump a pointer forward.
- **Frame reset.** At `vm.frame_end()`, the arena pointer resets. Previous allocations gone.
- **Persistent slots.** Globals, coroutine state, and explicitly-held values live in a separate persistent region that doesn't wrap.
- **Fixed size.** Arena + persistent region have a combined limit. Exceeding raises a runtime error.

```rask
// Game loop
while running {
    try vm.exec_with(|scope| {
        scope.provide_pool("enemies", enemies)
        scope.call("on_update", [raido.Value.number(dt)])
    })
    vm.frame_end()  // arena wraps — frame temporaries freed
}
```

This means scripts can't hold references to frame-local data across yields... unless the data is in a global, a coroutine local, or a pool field. Temporaries (intermediate concat strings, temporary arrays) vanish at frame end.

**Why not explicit `vm.reset()`?** Reset destroys everything — globals, coroutines, all state. Frame wrapping preserves persistent state (globals, coroutines, closures assigned to globals) while reclaiming frame temporaries. Reset is still available for hot reload.

## Instruction Limits

Each `vm.call()` has an instruction budget. Every instruction decrements it. Exceeding raises a runtime error. Budget is part of the serialized state.

## Instruction Set (sketch)

Stack-based — operands come from the stack, results pushed onto the stack.

| Category | Opcodes |
|----------|---------|
| Stack | `PUSH_NIL`, `PUSH_TRUE`, `PUSH_FALSE`, `PUSH_INT`, `PUSH_NUM`, `PUSH_CONST`, `POP`, `DUP` |
| Variables | `GET_LOCAL`, `SET_LOCAL`, `GET_GLOBAL`, `SET_GLOBAL`, `GET_UPVALUE`, `SET_UPVALUE` |
| Arithmetic | `ADD`, `SUB`, `MUL`, `DIV`, `MOD`, `NEG` (all softfloat for number operands) |
| Comparison | `EQ`, `NE`, `LT`, `LE`, `GT`, `GE` |
| Logic | `NOT`, `AND`, `OR` |
| String | `CONCAT`, `LEN`, `INTERPOLATE` |
| Collection | `NEW_ARRAY`, `NEW_MAP`, `GET_INDEX`, `SET_INDEX`, `GET_FIELD`, `SET_FIELD`, `PUSH_ELEM` |
| Handle | `GET_HANDLE_FIELD`, `SET_HANDLE_FIELD` |
| Function | `CALL`, `RETURN`, `TAIL_CALL`, `CLOSURE` |
| Jump | `JMP`, `JMP_IF`, `JMP_IF_NOT` |
| Loop | `FOR_ITER`, `FOR_RANGE` |
| Coroutine | `YIELD`, `RESUME` |

## Hot Reload

`vm.reset()` destroys all state (globals, coroutines, arena), then `vm.exec(new_chunk)` loads fresh code. Pool data in Rask is untouched.
