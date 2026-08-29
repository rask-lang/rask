<!-- id: std.strings -->
<!-- status: decided -->
<!-- summary: Immutable refcounted string (Copy), bytes for indexing and graphemes for display, inline slicing, StringView for storable zero-copy substrings, StringBuilder for construction -->
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
| **S10: ASCII flag** | Every string knows whether it is pure ASCII, decided at construction and stored in the value. `is_ascii()` is O(1). This is what makes U4 work |

### Internal Layout (S1 + S8)

16 bytes, tagged union. The MSB of the last byte discriminates between modes:

```
Heap mode (last byte MSB = 0):
  [header_ptr: *u8 (8B)][len: usize (8B)]
  Header at ptr: { refcount: atomic_u32, flags: u32, capacity: u32, data: [u8] }
                                         ^ bit 0: ASCII

SSO mode (last byte MSB = 1):
  [inline_data: [u8; 15]][len_tag: u8]
  Length = len_tag & 0x0F (range 0..15), bit 6 = ASCII, bit 7 = SSO tag
```

SSO strings are pure value copies — no heap, no refcount. Heap strings share backing storage via atomic refcount. Both modes are 16 bytes, both are Copy. The mode is invisible to user code.

**Where the ASCII flag comes from (S10):** every way to build a string already touches every byte — a literal is scanned at compile time, `from_utf8` validates, `StringBuilder` sees each push, a slice copies. So the flag is computed on a pass that was happening anyway; it is not a lazy cache and there is no "unknown" state. Views inherit what they can: a view of an ASCII string is ASCII for free. A view of a non-ASCII string answers `is_ascii()` by scanning its own range — O(range), not O(1) — because computing it at `.view()` time would cost the zero-copy guarantee (V1).

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

## Text Units

Text has three possible units — bytes, Unicode scalars, and graphemes (what a reader calls "a character"). Mixing them is where string APIs go wrong: an index that means bytes coming out and characters going in, a padding width that counts the wrong thing, a truncation that splits an emoji in half.

Rask picks per purpose and never leaves it ambiguous.

| Rule | Description |
|------|-------------|
| **U1: Bytes for machines** | Every index, length, offset and range is a byte offset. `len`, `s[i..j]`, `index_of`, `byte_at`, `char_indices`, `Span` all speak bytes, so a result feeds straight back in as an argument. No operation takes an index in any other unit |
| **U1b: `[]` on a string is a range** | `s[i..j]` slices; `s[i]` does not exist. A single-element index is ambiguous about what it yields, and `s[i]` counting characters while `s[i..j]` counted bytes is exactly the bug this rule closes. Read one position with `byte_at(i)` or `char_at(i)` — both byte-offset, both O(1) — and walk with `chars()`/`graphemes()` |
| **U2: Graphemes for humans** | Anything a person sees or counts works in graphemes or display columns: `width`, `truncate`, `graphemes`, `reverse`. These are O(n) and named so the scan is visible |
| **U3: Scalars are not a unit** | A Unicode scalar is neither a byte nor a character, so *counting* them answers no real question and there is no `char_count`. Scalars reach user code through `chars()`, `char_indices()` and `char_at(byte_index)` — the lexer's tools — and nowhere else |
| **U4: ASCII is free** | Every string carries an ASCII flag, set at construction (S10). For an ASCII string all three units coincide, so `width`, `graphemes`, `truncate` and `normalized` take a byte-only fast path. Unicode correctness costs nothing until the bytes are actually non-ASCII |
| **U5: Normalization is explicit** | `"café"` composed and decomposed are different byte sequences, so `==` says false. Making `==` normalize would hide an O(n) table lookup behind an operator. `s.normalized()` returns the NFC form and the cost is at the call site |

`s.width()` is the load-bearing one: it's what `fmt`'s padding counts (`std.fmt/S2`), which is what makes an aligned table actually align.

<!-- test: skip -->
```rask
let s = "héllo"

s.len()          // 6 — bytes
s.width()        // 5 — display columns
s.graphemes()    // iterator: "h", "é", "l", "l", "o"

// The four operations people get wrong, done right:
s.truncate(3)                    // "hél" — never splits a grapheme
s.reverse()                      // "olléh" — not scalar-reversed mojibake
format("{:<10}|", s)             // pads to 10 columns, not 10 bytes
a.normalized() == b.normalized() // "café" == "café" regardless of source
```

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
| `s[i..j].span()` | `Span` | The slice's byte range. Works on any expression-scoped slice — `s.trim().span()`, split items |
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
| `s.graphemes()` | Expression-scoped slices | What a reader calls characters (U2). The unit for cursors and truncation |
| `s.chars()` | `char` (u32 Unicode scalar) | Expression-scoped iterator. For lexers and protocol parsers (U3) |
| `s.bytes()` | `u8` | Raw byte iterator |
| `s.char_indices()` | `(usize, char)` | Byte offset + scalar pairs. The offset is a byte index (U1) |
| `s.lines()` | Expression-scoped slices | Split on newlines |
| `s.split(pat)` | Expression-scoped slices | Split on pattern |
| `s.split_whitespace()` | Expression-scoped slices | Split on Unicode whitespace, skip empty |

Slice-yielding iterators are zero-cost per item. To keep items, convert each: `.view()` (zero-copy, pins source — V2) or `.to_string()` (copies).

## Length and Properties

| Operation | Return | Cost |
|-----------|--------|------|
| `s.len()` | `usize` | O(1), byte length (U1) |
| `s.width()` | `usize` | Display columns (U2). O(1) for ASCII, else O(n) |
| `s.is_empty()` | `bool` | O(1) |
| `s.is_ascii()` | `bool` | O(1) — decided at construction (S10) |

`len` is what you index with; `width` is what you align with. There is no third
count. "How many characters" as a bare number answers no real question — the
caller either wants bytes (indexing), columns (layout), or to walk the graphemes
(editing), and `char_count` was none of those (U3).

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

Need the trimmed region's offsets rather than its text? `s.trim().span()` — `Span`
is the type for positions, so trimming doesn't need a second spelling that returns
a bare `(usize, usize)`.

## Case and Normalization

| Operation | Return | Notes |
|-----------|--------|-------|
| `s.to_uppercase()` | `string` | Full Unicode case mapping — allocates. `"straße"` → `"STRASSE"`, so length can change |
| `s.to_lowercase()` | `string` | Full Unicode case mapping — allocates |
| `s.normalized()` | `string` | NFC form (U5). O(1) return for ASCII (already normal), else allocates |

Case mapping on `string` is the full 1:many mapping; `char.to_uppercase()` is a
1:1 scalar mapping and cannot be otherwise, since `ß` uppercases to two
characters. They disagree on purpose, and the `char` version says so — reach for
the `string` one unless you're doing scalar work.

`normalized()` is the fix for the comparison that surprises everyone: text typed
on macOS is usually NFD, text from a web form usually NFC, and the same visible
word compares unequal. Normalize both sides when the source is outside your
program — filenames, form input, anything off the network:

<!-- test: skip -->
```rask
if a.normalized() == b.normalized() { … }
```

Not built into `==`. That would put a table lookup behind an operator, on the
one comparison that must stay O(1) on the fast path.

## Indexed Access

| Operation | Return | Notes |
|-----------|--------|-------|
| `s.byte_at(i)` | `u8?` | The byte at byte offset `i` |
| `s.char_at(i)` | `char?` | The scalar *starting* at byte offset `i`. `none` when out of range or when `i` is inside a character |

Both take byte offsets (U1) and both are O(1). That's the whole fix: `char_at`
used to take a *character* index, so it scanned from byte zero on every call and a
cursor loop built on it was quadratic while reading like `Vec` indexing.

With byte offsets a cursor is O(1) per step, and `char.len_utf8()` advances it:

<!-- test: skip -->
```rask
func peek(self) -> char? {
    return self.source.char_at(self.pos)
}

func advance(mutate self) {
    if self.source.char_at(self.pos)? as c {
        self.pos = self.pos + c.len_utf8()
    }
}
```

To walk without a cursor, iterate: `s.chars()` for scalars, `s.graphemes()` for
what a reader sees.

## Substring Extraction

Slice it: `s[start..end]` for expression scope, `.to_string()` to keep it,
`.view()` to keep it without copying. There is no `substring` method — it was
`s[a..b].to_string()` under a second name (`std.api/SD5`), and the Java spelling
invited the character-index reading that U1 rules out.

Out-of-range clamps; a cut that lands *inside* a character panics, because the
result would be a `string` that isn't valid UTF-8, which the type says can't
exist. Every offset the API hands you (`index_of`, `last_index_of`,
`char_indices`) is already a boundary, so this only fires on arithmetic the caller
did themselves.

## Parsing

| Operation | Return | Notes |
|-----------|--------|-------|
| `s.parse<T>()` | `T or ParseError` | Any numeric `T`, answered at `T`'s width. Trims whitespace |

One function. `parse<i64>()` and `parse<f64>()` are what `parse_int` and
`parse_float` were, and three names for one operation is surface growth by name
where a type parameter already does the job (`std.api/SD2`).

`parse<T>` answers at the target's own width: `"70000".parse<u8>()` and
`"-1".parse<u64>()` are both `OutOfRange`, and `parse<u64>` reaches `u64::MAX`.
The error is a real type (`type.errors/ER4` forbids `string` as an error):

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
| `s.replace(from, to)` | `string` | Replace all occurrences, allocates new string |
| `s.replace(from, to, limit: n)` | `string` | Replace at most `n`, left to right. Absent means all |
| `s.reverse()` | `string` | Reverse by graphemes (U2), allocates new string |
| `s.truncate(cols)` | `string` | Cut to at most `cols` display columns, never splitting a grapheme (U2) |

`replace` takes a count as an optional parameter rather than splitting into a
`replacen` sibling — same operation, one more knob (`std.api/SD2`). The parameter
is absent-or-a-number, not a sentinel: `limit: 0` replaces nothing, and "all" is
what you get by not asking for a limit.

`reverse` works in graphemes because scalar reversal has no correct use: it turns
`"né"` into a combining accent looking for a letter, and any emoji into rubble.
`truncate` is the operation people hand-roll wrong — it's `width` and `graphemes`
composed correctly, so the stdlib does it once.

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
| `truncate(n)` where column `n` lands mid-grapheme | U2 | Cuts before it — result is narrower than `n`, never wider |
| `truncate(n)` on a zero-width or combining prefix | U2 | Combining marks stay with their base character |
| `width()` of a control character | U2 | Zero. Terminal output of raw controls is the caller's problem |
| `width()` of an unassigned codepoint | U2 | 1 — the common terminal behavior, and stable across Unicode updates |
| `normalized()` on already-NFC text | U5 | Returns the same string, no allocation |
| `to_uppercase()` changing byte length | — | Expected — `"straße"` → `"STRASSE"`. Offsets into the source do not carry over |

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

**U1–U3 (text units):** The first draft of this API had it both ways — and the sharpest case was the indexing operator itself. `s[i]` counted characters while `s[i..j]` counted bytes, so the same bracket meant two different units depending on whether a `..` was in it:

```rask
let s = "aöb"     // 4 bytes, 3 characters
s[1]              // 'ö'  — character index 1
s[1..3]           // "ö"  — bytes 1 through 3
s.len()           // 4    — bytes
```

Every `s[i]` rescans from byte zero, so a loop over a string by index is quadratic while looking exactly like `Vec` indexing.

S5 had argued for byte indices and given the reason — "counting characters would make an index mean one thing coming out and another going in, and it hides an O(n) scan behind what looks like a slice" — and then `char_at(idx)` shipped taking a character index, doing exactly that. Three lexers in this repo were built on it:

```rask
// projects/raido/src/lexer.rk — self.pos advances per character
return self.source.char_at(self.pos)
```

Every call rescans from byte zero, so each lexer was quadratic in its input. Not a Unicode problem — a units problem, in a language whose pitch is removing abstraction tax. `char_count` was the same mistake with the count instead of the index, and it never even reached real code: all six call sites in the repo were tests asserting `"hello".char_count() == 5`.

The fix isn't to pick one unit, it's to stop leaving it implicit. Bytes are what indexing means, graphemes are what people mean, and scalars are neither — they're a UTF-8 implementation detail that lexers legitimately want and nobody else does.

So `char_at` survives, byte-indexed: the name was never the problem, the unit was. Byte-indexed it's O(1), it sits beside `byte_at` reading the same offset two ways, and every offset the API produces feeds it. `char_count` doesn't survive, because a scalar count answers nothing — you wanted `len` (indexing), `width` (layout), or to walk the graphemes (editing).

**U4 (ASCII is free):** This is what makes U2 affordable. Correct text handling normally trades against speed — Swift segments graphemes on every count, Rust makes you reach for a crate so most programs just get it wrong. Rask charges by the data instead: for ASCII, bytes and columns and graphemes are the same number, so the correct call is a byte op. Since every construction path already walks the bytes, knowing which case you're in costs nothing (S10), and the branch is one flag test. Programs that never see non-ASCII pay for none of it, and — with dead code elimination — don't link the tables either.

**U5 (explicit normalization):** The guess test (`std.api/SD3`) says a developer types `a == b` and expects `"café" == "café"` regardless of how each was typed. That guess is reasonable and we're refusing it, which the rule normally forbids. It loses to transparency of cost: `==` on strings has an O(1) fast path (same buffer, same length) that a normalizing comparison would destroy, on the most-called operation in most programs. So the guess fails, and the compensation is that the right call is short, obvious, and mentioned wherever text crosses into the program.

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

**`.view()` shares, but the slice under it already allocated:**
- `.view()` works on both backends. A view is a `RaskStr` sharing the source's buffer and holding a refcount on it, so V1, V2, V4 and V6 hold and a stored view can't dangle
- What doesn't hold yet is V5's "no copy either way" on a *sub-range*. `s[i..j]` and `s.trim()` allocate today (above), so `s.trim().view()` shares the trimmed copy rather than referencing a range of `s` — one allocation per slice, not per view
- The header-pointer + start/len layout under "Internal Layout" is what removes that last allocation. Until slices are ranges instead of copies, a view always covers the whole of its (already copied) source, so `start`/`len` have nothing to say
- Read-only API on a view is `len`, `is_empty`, `to_string`, `view`, `hash` and interpolation. The rest of V3 (`index_of`, `chars`, sub-slicing, comparison) isn't wired up

**Method name aliases:**
- The interpreter still accepts `s.find(pat)` as an alias for `index_of`

**`replace` has no `limit:` yet:** default arguments don't work on methods
(rask-lang/rask#1028), so only the two-argument form exists today. The limit
form lands with that fix.

**Text units not yet implemented:**
- `width`, `graphemes`, `truncate` and `normalized` (U2, U5) need the grapheme-break, East Asian Width and NFC tables in the runtime. Until those land, the ASCII fast path (U4) is the only path — correct for ASCII input, wrong for the rest
- The ASCII flag (S10) is not stored yet; `is_ascii()` scans

These will converge to spec behavior in the compiled version.

### See Also

- `mem.borrowing` — Inline access (B2) for strings, block-scoped (B1) for struct fields/arrays
- `mem.boxes` — Why refcounted sharing is a closed set (`BX1`–`BX4`)
- `comp.string-refcount-elision` — Atomic op elision, applies to views identically
- `std.iteration` — General iteration design
- `std.path` — Path type wraps string
