<!-- id: raido.vm -->
<!-- status: proposed -->
<!-- summary: Register-based VM with arena allocation, memory limits, and instruction budgets -->
<!-- depends: raido/values.md -->

# VM Architecture

Register-based bytecode VM. Arena allocation (no GC). Memory-limited, instruction-budgeted, single-threaded.

## Core Properties

- **Register-based, 32-bit instructions.** Fewer instructions per operation than stack VMs.
- **Send, not Sync.** A VM can move between threads but not be shared.
- **`@resource`** in Rask — must be closed via `vm.close()`.
- **~1 KB base overhead.** A server can run hundreds of VMs.

## Arena Allocation

All VM allocations (arrays, maps, strings, closures, bytecode) come from one contiguous arena.

- **Bump allocator.** O(1) allocation.
- **Bulk reset.** `vm.reset()` frees everything at once. Pointer resets to start.
- **Fixed size.** Set at creation (default 256 KB). Exceeding it raises a runtime error.
- **No GC.** No mark phase, no sweep, no write barriers, no pauses.

Persistent state lives in Rask pools, not in the VM. The arena holds temporaries — intermediate values, string concatenations, closure captures. Reset clears script state; pool data survives.

## Instruction Limits

Each `vm.call()` / `vm.exec()` has an instruction budget. Every instruction decrements it. Exceeding it raises a runtime error. Prevents runaway scripts from freezing the server.

## Hot Reload

`vm.reset()` then `vm.exec(new_chunk)`. Clean slate — old code gone, new code loaded. Entity data in pools untouched.

```rask
const new_chunk = try vm.compile("goblin_ai.raido", new_source)
vm.reset()
try vm.exec(new_chunk)
```

## Instruction Set (sketch)

| Category | Opcodes |
|----------|---------|
| Load | `LOADNIL`, `LOADBOOL`, `LOADINT`, `LOADNUM`, `LOADK`, `LOADGLOBAL`, `STOREGLOBAL` |
| Arithmetic | `ADD`, `SUB`, `MUL`, `DIV`, `MOD`, `NEG` |
| Comparison | `EQ`, `NE`, `LT`, `LE`, `GT`, `GE` |
| Logic | `NOT`, `AND`, `OR` |
| String | `CONCAT`, `LEN`, `INTERPOLATE` |
| Collection | `NEWARRAY`, `NEWMAP`, `GETINDEX`, `SETINDEX`, `GETFIELD`, `SETFIELD` |
| Handle | `GETHANDLE`, `SETHANDLE` |
| Function | `CALL`, `RETURN`, `TAILCALL`, `CLOSURE` |
| Jump | `JMP`, `JMPIF`, `JMPIFNOT` |
| Loop | `FORPREP`, `FORLOOP`, `ITERPREP`, `ITERLOOP` |
| Coroutine | `YIELD`, `RESUME` |
