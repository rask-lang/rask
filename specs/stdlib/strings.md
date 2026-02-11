<!-- id: std.strings -->
<!-- status: decided -->
<!-- summary: Owned string, expression-scoped slicing, string_view, StringPool, string_builder, C interop -->
<!-- depends: memory/borrowing.md, memory/value-semantics.md, memory/pools.md -->

# String Handling

Single owned `string` type with UTF-8 validation, expression-scoped slicing for zero-copy reads, `string_view` for lightweight stored indices, and `StringPool` for validated handle-based access.

## Type Categories

| Rule | Description |
|------|-------------|
| **S1: Owned string** | `string` is the default. UTF-8 validated, move on assignment |
| **S2: Expression slicing** | `s[i..j]` creates a temporary view valid only within the expression |
| **S3: Public APIs use string** | Never use `string_view` or `StringSlice` in public APIs |
| **S4: UTF-8 required** | Strings must contain valid UTF-8. Validated at construction |
| **S5: Byte indices** | Slicing uses byte indices. Mid-codepoint slice panics at runtime |

| Type | Description | Ownership | Storable? |
|------|-------------|-----------|-----------|
| `string` | UTF-8 validated, owned | Move on assignment | Yes |
| `string_view` | Plain indices into a string | Copy (2 words) | Yes |
| `string_builder` | Growable mutable buffer | Move on assignment | Yes |
| `StringPool` | Pool of strings with validated handles | Move on assignment | Yes |
| `StringSlice` | Handle + indices into StringPool | Copy (4 words) | Yes |
| `cstring` | Null-terminated for C FFI | Move on assignment | Yes (unsafe only) |

## Ownership Rules

| Rule | Description |
|------|-------------|
| **O1: Move on assign** | `const s2 = s1` moves; `s1` becomes invalid |
| **O2: Explicit clone** | `s1.clone()` creates independent copy (visible allocation) |
| **O3: Borrow inferred** | `func foo(s: string)` borrows for call duration (compiler infers mode) |
| **O4: Explicit take** | `func foo(take s: string)` transfers ownership |

## Expression-Scoped Slicing

`s[i..j]` creates a temporary view valid only within the expression (S2). Cannot be assigned, stored, or returned.

| Context | Example | Valid? |
|---------|---------|--------|
| Function argument | `process(s[0..5])` | Yes |
| Method receiver | `s[0..5].len()` | Yes |
| Chained expression | `s[0..5].to_uppercase()` | Yes |
| Variable assignment | `const x = s[0..5]` | Compile error |
| Struct field | `Foo { field: s[0..5] }` | Compile error |
| Return value | `return s[0..5]` | Compile error |

## The `string_view` Type

Plain indices for lightweight stored references. No validation -- user ensures source string validity (like storing a Vec index).

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
| `pool[slice]` | expression-scoped slice | Panics if invalid |
| `pool.get(slice)` | `Option<expression-scoped slice>` | Safe access |
| `pool.read(slice, \|s\| R)` | `Option<R>` | Closure-based access |
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
| `"literal"` | `string` | Static storage, compile-time validated |
| `string.from_utf8(bytes)` | `Result<string, utf8_error>` | Validates bytes |
| `string.from_char(c)` | `string` | Single-char string |
| `string.repeat(s, n)` | `string` | `s` repeated `n` times |
| `slice.to_owned()` | `string` | Convert expression slice to owned (allocates) |

## String Builder

| Operation | Signature | Notes |
|-----------|-----------|-------|
| `string_builder.new()` | `() -> string_builder` | Empty builder |
| `string_builder.with_capacity(n)` | `(usize) -> string_builder` | Pre-allocate |
| `b.append(s: string)` | `(self)` | Append string/slice |
| `b.append_char(c)` | `(self, c: char)` | Append char |
| `b.build()` | `(self) -> string` | Consume builder, return string |
| `b.clear()` | `(self)` | Clear contents, keep capacity |
| `b.len()` | `(self) -> usize` | Current byte length |

`build()` consumes the builder. To reuse: call `clear()` after building.

## Concatenation and Formatting

| Operation | Return | Notes |
|-----------|--------|-------|
| `string.concat(a, b)` | `string` | Allocates new string |
| `"hello {name}"` | `string` | String interpolation, desugars to builder calls, allocates |

No `+` operator. Allocation must be visible via method name or interpolation.

## In-Place Mutation

| Operation | Signature | Notes |
|-----------|-----------|-------|
| `s.push_char(c)` | `(self, c: char)` | Append char, may reallocate |
| `s.push_str(other: string)` | `(self)` | Append string/slice |
| `s.truncate(len)` | `(self, len: usize)` | Truncate to `len` bytes |
| `s.clear()` | `(self)` | Clear contents, keep capacity |

## Searching

| Operation | Return | Notes |
|-----------|--------|-------|
| `s.find(pat)` | `Option<usize>` | Byte index of first match |
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

## Equality and Comparison

| Operation | Cost | Notes |
|-----------|------|-------|
| `s1 == s2` | O(n) | Byte-wise (length check first) |
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
ERROR [std.strings/S2]: cannot store expression-scoped slice
   |
3  |  const x = s[0..5]
   |            ^^^^^^^ expression-scoped slice cannot be assigned

WHY: Slices are temporary views valid only within the expression.

FIX: Copy to owned string, or use string_view for stored indices:

  const x = s[0..5].to_owned()   // allocate copy
  const v = string_view(0, 5)    // store indices
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

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Empty string `""` | — | Valid, `len() == 0` |
| Out-of-bounds slice `s[0..999]` | S5 | Panic at runtime |
| Slice not on char boundary | S5 | Panic at runtime |
| Embedded `\0` in string | — | Valid; `to_cstring()` returns error |
| Allocation failure | — | Returns `Result` error |
| String literal moved | O1 | Semantic move, memory never freed (static storage) |
| `string_view` of freed source | — | Undefined behavior (user's responsibility) |
| `string_view` out of bounds | — | Panic on `s[view]`, `None` on `s.substr(view)` |
| `StringSlice` with stale handle | — | `pool.get(slice)` returns `None` |
| `StringSlice` wrong pool | — | `pool.get(slice)` returns `None` |
| Mutation during iteration | — | Compile error (iterator holds borrow) |
| Multiple simultaneous iterators | — | Allowed for read-only |

---

## Appendix (non-normative)

### Rationale

**S1 (single owned type):** One string type covers the common case. `string_view` and `StringPool` handle the uncommon stored-reference case without polluting the default API.

**S2 (expression-scoped slicing):** The cost is more `.to_owned()` calls. I think that's better than lifetime annotations on string slices leaking into function signatures.

**S3 (public APIs use string):** Forces a clean boundary. Callers never need to know about internal storage strategies.

**S5 (byte indices):** Byte indexing matches the underlying UTF-8 representation. Character indexing would be O(n) and misleading for multi-byte characters.

### Patterns & Guidance

**Basic usage:**

<!-- test: skip -->
```rask
const s1 = "hello"
const s2 = s1  // MOVE: s1 invalid

process(s2[0..3])  // passes "hel" as expression-scoped borrow

const view = string_view(0, 3)
process(s2[view])  // user ensures s2 is still valid
```

**Building strings:**

<!-- test: skip -->
```rask
const builder = string_builder.with_capacity(100)
builder.append("User: ")
builder.append(name)
builder.append_char('\n')
const msg = builder.build()
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
        try tokens.push(Token { text: slice, kind })
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

- `string` implements `Clone`, `Display`, `Hash`, `Ord` traits
- All types (`string`, `string_view`, `string_builder`, `StringPool`, `StringSlice`) are in core prelude
- String builders can contain linear resources; `build()` consumes builder to preserve linearity
- String literals and interpolation at comptime produce static strings

### See Also

- `mem.borrowing` — Expression-scoped vs block-scoped view rules
- `mem.pools` — Pool/Handle pattern used by StringPool
- `std.iteration` — General iteration design
- `std.path` — Path type wraps string
