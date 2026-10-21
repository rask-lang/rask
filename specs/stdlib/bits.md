<!-- id: std.bits -->
<!-- status: decided -->
<!-- summary: Bit manipulation, byte order conversion, binary parsing and building -->

# Bits Module

Bit manipulation utilities, byte order conversion, and binary data parsing/building on integer types and `Vec<u8>`.

The surface blocks below write the integer methods once, as `extend Integer`. That stands for every width, `i8` through `u128`, with `Self` as the width and `BYTES` as its size in bytes. `isize` and `usize` are `i64` and `u64` under another name, so their methods are those types' methods.

## Bit Operations

Methods on all integer types.

| Rule | Description |
|------|-------------|
| **B1: Integer methods** | `count_ones`, `count_zeros`, `leading_zeros`, `trailing_zeros`, `leading_ones`, `trailing_ones`, `reverse_bits`, `rotate_left`, `rotate_right`, `swap_bytes` are methods on all integer types. Each answers at the receiver's own width: `(0 as u8).leading_zeros()` is 8, not 64. A rotation turns by `n` modulo the width, so rotating by the width or more never panics |

<!-- surface -->
```rask
extend Integer {
    func count_ones(self) -> Self
    func count_zeros(self) -> Self
    func leading_zeros(self) -> Self
    func trailing_zeros(self) -> Self
    func leading_ones(self) -> Self
    func trailing_ones(self) -> Self
    func reverse_bits(self) -> Self
    func swap_bytes(self) -> Self
    func rotate_left(self, n: Self) -> Self
    func rotate_right(self, n: Self) -> Self
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
extend Integer {
    func to_be(self) -> Self
    func to_le(self) -> Self
    func from_be(x: Self) -> Self
    func from_le(x: Self) -> Self
    func to_be_bytes(self) -> [u8; BYTES]
    func to_le_bytes(self) -> [u8; BYTES]
    func to_ne_bytes(self) -> [u8; BYTES]
    func from_be_bytes(bytes: [u8; BYTES]) -> Self
    func from_le_bytes(bytes: [u8; BYTES]) -> Self
    func from_ne_bytes(bytes: [u8; BYTES]) -> Self
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
| **P3: Read methods** | `data.read_u8()`, `data.read_u16be()`, … read one value off the front of a `Vec<u8>` and return `(T, Vec<u8>) or BinaryParseError`: the value and what's left. One per type and byte order: `u8`, `i8`, then `u16`, `i16`, `u32`, `i32`, `u64`, `i64`, `f32`, `f64`, each with a `be` and an `le` form |
| **P4: read_bytes** | `data.read_bytes(n)` splits off the first `n` bytes: `(Vec<u8>, Vec<u8>) or BinaryParseError` |
| **P5: Short input** | Reading more bytes than there are is `BinaryParseError.UnexpectedEnd(expected, actual)`: how many the read needed, how many were there. Nothing panics |

<!-- surface -->
```rask
extend Vec<u8> {
    func read_u8(self) -> (u8, Vec<u8>) or BinaryParseError
    func read_i8(self) -> (i8, Vec<u8>) or BinaryParseError
    func read_u16be(self) -> (u16, Vec<u8>) or BinaryParseError
    func read_u16le(self) -> (u16, Vec<u8>) or BinaryParseError
    func read_i16be(self) -> (i16, Vec<u8>) or BinaryParseError
    func read_i16le(self) -> (i16, Vec<u8>) or BinaryParseError
    func read_u32be(self) -> (u32, Vec<u8>) or BinaryParseError
    func read_u32le(self) -> (u32, Vec<u8>) or BinaryParseError
    func read_i32be(self) -> (i32, Vec<u8>) or BinaryParseError
    func read_i32le(self) -> (i32, Vec<u8>) or BinaryParseError
    func read_u64be(self) -> (u64, Vec<u8>) or BinaryParseError
    func read_u64le(self) -> (u64, Vec<u8>) or BinaryParseError
    func read_i64be(self) -> (i64, Vec<u8>) or BinaryParseError
    func read_i64le(self) -> (i64, Vec<u8>) or BinaryParseError
    func read_f32be(self) -> (f32, Vec<u8>) or BinaryParseError
    func read_f32le(self) -> (f32, Vec<u8>) or BinaryParseError
    func read_f64be(self) -> (f64, Vec<u8>) or BinaryParseError
    func read_f64le(self) -> (f64, Vec<u8>) or BinaryParseError
    func read_bytes(self, n: usize) -> (Vec<u8>, Vec<u8>) or BinaryParseError
}
```

<!-- test: skip -->
```rask
let (magic, rest) = try data.read_u32be()
let (version, rest) = try rest.read_u8()
let (length, rest) = try rest.read_u16be()
let (payload, rest) = try rest.read_bytes(length)
```

P4 used to be `take(n)`, infallible. Two things were wrong with it. A short buffer has to fail somewhere, and an infallible `take` could only panic, while everything around it returns an error, so the example already wrote `try` in front of it. And `Vec.take(n)` is the sequence method that lazily yields the first `n` elements; one name meaning two different things on the same receiver is a trap. `read_bytes` is the byte-count sibling of the other `read_*` methods and fails the same way they do.

There's no variadic `unpack(u32be, u8, u16be)` and no `pack(...)`. Rask has no variadic functions, and those two would need one plus type specifiers passed as values: a language feature for one module. The read chain above is one line per field, and a layout used more than once is a `@binary` struct (`type.binary`), which checks the layout at compile time.

## Binary Building

| Rule | Description |
|------|-------------|
| **K2: BinaryBuilder** | Builder for incremental construction. One `write_*` per type and byte order, the same set P3 reads, plus `write_bytes` |
| **K3: Buffer write** | `buffer[cursor..].write_*(value)` for zero-allocation building, returns bytes written |

<!-- surface -->
```rask
struct BinaryBuilder { }

extend BinaryBuilder {
    func new() -> BinaryBuilder
    func with_capacity(n: usize) -> BinaryBuilder
    func write_u8(take self, value: u8) -> BinaryBuilder
    func write_i8(take self, value: i8) -> BinaryBuilder
    func write_u16be(take self, value: u16) -> BinaryBuilder
    func write_u16le(take self, value: u16) -> BinaryBuilder
    func write_i16be(take self, value: i16) -> BinaryBuilder
    func write_i16le(take self, value: i16) -> BinaryBuilder
    func write_u32be(take self, value: u32) -> BinaryBuilder
    func write_u32le(take self, value: u32) -> BinaryBuilder
    func write_i32be(take self, value: i32) -> BinaryBuilder
    func write_i32le(take self, value: i32) -> BinaryBuilder
    func write_u64be(take self, value: u64) -> BinaryBuilder
    func write_u64le(take self, value: u64) -> BinaryBuilder
    func write_i64be(take self, value: i64) -> BinaryBuilder
    func write_i64le(take self, value: i64) -> BinaryBuilder
    func write_f32be(take self, value: f32) -> BinaryBuilder
    func write_f32le(take self, value: f32) -> BinaryBuilder
    func write_f64be(take self, value: f64) -> BinaryBuilder
    func write_f64le(take self, value: f64) -> BinaryBuilder
    func write_bytes(take self, bytes: Vec<u8>) -> BinaryBuilder
    func len(self) -> usize
    func build(take self) -> Vec<u8>
}
```

<!-- test: skip -->
```rask
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
| **E1: BinaryParseError** | Parsing operations return `T or BinaryParseError` |

<!-- surface -->
```rask
enum BinaryParseError {
    UnexpectedEnd(expected: usize, actual: usize)
    InvalidData(message: string)
}

extend BinaryParseError {
    func message(self) -> string
}
```

It isn't called `ParseError`: `std.strings` already has one, for text, and a program that parses numbers out of a binary header uses both.

## Error Messages

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
| Empty input to any `read_*` | P5 | `BinaryParseError.UnexpectedEnd(expected, 0)` |
| `read_bytes(0)` | P4 | `(empty, original)` |
| `read_bytes(n)` past the end | P5 | `UnexpectedEnd(n, len)` |
| `rotate_left(n)` with `n >= width` | B1 | Turns by `n % width` |
| `leading_zeros()` of 0 | B1 | The width |
| Buffer too small for write | K3 | Panics (bounds check) |

---

## Appendix (non-normative)

### Rationale

**K3 (buffer write):** Embedded and network code often needs zero-allocation binary building. Cursor-based writes into a stack buffer avoid heap allocation entirely.

**B2/B3 in Rask:** only `to_be` and `to_le` know the host's byte order. Everything else in B2 and B3 is written in Rask on top of them and of shifts, once for every width.

### Patterns & Guidance

**Choosing between approaches:**

| Need | Use |
|------|-----|
| Reusable, documented binary layout | `@binary struct` |
| One-off inline parse | `read_*` chain |
| Build fixed-size packets | Buffer writes |
| Build variable-size data | `BinaryBuilder` |

### Performance

- Each `read_*` hands back the remainder as a new `Vec<u8>`, so a chain of reads copies the tail once per read. Fine for headers; for a large body, read the header and then index the rest
- `to_*_bytes` and `from_*_bytes` are a byte swap and shifts; nothing allocates

### See Also

- `std.collections` — Vec used by BinaryBuilder
- `type.binary` — `@binary` struct attribute for reusable layouts
