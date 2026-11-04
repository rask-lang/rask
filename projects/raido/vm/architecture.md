<!-- id: raido.vm -->
<!-- status: proposed -->
<!-- summary: Deterministic register-based VM with static types, strict floats, serializable state, configurable arena -->
<!-- depends: raido/language/types.md -->

# VM Architecture

Register-based bytecode VM. Deterministic. Serializable. Sandboxed. Lightweight -- the implementation should be small enough to audit by hand.

## Core Properties

- **Register-based.** Each call frame has a fixed-size register window. Instructions encode register operands directly.
- **Statically typed.** The compiler knows every register's type. No runtime type tags, no type dispatch.
- **Deterministic.** Strict IEEE floats, specified PRNG, insertion-ordered maps. Identical execution on every platform.
- **Serializable.** Entire state -> bytes -> restore. No pointers, only arena offsets.
- **Send, not Sync.** Movable between threads, not shareable.
- **`@resource`** in Rask -- must be closed.

## Value Representation

**8 bytes per value.** With static types, the compiler knows the type at every register position. No runtime type tags needed.

| Type | Payload (8 bytes) |
|------|-------------------|
| `int` | i64 |
| `float` | f64 |
| `bool` | i64 (0 or 1) |
| `string` | u32 arena offset (zero-extended to i64) |
| `array` | u32 arena offset |
| `map` | u32 arena offset |
| `struct` | u32 arena offset |
| `enum` | u32 discriminant + u32 arena offset (or inline for simple enums) |
| `T?` | u8 tag (0=None, 1=Some) + 7 bytes payload |
| `func ref` | u32 prototype index |

256 registers x 8 bytes = 2 KB per call frame. At max call depth 256, worst case register memory is 512 KB.

## Determinism

`float` arithmetic is IEEE 754 binary64 under a strict profile, bitwise identical on all platforms: round-to-nearest, subnormals kept, no fused multiply-add, correctly rounded math functions (rask-lang/rask#1683). An operation that would produce NaN or an infinity raises `FloatError`, so registers only ever hold finite floats.

Integer overflow panics by default. `wrapping_add()`, `wrapping_mul()`, etc. wrap. Both behaviors are deterministic and part of the contract.

Determinism enables: lockstep networking, replay, migration, reproducible evaluation, audit trails.

### Formal Contract

- **Register-level equivalence.** Same bytecode + same inputs + same fuel -> identical register state at every instruction boundary.
- **Map iteration order** preserved across serialize/deserialize (insertion-ordered).
- **Coroutine resume sequence** is deterministic -- same yields in same order, same values.
- **Error kind and stack trace structure** are part of the contract. Exact error message text is not.
- **Fuel cost** is 1 per instruction, no exceptions.
- **Arena exhaustion** is deterministic -- same allocation sequence -> same failure point.
- **PRNG state evolution** is part of the contract -- xoshiro128++, specified seed expansion via SplitMix64.
- **Float arithmetic** follows the strict profile above -- bitwise identical on all platforms.
- **Integer overflow** panics by default. Wrapping ops wrap. Both deterministic, part of the contract.
- **Sort stability** is required -- `array.sort()` uses a stable sort algorithm.

### Provable Profile

The primary verification mode is bilateral re-execution: both parties run the same script and compare outputs. Deterministic floats replay exactly, so this covers every script.

Zero-knowledge proofs are a future option for two cases re-execution can't handle:

- **Privacy.** Re-execution requires sharing inputs. A ZK proof shows "this output came from this script" without revealing them, so a domain can prove correct minting without exposing its crafting formula or stockpile.
- **Expensive scripts.** Large simulations where re-execution is costly and many parties verify.

A ZK proof works over arithmetic in a prime field: additions and multiplications of large integers. Integer operations map onto that cheaply, with range checks to enforce 64-bit wrapping. A float operation doesn't: unpacking sign, exponent and mantissa, aligning exponents by a variable shift, normalising, rounding to nearest-even and repacking are each cheap in hardware and expensive in a circuit. General-purpose zkVMs emulate floats in software for this reason, at a cost of about a hundred times an integer operation.

So proving is a profile, not the whole language. A chunk compiled as **provable** may not use `float`; the compiler rejects it, and the chunk's header records the profile so a prover accepts only provable chunks. Minting, crafting, trades and anything conserved already count in `int`, so the scripts worth proving are the ones the profile allows.

**What the provable profile relies on:**

| Property | Why it matters for ZK |
|---|---|
| Integer arithmetic only (i64) | Maps to field arithmetic with range checks. |
| Fuel-bounded execution | Circuits need bounded loops. Fuel guarantees a max step count. |
| Fixed arena size | Bounded memory = bounded memory-consistency constraints. |
| Bump allocator | Predictable allocation. Provable chunks run with `reset()` between evaluations, so no collection has to be proven. |
| Small opcode set | Small selector circuit per step. |
| Static bytecode | Immutable during execution. Encoded once as a public input. |
| Deterministic branching | All branches depend on register values. No nondeterminism. |
| Flat register window | Fixed-size, known count. Efficient as circuit wires. |

**Execution trace.** A ZK backend would prove execution via a trace: a record of every step (opcode, PC, register reads/writes, memory reads/writes, fuel consumed). The executor records the trace, the prover converts it to a proof, the verifier checks the proof without re-executing.

**Design constraint:** future opcodes and VM changes must keep bounded execution, static bytecode, bounded memory and deterministic control flow for every chunk, and integer-only arithmetic within the provable profile.

The ZK prover would be a separate project sharing the bytecode format and semantics. It does not affect the VM spec or the direct-execution backend.

## PRNG

**xoshiro128++.** 128-bit state (four u32s), 32-bit output.

- Small state (16 bytes). Fast (3 ops per output). Well-studied. Public domain.
- State is part of VM state and serialized.
- `math.random()` -> float (0.0 to 1.0 exclusive): 53 bits from two PRNG outputs, scaled by 2^-53.
- `math.random(n)` -> int (0 to n-1 inclusive).
- Seeded at VM creation. `vm.seed(u64)` splits into initial state via SplitMix64.

Reference: Blackman & Vigna, 2018. The algorithm is fixed -- changing it is a breaking change to determinism.

## Arena

All VM-managed heap objects live in a single contiguous byte array. Bump allocation; dead objects are reclaimed by a deterministic mark-compact collection at frame boundaries.

### Layout

Arena is a flat `[u8]` with a bump pointer (`top: u32`). Every object starts with an 8-byte header:

```
Object header (8 bytes):
  kind (u8)      -- object kind tag (see table)
  flags (u8)     -- bit 0: mark (used only during collection); others reserved, zero
  type_id (u16)  -- struct/enum type, or element type for arrays and value type for maps
  size (u32)     -- object body size in bytes
```

The collector needs `kind` and `type_id` to know which 8-byte slots of an object hold arena references; nothing else reads them during normal execution, since static typing means every access already knows the layout. They also serve serialization validation and debugging.

**Object kind tags** (exhaustive):

| Value | Kind | Body layout |
|-------|------|-------------|
| 0 | *reserved* | zeroed arena = invalid, not a zero-length string |
| 1 | String | `len (u32) \| utf-8 bytes[len]` |
| 2 | Array | `len (u32) \| cap (u32) \| values[cap]` |
| 3 | Map | `live (u32) \| cap (u32) \| indices[cap] \| entries[cap]` |
| 4 | Struct | `fields[N] (each 8 bytes)` |
| 5 | Enum (with payload) | `payload fields[N] (each 8 bytes)` |
| 6 | Closure | `proto (u32) \| _pad (u32) \| captures[N] (each 8 bytes)` |

Values 7--255 are reserved. Types that live in registers -- `int`, `float`, `bool`, `T?` of a register type, a function reference with no captures, simple enum (no payload) -- have no arena object. Coroutines are stored in a separate VM-side structure, not the arena; their registers are roots for the collector.

All objects are 8-byte aligned. The bump pointer advances by `8 (header) + size`, rounded up to 8.

### Object Bodies

**String:**
```
len (u32) | utf-8 bytes[len]
```

**Array:**
```
len (u32) | cap (u32) | values[cap] (each 8 bytes)
```
`len` is the logical length. `cap` is the allocated slot count. `push` beyond `cap` allocates a new, larger array and copies -- the old one becomes dead space. No type tags per element -- `array<int>` stores raw i64 values. Element type is known from bytecode metadata.

**Map (compact dict layout):**
```
live (u32) | cap (u32) | indices[cap] (each i32) | entries[cap]
```
Each entry: `key (8 bytes) | value (8 bytes) | hash (u32) | _pad (4 bytes)` = 24 bytes. `live` is the count of live entries. Entries are stored dense in insertion order, appended at the end.

The `indices` array is the hash table -- open addressing, linear probing. Each slot holds an index into the entries array, or -1 for empty, -2 for deleted (tombstone). Lookup: hash key -> probe indices -> follow index to entry -> compare key. Insert: append entry, store its position in indices. Delete: tombstone the index slot, mark the entry as dead.

Iteration walks entries[0..len], skipping deleted entries. This preserves insertion order.

Map growth: when load factor exceeds ~75%, allocate a new map with larger `cap` in the arena, rehash all live entries, copy them in order. The old map becomes dead space. Same pattern as array growth.

**Struct:**
```
fields[N] (each 8 bytes)
```
Fixed-size layout determined at compile time from the struct declaration. Field count and types known from bytecode metadata. No header fields beyond the arena object header.

**Enum (with payloads):**
```
payload fields[N] (each 8 bytes)
```
The discriminant lives in the register (top 32 bits), not in the arena. The arena body contains only the payload fields for the active variant. The compiler knows the field count per variant from the type table.

Simple enums (no payloads) are stored inline as a u32 discriminant in the register -- no arena allocation.

### Limits

- **Fixed size.** Set at VM creation. Exceeding = `ArenaExhausted` error.
- **No auto-grow.** Hides allocation cost.
- **Max arena: 4 GB** (u32 offsets). Practical limit set by config, usually 256 KB--4 MB.

### Collection at Frame Boundaries

Scripts allocate freely during a call. Dead objects are reclaimed only at points the host chooses, never in the middle of script code, so collection is deterministic and needs no stack maps for running frames.

**`reset()`** -- sets `top = 0`. Clears all arena contents and coroutine state. Total wipe. Use between independent evaluations.

**`frame_end()`** -- a collection. Everything reachable from the roots survives, slid down in allocation order to the bottom of the arena (mark-compact; no second copy of the arena is needed). Everything else is gone. Then `top` is the end of the survivors.

**Roots** are the `Root` handles the host holds ([interop.md](interop.md#host-references)). That includes the coroutines the host resumes each tick. Everything else lives because something reachable refers to it:
- a reachable coroutine keeps its registers alive. The compiler emits a reference map for each `yield` site, since static types say exactly which registers hold arena references there;
- a reachable closure keeps its captures alive;
- structs, arrays, maps and enums keep their reference slots alive, found through each object's `type_id`.

A suspended coroutine nothing refers to can never be resumed, so it's collected with its registers. No script frame is running at `frame_end()`, so there are no other registers to scan.

**Deterministic by construction.** The mark walks roots in a fixed order (coroutine table order, register index order, root-handle creation order) and sliding compaction keeps allocation order, so the arena's bytes after a collection are identical on every machine. Collection costs no fuel; it's the host's work, done at the host's chosen point. For replay, the host calls `frame_end()` at the same ticks on every machine, as it already adds fuel at the same points.

**Why not a frame/persistent split.** The arena used to keep allocations below a `frame_base` and throw away everything above it at `frame_end()`. A value made during a frame and kept past it (a coroutine's waypoints array, a struct stored in a long-lived map) then pointed at freed memory, and the spec called that a host bug. Coroutines and closures make keeping values past a frame the normal case, so the arena now finds what's still reachable instead of asking scripts to promise.

**Cost.** A collection costs time proportional to what survives. A host with large long-lived state collects less often (every N frames, or when the arena passes a threshold); the deterministic-replay rule above applies to whatever schedule it picks.

## Resource Limits

Three runaway vectors, three limits:

### Fuel (instruction budget)

Every instruction costs 1 fuel. Reaching zero = `FuelExhausted` error.

```rask
let vm = raido.Vm.new(raido.Config {
    arena_size: 256.kilobytes(),
    initial_fuel: 100_000,
    max_call_depth: 256,
})
```

```rask
let remaining = vm.fuel()
vm.add_fuel(50_000)
vm.set_fuel(100_000)
```

Fuel is serialized. For deterministic replay, hosts must add fuel at the same points.

### Call Depth

Max call stack depth. Default 256. `CALL` / `RESUME` increment, `RETURN` / `YIELD` decrement. Exceeding = `CallOverflow` error.

`TAIL_CALL` reuses the current frame -- doesn't increment.

### Arena Size

Fixed at creation. Exceeding = `ArenaExhausted` error.

## Serialization

`vm.serialize()` -> bytes. `Vm.deserialize(bytes)` -> restored VM.

**Captures:** registers, call frames, coroutines, arena contents, root handles, PRNG state (4 x u32), fuel remaining.

**Does not capture:** host function closures (by name), bytecode (re-loaded by host).

Format version in the header. Unknown version = `VersionMismatch` error on deserialize.

With static types, serialization is simpler -- no type tags to encode per value. The deserializer knows the type of every register from the bytecode metadata.

## Instruction Set

Instructions are 32 bits. Three formats:

```
ABC:  op(8) | A(8)  | B(8)  | C(8)       -- 3 register operands
ABx:  op(8) | A(8)  | Bx(16)             -- register + 16-bit unsigned index
AsBx: op(8) | A(8)  | sBx(16, signed)    -- register + 16-bit signed offset
```

Register operands are 8-bit (0--255). Constant/index operands are 16-bit (0--65535).

### Opcodes

~38 instructions. Each described with format, operands, and exact behavior.

#### Load/Move

| Op | Fmt | Semantics |
|----|-----|-----------|
| `LOAD_TRUE A` | ABx | `R[A] = true` |
| `LOAD_FALSE A` | ABx | `R[A] = false` |
| `LOAD_INT A Bx` | ABx | `R[A] = Bx as i64` (16-bit signed, sign-extended) |
| `LOAD_CONST A Bx` | ABx | `R[A] = K[Bx]` (constant pool lookup) |
| `LOAD_NONE A` | ABx | `R[A] = None` (optional with tag=0) |
| `MOVE A B` | ABC | `R[A] = R[B]` |

#### Arithmetic

All arithmetic is type-specific -- the compiler emits the correct variant based on operand types. No runtime type dispatch. In mixed `int`/`float` expressions, the compiler converts the `int` operand to `float` before the operation.

| Op | Fmt | Semantics |
|----|-----|-----------|
| `ADD A B C` | ABC | `R[A] = R[B] + R[C]` |
| `SUB A B C` | ABC | `R[A] = R[B] - R[C]` |
| `MUL A B C` | ABC | `R[A] = R[B] * R[C]` |
| `DIV A B C` | ABC | `R[A] = R[B] / R[C]` (division by zero = `DivisionByZero` error) |
| `MOD A B C` | ABC | `R[A] = R[B] % R[C]` (sign follows dividend) |
| `NEG A B` | ABC | `R[A] = -R[B]` |

**Result types:** `int OP int -> int` (except `DIV` which always returns `float`). `float OP float -> float`. Mixed ops: compiler promotes int to float, result is `float`. Integer overflow panics. A float result that is NaN or infinite raises `FloatError`.

#### Comparison

Result stored in register as `bool`.

| Op | Fmt | Semantics |
|----|-----|-----------|
| `EQ A B C` | ABC | `R[A] = R[B] == R[C]` |
| `LT A B C` | ABC | `R[A] = R[B] < R[C]` |
| `LE A B C` | ABC | `R[A] = R[B] <= R[C]` |

`NE`, `GT`, `GE` not needed -- compiler emits `EQ` + `NOT`, or swaps operands.

#### Logic / Misc

| Op | Fmt | Semantics |
|----|-----|-----------|
| `NOT A B` | ABC | `R[A] = !R[B]` (boolean negation) |
| `LEN A B` | ABC | `R[A] = len(R[B])` (string byte length, array length, map length) |
| `CONCAT A B C` | ABC | `R[A] = tostring(R[B]) ++ tostring(R[C])` (allocates string in arena) |

#### Collections

| Op | Fmt | Semantics |
|----|-----|-----------|
| `NEW_ARRAY A B` | ABC | `R[A] = new array`. `B` items from `R[A+1..A+B]` as initial elements. |
| `NEW_MAP A B` | ABC | `R[A] = new map`. `B` key-value pairs from `R[A+1..A+2B]`. |
| `GET_INDEX A B C` | ABC | `R[A] = R[B][R[C]]`. Array: int index, bounds-checked. Map: key lookup. |
| `SET_INDEX A B C` | ABC | `R[A][R[B]] = R[C]`. |

#### Structs

| Op | Fmt | Semantics |
|----|-----|-----------|
| `NEW_STRUCT A Bx` | ABx | Allocate struct of type `Bx` in arena, fields from registers starting at `A+1`. |
| `GET_STRUCT_FIELD A B C` | ABC | `R[A] = R[B].fields[C]`. Field index known at compile time. |
| `SET_STRUCT_FIELD A B C` | ABC | `R[A].fields[B] = R[C]`. |

#### Enums

| Op | Fmt | Semantics |
|----|-----|-----------|
| `ENUM_TAG A B` | ABC | `R[A] = discriminant(R[B])`. For `match` dispatch. |

#### Host Fields (extern struct)

| Op | Fmt | Semantics |
|----|-----|-----------|
| `GET_FIELD A B C` | ABC | `R[A] = R[B].fields[C]`. `C` is the vtable slot index. Calls the host getter. |
| `SET_FIELD A B C` | ABC | `R[A].fields[B] = R[C]`. Calls the host setter. Readonly field = compile error. |

#### Control Flow

| Op | Fmt | Semantics |
|----|-----|-----------|
| `JMP sBx` | AsBx | `PC += sBx` |
| `JMP_IF A sBx` | AsBx | `if R[A]: PC += sBx` |
| `JMP_IF_NOT A sBx` | AsBx | `if !R[A]: PC += sBx` |

Jumps are relative to the *next* instruction (PC after decode, before jump).

#### Functions

| Op | Fmt | Semantics |
|----|-----|-----------|
| `CALL A B C` | ABC | Call `R[A]` with args `R[A+1..A+B]`. `B` = arg count. Result in `R[A]`. |
| `TAIL_CALL A B` | ABC | Same as `CALL` but reuses current frame. No call depth increment. |
| `RETURN A` | ABx | Return `R[A]` to caller. If `A = 255`, return void. |
| `FUNC_REF A Bx` | ABx | `R[A] = reference to prototype Bx`. For function references. |

**Call convention:** caller places function in `R[A]`, args in `R[A+1..A+B]`. Callee sees args in `R[0..B-1]` of its own window. Single return value placed in caller's `R[A]` after return.

#### Optionals

| Op | Fmt | Semantics |
|----|-----|-----------|
| `IS_SOME A B` | ABC | `R[A] = bool(R[B] is Some)`. Check optional tag. |
| `UNWRAP A B` | ABC | `R[A] = payload(R[B])`. Panics with `UnwrapNone` if None. |
| `WRAP_SOME A B` | ABC | `R[A] = Some(R[B])`. Construct optional from value. |

`LOAD_NONE` (in Load/Move) constructs `None`. The compiler emits `IS_SOME` + `JMP_IF_NOT` for `is Some` patterns, `??`, and match arms. `UNWRAP` is used for `!` force unwrap and after successful `IS_SOME` checks.

#### Coroutines

| Op | Fmt | Semantics |
|----|-----|-----------|
| `COROUTINE A B C` | ABC | `R[A] = new coroutine` from func ref `R[B]` with `C` args in `R[B+1..B+C]`. State: `suspended`. |
| `YIELD A` | ABx | Suspend current coroutine. `R[A]` is the yielded value. |
| `RESUME A B` | ABC | Resume coroutine `R[A]` with value `R[B]`. Result in `R[A]`. Dead coroutine = `CoroutineDead` error. |

Coroutine creation is similar to `CALL`: the function reference is in `R[B]`, arguments follow in `R[B+1..B+C]`. The coroutine's register window receives the arguments in `R[0..C-1]`. First `resume()` starts execution from the top of the function.

## Error Types

Every runtime error is a `raido.ScriptError` with a `kind` enum and a message string:

| Kind | Cause |
|------|-------|
| `DivisionByZero` | Division or modulo by zero |
| `IntegerOverflow` | Integer arithmetic overflow (non-wrapping) |
| `FloatError` | A float operation produced NaN or an infinity |
| `IndexOutOfBounds` | Array index < 0 or >= len |
| `KeyNotFound` | Map key doesn't exist (on direct access) |
| `UnwrapNone` | Force unwrap (`!`) on `None` |
| `ArenaExhausted` | Bump allocator exceeded arena size |
| `FuelExhausted` | Instruction budget reached zero |
| `CallOverflow` | Call depth exceeded max_call_depth |
| `CoroutineDead` | Resumed a finished coroutine |
| `HostError` | Raised by a host function |
| `ScriptError` | Raised by `error(msg)` in script code |

All errors carry: kind, message (string), and a stack trace (array of `{file, line, function}` if debug info is present, `{proto_idx, pc_offset}` if not).

Scripts can't catch any of these. An error unwinds the whole call stack to the host, which receives it as a `raido.ScriptError`, and the call's changes are rolled back ([interop.md](interop.md#calls-are-transactions)).

**Eliminated by static types:** `TypeError` (impossible by construction), `ReadOnlyField` (compiler rejects writes to `readonly` extern fields).

## Compilation

Two-pass compiler. No full AST. Memory is O(declarations), not O(program size).

1. **Declaration pass.** Scan for `struct`, `enum`, `extend` blocks, `extern struct`, `extern func`, `func` signatures, and `import` statements. Build a type table mapping names to types. Record function and method signatures but not bodies.
2. **Compile pass.** Recursive descent with type checking. Emit bytecode directly during parsing. Register allocation same as current (linear, locals sequential, temporaries bump). Type inference for locals uses forward flow: `let x = 42` -> x is `int`. No backward inference, no constraint solving.
3. **Register allocation.** Linear. Locals get sequential registers. Temporaries use a stack-like bump above locals. No optimization passes.
4. **Constant folding.** Compile-time evaluation of constant expressions (`1 + 2` -> `LOAD_INT 3`). Only for arithmetic on literals.
5. **Output.** Chunk (see [chunk-format.md](chunk-format.md)).

**Tail call constraint.** The compiler only emits `TAIL_CALL` when a call expression is in tail position -- the last expression in a function body, directly before an implicit or explicit `return`. A `TAIL_CALL` in non-tail position would corrupt the caller's frame, so the compiler must never emit one there.
