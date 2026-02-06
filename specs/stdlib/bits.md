# Bits Module

## Overview

The `bits` module provides bit manipulation utilities, byte order conversion, and binary data parsing/building helpers.

## Bit Operations

Methods available on all integer types:

| Method | Description |
|--------|-------------|
| `x.popcount()` | Count set bits (population count) |
| `x.leading_zeros()` | Count leading zero bits |
| `x.trailing_zeros()` | Count trailing zero bits |
| `x.leading_ones()` | Count leading one bits |
| `x.trailing_ones()` | Count trailing one bits |
| `x.reverse_bits()` | Reverse bit order |
| `x.rotate_left(n)` | Rotate bits left by n positions |
| `x.rotate_right(n)` | Rotate bits right by n positions |
| `x.swap_bytes()` | Reverse byte order |

```rask
let x: u32 = 0b1100_0000_0000_0000_0000_0000_0000_0011

x.popcount()        // 4
x.leading_zeros()   // 0
x.trailing_zeros()  // 0
x.leading_ones()    // 2
x.trailing_ones()   // 2
x.reverse_bits()    // 0b1100_0000...0011 (reversed)
```

## Byte Order Conversion

Methods for explicit endianness conversion:

| Method | Description |
|--------|-------------|
| `x.to_be()` | Convert to big-endian byte order |
| `x.to_le()` | Convert to little-endian byte order |
| `x.from_be()` | Convert from big-endian byte order |
| `x.from_le()` | Convert from little-endian byte order |
| `x.to_be_bytes()` | Convert to big-endian byte array |
| `x.to_le_bytes()` | Convert to little-endian byte array |
| `x.to_ne_bytes()` | Convert to native-endian byte array |
| `T.from_be_bytes(b)` | Parse from big-endian bytes |
| `T.from_le_bytes(b)` | Parse from little-endian bytes |
| `T.from_ne_bytes(b)` | Parse from native-endian bytes |

```rask
let port: u16 = 8080

// To bytes
const be_bytes = port.to_be_bytes()   // [0x1F, 0x90]
const le_bytes = port.to_le_bytes()   // [0x90, 0x1F]

// From bytes
const p1 = u16.from_be_bytes([0x1F, 0x90])  // 8080
const p2 = u16.from_le_bytes([0x90, 0x1F])  // 8080
```

### Network Byte Order

Aliases for network programming (big-endian):

| Function | Equivalent |
|----------|------------|
| `bits.hton_u16(x)` | `x.to_be()` |
| `bits.hton_u32(x)` | `x.to_be()` |
| `bits.ntoh_u16(x)` | `u16.from_be(x)` |
| `bits.ntoh_u32(x)` | `u32.from_be(x)` |

## Binary Parsing

For parsing binary data without defining a `@binary` struct:

### `unpack`

Parse multiple values from a byte slice:

```rask
// Signature (variadic generic)
func unpack<T...>(data: []u8, types: T...) -> (T..., []u8) or ParseError

// Usage
let (magic, version, length, rest) = try data.unpack(u32be, u8, u16be)

// With match
match data.unpack(u32be, u8, u16be) {
    Ok(0xCAFEBABE, 1, len, rest): process(len, rest)
    Ok(_, _, _, _): Err(BadHeader)
    Err(e): Err(e)
}
```

Type specifiers for `unpack`:
- `u8`, `i8` — single byte
- `u16be`, `u16le`, `i16be`, `i16le` — 16-bit with endianness
- `u32be`, `u32le`, `i32be`, `i32le` — 32-bit with endianness
- `u64be`, `u64le`, `i64be`, `i64le` — 64-bit with endianness
- `f32be`, `f32le`, `f64be`, `f64le` — floats with endianness

### Slice Methods

Methods on `[]u8` for binary parsing:

| Method | Description |
|--------|-------------|
| `data.take(n)` | Split off first n bytes: `([]u8, []u8)` |
| `data.read_u8()` | Read u8, return `(u8, []u8)` |
| `data.read_u16be()` | Read big-endian u16 |
| `data.read_u16le()` | Read little-endian u16 |
| `data.read_u32be()` | Read big-endian u32 |
| `data.read_u32le()` | Read little-endian u32 |
| `data.read_u64be()` | Read big-endian u64 |
| `data.read_u64le()` | Read little-endian u64 |

All read methods return `Result<(T, []u8), ParseError>`.

```rask
let (magic, rest) = try data.read_u32be()
let (length, rest) = try rest.read_u16be()
let (payload, rest) = try rest.take(length as usize)
```

## Binary Building

### `pack`

Build binary data from values:

```rask
// Signature (variadic generic)
func pack<T...>(values: T...) -> Vec<u8>

// Usage
const header = pack(u32be(0xCAFEBABE), u8(1), u16be(payload.len()))
```

### Builder Pattern

For incremental construction:

```rask
const data = BinaryBuilder.new()
    .write_u32be(0xCAFEBABE)
    .write_u8(1)
    .write_u16be(payload.len())
    .write_bytes(payload)
    .build()
```

### Write to Buffer

For zero-allocation building:

```rask
const buffer: [u8; 64] = [0; 64]
let cursor = 0

cursor += buffer[cursor..].write_u32be(0xCAFEBABE)
cursor += buffer[cursor..].write_u8(1)
cursor += buffer[cursor..].write_u16be(length)
```

## Error Types

```rask
enum ParseError {
    UnexpectedEnd { expected: usize, actual: usize }
    InvalidData { message: string }
}
```

## Integration with @binary Structs

The `bits` module complements `@binary` structs:

- Use `@binary struct` for reusable, documented layouts
- Use `bits.unpack` for one-off inline parsing
- Use slice methods for streaming/incremental parsing

```rask
// Struct for well-known format
@binary
struct TcpHeader { ... }

// Inline for quick one-off
let (type, len, rest) = try data.unpack(u8, u16be)

// Streaming for variable data
let (header, rest) = try TcpHeader.parse(data)
let (options, rest) = try rest.take(header.data_offset * 4 - 20)
```

## Performance Notes

- All parsing is zero-copy where possible (returns slices into original data)
- `unpack` validates all lengths upfront (single bounds check)
- Builder pre-allocates when total size is known at comptime
