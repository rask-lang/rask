# Raido in Rask — Implementation Plan

Raido VM implemented in Rask, for correctness and dogfooding. Not the eventual
production implementation (that should be Rust for the 1 KB base target), but
the reference that proves the spec works and stress-tests Rask's type system,
enums, generics, and collections.

## Prerequisites

**One compiler change needed before starting:**

Add `fs.read_bytes(path) -> Vec<u8>` and `fs.write_bytes(path, bytes: Vec<u8>)`
to `rask-interp`'s stdlib. Without this, serialization and chunk loading are
impossible. Everything else has workarounds.

**Known constraints (work around, don't block on):**

- `Vec<u8>` stores each byte as `Value::Int(i64)` — 8x overhead. Correct but
  wasteful. Fix in the compiler later with a native `Value::U8` variant.
- No FFI. Host functions are Rask closures. This is fine — Raido-in-Rask is
  Rask-hosted by definition.

## File Structure

```
raido/
  value.rk        # Value enum, fixed-point Number type
  arena.rk        # Typed arena allocator
  chunk.rk        # Bytecode chunk representation
  opcodes.rk      # Instruction encoding/decoding
  lexer.rk        # Tokenizer
  ast.rk          # AST node types
  parser.rk       # Recursive descent parser
  compiler.rk     # AST → bytecode
  vm.rk           # Execution engine
  stdlib.rk       # Built-in functions
  main.rk         # CLI entry point
```

## Phases

### Phase 0: Skeleton

Create all files with stub types. Verify `rask check` passes on everything.

### Phase 1: Values and fixed-point arithmetic

Value enum and 32.32 fixed-point Number type.

```rask
struct Number { raw: i64 }

enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Num(Number),
    Str(string),
    Array(i64),     // arena index
    Map(i64),       // arena index
    Func(i64),      // prototype index
    HostRef(i64),   // opaque ID
}
```

Fixed-point operations — all integer math + bit shifts:
- Add/sub: just add/sub the raw i64
- Mul: `((a as i128) * (b as i128)) >> 32` (need i128 or split into hi/lo)
- Div: `((a as i128) << 32) / (b as i128)`
- Comparisons: direct i64 comparison
- Conversions: `int_to_num(n)` = `n << 32`, `num_to_int(n)` = `n >> 32`

**Test:** Fixed-point math round-trips, overflow saturation, division by zero
raises error.

**Risk:** i128 arithmetic. If Rask doesn't support it, split mul/div into 32-bit
halves using bitwise ops. Verify early.

### Phase 2: Instruction encoding

37 opcodes encoded as i64, decoded with bitwise ops.

Three formats (32-bit instructions stored in i64):
- **ABC**: `opcode(6) | A(8) | B(9) | C(9)`
- **ABx**: `opcode(6) | A(8) | Bx(18)`
- **AsBx**: `opcode(6) | A(8) | sBx(18, signed)`

```rask
func encode_abc(op: i64, a: i64, b: i64, c: i64) -> i64 {
    return (op << 26) | (a << 18) | (b << 9) | c
}
```

Define all 37 opcodes as constants (or an enum if Rask supports integer-backed
enums, otherwise `const OP_LOAD_NIL: i64 = 0` etc).

**Test:** Round-trip encode/decode for every format.

### Phase 3: Arena

Flat byte buffer — `Vec<u8>` with bump allocation, integer offsets, and
byte-level encode/decode. This matches the spec's design directly.

```rask
struct Arena {
    buf: Vec<u8>,
    top: i64,
    capacity: i64,
    frame_base: i64,
}
```

Arena offsets are integers — same relationship to the byte buffer as Handles to
a Pool. Bounds-checked on access. Type tags in object headers catch mismatches
at runtime.

**Byte encoding helpers** (small set, used everywhere):

```rask
func write_u8(buf: Vec<u8>, offset: i64, val: u8)
func read_u8(buf: Vec<u8>, offset: i64) -> u8
func write_u32le(buf: Vec<u8>, offset: i64, val: i64)
func read_u32le(buf: Vec<u8>, offset: i64) -> i64
func write_i64le(buf: Vec<u8>, offset: i64, val: i64)
func read_i64le(buf: Vec<u8>, offset: i64) -> i64
```

**Object layout** (matches spec):

Each object starts with a 4-byte header: `type_tag(u8) | padding(u8) | size(u16)`.

- **String**: `header(4) | len(4) | utf8_bytes[len]`
- **Array**: `header(4) | len(4) | cap(4) | values[cap]` (16 bytes per value)
- **Map**: `header(4) | len(4) | cap(4) | entries[cap]` (open addressing)
- **Closure**: `header(4) | proto_idx(2) | upvalue_count(2) | upvalue_offsets[](4)`
- **Upvalue**: `header(4) | value(16)`

**Arena methods** — one alloc/read pair per object type:

```rask
func alloc_string(self, s: string) -> i64    // returns offset
func read_string(self, offset: i64) -> string
func alloc_array(self, cap: i64) -> i64
func array_get(self, offset: i64, idx: i64) -> Value
func array_set(self, offset: i64, idx: i64, val: Value)
func array_len(self, offset: i64) -> i64
// ~6 object types, ~6 method pairs
```

The rest of the VM calls `arena.alloc_string()` / `arena.read_value()` and
doesn't touch bytes directly.

**Memory accounting is exact**: `top` tracks bytes used. `capacity` is the hard
limit. `alloc_*` checks `top + size <= capacity` and returns ArenaExhausted on
overflow. No hidden heap allocations — arena strings are byte sequences in the
buffer, not Rask `string` values.

**Frame management**: `frame_begin()` saves `top` as `frame_base`.
`frame_end()` resets `top = frame_base`, reclaiming all frame-local allocations.

**Test:** Allocate strings/arrays/maps, read back, verify byte-level layout.
Frame begin/end clears correctly. ArenaExhausted triggers at capacity.

**Serialization consequence**: `buf[0..top]` is close to the serialized form
already. Offsets are integers, not pointers — no relocation needed.

### Phase 4: Lexer

Tokenize Raido source. Keywords: `const let func return if else for while loop
break continue match global coroutine yield try nil true false in`.

String interpolation: `"hello {name}"` desugars to concat during compilation
(lexer emits the string parts and expressions separately).

**Test:** Lex several snippets, dump token streams, spot-check.

### Phase 5: Parser

**First: verify recursive types work.** Try a self-referential enum or struct
in Rask. If `Box<T>` doesn't work, use an index pool:

```rask
struct ExprPool {
    nodes: Vec<Expr>,
}

// ExprRef is an index, not a pointer
struct ExprRef { idx: i64 }
```

Recursive descent with Pratt parsing for expression precedence. Standard
precedence table (assignment < or < and < equality < comparison < addition <
multiplication < unary < call < primary).

AST node types: Expr enum, Stmt enum, Decl enum, Block = Vec<Stmt>.

**Test:** Parse snippets → pretty-print → verify structure.

### Phase 6: Compiler (AST → bytecode)

Single-pass walk over AST. Emits instructions into a chunk's `Vec<i64>`.

Register allocation: locals get sequential register slots. Compiler tracks
`next_reg` counter. Temporaries are allocated and freed per-expression.

**Implement in this order:**
1. Literals and constants → LOAD_CONST, LOAD_NIL, LOAD_TRUE
2. Local variables → MOVE between registers
3. Arithmetic → ADD, SUB, MUL, DIV, MOD, NEG
4. Globals → GET_GLOBAL, SET_GLOBAL
5. Comparisons → EQ, LT, LE
6. Control flow → JMP, JMP_IF, JMP_IF_NOT (with backpatching)
7. Functions → CLOSURE, CALL, RETURN
8. Strings → LOAD_CONST with string pool, CONCAT

Upvalue resolution: when a closure references an outer variable, record it.
Emit CLOSE_UPVALUE before the outer function returns.

**Test:** Compile simple expressions, disassemble, verify bytecode.

### Phase 7: VM execution loop

The core dispatch loop with fuel counting.

```rask
func execute(self) -> Value or VmError {
    loop {
        if self.fuel <= 0 { return Err(VmError.FuelExhausted) }
        self.fuel = self.fuel - 1
        const instr = self.code.get(self.pc)
        self.pc = self.pc + 1
        const op = decode_op(instr)
        match op {
            OP_LOAD_NIL => { ... }
            OP_ADD => { ... }
            // ...
        }
    }
}
```

Registers: `Vec<Value>` sized to 256 per frame. Call stack: `Vec<CallFrame>`.

**Implement in order:**
1. Load/move/arithmetic → evaluate `1 + 2 * 3`
2. Globals → store/retrieve named values
3. Comparisons + jumps → `if`/`else`
4. CALL/RETURN → function calls
5. Loops → `for`/`while`

**Test at each sub-step** with Raido scripts compiled and executed.

**Milestone:** After this phase, Raido can run `fibonacci(10)`.

### Phase 8: Collections

- NEW_ARRAY / NEW_MAP → `arena.alloc_array(cap)` / `arena.alloc_map(cap)`
- GET_INDEX / SET_INDEX → `arena.array_get()` / `arena.map_get()` etc.
- LEN → `arena.array_len()` / `arena.map_len()` / string byte length

Arrays and maps live in the byte arena as contiguous byte sequences. Array
elements are 16-byte value slots. Maps use open addressing with linear probing,
entries are `key_offset(4) | key_len(4) | value(16)` — string keys stored
elsewhere in the arena.

Array growth: when `push` exceeds capacity, allocate a new larger array at
`top`, copy elements, update the Value's offset. The old space is wasted (bump
allocator doesn't free). This is fine — `frame_end()` or `reset()` reclaims it.

**Test:** Array/map creation, mutation, nested structures, growth, iteration.
Verify byte-level layout matches spec.

### Phase 9: Closures and upvalues

Closures are arena objects: `header(4) | proto_idx(2) | upvalue_count(2) |
upvalue_offsets[](4)`. Each upvalue offset points to an arena upvalue slot
(bare 16-byte value).

**Before close**: upvalue slot doesn't exist yet. The upvalue offset in the
closure is a sentinel meaning "read from register X in frame Y." The VM checks
this during GET_UPVALUE.

**CLOSE_UPVALUE**: Allocates an upvalue slot in the arena, copies the register
value into it, patches all closures that reference this variable to point at
the new arena slot. Multiple closures sharing a variable all get the same
arena offset — writes through one are visible to all.

**After close**: GET_UPVALUE/SET_UPVALUE read/write the arena slot directly.

**Test:** Shared mutation between closures, closures surviving enclosing scope,
counter factory pattern.

### Phase 10: Error handling

- `error(msg)` raises ScriptError
- TRY instruction: sets catch PC on call frame; on error, jump there with error
  in a register
- `try expr else |e| { ... }` compiles to TRY + conditional jump
- Non-catchable errors: FuelExhausted, ArenaExhausted, CallOverflow (propagate
  to host)

**Test:** try/else, error propagation through call chains, assert().

### Phase 11: Standard library

**Core (always loaded):**
`type()`, `tostring()`, `int()`, `number()`, `len()`, `error()`, `assert()`,
`print()`

**Opt-in modules** (host enables per-VM):
- **math**: abs, floor, ceil, round, sqrt (Newton's method on fixed-point),
  min, max, clamp, lerp, sin/cos/atan2 (CORDIC), random (xoshiro128++)
- **string**: sub, find, upper, lower, split, trim, starts_with, ends_with,
  rep, byte, char
- **array**: push, pop, insert, remove, sort (insertion sort), contains, join,
  reverse
- **map**: keys, values, contains, remove
- **bit**: and, or, xor, not, lshift, rshift

CORDIC for trig: loop of shifts and adds on fixed-point. ~15 iterations for
10-bit accuracy. Pure integer math — works fine in Rask.

**Test:** Each function individually, then Raido scripts that combine them.

### Phase 12: Coroutines

```rask
struct Coroutine {
    registers: Vec<Value>,
    call_stack: Vec<CallFrame>,
    pc: i64,
    status: CoroutineStatus,  // Suspended, Running, Dead
}
```

- COROUTINE: create with function ref, status = Suspended
- RESUME: save current VM state, load coroutine state, run
- YIELD: save coroutine state, restore caller state, pass value back

Main VM holds a stack of active coroutines (only one runs at a time).

**Test:** Producer/consumer, AI patrol pattern from spec, nested resume/yield.

### Phase 13: Host interop API

The Rask-facing API for embedding Raido:

```rask
const vm = Vm.new(VmConfig {
    arena_size: 4096,
    initial_fuel: 100_000,
    max_call_depth: 256,
})

vm.register("send_message", |ctx: CallContext| -> Value or VmError {
    const target = try ctx.arg_string(0)
    // host logic
    return Value.Nil
})

const chunk = try vm.compile(source)
try vm.exec(chunk)
const result = try vm.call("on_update", [Value.Int(42)])
```

Host functions stored as closures in a Vec. VM dispatch calls them when CALL
targets a host function index.

Host references: `Map<string, RefType>` with field getters/setters as closures.

**Test:** Rask program creates VM, registers functions, runs Raido script,
reads results back.

### Phase 14: Serialization

**Requires `fs.read_bytes` / `fs.write_bytes` prerequisite.**

The arena is already a byte buffer — `buf[0..top]` is most of the serialized
state. What remains is the VM execution state layered on top:

- Version header (4 bytes)
- Arena: `buf[0..top]` verbatim
- Registers: 16 bytes × register count (same encoding as arena values)
- Globals: name table + value slots
- Call stack: return PC, base register, prototype index per frame
- Coroutine states: each coroutine's registers + call stack + PC + status
- PRNG state: 4 × u32 (xoshiro128++)
- Fuel remaining: i64

No pointer fixup needed — all references are integer offsets into the arena,
which is serialized in place. Host function bindings and bytecode are NOT
serialized (re-registered/re-loaded on restore, matched by name).

**Test:** Serialize mid-execution, deserialize, resume, verify same result as
uninterrupted execution. Round-trip through file with `fs.write_bytes` /
`fs.read_bytes`.

### Phase 15: Content identity

SHA-256 of chunk bytecode + constants + prototypes.

SHA-256 is pure integer math + bitwise ops. Tedious but straightforward to
implement in Rask (~200 lines). Alternatively, use a simpler hash initially
(FNV-1a — 10 lines) and swap to SHA-256 later.

**Test:** Same source → same hash. Different source → different hash. Hash
matches a known test vector.

## Dependency Graph

```
         Phase 0 (skeleton)
              |
    +---------+---------+
    |                   |
Phase 1 (values)    Phase 4 (lexer)
    |                   |
Phase 2 (opcodes)   Phase 5 (parser)
    |                   |
Phase 3 (arena)         |
    |                   |
    +---------+---------+
              |
        Phase 6 (compiler)
              |
        Phase 7 (VM core)        ← milestone: fibonacci works
              |
    +---------+---------+------- Phase 13 (host API)
    |         |         |
Phase 8   Phase 9   Phase 10
(colls)   (closures) (errors)
    |         |         |
    +---------+---------+
              |
        Phase 11 (stdlib)        ← milestone: full language works
              |
        Phase 12 (coroutines)    ← milestone: cooperative multitasking
              |
        Phase 14 (serialization) ← milestone: save/restore
              |
        Phase 15 (content hash)  ← milestone: verifiable chunks
```

Phases 1-3 and Phase 4-5 can be done in parallel.
Phases 8, 9, 10 can be done in parallel after Phase 7.
Phase 13 can start after Phase 7 (independent of 8-12).

## Key Risks

**Recursive AST types (Phase 5):** If Rask can't do recursive enums or Box<T>,
the entire parser/compiler must use index-based node pools. Test this at the
very start of Phase 5 — it changes the design of Phases 5 and 6.

**i128 for fixed-point mul/div (Phase 1):** 32.32 multiplication needs 64-bit
intermediate results. If Rask doesn't support i128, split into 32-bit halves.
Test early.

**Vec<u8> interpreter overhead (Phase 3):** Each byte is stored as
`Value::Int(i64)` in the interpreter — 8x memory overhead. The arena's byte
budget is still enforced correctly (capacity counts logical bytes, not Rask
heap), but the host process uses more memory than expected. Not a correctness
issue. Fix later with a native `Value::U8` variant in the interpreter.

**Performance:** Interpreter-on-interpreter. A Raido loop doing 100k iterations
might be slow. This is expected and acceptable — this implementation is for
correctness, not speed.

## What This Proves

After all phases:
- Raido spec is implementable and consistent
- Rask can express a non-trivial VM (good dogfooding signal)
- Reference implementation for testing the eventual Rust version against
- Working host interop for Rask applications that want embedded scripting
