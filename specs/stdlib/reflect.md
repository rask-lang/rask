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
| **R4: Visibility respected** | Reflection shows private fields exist (name, type, size) but generated code respects visibility. Auto-derived conformances act as if generated in the defining module — see [below](#reflection-and-auto-derived-conformances-r4) |
| **R5: Concrete types** | Reflection on generic types reflects the monomorphized type, not the generic template |

<!-- test: skip -->
```rask
import std.reflect

const FIELD_COUNT = comptime reflect.fields<MyStruct>().len
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

`name_of`, `is_struct`, `is_enum`, `is_optional`, `is_vec`, `is_map`, `is_integer`, `is_float`, `is_resource` and `is_flat` fold to constants on both backends.

`size_of`, `align_of` and `is_copy` don't. They need a size, and the compiler has two of them: the language model behind the 16-byte Copy threshold (`i32` is 4 bytes, which is what the example below counts with) and the 8-byte-slot model codegen lays out with, where every scalar takes a word. They disagree about every struct with a narrow field, so answering with either would bake the choice in before it's made. Both backends report it rather than guessing (#791).

`is_resource` and `is_flat` needed no layout at all — they ask about the *declaration*, and a layout has dropped `@resource` and substituted its field types by the time it exists. Native reads the checker's type table and the interpreter reads its AST maps; one shared classifier turns either into the same answer. One case still reports: `is_flat<Boxed<i32>>()` on a generic type, because R5 wants the monomorphized type and the declaration's fields are written in its type parameters.

<!-- test: skip -->
```rask
comptime {
    let size = reflect.size_of<Point>()       // 8
    let align = reflect.align_of<Point>()     // 4
    let copy = reflect.is_copy<Point>()       // true (two i32 = 8 bytes)

    let yes = reflect.is_struct<Point>()      // true
    let no = reflect.is_enum<Point>()         // false
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
    is_private: bool
    serial_name: string       // @rename value, or same as name
    serialized: bool          // part of the wire form (std.encoding/E13)
    has_default: bool         // @default or a declared default present
}
```

`serial_name` equals `name` unless the field has `@rename("...")`. See `std.encoding` for field annotation semantics.

`serialized` is the **inclusion decision, already made** — false for a `private` field or one marked `@no_serialize`, true otherwise. Format libraries read the one flag instead of re-deriving the rule from `is_private` and the annotations; when `std.encoding`'s policy changes, they don't.

### Reflection and Auto-Derived Conformances [R4]

A format library is an external package, so `reflect.fields<T>()` called from inside it would see less than `Encode` covers — the package-default fields of someone else's type aren't visible to it as *code*.

The rule that resolves this: **an auto-derived conformance acts as if it were generated in the module that defines the type.** `Encode`/`Decode` are the compiler's own derivation, not library code reaching in, so they see the full `serialized` field set (`std.encoding/E13`) and the `FieldInfo` list handed to a format library reflects that set. What the format library cannot do is reach *past* it: a `private` field has `serialized == false`, and no amount of reflection from outside makes it readable.

So there are two different questions and they have different answers:

| Question | Answer |
|---|---|
| Can this package's code read the field? | Normal visibility rules (R4) |
| Is the field on the wire? | `FieldInfo.serialized` (`std.encoding/E13`) |

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
| `trait_names<T>()` | `-> []string` | Names of traits T explicitly extends. Name-only — unlike `fields`/`methods`/`variants` there's no Info struct |

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
5  |  let fields = reflect.fields<Point>()
   |                 ^^^^^^^^^^^^^^^^^^^^^^^^ reflect requires comptime

WHY: Reflection resolves at compile time. No runtime introspection.

FIX: Wrap in comptime block:

  let fields = comptime reflect.fields<Point>()
```

```
ERROR [std.reflect/R2]: cannot discover types not in scope
   |
3  |  let impls = reflect.implementors<Displayable>()
   |                ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ whole-program query

WHY: Reflection operates on imported types only. Type discovery requires whole-program analysis.
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Reflect on non-struct with `fields<T>()` | — | Compile error |
| Reflect on non-enum with `variants<T>()` | — | Compile error |
| Private fields in `fields<T>()` | R4 | Visible in metadata, access respects visibility; `serialized == false` |
| `fields<T>()` from an external format library | R4 | Sees the full `serialized` set — auto-derive acts as-if in the defining module |
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
