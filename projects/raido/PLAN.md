# Raido in Rask -- Implementation Plan

The real implementation, not a throwaway reference. If Rask can't express a
bytecode VM cleanly, that's a language bug to fix.

Two-pass compiler: declaration pass scans types, compile pass emits bytecode
with type checking. No full AST. Memory is O(declarations), not O(program size).

## Prerequisites

1. ~~Bitwise ops on explicit integer types~~ -- **Fixed.** Typo in
   `unify.rs:100`: `"bitand"` should be `"bit_and"`.
2. ~~`string.concat` arity mismatch~~ -- **Fixed.** Stub was static
   `concat(a, b)`, changed to method `concat(self, other)`.
3. ~~Vec index type mismatch~~ -- **Fixed.** Stubs used `usize`, type
   checker enforces `i64`. Changed stubs to `i64`.
4. **`fs.read_bytes` / `fs.write_bytes`** -- binary file I/O in the stdlib.
5. **Vec<u8> codegen load width** -- `ArrayIndex` in `builder.rs` defaults to
   i64 loads when `expected_ty` is None. Fix: use the tracked `elem_type`
   (already in `LocalMeta`) as the load width. The 1-byte stride is already
   correct.

## File Structure

```
raido/
  value.rk         # 8-byte values, 32.32 fixed-point Number
  bytes.rk         # Byte encoding helpers (read/write u8, u16, u32, i64)
  arena.rk         # Byte-buffer arena with bump allocation
  opcodes.rk       # ~35 opcodes, encode/decode 32-bit instructions
  chunk.rk         # Prototype, TypeTable, and Chunk types
  types.rk         # Type table: struct layouts, enum defs, extern declarations
  lexer.rk         # Tokenizer
  compiler.rk      # Two-pass: declaration scan + recursive descent with type checking
  vm.rk            # Dispatch loop, registers, call stack
  stdlib.rk        # Core + opt-in module functions
  coroutine.rk     # Coroutine state, resume/yield
  host.rk          # Extern struct/func registry
  serialize.rk     # VM state serialization
  main.rk          # CLI entry point
```

## Phases

### Phase 1: Byte helpers and value types

**bytes.rk** -- encode/decode integers as bytes in a `Vec<u8>`:

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

**value.rk** -- 32.32 fixed-point `Number`. No Value enum -- static types mean
values are 8 bytes with the type known at compile time:

```rask
struct Number { raw: i64 }
```

Fixed-point arithmetic: add, sub, mul, div, comparisons, int<->number
conversions. Mul/div need 128-bit intermediates -- if Rask lacks i128, split
into 32-bit halves with bitwise ops.

**Test:** Fixed-point round-trips. Overflow panics (not saturates). Division
by zero errors. Wrapping arithmetic helpers.

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
func alloc_array(self, elem_size: i64, cap: i64) -> i64 or ArenaError
func array_get(self, offset: i64, idx: i64) -> i64   // raw 8-byte value
func array_set(self, offset: i64, idx: i64, val: i64)
func array_len(self, offset: i64) -> i64
func array_cap(self, offset: i64) -> i64
func array_push(self, offset: i64, val: i64) -> i64 or ArenaError
func alloc_map(self, cap: i64) -> i64 or ArenaError  // compact dict layout
func map_get(self, offset: i64, key: i64, hash: i64) -> i64  // raw 8-byte value
func map_set(self, offset: i64, key: i64, hash: i64, val: i64) -> i64 or ArenaError
func map_len(self, offset: i64) -> i64
func alloc_struct(self, field_count: i64) -> i64 or ArenaError
func struct_get_field(self, offset: i64, field_idx: i64) -> i64
func struct_set_field(self, offset: i64, field_idx: i64, val: i64)
```

No closure or upvalue allocation -- those concepts are gone.

`frame_begin()` saves `top`. `frame_end()` resets `top = frame_base`.
`reset()` sets `top = 0`.

**Test:** Alloc each object type, read back, verify byte layout. Frame
begin/end reclaims correctly. ArenaExhausted at capacity. Array push
beyond cap reallocates. Struct field get/set round-trips.

### Phase 3: Opcodes and chunks

**opcodes.rk** -- ~38 opcodes as constants. Three 32-bit instruction formats:

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

**chunk.rk** -- compiled output:

```rask
struct Prototype {
    code: Vec<i64>,       // instructions
    constants: Vec<i64>,  // 8-byte constant values
    num_registers: i64,
    arity: i64,
    name: string,
    param_types: Vec<i64>,   // type IDs for parameters
    return_type: i64,        // type ID for return
    lines: Vec<i64>,         // source line per instruction (debug)
}

struct Chunk {
    main: Prototype,
    prototypes: Vec<Prototype>,  // all function prototypes
    type_table: TypeTable,
    imports: Vec<ExternDecl>,
    module_imports: Vec<string>,
    exports: Vec<string>,
}
```

**types.rk** -- type metadata:

```rask
struct TypeTable {
    structs: Vec<StructDef>,
    enums: Vec<EnumDef>,
    extern_structs: Vec<ExternStructDef>,
    extern_funcs: Vec<ExternFuncDef>,
}

struct StructDef {
    name: string,
    field_names: Vec<string>,
    field_types: Vec<i64>,    // type IDs
}

struct EnumDef {
    name: string,
    variant_names: Vec<string>,
    variant_payloads: Vec<Vec<i64>>,  // type IDs per variant
}
```

**Test:** Instruction encode/decode round-trips for all three formats.
TypeTable construction and lookup.

### Phase 4: Lexer

Tokenize Raido source. Newline-sensitive (statement terminator).

Tokens: keywords (`const let func return if else for while loop break continue
match try true false in yield struct enum extern import`), operators,
literals (int, number, string with interpolation), identifiers, type
annotations (`:`, `->`, `?`).

String interpolation: `"hello {name}"` emits token sequence the compiler
can handle.

**Test:** Lex snippets, verify token streams. Verify typed function signatures
tokenize correctly.

### Phase 5: Two-pass compiler

**Pass 1: Declaration scan.**

Walk the source collecting:
- `struct` declarations (name, fields, field types)
- `enum` declarations (name, variants, payloads)
- `extern struct` declarations (name, fields, types, readonly flags)
- `extern func` declarations (name, param types, return type)
- `func` signatures (name, param types, return type) -- not bodies
- `import` statements

Build a type table mapping names to type IDs. This is O(declarations), not
O(program size).

**Pass 2: Compile with type checking.**

Recursive descent parser that emits bytecode directly. Same architecture as
the old single-pass compiler, but now with type information available from
pass 1.

The compiler holds:
- Current and previous token (from lexer)
- Scope stack: `Vec<Scope>` where each Scope holds local names, register
  indices, and types
- Current prototype being built
- Prototype stack (for nested functions -- still needed for coroutine funcs)
- Type table from pass 1

```rask
struct Compiler {
    lexer: Lexer,
    current: Token,
    previous: Token,
    scopes: Vec<Scope>,
    proto: Prototype,
    proto_stack: Vec<Prototype>,
    next_reg: i64,
    type_table: TypeTable,
    imports: Vec<ExternDecl>,
    module_imports: Vec<string>,
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
    type_id: i64,    // known type
}
```

**Type checking during compilation:**

- `const x = 42` -> infer `x: int` from literal
- `const y = foo(a, b)` -> look up `foo`'s return type from type table
- Binary ops: check types. `int + int -> int`. `number + number -> number`. `int + number -> number` (promote int). `int / int -> number` (division always returns number).
- Function calls: check argument types match parameter types
- Struct construction: check all fields present, types match
- Match: check exhaustiveness on enums

**Parsing functions emit bytecode as they go:**

```rask
func compile_expression(self, min_prec: i64) -> (i64, i64)  // (register, type_id)
func compile_statement(self)
func compile_block(self)
func compile_function(self, name: string)
```

`compile_expression` returns the register holding the result and its type.

**Register allocation:** Same as before -- locals get sequential registers,
temporaries bump `next_reg`.

**Backpatching:** Same as before -- jumps with placeholder offsets, patched
after body compilation.

**No upvalue resolution.** No closures. Function references are just prototype
indices loaded with `FUNC_REF`.

**Constant folding:** Same as before -- literal-on-literal only.

**Implement in order:**
1. Literals -> LOAD_TRUE, LOAD_FALSE, LOAD_INT, LOAD_CONST, LOAD_NONE
2. Local variables -> register assignment with types, MOVE
3. Arithmetic/comparison/logic -> ADD..NEG, EQ/LT/LE, NOT (type-checked)
4. Control flow -> JMP, JMP_IF, JMP_IF_NOT (with backpatching)
5. Functions -> FUNC_REF, CALL, RETURN, TAIL_CALL (typed signatures)
6. Structs -> NEW_STRUCT, GET_STRUCT_FIELD, SET_STRUCT_FIELD
7. Enums -> ENUM_TAG, match dispatch
8. Collections -> NEW_ARRAY, NEW_MAP, GET_INDEX, SET_INDEX, LEN, CONCAT
9. Optionals -> LOAD_NONE, IS_SOME, UNWRAP, WRAP_SOME, is/match patterns, ??, !
10. Extern access -> GET_FIELD, SET_FIELD
11. Error handling -> TRY
12. Coroutines -> COROUTINE, YIELD, RESUME
13. Module imports -> import resolution

**Test at each step:** compile snippets, disassemble bytecode, verify
instruction sequences and type checking. A `disassemble(proto)` function
is useful here.

### Phase 6: VM

The dispatch loop.

```rask
struct Vm {
    arena: Arena,
    registers: Vec<i64>,     // flat: base_reg indexes into it (8 bytes each)
    call_stack: Vec<CallFrame>,
    fuel: i64,
    max_call_depth: i64,
    prng: PrngState,          // xoshiro128++: 4 x u32
    extern_funcs: Vec<ExternFunc>,
    extern_structs: Vec<ExternStructDef>,
    chunk: Chunk,
}

struct CallFrame {
    proto_idx: i64,
    pc: i64,
    base_reg: i64,    // offset into registers vec
}
```

No globals array -- mutable globals are gone.

**Implement opcode handlers in the same order as Phase 5.** After each
sub-step, you can compile + run Raido scripts that use those features.

**Milestones:**
- After step 3: `1 + 2 * 3` evaluates correctly (type-checked)
- After step 5: `fibonacci(10)` runs with typed functions
- After step 6: struct creation and field access work
- After step 7: enum match dispatch works
- After step 8: arrays and maps work

### Phase 7: Standard library

Host functions registered before script execution.

**Core (always present):** `tostring()`, `int()`, `number()`, `len()`,
`error()`, `assert()`, `print()`.

No `type()` -- types are known at compile time.

**Built-in methods (always present, compiler-known):**
- **int:** wrapping_add, wrapping_sub, wrapping_mul, abs
- **string:** len, sub, find, upper, lower, split, trim, starts_with,
  ends_with, rep, byte, char
- **array:** len, get (T?), push, pop, insert, remove, sort (stable,
  function ref comparator), contains, join, reverse
- **map:** len, get (V?), keys, values, contains, remove

**Opt-in modules:**
- **math:** abs, floor, ceil, round, sqrt (Newton's method), min, max, clamp,
  lerp, sin/cos/atan2 (CORDIC), random (xoshiro128++), pi
- **bit:** and, or, xor, not, lshift, rshift

**Test:** Each function with typed arguments. Then Raido scripts that
combine them.

### Phase 8: Coroutines

```rask
struct Coroutine {
    registers: Vec<i64>,       // 8 bytes each, initial args in R[0..N-1]
    call_stack: Vec<CallFrame>,
    pc: i64,
    status: CoroutineStatus,   // Suspended, Running, Dead
    proto_idx: i64,            // function reference (no closure)
}
```

`COROUTINE A B C` creates one from func ref `R[B]` with `C` args in
`R[B+1..B+C]`. The args are placed in the coroutine's register window
at `R[0..C-1]`. First `resume()` starts execution from the function's
first instruction. RESUME swaps VM state with coroutine state. YIELD
swaps back. One runs at a time.

**Test:** Producer/consumer. AI patrol from spec. Nested resume/yield.
Resume-with-value for combat tick inputs.

### Phase 9: Host interop

```rask
vm.register_extern_func("send_message", |ctx: CallContext| -> i64 or VmError {
    const target = try ctx.arg_string(0)
    const body = try ctx.arg_string(1)
    // host logic
    return 0  // void
})

vm.register_extern_struct("Enemy", raido.ExternStruct {
    fields: [
        raido.Field.int("health", get_health, set_health),
        raido.Field.number("x", get_x, set_x),
    ],
})
```

Extern funcs: closures stored in a Vec, invoked when CALL targets one.

Extern structs: GET_FIELD/SET_FIELD dispatch through the vtable by slot
index. Type checked at load time against script declarations.

**Test:** Rask program creates VM, registers extern bindings, compiles and
loads script, verifies load-time type checking catches mismatches, runs
script, reads results back.

### Phase 10: Serialization

Arena is already a byte buffer -- `buf[0..top]` serializes in place.

Remaining state layered on top:
- Version header
- Arena bytes verbatim
- Registers (8 bytes each -- no type tags)
- Call stack (return PC, base register, prototype index per frame)
- Coroutine states
- PRNG state (4 x u32)
- Fuel remaining
- frame_base

No pointer fixup -- all references are integer arena offsets.

Simpler than the dynamic version: no per-value type tags. The deserializer
knows every register's type from bytecode metadata.

**Test:** Serialize mid-execution, deserialize, resume, verify identical
result.

### Phase 11: Content identity

SHA-256 of bytecode + constants + prototypes + type table. Pure integer
math + bitwise ops -- implementable in Rask (~200 lines). Start with
FNV-1a if SHA-256 is too tedious, swap later.

**Test:** Same source -> same hash. Known test vectors.

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
         Phase 5 (compiler, two-pass)
               |
         Phase 6 (VM)            <- fibonacci works (typed)
               |
    +----------+----------+
    |          |          |
Phase 7    Phase 8    Phase 9
(stdlib)   (coroutines) (host)
    |          |          |
    +----------+----------+
               |
         Phase 10 (serialization) <- save/restore works
               |
         Phase 11 (content hash)  <- verifiable chunks
```

Phases 1-3 and Phase 4 are parallel tracks.
Phases 7, 8, 9 are parallel after Phase 6.

## Risks

**i128 for fixed-point mul/div (Phase 1):** If Rask lacks i128, split into
32-bit halves. Test first.

**Recursive struct in Prototype (Phase 3):** `Prototype` contains
`Vec<Prototype>`. If Rask doesn't handle this, flatten into a
`Vec<Prototype>` pool with index references.

**Two-pass complexity (Phase 5):** The declaration pass needs to re-lex
the source or store enough position info to restart the compile pass. If
the lexer isn't cheaply resettable, buffer tokens from pass 1 or just
re-lex from the start.

## Rask friction log

Track any point where the language gets in the way. This is the whole
reason to write it in Rask. Examples to watch for:

- Two-pass compilation -- is re-lexing awkward?
- Scope chain walking without pointers -- is Vec indexing awkward?
- Backpatching -- does mutating a Vec<i64> by index feel natural?
- Token matching -- does `match` on enums with payloads work well?
- Error propagation in the compiler -- does `try` compose cleanly?
- Type table construction -- does struct-of-vecs work ergonomically?

If something is painful, don't work around it -- file it.

### Findings (from skeleton setup)

1. ~~Bitwise ops on i64~~ -- **Fixed.** Typo in unify.rs.
2. ~~Codegen crash on bitwise~~ -- likely cascading from #1, needs retest.
3. **Vec.get() returns T? (optional).** Every register/code access needs
   unwrapping. Workable -- helper functions `reg_get`/`code_get` wrap it.
4. ~~Multi-file packages~~ -- **Works fine.** Was a red herring from #1.
5. ~~println with interpolation~~ -- **Fixed.** concat stub was wrong.
6. **Newlines as statement terminators.** Multi-line `&&` chains like
   `a == 1 \n && b == 2` fail because the newline terminates the statement.
   Must keep `&&` chains on one line. Not a bug -- by design.
