<!-- id: type.binary -->
<!-- status: decided -->
<!-- summary: @binary attribute for struct-based binary parsing and building -->
<!-- depends: types/structs.md, control/comptime.md -->

# Binary Structs

`@binary` attribute on structs defines bit-level wire format. Compiler generates `.parse()` and `.build()` methods.

## Attribute and Layout

| Rule | Description |
|------|-------------|
| **B1: Declaration order** | Field order is the bit layout. No reordering |
| **B2: Sequential packing** | Fields packed sequentially, no padding |
| **B3: MSB-first** | Bits packed MSB-first (network byte order) |
| **B4: Final byte** | Unused bits in final byte zeroed on build, ignored on parse |

<!-- test: skip -->
```rask
@binary
struct IpHeader {
    version: 4            // 4 bits
    ihl: 4                // 4 bits
    dscp: 6               // 6 bits
    ecn: 2                // 2 bits
    total_length: u16be   // 16 bits big-endian
    identification: u16be
    flags: 3
    fragment_offset: 13
    ttl: u8
    protocol: u8
    checksum: u16be
    src: u32be
    dst: u32be
}
```

## Field Specifiers

| Rule | Description |
|------|-------------|
| **F1: Bare number** | `N` means N bits, becomes smallest fitting uint |
| **F2: Endian required** | Multi-byte fields must specify `be` or `le` — no default |
| **F3: Byte alignment** | Multi-byte endian types must start at byte boundaries |
| **F4: Nesting** | Binary structs can contain other binary structs (inlined, no indirection) |

| Specifier | Bits | Runtime Type | Description |
|-----------|------|--------------|-------------|
| `N` | N | smallest fitting uint | Bare number = N bits |
| `u8` | 8 | u8 | Unsigned byte |
| `i8` | 8 | i8 | Signed byte |
| `u16be`/`u16le` | 16 | u16 | Big/little-endian unsigned |
| `i16be`/`i16le` | 16 | i16 | Big/little-endian signed |
| `u32be`/`u32le` | 32 | u32 | Big/little-endian unsigned |
| `i32be`/`i32le` | 32 | i32 | Big/little-endian signed |
| `u64be`/`u64le` | 64 | u64 | Big/little-endian unsigned |
| `i64be`/`i64le` | 64 | i64 | Big/little-endian signed |
| `f32be`/`f32le` | 32 | f32 | Big/little-endian float |
| `f64be`/`f64le` | 64 | f64 | Big/little-endian double |
| `[N]u8` | N×8 | [u8; N] | Fixed byte array |

**Runtime type mapping for bare number `N`:**

| Bits | Runtime Type |
|------|--------------|
| 1-8 | u8 |
| 9-16 | u16 |
| 17-32 | u32 |
| 33-64 | u64 |

## Generated API

| Rule | Description |
|------|-------------|
| **G1: Parse** | `.parse(data)` returns `(T, []u8) or ParseError` |
| **G2: Build** | `.build()` returns `Vec<u8>` |
| **G3: Build into** | `.build_into(buffer)` returns `usize or BuildError` |
| **G4: Size constants** | `T.SIZE` (bytes, rounded up) and `T.SIZE_BITS` (bits) are comptime constants |

<!-- test: skip -->
```rask
extend T {
    func parse(data: []u8) -> (T, []u8) or ParseError
    func build(self) -> Vec<u8>
    func build_into(self, buffer: []u8) -> usize or BuildError
    const SIZE: usize
    const SIZE_BITS: usize
}
```

## Compile-Time Validation

| Rule | Description |
|------|-------------|
| **V1: Bit count range** | Fields must be 1-64 bits |
| **V2: Endian alignment** | Endian types must be byte-aligned within the struct |
| **V3: Size limit** | Total must not exceed 65535 bits (8KB) |
| **V4: Type fit** | Field values must fit in declared bit width |

## Inline Parsing

For one-off parsing without a struct, use `unpack`:

<!-- test: skip -->
```rask
let (magic, version, length, rest) = try data.unpack(u32be, u8, u16be)
```

See [stdlib/bits.md](../stdlib/bits.md) for `unpack`, `pack`, and related functions.

## Error Messages

```
ERROR [type.binary/F2]: multi-byte field must specify endianness
   |
5  |  port: u16
   |        ^^^ must be u16be or u16le

WHY: Implicit endianness causes bugs. Explicit is required.

FIX: port: u16be    // or u16le
```

```
ERROR [type.binary/F3]: endian type not byte-aligned
   |
4  |  flags: 4
5  |  length: u16be
   |          ^^^^^ starts at bit 4, not byte-aligned

WHY: Unaligned multi-byte reads are complex and slow.

FIX: Pad to byte boundary before multi-byte fields.
```

```
ERROR [type.binary/V1]: invalid bit count
   |
3  |  x: 0
   |     ^ bit count must be >= 1 and <= 64
```

## Edge Cases

| Case | Rule | Behavior |
|------|------|----------|
| Empty struct | B1 | Valid, SIZE = 0 |
| Single field | B1 | Valid |
| > 64 bit field | V1 | Compile error |
| 0 bit field | V1 | Compile error |
| Total > 65535 bits | V3 | Compile error (8KB limit) |
| Non-binary struct in @binary | F4 | Error unless also @binary |

---

## Appendix (non-normative)

### Rationale

**B1 (declaration order):** Erlang's bit syntax is powerful but requires special syntax (`<<>>`, `/` specifiers). Making binary layout a struct property gives reusable layouts, works with existing `match`, and gets IDE support for free — all without new syntax.

**F2 (endian required):** Implicit endianness causes bugs in protocol code. Explicit `be`/`le` on every multi-byte field eliminates an entire class of mistakes.

**F3 (byte alignment):** Unaligned multi-byte reads add complexity and hurt performance. Requiring alignment keeps the generated code simple.

### Patterns & Guidance

**Construction:**

<!-- test: skip -->
```rask
const header = IpHeader {
    version: 4,
    ihl: 5,
    dscp: 0,
    ecn: 0,
    total_length: 60,
    identification: 0x1234,
    flags: 0b010,        // Don't Fragment
    fragment_offset: 0,
    ttl: 64,
    protocol: 6,         // TCP
    checksum: 0,
    src: 0xC0A80001,     // 192.168.0.1
    dst: 0xC0A80002,     // 192.168.0.2
}

const bytes = header.build()  // Vec<u8> of 20 bytes
```

**Nested binary structs:**

<!-- test: skip -->
```rask
@binary
struct MacHeader {
    dst: [6]u8
    src: [6]u8
    ethertype: u16be
}

@binary
struct EthernetFrame {
    mac: MacHeader
    // ... rest of frame
}
```

**Comptime integration:**

<!-- test: skip -->
```rask
const MAGIC_HEADER: [u8; 8] = comptime {
    @binary
    struct Header {
        magic: u32be
        version: u16be
        flags: u16be
    }

    Header {
        magic: 0xCAFEBABE,
        version: 1,
        flags: 0
    }.build().freeze()
}
```

**Performance:** Parse is single-pass with no allocations (bounds check once upfront). Build is direct memory writes. Field access is zero-cost after parse.

**Relationship to other attributes:**

| Attribute | Purpose | Field types |
|-----------|---------|-------------|
| `@binary` | Wire format parsing/building | Bit widths, endian types |
| `@layout(C)` | C ABI compatibility | C-compatible types |
| `@packed` | Remove padding | Any types |

`@binary` is for network/file formats. `@layout(C)` is for C FFI. Don't combine them.

### See Also

- `ctrl.comptime` — Compile-time execution
- `type.structs` — Struct definitions and methods
- `std.bits` — `unpack`, `pack`, and related functions
