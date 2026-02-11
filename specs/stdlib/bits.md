<!-- id: std.bits -->
<!-- status: decided -->
<!-- summary: Bit manipulation, byte order conversion, binary parsing and building -->

# Bits Module

Bit manipulation utilities, byte order conversion, and binary data parsing/building on integer types and byte slices.

## Bit Operations

Methods on all integer types.

| Rule | Description |
|------|-------------|
| **B1: Integer methods** | `popcount`, `leading_zeros`, `trailing_zeros`, `leading_ones`, `trailing_ones`, `reverse_bits`, `rotate_left`, `rotate_right`, `swap_bytes` are methods on all integer types |

<!-- test: skip -->
```rask
const x: u32 = 0b1100_0000_0000_0000_0000_0000_0000_0011

x.popcount()        // 4
x.leading_zeros()   // 0
x.trailing_zeros()  // 0
x.reverse_bits()    // bit-reversed value
```

## Byte Order Conversion

| Rule | Description |
|------|-------------|
| **B2: Endian methods** | `to_be`, `to_le`, `from_be`, `from_le` convert integer byte order |
| **B3: Byte array methods** | `to_be_bytes`, `to_le_bytes`, `to_ne_bytes` produce byte arrays; `T.from_be_bytes`, `T.from_le_bytes`, `T.from_ne_bytes` parse them |
| **B4: Network aliases** | `bits.hton_*` and `bits.ntoh_*` are aliases for big-endian conversion |

<!-- test: skip -->
```rask
const port: u16 = 8080
const be_bytes = port.to_be_bytes()   // [0x1F, 0x90]
const p1 = u16.from_be_bytes([0x1F, 0x90])  // 8080
```

Network byte order aliases:

| Function | Equivalent |
|----------|------------|
| `bits.hton_u16(x)` | `x.to_be()` |
| `bits.hton_u32(x)` | `x.to_be()` |
| `bits.ntoh_u16(x)` | `u16.from_be(x)` |
| `bits.ntoh_u32(x)` | `u32.from_be(x)` |

## Binary Parsing

| Rule | Description |
|------|-------------|
| **P1: unpack** | `data.unpack(types...)` parses multiple values from a byte slice, returns `(T..., []u8) or ParseError` |
| **P2: Type specifiers** | Specifiers encode type and endianness: `u8`, `u16be`, `u32le`, `f64be`, etc. |
| **P3: Slice read methods** | `data.read_u8()`, `data.read_u16be()`, etc. return `(T, []u8) or ParseError` |
| **P4: take** | `data.take(n)` splits off first n bytes: `([]u8, []u8)` |

<!-- test: skip -->
```rask
// Variadic unpack
const (magic, version, length, rest) = try data.unpack(u32be, u8, u16be)

// Incremental read
const (magic, rest) = try data.read_u32be()
const (length, rest) = try rest.read_u16be()
const (payload, rest) = try rest.take(length as usize)
```

Type specifiers for `unpack`: `u8`, `i8`, `u16be`, `u16le`, `i16be`, `i16le`, `u32be`, `u32le`, `i32be`, `i32le`, `u64be`, `u64le`, `i64be`, `i64le`, `f32be`, `f32le`, `f64be`, `f64le`.

## Binary Building

| Rule | Description |
|------|-------------|
| **K1: pack** | `pack(values...)` builds a `Vec<u8>` from typed values |
| **K2: BinaryBuilder** | Builder pattern for incremental construction via `write_*` methods |
| **K3: Buffer write** | `buffer[cursor..].write_*(value)` for zero-allocation building, returns bytes written |

<!-- test: skip -->
```rask
// pack
const header = pack(u32be(0xCAFEBABE), u8(1), u16be(payload.len()))

// Builder
const data = BinaryBuilder.new()
    .write_u32be(0xCAFEBABE)
    .write_u8(1)
    .write_bytes(payload)
    .build()

// Zero-alloc buffer write
const buffer: [u8; 64] = [0; 64]
let cursor = 0
cursor += buffer[cursor..].write_u32be(0xCAFEBABE)
cursor += buffer[cursor..].write_u8(1)
```

## Error Types

| Rule | Description |
|------|-------------|
| **E1: ParseError** | Parsing operations return `T or ParseError` |

<!-- test: skip -->
```rask
enum ParseError {
    UnexpectedEnd { expected: usize, actual: usize }
    InvalidData { message: string }
}
```

## Error Messages

```
ERROR [std.bits/P1]: unexpected end of data
   |
5  |  const (magic, ver, rest) = try data.unpack(u32be, u8)
   |                                 ^^^^ expected 5 bytes, got 3

WHY: unpack validates all lengths upfront before parsing.

FIX: Check data length before unpacking.
```

```
ERROR [std.bits/P3]: unexpected end of data
   |
3  |  const (val, rest) = try data.read_u32be()
   |                            ^^^^ need 4 bytes, have 2

WHY: Read methods require enough bytes for the target type.

FIX: Verify slice length or handle the error with try/match.
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Empty slice to `unpack` | P1 | Returns `ParseError.UnexpectedEnd` |
| Zero-length `take(0)` | P4 | Returns `(empty_slice, original)` |
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

- Parsing is zero-copy where possible (returns slices into original data)
- `unpack` validates all lengths upfront (single bounds check)
- Builder pre-allocates when total size is known at comptime

### See Also

- `std.collections` — Vec used by pack/BinaryBuilder
- `type.structs` — `@binary` struct attribute for reusable layouts
