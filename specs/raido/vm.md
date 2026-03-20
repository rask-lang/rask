<!-- id: raido.vm -->
<!-- status: proposed -->
<!-- summary: Register-based VM with arena allocation, memory limits, and instruction budgets -->
<!-- depends: raido/values.md -->

# VM Architecture

Register-based bytecode VM with arena allocation. No garbage collector. Memory-limited, instruction-budgeted, single-threaded.

## VM Structure

| Rule | Description |
|------|-------------|
| **VM1: Register-based** | Instructions operate on numbered registers. Fewer instructions per operation than stack VMs. |
| **VM2: 32-bit instructions** | Fixed-width: 8-bit opcode + operands (register indices, constants, jump offsets). |
| **VM3: Send, not Sync** | A VM can be moved between threads but not shared. No internal synchronization. |
| **VM4: Resource type** | `Vm` is `@resource` in Rask — must be closed explicitly via `vm.close()`. |
| **VM5: Separate compilation** | Scripts compile to bytecode chunks. Compilation is fallible. Execution is separate. |
| **VM6: Multiple chunks** | A VM can load and execute multiple chunks. They share the global table. |
| **VM7: Tiny** | Base overhead targets ~1 KB. A server can run hundreds of VMs concurrently. |

```rask
const vm = raido.Vm.new(raido.Config {
    arena_size: 256.kilobytes(),
    instruction_limit: 100_000,
})
ensure vm.close()

const chunk = try vm.compile("goblin_ai.raido", source)
try vm.exec(chunk)
```

## Arena Allocation

| Rule | Description |
|------|-------------|
| **A1: Single arena** | All VM allocations (tables, strings, closures, bytecode) come from one contiguous arena. |
| **A2: Bump allocator** | Allocation is a pointer bump. O(1). No per-object overhead. |
| **A3: Reset** | `vm.reset()` frees everything at once. Pointer resets to start. All script state gone. |
| **A4: Memory limit** | Arena has a fixed size set at creation. Allocation beyond it raises a runtime error. |
| **A5: No GC** | No garbage collector. No mark phase, no sweep, no write barriers, no finalizers. |
| **A6: No individual free** | Objects cannot be freed individually. Only bulk reset via `vm.reset()`. |

```rask
const vm = raido.Vm.new(raido.Config {
    arena_size: 256.kilobytes(),
})

// Run script, allocates tables/strings in arena
try vm.exec_with(|scope| {
    scope.provide_pool("enemies", enemies)
    scope.call("on_update", [raido.Value.number(dt)])
})

// Reset arena — all script-local state freed at once
vm.reset()
```

No GC means no pauses, no tuning, no write barriers. The tradeoff: scripts can't selectively free memory. Temporaries accumulate until the host resets the arena.

**When to reset:** Between game events, between script reloads, or when the arena is getting full. The host controls this — scripts never trigger a reset.

**Persistent state lives in Rask pools, not in the VM.** Entity data, inventory, world state — all managed by the server's Rask code. Scripts read and write that state through handles. The VM's arena is for script-local temporaries: intermediate tables, string concatenations, closure captures.

### String Handling

| Rule | Description |
|------|-------------|
| **AS1: Arena strings** | Strings created by scripts are allocated in the arena. They use Raido's own repr, not Rask's refcounted `string`. |
| **AS2: Host strings copied in** | When Rask passes a string to Raido, the bytes are copied into the arena. |
| **AS3: Script strings copied out** | When Raido returns a string to Rask, a new Rask `string` is created from the arena bytes. |
| **AS4: Interned constants** | String literals in bytecode are interned in the arena at compile time. |

I considered sharing Rask's refcounted string representation to avoid copies. But arena allocation makes that unnecessary and complicated — arena-allocated objects don't have individual lifetimes, so refcount management becomes a headache. Copying a few strings per frame is cheap. Keep it simple.

## Instruction Set

| Category | Opcodes |
|----------|---------|
| **Load** | `LOADNIL`, `LOADBOOL`, `LOADINT`, `LOADNUM`, `LOADK`, `LOADGLOBAL`, `STOREGLOBAL` |
| **Move** | `MOVE` |
| **Arithmetic** | `ADD`, `SUB`, `MUL`, `DIV`, `IDIV`, `MOD`, `POW`, `NEG` |
| **Comparison** | `EQ`, `NE`, `LT`, `LE`, `GT`, `GE` |
| **Logic** | `NOT`, `AND`, `OR` |
| **String** | `CONCAT`, `LEN` |
| **Table** | `NEWTABLE`, `GETTABLE`, `SETTABLE`, `GETFIELD`, `SETFIELD` |
| **Handle** | `GETHANDLE`, `SETHANDLE` (pool-resolved field access) |
| **Function** | `CALL`, `RETURN`, `TAILCALL`, `CLOSURE`, `VARARG` |
| **Jump** | `JMP`, `JMPIF`, `JMPIFNOT` |
| **Loop** | `FORPREP`, `FORLOOP`, `ITERPREP`, `ITERLOOP` |
| **Coroutine** | `YIELD`, `RESUME` |

`GETHANDLE`/`SETHANDLE` are the Raido-specific opcodes. They encode a field name (interned string index) and route through the pool lookup at runtime.

## Instruction Limits

| Rule | Description |
|------|-------------|
| **IL1: Per-call budget** | Each `vm.call()` or `vm.exec()` has an instruction limit. Exceeding it raises a runtime error. |
| **IL2: Configurable** | Set at VM creation. Can be overridden per call. |
| **IL3: Counting** | Every instruction decrements the budget. |
| **IL4: Coroutine-aware** | Yielding saves the remaining budget. Resuming restores it. |

Instruction limits prevent runaway scripts. A modder's infinite loop shouldn't freeze the server.

## Bytecode

| Rule | Description |
|------|-------------|
| **B1: Chunk structure** | Header (magic, version) + constants pool + instructions + function prototypes. |
| **B2: Constants** | Interned strings, numbers, ints in a per-chunk constant pool. |
| **B3: Debug info** | Source file name, line number mapping. Optional, strippable. |
| **B4: Nested functions** | Inner functions stored as prototypes. `CLOSURE` instantiates them. |
| **B5: Arena-allocated** | Bytecode lives in the VM's arena. Reset frees it too. |

```
Chunk layout:
  [magic: u32]          0x52616964 ("Raid")
  [version: u8]         1
  [num_constants: u32]
  [constants...]
  [num_instructions: u32]
  [instructions...]
  [num_prototypes: u32]
  [prototypes...]
  [debug_info...]        (optional)
```

## Call Frames

| Rule | Description |
|------|-------------|
| **CF1: Fixed register window** | Each call frame has a fixed register count (determined at compile time). |
| **CF2: Max depth** | Call stack depth limited (default: 128). Stack overflow raises error. |
| **CF3: Tail calls** | `TAILCALL` reuses the current frame. |
| **CF4: Host calls** | Host function calls push a special frame. Closure runs with `CallContext`. |

## Hot Reload

| Rule | Description |
|------|-------------|
| **HR1: Reset + reload** | Hot reload = `vm.reset()` then `vm.exec(new_chunk)`. Clean slate. |
| **HR2: No state preservation** | Arena reset destroys all script state. Persistent state lives in Rask pools. |

```rask
// Hot reload: recompile, reset, re-execute
const new_chunk = try vm.compile("goblin_ai.raido", new_source)
vm.reset()
try vm.exec(new_chunk)
// Script-local state gone. Pool state (entity data) untouched.
```

This is simpler than trying to preserve function definitions while keeping state. Reset is a clean cut — old code gone, new code loaded. Entity data in pools survives because it was never in the VM.

## Edge Cases

| Case | Rule | Behavior |
|------|------|----------|
| Arena full | A4 | Runtime error: "arena exhausted (256 KB limit)". |
| `vm.reset()` during `exec_with` | A3 | Not allowed — reset while executing raises a panic. |
| `vm.close()` with active coroutines | VM4 | Arena freed. Coroutine state gone. No cleanup needed. |
| String larger than remaining arena | AS1 | Runtime error: arena exhausted. |
| Compile after reset | B5 | Bytecode re-allocated in fresh arena. Must recompile. |
| Many small VMs | VM7 | Each VM is ~1 KB base + arena. 1000 VMs = ~250 MB with 256 KB arenas. |

## Error Messages

```
ERROR [raido.vm/A4]: arena exhausted
   |
   |  VM arena: 256 KB used of 256 KB limit
   |  Last allocation: table with 64 entries (512 bytes)

WHY: Script exceeded the arena size.

FIX: Increase arena_size in Vm config, reset the arena more frequently,
     or move data to Rask pools instead of script-local tables.
```

```
ERROR [raido.vm/IL1]: instruction limit exceeded
   |
3  |  while true do end
   |  ^^^^^^^^^^^^^^^^^ ran 100,000 instructions

WHY: Script exceeded the per-call instruction budget.

FIX: Break work into smaller calls, or increase instruction_limit.
```

---

## Appendix (non-normative)

### Rationale

**A1/A5 (arena, no GC):** The use case is custom entity scripts on a game server. Potentially hundreds of VMs, one per entity type. GC adds per-VM overhead (write barriers, mark state, tune parameters) and unpredictable pauses. Arena allocation is O(1) alloc, O(1) reset, zero overhead between those operations.

The key insight: persistent state doesn't belong in the VM. Entity data lives in Rask pools. Script-local state (temporary tables, string concatenation, local variables) is ephemeral — it exists for one call or one frame, then it's garbage. Arena reset clears it all at once.

**AS1 (arena strings, not shared):** Sharing Rask's refcounted string representation was tempting for zero-copy. But arena-allocated objects have no individual lifetimes — you can't decrement a refcount when an arena string "dies" because arena strings don't die individually. The complexity of tracking which arena slots contain Rask strings (for refcount cleanup on reset) isn't worth it. Copy strings at the boundary. For game scripting workloads (short strings, entity names, status messages), the copy cost is negligible.

**VM7 (tiny):** A game server running custom scripts for 200 entity types needs 200 VMs. At 256 KB arena each, that's 50 MB — reasonable. The base VM overhead (registers, globals, call stack) targets ~1 KB. If arenas were 1 MB each, 200 VMs would be 200 MB — too much. Arena size should match the workload: entity AI scripts rarely need more than a few KB of temporaries.

### Performance Expectations

- **Instruction dispatch:** ~100-200M instructions/sec (register-based, direct threading)
- **Allocation:** ~1 ns (pointer bump)
- **Arena reset:** ~1 μs (pointer reset + string copy-out if needed)
- **VM creation:** <10 μs
- **Memory per VM:** ~1 KB base + arena size

### See Also

- `raido.values` — Value representation and NaN-boxing
- `raido.interop` — How the host creates and controls VMs
- `raido.coroutines` — Coroutine state lives in the arena
- `mem.pools` — Where persistent entity state lives (not in the VM)
