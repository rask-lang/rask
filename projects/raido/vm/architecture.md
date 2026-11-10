<!-- id: raido.vm -->
<!-- status: proposed -->
<!-- summary: Deterministic register-based VM with static types, strict floats, a garbage-collected heap scripts can't observe, serializable state -->
<!-- depends: raido/language/types.md -->

# VM Architecture

Register-based bytecode VM. Deterministic. Serializable. Sandboxed. Lightweight -- the implementation should be small enough to audit by hand.

## Core Properties

- **Register-based.** Each call frame has a fixed-size register window. Instructions encode register operands directly.
- **Statically typed.** Every register has one type for the whole function, recorded in the chunk, and the VM verifies bytecode against those types before running it. Arithmetic never looks at a value's tag. Values carry one anyway, for the collector, serialization and `CALL`.
- **Deterministic.** Strict IEEE floats, specified PRNG, insertion-ordered maps. Identical execution on every platform.
- **Serializable.** Entire state -> bytes -> restore, in a canonical form that's identical on every machine.
- **Send, not Sync.** Movable between threads, not shareable.
- **`@resource`** in Rask -- must be closed.

## Value Representation

**16 bytes per value: an 8-byte tag word and an 8-byte payload.** The verified register types mean arithmetic and field access never read the tag. The collector does, to find references; so do serialization and the debugger; and `CALL` does, to tell a plain function from a closure.

| Tag | Payload |
|-----|---------|
| `int` | i64 |
| `float` | f64 (never NaN or infinite) |
| `bool` | 0 or 1 |
| `none` | depth: `None` is 0, `Some(None)` is 1, `Some(Some(None))` is 2 |
| `func` | prototype index (a function or a closure that captures nothing) |
| `enum` | discriminant (an enum with no payloads) |
| `ref` | reference to a heap object: string, array, map, struct, tuple, enum with payload, closure, cell, task |
| `const` | a string constant in a loaded chunk: chunk load position in the high 32 bits, constant index in the low |
| `extern` | the host's 64-bit id for one of its objects; the tag word also holds the extern struct's type index |

**Optionals cost nothing.** `Some(x)` of a value that isn't `none` is `x` itself. Wrapping a `none` adds one to its depth, and unwrapping takes one off, so `int??` keeps `None` and `Some(None)` apart without a box. 256 registers x 16 bytes = 4 KB per call frame. A frame's registers start as `none` (depth 0); the verifier makes sure none is read before it's written.

**Containers store payloads without tags.** An array or map holds one element type, so the tag is the same for every slot. The object records its element type once, and each slot is the 8-byte payload: an `array<int>` of 1,000 items is 8 KB, not 16. The collector reads the element type to know whether the slots are references. Element types that need the tag to tell two cases apart keep 16-byte slots: an optional of a non-reference type such as `int?`, any function type, `string` (a heap string or a constant), and an enum type with at least one payload variant (its other variants are plain discriminants). An optional of a reference type uses a null payload for `None` and stays at 8 bytes; an optional of an optional keeps its tag. Struct fields, closure captures and a paused task's registers keep their tags, since one object mixes types there.

## Determinism

`float` arithmetic is IEEE 754 binary64 under a strict profile, bitwise identical on all platforms: round-to-nearest, subnormals kept, no fused multiply-add, correctly rounded math functions (rask-lang/rask#1683). An operation that would produce NaN or an infinity raises `FloatError`, so registers only ever hold finite floats.

Integer overflow panics by default. `wrapping_add()`, `wrapping_mul()`, etc. wrap. Both behaviors are deterministic and part of the contract.

Determinism enables: lockstep networking, replay, migration, reproducible evaluation, audit trails.

### Formal Contract

- **Register-level equivalence.** Same bytecode + same inputs + same config (fuel, memory limit, call depth) -> identical register state at every instruction boundary. Two parties re-executing a script agree on the config as part of the inputs.
- **Map iteration order** preserved across serialize/deserialize (insertion-ordered).
- **Pause sequence** is deterministic -- the same pausing calls in the same order, with the same arguments.
- **Error kind and stack trace** are part of the contract: the trace is each frame's chunk, prototype index and instruction offset. Function names and source lines come from debug info and aren't. Exact message text isn't either.
- **Fuel cost** is in [Fuel](#fuel-instruction-budget). Fuel spent by a call that fails stays spent.
- **Memory exhaustion** is deterministic -- it depends on what's reachable at points fixed by the allocation count, not on when garbage was collected ([Memory Limit](#memory-limit)).
- **PRNG state evolution** is part of the contract ([PRNG](#prng)).
- **Float arithmetic** follows the strict profile above -- bitwise identical on all platforms. Conversions to and from text are exact too ([stdlib.md](../language/stdlib.md#core-functions)).
- **Integer overflow** panics by default. Wrapping ops wrap. Both deterministic, part of the contract.
- **Sort** is one algorithm, so the comparator sees the same calls in the same order everywhere, even when it's inconsistent: top-down merge sort. A run of 0 or 1 items is sorted. Otherwise split at `n // 2` (the left half gets the smaller part), sort the left then the right, and merge by calling `cmp(right_item, left_item)`: take the right item if it returns `true`, the left one otherwise. When either half runs out, the rest is copied without calls.

### Profiles

Raido is verification-first, but most of what makes it pleasant for mods (state that lives between calls, closures that share a counter, pausing, spawning, writing to the world) is exactly what makes a result depend on more than its inputs. So every chunk is compiled in one of three profiles, recorded in its header and in its hash, and the strict ones are enforced by the compiler and again by the verifier on bytecode, so a chunk can't claim a profile it doesn't keep.

| Profile | For | Rules |
|---------|-----|-------|
| **Normal** | mods, game logic | the whole language |
| **Pure** | scripts someone else re-executes: Allgard transforms, GDL client scripts | a call's result depends only on its arguments, the config and immediate host functions, and nothing it does outlasts the call |
| **Provable** | scripts that may be proved with zero knowledge | pure, plus no `float`, plus the no-collection memory rule below |

A **pure** chunk may not have:

- module variables (a top-level `let`; `const` is fine), its own or an import's;
- a captured variable that's reassigned, so no cells: a closure only sees values;
- calls to pausing or deferred externs, `spawn`, or writes to extern struct fields;
- `random()` or `random(n)`, since the PRNG is state a checker would have to reproduce. A script that needs randomness takes a seed as an argument;
- imports of chunks that aren't pure or provable.

It may call immediate externs and read extern struct fields; the checker has to have the same host API, and a protocol can narrow that further (Allgard allows none). `print` is allowed, since it's a log. The verifier checks the same rules on bytecode: no `GET_MODULE` or `SET_MODULE` on a variable, no `NEW_CELL`, `SPAWN` or `SET_FIELD`, no `CALL_EXTERN` to a deferred or pausing extern, no `CALL_BUILTIN` to `random`.

A normal VM loads normal and pure chunks, and runs both under the same rules, so a mod can import a pure library. A provable VM loads only provable chunks, because its memory rule differs.

The primary verification mode is bilateral re-execution: both parties run the same script and compare outputs. Deterministic floats replay exactly, so this covers every script.

Zero-knowledge proofs are a future option for two cases re-execution can't handle:

- **Privacy.** Re-execution requires sharing inputs. A ZK proof shows "this output came from this script" without revealing them, so a domain can prove correct minting without exposing its crafting formula or stockpile.
- **Expensive scripts.** Large simulations where re-execution is costly and many parties verify.

A ZK proof works over arithmetic in a prime field: additions and multiplications of large integers. Integer operations map onto that cheaply, with range checks to enforce 64-bit wrapping. A float operation doesn't: unpacking sign, exponent and mantissa, aligning exponents by a variable shift, normalising, rounding to nearest-even and repacking are each cheap in hardware and expensive in a circuit. General-purpose zkVMs emulate floats in software for this reason, at a cost of about a hundred times an integer operation.

So proving is a profile, not the whole language. A chunk compiled as **provable** keeps every pure rule and may not use `float`, so `/` is out and integer division is `//`; the compiler rejects the rest, and the chunk's header records the profile so a prover accepts only provable chunks. Minting, crafting, trades and anything conserved already count in `int`, so the scripts worth proving are the ones the profile allows.

The profile changes one other rule. A provable VM never collects, so at each checkpoint its limit counts everything allocated since the last `reset()`, reachable or not. That's a different point of running out than a normal VM's, which is why the profile is in the chunk header and in the content hash, and why a provable VM loads only provable chunks, imports included. One hash never runs under two rules. A provable chunk can't use `float` from the API module either.

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

Strings, arrays, maps, structs, tuples, enums with payloads, closures and tasks are heap objects. A value refers to one with a `ref`. The heap is garbage collected. String constants live in the chunk, not the heap: a `const` value, costing nothing against the limit.

**Object kinds:**

| Kind | Holds |
|------|-------|
| String | immutable UTF-8 bytes |
| Array | length and values, growable |
| Map | entries in insertion order, keys `int` or `string` |
| Struct, tuple | its fields; a tuple's can't change |
| Enum (with payload) | discriminant and payload fields |
| Closure | prototype index and captured values |
| Cell | one value: a captured variable that's assigned after its `let`, shared by the function and its closures |
| Task | a hook that's paused or spawned and not yet started: status, call frames and their registers |

How objects are laid out in memory is the implementation's business. Nothing in the language or the serialized form depends on it.

### Roots

An object is **reachable** if a chain of references leads to it from a root. The roots are fixed so that reachability is the same on every implementation:

- every module variable of every loaded chunk;
- every live host root handle (tasks are root handles), in creation order;
- tasks spawned by committed runs that the host hasn't collected with `vm.spawned()` yet, in spawn order;
- the result of the host's last call, and values the host made for arguments (`vm.new_string(...)`), until the host's next call starts. Once it starts, the arguments it was given are in registers;
- while a call runs: every register of every active frame, all of the prototype's registers;
- while a call runs: the undo log (the objects it writes back into, and their old values), the arguments of queued deferred calls, and the queued spawns.

A built-in that runs a callback (`sort`, `filter`) keeps nothing out of sight of the collector: the container it works on is in a register, and the callback can't change it (that's `ModifiedDuringIteration`).

A value the host holds without rooting is not a root ([interop.md](interop.md#host-references)).

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
| Map | 16 + 24 per slot of capacity, or 32 when the value type keeps its tag (keys never do) |
| Struct, tuple, enum payload, closure | 16 + 16 per field or capture |
| Cell | 32 |
| Task | 64 + 16 per register in its saved frames |

**Capacity here is computed from the length**, not taken from the real buffer: the smallest power of two that's at least 4 and at least the number of items. So a `remove`, a rolled-back `push` or a save and load can't make two VMs disagree, whatever their real buffers hold.

**What counts as an allocation.** The checkpoints below depend on counting allocations the same way everywhere, so the list is closed:

- making a string, array, map, struct, tuple, enum with payload, closure or cell (`NEW_*`, `CLOSURE`, `NEW_CELL`, `CONCAT`): its logical size;
- a built-in whose result is a new string, array or map: one allocation of the result at its final size, even when it equals an input (`lower()` on a lowercase string still allocates);
- an array or map growing: the increase in logical size;
- a string entering from the host (a getter, an extern's result, a resume value, `vm.new_string`): its logical size, each time;
- `SPAWN`: a task whose saved frame is the entry frame of the function it will run;
- a task pausing: the increase in its logical size, if its saved frames are bigger than before. A running task's frames are registers and don't count as part of the task.

Nothing else allocates: not a built-in's intermediate values or work space, not iteration, not the undo log. An implementation may share identical strings internally, but counts as if it didn't.

**When the limit is checked.** Checking reachability at every allocation would be exact and, near the limit, would run a full collection each time: quadratic and unmetered. So the limit is checked at **checkpoints** instead. The VM counts allocation sizes since the last checkpoint. When an allocation brings the count to an eighth of the limit or more, a checkpoint happens right after it and the count goes back to zero; one allocation makes at most one checkpoint. At a checkpoint the reachable total, counting the object just made or grown, must be at most the limit, or the call fails with `MemoryExhausted`. A single allocation bigger than the whole limit fails at once, before anything is allocated. The count is part of the VM state and is serialized. Like fuel, it isn't rolled back when a call fails.

An implementation needs to collect at a checkpoint only when what it knew was live after its last collection, plus what's been allocated since, could exceed the limit. Near the limit that's one full collection per eighth of the limit allocated, which is linear. Real memory stays under about 1.125 times the limit plus garbage.

**`reset()`** clears the heap, every root handle (tasks included) and the spawn queue, re-seeds the PRNG with the last seed, sets the allocation count to zero, and runs every module's initializers again as one run. Fuel and config are kept. Use it between independent evaluations.

## Resource Limits

Three runaway vectors, three limits:

### Fuel (instruction budget)

Every instruction costs 1 fuel. An instruction whose work grows with its data costs more: `CALL_BUILTIN`, `CONCAT`, `NEW_ARRAY`, `NEW_MAP` and `EQ` cost 1 plus the size of every string, array or map they take or produce, where a string's size is its byte length divided by 8, rounded up, an array's is its length and a map's is its entry count. A result whose size is known before the work starts (`rep`, `substring`, `CONCAT`) is charged before the work, so `"x".rep(10_000_000_000)` runs out of fuel instead of allocating. A built-in that runs a callback also pays for the callback's own instructions. Reaching zero = `FuelExhausted` error.

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

Fuel is serialized and shared by everything the VM runs. A host gives each call its budget with `vm.set_fuel` before it. For deterministic replay, hosts must set fuel at the same points.

### Call Depth

Max call stack depth. Default 256. The depth is the number of frames on the running stack. `CALL` adds one and `RETURN` removes one. Resuming a task puts all of its frames back on the stack, and pausing takes them all off, so a task paused five calls deep counts five while it runs and nothing while it's paused. Exceeding the limit is a `CallOverflow` error.

`TAIL_CALL` reuses the current frame and doesn't increment. The reused frame starts fresh, as a new call's would: every register is `none` before the arguments go in.

### Memory

Reachable memory is limited at creation. Exceeding it = `MemoryExhausted` error. See [Memory Limit](#memory-limit).

## Serialization

`vm.snapshot()` -> bytes. `Vm.restore(bytes, chunks, bindings)` -> restored VM, given the chunks the snapshot names by hash. Serialization happens between host calls.

**Captures:** the config (memory limit, call depth, profile), each loaded chunk's hash in load order, module variables, the root handles in creation order (dead ones too, so a root index means the same root after a load), the spawn queue in order, the objects reachable from all of these, PRNG state, fuel remaining, and the allocation count since the last checkpoint. The host finds its tasks again by their root indices.

**Canonical form.** Serialization walks the object graph depth first from the roots in a fixed order (module variables by chunk load order then declaration order, then root handles in creation order, then the spawn queue, then each object's slots in order), numbers each object the first time it's reached, and writes objects by number. Two VMs in the same state write the same bytes, whatever their memory layout or collection history. Unreachable objects aren't written.

**Bytes.** Integers are little-endian. After a header (`RDSV`, the format version, the config, PRNG, fuel and allocation count) come the chunk hashes, then the module variables, roots, spawn queue and objects, each list prefixed with a u32 count.

- A **value** is its 16 bytes: the tag word, then the payload. A `ref`'s payload is the object's number. An uninitialized module variable is the tag word 0. A dead root is a single 0 byte; a live one is a 1 byte and then its value.
- A **function** is named by a u32: chunk load position in the high 16 bits, prototype index in the low 16.
- An **object** is a kind byte, then:
  - string: u32 byte length, bytes;
  - array: u32 element count, values;
  - map: u32 entry count, then key and value for each entry in insertion order;
  - struct or tuple: u32 type index, values;
  - enum with payload: u32 type index, u32 variant, values;
  - closure: the function, values;
  - cell: its value;
  - task: status byte (0 not started, 1 paused), u32 frame count, then each frame outermost first: the function, u32 instruction offset, u8 result register, u32 register count, registers.

Containers are written with every value's full 16 bytes, whatever they store in memory.

**Extern references** are written as values too: the tag word holds the extern struct's type index in the API module, and the payload is the host's 64-bit id. The host's ids have to mean the same thing after a load: entity ids, not pointers.

**Does not capture:** host function bindings, bytecode (the host supplies the chunks).

Format version in the header. Unknown version = `VersionMismatch` error on deserialize.

## Instruction Set

Instructions are 32 bits. Three formats:

```
ABC:  op(8) | A(8)  | B(8)  | C(8)       -- 3 register operands
ABx:  op(8) | A(8)  | Bx(16)             -- register + 16-bit unsigned index
AsBx: op(8) | A(8)  | sBx(16, signed)    -- register + 16-bit signed value
```

Register operands are 8-bit (0--255). Index operands are 16-bit (0--65535). Opcodes are numbered from 0 in the order of the tables below.

Indexes in `Bx` point into the chunk's own tables ([chunk-format.md](chunk-format.md)): constants, types, enum variants, functions, module variables, built-ins and externs. The function and module-variable tables list imported ones too, resolved at load, so `combat.hits` is a `GET_MODULE` like any other.

### Load and Move

| Op | Fmt | Semantics |
|----|-----|-----------|
| `LOAD_TRUE A` | ABx | `R[A] = true` |
| `LOAD_FALSE A` | ABx | `R[A] = false` |
| `LOAD_INT A sBx` | AsBx | `R[A] = sBx` |
| `LOAD_CONST A Bx` | ABx | `R[A] = K[Bx]`: an int, a float, or a string constant |
| `LOAD_NONE A` | ABx | `R[A] = None` |
| `MOVE A B` | ABC | `R[A] = R[B]` |

### Arithmetic

Int and float have their own instructions. `int` meeting `float` goes through `ITOF` first.

| Op | Fmt | Semantics |
|----|-----|-----------|
| `ADD_I`, `SUB_I`, `MUL_I` `A B C` | ABC | `R[A] = R[B] op R[C]` on ints. Overflow = `IntegerOverflow` |
| `IDIV A B C` | ABC | `R[A] = R[B] // R[C]`, truncating. Zero = `DivisionByZero`; `MIN // -1` = `IntegerOverflow` |
| `MOD_I A B C` | ABC | `R[A] = R[B] % R[C]`, sign of the dividend. Zero = `DivisionByZero`; `MIN % -1` is 0 |
| `NEG_I A B` | ABC | `R[A] = -R[B]`. `-MIN` = `IntegerOverflow` |
| `ADD_F`, `SUB_F`, `MUL_F`, `DIV_F`, `MOD_F` `A B C` | ABC | `R[A] = R[B] op R[C]` on floats. A zero divisor = `DivisionByZero`, checked first; any other non-finite result = `FloatError`. `MOD_F` is the IEEE `fmod`, sign of the dividend |
| `NEG_F A B` | ABC | `R[A] = -R[B]` |
| `ITOF A B` | ABC | `R[A] = float(R[B])`, nearest, ties to even |

### Comparison and Logic

| Op | Fmt | Semantics |
|----|-----|-----------|
| `EQ A B C` | ABC | `R[A] = R[B] == R[C]`, by content or identity per [Equality](../language/types.md#equality); both operands have the same type |
| `LT_I`, `LE_I`, `LT_F`, `LE_F`, `LT_S`, `LE_S` `A B C` | ABC | `R[A] = R[B] < R[C]` or `<=` on ints, floats, or strings by bytes |
| `NOT A B` | ABC | `R[A] = !R[B]` |
| `LEN A B` | ABC | `R[A]` = byte length, array length or map entry count of `R[B]` |
| `CONCAT A B C` | ABC | `R[A]` = the `C` strings `R[B..B+C-1]` joined, as one new string |

`!=`, `>` and `>=` are `EQ` then `NOT`, or the operands swapped.

### Collections

| Op | Fmt | Semantics |
|----|-----|-----------|
| `NEW_ARRAY A B C` | ABC | `R[A]` = new array of the `B` items `R[A+1..A+B]`. `C` is 1 when the element type keeps its tag |
| `NEW_MAP A B C` | ABC | `R[A]` = new map of the `B` pairs in `R[A+1..A+2B]`. `C` is 1 when the value type keeps its tag |
| `GET_INDEX A B C` | ABC | `R[A] = R[B][R[C]]`. Array: bounds-checked. Map: missing key = `KeyNotFound` |
| `SET_INDEX A B C` | ABC | `R[A][R[B]] = R[C]`. Array: bounds-checked. Map: insert or replace |
| `MAP_KEY_AT A B C` | ABC | `R[A]` = the key at position `R[C]` of map `R[B]`, in insertion order |
| `MAP_VALUE_AT A B C` | ABC | `R[A]` = the value at position `R[C]` |
| `MAP_VERSION A B` | ABC | `R[A]` = how many keys have been added to or removed from `R[B]` since it was made |

`for` compiles to these. Over an array it reads `R[i]` while `i` is below the current length. Over a map it records `MAP_VERSION` at the start and compares before each step; a difference is `ModifiedDuringIteration`. That holds across a pause too: if another hook adds a key while the loop waits, the next step fails.

### Structs, Tuples and Enums

| Op | Fmt | Semantics |
|----|-----|-----------|
| `NEW_STRUCT A Bx` | ABx | `R[A]` = new struct or tuple of type `Bx`, fields from `R[A+1..]` |
| `GET_STRUCT_FIELD A B C` | ABC | `R[A] = R[B].fields[C]`, struct or tuple |
| `SET_STRUCT_FIELD A B C` | ABC | `R[A].fields[B] = R[C]`, struct only |
| `NEW_ENUM A Bx` | ABx | `R[A]` = variant `Bx` from the chunk's variant table, payload from `R[A+1..]`. A variant without a payload is an `enum` value, not an allocation |
| `ENUM_TAG A B` | ABC | `R[A]` = the variant number of `R[B]` |
| `GET_PAYLOAD A B C` | ABC | `R[A]` = payload field `C` of `R[B]` |

### Host Struct Fields

| Op | Fmt | Semantics |
|----|-----|-----------|
| `GET_FIELD A B C` | ABC | `R[A]` = field `C` of the extern struct `R[B]`, through the host's getter |
| `SET_FIELD A B C` | ABC | field `B` of the extern struct `R[A]` = `R[C]`, through the host's setter, logged for rollback ([interop.md](interop.md#calls-are-transactions)) |

### Control Flow

| Op | Fmt | Semantics |
|----|-----|-----------|
| `JMP sBx` | AsBx | `PC += sBx` |
| `JMP_IF A sBx` | AsBx | `if R[A]: PC += sBx` |
| `JMP_IF_NOT A sBx` | AsBx | `if !R[A]: PC += sBx` |

Jumps are relative to the next instruction.

### Functions

| Op | Fmt | Semantics |
|----|-----|-----------|
| `CALL A B` | ABC | Call `R[A]` with args `R[A+1..A+B]`; the result goes in `R[A]` |
| `TAIL_CALL A B` | ABC | The same, reusing the current frame, and ending this function: its result is this function's result |
| `RETURN A` | ABx | Return `R[A]` |
| `RETURN_VOID` | ABx | Return nothing |
| `FUNC_REF A Bx` | ABx | `R[A]` = function `Bx` |
| `CLOSURE A Bx` | ABx | `R[A]` = new closure of function `Bx`, capturing `R[A+1..A+n]`, where `n` is its capture count |
| `GET_CAPTURE A B` | ABC | `R[A]` = capture `B` of the running closure |
| `NEW_CELL A B` | ABC | `R[A]` = new cell holding `R[B]` |
| `GET_CELL A B` | ABC | `R[A]` = the value in cell `R[B]` |
| `SET_CELL A B` | ABC | the value in cell `R[A]` = `R[B]`, logged for rollback |
| `GET_MODULE A Bx` | ABx | `R[A]` = module variable `Bx`. Reading one whose initializer hasn't run = `Uninitialized` |
| `SET_MODULE A Bx` | ABx | module variable `Bx` = `R[A]`, logged for rollback |

The callee sees its arguments in `R[0..B-1]` of its own window.

A variable that a closure captures and that's assigned after its `let` lives in a cell from its declaration on: the register holds the cell, reads and writes go through `GET_CELL` and `SET_CELL`, and `CLOSURE` captures the cell. A loop makes a new cell each iteration, so each iteration's closures share their own. A cell's register type is `cell<T>`, so the verifier checks the value type through it.

### Optionals

| Op | Fmt | Semantics |
|----|-----|-----------|
| `IS_SOME A B` | ABC | `R[A] = R[B] is Some` |
| `UNWRAP A B` | ABC | `R[A]` = the value inside `R[B]`. `None` = `UnwrapNone` |
| `WRAP_SOME A B` | ABC | `R[A] = Some(R[B])` |

### Built-ins and the Host

| Op | Fmt | Semantics |
|----|-----|-----------|
| `CALL_BUILTIN A Bx` | ABx | Call built-in `Bx` with its arguments in `R[A+1..]`; the result goes in `R[A]` |
| `CALL_EXTERN A Bx` | ABx | Call extern `Bx` the same way. A deferred extern is queued instead. A pausing extern pauses the task; on resume, `R[A]` gets the value the host passed |
| `SPAWN A B` | ABC | Queue a new task that will run `R[A]` with args `R[A+1..A+B]`; handed to the host if the run commits |

A built-in or extern takes as many arguments as its signature says.

## Error Types

Every runtime error is a `raido.ScriptError` with a `kind` enum and a message string:

| Kind | Cause |
|------|-------|
| `DivisionByZero` | Integer or float division or remainder by zero |
| `IntegerOverflow` | Integer arithmetic or conversion out of range |
| `FloatError` | A float operation produced NaN or an infinity |
| `IndexOutOfBounds` | Array index, or string offset, outside the valid range |
| `KeyNotFound` | Map key doesn't exist (on direct access) |
| `UnwrapNone` | Force unwrap (`!`) on `None` |
| `ArgumentError` | A built-in got an argument outside what it accepts: `random(0)`, `rep(-1)`, a shift outside 0..63, `split("")`, a `substring` that cuts a character |
| `Uninitialized` | A module variable read before its initializer ran |
| `ModifiedDuringIteration` | A map gained or lost a key during `for` over it, or a `sort`/`filter` callback changed the array it's working on |
| `PauseNotAllowed` | A pausing extern called in a built-in's callback or a module initializer |
| `MemoryExhausted` | Reachable memory exceeded the configured limit |
| `FuelExhausted` | Instruction budget reached zero |
| `CallOverflow` | Call depth exceeded max_call_depth |
| `HostError` | Raised by a host function, or a host misuse: a stale handle, a value of the wrong type, a call into the VM from inside an extern, resuming a finished task |
| `ScriptError` | Raised by `error(msg)` in script code |

All errors carry: kind, message (string), and a stack trace: chunk, prototype index and instruction offset per frame, plus file, line and function name when debug info is present.

Scripts can't catch any of these. An error unwinds the whole call stack to the host, which receives it as a `raido.ScriptError`, and the call's changes are rolled back ([interop.md](interop.md#calls-are-transactions)).

**Eliminated by static types and verification:** `TypeError` and `ReadOnlyField` (the compiler rejects writes to `readonly` extern fields, the verifier rejects them in bytecode).

## Verification

Bytecode can come from someone else: a trading partner's chunk, a mod downloaded as bytes. `raido.Chunk.decode` verifies it before anything runs, so a chunk that passes can't make the VM misread a value, whoever wrote it. It's one pass per function:

- every instruction's operands have the types its row above requires, using the function's register types: `ADD_I` takes two `int` registers, `GET_STRUCT_FIELD` a struct register and a field index inside it, and so on;
- `CALL`, `TAIL_CALL`, `CALL_BUILTIN` and `CALL_EXTERN` pass as many arguments as the callee takes, of its parameter types, into a register of its result type;
- every register is written on every path before it's read (a forward dataflow over the function's jumps), so a register's starting `none` is never seen;
- jumps land inside the same function, and no path runs off its end: every path ends in `RETURN`, `RETURN_VOID` or `TAIL_CALL`, with a value of the function's return type;
- every index is inside its table;
- a pure or provable chunk keeps its profile's rules ([Profiles](#profiles)), and a provable one has no float instructions or constants.

A chunk compiled from source passes by construction; the compiler runs the same check in debug builds.

## Compilation

Two-pass compiler. No full AST. Memory is O(declarations), not O(program size).

1. **Declaration pass.** Scan for `struct`, `enum`, `extend` blocks, `const`, top-level `let`, `func` signatures, and `import` statements, and read the API module's declarations. Build a type table mapping names to types. Record function and method signatures but not bodies.
2. **Compile pass.** Recursive descent with type checking. Emit bytecode directly during parsing. Type inference for locals uses forward flow: `let x = 42` -> x is `int`. A closure's parameter types come from the type expected where it's written (an argument, an annotated variable); with no expected type they must be written out. No constraint solving.
3. **Register allocation.** Linear. Locals get sequential registers, temporaries a stack-like bump above them. A register keeps one type for the whole function, so a temporary of a new type takes a fresh register instead of reusing one. No optimization passes.
4. **Constant folding.** Compile-time evaluation of constant expressions (`1 + 2` -> `LOAD_INT 3`). Only for arithmetic on literals.
5. **Output.** Chunk (see [chunk-format.md](chunk-format.md)).

**Tail calls.** The compiler emits `TAIL_CALL` for a call in tail position: `return f(x)`, or the last expression of a function body.
