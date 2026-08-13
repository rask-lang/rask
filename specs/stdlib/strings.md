<!-- id: std.strings -->
<!-- status: decided -->
<!-- summary: Immutable refcounted string (Copy), inline slicing, StringView for storable zero-copy substrings, StringBuilder for construction -->
<!-- depends: memory/borrowing.md, memory/value-semantics.md -->

# String Handling

Immutable refcounted `string` type with UTF-8 validation, inline slicing for zero-copy expression access, `StringView` for storable zero-copy substrings, `StringBuilder` for construction. `Span` (a core type, not string-specific) is used for byte-index ranges.

## Type Categories

| Rule | Description |
|------|-------------|
| **S1: Immutable, refcounted, Copy** | `string` is UTF-8, immutable, 16 bytes (tagged union — see S8). Under VS1 threshold → implicit Copy |
| **S2: Inline slicing** | `s[i..j]` creates a temporary view valid only within the expression |
| **S3: Public APIs use string** | Prefer `string` parameters in public APIs — callers shouldn't need to know your storage strategy. `StringView` is a storage and return type for parsing layers; `Span` is fine anywhere — it's a general-purpose range type |
| **S4: UTF-8 required** | Strings must contain valid UTF-8. Validated at construction |
| **S5: Byte indices** | Slicing uses byte indices. Mid-codepoint slice panics at runtime |
| **S6: Refcount semantics** | Atomic refcount in heap header. SSO strings (S8) bypass refcounting entirely. Literals ≤ 15 bytes use SSO; longer literals use sentinel refcount (never freed/decremented). Compiler elides atomic ops for provably sole-owner heap strings (see `comp.string-refcount-elision`). This is a language primitive — not available to user-defined types |
| **S7: Builder for mutation** | `push`, `push_char` live on `StringBuilder` only. `string` has no mutation methods |
| **S8: Small string optimization** | Strings ≤ 15 bytes are stored inline in the 16-byte value (no heap allocation, no refcount). Longer strings use heap mode with refcounted header. Layout is a tagged union — discriminant is the MSB of the last byte. User-facing semantics are identical in both modes |
| **S9: Storable views** | `StringView` is the storable form of a slice — zero-copy, shares the source's buffer, holds a refcount on it. 16 bytes, Copy. See V1–V6 |

### Internal Layout (S1 + S8)

16 bytes, tagged union. The MSB of the last byte discriminates between modes:

```
Heap mode (last byte MSB = 0):
  [header_ptr: *u8 (8B)][len: usize (8B)]
  Header at ptr: { refcount: atomic_u32, capacity: u32, data: [u8] }

SSO mode (last byte MSB = 1):
  [inline_data: [u8; 15]][len_tag: u8]
  Length = len_tag & 0x7F (range 0..15)
```

SSO strings are pure value copies — no heap, no refcount. Heap strings share backing storage via atomic refcount. Both modes are 16 bytes, both are Copy. The mode is invisible to user code.

| String variant | Refcount | Allocation | Copy cost |
|----------------|----------|------------|-----------|
| SSO (≤ 15 bytes) | None | None | 16-byte memcpy |
| Literal (> 15 bytes) | Sentinel (never freed) | Static | 16-byte memcpy |
| Literal (≤ 15 bytes) | None (SSO) | None | 16-byte memcpy |
| Heap (shared) | Atomic inc/dec | Heap | 16-byte memcpy + atomic inc |
| Heap (unique, elided) | Skipped (RE1/RE2) | Heap | 16-byte memcpy |

| Type | Description | Ownership | Storable? |
|------|-------------|-----------|-----------|
| `string` | UTF-8 immutable, refcounted | Copy (16 bytes) | Yes |
| `StringView` | Zero-copy substring, shares source buffer | Copy (16 bytes) | Yes |
| `Span` | Plain indices into a string | Copy (2 words) | Yes |
| `StringBuilder` | Growable mutable buffer | Move on assignment | Yes |
| `cstring` | Null-terminated for C FFI | Move on assignment | Yes (unsafe only) |

## Ownership Rules

| Rule | Description |
|------|-------------|
| **O1: Copy on assign** | `let s2 = s1` copies 16 bytes. For heap strings, atomic refcount increment. For SSO strings, plain memcpy (no refcount). Both remain valid |
| **O2: Borrow inferred** | `func foo(s: string)` borrows for call duration. No refcount change |
| **O3: Explicit take** | `func foo(take s: string)` transfers ownership, decrements caller's count |

> `string` is Copy. No `.clone()` needed — assignment copies 16 bytes. For SSO strings (≤ 15 bytes), that's it — no heap, no refcount. For heap strings, the refcount is bumped atomically. This is one of the few types that owns heap memory but is still Copy, because the immutable + refcounted design makes sharing safe.

## Inline Slicing

`s[i..j]` creates a temporary view valid only within the expression (S2). Cannot be assigned, stored, or returned. `.to_string()` copies the slice bytes into a new independent refcounted string — no shared backing with the source.

Slicing follows the same inline access rules as Vec and other growable sources under `mem.borrowing/B2`.

| Context | Example | Valid? |
|---------|---------|--------|
| Function argument | `process(s[0..5])` | Yes |
| Method receiver | `s[0..5].len()` | Yes |
| Chained expression | `s[0..5].to_uppercase()` | Yes |
| Storable conversion | `s[0..5].view()`, `s[0..5].to_string()` | Yes |
| Variable assignment | `let x = s[0..5]` | Compile error |
| Struct field | `Foo { field: s[0..5] }` | Compile error |
| Return value | `return s[0..5]` | Compile error |

To keep a substring past the expression, pick the conversion whose cost you want: `.view()` is zero-copy but keeps the whole source buffer alive (V2); `.to_string()` copies the bytes and is independent of the source.

> **Why does `.to_string()` copy instead of sharing?** A 50-byte substring must not *silently* retain a 10MB source buffer. `.to_string()` copies bytes into a fresh allocation with its own refcount — cost bounded by the slice size, not the source size. Sharing exists, but it's opt-in and named: `.view()` returns a `StringView`, and the pinned buffer is visible in the type (V2).

## The `StringView` Type

A `StringView` is the storable form of a string slice: it references a byte range of a source string's buffer and holds a refcount on that buffer, so the view can never dangle. Zero-copy, 16 bytes, Copy. This is what parsers store in tokens, deserializers store in fields, and split results collect into — without allocating per substring.

| Rule | Description |
|------|-------------|
| **V1: Refcounted view** | A view shares the source string's heap buffer and holds a refcount on it. The buffer stays alive as long as any view does. 16 bytes, Copy — same copy semantics as `string` (memcpy + atomic increment, elidable per `comp.string-refcount-elision`) |
| **V2: Pin is type-visible** | A view keeps the *whole* source buffer alive, not just its range. That cost lives in the type: a `StringView` field says "shares a buffer", a `string` field says "independent". `.to_string()` copies out and releases the pin |
| **V3: Read-only string API** | Views support the full read-only string API — length, search, iteration, slicing, trimming, parsing, comparison, interpolation. No mutation, same as `string`. `==` between views and between a view and a string compares bytes |
| **V4: Small views inline** | Views ≤ 15 bytes store their bytes inline (same SSO layout as S8) — no refcount, no pin. Invisible to user code |
| **V5: Creation via `.view()`** | `.view()` on a string views the whole string; on an expression-scoped slice (`s[i..j]`, `.trim()`, split/lines items) it captures that slice's range. No copy either way |
| **V6: Views never chain** | Slicing a view and calling `.view()` re-references the original buffer directly. There is one header pointer, no matter how many times you re-slice |

| Operation | Return | Notes |
|-----------|--------|-------|
| `s.view()` | `StringView` | View of the whole string. Refcount bump (heap mode) |
| `s[i..j].view()` | `StringView` | Storable view of the slice. Works on any expression-scoped slice: `s.trim().view()`, split items |
| `v[i..j]` | expression-scoped slice | Same inline slicing rules as string (S2, S5) |
| `v.to_string()` | `string` | Copy bytes into an independent string — releases the pin |
| `v.len()`, `v.chars()`, `v.index_of(pat)`, ... | — | Full read-only string API (V3) |

<!-- test: parse -->
```rask
struct Header {
    name: StringView
    value: StringView
}

func parse_header(line: string) -> Header? {
    let colon = try line.index_of(":")
    return Header {
        name: line[0..colon].trim().view(),
        value: line[colon+1..].trim().view(),
    }
}
```

No allocation per header — both fields share `line`'s buffer. The pre-view version of this pattern called `.to_string()` twice per line.

### Internal Layout (V1 + V4)

16 bytes, tagged union — same discriminant trick as `string` (S8):

```
Heap-view mode (last byte MSB = 0):
  [header_ptr: *u8 (8B)][start: u32 (4B)][len: u32 (4B)]
  header_ptr → the source string's heap header (shared refcount)

SSO mode (last byte MSB = 1):
  [inline_data: [u8; 15]][len_tag: u8]     — identical to string's SSO mode
```

The `start`/`len` fields cap views at 4 GiB source offset and 2 GiB view length (the tag bit lives in `len`'s top byte). `.view()` panics beyond those — sources that size are `mmap` territory; use `.to_string()` for the range instead. Views of string literals reference sentinel-refcount storage (S6): no atomic ops, nothing pinned.

`StringView` is a language primitive like `string` — user types can't opt into refcounted sharing (see "Why Only String?" and `mem.boxes/BX2`).

## The `Span` Type

Plain indices for lightweight stored references — offsets for diagnostics, serialization, or when you don't want the source buffer pinned. No validation — user ensures source string validity (like storing a Vec index). 16 bytes, copy-eligible.

| Operation | Return | Notes |
|-----------|--------|-------|
| `Span(i, j)` | `Span` | Create span (just start, end indices) |
| `source[span]` | expression-scoped slice | Panics if out of bounds |
| `source.get(span)` | `(expression-scoped slice)?` | Safe bounds check |
| `span.to_string(source)` | `string` | Allocates copy (panics if OOB) |
| `span.start`, `span.end` | `usize` | Read indices |
| `span.len()` | `usize` | `end - start` |

Prefer `StringView` when you want the text itself; prefer `Span` when you want positions (error messages pointing into source, byte offsets in a wire format) or when pinning the source buffer is unacceptable.

## UTF-8 Validation

| Operation | Return Type | Validation Cost |
|-----------|-------------|-----------------|
| `"literal"` | `string` | Compile-time |
| `string.from_utf8(bytes)` | `string or Utf8Error` | Runtime O(n), one-time |
| `string.from_utf8_unchecked(bytes)` | `string` | None (unsafe block only) |

## Iteration

Iterators borrow for expression scope only. Cannot be stored.

| Method | Yields | Notes |
|--------|--------|-------|
| `s.chars()` | `char` (u32 Unicode scalar) | Expression-scoped iterator |
| `s.bytes()` | `u8` | Raw byte iterator |
| `s.char_indices()` | `(usize, char)` | Index + char pairs |
| `s.lines()` | Expression-scoped slices | Split on newlines |
| `s.split(pat)` | Expression-scoped slices | Split on pattern |
| `s.split_whitespace()` | Expression-scoped slices | Split on Unicode whitespace, skip empty |

Slice-yielding iterators are zero-cost per item. To keep items, convert each: `.view()` (zero-copy, pins source — V2) or `.to_string()` (copies).

## Length and Properties

| Operation | Return | Cost |
|-----------|--------|------|
| `s.len()` | `usize` | O(1), byte length |
| `s.char_count()` | `usize` | O(n), count Unicode scalars |
| `s.is_empty()` | `bool` | O(1) |
| `s.is_ascii()` | `bool` | O(n) first call, cached |

## Construction

| Operation | Return Type | Notes |
|-----------|-------------|-------|
| `"literal"` | `string` | Compile-time validated. ≤ 15 bytes → SSO (inline, no allocation). > 15 bytes → static storage, sentinel refcount (never freed) |
| `string.from_utf8(bytes)` | `string or Utf8Error` | Validates bytes |
| `string.from_char(c)` | `string` | Single-char string |
| `s.repeat(n)` | `string` | `s` repeated `n` times, allocates |
| `slice.to_string()` | `string` | Copy slice bytes into new independent string (allocates) |

## String Builder

`StringBuilder` is the sole owner of its buffer — mutation is always O(1) amortized. `string` has no mutation methods.

| Operation | Signature | Notes |
|-----------|-----------|-------|
| `StringBuilder.new()` | `() -> StringBuilder` | Empty builder |
| `StringBuilder.with_capacity(n)` | `(usize) -> StringBuilder` | Pre-allocate |
| `b.push(s)` | `(mutate self, s: string)` | Add string at end |
| `b.push_char(c)` | `(mutate self, c: char)` | Add char at end |
| `b.build()` | `(take self) -> string` | Consume builder, return string. Zero-copy |
| `b.len()` | `(self) -> usize` | Current byte length |
| `b.is_empty()` | `(self) -> bool` | True if no bytes written |

`build()` consumes the builder and transfers the internal buffer to the new string without copying. The buffer is guaranteed valid UTF-8 by construction — `push` only accepts `string`, `push_char` only accepts `char`. `push` is the one "add to the end of a growable thing" verb — same as `Vec.push`.

**Interpolation optimization:** `b.push("hello {name}")` — compiler desugars interpolation directly into builder pushes, avoiding temp string allocation.

## Concatenation and Formatting

| Operation | Return | Notes |
|-----------|--------|-------|
| `"hello {name}"` | `string` | String interpolation, desugars to builder calls, allocates |

No `+` operator and no `concat` function. Interpolation is the one way to combine strings (`StringBuilder` for loops, `join` for lists). Allocation stays visible.

## Join

| Operation | Return | Notes |
|-----------|--------|-------|
| `strings.join(sep)` | `string` | Join a `Vec<string>` with separator, allocates |

<!-- test: skip -->
```rask
let names = ["Alice", "Bob", "Charlie"]
let result = names.join(", ")    // "Alice, Bob, Charlie"
let csv = headers.join(",")      // CSV header row
```

## Searching

| Operation | Return | Notes |
|-----------|--------|-------|
| `s.index_of(pat)` | `usize?` | Byte index of first match |
| `s.last_index_of(pat)` | `usize?` | Byte index of last match |
| `s.contains(pat)` | `bool` | Substring check |
| `s.starts_with(pat)` | `bool` | Prefix check |
| `s.ends_with(pat)` | `bool` | Suffix check |

## Trimming

| Operation | Return | Notes |
|-----------|--------|-------|
| `s.trim()` | Expression-scoped slice | Zero-copy, removes leading/trailing whitespace |
| `s.trim_start()` | Expression-scoped slice | Leading whitespace only |
| `s.trim_end()` | Expression-scoped slice | Trailing whitespace only |
| `s.trim_indices()` | `(usize, usize)` | Returns (start, end) byte indices of the trimmed region |

## Case Conversion

| Operation | Return | Notes |
|-----------|--------|-------|
| `s.to_uppercase()` | `string` | Allocates new string |
| `s.to_lowercase()` | `string` | Allocates new string |

## Character and Byte Access

| Operation | Return | Notes |
|-----------|--------|-------|
| `s.char_at(idx)` | `char?` | Get Unicode scalar at char index (not byte index) |
| `s.byte_at(idx)` | `u8?` | Get byte at byte index |

## Substring Extraction

| Operation | Return | Notes |
|-----------|--------|-------|
| `s.substring(start, end)` | `string` | Bytes from start (inclusive) to end (exclusive), allocates. Out-of-range clamps |

Indices are byte offsets, like everything else here: `len` is a byte count, and
`index_of` / `last_index_of` / `byte_at` all hand you byte offsets, so their
results feed straight back into `substring`. Counting characters instead would
make an index mean one thing coming out and another going in — and it hides an
O(n) scan behind what looks like a slice.

## Parsing

| Operation | Return | Notes |
|-----------|--------|-------|
| `s.parse_int()` | `i64 or ParseError` | Parse to integer, trims whitespace |
| `s.parse_float()` | `f64 or ParseError` | Parse to floating point, trims whitespace |

One name per operation — there is no generic `s.parse()`. The error is a real type (`type.errors/ER4` forbids `string` as an error):

<!-- test: parse -->
```rask
enum ParseError {
    Empty               // no digits found
    Invalid             // non-numeric character
    OutOfRange          // doesn't fit the target type
}
```

## String Manipulation

| Operation | Return | Notes |
|-----------|--------|-------|
| `s.replace(from, to)` | `string` | Replace all occurrences of pattern, allocates new string |
| `s.reverse()` | `string` | Reverse string by Unicode scalars, allocates new string |

## Equality and Comparison

| Operation | Cost | Notes |
|-----------|------|-------|
| `s1 == s2` | O(1) or O(n) | SSO: byte comparison (length check first, then memcmp). Heap: pointer+length fast path — same backing buffer and same length → equal without byte comparison. Otherwise byte-wise |
| `s1 < s2` | O(n) | Lexicographic |
| `s.hash()` | O(n) | Not cached |

## C Interop

| Type/Operation | Description |
|----------------|-------------|
| `cstring` | Owned null-terminated string |
| `c"literal"` | Null-terminated string literal |
| `s.to_cstring()` | `cstring or NullByteError` (fails if `\0` present) |
| `cstring.as_ptr()` | `*u8` (unsafe context only) |
| `cstring.from_ptr(ptr)` | `cstring` (unsafe, takes ownership) |
| `cstring.to_string()` | `string or Utf8Error` |

<!-- test: skip -->
```rask
unsafe {
    let c_path = try path.to_cstring()
    let fd = c_open(cstring.as_ptr(c_path), O_RDONLY)
}
```

## Error Messages

```
ERROR [std.strings/S2]: cannot store string slice
   |
3  |  let x = s[0..5]
   |            ^^^^^^^ string slices can't be stored

WHY: String slices are temporary views into a heap buffer.
     Convert to a storable form or use inline.

FIX 1: Store a zero-copy view (keeps s's buffer alive):

  let x = s[0..5].view()

FIX 2: Copy to an independent string:

  let x = s[0..5].to_string()  // allocate copy

FIX 3: Store indices instead:

  let v = Span(0, 5)    // store indices, resolve later
```

```
ERROR [std.strings/S5]: slice not on character boundary
   |
5  |  let x = text[0..2]
   |                 ^^^^ byte index 2 is not a char boundary

WHY: Slicing uses byte indices. Index must land on a UTF-8 character boundary.

FIX: Use char_indices() to find safe boundaries:

  for (i, c) in text.char_indices() { ... }
```

```
ERROR [std.strings/S7]: cannot mutate string
   |
3  |  s.push("x")
   |    ^^^^ string is immutable

WHY: Use StringBuilder for construction.

FIX:
  mut b = StringBuilder.new()
  b.push(s)
  b.push("x")
  let result = b.build()
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Empty string `""` | — | Valid, `len() == 0` |
| Out-of-bounds slice `s[0..999]` | S5 | Panic at runtime |
| Slice not on char boundary | S5 | Panic at runtime |
| Embedded `\0` in string | — | Valid; `to_cstring()` returns error |
| Allocation failure | — | Returns `T or E` error |
| String literal ≤ 15 bytes | S8 | SSO — inline value, no heap, no refcount |
| String literal > 15 bytes | S6 | Sentinel refcount, never freed/decremented |
| Short string (≤ 15 bytes) | S8 | SSO — pure value copy, no atomic ops |
| `Span` of freed source | — | Undefined behavior (user's responsibility) |
| `Span` out of bounds | — | Panic on `s[span]`, `none` on `s.get(span)` |
| `.view()` on SSO string | V4 | Bytes copied inline — no heap, no pin |
| View ≤ 15 bytes from heap string | V4 | Stored inline — no refcount, no pin |
| View of literal (> 15 bytes) | V1/S6 | References sentinel storage — no atomic ops, nothing pinned |
| `.view()` mid-codepoint or OOB slice | S5 | Panic (the slice panics before `.view()` runs) |
| View from source offset > 4 GiB or length ≥ 2 GiB | V1 | Panic — use `.to_string()` for the range |
| `view == string` | V3 | Byte comparison, both directions |
| View sent cross-task | V1 | Allowed — refcount is atomic |
| Last view outlives all copies of source string | V1 | Fine — buffer freed when last holder (string or view) drops |
| Refcount overflow | S6 | Panic (practically unreachable — requires ~4 billion live copies) |
| Multiple simultaneous iterators | — | Allowed (string is immutable) |

---

## Appendix (non-normative)

### Rationale

**S1 (immutable, refcounted, Copy):** I audited all validation programs (~5,000 lines including LSM database, stdlib). Found ~60+ `.clone()` on strings — concentrated in lock scope reads, divergent use, and parser state flush. Evaluated three models:

- **Status quo** — O(n) clone, 60+ explicit `.clone()` calls
- **COW** — O(1) clone but hidden O(n) mutation cost (violates transparency)
- **Immutable + refcount** — O(1) copy, no hidden costs, builder for mutation

Immutable wins over COW: no hidden mutation cost, builder is sole owner so mutation is always O(1). Refcount over GC: deterministic, fits the ownership model. Builder pattern is established (Go, C#, Java) and concentrated in few callsites (~8 `.build()` calls across a 1,114-line renderer).

This is one of the few cases where a type owns heap memory but is still Copy. The immutable + refcounted design makes sharing safe — there's no aliased mutation to worry about. The 16-byte representation (tagged union — see S8) fits under the VS1 threshold. SSO means most short strings never touch the heap at all.

**S2 (inline slicing):** String slices are temporary views into the buffer without their own refcount — storing one would dangle if the source string is freed. Slices are valid for the expression only. `.to_string()` copies bytes into a new independent string — no shared backing. A 50-byte slice must not silently retain a 10MB source buffer. The `.to_string()` calls are honest cost markers bounded by the slice size, not the source size.

**S3 (public APIs use string):** Forces a clean boundary. Callers never need to know about internal storage strategies.

**S5 (byte indices):** Byte indexing matches the underlying UTF-8 representation. Character indexing would be O(n) and misleading for multi-byte characters.

**S6 (refcount semantics):** Atomic refcount enables safe sharing across tasks. Sentinel refcount for literals avoids overhead on the most common case. Compiler optimization for provably sole-owner strings eliminates atomic ops when sharing can't happen.

**S7 (builder for mutation):** All mutation lives on `StringBuilder`. This means `string` is truly immutable — no COW surprise, no hidden cost. The builder is always the sole owner of its buffer, so mutation is always O(1) amortized.

**S8 (small string optimization):** Short strings are the most common case in many programs — field names, status codes, short identifiers, small log messages. The 15-byte threshold covers the vast majority of these. Without SSO, every string — even `"OK"` — heap-allocates and atomic-refcounts. With SSO, short strings are pure 16-byte values: no heap, no refcount, same cost as copying an `i128`. The tagged union uses a well-proven technique (same approach as libc++ and fbstring): the MSB of the last byte discriminates between SSO and heap mode. The 16-byte size and Copy semantics are unchanged — SSO is invisible to user code. `StringBuilder.build()` produces an SSO string when the result is ≤ 15 bytes, avoiding the heap allocation entirely.

**V1–V6 (StringView):** This is the zero-copy answer (issue #492). Before views, everything that escaped an expression was a copy: parsers couldn't return slices into their input, deserializers couldn't have borrowed fields, split results couldn't be collected without allocating per item. For a language whose pitch is "eliminate abstraction tax", copies-everywhere in parsing hot paths was the tax, relocated.

Directions evaluated:

- **Typed spans** (a `Span` carrying provenance so resolving against the wrong string errors): compile-time branding is lifetime tracking through the side door — exactly the machinery Rask exists to avoid. Runtime provenance needs per-string identity (a generation), which refcounted immutable strings don't have. `StringPool` was this idea built out of existing parts, and it stayed manual: an extra pool value threaded everywhere, handles resolved by hand at each use.
- **A borrowed-input scope** (`with input as ...` where slice-typed locals and fields of locally-scoped structs are legal): needs a new kind of escape checking and a new "local-only struct" concept that infects the type system — any struct with a slice field becomes scope-bound, and that property propagates through everything that contains it. Too much machinery for one pattern.
- **Accepting the copy tax:** gives up the pitch in exactly the workloads (parsing, proxying, networking) a systems language is judged on.
- **Refcounted views** (chosen): `string` is already the blessed refcount exception — immutable, so sharing is safe; refcounted, so a view holding a count can't dangle. A view is mechanically safe by construction, needs zero new analysis (it's just a value), and costs one atomic bump that the existing elision pass (`comp.string-refcount-elision`) already knows how to delete.

**Why the old "small slice pins large buffer" rejection doesn't apply:** that argument (S2 rationale) was against `.to_string()` sharing *implicitly* — a `string` that sometimes secretly retains 10MB. `StringView` makes the pin opt-in and visible at both ends: `.view()` at the creation site, `StringView` in the struct definition. A reader seeing `StringView` fields knows the source buffer outlives them; `.to_string()` is the release valve when that's wrong (long-lived tokens from a huge input). Same transparency principle, both directions.

**Why `.view()` and not `s.view(start, end)`:** slicing syntax already exists and composes — `line[0..colon].trim().view()` captures the trimmed sub-range with no copy. One method, no arity overloads, and every slice-producing operation (indexing, `trim`, `split`, `lines`, `Span` resolution) gets storability for free.

**Why StringPool/StringSlice are gone:** `StringView` covers the same use case — validated storable substrings — with no pool value to thread through call graphs and no handle resolution at use sites. What remains of StringPool's pitch is interning, and interning is deduplication, not new sharing: `Map<string, Handle<T>>` per `mem.boxes/BX3`. Neither type was implemented.

**What views don't cover:** binary data. Parsing bytes zero-copy (`Vec<u8>` views) has no equivalent — `Vec<u8>` is mutable, so views into it would need real borrow tracking. `Shared<Vec<u8>>` remains the refcounted-buffer answer (`mem.boxes/BX3`). If an immutable `bytes` primitive ever lands, it should get the same view treatment; that's a separate decision.

### Why Immutable Strings?

Three models were evaluated with concrete impact across ~5,000 lines of validation programs:

**Status quo (owned, mutable, move semantics):** 60+ `.clone()` calls. Half eliminable with O3 borrow inference, but ~30 genuinely needed for lock scope reads, divergent use, and parser state flush. Each clone is O(n).

**COW (copy-on-write, shared buffer):** O(1) clone — just bump the refcount. But mutation is O(n) when shared, O(1) when unique. The cost depends on sharing state established elsewhere — non-local reasoning that Rask exists to prevent. Violates transparency of cost.

**Immutable + refcount (this design):** O(1) copy (16-byte header + atomic increment). No hidden costs. Builder is sole owner so mutation is always O(1). Eliminates all `.clone()` on strings.

The grep_clone validation program (string-heavy CLI tool) had zero `.clone()` calls even under the status quo — but that's because it was carefully structured. The immutable design means you don't need careful structuring; strings just copy freely like in Go.

**Why not COW?** The call site looks identical for O(1) and O(n) mutation. The cost depends on how many other references exist — invisible at the mutation site. This is exactly the kind of hidden cost Rask exists to prevent.

**Why refcount, not GC?** Deterministic cleanup. Fits the ownership model. No pauses.

**Why Copy despite owning heap memory?** Normally heap-owning types move. But `string` is immutable — there's no aliased mutation risk. The refcount makes sharing safe. And at 16 bytes, it fits under the Copy threshold. This is a principled exception, not a hack.

### Why Only String?

`string` is a language primitive, like `i32` or `bool`. The compiler knows its exact layout and refcount semantics — user types can't opt into refcounted Copy behavior.

The pressure to extend this to `Path`, `Vec<u8>`, or custom wrappers is anticipated and rejected. Those types are mutable — refcounted Copy requires immutability. And even for hypothetical user-defined immutable types, the compiler can't verify deep immutability without a whole new annotation system. Getting it wrong means data races from elided refcounts on aliased mutable data.

For cheap sharing of arbitrary data, use `Shared<T>` — explicit, visible, correct. `string` gets special treatment because it's the most common type in most programs and the ergonomic cost of `.clone()` on strings was disproportionate to the actual risk.

### Builder Patterns

**Basic construction:**

<!-- test: skip -->
```rask
mut b = StringBuilder.new()
b.push("User: ")
b.push(name)
b.push_char('\n')
let msg = b.build()
```

**Accumulator pattern** — create a new builder per iteration:

<!-- test: skip -->
```rask
func flush_lines(lines: Vec<string>) -> Vec<string> {
    mut results = Vec.new()
    for line in lines {
        mut b = StringBuilder.new()
        b.push(line)
        b.push_char('\n')
        results.push(b.build())
    }
    return results
}
```

**Rendering trait pattern:**

<!-- test: skip -->
```rask
trait Renderable {
    func render(self, mutate builder: StringBuilder)
}

extend HtmlTag: Renderable {
    func render(self, mutate builder: StringBuilder) {
        builder.push("<{self.tag}>")
        for child in self.children {
            child.render(builder)
        }
        builder.push("</{self.tag}>")
    }
}
```

**Interpolation in builder** — compiler desugars efficiently:

<!-- test: skip -->
```rask
// These are equivalent, but the compiler optimizes the interpolation
// form to avoid creating a temp string:
builder.push("tag {value}")
// ≈
builder.push("tag ")
builder.push(value.to_string())
```

### Patterns & Guidance

**Basic usage:**

<!-- test: skip -->
```rask
let s1 = "hello"
let s2 = s1  // COPY: both s1 and s2 valid (refcount incremented)

process(s2[0..3])  // passes "hel" as temporary slice

let kept = s2[0..3].view()  // storable zero-copy view (V1)
process(kept)

let span = Span(0, 3)
process(s2[span])  // same as s2[0..3], user ensures s2 is still valid
```

**Parsing with StringView (zero-copy tokens):**

<!-- test: skip -->
```rask
struct Token {
    kind: TokenKind
    text: StringView
}

func tokenize(source: string) -> Vec<Token> {
    mut tokens = Vec.new()
    for (start, end, kind) in scan(source) {
        tokens.push(Token { kind, text: source[start..end].view() })
    }
    return tokens
}
```

One source buffer, N tokens, zero copies. Each view bumps the source's refcount (elidable); the buffer lives until the last token drops.

**Collecting split results:**

<!-- test: skip -->
```rask
mut fields: Vec<StringView> = Vec.new()
for part in line.split(",") {
    fields.push(part.trim().view())
}
```

**Safe character-boundary access:**

<!-- test: skip -->
```rask
let text = "日本語"
for (i, c) in text.char_indices() {
    process(text[i..i+c.len_utf8()])
}
```

### Integration

- `string` and `StringView` implement `Displayable`, `Hashable`, `Comparable` traits. Copy is structural (S1, V1)
- All types (`string`, `StringView`, `Span`, `StringBuilder`) are in core prelude
- String builders can contain linear resources; `build()` consumes builder to preserve linearity
- String literals ≤ 15 bytes produce SSO values (inline, no allocation). Longer literals use static storage with sentinel refcount. Comptime interpolation follows the same rule based on result length
- Auto-derived `Decode` into `StringView` fields (serde-style borrowed deserialization) is a natural extension once `std.encoding` lands — the decoded struct's fields view the input document. Not specified yet

### Implementation Notes (Interpreter)

Current interpreter behavior differs from spec in some areas:

**Trimming returns owned strings:**
- `s.trim()`, `s.trim_start()`, `s.trim_end()` return new `string` instead of expression-scoped slices
- This causes allocation but matches common usage patterns

**Method name aliases:**
- The interpreter still accepts the removed aliases `s.parse()` (for `parse_int`) and `s.find(pat)` (for `index_of`)

These will converge to spec behavior in the compiled version.

### See Also

- `mem.borrowing` — Inline access (B2) for strings, block-scoped (B1) for struct fields/arrays
- `mem.boxes` — Why refcounted sharing is a closed set (`BX1`–`BX4`)
- `comp.string-refcount-elision` — Atomic op elision, applies to views identically
- `std.iteration` — General iteration design
- `std.path` — Path type wraps string
