<!-- id: std.digest -->
<!-- status: decided -->
<!-- summary: SHA-256, SHA-1, MD5 and CRC32 digests — one-shot functions plus incremental builders -->
<!-- depends: stdlib/collections.md, stdlib/hex.md -->

# Digest

Content hashing: integrity checks, content addressing, cache keys, checksums.

Named `digest`, not `hash`, because `hash` already means something in Rask —
`Hashable.hash() -> u64`, the thing `Map` calls. Those are different jobs with
different guarantees, and a beginner conflating them writes a security bug. The
module name keeps them apart: `digest.sha256(bytes)` reads as the noun it is.

## One-shot

<!-- test: skip -->
```rask
digest.sha256(data: Vec<u8>) -> [32]u8
digest.sha1(data: Vec<u8>) -> [20]u8
digest.md5(data: Vec<u8>) -> [16]u8
digest.crc32(data: Vec<u8>) -> u32
```

<!-- test: skip -->
```rask
let sum = digest.sha256(contents)
println(hex.encode(sum))         // "e3b0c44298fc1c149afb..."
```

Digests are byte arrays. Rendering is `hex.encode` or `base64.encode` — there's no
`sha256_hex` sibling, because the two modules compose (`std.api/SD2`).

## Incremental

For data you don't have all at once — a file you're streaming, a socket:

<!-- test: skip -->
```rask
mut d = digest.Sha256.new()
for chunk in file.chunks(64 * 1024) {
    d.update(try chunk)
}
let sum = d.build()
```

`Sha256`, `Sha1`, `Md5` and `Crc32` all have `new()`, `update(chunk)`, `build()`.
`build()` because that's the terminating verb for every builder in the stdlib
(`spec.canonical-patterns`) — `StringBuilder`, `JsonWriter`, this.

## Core Rules

| Rule | Description |
|------|-------------|
| **D1: Fixed-size output** | Digests are `[N]u8` arrays, not `Vec<u8>` — the size is known and the value is `Copy`. Compare them with `==` |
| **D2: One-shot and incremental agree** | `digest.sha256(data)` and a `Sha256` fed the same bytes in any chunking give the same result. The one-shot is the builder with the loop written for you |
| **D3: Not a MAC** | These are unkeyed. Authenticating a message needs HMAC, and appending a secret to a digest is the classic length-extension mistake. HMAC and signatures are a `crypto` package |
| **D4: MD5 and SHA-1 are for reading old formats** | They exist because Git objects, legacy checksums and existing file formats use them. They are broken for anything adversarial and the docs say so at the function |

## Choosing one

| Function | Output | Use it for |
|----------|--------|-----------|
| `sha256` | 32 bytes | The default. Content addressing, integrity, cache keys |
| `crc32` | 4 bytes | Fast corruption detection where an attacker isn't in the picture — ZIP, PNG, wire framing |
| `sha1` | 20 bytes | Git compatibility. Collidable in practice since 2017 |
| `md5` | 16 bytes | Reading formats that specified it. Collidable since 2004 |

## Error Messages

```
WARNING [std.digest/D4]: md5 is not collision-resistant
   |
12 |  let fingerprint = digest.md5(upload)
   |                           ^^^ using MD5 to identify content

WHY: MD5 collisions are cheap to construct. Two different uploads can
     be made to share a fingerprint.

FIX: Use sha256 unless an existing format requires MD5:

  let fingerprint = digest.sha256(upload)
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Empty input | D2 | The well-known empty digest, not an error |
| `update` with an empty chunk | D2 | No-op |
| `build()` called twice | — | `build()` consumes the builder (`take self`), so it can't be |
| Digest of a `string` | D1 | Pass `s.as_bytes()`. Hashing text means hashing an encoding, and being explicit about which is the point |
| Comparing digests for equality | D1 | `==` on the arrays. Constant-time comparison is a `crypto` concern |

---

## Appendix (non-normative)

### Rationale

**Why `digest` and not `hash`:** `Hashable.hash()` produces a `u64` for bucketing
in a `Map`. It is allowed to be fast, weak, and randomly seeded per process. A
`digest` is stable across runs and machines and is what you compare to decide two
files are the same. Both are "hashing" in English, which is exactly the problem —
one module named `hash` containing `hash.sha256` next to a `Hashable.hash` trait
invites using one where the other belongs.

**Why one-shot functions and builders both (D2):** these are the fallible-pair
situation inverted — not two spellings of one call, but one call and a decomposition
of it. Most uses have all the bytes and want a line; streaming a 4 GB file can't
have them. Making everyone open a builder for a one-line checksum is the ceremony
`std.api/SD1` exists to prevent, and making streaming impossible fails the
data-processing coverage goal.

**Why these four:** SHA-256 is the modern default and what content addressing wants.
CRC32 is the fast non-cryptographic checksum every container format uses. SHA-1 and
MD5 are legacy-format readers. SHA-512 and the SHA-3 family are absent — nothing in
the stdlib's own surface needs them, and adding a family member per NIST publication
is how a module stops fitting on a screen.

**Why not the full `crypto` surface:** AES, RSA, ECDSA and HMAC need expert
maintenance and constant-time implementations, and getting them subtly wrong is
worse than not shipping them (`std.stdlib` README, out of scope).

### See Also

- `std.hex` / `std.base64` — rendering a digest as text
- `type.traits` — `Hashable`, the other kind of hashing
- `std.fs` — streaming a file into an incremental digest
