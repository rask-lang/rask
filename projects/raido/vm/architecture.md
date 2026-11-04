<!-- id: raido.vm -->
<!-- status: proposed -->
<!-- summary: Deterministic register-based VM with static types, strict floats, a garbage-collected heap scripts can't observe, serializable state -->
<!-- depends: raido/language/types.md -->

# VM Architecture

Register-based bytecode VM. Deterministic. Serializable. Sandboxed. Lightweight -- the implementation should be small enough to audit by hand.

## Core Properties

- **Register-based.** Each call frame has a fixed-size register window. Instructions encode register operands directly.
- **Statically typed.** The compiler knows every register's type, so instructions never dispatch on type. Values still carry a small tag, for the collector, serialization and debugging.
- **Deterministic.** Strict IEEE floats, specified PRNG, insertion-ordered maps. Identical execution on every platform.
- **Serializable.** Entire state -> bytes -> restore, in a canonical form that's identical on every machine.
- **Send, not Sync.** Movable between threads, not shareable.
- **`@resource`** in Rask -- must be closed.

## Value Representation

**16 bytes per value: an 8-byte tag word and an 8-byte payload.** The compiler already knows every register's type, so no instruction reads the tag. The collector does, to find references, and so do serialization and the debugger.

| Tag | Payload |
|-----|---------|
| `int` | i64 |
| `float` | f64 (never NaN or infinite) |
| `bool` | 0 or 1 |
| `none` | depth: `None` is 0, `Some(None)` is 1, `Some(Some(None))` is 2 |
| `func` | prototype index (a function or a closure that captures nothing) |
| `enum` | discriminant (an enum with no payloads) |
| `ref` | reference to a heap object: string, array, map, struct, enum with payload, closure, coroutine |
| `extern` | the host's 64-bit id for one of its objects; the tag word also holds the extern struct's type index |

**Optionals cost nothing.** `Some(x)` of a value that isn't `none` is `x` itself. Wrapping a `none` adds one to its depth, and unwrapping takes one off, so `int??` keeps `None` and `Some(None)` apart without a box. 256 registers x 16 bytes = 4 KB per call frame. A frame's registers start as `none` (depth 0).

**Containers store payloads without tags.** An array or map holds one element type, so the tag is the same for every slot. The object records its element type once, and each slot is the 8-byte payload: an `array<int>` of 1,000 items is 8 KB, not 16. The collector reads the element type to know whether the slots are references. Element types that need the tag to tell two cases apart (an optional of a non-reference type such as `int?`, or a function value that may be a closure) keep 16-byte slots. An optional of a reference type uses a null payload for `None` and stays at 8 bytes; an optional of an optional keeps its tag. Struct fields, closure captures and coroutine registers keep their tags, since one object mixes types there.

## Determinism

`float` arithmetic is IEEE 754 binary64 under a strict profile, bitwise identical on all platforms: round-to-nearest, subnormals kept, no fused multiply-add, correctly rounded math functions (rask-lang/rask#1683). An operation that would produce NaN or an infinity raises `FloatError`, so registers only ever hold finite floats.

Integer overflow panics by default. `wrapping_add()`, `wrapping_mul()`, etc. wrap. Both behaviors are deterministic and part of the contract.

Determinism enables: lockstep networking, replay, migration, reproducible evaluation, audit trails.

### Formal Contract

- **Register-level equivalence.** Same bytecode + same inputs + same fuel -> identical register state at every instruction boundary.
- **Map iteration order** preserved across serialize/deserialize (insertion-ordered).
- **Coroutine resume sequence** is deterministic -- same yields in same order, same values.
- **Error kind and stack trace structure** are part of the contract. Exact error message text is not.
- **Fuel cost** is 1 per instruction, no exceptions. A built-in that calls back into script code (`sort`, `filter`) costs 1, plus the instructions of each callback. Fuel spent by a call that fails stays spent.
- **Memory exhaustion** is deterministic -- it depends on what's reachable at points fixed by the allocation count, not on when garbage was collected ([Memory Limit](#memory-limit)).
- **PRNG state evolution** is part of the contract ([PRNG](#prng)).
- **Float arithmetic** follows the strict profile above -- bitwise identical on all platforms. Conversions to and from text are exact too ([stdlib.md](../language/stdlib.md#core-functions)).
- **Integer overflow** panics by default. Wrapping ops wrap. Both deterministic, part of the contract.
- **Sort** is one algorithm, so the comparator sees the same calls in the same order everywhere, even when it's inconsistent: top-down merge sort. A run of 0 or 1 items is sorted. Otherwise split at `n // 2` (the left half gets the smaller part), sort the left then the right, and merge by calling `cmp(right_item, left_item)`: take the right item if it returns `true`, the left one otherwise. When either half runs out, the rest is copied without calls.

### Provable Profile

The primary verification mode is bilateral re-execution: both parties run the same script and compare outputs. Deterministic floats replay exactly, so this covers every script.

Zero-knowledge proofs are a future option for two cases re-execution can't handle:

- **Privacy.** Re-execution requires sharing inputs. A ZK proof shows "this output came from this script" without revealing them, so a domain can prove correct minting without exposing its crafting formula or stockpile.
- **Expensive scripts.** Large simulations where re-execution is costly and many parties verify.

A ZK proof works over arithmetic in a prime field: additions and multiplications of large integers. Integer operations map onto that cheaply, with range checks to enforce 64-bit wrapping. A float operation doesn't: unpacking sign, exponent and mantissa, aligning exponents by a variable shift, normalising, rounding to nearest-even and repacking are each cheap in hardware and expensive in a circuit. General-purpose zkVMs emulate floats in software for this reason, at a cost of about a hundred times an integer operation.

So proving is a profile, not the whole language. A chunk compiled as **provable** may not use `float`, so `/` is out and integer division is `//`; the compiler rejects the rest, and the chunk's header records the profile so a prover accepts only provable chunks. Minting, crafting, trades and anything conserved already count in `int`, so the scripts worth proving are the ones the profile allows.

The profile changes one other rule. A provable chunk never collects, so its memory limit counts everything it allocated since the last `reset()`, reachable or not. That's a different point of running out than a normal chunk's, which is why the profile is in the header and in the content hash: one hash never runs under two rules.

**What the provable profile relies on:**

| Property | Why it matters for ZK |
|---|---|
| Integer arithmetic only (i64) | Maps to field arithmetic with range checks. |
| Fuel-bounded execution | Circuits need bounded loops. Fuel guarantees a max step count. |
| Memory limit | Bounded memory = bounded memory-consistency constraints. |
| No collection inside a proof | Provable chunks run with `reset()` between evaluations and count every allocation against the limit, so no collector has to be proven. |
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
- State is part of VM state, serialized, and rolled back when a call fails.
- **Seeding.** `vm.seed(s)` runs SplitMix64 twice from `z = s`: each step does `z += 0x9E3779B97F4A7C15`, then `r = z`, `r = (r ^ (r >> 30)) * 0xBF58476D1CE4E5B9`, `r = (r ^ (r >> 27)) * 0x94D049BB133111EB`, `r = r ^ (r >> 31)`, all wrapping u64. The two results `r1`, `r2` give the state `[low32(r1), high32(r1), low32(r2), high32(r2)]`. A VM that's never seeded uses seed 0.
- **`random()`.** Two outputs `a`, `b`; the result is `((a >> 5) * 2^26 + (b >> 6)) * 2^-53`, which is exact in a double.
- **`random(n)`.** Two outputs make `x = (a << 32) | b`, a u64. Let `lim = 2^64 - (2^64 mod n)`; while `x >= lim`, draw a new `x`. The result is `x mod n`, unbiased, and it draws the same outputs everywhere.

Reference: Blackman & Vigna, 2018. The algorithm is fixed -- changing it is a breaking change to determinism.

## Heap

Strings, arrays, maps, structs, enums with payloads, closures and coroutines are heap objects. A value refers to one with a `ref`. The heap is garbage collected. String constants live in the chunk, not the heap, and cost nothing against the limit.

**Object kinds:**

| Kind | Holds |
|------|-------|
| String | immutable UTF-8 bytes |
| Array | length and values, growable |
| Map | entries in insertion order, keys `int` or `string` |
| Struct | its fields |
| Enum (with payload) | discriminant and payload fields |
| Closure | prototype index and captured values |
| Coroutine | status, call frames and their registers |

How objects are laid out in memory is the implementation's business. Nothing in the language or the serialized form depends on it.

### Roots

An object is **reachable** if a chain of references leads to it from a root. The roots are fixed so that reachability is the same on every implementation:

- every module variable of every loaded chunk;
- every host root handle, tasks included, in creation order;
- while a call runs: every register of every active frame, all of the prototype's registers, including temporaries the compiler has finished with (they hold the same stale values on every machine, since execution is identical);
- while a call runs: the undo log's old values, the arguments queued for deferred extern functions, and the extern write buffer, since rollback and commit still need them;
- the result of the host's last call, until the host's next call into the VM.

A value the host holds without rooting is not a root. Using one after it's collected is detected every time ([interop.md](interop.md#host-references)).

### Why the collector is invisible

A script can't observe when or whether the collector has run, so the VM can collect whenever it likes, on any machine, and results stay identical. That holds because the language exposes nothing that depends on it:

- **No finalizers.** Nothing runs when an object dies.
- **No weak references.** Reachability is all-or-nothing.
- **Nothing derived from addresses.** Maps hash keys by content (an `int`'s value, a `string`'s bytes), never by where an object lives; there's no identity hash and no ordering by identity. `==` on structs and other references compares identity, which is a yes-or-no answer, not an address.

Any collector satisfies this. Non-moving mark-and-sweep is the simple choice, and it keeps host references stable.

**Pauses.** A stop-the-world collection pauses in proportion to what's live, so a mod holding many megabytes could cause a frame hitch. Because collection can't be observed, an implementation is free to collect incrementally (a slice of marking per allocation or per frame) or generationally, without changing any result. The arena this replaced paid a comparable cost at each reset, freeing in bulk, so pauses aren't new; they're just no longer tied to a fixed moment.

### Memory Limit

The VM has a configured limit on **reachable** memory, measured in a logical size every implementation computes the same way:

| Object | Logical size |
|--------|--------------|
| String | 16 + byte length |
| Array | 16 + 8 per slot of capacity (16 for element types that keep their tag) |
| Map | 16 + 24 per slot of capacity, plus 8 for each of key and value that keeps its tag |
| Struct, enum payload, closure | 16 + 16 per field or capture |
| Coroutine | 64 + 16 per register in its frames |

**Capacity here is computed from the length**, not taken from the real buffer: the smallest power of two that's at least 4 and at least the number of items. So a `remove`, a rolled-back `push` or a save and load can't make two VMs disagree, whatever their real buffers hold. An object's **allocation size** is its logical size when it's made, plus the increase whenever it grows.

**When the limit is checked.** Checking reachability at every allocation would be exact and, near the limit, would run a full collection each time: quadratic and unmetered. So the limit is checked at **checkpoints** instead. The VM counts allocation sizes, and a checkpoint happens whenever the count since the last one reaches an eighth of the limit. At a checkpoint the reachable total must be at most the limit, or the call fails with `MemoryExhausted`. The count is part of the VM state and is serialized, so checkpoints fall at the same allocation everywhere.

An implementation needs to collect at a checkpoint only when what it knew was live after its last collection, plus what's been allocated since, could exceed the limit. Near the limit that's one full collection per eighth of the limit allocated, which is linear. Real memory stays under about 1.125 times the limit plus garbage.

**`reset()`** clears the heap and all tasks and coroutines, and runs every module's initializers again. Use it between independent evaluations.

## Resource Limits

Three runaway vectors, three limits:

### Fuel (instruction budget)

Every instruction costs 1 fuel. Reaching zero = `FuelExhausted` error.

```rask
let vm = raido.Vm.new(raido.Config {
    memory_limit: 256.kilobytes(),
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

Max call stack depth. Default 256. The depth is the number of frames on the running stack. `CALL` adds one and `RETURN` removes one. `RESUME` puts all of the coroutine's frames back on the stack, and `YIELD` takes them all off, so a coroutine paused five calls deep counts five while it runs and nothing while it's paused. Exceeding the limit is a `CallOverflow` error.

`TAIL_CALL` reuses the current frame -- doesn't increment.

### Memory

Reachable memory is limited at creation. Exceeding it = `MemoryExhausted` error. See [Memory Limit](#memory-limit).

## Serialization

`vm.serialize()` -> bytes. `Vm.deserialize(bytes)` -> restored VM.

Serialization happens between host calls, so the roots are the module variables and the host root handles ([Roots](#roots)).

**Captures:** each loaded chunk's hash and module variables, the root handles in creation order, the objects reachable from them, PRNG state (4 x u32), fuel remaining, and the allocation count since the last checkpoint.

**Canonical form.** Serialization walks the object graph depth first from the roots in a fixed order (module variables by chunk load order then declaration order, then root handles in creation order, then each object's slots in order), numbers each object the first time it's reached, and writes objects by number. Two VMs in the same state write the same bytes, whatever their memory layout or collection history. Unreachable objects aren't written.

**Extern references** are written as the extern struct's name and the host's 64-bit id. The host's ids have to mean the same thing after a load: entity ids, not pointers.

**Does not capture:** host function closures (by name), bytecode (re-loaded by host).

Format version in the header. Unknown version = `VersionMismatch` error on deserialize.

## Instruction Set

Instructions are 32 bits. Three formats:

```
ABC:  op(8) | A(8)  | B(8)  | C(8)       -- 3 register operands
ABx:  op(8) | A(8)  | Bx(16)             -- register + 16-bit unsigned index
AsBx: op(8) | A(8)  | sBx(16, signed)    -- register + 16-bit signed offset
```

Register operands are 8-bit (0--255). Constant/index operands are 16-bit (0--65535).

### Opcodes

~44 instructions. Each described with format, operands, and exact behavior.

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
| `DIV A B C` | ABC | `R[A] = R[B] / R[C]` on floats (division by zero = `DivisionByZero` error) |
| `IDIV A B C` | ABC | `R[A] = R[B] // R[C]` on ints, truncating toward zero |
| `MOD A B C` | ABC | `R[A] = R[B] % R[C]` (sign follows dividend) |
| `NEG A B` | ABC | `R[A] = -R[B]` |

**Result types:** `int OP int -> int`, except `DIV`, whose int operands the compiler converts first, so it always returns `float`. `float OP float -> float`. Mixed ops: compiler promotes int to float, result is `float`. Integer overflow panics. A float result that is NaN or infinite raises `FloatError`.

#### Comparison

Result stored in register as `bool`.

| Op | Fmt | Semantics |
|----|-----|-----------|
| `EQ A B C` | ABC | `R[A] = R[B] == R[C]`, by content or identity per [Equality](../language/types.md#equality) |
| `LT A B C` | ABC | `R[A] = R[B] < R[C]` |
| `LE A B C` | ABC | `R[A] = R[B] <= R[C]` |

`NE`, `GT`, `GE` not needed -- compiler emits `EQ` + `NOT`, or swaps operands.

#### Logic / Misc

| Op | Fmt | Semantics |
|----|-----|-----------|
| `NOT A B` | ABC | `R[A] = !R[B]` (boolean negation) |
| `LEN A B` | ABC | `R[A] = len(R[B])` (string byte length, array length, map length) |
| `CONCAT A B C` | ABC | `R[A] = tostring(R[B]) ++ tostring(R[C])` (allocates a string) |

#### Collections

| Op | Fmt | Semantics |
|----|-----|-----------|
| `NEW_ARRAY A B` | ABC | `R[A] = new array`. `B` items from `R[A+1..A+B]` as initial elements. |
| `NEW_MAP A B` | ABC | `R[A] = new map`. `B` key-value pairs from `R[A+1..A+2B]`. |
| `GET_INDEX A B C` | ABC | `R[A] = R[B][R[C]]`. Array: int index, bounds-checked. Map: key lookup, missing key = `KeyNotFound`. |
| `SET_INDEX A B C` | ABC | `R[A][R[B]] = R[C]`. Array: bounds-checked. Map: insert or replace. |

#### Structs

| Op | Fmt | Semantics |
|----|-----|-----------|
| `NEW_STRUCT A Bx` | ABx | Allocate struct of type `Bx`, fields from registers starting at `A+1`. |
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
| `CLOSURE A Bx` | ABx | `R[A] = new closure` of prototype `Bx`, capturing `R[A+1..A+n]` where `n` is the prototype's capture count. |
| `GET_CAPTURE A B` | ABC | `R[A] = capture B` of the running closure. |

#### Module Variables

| Op | Fmt | Semantics |
|----|-----|-----------|
| `GET_MODULE A Bx` | ABx | `R[A] = module variable Bx` of the running chunk. |
| `SET_MODULE A Bx` | ABx | `module variable Bx = R[A]`, logged for rollback. |

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
| `YIELD A` | ABx | Suspend the running coroutine or task with `R[A]` as the yielded value. On resume, `R[A]` receives the value passed in. |
| `RESUME A B` | ABC | Resume coroutine `R[A]` with value `R[B]`. `R[A]` = `Some(yielded)`, or `None` if it finished. Finished coroutine = `CoroutineDead` error. |

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
| `MemoryExhausted` | Reachable memory exceeded the configured limit |
| `FuelExhausted` | Instruction budget reached zero |
| `CallOverflow` | Call depth exceeded max_call_depth |
| `CoroutineDead` | Resumed a finished coroutine |
| `ModifiedDuringIteration` | Added or removed a map key inside `for` over that map |
| `HostError` | Raised by a host function, or a host misuse: a stale handle, a call into the VM from inside an extern |
| `ScriptError` | Raised by `error(msg)` in script code |

All errors carry: kind, message (string), and a stack trace (array of `{file, line, function}` if debug info is present, `{proto_idx, pc_offset}` if not).

Scripts can't catch any of these. An error unwinds the whole call stack to the host, which receives it as a `raido.ScriptError`, and the call's changes are rolled back ([interop.md](interop.md#calls-are-transactions)).

**Eliminated by static types:** `TypeError` (impossible by construction), `ReadOnlyField` (compiler rejects writes to `readonly` extern fields).

## Compilation

Two-pass compiler. No full AST. Memory is O(declarations), not O(program size).

1. **Declaration pass.** Scan for `struct`, `enum`, `extend` blocks, `const`, top-level `let`, `func` signatures, and `import` statements, and read the API module's declarations. Build a type table mapping names to types. Record function and method signatures but not bodies.
2. **Compile pass.** Recursive descent with type checking. Emit bytecode directly during parsing. Register allocation same as current (linear, locals sequential, temporaries bump). Type inference for locals uses forward flow: `let x = 42` -> x is `int`. No backward inference, no constraint solving.
3. **Register allocation.** Linear. Locals get sequential registers. Temporaries use a stack-like bump above locals. No optimization passes.
4. **Constant folding.** Compile-time evaluation of constant expressions (`1 + 2` -> `LOAD_INT 3`). Only for arithmetic on literals.
5. **Output.** Chunk (see [chunk-format.md](chunk-format.md)).

**Tail call constraint.** The compiler only emits `TAIL_CALL` when a call expression is in tail position -- the last expression in a function body, directly before an implicit or explicit `return`. A `TAIL_CALL` in non-tail position would corrupt the caller's frame, so the compiler must never emit one there.
