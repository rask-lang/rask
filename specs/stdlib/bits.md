<!-- id: std.bits -->
<!-- status: decided -->
<!-- summary: Bit manipulation, byte order conversion, binary parsing and building -->

# Bits Module

Bit manipulation utilities, byte order conversion, and binary data parsing/building on integer types and `Vec<u8>`.

## Bit Operations

Methods on all integer types.

| Rule | Description |
|------|-------------|
| **B1: Integer methods** | `count_ones`, `count_zeros`, `leading_zeros`, `trailing_zeros`, `leading_ones`, `trailing_ones`, `reverse_bits`, `rotate_left`, `rotate_right`, `swap_bytes` are methods on all integer types |

<!-- surface -->
```rask
extend i8 {
    func count_ones(self) -> i8
    func count_zeros(self) -> i8
    func leading_zeros(self) -> i8
    func trailing_zeros(self) -> i8
    func leading_ones(self) -> i8
    func trailing_ones(self) -> i8
    func reverse_bits(self) -> i8
    func swap_bytes(self) -> i8
    func rotate_left(self, n: i8) -> i8
    func rotate_right(self, n: i8) -> i8
}

extend u8 {
    func count_ones(self) -> u8
    func count_zeros(self) -> u8
    func leading_zeros(self) -> u8
    func trailing_zeros(self) -> u8
    func leading_ones(self) -> u8
    func trailing_ones(self) -> u8
    func reverse_bits(self) -> u8
    func swap_bytes(self) -> u8
    func rotate_left(self, n: u8) -> u8
    func rotate_right(self, n: u8) -> u8
}

extend i16 {
    func count_ones(self) -> i16
    func count_zeros(self) -> i16
    func leading_zeros(self) -> i16
    func trailing_zeros(self) -> i16
    func leading_ones(self) -> i16
    func trailing_ones(self) -> i16
    func reverse_bits(self) -> i16
    func swap_bytes(self) -> i16
    func rotate_left(self, n: i16) -> i16
    func rotate_right(self, n: i16) -> i16
}

extend u16 {
    func count_ones(self) -> u16
    func count_zeros(self) -> u16
    func leading_zeros(self) -> u16
    func trailing_zeros(self) -> u16
    func leading_ones(self) -> u16
    func trailing_ones(self) -> u16
    func reverse_bits(self) -> u16
    func swap_bytes(self) -> u16
    func rotate_left(self, n: u16) -> u16
    func rotate_right(self, n: u16) -> u16
}

extend i32 {
    func count_ones(self) -> i32
    func count_zeros(self) -> i32
    func leading_zeros(self) -> i32
    func trailing_zeros(self) -> i32
    func leading_ones(self) -> i32
    func trailing_ones(self) -> i32
    func reverse_bits(self) -> i32
    func swap_bytes(self) -> i32
    func rotate_left(self, n: i32) -> i32
    func rotate_right(self, n: i32) -> i32
}

extend u32 {
    func count_ones(self) -> u32
    func count_zeros(self) -> u32
    func leading_zeros(self) -> u32
    func trailing_zeros(self) -> u32
    func leading_ones(self) -> u32
    func trailing_ones(self) -> u32
    func reverse_bits(self) -> u32
    func swap_bytes(self) -> u32
    func rotate_left(self, n: u32) -> u32
    func rotate_right(self, n: u32) -> u32
}

extend i64 {
    func count_ones(self) -> i64
    func count_zeros(self) -> i64
    func leading_zeros(self) -> i64
    func trailing_zeros(self) -> i64
    func leading_ones(self) -> i64
    func trailing_ones(self) -> i64
    func reverse_bits(self) -> i64
    func swap_bytes(self) -> i64
    func rotate_left(self, n: i64) -> i64
    func rotate_right(self, n: i64) -> i64
}

extend u64 {
    func count_ones(self) -> u64
    func count_zeros(self) -> u64
    func leading_zeros(self) -> u64
    func trailing_zeros(self) -> u64
    func leading_ones(self) -> u64
    func trailing_ones(self) -> u64
    func reverse_bits(self) -> u64
    func swap_bytes(self) -> u64
    func rotate_left(self, n: u64) -> u64
    func rotate_right(self, n: u64) -> u64
}

extend i128 {
    func count_ones(self) -> i128
    func count_zeros(self) -> i128
    func leading_zeros(self) -> i128
    func trailing_zeros(self) -> i128
    func leading_ones(self) -> i128
    func trailing_ones(self) -> i128
    func reverse_bits(self) -> i128
    func swap_bytes(self) -> i128
    func rotate_left(self, n: i128) -> i128
    func rotate_right(self, n: i128) -> i128
}

extend u128 {
    func count_ones(self) -> u128
    func count_zeros(self) -> u128
    func leading_zeros(self) -> u128
    func trailing_zeros(self) -> u128
    func leading_ones(self) -> u128
    func trailing_ones(self) -> u128
    func reverse_bits(self) -> u128
    func swap_bytes(self) -> u128
    func rotate_left(self, n: u128) -> u128
    func rotate_right(self, n: u128) -> u128
}

extend isize {
    func count_ones(self) -> isize
    func count_zeros(self) -> isize
    func leading_zeros(self) -> isize
    func trailing_zeros(self) -> isize
    func leading_ones(self) -> isize
    func trailing_ones(self) -> isize
    func reverse_bits(self) -> isize
    func swap_bytes(self) -> isize
    func rotate_left(self, n: isize) -> isize
    func rotate_right(self, n: isize) -> isize
}

extend usize {
    func count_ones(self) -> usize
    func count_zeros(self) -> usize
    func leading_zeros(self) -> usize
    func trailing_zeros(self) -> usize
    func leading_ones(self) -> usize
    func trailing_ones(self) -> usize
    func reverse_bits(self) -> usize
    func swap_bytes(self) -> usize
    func rotate_left(self, n: usize) -> usize
    func rotate_right(self, n: usize) -> usize
}
```

<!-- test: skip -->
```rask
let x: u32 = 0b1100_0000_0000_0000_0000_0000_0000_0011

x.count_ones()      // 4
x.leading_zeros()   // 0
x.trailing_zeros()  // 0
x.reverse_bits()    // bit-reversed value
```

## Byte Order Conversion

| Rule | Description |
|------|-------------|
| **B2: Endian methods** | `to_be`, `to_le`, `from_be`, `from_le` convert integer byte order |
| **B3: Byte array methods** | `to_be_bytes`, `to_le_bytes`, `to_ne_bytes` produce byte arrays; `T.from_be_bytes`, `T.from_le_bytes`, `T.from_ne_bytes` parse them |

<!-- surface -->
```rask
extend i8 {
    func to_be(self) -> i8
    func to_le(self) -> i8
    func from_be(x: i8) -> i8
    func from_le(x: i8) -> i8
    func to_be_bytes(self) -> [u8; 1]
    func to_le_bytes(self) -> [u8; 1]
    func to_ne_bytes(self) -> [u8; 1]
    func from_be_bytes(bytes: [u8; 1]) -> i8
    func from_le_bytes(bytes: [u8; 1]) -> i8
    func from_ne_bytes(bytes: [u8; 1]) -> i8
}

extend u8 {
    func to_be(self) -> u8
    func to_le(self) -> u8
    func from_be(x: u8) -> u8
    func from_le(x: u8) -> u8
    func to_be_bytes(self) -> [u8; 1]
    func to_le_bytes(self) -> [u8; 1]
    func to_ne_bytes(self) -> [u8; 1]
    func from_be_bytes(bytes: [u8; 1]) -> u8
    func from_le_bytes(bytes: [u8; 1]) -> u8
    func from_ne_bytes(bytes: [u8; 1]) -> u8
}

extend i16 {
    func to_be(self) -> i16
    func to_le(self) -> i16
    func from_be(x: i16) -> i16
    func from_le(x: i16) -> i16
    func to_be_bytes(self) -> [u8; 2]
    func to_le_bytes(self) -> [u8; 2]
    func to_ne_bytes(self) -> [u8; 2]
    func from_be_bytes(bytes: [u8; 2]) -> i16
    func from_le_bytes(bytes: [u8; 2]) -> i16
    func from_ne_bytes(bytes: [u8; 2]) -> i16
}

extend u16 {
    func to_be(self) -> u16
    func to_le(self) -> u16
    func from_be(x: u16) -> u16
    func from_le(x: u16) -> u16
    func to_be_bytes(self) -> [u8; 2]
    func to_le_bytes(self) -> [u8; 2]
    func to_ne_bytes(self) -> [u8; 2]
    func from_be_bytes(bytes: [u8; 2]) -> u16
    func from_le_bytes(bytes: [u8; 2]) -> u16
    func from_ne_bytes(bytes: [u8; 2]) -> u16
}

extend i32 {
    func to_be(self) -> i32
    func to_le(self) -> i32
    func from_be(x: i32) -> i32
    func from_le(x: i32) -> i32
    func to_be_bytes(self) -> [u8; 4]
    func to_le_bytes(self) -> [u8; 4]
    func to_ne_bytes(self) -> [u8; 4]
    func from_be_bytes(bytes: [u8; 4]) -> i32
    func from_le_bytes(bytes: [u8; 4]) -> i32
    func from_ne_bytes(bytes: [u8; 4]) -> i32
}

extend u32 {
    func to_be(self) -> u32
    func to_le(self) -> u32
    func from_be(x: u32) -> u32
    func from_le(x: u32) -> u32
    func to_be_bytes(self) -> [u8; 4]
    func to_le_bytes(self) -> [u8; 4]
    func to_ne_bytes(self) -> [u8; 4]
    func from_be_bytes(bytes: [u8; 4]) -> u32
    func from_le_bytes(bytes: [u8; 4]) -> u32
    func from_ne_bytes(bytes: [u8; 4]) -> u32
}

extend i64 {
    func to_be(self) -> i64
    func to_le(self) -> i64
    func from_be(x: i64) -> i64
    func from_le(x: i64) -> i64
    func to_be_bytes(self) -> [u8; 8]
    func to_le_bytes(self) -> [u8; 8]
    func to_ne_bytes(self) -> [u8; 8]
    func from_be_bytes(bytes: [u8; 8]) -> i64
    func from_le_bytes(bytes: [u8; 8]) -> i64
    func from_ne_bytes(bytes: [u8; 8]) -> i64
}

extend u64 {
    func to_be(self) -> u64
    func to_le(self) -> u64
    func from_be(x: u64) -> u64
    func from_le(x: u64) -> u64
    func to_be_bytes(self) -> [u8; 8]
    func to_le_bytes(self) -> [u8; 8]
    func to_ne_bytes(self) -> [u8; 8]
    func from_be_bytes(bytes: [u8; 8]) -> u64
    func from_le_bytes(bytes: [u8; 8]) -> u64
    func from_ne_bytes(bytes: [u8; 8]) -> u64
}

extend i128 {
    func to_be(self) -> i128
    func to_le(self) -> i128
    func from_be(x: i128) -> i128
    func from_le(x: i128) -> i128
    func to_be_bytes(self) -> [u8; 16]
    func to_le_bytes(self) -> [u8; 16]
    func to_ne_bytes(self) -> [u8; 16]
    func from_be_bytes(bytes: [u8; 16]) -> i128
    func from_le_bytes(bytes: [u8; 16]) -> i128
    func from_ne_bytes(bytes: [u8; 16]) -> i128
}

extend u128 {
    func to_be(self) -> u128
    func to_le(self) -> u128
    func from_be(x: u128) -> u128
    func from_le(x: u128) -> u128
    func to_be_bytes(self) -> [u8; 16]
    func to_le_bytes(self) -> [u8; 16]
    func to_ne_bytes(self) -> [u8; 16]
    func from_be_bytes(bytes: [u8; 16]) -> u128
    func from_le_bytes(bytes: [u8; 16]) -> u128
    func from_ne_bytes(bytes: [u8; 16]) -> u128
}

extend isize {
    func to_be(self) -> isize
    func to_le(self) -> isize
    func from_be(x: isize) -> isize
    func from_le(x: isize) -> isize
    func to_be_bytes(self) -> [u8; 8]
    func to_le_bytes(self) -> [u8; 8]
    func to_ne_bytes(self) -> [u8; 8]
    func from_be_bytes(bytes: [u8; 8]) -> isize
    func from_le_bytes(bytes: [u8; 8]) -> isize
    func from_ne_bytes(bytes: [u8; 8]) -> isize
}

extend usize {
    func to_be(self) -> usize
    func to_le(self) -> usize
    func from_be(x: usize) -> usize
    func from_le(x: usize) -> usize
    func to_be_bytes(self) -> [u8; 8]
    func to_le_bytes(self) -> [u8; 8]
    func to_ne_bytes(self) -> [u8; 8]
    func from_be_bytes(bytes: [u8; 8]) -> usize
    func from_le_bytes(bytes: [u8; 8]) -> usize
    func from_ne_bytes(bytes: [u8; 8]) -> usize
}
```

<!-- test: skip -->
```rask
let port: u16 = 8080
let be_bytes = port.to_be_bytes()   // [0x1F, 0x90]
let p1 = u16.from_be_bytes([0x1F, 0x90])  // 8080
```

Network byte order is big-endian — use `x.to_be()` / `u16.from_be(x)` directly for network code. No separate `hton`/`ntoh` aliases.

## Float Bit Patterns

| Rule | Description |
|------|-------------|
| **B4: Float bits** | `x.to_bits()` is a float's IEEE 754 pattern at its own width: `u32` for `f32`, `u64` for `f64`. `f32.from_bits(u32)` and `f64.from_bits(u64)` go back. Every NaN is one quiet NaN in both directions (`0x7fc00000`, `0x7ff8000000000000`) |

<!-- surface -->
```rask
extend f32 {
    func to_bits(self) -> u32
    func from_bits(bits: u32) -> f32
}

extend f64 {
    func to_bits(self) -> u64
    func from_bits(bits: u64) -> f64
}
```

<!-- test: skip -->
```rask
let x: f32 = 1.5
x.to_bits()                     // 0x3fc00000
f64.from_bits(0x400921fb54442d18) // 3.141592653589793
```

I canonicalize NaN because a payload is whatever the hardware left in it. A program that keys a map on the bits, or puts them on the wire, would otherwise get a different answer per machine, and nothing a program can do with a NaN depends on which one it is.

## Binary Parsing

| Rule | Description |
|------|-------------|
| **P1: unpack** | `data.unpack(types...)` parses multiple values from the front of a `Vec<u8>`, returns `(T..., Vec<u8>) or ParseError` — the second half is what's left |
| **P2: Type specifiers** | Specifiers encode type and endianness: `u8`, `u16be`, `u32le`, `f64be`, etc. |
| **P3: Slice read methods** | `data.read_u8()`, `data.read_u16be()`, etc. return `(T, Vec<u8>) or ParseError` |
| **P4: take** | `data.take(n)` splits off first n bytes: `(Vec<u8>, Vec<u8>)` |

<!-- surface -->
```rask
extend Vec<u8> {
    func read_u8(self) -> (u8, Vec<u8>) or ParseError
    func read_i8(self) -> (i8, Vec<u8>) or ParseError
    func read_u16be(self) -> (u16, Vec<u8>) or ParseError
    func read_u16le(self) -> (u16, Vec<u8>) or ParseError
    func read_i16be(self) -> (i16, Vec<u8>) or ParseError
    func read_i16le(self) -> (i16, Vec<u8>) or ParseError
    func read_u32be(self) -> (u32, Vec<u8>) or ParseError
    func read_u32le(self) -> (u32, Vec<u8>) or ParseError
    func read_i32be(self) -> (i32, Vec<u8>) or ParseError
    func read_i32le(self) -> (i32, Vec<u8>) or ParseError
    func read_u64be(self) -> (u64, Vec<u8>) or ParseError
    func read_u64le(self) -> (u64, Vec<u8>) or ParseError
    func read_i64be(self) -> (i64, Vec<u8>) or ParseError
    func read_i64le(self) -> (i64, Vec<u8>) or ParseError
    func read_f32be(self) -> (f32, Vec<u8>) or ParseError
    func read_f32le(self) -> (f32, Vec<u8>) or ParseError
    func read_f64be(self) -> (f64, Vec<u8>) or ParseError
    func read_f64le(self) -> (f64, Vec<u8>) or ParseError
    func take(self, n: usize) -> (Vec<u8>, Vec<u8>)
}
```

<!-- test: skip -->
```rask
// Variadic unpack
let (magic, version, length, rest) = try data.unpack(u32be, u8, u16be)

// Incremental read
let (magic, rest) = try data.read_u32be()
let (length, rest) = try rest.read_u16be()
let (payload, rest) = try rest.take(length as usize)
```

Type specifiers for `unpack`: `u8`, `i8`, `u16be`, `u16le`, `i16be`, `i16le`, `u32be`, `u32le`, `i32be`, `i32le`, `u64be`, `u64le`, `i64be`, `i64le`, `f32be`, `f32le`, `f64be`, `f64le`.

## Binary Building

| Rule | Description |
|------|-------------|
| **K1: pack** | `bits.pack(values...)` builds a `Vec<u8>` from typed values. Module function — the inverse `unpack` is a method on the data because it has a source to hang off; `pack` doesn't |
| **K2: BinaryBuilder** | Builder pattern for incremental construction via `write_*` methods |
| **K3: Buffer write** | `buffer[cursor..].write_*(value)` for zero-allocation building, returns bytes written |

<!-- surface -->
```rask
struct BinaryBuilder { }

extend BinaryBuilder {
    func new() -> BinaryBuilder
    func write_u8(take self, value: u8) -> BinaryBuilder
    func write_u32be(take self, value: u32) -> BinaryBuilder
    func write_bytes(take self, bytes: Vec<u8>) -> BinaryBuilder
    func build(take self) -> Vec<u8>
}
```

<!-- test: skip -->
```rask
// pack
let header = pack(u32be(0xCAFEBABE), u8(1), u16be(payload.len()))

// Builder
let data = BinaryBuilder.new()
    .write_u32be(0xCAFEBABE)
    .write_u8(1)
    .write_bytes(payload)
    .build()

// Zero-alloc buffer write
let buffer: [u8; 64] = [0; 64]
mut cursor = 0
cursor += buffer[cursor..].write_u32be(0xCAFEBABE)
cursor += buffer[cursor..].write_u8(1)
```

## Error Types

| Rule | Description |
|------|-------------|
| **E1: ParseError** | Parsing operations return `T or ParseError` |

<!-- surface -->
```rask
enum ParseError {
    UnexpectedEnd(expected: usize, actual: usize)
    InvalidData(message: string)
}
```

## Error Messages

```
ERROR [std.bits/P1]: unexpected end of data
   |
5  |  let (magic, ver, rest) = try data.unpack(u32be, u8)
   |                                 ^^^^ expected 5 bytes, got 3

WHY: unpack validates all lengths upfront before parsing.

FIX: Check data length before unpacking.
```

```
ERROR [std.bits/P3]: unexpected end of data
   |
3  |  let (val, rest) = try data.read_u32be()
   |                            ^^^^ need 4 bytes, have 2

WHY: Read methods require enough bytes for the target type.

FIX: Check the length first, or handle the error with try/match.
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Empty input to `unpack` | P1 | Returns `ParseError.UnexpectedEnd` |
| Zero-length `take(0)` | P4 | Returns `(empty, original)` |
| `pack()` with no args | K1 | Returns empty `Vec<u8>` |
| Buffer too small for write | K3 | Panics (bounds check) |

---

## Appendix (non-normative)

### Rationale

**P1 (unpack):** Variadic unpack covers the common case of parsing a fixed header without defining a full `@binary` struct. Single bounds check upfront avoids per-field error handling.

**K3 (buffer write):** Embedded and network code often needs zero-allocation binary building. Cursor-based writes into a stack buffer avoid heap allocation entirely.

### Patterns & Guidance

**Choosing between approaches:**

| Need | Use |
|------|-----|
| Reusable, documented binary layout | `@binary struct` |
| One-off inline parse | `bits.unpack` |
| Streaming/incremental parse | Slice `read_*` methods |
| Build fixed-size packets | `pack` or buffer writes |
| Build variable-size data | `BinaryBuilder` |

### Performance

- Parsing reads in place where it can, and what it hands back is the remainder
- `unpack` validates all lengths upfront (single bounds check)
- Builder pre-allocates when total size is known at comptime

### See Also

- `std.collections` — Vec used by pack/BinaryBuilder
- `type.structs` — `@binary` struct attribute for reusable layouts
