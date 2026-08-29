<!-- id: std.base64 -->
<!-- status: decided -->
<!-- summary: Base64 encoding and decoding, RFC 4648, alphabet and padding as parameters -->
<!-- depends: stdlib/strings.md, stdlib/collections.md -->

# Base64

Bytes to text and back. RFC 4648.

Two functions. The variants people need — URL-safe alphabet, no padding — are
parameters, because they're the same operation with a different table
(`std.api/SD2`).

## API

<!-- test: skip -->
```rask
base64.encode(data: Vec<u8>, url_safe: bool = false, padding: bool = true) -> string
base64.decode(s: string) -> Vec<u8> or Base64Error
```

<!-- test: skip -->
```rask
let token = base64.encode(bytes)                    // "SGVsbG8gd29ybGQ="
let token = base64.encode(bytes, url_safe: true)    // "-" and "_" instead of "+" and "/"
let jwt   = base64.encode(bytes, url_safe: true, padding: false)

let bytes = try base64.decode(token)
```

## Core Rules

| Rule | Description |
|------|-------------|
| **B1: Decode accepts both alphabets** | `decode` takes standard or URL-safe input, with or without padding, without being told which. The alphabets don't overlap, so there's nothing to disambiguate and no reason to make the caller declare it |
| **B2: Decode is strict otherwise** | A character outside both alphabets, or a length that can't be a whole number of bytes, is an error. No skipping, no "best effort" |
| **B3: Whitespace is not stripped** | Newlines in MIME-wrapped base64 are the caller's to remove. Silently ignoring them would also silently accept corrupt input |

## Errors

<!-- test: parse -->
```rask
enum Base64Error {
    BadCharacter(byte: u8, at: usize)
    BadLength(usize)        // not a valid base64 length
    BadPadding              // "=" somewhere other than the end, or too much of it
}
```

## Error Messages

```
ERROR [std.base64/B3]: invalid character in base64 input

WHY: Byte 0x0A (newline) at offset 76 is not in either base64 alphabet.
     MIME-wrapped base64 has newlines every 76 characters.

FIX: Strip them first:

  let clean = wrapped.replace("\n", "")
  let bytes = try base64.decode(clean)
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Empty input | — | `encode([])` is `""`; `decode("")` is `[]` |
| Padding when `padding: false` | B2 | Rejected — the caller asked for none |
| Missing padding on standard input | B1 | Accepted. Length still has to be valid |
| Non-zero bits in the final partial group | B2 | Accepted, bits discarded — rejecting it breaks real encoders |

---

## Appendix (non-normative)

### Rationale

**Why its own module, not `std.encoding`:** `std.encoding` is the `Encode`/`Decode`
trait system — turning a struct into some format. Base64 turns bytes into text.
Filing both under "encoding" would mean the module name says nothing about which
one you get, and `encoding.base64.encode(x)` says "encode" twice to reach an
operation neither module is really about.

A two-function module isn't a problem: `std.api/SD1` is a ceiling on how big a
module gets, not a floor on how big it must be. `base64.encode(data)` is exactly
what a developer would guess and type.

**Why decode accepts both alphabets (B1):** every caller who needs the URL-safe
variant knows it on the encode side and almost never on the decode side — the input
arrived from somewhere else. A `url_safe:` parameter on `decode` would be a
question the caller usually can't answer, guarded by nothing, since the alphabets
are disjoint and a wrong guess just fails.

### See Also

- `std.hex` — the other binary-to-text encoding, same shape
- `std.encoding` — `Encode`/`Decode` traits, a different job that shares a word
- `std.url` — percent-encoding, which is URL syntax rather than a byte encoding
