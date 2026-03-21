<!-- id: raido.vm -->
<!-- status: proposed -->
<!-- summary: Deterministic register-based VM with fixed-point math, serializable state, configurable arena -->
<!-- depends: raido/language/types.md -->

# VM Architecture

Register-based bytecode VM. Deterministic. Serializable. Sandboxed. Lightweight — the implementation should be small enough to audit by hand.

## Core Properties

- **Register-based.** Each call frame has a fixed-size register window. Instructions encode register operands directly.
- **Deterministic.** Fixed-point arithmetic, specified PRNG, insertion-ordered maps. Identical execution on every platform.
- **Serializable.** Entire state → bytes → restore. No pointers, only arena offsets.
- **Send, not Sync.** Movable between threads, not shareable.
- **`@resource`** in Rask — must be closed.

## Value Representation

Tagged enum, 16 bytes per value. Larger than NaN-boxing (8 bytes) but trivial to implement, debug, and serialize. Values live in registers, not on the wire — chunk format determines wire size, not value representation.

```
Tag (u8) | Padding (7 bytes) | Payload (8 bytes)
```

| Tag | Payload |
|-----|---------|
| 0 `nil` | unused |
| 1 `bool` | 0 or 1 (u8) |
| 2 `int` | i64 |
| 3 `number` | i64 (32.32 fixed-point, raw bits) |
| 4 `string` | u32 arena offset |
| 5 `array` | u32 arena offset |
| 6 `map` | u32 arena offset |
| 7 `closure` | u32 arena offset |
| 8 `host_ref` | u32 type_id + u32 ref_id |

16 bytes per register slot. 256 registers per frame = 4 KB per call frame. At max call depth 256, worst case register memory is 1 MB. Acceptable.

## Determinism

All `number` arithmetic is 32.32 fixed-point (integer math). No FPU. Bitwise-identical on all platforms.

Add/sub = single i64 op. Mul/div = 128-bit intermediate.

Determinism enables: lockstep networking, replay, migration, reproducible evaluation, audit trails.

## PRNG

**xoshiro128++.** 128-bit state (four u32s), 32-bit output.

- Small state (16 bytes). Fast (3 ops per output). Well-studied. Public domain.
- State is part of VM state and serialized.
- `math.random()` → number (0.0 to 1.0 exclusive, 32.32 fixed-point).
- `math.random(n)` → int (0 to n-1 inclusive).
- Seeded at VM creation. `vm.seed(u64)` splits into initial state via SplitMix64.

Reference: Blackman & Vigna, 2018. The algorithm is fixed — changing it is a breaking change to determinism.

## Arena

All VM-managed heap objects live in a single contiguous byte array. Bump allocator. No GC.

### Layout

Arena is a flat `[u8]` with a bump pointer (`top: u32`). Every object starts with a 4-byte header:

```
Object header (4 bytes):
  type (u8)   — same as value tag (4=string, 5=array, 6=map, 7=closure)
  _pad (u8)   — reserved, zero
  size (u16)  — object body size in bytes (max 64 KB per object)
```

All objects are 4-byte aligned. The bump pointer advances by `4 (header) + size`, rounded up to 4-byte alignment.

### Object Bodies

**String:**
```
len (u32) | utf-8 bytes[len]
```

**Array:**
```
len (u32) | cap (u32) | values[cap] (each 16 bytes)
```
`len` is the logical length. `cap` is the allocated slot count. `push` beyond `cap` allocates a new, larger array and copies — the old one becomes dead space. No compaction.

**Map:**
```
len (u32) | cap (u32) | entries[cap]
```
Each entry: `key (16 bytes) | value (16 bytes) | hash (u32) | occupied (u8) | _pad (3 bytes)`. Open addressing, linear probing. Insertion-ordered: iteration walks entries 0..len.

**Closure:**
```
proto_idx (u16) | upvalue_count (u16) | upvalue_offsets[upvalue_count] (each u32)
```
Each upvalue offset points to a 16-byte Value slot in the arena.

**Upvalue:**
```
value (16 bytes)
```
Bare Value slot. Closures point to these. When a local is captured, it's moved from the register to a new upvalue slot in the arena.

### Limits

- **Fixed size.** Set at VM creation. Exceeding = `ArenaExhausted` error.
- **No auto-grow.** Hides allocation cost.
- **Max object size: 64 KB** (u16 size field). Arrays/maps beyond this are a design smell in script code.
- **Max arena: 4 GB** (u32 offsets). Practical limit set by config, usually 256 KB–4 MB.

### reset() and frame_end()

**`reset()`** — sets `top = 0`. Clears all arena contents, globals, coroutine state. Total wipe. Use between independent evaluations.

**`frame_end()`** — resets allocations made since the last `frame_begin()` marker, keeping everything below the marker.

How it works:
1. At VM creation or after `reset()`, host calls `vm.frame_begin()`. This saves `top` as the `frame_base`.
2. Script runs. Allocations bump `top` above `frame_base`.
3. Host calls `vm.frame_end()`. Sets `top = frame_base`. All frame allocations become dead.
4. Next frame: host calls `vm.frame_begin()` again (saves current `top` as new `frame_base`).

**What's persistent:** everything below `frame_base` — globals, long-lived closures, data allocated before the first `frame_begin()` or between frames by host calls.

**What's frame-local:** everything above `frame_base` — temporary strings from interpolation, arrays built during the frame, intermediate results.

If a script stores a frame-local value into a global, that value becomes a dangling offset after `frame_end()`. This is a **host bug**, not a VM bug. The rule: don't store frame-local arena objects into persistent state. The VM doesn't enforce this — enforcement would require a write barrier, which contradicts "lightweight." Document it clearly in the host API docs.

## Resource Limits

Three runaway vectors, three limits:

### Fuel (instruction budget)

Every instruction costs 1 fuel. Reaching zero = `FuelExhausted` error.

```rask
const vm = raido.Vm.new(raido.Config {
    arena_size: 256.kilobytes(),
    initial_fuel: 100_000,
    max_call_depth: 256,
})
```

```rask
const remaining = vm.fuel()
vm.add_fuel(50_000)
vm.set_fuel(100_000)
```

Fuel is serialized. For deterministic replay, hosts must add fuel at the same points.

### Call Depth

Max call stack depth. Default 256. `CALL` / `RESUME` increment, `RETURN` / `YIELD` decrement. Exceeding = `CallOverflow` error.

`TAIL_CALL` reuses the current frame — doesn't increment.

### Arena Size

Fixed at creation. Exceeding = `ArenaExhausted` error.

## Serialization

`vm.serialize()` → bytes. `Vm.deserialize(bytes)` → restored VM.

**Captures:** registers, call frames, globals, coroutines, arena contents, PRNG state (4 × u32), fuel remaining, frame_base.

**Does not capture:** host function closures (by name), bytecode (re-loaded by host).

Format version in the header. Unknown version = `VersionMismatch` error on deserialize.

## Closures and Upvalues

When a local variable is captured:
1. Compiler emits `CLOSE_UPVALUE` when the variable goes out of scope.
2. The variable moves from the register to a 16-byte upvalue slot in the arena.
3. The closure stores the arena offset.
4. Multiple closures capturing the same variable share the arena offset.

Before the variable is closed, the closure's upvalue entry points to the register directly (by frame index + register index). After closing, it points to the arena slot. The `GET_UPVALUE` / `SET_UPVALUE` instructions check which mode applies.

## Instruction Set

Instructions are 32 bits. Three formats:

```
ABC:  op(8) | A(8)  | B(8)  | C(8)       — 3 register operands
ABx:  op(8) | A(8)  | Bx(16)             — register + 16-bit unsigned index
AsBx: op(8) | A(8)  | sBx(16, signed)    — register + 16-bit signed offset
```

Register operands are 8-bit (0–255). Constant/global indices are 16-bit (0–65535).

### Opcodes

37 instructions. Each described with format, operands, and exact behavior.

#### Load/Move

| Op | Fmt | Semantics |
|----|-----|-----------|
| `LOAD_NIL A` | ABx | `R[A] = nil` |
| `LOAD_TRUE A` | ABx | `R[A] = true` |
| `LOAD_FALSE A` | ABx | `R[A] = false` |
| `LOAD_INT A Bx` | ABx | `R[A] = Bx as i64` (16-bit signed, sign-extended) |
| `LOAD_CONST A Bx` | ABx | `R[A] = K[Bx]` (constant pool lookup) |
| `MOVE A B` | ABC | `R[A] = R[B]` |

#### Globals

| Op | Fmt | Semantics |
|----|-----|-----------|
| `GET_GLOBAL A Bx` | ABx | `R[A] = globals[Bx]` |
| `SET_GLOBAL A Bx` | ABx | `globals[Bx] = R[A]` |

Global names resolved to indices at compile time. Globals array is part of VM state.

#### Upvalues

| Op | Fmt | Semantics |
|----|-----|-----------|
| `GET_UPVALUE A B` | ABC | `R[A] = upvalues[B]` (current closure's upvalue list) |
| `SET_UPVALUE A B` | ABC | `upvalues[B] = R[A]` |
| `CLOSE_UPVALUE A` | ABx | Move `R[A]` to arena upvalue slot. Update all closures referencing this register. |

#### Arithmetic

All arithmetic ops: if both operands are `int`, result is `int` (except `DIV` which always returns `number`). If either is `number`, promote and result is `number`. Type mismatch = `TypeError` error.

| Op | Fmt | Semantics |
|----|-----|-----------|
| `ADD A B C` | ABC | `R[A] = R[B] + R[C]` |
| `SUB A B C` | ABC | `R[A] = R[B] - R[C]` |
| `MUL A B C` | ABC | `R[A] = R[B] * R[C]` |
| `DIV A B C` | ABC | `R[A] = R[B] / R[C]` (division by zero = `DivisionByZero` error) |
| `MOD A B C` | ABC | `R[A] = R[B] % R[C]` |
| `NEG A B` | ABC | `R[A] = -R[B]` |

#### Comparison

Result stored in register as `bool`.

| Op | Fmt | Semantics |
|----|-----|-----------|
| `EQ A B C` | ABC | `R[A] = R[B] == R[C]` |
| `LT A B C` | ABC | `R[A] = R[B] < R[C]` |
| `LE A B C` | ABC | `R[A] = R[B] <= R[C]` |

`NE`, `GT`, `GE` not needed — compiler emits `EQ` + `NOT`, or swaps operands. Fewer opcodes = smaller validator, smaller interpreter loop.

#### Logic / Misc

| Op | Fmt | Semantics |
|----|-----|-----------|
| `NOT A B` | ABC | `R[A] = !R[B]` (falsy = nil/false, all else truthy) |
| `LEN A B` | ABC | `R[A] = len(R[B])` (string byte length, array length, map length) |
| `CONCAT A B C` | ABC | `R[A] = tostring(R[B]) ++ tostring(R[C])` (allocates string in arena) |

String interpolation compiles to a chain of `CONCAT` ops.

#### Collections

| Op | Fmt | Semantics |
|----|-----|-----------|
| `NEW_ARRAY A B` | ABC | `R[A] = new array`. `B` items popped from `R[A+1..A+B]` as initial elements. |
| `NEW_MAP A B` | ABC | `R[A] = new map`. `B` key-value pairs from `R[A+1..A+2B]`. |
| `GET_INDEX A B C` | ABC | `R[A] = R[B][R[C]]`. Array: int index, bounds-checked. Map: key lookup. |
| `SET_INDEX A B C` | ABC | `R[A][R[B]] = R[C]`. |

#### Host References

| Op | Fmt | Semantics |
|----|-----|-----------|
| `GET_FIELD A B C` | ABC | `R[A] = R[B].fields[C]`. `C` is the vtable slot index. Calls the host getter. |
| `SET_FIELD A B C` | ABC | `R[A].fields[B] = R[C]`. Calls the host setter. Read-only field = `ReadOnlyField` error. |

#### Control Flow

| Op | Fmt | Semantics |
|----|-----|-----------|
| `JMP sBx` | AsBx | `PC += sBx` |
| `JMP_IF A sBx` | AsBx | `if truthy(R[A]): PC += sBx` |
| `JMP_IF_NOT A sBx` | AsBx | `if falsy(R[A]): PC += sBx` |

Jumps are relative to the *next* instruction (PC after decode, before jump).

#### Functions

| Op | Fmt | Semantics |
|----|-----|-----------|
| `CALL A B C` | ABC | Call `R[A]` with args `R[A+1..A+B]`. `B` = arg count. Result stored in `R[A]`. Increment call depth. |
| `TAIL_CALL A B` | ABC | Same as `CALL` but reuses current frame. No call depth increment. |
| `RETURN A` | ABx | Return `R[A]` to caller. If `A = 255`, return `nil`. Decrement call depth. Pop frame, restore PC. |
| `CLOSURE A Bx` | ABx | `R[A] = new closure` from prototype `Bx`. Captures upvalues per prototype's upvalue descriptor list. |

**Call convention:** caller places function in `R[A]`, args in `R[A+1..A+B]`. Callee sees args in `R[0..B-1]` of its own window. Single return value placed in caller's `R[A]` after return.

#### Coroutines

| Op | Fmt | Semantics |
|----|-----|-----------|
| `COROUTINE A B` | ABC | `R[A] = new coroutine` wrapping closure `R[B]`. State: `suspended`. |
| `YIELD A` | ABx | Suspend current coroutine. `R[A]` is the yielded value. Control returns to resumer. |
| `RESUME A B` | ABC | Resume coroutine `R[A]` with value `R[B]`. Result (yielded or returned) stored in `R[A]`. Dead coroutine = `CoroutineDead` error. |

#### Error Handling

| Op | Fmt | Semantics |
|----|-----|-----------|
| `TRY A sBx` | AsBx | Execute next instruction. If it raises an error, jump `PC += sBx` with error value in `R[A]`. If no error, continue. |

`TRY` acts as a one-instruction error trap. The compiler emits:

```
TRY rErr +2      // if next instruction errors, jump +2 with error in rErr
CALL rFn ...     // the protected call
JMP +N           // skip the error handler (no error)
// error handler starts here, rErr has the error value
```

`try expr` compiles to `TRY` + the expression. `try expr else |e| { ... }` compiles to `TRY` + expression + `JMP` over handler + handler block.

`error(msg)` is a host-provided core function that raises a `ScriptError`.

## Error Types

Every runtime error is a `raido.ScriptError` with a `kind` enum and a message string:

| Kind | Cause |
|------|-------|
| `TypeError` | Wrong type for operation (e.g., `"a" + 1`) |
| `DivisionByZero` | Division or modulo by zero |
| `IndexOutOfBounds` | Array index < 0 or >= len |
| `KeyNotFound` | Map key doesn't exist (on read) |
| `ArenaExhausted` | Bump allocator exceeded arena size |
| `FuelExhausted` | Instruction budget reached zero |
| `CallOverflow` | Call depth exceeded max_call_depth |
| `CoroutineDead` | Resumed a finished coroutine |
| `ReadOnlyField` | Wrote to a read-only host ref field |
| `ScriptError` | Raised by `error(msg)` in script code |
| `HostError` | Raised by a host function |

All errors carry: kind, message (string), and a stack trace (array of `{file, line, function}` if debug info is present, `{proto_idx, pc_offset}` if not).

`FuelExhausted`, `ArenaExhausted`, and `CallOverflow` are **not catchable** by `TRY`. They propagate directly to the host as `raido.ScriptError`. Scripts can't mask resource exhaustion.

## Compilation

Single-pass compiler. Source → bytecode in one walk. No AST. Same architecture as Lua's compiler (lparser.c / lcode.c).

1. **Lexer.** Source → tokens. Newline-sensitive (statement terminator).
2. **Parser + codegen.** Recursive descent. Emits bytecode directly during parsing. Local variable tracking via a compile-time scope stack. Upvalue resolution by walking the scope chain.
3. **Register allocation.** Linear. Locals get sequential registers. Temporaries use a stack-like bump above locals. No optimization passes.
4. **Constant folding.** Compile-time evaluation of constant expressions (`1 + 2` → `LOAD_INT 3`). Only for arithmetic on literals.
5. **Output.** Chunk (see [chunk-format.md](chunk-format.md)).

No AST means no intermediate representation, no memory proportional to source size. The compiler holds the current token, the previous token, and the scope stack. Memory use is O(scope depth), not O(program size).
