<!-- id: std.bytes -->
<!-- status: decided -->
<!-- summary: Immutable refcounted byte buffer (Copy), the string design without the UTF-8 rule. Same 16-byte value and runtime as string. Vec<u8> builds one: freezing is free, to_vec reuses the buffer when nobody else holds it. x"…" literals, BytesSlice and BytesView like string's slices and views -->
<!-- depends: stdlib/strings.md, stdlib/collections.md, memory/value-semantics.md, types/temporary.md, types/reads-as.md -->

# Bytes

`bytes` is `string` without the UTF-8 rule. Immutable, refcounted, Copy, 16 bytes, and
it slices and views the same way. A parser can keep "the header is bytes 10–20 of this
packet" without copying it and without a reference that could dangle.

It's the same value as a string underneath: the same 16-byte layout, the same
small-inline mode, the same refcounted heap buffer, the same runtime functions for
length, comparison, hashing, search and slicing. A string is bytes plus a promise that
they're UTF-8, and the operations that need that promise (characters, case,
graphemes) are the only ones `string` adds. Refcounting is something the compiler
provides and user types don't (`mem.value-semantics/VS3`), so `bytes` is a language
primitive like `string`.

| Rule | Description |
|------|-------------|
| **BY1: Immutable, refcounted, Copy** | 16 bytes. Copying bumps an atomic refcount in the buffer's header, or nothing in small mode. Same semantics as `std.strings/S1` and S6, including refcount elision for a provable sole owner |
| **BY2: Any byte values** | No encoding rule. That's the whole difference from `string` |
| **BY3: One representation** | The value and the buffer are a string's (`std.strings/S8`): 15 bytes or fewer inline, more on the heap behind a refcounted header. `s.as_bytes()` is therefore free, and so is the string `string.from_utf8(b)` makes: it validates and shares the buffer |
| **BY4: Indexing** | `b[i]` is the `u8` at `i`. Out of range panics, as for `Vec` |
| **BY5: Bytewise comparison** | `==`, order and hashing compare the bytes. `bytes` can key a `Map` |
| **BY6: Not text** | `bytes` isn't Displayable: `"{b}"` is a compile error whose fix is `{b:debug}`, which prints the value as a hex literal (BY10), so debug output pastes back as source |

## One buffer, two types

`Vec<u8>` and `bytes` are the same allocation in two states. A `Vec<u8>` has one owner
and can change. `bytes` has many owners and can't. You fill a `Vec<u8>`, then freeze it:

<!-- test: skip -->
```rask
mut buf = Vec<u8>.with_capacity(4096)
try socket.read_into(mutate buf)
let packet = buf.freeze()          // no copy: same allocation, now shared and immutable
```

One type that is both changeable and shared would need copy-on-write: a write checks
whether anyone else holds the buffer and silently copies it if so. That makes a single
`push` cost O(n) depending on who else holds the buffer, which is invisible at the call
site. Two types with a free conversion keep both costs in plain sight: changing needs one
owner, sharing gives up changing.

| Rule | Description |
|------|-------------|
| **BY7: Freezing is free** | `v.freeze()` takes a `Vec<u8>` and returns `bytes` over the same allocation. O(1). A `Vec<u8>`'s buffer reserves room for the refcount header and a terminating byte, so nothing moves |
| **BY8: to_vec reuses what nobody shares** | `b.to_vec()` returns a `Vec<u8>` with the same bytes. When `b` is the buffer's only holder and isn't used again, the vector takes the allocation over; otherwise it copies. The contents are the same either way, only the cost differs |

The reuse isn't something you can observe, so it isn't a copy-on-write trap: no
later write pays for it, and the copy, when it happens, happens at the call you
wrote. An implementation that always copies is correct (the interpreter does).

`freeze` isn't `to_bytes` because it isn't one: `to_*` leaves the source alone and
allocates, `freeze` consumes the vector and allocates nothing.

## Slices and views

`b[i..j]` is a `BytesSlice` and `.view()` keeps one as a `BytesView`, built exactly as
`StringSlice` and `StringView` are: ordinary stdlib structs over a `bytes` that shares
the source's buffer.

<!-- test: skip -->
```rask
@temporary(keep: [view, to_bytes])
@reads_as(data)
public struct BytesSlice { data: bytes  start: usize  end: usize }

@reads_as(data)
public struct BytesView { data: bytes }
```

| Rule | Description |
|------|-------------|
| **BY9: Slices and views are strings' rules** | A slice lives for its expression (`type.temporary`); every read-only `bytes` method works on a slice or a view (`type.reads-as`); `.view()` keeps the range and pins the whole source buffer, `.to_bytes()` copies the range out and pins nothing (`std.strings/V2`'s reasoning: a 40-byte header mustn't silently hold a 64 KB packet). A slice of a slice is placed in the original |

<!-- test: skip -->
```rask
struct Frame {
    kind: u8
    payload: BytesView          // shares the packet's buffer
}

func parse_frame(packet: bytes) -> Frame or FrameError {
    if packet.len() < 3 { return FrameError.Short }
    mut r = BinaryReader.new(packet)
    let kind = try r.read_u8()
    let len = try r.read_u16be()
    let payload = try r.read_bytes(len as usize)
    return Frame(kind: kind, payload: payload)
}
```

One allocation for the packet and none per frame. Binary reading is `std.bits`'
`BinaryReader`, which reads from `bytes`; `bytes` itself has no `read_*` methods.

## Literals

| Rule | Description |
|------|-------------|
| **BY10: Hex literal** | `x"89 50 4E 47"` is a `bytes` constant: pairs of hex digits, either case, with spaces or newlines allowed between pairs. An odd digit count or any other character is a compile error at that character. `x""` is empty. It's compiled into the program like a string literal and allocates nothing at run time |

<!-- test: skip -->
```rask
const PNG_MAGIC: bytes = x"89 50 4E 47 0D 0A 1A 0A"
const GET: bytes = "GET ".as_bytes()
```

That's the only `bytes` literal. Text-shaped bytes come from a string, and a list
literal doesn't turn into `bytes`.

## Crossing to and from other types

| Operation | Returns | Cost |
|-----------|---------|------|
| `v.freeze()` | `bytes` | None (BY7) |
| `b.to_vec()` | `Vec<u8>` | None if nothing else holds it, else copies (BY8) |
| `s.as_bytes()` | `bytes` | None (BY3) |
| `string.from_utf8(b)` | `string or Utf8Error` | O(n) validation, no copy (BY3) |

I/O needs nothing new: reads fill a `Vec<u8>` as they do now, and a caller who wants to
share the result freezes it.

## What it costs `Vec`

Every `Vec` of one-byte elements carries a 16-byte header and one spare byte that it
uses only once frozen. For a read buffer that's nothing. A million tiny byte buffers
want to be `bytes` from the start, which keeps anything 15 bytes or under inline.

<!-- surface -->
```rask
extend bytes {
    func len(self) -> usize
    func is_empty(self) -> bool
    func starts_with(self, prefix: bytes) -> bool
    func ends_with(self, suffix: bytes) -> bool
    func contains(self, needle: bytes) -> bool
    func index_of(self, needle: bytes) -> usize?
    func hash(self) -> u64
    func to_vec(take self) -> Vec<u8>
    func view(self) -> BytesView
}

extend Vec<u8> {
    func freeze(take self) -> bytes
}

struct BytesSlice { }
extend BytesSlice {
    func view(self) -> BytesView
    func to_bytes(self) -> bytes
}

struct BytesView { }
extend BytesView {
    func view(self) -> BytesView
    func to_bytes(self) -> bytes
}
```
