<!-- id: std.strings -->
<!-- status: decided -->
<!-- summary: Immutable refcounted string (Copy), inline slicing, string_builder for construction -->
<!-- depends: memory/borrowing.md, memory/value-semantics.md, memory/pools.md -->

# String Handling

Immutable refcounted `string` type with UTF-8 validation, inline slicing for zero-copy expression access, `string_view` for lightweight stored indices, `StringPool` for validated handle-based access, and `string_builder` for construction.

## Type Categories

| Rule | Description |
|------|-------------|
| **S1: Immutable, refcounted, Copy** | `string` is UTF-8, immutable, 16 bytes `(header_ptr, len)`. Header: `(refcount: atomic_u32, capacity: u32, data: [u8])`. Under VS1 threshold → implicit Copy |
| **S2: Inline slicing** | `s[i..j]` creates a temporary view valid only within the expression |
| **S3: Public APIs use string** | Never use `string_view` or `StringSlice` in public APIs |
| **S4: UTF-8 required** | Strings must contain valid UTF-8. Validated at construction |
| **S5: Byte indices** | Slicing uses byte indices. Mid-codepoint slice panics at runtime |
| **S6: Refcount semantics** | Atomic refcount in heap header. Literals use sentinel refcount (never freed/decremented). Compiler may skip atomic ops for provably sole-owner strings |
| **S7: Builder for mutation** | `push_str`, `push_char`, `truncate`, `clear` live on `string_builder` only. `string` has no mutation methods |

| Type | Description | Ownership | Storable? |
|------|-------------|-----------|-----------|
| `string` | UTF-8 immutable, refcounted | Copy (16 bytes) | Yes |
| `string_view` | Plain indices into a string | Copy (2 words) | Yes |
| `string_builder` | Growable mutable buffer | Move on assignment | Yes |
| `StringPool` | Pool of strings with validated handles | Move on assignment | Yes |
| `StringSlice` | Handle + indices into StringPool | Copy (4 words) | Yes |
| `cstring` | Null-terminated for C FFI | Move on assignment | Yes (unsafe only) |

## Ownership Rules

| Rule | Description |
|------|-------------|
| **O1: Copy on assign** | `const s2 = s1` copies 16-byte header + atomic increment. Both remain valid |
| **O2: Borrow inferred** | `func foo(s: string)` borrows for call duration. No refcount change |
| **O3: Explicit take** | `func foo(take s: string)` transfers ownership, decrements caller's count |

> `string` is Copy. No `.clone()` needed — assignment copies the 16-byte header and bumps the refcount. This is one of the few types that owns heap memory but is still Copy, because the immutable + refcounted design makes sharing safe.

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

## The `string_view` Type

Plain indices for lightweight stored references — the span type for parsers, tokenizers, and diagnostics. No validation — user ensures source string validity (like storing a Vec index). 16 bytes, copy-eligible.

| Operation | Return | Notes |
|-----------|--------|-------|
| `string_view(i, j)` | `string_view` | Create view (just start, end indices) |
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
| `"literal"` | `string` | Static storage, compile-time validated, sentinel refcount (never freed) |
| `string.from_utf8(bytes)` | `Result<string, utf8_error>` | Validates bytes |
| `string.from_char(c)` | `string` | Single-char string |
| `string.repeat(s, n)` or `s.repeat(n)` | `string` | `s` repeated `n` times, allocates |
| `slice.to_string()` | `string` | Copy slice bytes into new independent string (allocates) |

## String Builder

`string_builder` is the sole owner of its buffer — mutation is always O(1) amortized. `string` has no mutation methods.

| Operation | Signature | Notes |
|-----------|-----------|-------|
| `string_builder.new()` | `() -> string_builder` | Empty builder |
| `string_builder.with_capacity(n)` | `(usize) -> string_builder` | Pre-allocate |
| `b.append(s)` | `(self, s: string)` | Append string/slice |
| `b.append_char(c)` | `(self, c: char)` | Append char |
| `b.build()` | `(take self) -> string` | Consume builder, return string |
| `b.build_and_reset()` | `(self) -> string` | Return built string, reset to empty. Zero-copy buffer handoff |
| `b.clear()` | `(self)` | Clear contents, keep capacity |
| `b.len()` | `(self) -> usize` | Current byte length |

`build()` consumes the builder. `build_and_reset()` hands off the internal buffer to the new string and gives the builder a fresh allocation — for use with `mutate` parameters or accumulator loops where consuming isn't possible.

**Interpolation optimization:** `builder.append("hello {name}")` — compiler desugars interpolation directly into builder appends, avoiding temp string allocation.

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
| `s1 == s2` | O(1) or O(n) | Pointer+length fast path: same backing buffer and same length → equal without byte comparison. Otherwise byte-wise (length check first) |
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

  const v = string_view(0, 5)    // store indices, resolve later
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
ERROR [std.strings/S3]: string_view in public API
   |
3  |  public func parse(s: string_view) -> Token
   |                       ^^^^^^^^^^^ use string instead

WHY: Public APIs always use string. string_view is an internal storage tool.

FIX: Accept string and let the compiler infer borrow mode:

  public func parse(s: string) -> Token
```

```
ERROR [std.strings/S7]: cannot mutate string
   |
3  |  s.push_str("x")
   |    ^^^^^^^^ string is immutable

WHY: Use string_builder for construction.

FIX:
  let b = string_builder.new()
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
| String literal | S6 | Sentinel refcount, never freed/decremented |
| `string_view` of freed source | — | Undefined behavior (user's responsibility) |
| `string_view` out of bounds | — | Panic on `s[view]`, `None` on `s.substr(view)` |
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

This is one of the few cases where a type owns heap memory but is still Copy. The immutable + refcounted design makes sharing safe — there's no aliased mutation to worry about. The 16-byte `(header_ptr, len)` representation fits under the VS1 threshold.

**S2 (inline slicing):** Strings own heap buffers — they're growable sources, same as Vec. Slices are temporary views for the expression. `.to_string()` copies bytes into a new independent string — no shared backing. A 50-byte slice must not silently retain a 10MB source buffer. The `.to_string()` calls are honest cost markers bounded by the slice size, not the source size.

**S3 (public APIs use string):** Forces a clean boundary. Callers never need to know about internal storage strategies.

**S5 (byte indices):** Byte indexing matches the underlying UTF-8 representation. Character indexing would be O(n) and misleading for multi-byte characters.

**S6 (refcount semantics):** Atomic refcount enables safe sharing across tasks. Sentinel refcount for literals avoids overhead on the most common case. Compiler optimization for provably sole-owner strings eliminates atomic ops when sharing can't happen.

**S7 (builder for mutation):** All mutation lives on `string_builder`. This means `string` is truly immutable — no COW surprise, no hidden cost. The builder is always the sole owner of its buffer, so mutation is always O(1) amortized.

### Why Immutable Strings?

Three models were evaluated with concrete impact across ~5,000 lines of validation programs:

**Status quo (owned, mutable, move semantics):** 60+ `.clone()` calls. Half eliminable with O3 borrow inference, but ~30 genuinely needed for lock scope reads, divergent use, and parser state flush. Each clone is O(n).

**COW (copy-on-write, shared buffer):** O(1) clone — just bump the refcount. But mutation is O(n) when shared, O(1) when unique. The cost depends on sharing state established elsewhere — non-local reasoning that Rask exists to prevent. Violates transparency of cost.

**Immutable + refcount (this design):** O(1) copy (16-byte header + atomic increment). No hidden costs. Builder is sole owner so mutation is always O(1). Eliminates all `.clone()` on strings.

The grep_clone validation program (string-heavy CLI tool) had zero `.clone()` calls even under the status quo — but that's because it was carefully structured. The immutable design means you don't need careful structuring; strings just copy freely like in Go.

**Why not COW?** The call site looks identical for O(1) and O(n) mutation. The cost depends on how many other references exist — invisible at the mutation site. This is exactly the kind of hidden cost Rask exists to prevent.

**Why refcount, not GC?** Deterministic cleanup. Fits the ownership model. No pauses.

**Why Copy despite owning heap memory?** Normally heap-owning types move. But `string` is immutable — there's no aliased mutation risk. The refcount makes sharing safe. And at 16 bytes, it fits under the Copy threshold. This is a principled exception, not a hack.

### Builder Patterns

**Basic construction:**

<!-- test: skip -->
```rask
let b = string_builder.new()
b.append("User: ")
b.append(name)
b.append_char('\n')
const msg = b.build()
```

**Accumulator pattern** — `build_and_reset()` for flush-text style loops:

<!-- test: skip -->
```rask
func flush_lines(lines: Vec<string>, mutate builder: string_builder) -> Vec<string> {
    let results = Vec.new()
    for line in lines {
        builder.append(line)
        builder.append_char('\n')
        results.push(builder.build_and_reset())
    }
    return results
}
```

**Rendering trait pattern:**

<!-- test: skip -->
```rask
trait Renderable {
    func render(self, mutate builder: string_builder)
}

extend HtmlTag: Renderable {
    func render(self, mutate builder: string_builder) {
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

const view = string_view(0, 3)
process(s2[view])  // user ensures s2 is still valid
```

**Parsing with StringPool (validated access):**

<!-- test: skip -->
```rask
func tokenize(source: string) -> (StringPool, Vec<Token>) or Error {
    const pool = StringPool.new()
    const source_handle = try pool.insert(source)
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
- All types (`string`, `string_view`, `string_builder`, `StringPool`, `StringSlice`) are in core prelude
- String builders can contain linear resources; `build()` consumes builder to preserve linearity
- String literals and interpolation at comptime produce static strings (sentinel refcount)

### Implementation Notes (Interpreter)

Current interpreter behavior differs from spec in some areas:

**Trimming returns owned strings:**
- `s.trim()`, `s.trim_start()`, `s.trim_end()` return new `string` instead of expression-scoped slices
- This causes allocation but matches common usage patterns

**Method name aliases:**
- `s.push(c)` and `s.push_char(c)` both work (on builder)
- `s.parse()` and `s.parse_int()` both work
- `s.index_of(pat)` is alias for `s.find(pat)`

These will converge to spec behavior in the compiled version.

### See Also

- `mem.borrowing` — Inline access (B2) for strings, block-scoped (B1) for struct fields/arrays
- `mem.pools` — Pool/Handle pattern used by StringPool
- `std.iteration` — General iteration design
- `std.path` — Path type wraps string
