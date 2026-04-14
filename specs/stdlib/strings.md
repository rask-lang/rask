<!-- id: std.strings -->
<!-- status: decided -->
<!-- summary: Immutable refcounted string (Copy), inline slicing, StringBuilder for construction -->
<!-- depends: memory/borrowing.md, memory/value-semantics.md, memory/pools.md -->

# String Handling

Immutable refcounted `string` type with UTF-8 validation, inline slicing for zero-copy expression access, `StringBuilder` for construction, `StringPool` for validated handle-based access. `Span` (a core type, not string-specific) is used for byte-index ranges.

## Type Categories

| Rule | Description |
|------|-------------|
| **S1: Immutable, refcounted, Copy** | `string` is UTF-8, immutable, 16 bytes (tagged union — see S8). Under VS1 threshold → implicit Copy |
| **S2: Inline slicing** | `s[i..j]` creates a temporary view valid only within the expression |
| **S3: Public APIs use string** | Prefer `string` over `StringSlice` in public APIs. `Span` is fine — it's a general-purpose range type |
| **S4: UTF-8 required** | Strings must contain valid UTF-8. Validated at construction |
| **S5: Byte indices** | Slicing uses byte indices. Mid-codepoint slice panics at runtime |
| **S6: Refcount semantics** | Atomic refcount in heap header. SSO strings (S8) bypass refcounting entirely. Literals ≤ 15 bytes use SSO; longer literals use sentinel refcount (never freed/decremented). Compiler elides atomic ops for provably sole-owner heap strings (see `comp.string-refcount-elision`). This is a language primitive — not available to user-defined types |
| **S7: Builder for mutation** | `append`, `append_char` live on `StringBuilder` only. `string` has no mutation methods |
| **S8: Small string optimization** | Strings ≤ 15 bytes are stored inline in the 16-byte value (no heap allocation, no refcount). Longer strings use heap mode with refcounted header. Layout is a tagged union — discriminant is the MSB of the last byte. User-facing semantics are identical in both modes |

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
| `Span` | Plain indices into a string | Copy (2 words) | Yes |
| `StringBuilder` | Growable mutable buffer | Move on assignment | Yes |
| `StringPool` | Pool of strings with validated handles | Move on assignment | Yes |
| `StringSlice` | Handle + indices into StringPool | Copy (4 words) | Yes |
| `cstring` | Null-terminated for C FFI | Move on assignment | Yes (unsafe only) |

## Ownership Rules

| Rule | Description |
|------|-------------|
| **O1: Copy on assign** | `const s2 = s1` copies 16 bytes. For heap strings, atomic refcount increment. For SSO strings, plain memcpy (no refcount). Both remain valid |
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
| Variable assignment | `const x = s[0..5]` | Compile error |
| Struct field | `Foo { field: s[0..5] }` | Compile error |
| Return value | `return s[0..5]` | Compile error |

> **Why copy on `.to_string()`, not shared slice?** A 50-byte substring must not silently retain a 10MB source buffer. `.to_string()` copies bytes into a fresh allocation with its own refcount. The cost is visible and bounded by the slice size, not the source size. This prevents the classic "small slice pins large buffer" memory leak.

## The `Span` Type

Plain indices for lightweight stored references — the span type for parsers, tokenizers, and diagnostics. No validation — user ensures source string validity (like storing a Vec index). 16 bytes, copy-eligible.

| Operation | Return | Notes |
|-----------|--------|-------|
| `Span(i, j)` | `Span` | Create view (just start, end indices) |
| `source[view]` | expression-scoped slice | Panics if out of bounds |
| `source.substr(view)` | `Option<expression-scoped slice>` | Safe bounds check |
| `view.to_string(source)` | `string` | Allocates copy (panics if OOB) |
| `view.start`, `view.end` | `usize` | Read indices |
| `view.len()` | `usize` | `end - start` |

## The `StringPool` Type

Validated stored references using handles (parsers, tokenizers, ASTs). Follows `Pool<T>` pattern.

| Operation | Return | Notes |
|-----------|--------|-------|
| `StringPool.new()` | `StringPool` | Empty pool |
| `pool.insert(s)` | `Result<Handle<string>, InsertError>` | Add string, get handle |
| `pool.slice(h, i, j)` | `Result<StringSlice, Error>` | Create validated slice |
| `pool[slice]` | inline access (expression-scoped) | Panics if invalid |
| `pool.get(slice)` | `Option<inline access>` | Safe access |
| `with pool[slice] as s { ... }` | block value | Multi-statement access |
| `pool.remove(h)` | `Option<string>` | Remove and return ownership |

Handle validation: pool_id + index + generation. Wrong pool or stale handle returns `None`.

## UTF-8 Validation

| Operation | Return Type | Validation Cost |
|-----------|-------------|-----------------|
| `"literal"` | `string` | Compile-time |
| `string.from_utf8(bytes)` | `Result<string, utf8_error>` | Runtime O(n), one-time |
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
| `string.from_utf8(bytes)` | `Result<string, utf8_error>` | Validates bytes |
| `string.from_char(c)` | `string` | Single-char string |
| `string.repeat(s, n)` or `s.repeat(n)` | `string` | `s` repeated `n` times, allocates |
| `slice.to_string()` | `string` | Copy slice bytes into new independent string (allocates) |

## String Builder

`StringBuilder` is the sole owner of its buffer — mutation is always O(1) amortized. `string` has no mutation methods.

| Operation | Signature | Notes |
|-----------|-----------|-------|
| `StringBuilder.new()` | `() -> StringBuilder` | Empty builder |
| `StringBuilder.with_capacity(n)` | `(usize) -> StringBuilder` | Pre-allocate |
| `b.append(s)` | `(mutate self, s: string)` | Append string |
| `b.append_char(c)` | `(mutate self, c: char)` | Append char |
| `b.build()` | `(take self) -> string` | Consume builder, return string. Zero-copy |
| `b.len()` | `(self) -> usize` | Current byte length |
| `b.is_empty()` | `(self) -> bool` | True if no bytes written |

`build()` consumes the builder and transfers the internal buffer to the new string without copying. The buffer is guaranteed valid UTF-8 by construction — `append` only accepts `string`, `append_char` only accepts `char`.

**Interpolation optimization:** `b.append("hello {name}")` — compiler desugars interpolation directly into builder appends, avoiding temp string allocation.

## Concatenation and Formatting

| Operation | Return | Notes |
|-----------|--------|-------|
| `string.concat(a, b)` | `string` | Allocates new string |
| `"hello {name}"` | `string` | String interpolation, desugars to builder calls, allocates |

No `+` operator. Allocation must be visible via method name or interpolation.

## Join

| Operation | Return | Notes |
|-----------|--------|-------|
| `strings.join(sep)` | `string` | Join a `Vec<string>` with separator, allocates |

<!-- test: skip -->
```rask
const names = ["Alice", "Bob", "Charlie"]
const result = names.join(", ")    // "Alice, Bob, Charlie"
const csv = headers.join(",")      // CSV header row
```

## Searching

| Operation | Return | Notes |
|-----------|--------|-------|
| `s.find(pat)` or `s.index_of(pat)` | `Option<usize>` | Byte index of first match |
| `s.rfind(pat)` | `Option<usize>` | Byte index of last match |
| `s.contains(pat)` | `bool` | Substring check |
| `s.starts_with(pat)` | `bool` | Prefix check |
| `s.ends_with(pat)` | `bool` | Suffix check |

## Trimming

| Operation | Return | Notes |
|-----------|--------|-------|
| `s.trim()` | Expression-scoped slice | Zero-copy, removes leading/trailing whitespace |
| `s.trim_start()` | Expression-scoped slice | Leading whitespace only |
| `s.trim_end()` | Expression-scoped slice | Trailing whitespace only |
| `s.trim_bounds()` | `(usize, usize)` | Returns (start, end) indices |

## Case Conversion

| Operation | Return | Notes |
|-----------|--------|-------|
| `s.to_uppercase()` | `string` | Allocates new string |
| `s.to_lowercase()` | `string` | Allocates new string |

## Character and Byte Access

| Operation | Return | Notes |
|-----------|--------|-------|
| `s.char_at(idx)` | `Option<char>` | Get Unicode scalar at char index (not byte index) |
| `s.byte_at(idx)` | `Option<u8>` | Get byte at byte index |

## Substring Extraction

| Operation | Return | Notes |
|-----------|--------|-------|
| `s.substring(start, end)` | `string` | Extract chars from start (inclusive) to end (exclusive), allocates |

## Parsing

| Operation | Return | Notes |
|-----------|--------|-------|
| `s.parse_int()` or `s.parse()` | `Result<i64, string>` | Parse to integer, trims whitespace |
| `s.parse_float()` | `Result<f64, string>` | Parse to floating point, trims whitespace |

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
| `s.to_cstring()` | `Result<cstring, null_byte_error>` (fails if `\0` present) |
| `cstring.as_ptr()` | `*u8` (unsafe context only) |
| `cstring.from_ptr(ptr)` | `cstring` (unsafe, takes ownership) |
| `cstring.to_string()` | `Result<string, utf8_error>` |

<!-- test: skip -->
```rask
unsafe {
    const c_path = try path.to_cstring()
    const fd = c_open(cstring.as_ptr(c_path), O_RDONLY)
}
```

## Error Messages

```
ERROR [std.strings/S2]: cannot store string slice
   |
3  |  const x = s[0..5]
   |            ^^^^^^^ string slices can't be stored

WHY: String slices are temporary views into a heap buffer.
     Use inline or copy out.

FIX 1: Copy to owned string:

  const x = s[0..5].to_string()  // allocate copy

FIX 2: Store indices instead:

  const v = Span(0, 5)    // store indices, resolve later
```

```
ERROR [std.strings/S5]: slice not on character boundary
   |
5  |  const x = text[0..2]
   |                 ^^^^ byte index 2 is not a char boundary

WHY: Slicing uses byte indices. Index must land on a UTF-8 character boundary.

FIX: Use char_indices() to find safe boundaries:

  for (i, c) in text.char_indices() { ... }
```

```
ERROR [std.strings/S7]: cannot mutate string
   |
3  |  s.append("x")
   |    ^^^^^^ string is immutable

WHY: Use StringBuilder for construction.

FIX:
  let b = StringBuilder.new()
  b.append(s)
  b.append("x")
  const result = b.build()
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Empty string `""` | — | Valid, `len() == 0` |
| Out-of-bounds slice `s[0..999]` | S5 | Panic at runtime |
| Slice not on char boundary | S5 | Panic at runtime |
| Embedded `\0` in string | — | Valid; `to_cstring()` returns error |
| Allocation failure | — | Returns `Result` error |
| String literal ≤ 15 bytes | S8 | SSO — inline value, no heap, no refcount |
| String literal > 15 bytes | S6 | Sentinel refcount, never freed/decremented |
| Short string (≤ 15 bytes) | S8 | SSO — pure value copy, no atomic ops |
| `Span` of freed source | — | Undefined behavior (user's responsibility) |
| `Span` out of bounds | — | Panic on `s[view]`, `None` on `s.substr(view)` |
| `StringSlice` with stale handle | — | `pool.get(slice)` returns `None` |
| `StringSlice` wrong pool | — | `pool.get(slice)` returns `None` |
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
let b = StringBuilder.new()
b.append("User: ")
b.append(name)
b.append_char('\n')
const msg = b.build()
```

**Accumulator pattern** — create a new builder per iteration:

<!-- test: skip -->
```rask
func flush_lines(lines: Vec<string>) -> Vec<string> {
    let results = Vec.new()
    for line in lines {
        let b = StringBuilder.new()
        b.append(line)
        b.append_char('\n')
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
        builder.append("<{self.tag}>")
        for child in self.children {
            child.render(builder)
        }
        builder.append("</{self.tag}>")
    }
}
```

**Interpolation in builder** — compiler desugars efficiently:

<!-- test: skip -->
```rask
// These are equivalent, but the compiler optimizes the interpolation
// form to avoid creating a temp string:
builder.append("tag {value}")
// ≈
builder.append("tag ")
builder.append(value.to_string())
```

### Patterns & Guidance

**Basic usage:**

<!-- test: skip -->
```rask
const s1 = "hello"
const s2 = s1  // COPY: both s1 and s2 valid (refcount incremented)

process(s2[0..3])  // passes "hel" as temporary slice

const view = Span(0, 3)
process(s2[view])  // user ensures s2 is still valid
```

**Parsing with StringPool (validated access):**

<!-- test: skip -->
```rask
func tokenize(source: string) -> (StringPool, Vec<Token>) or Error {
    const pool = StringPool.new()
    const source_handle = pool.insert(source)
    const tokens: Vec<Token> = Vec.new()

    for (start, end, kind) in scan(pool[source_handle]) {
        const slice = try pool.slice(source_handle, start, end)
        tokens.push(Token { text: slice, kind })
    }

    return Ok((pool, tokens))
}
```

**Safe character-boundary access:**

<!-- test: skip -->
```rask
const text = "日本語"
for (i, c) in text.char_indices() {
    process(text[i..i+c.len_utf8()])
}
```

### Integration

- `string` implements `Displayable`, `Hashable`, `Comparable` traits. Copy is structural (S1)
- All types (`string`, `Span`, `StringBuilder`, `StringPool`, `StringSlice`) are in core prelude
- String builders can contain linear resources; `build()` consumes builder to preserve linearity
- String literals ≤ 15 bytes produce SSO values (inline, no allocation). Longer literals use static storage with sentinel refcount. Comptime interpolation follows the same rule based on result length

### Implementation Notes (Interpreter)

Current interpreter behavior differs from spec in some areas:

**Trimming returns owned strings:**
- `s.trim()`, `s.trim_start()`, `s.trim_end()` return new `string` instead of expression-scoped slices
- This causes allocation but matches common usage patterns

**Method name aliases:**
- `s.parse()` and `s.parse_int()` both work
- `s.index_of(pat)` is alias for `s.find(pat)`

These will converge to spec behavior in the compiled version.

### See Also

- `mem.borrowing` — Inline access (B2) for strings, block-scoped (B1) for struct fields/arrays
- `mem.pools` — Pool/Handle pattern used by StringPool
- `std.iteration` — General iteration design
- `std.path` — Path type wraps string
