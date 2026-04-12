# Raido in Rask -- Implementation Plan

The real implementation, not a throwaway reference. If Rask can't express a
bytecode VM cleanly, that's a language bug to fix.

Two-pass compiler: declaration pass scans types, compile pass emits bytecode
with type checking. No full AST. Memory is O(declarations), not O(program size).

## Prerequisites

1. ~~Bitwise ops on explicit integer types~~ -- **Fixed.**
2. ~~`string.concat` arity mismatch~~ -- **Fixed.**
3. ~~Vec index type mismatch~~ -- **Fixed.**
4. **`fs.read_bytes` / `fs.write_bytes`** -- binary file I/O in the stdlib.
   Needed for Phase 10 (serialization) and Phase 11 (chunk save/load).
5. **Vec<u8> codegen load width** -- `ArrayIndex` in `builder.rs` defaults to
   i64 loads when `expected_ty` is None. Fix: use the tracked `elem_type`
   (already in `LocalMeta`) as the load width.

## Skeleton Status

Files in `raido/` and their state:

| File | Status | Notes |
|------|--------|-------|
| `build.rk` | Done | Package declaration |
| `bytes.rk` | **Done** | Little-endian read/write helpers |
| `value.rk` | **Done** | Number type + Value enum |
| `opcodes.rk` | **Done** | 42 opcodes, encode/decode, disassembler |
| `chunk.rk` | **Done** | Prototype + Chunk types with type metadata |
| `lexer.rk` | **Partial** | Structure done, `next_token()` is a stub |
| `compiler.rk` | **Partial** | Scaffolding done. compile/statement/expression are stubs |
| `vm.rk` | **Partial** | 6 opcodes implemented (load, return) |
| `main.rk` | **Done** | Standalone test: encoding round-trips, 40+2, sum countdown |
| `arena.rk` | Not started | |
| `types.rk` | Not started | |
| `stdlib.rk` | Not started | |
| `coroutine.rk` | Not started | |
| `host.rk` | Not started | |
| `serialize.rk` | Not started | |

## Phases

### Phase 1: Byte helpers and value types ✓ (mostly)

**bytes.rk** — **Done.** Little-endian encode/decode for u8, u16, u32, i64.

**value.rk** — **Done.** Number type (32.32 fixed-point with add, sub, mul, div,
comparisons, to_string). Value enum: None, Bool, Int, Num, Str, Array, MapVal,
Struct, Enum, FuncRef, HostRef.

**Remaining:** Test fixed-point edge cases: overflow saturation, division by
zero, negative zero.

### Phase 2: Arena

Flat `Vec<u8>` with bump allocation. Matches vm/architecture.md byte-level layout.

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
func array_get(self, offset: i64, idx: i64) -> i64
func array_set(self, offset: i64, idx: i64, val: i64)
func array_len(self, offset: i64) -> i64
func array_push(self, offset: i64, val: i64) -> i64 or ArenaError
func alloc_map(self, cap: i64) -> i64 or ArenaError
func map_get(self, offset: i64, key: i64, hash: i64) -> i64
func map_set(self, offset: i64, key: i64, hash: i64, val: i64) -> i64 or ArenaError
func map_len(self, offset: i64) -> i64
func alloc_struct(self, field_count: i64) -> i64 or ArenaError
func struct_get_field(self, offset: i64, field_idx: i64) -> i64
func struct_set_field(self, offset: i64, field_idx: i64, val: i64)
```

`frame_begin()` saves `top`. `frame_end()` resets `top = frame_base`.
`reset()` sets `top = 0`.

**Test:** Alloc each object type, read back, verify byte layout. Frame
begin/end reclaims correctly. ArenaExhausted at capacity. Struct field
get/set round-trips.

### Phase 3: Opcodes and chunks ✓ (partial)

**opcodes.rk** — **Done.** 42 opcodes, encode/decode, disassembler:

```
Spec opcodes (~38):
  LOAD_TRUE, LOAD_FALSE, LOAD_INT, LOAD_CONST, LOAD_NONE, MOVE,
  ADD, SUB, MUL, DIV, MOD, NEG,
  EQ, LT, LE,
  NOT, LEN, CONCAT,
  NEW_ARRAY, NEW_MAP, GET_INDEX, SET_INDEX,
  NEW_STRUCT, GET_STRUCT_FIELD, SET_STRUCT_FIELD,
  ENUM_TAG,
  GET_FIELD, SET_FIELD,
  JMP, JMP_IF, JMP_IF_NOT,
  CALL, TAIL_CALL, RETURN, FUNC_REF,
  IS_SOME, UNWRAP, WRAP_SOME,
  COROUTINE, YIELD, RESUME,
  TRY
```

**chunk.rk** — **Done.** Prototype has type metadata (param_types, return_type).
Chunk has module_imports. Test recursive `Vec<Prototype>` early — if Rask
can't handle it, flatten into a pool.

**types.rk** — New file:

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
    field_types: Vec<i64>,
}

struct EnumDef {
    name: string,
    variant_names: Vec<string>,
    variant_payloads: Vec<Vec<i64>>,
}
```

**Remaining:** Create types.rk. Test recursive Vec<Prototype>.

### Phase 4: Lexer ✓ (partial)

**lexer.rk** — Structure done: Lexer struct, peek/advance/match_char,
skip_whitespace (including line and block comments), keyword_or_ident.

**`next_token()` is a stub** — returns `Ok(Token.Eof)`. This is the main
work item.

**Token enum is spec-aligned:** keywords (struct/enum/extern/extend/import/is/as),
compound operators (+=, -=, ??, etc.), type annotation tokens (->, ?).

String interpolation: `"hello {name}"` emits a token sequence the compiler
can handle. Implement after basic strings work.

**Test:** Lex snippets, verify token streams. Typed function signatures
(`func foo(x: int) -> number`). String escapes. Hex/binary int literals.

### Phase 5: Two-pass compiler

**compiler.rk** — Scaffolding done: token consumption, register allocation,
bytecode emission, scope management, backpatching. Core compile methods are
stubs.

**Pass 1: Declaration scan.**

Walk the source collecting:
- `struct` declarations (name, fields, field types)
- `enum` declarations (name, variants, payloads)
- `extend` blocks (type name, method signatures) — not bodies
- `extern struct` declarations (name, fields, types, readonly flags)
- `extern func` declarations (name, param types, return type)
- `func` signatures (name, param types, return type) — not bodies
- `import` statements

Build TypeTable. This is O(declarations), not O(program size).

**Pass 2: Compile with type checking.**

Recursive descent emitting bytecode directly. The existing scaffolding
(advance, emit, alloc_reg, begin_scope/end_scope, backpatching) is the
foundation.

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

struct Local {
    name: string,
    reg: i64,
    depth: i64,
    type_id: i64,
}
```

**Type checking during compilation:**

- `const x = 42` → infer `x: int` from literal
- `const y = foo(a, b)` → look up `foo`'s return type from type table
- Binary ops: `int + int → int`, `number + number → number`,
  `int + number → number` (promote), `int / int → number`
- Function calls: check argument types match parameter types
- Struct construction: check all fields present, types match
- Match: check exhaustiveness on enums

`compile_expression` returns `(register, type_id)`.

**No upvalue resolution.** No closures. Function references are prototype
indices loaded with `FUNC_REF`.

**Implement in order:**
1. Literals → LOAD_TRUE, LOAD_FALSE, LOAD_INT, LOAD_CONST, LOAD_NONE
2. Local variables → register assignment with types, MOVE
3. Arithmetic/comparison/logic → ADD..NEG, EQ/LT/LE, NOT (type-checked)
4. Control flow → JMP, JMP_IF, JMP_IF_NOT (backpatching)
5. Functions → FUNC_REF, CALL, RETURN, TAIL_CALL (typed signatures)
6. Structs + extend → NEW_STRUCT, GET_STRUCT_FIELD, SET_STRUCT_FIELD, method calls
7. Enums → ENUM_TAG, match dispatch
8. Collections → NEW_ARRAY, NEW_MAP, GET_INDEX, SET_INDEX, LEN, CONCAT
9. Optionals → LOAD_NONE, IS_SOME, UNWRAP, WRAP_SOME, is/match patterns, ??, !
10. Extern access → GET_FIELD, SET_FIELD
11. Error handling → TRY
12. Coroutines → COROUTINE, YIELD, RESUME
13. Module imports → import resolution

### Phase 6: VM

**vm.rk** — Skeleton exists with dispatch loop structure. 6 opcodes
implemented: LOAD_TRUE, LOAD_FALSE, LOAD_INT, LOAD_CONST, LOAD_NONE,
RETURN. Missing arena.

```rask
struct Vm {
    arena: Arena,
    registers: Vec<Value>,
    call_stack: Vec<CallFrame>,
    fuel: i64,
    max_call_depth: i64,
    prng: PrngState,
    extern_funcs: Vec<ExternFunc>,
    extern_structs: Vec<ExternStructDef>,
    chunk: Chunk,
}

struct CallFrame {
    proto_idx: i64,
    pc: i64,
    base_reg: i64,
}
```

The Rask implementation uses `Vec<Value>` (tagged) for registers even though
Raido scripts are statically typed — the VM dispatch loop operates generically
on register contents. This is not a spec violation; the spec's "no type tags"
describes Raido's semantics, not the host implementation.

**Implement opcode handlers in the same order as Phase 5.** After each step,
compile + run Raido scripts using those features.

**Milestones:**
- After step 3: `1 + 2 * 3` evaluates correctly (type-checked)
- After step 5: `fibonacci(10)` runs with typed functions
- After step 6: struct creation and field access
- After step 7: enum match dispatch
- After step 8: arrays and maps

**main.rk** has a standalone test that already exercises encoding, basic
arithmetic, and a countdown loop — keep expanding it as opcodes land.

### Phase 7: Standard library

**Core (always present):** `tostring()`, `int()`, `number()`, `len()`,
`error()`, `assert()`, `print()`.

No `type()` — types known at compile time.

**Built-in methods (always present, compiler-known):**
- **int:** wrapping_add, wrapping_sub, wrapping_mul, abs
- **string:** len, sub, find, upper, lower, split, trim, starts_with,
  ends_with, rep, byte, char
- **array\<T\>:** len, get (→T?), push, pop, insert, remove, sort (stable,
  function ref comparator), contains, join, reverse
- **map\<K,V\>:** len, get (→V?), keys, values, contains, remove

**Opt-in modules:**
- **math:** abs, floor, ceil, round, sqrt (Newton's), min, max, clamp,
  lerp, sin/cos/atan2 (CORDIC, ~10-bit), random (xoshiro128++), pi
- **bit:** and, or, xor, not, lshift, rshift

### Phase 8: Coroutines

```rask
struct Coroutine {
    registers: Vec<Value>,
    call_stack: Vec<CallFrame>,
    pc: i64,
    status: CoroutineStatus,
    proto_idx: i64,
}
```

`COROUTINE A B C` creates from func ref `R[B]` with `C` args in
`R[B+1..B+C]`. First `resume()` starts execution. RESUME swaps VM state
with coroutine state. YIELD swaps back. One runs at a time.

**Test:** Producer/consumer. AI patrol from spec. Nested resume/yield.
Resume-with-value.

### Phase 9: Host interop

Extern funcs: closures stored in a Vec, invoked when CALL targets one.

Extern structs: GET_FIELD/SET_FIELD dispatch through vtable by slot index.
Type checked at load time against script declarations.

**Test:** Rask program creates VM, registers extern bindings, compiles and
loads script, verifies load-time type checking catches mismatches, runs
script, reads results back.

### Phase 10: Serialization

Arena is already a byte buffer — `buf[0..top]` serializes in place.

Remaining state:
- Version header
- Arena bytes verbatim
- Registers (8 bytes each, type known from bytecode metadata)
- Call stack (return PC, base register, prototype index per frame)
- Coroutine states
- PRNG state (4 × u32)
- Fuel remaining
- frame_base

No pointer fixup — all references are integer arena offsets.

**Test:** Serialize mid-execution, deserialize, resume, verify identical result.

### Phase 11: Content identity

SHA-256 of bytecode + constants + prototypes + type table. Pure integer
math + bitwise ops — implementable in Rask (~200 lines). Start with FNV-1a
if SHA-256 is too tedious, swap later.

**Test:** Same source → same hash. Known test vectors.

## Dependency Graph

```
Phase 1 (values + bytes) ✓
    |
Phase 2 (arena)          Phase 4 (lexer)
    |                        |
Phase 3 (opcodes + chunk)   |
    |                        |
    +----------+-------------+
               |
         Phase 5 (compiler, two-pass)
               |
         Phase 6 (VM)            ← fibonacci works (typed)
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

## Next Steps

The immediate work before building further:

1. **Align skeleton with spec.** Fix the divergences listed above — Value
   variants, opcodes, chunk fields, lexer tokens, VM fields. This is
   cleanup, not new features.
2. **Implement `next_token()`** in lexer.rk. Everything downstream blocks on it.
3. **Build arena.rk and types.rk.** These are needed before the compiler can
   emit struct/enum/collection operations.

After that, the compiler (Phase 5) is the critical path.

## Risks

**Recursive struct in Prototype:** `Prototype` contains `Vec<Prototype>`.
If Rask doesn't handle this, flatten into a `Vec<Prototype>` pool with
index references. Test early.

**Two-pass complexity:** The declaration pass needs to re-lex the source or
store enough position info to restart. If the lexer isn't cheaply resettable,
buffer tokens from pass 1 or just re-lex from the start (source is a string,
cheap to reconstruct a new Lexer).

**Vec\<Vec\<i64\>\> for enum payloads:** TypeTable uses nested Vecs. Verify
Rask handles this.

## Rask Friction Log

Track any point where the language gets in the way. This is the whole
reason to write it in Rask.

### Findings

1. ~~Bitwise ops on i64~~ — **Fixed.** Typo in unify.rs.
2. ~~Codegen crash on bitwise~~ — cascading from #1.
3. **Vec.get() returns T? (optional).** Every register/code access needs
   unwrapping. Workable — helper functions `reg_get`/`code_get` wrap it.
   Not a bug, but friction. Consider whether Vec should have an unchecked
   `[]` operator.
4. ~~Multi-file packages~~ — **Works fine.**
5. ~~println with interpolation~~ — **Fixed.**
6. **Newlines as statement terminators.** Multi-line `&&` chains like
   `a == 1 \n && b == 2` fail because the newline terminates the statement.
   Must keep `&&` chains on one line. Not a bug — by design.
