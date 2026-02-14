<!-- id: compiler.layout -->
<!-- status: decided -->
<!-- summary: ABI-level memory layout for enums, closures, trait objects -->
<!-- depends: type.enums, type.traits, mem.closures, mem.value -->

# Memory Layout

Precise memory layout for enums, closures, trait objects, and other compound types. Defines field ordering, alignment, padding, and size calculations for compiler codegen.

## Alignment and Padding Rules

| Rule | Description |
|------|-------------|
| **L1: Natural alignment** | Type aligned to its largest field's alignment (max 16 bytes) |
| **L2: Padding inserted** | Fields padded to maintain alignment of next field |
| **L3: Struct tail padding** | Structs padded to multiple of their alignment |
| **L4: Field ordering** | Fields laid out in source order (no reordering) |

## Primitive Sizes and Alignment

| Type | Size (bytes) | Alignment (bytes) |
|------|--------------|-------------------|
| `bool` | 1 | 1 |
| `i8`, `u8` | 1 | 1 |
| `i16`, `u16` | 2 | 2 |
| `i32`, `u32`, `f32` | 4 | 4 |
| `i64`, `u64`, `f64` | 8 | 8 |
| `isize`, `usize` | 8 | 8 (64-bit target) |
| `*T` (pointer) | 8 | 8 |
| `Handle<T>` | 8 | 8 (u64 internally) |

## Structs

Fields stored in source order, padded for alignment.

```rask
struct Example {
    a: u8,      // offset 0, size 1
    b: u32,     // offset 4, size 4 (3 bytes padding after a)
    c: u16,     // offset 8, size 2
}
// total: 10 bytes + 6 bytes tail padding = 16 bytes (aligned to 4)
```

| Rule | Description |
|------|-------------|
| **S1: Source order** | Fields laid out exactly as declared |
| **S2: No reordering** | Compiler doesn't reorder for optimal packing |
| **S3: Offset calculation** | `offset[i] = align_up(offset[i-1] + size[i-1], align[i])` |
| **S4: Total size** | `size = align_up(offset[last] + size[last], struct_align)` |

## Enums

Enums use tagged union layout: discriminant + union of variant payloads.

### Simple Enums (No Payloads)

```rask
enum Color { Red, Green, Blue }
```

Layout:
- Size: 1 byte (discriminant only)
- Discriminant: `u8` with values 0, 1, 2

### Enums with Payloads

```rask
enum Result<T, E> {
    Ok(T),      // variant 0
    Err(E),     // variant 1
}
```

Layout structure:
```
[discriminant: u8 or u16][padding][payload union]
```

| Rule | Description |
|------|-------------|
| **E1: Discriminant first** | Tag stored as first field |
| **E2: Discriminant size** | `u8` (≤256 variants), `u16` (≤65536 variants) |
| **E3: Union layout** | All variant payloads stored in same memory location |
| **E4: Union size** | Size = max(size of all variants) |
| **E5: Enum alignment** | Alignment = max(discriminant_align, max variant alignment) |
| **E6: Padding after tag** | Discriminant padded to alignment of largest variant |

Example: `Result<i32, string>`

```
Result<i32, string>:
  discriminant: u8 at offset 0
  padding: 7 bytes (to align to 8-byte string)
  payload union at offset 8:
    - Ok variant: i32 (4 bytes)
    - Err variant: string (16 bytes: ptr + len)

Total size: 8 (discriminant+padding) + 16 (union) = 24 bytes
Alignment: 8 bytes (string's alignment)
```

### Discriminant Values

Variants numbered sequentially from 0 in source order:

```rask
enum Token {
    Plus,     // discriminant = 0
    Minus,    // discriminant = 1
    Star,     // discriminant = 2
}
```

### Niche Optimization

Certain types have unused bit patterns that can encode enum discriminants without extra storage.

| Rule | Description |
|------|-------------|
| **N1: Handle niche** | `Option<Handle<T>>` uses generation=0 to represent None (8 bytes, not 16) |
| **N2: Reference niche** | `Option<&T>` uses null pointer for None (8 bytes, not 16) |
| **N3: NonZero types** | Future: `Option<NonZeroU32>` etc. use zero as None |

**Option<Handle<T>> Layout:**
```
// Without niche (naive):       16 bytes = [tag: u8][pad: 7][Handle: 8]
// With niche (optimized):        8 bytes = [index: u32][generation: u32]
//   where generation=0 means None, generation>0 means Some
```

**Priority:** Handle niche optimization MUST be implemented before ABI stabilization. This is critical for graph algorithms using Pool handles.

## Closures

Closures are structs containing captured values plus a function pointer.

### Storable Closures

```rask
const x = 42
const y = 3.14
const f = |a: i32| -> i32 { a + x }
```

Generated struct:
```rask
struct Closure_f {
    x: i32,              // captured value
    fn_ptr: *u8,         // pointer to generated function
}
```

| Rule | Description |
|------|-------------|
| **CL1: Struct layout** | Closure = struct of captured values + function pointer |
| **CL2: Capture order** | Captures ordered by first use in closure body (declaration order) |
| **CL3: Function pointer last** | Function pointer stored as final field |
| **CL4: Zero-size if no captures** | Pure closures (no captures) = single function pointer |

Layout example:
```rask
const name = "Alice"
const age = 30
const greet = |msg: string| print("{msg}, {name}, age {age}")
```

Generated:
```rask
struct Closure_greet {
    name: string,        // offset 0, size 16 (first use in body)
    age: i32,            // offset 16, size 4 (second use in body)
    fn_ptr: *u8,         // offset 24, size 8
}
// Total: 32 bytes
```

Function signature: `fn(Closure_greet*, string) -> ()`

### Immediate Closures

No memory allocation. Closure accesses stack frame directly. Codegen inlines the closure body at call site when possible.

### Local-Only Closures

Same as storable closures, but type system prevents escape. Memory layout identical.

## Trait Objects (`any Trait`)

Trait objects are fat pointers: data pointer + vtable pointer.

```rask
const w: any Widget = button
```

Layout:
```
struct TraitObject {
    data: *u8,       // pointer to actual object
    vtable: *VTable, // pointer to vtable
}
// Total: 16 bytes (two pointers)
```

| Rule | Description |
|------|-------------|
| **TR1: Fat pointer** | Two-word value: data pointer + vtable pointer |
| **TR2: Data pointer first** | `*T` at offset 0 |
| **TR3: Vtable pointer second** | `*VTable` at offset 8 |
| **TR4: Alignment** | 8 bytes (pointer alignment) |

### VTable Layout

VTable = method pointer table + type metadata.

```
struct VTable {
    size: usize,              // offset 0: size of concrete type
    align: usize,             // offset 8: alignment of concrete type
    drop_fn: *u8,             // offset 16: destructor (null if trivial)
    method_1: *u8,            // offset 24: first method
    method_2: *u8,            // offset 32: second method
    ...                       // more methods
}
```

| Rule | Description |
|------|-------------|
| **V1: Type info first** | Size and alignment at fixed offsets (0, 8) |
| **V2: Drop function** | Drop function at offset 16 (null if type has trivial drop) |
| **V3: Method order** | Methods stored in trait declaration order |
| **V4: Per-type vtable** | One vtable per (trait, concrete type) pair |
| **V5: Static lifetime** | Vtables stored in read-only data section |

Example vtable for `Button: Widget`:

```rask
trait Widget {
    draw(self, canvas: Canvas)
    size(self) -> (i32, i32)
    click(self, x: i32, y: i32)
}
```

Vtable:
```
Button_Widget_vtable:
  [0]  size: 64              // sizeof(Button)
  [8]  align: 8              // alignof(Button)
  [16] drop: null            // Button has trivial drop
  [24] draw: &Button_draw    // function pointer
  [32] size: &Button_size    // function pointer
  [40] click: &Button_click  // function pointer
```

Method call translation:
```rask
widget.draw(canvas)
↓
vtable = widget.vtable
fn_ptr = vtable[24]  // offset of draw method
fn_ptr(widget.data, canvas)
```

## Arrays

Fixed-size arrays store elements inline, no indirection.

```rask
const arr: [i32; 4] = [1, 2, 3, 4]
```

| Rule | Description |
|------|-------------|
| **A1: Inline storage** | Elements stored contiguously |
| **A2: No padding between** | Elements packed with natural alignment |
| **A3: Total size** | `size = element_size * count` |
| **A4: Alignment** | Same as element alignment |

## Tuples

Tuples follow struct layout rules: elements in order, padded for alignment.

```rask
const t: (u8, i32, u16) = (1, 2, 3)
```

Layout:
```
offset 0: u8    (1 byte)
offset 1: pad   (3 bytes)
offset 4: i32   (4 bytes)
offset 8: u16   (2 bytes)
offset 10: pad  (2 bytes, tail padding to align to 4)
Total: 12 bytes, alignment 4
```

## Dynamic Collections

### Vec<T>

```rask
struct Vec<T> {
    ptr: *T,            // offset 0, 8 bytes
    len: usize,         // offset 8, 8 bytes
    cap: usize,         // offset 16, 8 bytes
}
// Total: 24 bytes
```

Heap layout:
```
[element_0][element_1][...][element_len-1][unused capacity...]
```

### Map<K, V>

Implementation-defined hash table layout. Struct contains:

```rask
struct Map<K, V> {
    buckets: *Bucket<K, V>,   // offset 0
    len: usize,               // offset 8
    cap: usize,               // offset 16
}
// Total: 24 bytes
```

Internal bucket structure and hashing strategy are implementation-defined and subject to change. Do not depend on internal layout.

### Pool<T>

```rask
struct Pool<T> {
    data: *T,              // offset 0: dense array
    len: usize,            // offset 8: element count
    cap: usize,            // offset 16: capacity
    free_head: u32,        // offset 24: freelist head
    generation: *u32,      // offset 32: generation array
}
// Total: 40 bytes
```

Handle:
```rask
struct Handle<T> {
    index: u32,         // offset 0: slot index
    generation: u32,    // offset 4: generation counter
}
// Total: 8 bytes
```

## String

Built-in `string` type:

```rask
struct string {
    ptr: *u8,          // offset 0, 8 bytes
    len: usize,        // offset 8, 8 bytes
}
// Total: 16 bytes
```

Heap data is UTF-8 bytes, no null terminator unless needed for FFI.

## References and Slices

References and slices are ephemeral (expression-scoped). Not stored in structs, but layout documented for calling convention:

### Reference `&T`

Single pointer (8 bytes).

### Slice `[]T`

Fat pointer:
```rask
struct Slice<T> {
    ptr: *T,           // offset 0
    len: usize,        // offset 8
}
// Total: 16 bytes
```

### String Slice `str`

Same as `[]u8`: pointer + length (16 bytes).

## Zero-Sized Types (ZST)

Types with no fields have zero size:

```rask
struct Unit {}
// size: 0, alignment: 1
```

| Rule | Description |
|------|-------------|
| **Z1: No allocation** | ZSTs never allocated on heap |
| **Z2: Distinct addresses** | Each instance may have distinct address (unspecified) |
| **Z3: Vec optimization** | `Vec<()>` stores only length, no pointer/capacity |

## Padding and Size Examples

### Example 1: Struct with Mixed Sizes

```rask
struct Example {
    a: u8,              // offset 0
    b: u64,             // offset 8 (7 bytes padding)
    c: u16,             // offset 16
    d: u8,              // offset 18
}
// Total: offset 18 + 1 = 19, padded to 24 (alignment 8)
```

### Example 2: Enum with Large Variant

```rask
enum Message {
    Ping,                           // no payload
    Data([u8; 1024]),               // 1024 bytes
    Ack(u32),                       // 4 bytes
}
```

Layout:
```
discriminant: u8 at offset 0
padding: 7 bytes
payload union at offset 8: 1024 bytes (max variant size)
Total: 1032 bytes
```

### Example 3: Closure Capturing Multiple Values

```rask
const x: u8 = 1
const y: u64 = 2
const z: u32 = 3
const f = |a: i32| a + z
```

Generated:
```rask
struct Closure_f {
    z: u32,             // offset 0 (only capture, used in body)
    fn_ptr: *u8,        // offset 8 (padded)
}
// Total: 16 bytes
```

Note: `x` and `y` not captured (not used in body).

## Calling Convention

Function calls follow System V AMD64 ABI on Linux, Windows x64 calling convention on Windows.

| Arguments | Location |
|-----------|----------|
| First 6 integers/pointers | RDI, RSI, RDX, RCX, R8, R9 |
| First 8 floats | XMM0-XMM7 |
| Remaining arguments | Stack (right-to-left) |
| Return value (≤16 bytes) | RAX, RDX |
| Return value (>16 bytes) | Pointer passed in RDI |

**Fat pointer calling convention:**

Fat pointers (trait objects, slices) are passed as two consecutive register arguments:
- **Trait object `any T`**: data pointer in first register, vtable pointer in second register
  - Example: `func f(w: any Widget)` → data in RDI, vtable in RSI
- **Slice `[]T`**: data pointer in first register, length in second register
  - Example: `func f(s: []i32)` → data in RDI, len in RSI

Fat pointers consume two argument slots. Example:
```rask
func process(x: i32, s: []u8, y: i32)
```
Arguments: `x` in RDI, `s.ptr` in RSI, `s.len` in RDX, `y` in RCX

## Codegen Validation

The compiler must:
1. Calculate sizes and offsets at compile time
2. Insert padding to maintain alignment invariants
3. Generate field access code with correct offsets
4. Emit vtables with correct method ordering

Test suite should validate:
- Struct sizes match manual calculation
- Enum discriminant values are sequential
- Vtable method offsets are correct
- Closure captures are alphabetically ordered

## Error Messages

Size/alignment errors surface during monomorphization:

```
ERROR [compiler.layout/L1]: type too large
  |
5 | struct BigStruct { data: [u8; 2^31] }
  |                           ^^^^^^^^^ exceeds maximum size

WHY: Types larger than 2^31 bytes cannot be allocated.

FIX: Use heap indirection or split into smaller chunks.
```

## See Also

- `type.enums` — Enum semantics and discriminant rules
- `type.traits` — Trait objects and object safety
- `mem.closures` — Closure capture semantics
- `mem.value` — Copy vs move threshold (16 bytes)
