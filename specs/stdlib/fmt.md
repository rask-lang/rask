# Formatting — format(), Display, Debug

## The Question

How does string formatting work in Rask? How do programs build formatted output for logging, debugging, and display?

## Decision

- `format(template, args...)` as a builtin function (not a macro -- macros are not specified yet)
- When macros are added later, `format!(template, args...)` can become syntax sugar for the function
- Standard placeholder syntax: `{}`, `{0}`, `{name}`, `{:spec}`
- Format specifiers follow industry conventions (Rust/Python/C#)
- `Display` and `Debug` traits for type-to-string conversion
- `println`/`print` do implicit interpolation of `{name}` in string arguments

## Rationale

**Why a function, not a macro?**
- Macros are not specified yet. A builtin function works today in the interpreter.
- When macros land, `format!(...)` becomes zero-cost sugar over the same semantics.
- No compile-time string parsing needed -- runtime is fine for now.

**Why `{name}` implicit interpolation in println?**
- Named placeholders leverage existing string interpolation (already works in println).
- Covers the 80% case without importing `format()` for simple debug prints.

**Why Rust-style format specifiers?**
- Proven, widely known syntax. No reason to invent something new.
- Familiar to anyone coming from Rust, Python, or C#.

**Why separate Display and Debug?**
- `Display` is the user-facing representation (clean output).
- `Debug` shows structure (field names, enum variants) -- essential for debugging.
- Matches Rust's proven model. Primitives get both for free.

**Why separate from strings.md?**
- `strings.md` covers the string data type, ownership, and slicing.
- Formatting is output-focused: converting values to text for display.
- `string_builder` handles low-level concatenation; `format()` handles structured templates.

## Specification

### format() Function

```rask
format(template: string, args...) -> string
```

Builds a string by replacing placeholders in `template` with formatted arguments.

**Placeholder types:**

| Syntax | Meaning |
|--------|---------|
| `{}` | Next positional arg, Display |
| `{0}`, `{1}` | Explicit positional arg |
| `{name}` | Variable `name` from scope |
| `{:spec}` | Next arg with format specifier |
| `{0:spec}` | Positional arg with specifier |
| `{name:spec}` | Named arg with specifier |
| `{{`, `}}` | Literal `{` and `}` |

Positional and named placeholders cannot be mixed in one template. Positional auto-indexing (`{}`) and explicit indexing (`{0}`) cannot be mixed either.

### Format Specifiers

| Specifier | Description | Example |
|-----------|-------------|---------|
| `{}` | Display (default) | `format("{}", 42)` -> `"42"` |
| `{:?}` | Debug representation | `format("{:?}", vec)` -> `"[1, 2, 3]"` |
| `{:x}` | Hex lowercase | `format("{:x}", 255)` -> `"ff"` |
| `{:X}` | Hex uppercase | `format("{:X}", 255)` -> `"FF"` |
| `{:b}` | Binary | `format("{:b}", 10)` -> `"1010"` |
| `{:o}` | Octal | `format("{:o}", 8)` -> `"10"` |
| `{:e}` | Scientific notation | `format("{:e}", 1000.0)` -> `"1e3"` |
| `{:>10}` | Right-align, width 10 | `format("{:>10}", "hi")` -> `"        hi"` |
| `{:<10}` | Left-align, width 10 | `format("{:<10}", "hi")` -> `"hi        "` |
| `{:^10}` | Center, width 10 | `format("{:^10}", "hi")` -> `"    hi    "` |
| `{:0>10}` | Zero-pad, right-align | `format("{:0>10}", 42)` -> `"0000000042"` |
| `{:.3}` | Precision (floats) | `format("{:.3}", 3.14159)` -> `"3.142"` |

**Full spec grammar:**

```
{[arg_id][:[[fill]align][width][.precision][type]]}
```

- `arg_id` -- positional index or name
- `fill` -- any single character (default space)
- `align` -- `<` (left), `>` (right), `^` (center)
- `width` -- integer minimum width
- `precision` -- `.` followed by integer
- `type` -- `?` (debug), `x`/`X` (hex), `b` (binary), `o` (octal), `e` (scientific)

### Display Trait

```rask
trait Display {
    func to_string(self) -> string
}
```

All primitive types (`i32`, `i64`, `f64`, `bool`, `string`, `char`, etc.) implement `Display` by default. Structs do NOT auto-implement `Display` -- you must add it:

```rask
struct Point { x: f64, y: f64 }

extend Point with Display {
    func to_string(self) -> string {
        return format("({}, {})", self.x, self.y)
    }
}
```

When `format()` encounters `{}`, it calls `to_string()` on the argument. If the type does not implement `Display`, the compiler reports an error.

### Debug Trait

```rask
trait Debug {
    func debug_string(self) -> string
}
```

All types auto-derive `Debug` by default. The auto-derived output shows struct fields and enum variants:

```rask
struct Point { x: f64, y: f64 }

const p = Point { x: 1.0, y: 2.0 }
println(format("{:?}", p))    // Point { x: 1.0, y: 2.0 }
```

Auto-derived `Debug` can be overridden:

```rask
extend Point with Debug {
    func debug_string(self) -> string {
        return format("P({}, {})", self.x, self.y)
    }
}
```

When `format()` encounters `{:?}`, it calls `debug_string()` on the argument.

### println / print Interpolation

`println` and `print` perform implicit variable interpolation on string arguments:

```rask
const name = "world"
println("Hello, {name}!")              // Hello, world!
```

Dotted field access works:

```rask
const point = Point { x: 1.0, y: 2.0 }
println("Position: {point.x}, {point.y}")
```

Only simple names and field access are supported. Expressions inside `{}` are not allowed:

```rask
println("{x + y}")     // ERROR: expressions not supported, use format()
```

For format specifiers or complex expressions, use `format()`:

```rask
println(format("{:08x}", value))
println(format("{} + {} = {}", x, y, x + y))
```

### Relationship to string_builder

`format()` allocates a new string. For repeated formatting in a loop, prefer `string_builder`:

```rask
const b = string_builder.with_capacity(1024)
for item in items.iter() {
    b.append(format("{}: {}\n", item.name, item.value))
}
const report = b.build()
```

This keeps allocation visible and predictable per Rask's transparency principle.

## Examples

### Basic Formatting

```rask
const msg = format("Hello, {}!", "world")
const hex = format("0x{:08X}", 0xDEAD)
const table = format("{:<10} {:>8}", "Name", "Score")
```

### Named Placeholders

```rask
const name = "Alice"
const age = 30
println("Name: {name}, Age: {age}")

// Equivalent using format():
const msg = format("Name: {name}, Age: {age}")
```

### Debug Output

```rask
const items = Vec.from([1, 2, 3])
println(format("items = {:?}", items))    // items = [1, 2, 3]

const map = Map.from([("a", 1), ("b", 2)])
println(format("map = {:?}", map))        // map = {"a": 1, "b": 2}
```

### Tabular Output

```rask
println(format("{:<20} {:>10} {:>10}", "Item", "Qty", "Price"))
println(format("{:<20} {:>10} {:>10.2}", "Widget", 5, 9.99))
println(format("{:<20} {:>10} {:>10.2}", "Gadget", 12, 24.50))
```

### Custom Display

```rask
struct Color { r: u8, g: u8, b: u8 }

extend Color with Display {
    func to_string(self) -> string {
        return format("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }
}

const red = Color { r: 255, g: 0, b: 0 }
println(format("Color: {}", red))    // Color: #FF0000
```

## Integration Notes

- **strings.md:** `format()` returns a `string`. Uses `string_builder` internally.
- **Error handling:** `format()` does not return errors. Missing args or type mismatches are compile-time errors (when possible) or panics at runtime.
- **Compile-time execution:** `format()` on comptime-known args produces a static string.
- **Concurrency:** `format()` is pure -- no shared state, safe to call from any thread.
- **Future:** When macros land, `format!(...)` can validate the template at compile time and eliminate runtime parsing overhead.

## Status

| Feature | Interpreter | Type Checker |
|---------|-------------|--------------|
| `format()` with `{}` | Planned | -- |
| Named `{name}` in println | Done | -- |
| Format specifiers | Planned | -- |
| Display trait | Not yet | Not yet |
| Debug trait | Not yet | Not yet |

**Specified** -- ready for implementation in interpreter and type system.
