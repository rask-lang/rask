<!-- id: std.reflect -->
<!-- status: decided -->
<!-- summary: Compile-time type introspection via stdlib module -->
<!-- depends: control/comptime.md, stdlib/encoding.md -->

# Reflect Module

Compile-time type introspection through `std.reflect`. All reflection resolves at compile time with zero runtime cost.

## Core Rules

| Rule | Description |
|------|-------------|
| **R1: Comptime only** | All `std.reflect` functions require `comptime` context. Runtime use is a compile error |
| **R2: Local analysis** | Reflection operates on types already in scope. No whole-program type discovery |
| **R3: No mutation** | Cannot add fields or methods to existing types through reflection |
| **R4: Visibility respected** | Reflection shows private fields exist (name, type, size) but generated code respects visibility |
| **R5: Concrete types** | Reflection on generic types reflects the monomorphized type, not the generic template |

<!-- test: skip -->
```rask
import std.reflect

const FIELD_COUNT = comptime reflect.fields(MyStruct).len
```

## Type Info

| Function | Signature | Description |
|----------|-----------|-------------|
| `size_of<T>()` | `-> usize` | Size in bytes |
| `align_of<T>()` | `-> usize` | Alignment in bytes |
| `name_of<T>()` | `-> string` | Type name as string (e.g. `"Vec<i32>"`) |
| `is_copy<T>()` | `-> bool` | Whether T is implicitly copyable (≤16 bytes, all fields Copy) |
| `is_resource<T>()` | `-> bool` | Whether T is a linear resource type |

### Type Category

| Function | Signature | Description |
|----------|-----------|-------------|
| `is_struct<T>()` | `-> bool` | Whether T is a struct |
| `is_enum<T>()` | `-> bool` | Whether T is an enum |
| `is_optional<T>()` | `-> bool` | Whether T is `U?` for some U |
| `is_vec<T>()` | `-> bool` | Whether T is `Vec<U>` for some U |
| `is_map<T>()` | `-> bool` | Whether T is `Map<K, V>` for some K, V |
| `is_integer<T>()` | `-> bool` | Whether T is an integer type (`i8`–`i64`, `u8`–`u64`, `usize`) |
| `is_float<T>()` | `-> bool` | Whether T is `f32` or `f64` |
| `is_flat<T>()` | `-> bool` | Whether T has no heap-backed fields recursively (no `string`, `Vec`, `Map`, `Cell`, `Shared`, `Mutex`, `any Trait`, closures, resources) |

These enable comptime type dispatch without string-comparing type names. Primary use cases: format libraries (`std.encoding`), relocatable memory (`mem.relocatable`).

<!-- test: skip -->
```rask
comptime {
    const size = reflect.size_of<Point>()       // 8
    const align = reflect.align_of<Point>()     // 4
    const copy = reflect.is_copy<Point>()       // true (two i32 = 8 bytes)

    const yes = reflect.is_struct<Point>()      // true
    const no = reflect.is_enum<Point>()         // false
}
```

## Struct Fields

| Function | Signature | Description |
|----------|-----------|-------------|
| `fields<T>()` | `-> []FieldInfo` | All fields of a struct (compile error if not a struct) |
| `has_field<T>(name: string)` | `-> bool` | Whether struct has a field with this name |

<!-- test: parse -->
```rask
struct FieldInfo {
    name: string
    type_name: string
    offset: usize
    size: usize
    is_public: bool
    serial_name: string       // @rename value, or same as name
    is_skipped: bool          // @skip present
    has_default: bool         // @default present
}
```

`serial_name` equals `name` unless the field has `@rename("...")`. See `std.encoding` for field annotation semantics.

## Methods

| Function | Signature | Description |
|----------|-----------|-------------|
| `methods<T>()` | `-> []MethodInfo` | All methods of a type |
| `has_method<T>(name: string)` | `-> bool` | Whether type has a method with this name |

<!-- test: parse -->
```rask
struct MethodInfo {
    name: string
    is_public: bool
    param_count: usize
    return_type_name: string
}
```

## Trait Checking

| Function | Signature | Description |
|----------|-----------|-------------|
| `implements<T, Trait>()` | `-> bool` | Whether T satisfies Trait (structural or explicit) |
| `traits<T>()` | `-> []string` | Names of traits T explicitly extends |

`implements` checks whether T has the required methods. Does NOT scan the codebase for all implementors (R2).

## Enum Variants

| Function | Signature | Description |
|----------|-----------|-------------|
| `variants<T>()` | `-> []VariantInfo` | All variants of an enum (compile error if not an enum) |

<!-- test: parse -->
```rask
struct VariantInfo {
    name: string
    has_fields: bool
    field_count: usize
    fields: []FieldInfo       // payload fields (empty for unit variants)
    serial_name: string       // @rename value, or same as name
}
```

## Error Messages

```
ERROR [std.reflect/R1]: reflect function used outside comptime context
   |
5  |  const fields = reflect.fields<Point>()
   |                 ^^^^^^^^^^^^^^^^^^^^^^^^ reflect requires comptime

WHY: Reflection resolves at compile time. No runtime introspection.

FIX: Wrap in comptime block:

  const fields = comptime reflect.fields<Point>()
```

```
ERROR [std.reflect/R2]: cannot discover types not in scope
   |
3  |  const impls = reflect.implementors<Displayable>()
   |                ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ whole-program query

WHY: Reflection operates on imported types only. Type discovery requires whole-program analysis.
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Reflect on non-struct with `fields<T>()` | — | Compile error |
| Reflect on non-enum with `variants<T>()` | — | Compile error |
| Private fields in `fields<T>()` | R4 | Visible in metadata, access respects visibility |
| Generic type `T` in comptime func | R5 | Reflects concrete monomorphized type |
| `implements<T, Trait>()` | R2 | Checks T's methods, not codebase-wide |

---

## Appendix (non-normative)

### Rationale

**R1 (comptime only):** No runtime reflection keeps binaries small and avoids the metadata bloat of languages like Java/C#.

**R2 (local analysis):** I chose a stdlib module over language-level syntax because it keeps the language small. The compiler provides the intrinsics; the stdlib wraps them in a stable API. "Find all types implementing Trait X" would require whole-program knowledge, breaking local analysis (`CORE_DESIGN.md` Principle 5).

### Patterns & Guidance

**Comptime field iteration** — the primary use case. Uses `comptime for` + field access (`std.encoding/E1`–`E3`):

<!-- test: skip -->
```rask
import std.reflect

func debug_print<T>(value: T) {
    print("{reflect.name_of<T>()}(")
    comptime for field in reflect.fields<T>() {
        print("  {field.name}: {value.(field.name)}")
    }
    print(")")
}
```

**Comptime assertions on type shape:**

<!-- test: skip -->
```rask
comptime func assert_all_public<T>() {
    for field in reflect.fields<T>() {
        @comptime_assert(
            field.is_public,
            "Field '{field.name}' of {reflect.name_of<T>()} must be public"
        )
    }
}
```

**Conditional logic based on type category:**

<!-- test: skip -->
```rask
func encode_value<T: Encode>(value: T, w: mutate Writer) -> void or Error {
    comptime if reflect.is_struct<T>() {
        comptime for field in reflect.fields<T>() {
            try encode_value(value.(field.name), mutate w)
        }
    } else if reflect.is_optional<T>() {
        if value? as v { try encode_value(v, mutate w) }
    }
}
```

### IDE Integration

Ghost annotations show reflected values on hover (e.g., hovering `reflect.fields<Point>()` shows `[{name: "x", ...}, {name: "y", ...}]`).

### See Also

- `ctrl.comptime` — Compile-time execution context
- `std.encoding` — Comptime field iteration and serialization (`std.encoding/E1`–`E3`)
- `type.traits` — Trait definitions and structural typing
- `type.structs` — Struct field layout and visibility
- `mem.relocatable` — Flat type constraint, `is_flat<T>()` usage (`mem.relocatable/FL4`)
