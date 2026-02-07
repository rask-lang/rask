# Binary Structs

## The Question

How do we parse and build binary data (network protocols, file formats, embedded systems) with Erlang's power but without new syntax?

## Decision

Use `@binary` attribute on structs. Field "types" specify bit layout. Numbers mean bit widths. Compiler generates `.parse()` and `.build()` methods.

## Rationale

Erlang's bit syntax is powerful but requires special syntax (`<<>>`, `/` specifiers). I make binary layout a struct property:
- No new syntax
- Layouts are reusable
- Works with existing `match`
- IDE support comes free

## Specification

### The `@binary` Attribute

**Note:** `@binary` structs use declaration order for wire format. No reordering. Field order is the bit layout.

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

### Field Specifiers

| Specifier | Bits | Runtime Type | Description |
|-----------|------|--------------|-------------|
| `N` | N | smallest fitting uint | Bare number = N bits |
| `u8` | 8 | u8 | Unsigned byte |
| `i8` | 8 | i8 | Signed byte |
| `u16be` | 16 | u16 | Big-endian unsigned |
| `u16le` | 16 | u16 | Little-endian unsigned |
| `i16be` | 16 | i16 | Big-endian signed |
| `i16le` | 16 | i16 | Little-endian signed |
| `u32be` | 32 | u32 | Big-endian unsigned |
| `u32le` | 32 | u32 | Little-endian unsigned |
| `i32be` | 32 | i32 | Big-endian signed |
| `i32le` | 32 | i32 | Little-endian signed |
| `u64be` | 64 | u64 | Big-endian unsigned |
| `u64le` | 64 | u64 | Little-endian unsigned |
| `i64be` | 64 | i64 | Big-endian signed |
| `i64le` | 64 | i64 | Little-endian signed |
| `f32be` | 32 | f32 | Big-endian float |
| `f32le` | 32 | f32 | Little-endian float |
| `f64be` | 64 | f64 | Big-endian double |
| `f64le` | 64 | f64 | Little-endian double |
| `[N]u8` | N×8 | [u8; N] | Fixed byte array |

### Runtime Types

Bare number `N` becomes the smallest unsigned integer that fits:

| Bits | Runtime Type |
|------|--------------|
| 1-8 | u8 |
| 9-16 | u16 |
| 17-32 | u32 |
| 33-64 | u64 |

### Generated Methods

For any `@binary struct T`:

```rask
extend T {
    /// Parse from byte slice, return (value, remaining_bytes)
    func parse(data: []u8) -> (T, []u8) or ParseError

    /// Build into new byte vector
    func build(self) -> Vec<u8>

    /// Build into existing buffer, return bytes written
    func build_into(self, buffer: []u8) -> usize or BuildError

    /// Total size in bytes (rounded up)
    const SIZE: usize

    /// Total size in bits
    const SIZE_BITS: usize
}
```

### Packing Rules

1. Fields packed sequentially, no padding
2. Bits packed MSB-first (network byte order)
3. Total size = sum of field bits, rounded up
4. Unused bits in final byte are zero on build, ignored on parse

### Example: IP Header

```rask
@binary
struct IpHeader {
    version: 4
    ihl: 4
    dscp: 6
    ecn: 2
    total_length: u16be
    identification: u16be
    flags: 3
    fragment_offset: 13
    ttl: u8
    protocol: u8
    checksum: u16be
    src: u32be
    dst: u32be
}

// SIZE = 20 bytes (160 bits)

func handle_packet(data: []u8) -> () or Error {
    let (header, payload) = try IpHeader.parse(data)

    match header.version {
        4: handle_ipv4(header, payload)
        6: handle_ipv6(header, payload)
        _: Err(UnsupportedVersion)
    }
}
```

### Example: TCP Header

```rask
@binary
struct TcpHeader {
    src_port: u16be
    dst_port: u16be
    seq: u32be
    ack: u32be
    data_offset: 4       // Header length in 32-bit words
    reserved: 3
    flags: 9             // NS, CWR, ECE, URG, ACK, PSH, RST, SYN, FIN
    window: u16be
    checksum: u16be
    urgent_ptr: u16be
}

// SIZE = 20 bytes (160 bits)

// Flag constants
const TCP_FIN: u16 = 0x001
const TCP_SYN: u16 = 0x002
const TCP_RST: u16 = 0x004
const TCP_PSH: u16 = 0x008
const TCP_ACK: u16 = 0x010
const TCP_URG: u16 = 0x020

func is_syn(header: TcpHeader) -> bool {
    header.flags & TCP_SYN != 0
}
```

### Construction

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

### Compile-Time Validation

The compiler validates:

1. **Bit count**: All fields must have valid bit counts (1-64 for integers)
2. **Alignment**: Endian types must be byte-aligned within the struct
3. **Total size**: Must not exceed reasonable limits
4. **Type compatibility**: Field values must fit in declared bit width

```rask
@binary
struct Invalid {
    x: 0        // Error: bit count must be >= 1
    y: 65       // Error: bit count must be <= 64
    z: u16be    // Error: u16be at bit offset 65, not byte-aligned
}
```

### Alignment Requirement

Multi-byte endian types (`u16be`, `u32le`, etc.) must start at byte boundaries:

```rask
@binary
struct Valid {
    flags: 8        // 8 bits = 1 byte
    length: u16be   // Starts at byte 1, OK
}

@binary
struct Invalid {
    flags: 4        // 4 bits
    length: u16be   // Error: starts at bit 4, not byte-aligned
}
```

**Rationale:** Unaligned multi-byte reads are complex and slow.

### Endianness Default

No default. Multi-byte fields must specify `be` or `le`:

```rask
@binary
struct Explicit {
    port: u16be     // OK: big-endian
    addr: u32le     // OK: little-endian
}

@binary
struct Ambiguous {
    port: u16       // Error: must specify u16be or u16le
}
```

**Rationale:** Implicit endianness causes bugs. Explicit is better.

### Nested Binary Structs

Binary structs can contain other binary structs:

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

The nested struct is inlined (no indirection).

## Inline Parsing

For one-off parsing without defining a struct, use `unpack`:

```rask
let (magic, version, length, rest) = try data.unpack(u32be, u8, u16be)
```

See [stdlib/bits.md](../stdlib/bits.md) for `unpack`, `pack`, and related functions.

## Edge Cases

| Case | Behavior |
|------|----------|
| Empty struct | Valid, SIZE = 0 |
| Single field | Valid |
| > 64 bit field | Error |
| 0 bit field | Error |
| Total > 65535 bits | Error (8KB limit) |
| Non-binary struct in @binary | Inline if also @binary, error otherwise |

## Integration

- **Match**: Works with normal pattern matching via `.parse()`
- **Error handling**: Parse returns `Result`, integrates with `try`
- **Comptime**: `SIZE` and `SIZE_BITS` are comptime constants
- **Generics**: Binary structs can be generic (rare)

### Comptime Integration

Binary structs work at compile time for building constant binary data:

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

**Comptime capabilities:**
- `T.SIZE` and `T.SIZE_BITS` available
- `.build()` works (returns `Vec<u8>`, must `.freeze()`)
- `.parse()` validates embedded data
- Pattern matching works

See [comptime.md](../control/comptime.md).

### Relationship to Other Attributes

| Attribute | Purpose | Field types |
|-----------|---------|-------------|
| `@binary` | Wire format parsing/building | Bit widths, endian types |
| `@layout(C)` | C ABI compatibility | C-compatible types |
| `@packed` | Remove padding | Any types |

`@binary` is for **network/file formats**. `@layout(C)` is for **C FFI**. They serve different purposes and should not be combined.

## Performance

- **Parse**: Single pass, no allocations, bounds check once upfront
- **Build**: Direct memory writes, no intermediate buffers
- **Field access**: Zero-cost after parse (fields stored as runtime types)

## Comparison with Erlang

| Erlang | Rask |
|--------|------|
| `<<Ver:4, IHL:4, ...>>` | `@binary struct { version: 4, ihl: 4, ... }` |
| Inline patterns | Named, reusable structs |
| Special syntax | Standard attribute |
| Runtime matching | Compile-time layout |
