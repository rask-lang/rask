<!-- id: std.fmt -->
<!-- status: decided -->
<!-- summary: String formatting via format(), Displayable/Debug traits, Error auto-bridge, println interpolation -->

# Formatting

`format(template, args...)` with `{}`, `{0}`, `{name}`, `{:spec}` placeholders. `Displayable` and `Debug` traits for type-to-string conversion. `println`/`print` do implicit `{name}` interpolation.

## format() Function

| Rule | Description |
|------|-------------|
| **F1: Signature** | `format(template: string, args...) -> string` |
| **F2: Positional** | `{}` uses next arg; `{0}`, `{1}` use explicit index. Cannot mix auto and explicit |
| **F3: Named** | `{name}` captures variable from scope. Cannot mix with positional |
| **F4: Escape** | `{{` and `}}` produce literal `{` and `}` |

## Format Specifiers

| Rule | Description |
|------|-------------|
| **S1: Grammar** | `{[arg_id][:[[fill]align][width][.precision][type]]}` |
| **S2: Align** | `<` left, `>` right, `^` center; fill char defaults to space |
| **S3: Types** | `?` debug, `x`/`X` hex, `b` binary, `o` octal, `e` scientific |

| Specifier | Example | Result |
|-----------|---------|--------|
| `{}` | `format("{}", 42)` | `"42"` |
| `{:?}` | `format("{:?}", vec)` | `"[1, 2, 3]"` |
| `{:x}` | `format("{:x}", 255)` | `"ff"` |
| `{:X}` | `format("{:X}", 255)` | `"FF"` |
| `{:b}` | `format("{:b}", 10)` | `"1010"` |
| `{:>10}` | `format("{:>10}", "hi")` | `"        hi"` |
| `{:0>10}` | `format("{:0>10}", 42)` | `"0000000042"` |
| `{:.3}` | `format("{:.3}", 3.14159)` | `"3.142"` |

<!-- test: parse -->
```rask
const hex = format("0x{:08X}", 0xDEAD)
const table = format("{:<10} {:>8}", "Name", "Score")
```

## Displayable Trait

| Rule | Description |
|------|-------------|
| **D1: Trait** | `trait Displayable { func to_string(self) -> string }` |
| **D2: Primitives** | All primitive types implement `Displayable` by default |
| **D3: Structs opt-in** | Structs do NOT auto-implement `Displayable` — must add via `extend Type with Displayable` |
| **D4: Required for {}** | `format("{}", x)` calls `to_string()`. Compile error if `Displayable` not implemented |
| **D5: Error bridge** | Types satisfying `Error` (have `message(self) -> string`) auto-satisfy `Displayable` — `to_string()` calls `message()`. No boilerplate needed for error types in `format("{}", err)` |

<!-- test: parse -->
```rask
struct Point { x: f64, y: f64 }

extend Point with Displayable {
    func to_string(self) -> string {
        return format("({}, {})", self.x, self.y)
    }
}
```

**Error types are automatically Displayable (D5):**

<!-- test: parse -->
```rask
enum AppError {
    NotFound(path: string),
    Timeout,
}

extend AppError {
    func message(self) -> string {
        match self {
            NotFound(p) => format("not found: {}", p),
            Timeout => "timed out",
        }
    }
}

// No extend with Displayable needed — Error types get it for free
// format("{}", AppError.Timeout) → "timed out"
```

## Debug Trait

| Rule | Description |
|------|-------------|
| **G1: Trait** | `trait Debug { func debug_string(self) -> string }` |
| **G2: Auto-derive** | All types auto-derive `Debug` by default |
| **G3: Override** | Auto-derived `Debug` can be overridden via `extend Type with Debug` |
| **G4: Debug format** | `format("{:?}", x)` calls `debug_string()` |

<!-- test: parse -->
```rask
struct Point { x: f64, y: f64 }

const p = Point { x: 1.0, y: 2.0 }
println(format("{:?}", p))    // Point { x: 1.0, y: 2.0 }
```

## println / print Interpolation

| Rule | Description |
|------|-------------|
| **I1: Variable capture** | `println("Hello, {name}!")` interpolates `name` from scope |
| **I2: Field access** | `{point.x}` works for dotted field access |
| **I3: No expressions** | `{x + y}` is an error — use `format()` for expressions |

<!-- test: skip -->
```rask
const name = "world"
println("Hello, {name}!")              // Hello, world!

const point = Point { x: 1.0, y: 2.0 }
println("Position: {point.x}, {point.y}")
```

## Error Messages

```
ERROR [std.fmt/D4]: type does not implement Displayable
   |
5  |  println(format("{}", my_struct))
   |                       ^^^^^^^^^ `MyStruct` does not implement Displayable

WHY: {} calls to_string(), which requires the Displayable trait.

FIX 1: Add Displayable implementation:

  extend MyStruct with Displayable {
      func to_string(self) -> string { ... }
  }

FIX 2: If this is an error type, add message() instead (auto-bridges to Displayable):

  extend MyStruct {
      func message(self) -> string { ... }
  }
```

```
ERROR [std.fmt/I3]: expression not supported in string interpolation
   |
3  |  println("{x + y}")
   |           ^^^^^^^ expressions not allowed in interpolation

WHY: println interpolation only supports names and field access.

FIX: Use format():
  println(format("{} + {} = {}", x, y, x + y))
```

```
ERROR [std.fmt/F2]: cannot mix auto and explicit positional args
   |
3  |  format("{} {0}", a, b)
   |          ^^ ^^^ mixed auto ({}) and explicit ({0})

WHY: Mixing auto-index and explicit index is ambiguous.

FIX: Use all auto ({}, {}) or all explicit ({0}, {1}).
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Missing arg for `{2}` | F2 | Compile-time error (when possible) or runtime panic |
| Type without Displayable in `{}` | D4 | Compile-time error |
| Error type in `{}` | D5 | Auto-bridges — calls `message()` |
| `{{` in template | F4 | Literal `{` |
| Empty template | F1 | Returns empty string |
| `format("{:?}", x)` on auto-derived type | G2 | Shows struct fields / enum variants |

## Compiler Mechanism

| Rule | Description |
|------|-------------|
| **CM1: Compiler-known** | `format()` is a compiler-known function, not a regular function. It accepts variable arguments through compiler support (`struct.modules/BF4`), not through a general variadic mechanism |
| **CM2: Template parsing** | The compiler parses the template string at compile time, extracting placeholder positions, names, and format specifiers |
| **CM3: Per-arg type check** | Each argument is type-checked against its placeholder: `{}` requires `Displayable`, `{:?}` requires `Debug`, `{:x}` requires integer type |
| **CM4: Compile-time errors** | Missing arguments, type mismatches, and malformed specifiers are compile-time errors. No runtime formatting failures for static templates |
| **CM5: Codegen** | The compiler generates specialized string-building code per call site. No runtime template parsing for static templates |
| **CM6: Comptime folding** | When all arguments are comptime-known, the result is a static string |

`println` and `print` use the same mechanism for interpolation: the compiler rewrites `println("Hello, {name}!")` into string-building code at compile time.

---

## Appendix (non-normative)

### Rationale

**D3 (structs opt-in):** Auto-deriving Displayable would produce output that looks intentional but isn't. Debug auto-derives because it's for developers. Displayable is for users, so you write it.

**D5 (Error bridge):** Every error type already has `message()` — requiring a separate `to_string()` that just calls `message()` is pure boilerplate. The compiler auto-bridges: if a type has `message(self) -> string`, it satisfies `Displayable` with `to_string()` delegating to `message()`. If you want different Displayable output than the error message, override with an explicit `extend Type with Displayable`.

**I3 (no expressions):** Expressions in string interpolation create hidden complexity. `format()` makes the formatting explicit. Keeps println simple.

### Patterns & Guidance

**Tabular output:**

<!-- test: parse -->
```rask
println(format("{:<20} {:>10} {:>10}", "Item", "Qty", "Price"))
println(format("{:<20} {:>10} {:>10.2}", "Widget", 5, 9.99))
```

**Custom Displayable with hex:**

<!-- test: parse -->
```rask
struct Color { r: u8, g: u8, b: u8 }

extend Color with Displayable {
    func to_string(self) -> string {
        return format("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }
}
```

**string_builder for loops:** `format()` allocates a new string each call. For repeated formatting, use `string_builder`:

<!-- test: skip -->
```rask
const b = string_builder.with_capacity(1024)
for item in items {
    b.append(format("{}: {}\n", item.name, item.value))
}
const report = b.build()
```

### Integration

- `format()` does not return errors. Missing args or type mismatches are compile-time errors or runtime panics.
- `format()` on comptime-known args produces a static string.
- `format()` is pure -- no shared state, safe from any thread.

### See Also

- `std.strings` — `format()` returns a `string`, uses `string_builder` internally
- `type.traits` — Displayable and Debug are standard traits
- `type.errors` — Error types auto-bridge to Displayable (D5)
