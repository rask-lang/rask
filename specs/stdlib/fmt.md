<!-- id: std.fmt -->
<!-- status: decided -->
<!-- summary: String formatting via format(), Display/Debug traits, println interpolation -->

# Formatting

`format(template, args...)` with `{}`, `{0}`, `{name}`, `{:spec}` placeholders. `Display` and `Debug` traits for type-to-string conversion. `println`/`print` do implicit `{name}` interpolation.

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

## Display Trait

| Rule | Description |
|------|-------------|
| **D1: Trait** | `trait Display { func to_string(self) -> string }` |
| **D2: Primitives** | All primitive types implement `Display` by default |
| **D3: Structs opt-in** | Structs do NOT auto-implement `Display` — must add via `extend Type with Display` |
| **D4: Required for {}** | `format("{}", x)` calls `to_string()`. Compile error if `Display` not implemented |

<!-- test: parse -->
```rask
struct Point { x: f64, y: f64 }

extend Point with Display {
    func to_string(self) -> string {
        return format("({}, {})", self.x, self.y)
    }
}
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
ERROR [std.fmt/D4]: type does not implement Display
   |
5  |  println(format("{}", my_struct))
   |                       ^^^^^^^^^ `MyStruct` does not implement Display

WHY: {} calls to_string(), which requires the Display trait.

FIX: Add Display implementation:
  extend MyStruct with Display {
      func to_string(self) -> string { ... }
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
| Type without Display in `{}` | D4 | Compile-time error |
| `{{` in template | F4 | Literal `{` |
| Empty template | F1 | Returns empty string |
| `format("{:?}", x)` on auto-derived type | G2 | Shows struct fields / enum variants |

---

## Appendix (non-normative)

### Rationale

**D3 (structs opt-in):** Auto-deriving Display would produce output that looks intentional but isn't. Debug auto-derives because it's for developers. Display is for users, so you write it.

**I3 (no expressions):** Expressions in string interpolation create hidden complexity. `format()` makes the formatting explicit. Keeps println simple.

### Patterns & Guidance

**Tabular output:**

<!-- test: parse -->
```rask
println(format("{:<20} {:>10} {:>10}", "Item", "Qty", "Price"))
println(format("{:<20} {:>10} {:>10.2}", "Widget", 5, 9.99))
```

**Custom Display with hex:**

<!-- test: parse -->
```rask
struct Color { r: u8, g: u8, b: u8 }

extend Color with Display {
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
- `type.traits` — Display and Debug are standard traits
