# Raido in Rask — Implementation Plan

The real implementation, not a throwaway reference. If Rask can't express a
bytecode VM cleanly, that's a language bug to fix.

Single-pass compiler (Lua-style): lexer feeds parser, parser emits bytecode
directly. No AST. Memory is O(scope depth), not O(program size).

## Prerequisites

1. ~~Bitwise ops on explicit integer types~~ — **Fixed.** Typo in
   `unify.rs:100`: `"bitand"` should be `"bit_and"`.
2. ~~`string.concat` arity mismatch~~ — **Fixed.** Stub was static
   `concat(a, b)`, changed to method `concat(self, other)`.
3. ~~Vec index type mismatch~~ — **Fixed.** Stubs used `usize`, type
   checker enforces `i64`. Changed stubs to `i64`.
4. **`fs.read_bytes` / `fs.write_bytes`** — binary file I/O in the stdlib.
5. **Vec<u8> codegen load width** — `ArrayIndex` in `builder.rs` defaults to
   i64 loads when `expected_ty` is None. Fix: use the tracked `elem_type`
   (already in `LocalMeta`) as the load width. The 1-byte stride is already
   correct.

## File Structure

```
raido/
  value.rk         # Value enum, 32.32 fixed-point Number
  bytes.rk         # Byte encoding helpers (read/write u8, u16, u32, i64)
  arena.rk         # Byte-buffer arena with bump allocation
  opcodes.rk       # 37 opcodes, encode/decode 32-bit instructions
  chunk.rk         # Prototype and Chunk types
  lexer.rk         # Tokenizer
  compiler.rk      # Single-pass: recursive descent parser + bytecode emitter
  vm.rk            # Dispatch loop, registers, call stack
  stdlib.rk        # Core + opt-in module functions
  coroutine.rk     # Coroutine state, resume/yield
  host.rk          # Host function registry, host references
  serialize.rk     # VM state serialization
  main.rk          # CLI entry point
```

## Phases

### Phase 1: Byte helpers and value types

**bytes.rk** — encode/decode integers as bytes in a `Vec<u8>`:

```rask
func write_u8(buf: Vec<u8>, offset: i64, val: u8)
func read_u8(buf: Vec<u8>, offset: i64) -> u8
func write_u16le(buf: Vec<u8>, offset: i64, val: i64)
func read_u16le(buf: Vec<u8>, offset: i64) -> i64
func write_u32le(buf: Vec<u8>, offset: i64, val: i64)
func read_u32le(buf: Vec<u8>, offset: i64) -> i64
func write_i64le(buf: Vec<u8>, offset: i64, val: i64)
func read_i64le(buf: Vec<u8>, offset: i64) -> i64
```

All implemented with bitwise ops and shifts.

**value.rk** — 32.32 fixed-point `Number` and the `Value` enum:

```rask
struct Number { raw: i64 }

enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Num(Number),
    Str(i64),       // arena offset
    Array(i64),     // arena offset
    Map(i64),       // arena offset
    Closure(i64),   // arena offset
    HostRef(i64, i64),  // type_id, ref_id
}
```

Fixed-point arithmetic: add, sub, mul, div, comparisons, int↔number
conversions. Mul/div need 128-bit intermediates — if Rask lacks i128, split
into 32-bit halves with bitwise ops.

Value encoding to/from 16 bytes in a `Vec<u8>`:

```rask
func write_value(buf: Vec<u8>, offset: i64, val: Value)
func read_value(buf: Vec<u8>, offset: i64) -> Value
```

**Test:** Fixed-point round-trips. Overflow saturates. Division by zero errors.
Value encode/decode round-trips for every variant.

### Phase 2: Arena

Flat `Vec<u8>` with bump allocation. Matches the spec's byte-level layout.

```rask
struct Arena {
    buf: Vec<u8>,
    top: i64,
    capacity: i64,
    frame_base: i64,
}
```

4-byte object headers: `type_tag(u8) | pad(u8) | body_size(u16)`.
4-byte aligned. Max 64 KB per object.

One alloc/read pair per object type:

```rask
func alloc_string(self, s: string) -> i64 or ArenaError
func read_string(self, offset: i64) -> string
func alloc_array(self, cap: i64) -> i64 or ArenaError
func array_get(self, offset: i64, idx: i64) -> Value
func array_set(self, offset: i64, idx: i64, val: Value)
func array_len(self, offset: i64) -> i64
func array_cap(self, offset: i64) -> i64
func array_push(self, offset: i64, val: Value) -> i64 or ArenaError
func alloc_map(self, cap: i64) -> i64 or ArenaError
func map_get(self, offset: i64, key: Value) -> Value
func map_set(self, offset: i64, key: Value, val: Value) -> i64 or ArenaError
func map_len(self, offset: i64) -> i64
func alloc_closure(self, proto_idx: i64, upvalue_offsets: Vec<i64>) -> i64 or ArenaError
func alloc_upvalue(self, val: Value) -> i64 or ArenaError
```

`frame_begin()` saves `top`. `frame_end()` resets `top = frame_base`.
`reset()` sets `top = 0`.

`alloc_*` checks `top + size <= capacity`, returns `ArenaExhausted` on
overflow. Memory accounting is exact — `top` is the byte count.

**Test:** Alloc each object type, read back, verify byte layout. Frame
begin/end reclaims correctly. ArenaExhausted at capacity. Array push
beyond cap reallocates.

### Phase 3: Opcodes and chunks

**opcodes.rk** — 37 opcodes as constants. Three 32-bit instruction formats:

```
ABC:  op(8) | A(8) | B(8) | C(8)
ABx:  op(8) | A(8) | Bx(16)
AsBx: op(8) | A(8) | sBx(16, signed)
```

Encode/decode with bitwise ops:

```rask
func encode_abc(op: i64, a: i64, b: i64, c: i64) -> i64
func encode_abx(op: i64, a: i64, bx: i64) -> i64
func encode_asbx(op: i64, a: i64, sbx: i64) -> i64
func decode_op(instr: i64) -> i64
func decode_a(instr: i64) -> i64
func decode_b(instr: i64) -> i64
func decode_c(instr: i64) -> i64
func decode_bx(instr: i64) -> i64
func decode_sbx(instr: i64) -> i64   // sign-extended
```

**chunk.rk** — compiled output:

```rask
struct Prototype {
    code: Vec<i64>,       // instructions
    constants: Vec<Value>,
    prototypes: Vec<Prototype>,  // nested functions
    num_registers: i64,
    num_upvalues: i64,
    arity: i64,
    name: string,
    lines: Vec<i64>,      // source line per instruction (debug)
}

struct Chunk {
    main: Prototype,
    imports: Vec<string>,
    exports: Vec<string>,
}
```

**Test:** Instruction encode/decode round-trips for all three formats.

### Phase 4: Lexer

Tokenize Raido source. Newline-sensitive (statement terminator).

Tokens: keywords (`const let func return if else for while loop break continue
match global try nil true false in yield`), operators, literals (int, number,
string with interpolation), identifiers.

String interpolation: `"hello {name}"` emits token sequence the compiler
can handle (e.g., `StringPart("hello ")`, expression tokens, `StringEnd("")`).

**Test:** Lex snippets, verify token streams.

### Phase 5: Single-pass compiler

Recursive descent parser that emits bytecode directly. No AST. Same
architecture as Lua's lparser.c / lcode.c.

The compiler holds:
- Current and previous token (from lexer)
- Scope stack: `Vec<Scope>` where each Scope holds local names + register
  indices
- Current prototype being built
- Prototype stack (for nested functions)

```rask
struct Compiler {
    lexer: Lexer,
    current: Token,
    previous: Token,
    scopes: Vec<Scope>,
    proto: Prototype,        // prototype being compiled
    proto_stack: Vec<Prototype>,  // saved when entering nested func
    next_reg: i64,           // next free register in current frame
    imports: Vec<string>,
    exports: Vec<string>,
}

struct Scope {
    locals: Vec<Local>,
    depth: i64,
}

struct Local {
    name: string,
    reg: i64,
    depth: i64,
    is_captured: bool,
}
```

**Parsing functions emit bytecode as they go:**

```rask
func compile_expression(self, min_prec: i64) -> i64  // returns register
func compile_statement(self)
func compile_block(self)
func compile_function(self, name: string)
```

`compile_expression` returns the register holding the result. Pratt
precedence: each `parse_*` method checks the next token's precedence and
recurses.

**Register allocation:** Locals get sequential registers. Temporaries
bump `next_reg`, freed after the expression completes (set `next_reg`
back). Same pattern as Lua — no graph coloring, no liveness analysis.

**Backpatching:** `if`/`while`/`for` emit a jump with placeholder offset,
save the instruction index, compile the body, then patch the offset:

```rask
func emit_jump(self, op: i64) -> i64 {
    const idx = self.proto.code.len()
    self.emit(encode_asbx(op, 0, 0))  // placeholder
    return idx
}

func patch_jump(self, idx: i64) {
    const offset = self.proto.code.len() - idx - 1
    const instr = self.proto.code.get(idx)
    const op = decode_op(instr)
    const a = decode_a(instr)
    self.proto.code.set(idx, encode_asbx(op, a, offset))
}
```

**Upvalue resolution:** When a variable isn't in the current function's
scope, walk up the scope stack. If found in an enclosing function, mark
it as captured in that scope and add an upvalue entry to the current
prototype. Uses the same logic as Lua's upvalue chain — each intermediate
function also gets an upvalue entry if needed.

**Constant folding:** When both operands of an arithmetic expression are
literals, emit the folded result as `LOAD_INT` or `LOAD_CONST` instead
of the arithmetic instruction. Only for literal-on-literal — no symbolic
analysis.

**Implement in order:**
1. Literals → LOAD_NIL, LOAD_TRUE, LOAD_FALSE, LOAD_INT, LOAD_CONST
2. Local variables → register assignment, MOVE
3. Arithmetic/comparison/logic → ADD..NEG, EQ/LT/LE, NOT
4. Globals → GET_GLOBAL, SET_GLOBAL
5. Control flow → JMP, JMP_IF, JMP_IF_NOT (with backpatching)
6. Functions → CLOSURE, CALL, RETURN, TAIL_CALL
7. Collections → NEW_ARRAY, NEW_MAP, GET_INDEX, SET_INDEX, LEN, CONCAT
8. Closures → upvalue tracking, CLOSE_UPVALUE, GET_UPVALUE, SET_UPVALUE
9. Error handling → TRY
10. Coroutines → COROUTINE, YIELD, RESUME
11. Host refs → GET_FIELD, SET_FIELD

**Test at each step:** compile snippets, disassemble bytecode, verify
instruction sequences. A `disassemble(proto)` function is useful here —
print each instruction in human-readable form.

**If single-pass is painful in Rask, document why.** The point is to find
out. Possible friction points: no pointer-based scope chains (use Vec
indices), no goto (use loop+break for error recovery), no union types for
the token-to-register return convention. If any of these require ugly
workarounds, that's a Rask design signal worth recording.

### Phase 6: VM

The dispatch loop.

```rask
struct Vm {
    arena: Arena,
    registers: Vec<Value>,   // flat: base_reg indexes into it
    call_stack: Vec<CallFrame>,
    globals: Vec<Value>,
    global_names: Map<string, i64>,
    fuel: i64,
    max_call_depth: i64,
    prng: PrngState,          // xoshiro128++: 4 × u32
    host_functions: Vec<HostFunc>,
    chunk: Chunk,
}

struct CallFrame {
    proto_idx: i64,
    pc: i64,
    base_reg: i64,    // offset into registers vec
}
```

**Implement opcode handlers in the same order as Phase 5.** After each
sub-step, you can compile + run Raido scripts that use those features.

**Milestones:**
- After step 3: `1 + 2 * 3` evaluates correctly
- After step 5: `if`/`while`/`for` work
- After step 6: `fibonacci(10)` runs
- After step 7: arrays and maps work
- After step 8: closures with shared upvalues work

### Phase 7: Standard library

Host functions registered before script execution.

**Core (always present):** `type()`, `tostring()`, `int()`, `number()`,
`len()`, `error()`, `assert()`, `print()`.

**Opt-in modules:**
- **math:** abs, floor, ceil, round, sqrt (Newton's method), min, max, clamp,
  lerp, sin/cos/atan2 (CORDIC — ~15 iterations of shifts and adds), random
  (xoshiro128++), pi
- **string:** sub, find, upper, lower, split, trim, starts_with, ends_with,
  rep, byte, char
- **array:** push, pop, insert, remove, sort (insertion sort), contains, join,
  reverse
- **map:** keys, values, contains, remove
- **bit:** and, or, xor, not, lshift, rshift

**Test:** Each function. Then Raido scripts that combine them.

### Phase 8: Coroutines

```rask
struct Coroutine {
    registers: Vec<Value>,
    call_stack: Vec<CallFrame>,
    pc: i64,
    status: CoroutineStatus,  // Suspended, Running, Dead
}
```

COROUTINE creates one. RESUME swaps VM state with coroutine state. YIELD
swaps back. One runs at a time.

**Test:** Producer/consumer. AI patrol from spec. Nested resume/yield.

### Phase 9: Host interop

```rask
vm.register("send_message", |ctx: CallContext| -> Value or VmError {
    const target = try ctx.arg_string(0)
    // host logic
    return Value.Nil
})
```

Host functions: closures stored in a Vec, invoked when CALL targets one.

Host references: `register_ref_type(name, fields)` where fields have
getter/setter closures. GET_FIELD/SET_FIELD dispatch through the vtable
by slot index.

**Test:** Rask program creates VM, registers host functions, runs script,
reads results back.

### Phase 10: Serialization

Arena is already a byte buffer — `buf[0..top]` serializes in place.

Remaining state layered on top:
- Version header
- Arena bytes verbatim
- Registers (16 bytes each via write_value)
- Globals (name table + values)
- Call stack (return PC, base register, prototype index per frame)
- Coroutine states
- PRNG state (4 × u32)
- Fuel remaining

No pointer fixup — all references are integer arena offsets.

**Test:** Serialize mid-execution, deserialize, resume, verify identical
result.

### Phase 11: Content identity

SHA-256 of bytecode + constants + prototypes. Pure integer math + bitwise
ops — implementable in Rask (~200 lines). Start with FNV-1a if SHA-256
is too tedious, swap later.

**Test:** Same source → same hash. Known test vectors.

## Dependency Graph

```
Phase 1 (values + bytes)
    |
Phase 2 (arena)          Phase 4 (lexer)
    |                        |
Phase 3 (opcodes + chunk)   |
    |                        |
    +----------+-------------+
               |
         Phase 5 (compiler)
               |
         Phase 6 (VM)            ← fibonacci works
               |
    +----------+----------+
    |          |          |
Phase 7    Phase 8    Phase 9
(stdlib)   (coroutines) (host)
    |          |          |
    +----------+----------+
               |
         Phase 10 (serialization) ← save/restore works
               |
         Phase 11 (content hash)  ← verifiable chunks
```

Phases 1-3 and Phase 4 are parallel tracks.
Phases 7, 8, 9 are parallel after Phase 6.

## Risks

**i128 for fixed-point mul/div (Phase 1):** If Rask lacks i128, split into
32-bit halves. Test first.

**Recursive struct in Prototype (Phase 3):** `Prototype` contains
`Vec<Prototype>`. If Rask doesn't handle this, flatten into a
`Vec<Prototype>` pool with index references.

## Rask friction log

Track any point where the language gets in the way. This is the whole
reason to write it in Rask. Examples to watch for:

- Scope chain walking without pointers — is Vec indexing awkward?
- Backpatching — does mutating a Vec<i64> by index feel natural?
- Token matching — does `match` on enums with payloads work well?
- Error propagation in the compiler — does `try` compose cleanly?
- Closures as host functions — do they capture context correctly?

If something is painful, don't work around it — file it. The fix might be
in Rask, not in the Raido code.

### Findings (from skeleton setup)

1. ~~Bitwise ops on i64~~ — **Fixed.** Typo in unify.rs.
2. ~~Codegen crash on bitwise~~ — likely cascading from #1, needs retest.
3. **Vec.get() returns T? (optional).** Every register/code access needs
   unwrapping. Workable — helper functions `reg_get`/`code_get` wrap it.
4. ~~Multi-file packages~~ — **Works fine.** Was a red herring from #1.
5. ~~println with interpolation~~ — **Fixed.** concat stub was wrong.
6. **Newlines as statement terminators.** Multi-line `&&` chains like
   `a == 1 \n && b == 2` fail because the newline terminates the statement.
   Must keep `&&` chains on one line. Not a bug — by design.
