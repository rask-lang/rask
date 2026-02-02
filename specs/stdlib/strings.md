# Solution: String Handling

## The Question
How do strings work? UTF-8 by default? Owned vs. borrowed string types? Interaction with C strings at FFI boundary?

## Decision
Single owned `string` type with UTF-8 validation, expression-scoped slicing for zero-copy reads, plain index-based `string_view` for lightweight stored references, and `StringPool` for validated handle-based access when safety is needed.

## Rationale
Eliminates the reference type problem by making slices ephemeral (expression-only). Stored references come in two flavors: `string_view` (plain indices, zero overhead, user ensures validity) and `StringPool` (handle-based with validation, follows Pool<T> pattern). No runtime tracking overhead on plain strings.

## Specification

### Type Categories

| Type | Description | Ownership | Layout | Storable? |
|------|-------------|-----------|--------|-----------|
| `string` | UTF-8 validated, owned | Move on assignment | (ptr, len, capacity) | Yes |
| `string_view` | Plain indices into a string | Copy (2 words) | (start, end) | Yes |
| `string_builder` | Growable mutable buffer | Move on assignment | (ptr, len, capacity) | Yes |
| `StringPool` | Pool of strings with validated handles | Move on assignment | (see Pool<T>) | Yes |
| `StringSlice` | Handle + indices into StringPool | Copy (4 words) | (handle, start, end) | Yes |
| `cstring` | Null-terminated for C FFI | Move on assignment | (ptr) | Yes (unsafe only) |

**When to use which:**
- `string` — Default for owned text data
- `string_view` — Lightweight stored indices, user ensures source validity (like storing an index into a Vec)
- `StringPool` + `StringSlice` — When you need validated access to stored substrings (parsers, tokenizers)

### API Boundaries (Avoiding Rust's String/&str Problem)

**Public APIs always use `string`:**

| Parameter | Meaning |
|-----------|---------|
| `func foo(s: string)` | Compiler infers borrow mode from usage |
| `func foo(take s: string)` | Explicit ownership transfer |

The compiler infers whether a parameter needs read-only borrow, mutable borrow, or can accept a slice based on how it's used in the function body.

**Never use `string_view` or `StringSlice` in public APIs.** They are internal storage tools.

```rask
// Library defines:
func search(text: string, pattern: string) -> Option<usize>

// All of these work - no conversion needed:
search(my_string, "foo")           // owned strings
search(my_string[10..50], "foo")   // expression slice
search(my_string[view], "foo")     // string_view converted via indexing
```

**Why this avoids Rust's problem:**
- Rust: APIs choose `String` vs `&str`, causing conversion friction
- Rask: APIs always take `string`, caller can pass owned or expression slice
- `string_view`/`StringSlice` are internal—convert to expression slice at call site

### Ownership Rules

| Operation | Behavior |
|-----------|----------|
| `const s2 = s1` | MOVE: `s1` becomes invalid, `s2` owns the data |
| `const s2 = s1.clone()` | CLONE: both valid, visible allocation |
| `func foo(s: string)` | Borrow for call duration (compiler infers read vs mutable) |
| `func foo(take s: string)` | Transfer ownership to callee |

### Expression-Scoped Slicing

Slicing syntax `s[i..j]` creates a temporary view valid ONLY within the expression. Cannot be assigned to variables or stored.

| Context | Example | Valid? |
|---------|---------|--------|
| Function argument | `process(s[0..5])` | ✅ |
| Method receiver | `s[0..5].len()` | ✅ |
| Chained expression | `s[0..5].to_uppercase()` | ✅ |
| Variable assignment | `const x = s[0..5]` | ❌ Compile error |
| Struct field | `Foo { field: s[0..5] }` | ❌ Compile error |
| Return value | `return s[0..5]` | ❌ Compile error |

**Implementation:** Compiler creates stack-local (ptr, len) view, passes to callee as borrowed parameter, invalidates after expression completes.

### Parameter Passing with Slicing

| Declaration | Accepts | What callee receives |
|-------------|---------|---------------------|
| `func foo(s: string)` | `string`, `s[i..j]`, `view.as_slice(src)` | Borrow (compiler infers mode) |
| `func foo(take s: string)` | `string` only | Ownership transfer |

Slicing syntax `s[i..j]` is only valid when passing to borrowed parameters (not `take`).

### The `string_view` Type

Plain indices for lightweight stored references. No validation—user ensures the source string is still valid and unchanged (like storing an index into a Vec).

```rask
// Create view (just stores indices)
const view = string_view(0, 5)

// Access via source string
process(source[view])           // equivalent to source[view.start..view.end]
const sub = source.substr(view)?  // bounds-checked, returns Option
```

| Operation | Return | Notes |
|-----------|--------|-------|
| `string_view(i, j)` | `string_view` | Create view (just start, end indices) |
| `source[view]` | expression-scoped slice | Panics if out of bounds |
| `source.substr(view)` | `Option<expression-scoped slice>` | Safe bounds check |
| `view.to_string(source)` | `string` | Allocates copy (panics if OOB) |
| `view.start`, `view.end` | `usize` | Read indices |
| `view.len()` | `usize` | `end - start` |

**No validation:** `string_view` is just two integers. If the source string is modified or freed, using the view is undefined behavior. For validated access, use `StringPool`.

### The `StringPool` Type

For validated stored references (parsers, tokenizers, ASTs). Follows the `Pool<T>` pattern from dynamic data structures.

```rask
const pool = StringPool.new()

// Insert strings, get handles
const h = pool.insert("hello world")?  // Handle<string>

// Create slices (handle + indices)
const slice = pool.slice(h, 0, 5)?  // StringSlice

// Access - validates handle, then expression-scoped
pool[slice]                      // panics if invalid handle
pool.get(slice)                  // Option<expression-scoped slice>
pool.read(slice, |s| s.len())    // closure pattern

// StringSlice is freely storable
struct Token {
    text: StringSlice,
    kind: TokenKind,
}
```

| Operation | Return | Notes |
|-----------|--------|-------|
| `StringPool.new()` | `StringPool` | Empty pool |
| `pool.insert(s)` | `Result<Handle<string>, InsertError>` | Add string, get handle |
| `pool.slice(h, i, j)` | `Result<StringSlice, Error>` | Create validated slice |
| `pool[slice]` | expression-scoped slice | Panics if invalid |
| `pool.get(slice)` | `Option<expression-scoped slice>` | Safe access |
| `pool.read(slice, \|s\| R)` | `Option<R>` | Closure-based access |
| `pool.remove(h)` | `Option<string>` | Remove and return ownership |

**Handle validation:** Same as `Pool<T>`—pool_id + index + generation. Wrong pool, stale handle, or invalid index returns `None`.

### UTF-8 Validation

Strings MUST contain valid UTF-8. Validation occurs at construction.

| Operation | Return Type | Validation Cost |
|-----------|-------------|-----------------|
| `"literal"` | `string` | Compile-time |
| `string.from_utf8(bytes)` | `result<string, utf8_error>` | Runtime O(n), one-time |
| `string.from_utf8_unchecked(bytes)` | `string` | None (unsafe block only) |

### Byte Slicing and UTF-8 Boundaries

Slicing uses **byte indices**. Slicing mid-codepoint MUST panic at runtime.

| Operation | Return | Notes |
|-----------|--------|-------|
| `s[i..j]` | Expression-scoped slice | Panics if not on char boundaries |
| `s.is_char_boundary(i)` | `bool` | O(1) check |
| `s.char_indices()` | Iterator of `(usize, char)` | Use to find safe boundaries |

### Iteration

Iterators borrow for expression scope only. Cannot be stored.

```rask
// Valid: immediate use
for c in s.chars() { ... }

// Valid: chained
const count = s.chars().filter(is_vowel).count()

// Invalid: cannot store iterator
const iter = s.chars()  // Compile error
```

| Method | Yields | Notes |
|--------|--------|-------|
| `s.chars()` | `char` (u32 Unicode scalar) | Expression-scoped iterator |
| `s.bytes()` | `u8` | Raw byte iterator |
| `s.char_indices()` | `(usize, char)` | Index + char pairs |
| `s.lines()` | Expression-scoped slices | Split on newlines |
| `s.split(pat)` | Expression-scoped slices | Split on pattern |

### String Length and Properties

| Operation | Return | Cost |
|-----------|--------|------|
| `s.len()` | `usize` | O(1), byte length (cached) |
| `s.char_count()` | `usize` | O(n), count Unicode scalars |
| `s.is_empty()` | `bool` | O(1) |
| `s.is_ascii()` | `bool` | O(n) first call, cached |

### String Construction

| Operation | Return Type | Notes |
|-----------|-------------|-------|
| `"literal"` | `string` | Static storage, compile-time validated |
| `string.from_utf8(bytes)` | `result<string, utf8_error>` | Validates bytes |
| `string.from_char(c)` | `string` | Allocates single-char string |
| `string.repeat(s, n)` | `string` | Allocates `s` repeated `n` times |

### String Builder

| Operation | Signature | Notes |
|-----------|-----------|-------|
| `string_builder.new()` | `() -> string_builder` | Empty builder |
| `string_builder.with_capacity(n)` | `(usize) -> string_builder` | Pre-allocate |
| `b.append(s: string)` | `(self)` | Append string/slice |
| `b.append_char(c)` | `(self, c: char)` | Append char |
| `b.build()` | `(self) -> string` | Consume builder, return string |
| `b.clear()` | `(self)` | Clear contents, keep capacity |
| `b.len()` | `(self) -> usize` | Current byte length |

**`build()` consumes the builder.** To reuse: call `clear()` after building.

### Concatenation and Formatting

| Operation | Return | Notes |
|-----------|--------|-------|
| `string.concat(a, b)` | `string` | Allocates new string |
| `format!(template, args...)` | `string` | Macro expands to builder calls, allocates |

**No `+` operator.** Allocation MUST be visible via method name or macro.

### In-Place Mutation

| Operation | Signature | Notes |
|-----------|-----------|-------|
| `s.push_char(c)` | `(self, c: char)` | Append char, may reallocate |
| `s.push_str(other: string)` | `(self)` | Append string/slice |
| `s.truncate(len)` | `(self, len: usize)` | Truncate to `len` bytes |
| `s.clear()` | `(self)` | Clear contents, keep capacity |

### Searching

| Operation | Return | Notes |
|-----------|--------|-------|
| `s.find(pat)` | `option<usize>` | Byte index of first match |
| `s.rfind(pat)` | `option<usize>` | Byte index of last match |
| `s.contains(pat)` | `bool` | Substring check |
| `s.starts_with(pat)` | `bool` | Prefix check |
| `s.ends_with(pat)` | `bool` | Suffix check |

### Trimming

| Operation | Return | Notes |
|-----------|--------|-------|
| `s.trim_bounds()` | `(usize, usize)` | Returns (start, end) indices, O(n) |
| Use with slicing | `s[bounds.0..bounds.1]` | Zero-copy trim via expression slice |

### Case Conversion

| Operation | Return | Notes |
|-----------|--------|-------|
| `s.to_uppercase()` | `string` | Allocates new string |
| `s.to_lowercase()` | `string` | Allocates new string |

### Equality and Comparison

| Operation | Cost | Notes |
|-----------|------|-------|
| `s1 == s2` | O(n) | Byte-wise comparison (length check first) |
| `s1 < s2` | O(n) | Lexicographic comparison |
| `s.hash()` | O(n) | Not cached |

### C Interop

| Type/Operation | Description |
|----------------|-------------|
| `cstring` | Owned null-terminated string |
| `c"literal"` | Null-terminated string literal |
| `s.to_cstring()` | `result<cstring, null_byte_error>` (fails if string contains `\0`) |
| `cstring.as_ptr()` | `*const u8` (unsafe context only) |
| `cstring.from_ptr(ptr)` | `cstring` (unsafe, takes ownership) |
| `cstring.to_string()` | `result<string, utf8_error>` |

**Example:**
```rask
unsafe {
    const c_path = path.to_cstring()?
    const fd = c_open(cstring.as_ptr(c_path), O_RDONLY)
}
```

### Edge Cases

| Case | Handling |
|------|----------|
| Empty string `""` | Valid, `len() == 0` |
| Out-of-bounds slice `s[0..999]` | Panic at runtime |
| Slice not on char boundary | Panic at runtime |
| String with embedded `\0` | Valid in `string`; `to_cstring()` returns error |
| Allocation failure | Returns `Result` error (consistent with collections) |
| String literal moved | Semantic move, memory never freed (static storage) |
| `string_view` of freed/modified source | Undefined behavior (user's responsibility) |
| `string_view` out of bounds | Panic on `s[view]`, `None` on `s.substr(view)` |
| `StringSlice` with stale handle | `pool.get(slice)` returns `None` |
| `StringSlice` wrong pool | `pool.get(slice)` returns `None` |
| Mutation during iteration | Compile error (iterator holds borrow) |
| Multiple simultaneous iterators | Allowed for read-only iteration |

## Examples

### Basic Usage
```rask
// Owned strings
const s1 = "hello"
const s2 = s1  // MOVE: s1 invalid

// Expression slicing (zero-copy)
process(s2[0..3])  // passes "hel" as read borrow

// Plain string_view (no validation)
const view = string_view(0, 3)
process(s2[view])  // user ensures s2 is still valid
```

### Building Strings
```rask
const builder = string_builder.with_capacity(100)
builder.append("User: ")
builder.append(name)
builder.append_char('\n')
const msg = builder.build()
```

### Formatting
```rask
const msg = format!("User {} logged in at {}", name, time)
```

### Parsing with Plain Views (User Manages Validity)
```rask
const line = "field1,field2,field3"
const fields: Vec<string_view> = Vec.new()

for (start, end) in find_field_boundaries(line) {
    fields.push(string_view(start, end))?
}

// Later: access via original string (user ensures line unchanged)
for view in fields.iter() {
    if line[view].starts_with("field") {
        process(line[view])
    }
}
```

### Parsing with StringPool (Validated Access)
```rask
func tokenize(source: string) -> Result<(StringPool, Vec<Token>), Error> {
    const pool = StringPool.new()
    const source_handle = pool.insert(source)?
    const tokens: Vec<Token> = Vec.new()

    for (start, end, kind) in scan(pool[source_handle]) {
        const slice = pool.slice(source_handle, start, end)?
        tokens.push(Token { text: slice, kind })?
    }

    Ok((pool, tokens))
}

// Later: safe access even if token is stored/passed around
func print_token(pool: StringPool, token: Token) {
    match pool.get(token.text) {
        Some(s) => print(s),
        None => print("<invalid>"),
    }
}
```

### Safe Character-Boundary Access
```rask
const text = "日本語"
for (i, c) in text.char_indices() {
    // i is guaranteed safe boundary
    process(text[i..i+c.len_utf8()])
}
```

## Integration Notes

- **Memory model:** Strings are plain value types with no runtime tracking. `StringPool` follows `Pool<T>` pattern for validated access.
- **Dynamic data structures:** `StringPool` uses same handle mechanism as `Pool<T>` (pool_id + index + generation). Allocation returns `Result`.
- **Concurrency:** `string` is sendable (owned value). `string_view` is just indices (user ensures source accessible). `StringSlice` requires its `StringPool` to be accessible.
- **Generics:** `string` implements `Clone`, `Display`, `Hash`, `Ord` traits
- **Error handling:** `from_utf8`, `to_cstring`, pool operations return `Result` or `Option`
- **Linear resources:** String builders can contain linear resources; `build()` must consume builder to preserve linearity
- **Compile-time execution:** String literals and `format!` at comptime produce static strings
- **Module system:** `string`, `string_view`, `string_builder`, `StringPool`, `StringSlice` are in core prelude
- **C interop boundary:** Unsafe blocks required for `cstring.as_ptr()` and raw pointer operations
- **Iteration:** String iteration follows the general iteration design (see [Iteration](iteration.md))