<!-- id: std.hex -->
<!-- status: decided -->
<!-- summary: Hexadecimal encoding and decoding of byte sequences -->
<!-- depends: stdlib/strings.md, stdlib/collections.md -->

# Hex

Bytes as hex text. What you reach for after `digest.sha256` or before printing a
wire dump.

## API

<!-- test: skip -->
```rask
hex.encode(data: Vec<u8>, upper: bool = false) -> string
hex.decode(s: string) -> Vec<u8> or HexError
```

<!-- test: skip -->
```rask
let sum = digest.sha256(contents)
println(hex.encode(sum))                  // "e3b0c44298fc1c14..."
println(hex.encode(sum, upper: true))     // "E3B0C44298FC1C14..."

let bytes = try hex.decode("deadbeef")
```

## Core Rules

| Rule | Description |
|------|-------------|
| **H1: Lowercase out, either in** | `encode` produces lowercase unless asked otherwise; `decode` accepts either case and mixed case. Output is one thing so hashes compare as strings; input is whatever arrived |
| **H2: Two characters per byte, always** | An odd-length input is an error, not a leading zero nibble. `"abc"` is a typo, not `0x0abc` |
| **H3: No prefixes, no separators** | `"0x"`, `":"` and spaces are rejected. Hex dump formatting is a display concern, and accepting some separators means arguing about which |

## Errors

<!-- test: parse -->
```rask
enum HexError {
    BadCharacter(byte: u8, at: usize)
    OddLength(usize)
}
```

## Error Messages

```
ERROR [std.hex/H3]: invalid character in hex input

WHY: "0x" at offset 0 is not hex. hex.decode takes bare hex digits.

FIX: Drop the prefix:

  let body = if s.starts_with("0x") { s[2..] } else { s[0..] }
  let bytes = try hex.decode(body.to_string())
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Empty input | — | `encode([])` is `""`; `decode("")` is `[]` |
| Mixed case — `"DeAdBeEf"` | H1 | Accepted |
| Odd length — `"abc"` | H2 | `OddLength` error |
| Non-ASCII input | H3 | `BadCharacter` — hex digits are ASCII by definition |

---

## Appendix (non-normative)

### Rationale

**H1 (lowercase output):** hex digests get compared as strings constantly — against
a file, a header, a database column. One canonical output means that comparison
works without normalizing first. Uppercase stays available because some wire
formats specify it.

**Why not `[]u8 -> string` on the byte slice itself:** a `.to_hex()` method on
`Vec<u8>` would put a formatting concern on the collection type, and then `base64`
wants one too. Encodings are modules; the collection stays a collection.

### See Also

- `std.base64` — the other binary-to-text encoding, same shape
- `std.digest` — produces the byte arrays this usually encodes
- `std.bits` — byte-order conversion and binary parsing, a different concern
