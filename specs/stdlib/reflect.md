# Reflect Module

## Overview

Compile-time type introspection. `std.reflect` provides functions that inspect types during compilation—struct fields, method signatures, trait implementations, size/alignment. No runtime cost; all reflection resolves at compile time.

I chose a stdlib module over language-level syntax because it keeps the language small. The compiler provides the intrinsics; the stdlib wraps them in a stable API.

## Local Analysis Constraint

Every function in `std.reflect` operates on types already in scope—function parameters, local variables, imported types. You cannot discover types you haven't imported.

**This preserves [Principle 5](../CORE_DESIGN.md) (Local Analysis Only):**
- Reflecting on `MyStruct` works if `MyStruct` is imported or defined locally
- "Find all types implementing Trait X" is not supported—that requires whole-program knowledge
- Reflection results depend only on the type definition, not on how it's used elsewhere

## Usage

All `std.reflect` functions are only callable inside `comptime` blocks or `comptime func`. Using them at runtime is a compile error.

```rask
import std.reflect

const FIELD_COUNT = comptime reflect.fields(MyStruct).len
```

## API

### Type Info

| Function | Signature | Description |
|----------|-----------|-------------|
| `size_of<T>()` | `-> usize` | Size in bytes |
| `align_of<T>()` | `-> usize` | Alignment in bytes |
| `name_of<T>()` | `-> string` | Type name as string (e.g. `"Vec<i32>"`) |
| `is_copy<T>()` | `-> bool` | Whether T is implicitly copyable (≤16 bytes, all fields Copy) |
| `is_resource<T>()` | `-> bool` | Whether T is a linear resource type |

```rask
comptime {
    const size = reflect.size_of<Point>()       // 8
    const align = reflect.align_of<Point>()     // 4
    const copy = reflect.is_copy<Point>()       // true (two i32 = 8 bytes)
}
```

### Struct Fields

| Function | Signature | Description |
|----------|-----------|-------------|
| `fields<T>()` | `-> []FieldInfo` | All fields of a struct (compile error if T is not a struct) |
| `has_field<T>(name: string)` | `-> bool` | Whether struct has a field with this name |

```rask
struct FieldInfo {
    name: string
    type_name: string
    offset: usize
    size: usize
    is_public: bool
}
```

**Example — serialization codegen:**
```rask
import std.reflect

comptime func gen_to_json<T>() -> string {
    const code = string.new()
    code.push_str("func to_json(self: T) -> string {\n")
    code.push_str("    const parts = Vec<string>.new()\n")

    for field in reflect.fields<T>() {
        code.push_str("    parts.push(\"\\\"{field.name}\\\": \" + self.{field.name}.to_string())\n")
    }

    code.push_str("    return \"{\" + parts.join(\", \") + \"}\"\n")
    code.push_str("}\n")
    return code.freeze()
}
```

### Methods

| Function | Signature | Description |
|----------|-----------|-------------|
| `methods<T>()` | `-> []MethodInfo` | All methods of a type |
| `has_method<T>(name: string)` | `-> bool` | Whether type has a method with this name |

```rask
struct MethodInfo {
    name: string
    is_public: bool
    param_count: usize
    return_type_name: string
}
```

### Trait Checking

| Function | Signature | Description |
|----------|-----------|-------------|
| `implements<T, Trait>()` | `-> bool` | Whether T satisfies Trait (structural or explicit) |
| `traits<T>()` | `-> []string` | Names of traits T explicitly extends |

```rask
comptime if reflect.implements<T, Display>() {
    // T has a display() method — generate pretty-print code
}
```

**Note:** `implements` checks whether T has the required methods. It does NOT scan the codebase for all implementors of a trait—that would break local analysis.

### Enum Variants

| Function | Signature | Description |
|----------|-----------|-------------|
| `variants<T>()` | `-> []VariantInfo` | All variants of an enum (compile error if T is not an enum) |

```rask
struct VariantInfo {
    name: string
    has_fields: bool
    field_count: usize
}
```

## Patterns

### Derive-Style Code Generation

The primary use case: generating repetitive implementations from type structure.

```rask
import std.reflect

// In a build script or comptime block, generate Display for any struct
comptime func gen_display<T>() -> string {
    const code = string.new()
    code.push_str("extend {reflect.name_of<T>()} {\n")
    code.push_str("    func display(self) -> string {\n")
    code.push_str("        const parts = Vec<string>.new()\n")

    for field in reflect.fields<T>() {
        code.push_str("        parts.push(\"{field.name}: \" + self.{field.name}.to_string())\n")
    }

    code.push_str("        return \"{reflect.name_of<T>()}(\" + parts.join(\", \") + \")\"\n")
    code.push_str("    }\n")
    code.push_str("}\n")
    return code.freeze()
}
```

### Comptime Assertions on Type Shape

```rask
comptime func assert_serializable<T>() {
    for field in reflect.fields<T>() {
        @comptime_assert(
            reflect.is_copy<T>() || reflect.has_method<T>("to_string"),
            "Field '{field.name}' of {reflect.name_of<T>()} is not serializable"
        )
    }
}
```

### Conditional Logic Based on Type Properties

```rask
func serialize<T>(value: T) -> []u8 {
    comptime if reflect.is_copy<T>() && reflect.size_of<T>() <= 8 {
        // Fast path: memcpy for small copy types
        return unsafe { mem_as_bytes(value) }
    } else {
        // General path: field-by-field
        return serialize_fields(value)
    }
}
```

## What This Does NOT Do

- **No runtime reflection.** All `std.reflect` calls resolve at compile time. There's no `reflect.fields(some_runtime_value)`.
- **No type discovery.** You cannot ask "what types implement Serializable?" That requires whole-program analysis.
- **No type modification.** You cannot add fields or methods to existing types through reflection.
- **No private field access.** Reflection shows private fields exist (name, type, size) but cannot bypass visibility for access. Code generation using field names still respects visibility rules.

## Integration Notes

- **Comptime:** All reflection functions are comptime-only. Using them outside `comptime` is a compile error.
- **Generics:** Reflection on generic types reflects the concrete monomorphized type, not the generic template.
- **Local Analysis:** Reflection results depend only on the type definition. Changing code elsewhere cannot change what `reflect.fields<T>()` returns.
- **IDE:** Ghost annotations should show reflected values on hover (e.g., hovering `reflect.fields<Point>()` shows `[{name: "x", ...}, {name: "y", ...}]`).
