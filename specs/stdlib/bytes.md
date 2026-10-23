<!-- id: std.bytes -->
<!-- status: proposed -->
<!-- summary: Immutable refcounted byte buffer (Copy), the same design as string without the UTF-8 rule. Inline slicing, BytesView for storable zero-copy pieces, BytesBuilder for construction -->
<!-- depends: stdlib/strings.md, memory/value-semantics.md, memory/borrowing.md -->

# Bytes

`bytes` is `string` without the UTF-8 rule. Immutable, refcounted, Copy, and it slices
and views the same way. A parser can keep "the header is bytes 10–20 of this packet"
without copying it and without a reference that could dangle. Rust gets this from the
`bytes` crate and Go from slices; here it's a language primitive, because refcounting
is something the compiler provides and user types don't (`mem.value-semantics/VS3`).

It doesn't replace `Vec<u8>`. A `Vec<u8>` is a buffer you change. `bytes` is one you've
finished changing and want to pass around and cut up for free.

## The type

| Rule | Description |
|------|-------------|
| **BY1: Immutable, refcounted, Copy** | 16 bytes. Copying bumps an atomic refcount in the heap header, or nothing in small mode (BY3). Same semantics as `std.strings/S1` and S6, including refcount elision for a provable sole owner |
| **BY2: Any byte values** | No encoding rule. That's the whole difference from `string` |
| **BY3: Small inline** | 15 bytes or fewer are stored in the value itself, no heap and no refcount. Same layout as `std.strings/S8`, so one runtime representation serves both types |
| **BY4: Inline slicing** | `b[i..j]` is a temporary view, valid for the expression, like `std.strings/S2`. It can be an argument, a receiver or the input to `.view()` or `.to_bytes()`. It can't be stored or returned |
| **BY5: Builder for construction** | `BytesBuilder` collects pushes and `.build()`s a `bytes` without copying. Like `std.strings/S7` |

## Keeping a piece: `BytesView`

| Rule | Description |
|------|-------------|
| **BV1: Refcounted view** | `b[i..j].view()` shares the source buffer and holds a refcount on it. 16 bytes, Copy. Never dangles |
| **BV2: The pin is in the type** | A view keeps the whole source buffer alive. A field typed `BytesView` says so; a `bytes` field says the data is independent. `.to_bytes()` copies the range out and drops the pin. Same reasoning as `std.strings/V2`: a 40-byte header mustn't silently hold a 64 KB packet |
| **BV3: Read-only API** | A view has the read-only `bytes` API: length, indexing, slicing, search, comparison, the `@binary` readers |
| **BV4: Views never chain** | Re-slicing a view refers to the original buffer, as `std.strings/V6` |

<!-- test: skip -->
```rask
struct Frame {
    kind: u8
    payload: BytesView          // shares the packet's buffer
}

func parse_frame(packet: bytes) -> Frame or FrameError {
    if packet.len() < 3 { return FrameError.Short }
    let len = packet.read_u16_be(1) as usize
    if packet.len() < 3 + len { return FrameError.Short }
    return Frame(kind: packet[0], payload: packet[3..3 + len].view())
}
```

One allocation for the packet and none per frame.

## Crossing to and from other types

| Operation | Returns | Cost |
|-----------|---------|------|
| `s.as_bytes()` | `BytesView` | None. A string's buffer is already a byte buffer (BY3) |
| `b.to_string()` | `string or Utf8Error` | O(n) validation, no copy: the string shares the buffer |
| `bytes.from(v)` | `bytes` | Copies. A `Vec`'s buffer has no refcount header to share |
| `b.to_vec()` | `Vec<u8>` | Copies, so you can change it |
| `BytesBuilder.new()` … `.build()` | `bytes` | No copy at `build` |

The two copies are the ones where ownership really changes shape: a `Vec` you can
change becomes a buffer many can share, and back.

## Why one design for two types

I could have made `bytes` its own thing with its own rules. But someone who has learned
how strings slice and view already knows this type, and that's worth more than anything a
different design would buy. It also means one runtime representation, so `as_bytes` and
`to_string` share the buffer and don't copy.

## Open questions

- **What I/O returns.** `File.read_bytes()` and socket reads return `Vec<u8>` today.
  Returning `bytes` is where this pays off for network code. But a read buffer you then
  modify wants to be a `Vec`. Probably both, with the read that fills a `BytesBuilder` as
  the default.
- **`@binary` input.** `@binary` parsing reads from a byte buffer. Reading from `bytes`
  or a `BytesView` gives it zero-copy input. Whether it keeps a `Vec<u8>` form too is
  `std.bits`' call.
- **Literals.** `b"\x01\x02"` or `bytes.from([1, 2])`? The first is new syntax for a
  common case. The second needs nothing new.
