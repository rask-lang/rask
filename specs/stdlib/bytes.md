<!-- id: std.bytes -->
<!-- status: proposed -->
<!-- summary: Immutable refcounted byte buffer (Copy), the string design without the UTF-8 rule. Vec<u8> is how you build one: freezing is free, thawing is free when nobody else holds it. Inline slicing, BytesView for storable zero-copy pieces -->
<!-- depends: stdlib/strings.md, stdlib/collections.md, memory/value-semantics.md, memory/borrowing.md -->

# Bytes

`bytes` is `string` without the UTF-8 rule. Immutable, refcounted, Copy, and it slices
and views the same way. A parser can keep "the header is bytes 10–20 of this packet"
without copying it and without a reference that could dangle. Rust gets this from the
`bytes` crate and Go from slices; here it's a language primitive, because refcounting
is something the compiler provides and user types don't (`mem.value-semantics/VS3`).

## One buffer, two types

`Vec<u8>` and `bytes` are the same allocation in two states. A `Vec<u8>` has one owner
and can change. `bytes` has many owners and can't. You fill a `Vec<u8>`, then freeze it:

<!-- test: skip -->
```rask
mut buf = Vec<u8>.with_capacity(4096)
try socket.read_into(mutate buf)
let packet = buf.freeze()          // no copy: same allocation, now shared and immutable
```

I looked at making them one type and didn't find a version I'd want. One type that is
both changeable and shared needs copy-on-write: a write checks whether anyone else holds
the buffer and silently copies it if so. Swift's arrays do this. It makes a single
`push` cost O(n) depending on who else holds the buffer, which is invisible at the call
site, and Rask makes big costs visible. Two types with a free conversion keep both costs
in plain sight: changing needs one owner, sharing gives up changing.

| Rule | Description |
|------|-------------|
| **BY1: Immutable, refcounted, Copy** | 16 bytes. Copying bumps an atomic refcount in the buffer's header, or nothing in small mode (BY3). Same semantics as `std.strings/S1` and S6, including refcount elision for a provable sole owner |
| **BY2: Any byte values** | No encoding rule. That's the whole difference from `string` |
| **BY3: Small inline** | 15 bytes or fewer are stored in the value itself, no heap and no refcount. Same layout as `std.strings/S8`, so one representation serves both types |
| **BY4: Inline slicing** | `b[i..j]` is a temporary view, valid for the expression, like `std.strings/S2`. It can be an argument, a receiver or the input to `.view()` or `.to_bytes()`. It can't be stored or returned |
| **BY5: Freezing is free** | `v.freeze()` takes a `Vec<u8>` and returns `bytes` over the same allocation. O(1). A `Vec<u8>`'s buffer reserves room for the refcount header, so nothing moves |
| **BY6: Thawing asks** | `b.thaw()` returns `Vec<u8>?`: the same allocation back if `b` is its only holder, `none` if anyone else shares it. `b.to_vec()` always copies. Getting the buffer back for free when it's yours, and a visible copy when it isn't |

`thaw` returning `none` is the copy-on-write check, moved to where you can see it.
The caller decides what a shared buffer costs: `b.thaw() ?? b.to_vec()` reads as
"reuse it if I can, copy if I must".

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
| `v.freeze()` | `bytes` | None (BY5) |
| `b.thaw()` | `Vec<u8>?` | None, or `none` if shared (BY6) |
| `b.to_vec()` | `Vec<u8>` | Copies |
| `s.as_bytes()` | `BytesView` | None. A string's buffer is a byte buffer (BY3) |
| `b.to_string()` | `string or Utf8Error` | O(n) validation, no copy: the string shares the buffer |

I/O needs nothing new: reads fill a `Vec<u8>` as they do now, and a caller who wants to
share the result freezes it.

## What it costs `Vec`

Every `Vec<u8>` allocation carries 8 bytes for a refcount it uses only once frozen. For a
read buffer that's nothing. For a million tiny `Vec<u8>`s it's noticeable, and the honest
answer is that a million tiny byte buffers want to be `bytes` from the start, which
inline anything 15 bytes or under (BY3). Only `Vec<u8>` needs the header; other element
types don't freeze.

## Open questions

- **Freezing other element types.** `Vec<u32>` to a shared `[u32]` buffer works the same
  way. Is that a second type, or does `bytes` stay special because bytes are what
  crosses wires?
- **`@binary` input.** `@binary` parsing reads from a byte buffer. Reading from `bytes`
  or a `BytesView` gives it zero-copy input. Whether it keeps a `Vec<u8>` form is
  `std.bits`' call.
- **Literal syntax** (`b"\x01\x02"`) is deferred. `Vec.from([1, 2]).freeze()` works
  without it.
