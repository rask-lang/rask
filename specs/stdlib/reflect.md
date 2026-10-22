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

<!-- surface -->
```rask
extend reflect {
    func size_of<T>() -> usize
    func align_of<T>() -> usize
    func name_of<T>() -> string
    func is_copy<T>() -> bool
    func is_resource<T>() -> bool
}
```

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
| `is_flat<T>()` | `-> bool` | Whether T has no heap-backed fields recursively (no `string`, `Vec`, `Map`, `Shared`, `any Interface`, closures, resources) |

<!-- surface -->
```rask
extend reflect {
    func is_struct<T>() -> bool
    func is_enum<T>() -> bool
    func is_optional<T>() -> bool
    func is_vec<T>() -> bool
    func is_map<T>() -> bool
    func is_integer<T>() -> bool
    func is_float<T>() -> bool
    func is_flat<T>() -> bool
}
```

These enable comptime type dispatch without string-comparing type names. Primary use cases: format libraries (`std.encoding`), relocatable memory (`mem.relocatable`).

`name_of`, `is_struct`, `is_enum`, `is_optional`, `is_vec`, `is_map`, `is_integer`, `is_float`, `is_resource` and `is_flat` fold to constants on both backends, and so does everything in the sections below.

`size_of` and `align_of` are the size and alignment the value is laid out with: each field at its own width, reordered most-aligned first (`@layout(C)` keeps declaration order), padded to its alignment. That's the same size the 16-byte Copy threshold measures, so `is_copy<T>()` and `size_of<T>() <= 16` never contradict each other. `struct { a: u8, b: i64, c: u8 }` is 16 bytes, not 10 and not 24.

`is_resource` and `is_flat` needed no layout at all — they ask about the *declaration*, and a layout has dropped `@resource` and substituted its field types by the time it exists. Native reads the checker's type table and the interpreter reads its AST maps; one shared classifier turns either into the same answer. One case still reports: `is_flat<Boxed<i32>>()` on a generic type, because R5 wants the monomorphized type and the declaration's fields are written in its type parameters.

```rask
struct Point {
    x: i32
    y: i32
}

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
| `fields<T>()` | `-> Vec<FieldInfo>` | All fields of a struct (compile error if not a struct) |
| `has_field<T>(name: string)` | `-> bool` | Whether struct has a field with this name. False for anything that isn't a struct |

<!-- surface -->
```rask
extend reflect {
    func fields<T>() -> Vec<FieldInfo>
    func has_field<T>(name: string) -> bool
}

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

`has_field` is a predicate, so it answers rather than refuses: an enum's payload isn't a field and a primitive has none, and both say false. `fields<T>()` on a non-struct stays a compile error, because there's no list to hand back.

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
| `methods<T>()` | `-> Vec<MethodInfo>` | Every method a call can reach on T |
| `has_method<T>(name: string)` | `-> bool` | Whether `methods<T>()` has one with this name |

<!-- surface -->
```rask
extend reflect {
    func methods<T>() -> Vec<MethodInfo>
    func has_method<T>(name: string) -> bool
}

struct MethodInfo {
    name: string
    is_public: bool
    param_count: usize
    return_type_name: string
}
```

"All methods" means everything `t.name(…)` or `T.name(…)` can call, because that's what a caller asking `has_method` wants to know:

| Included | Notes |
|---|---|
| Methods in `extend T` blocks | Static ones too. `param_count` never counts `self` |
| Methods in conformance blocks | Under the name a call writes. `Duration` conforms to `Div<i64>` and `Div<Duration>`, so it has two `div`s, told apart by `return_type_name` |
| What the compiler derives | `eq`, `hash`, `clone`, `compare`, `lt`/`le`/`gt`/`ge`, `debug`, and an enum's `message` (`type.errors/ER6`) — they're as callable as any other |
| Methods that aren't `public` | Listed with `is_public == false` (R4). A conformance method is as visible as the conformance, so it's public |

Order is the declaration order, then what was derived.

R5 decides generic receivers: a method whose `extend` header or `where` clause this instantiation doesn't meet isn't one of its methods. `Vec<i64>` has `sort` and a `Vec` of a struct that isn't `Comparable` doesn't; `read_u8` lives in `extend Vec<u8>` and only `Vec<u8>` has it. `return_type_name` has the receiver's arguments substituted. A method's own type parameters stay as written: `map` on `Vec<i64>` returns `Sequence<U>`.

The `name` argument of `has_method` and `has_field` is normally a literal, or `field.name` inside a `comptime for`, and the call folds to a constant. Any other string works too; it's compared at run time against the folded list.

## Interface Checking

| Function | Signature | Description |
|----------|-----------|-------------|
| `implements<T, Interface>()` | `-> bool` | Whether `T: Interface` would hold as a generic bound |
| `interfaces<T>()` | `-> Vec<string>` | The conformances written for T. Name-only — unlike `fields`/`methods`/`variants` there's no Info struct |

<!-- surface -->
```rask
extend reflect {
    func implements<T, Interface>() -> bool
    func interfaces<T>() -> Vec<string>
}
```

`implements` asks the question a bound asks, and gets the same answer: a written conformance, a derived one (`Equal`, `Hashable`, `Cloneable`, `Comparable`, `Debug`), or one the compiler answers itself (`i64: Comparable`, `Vec<i64>: Equal`, `Copy`, `Encode`). `Interface` has to name an interface, and naming anything else is a compile error. It does NOT scan the codebase for all implementors (R2).

`interfaces` is narrower on purpose: it lists what the source *wrote*, `T implements I` blocks and a nominal type's `with` list, in the order written. I left the derived conformances out. They depend on the fields rather than on anything the author said, and `Debug` would sit on every list as noise; `implements` answers for them. Each entry is the applied interface, defaults and `Self` filled in: `Duration` lists `Add<Duration>, Sub<Duration>, Mul<i64>, Div<i64>, Div<Duration>`. A bare `Div` twice would lose which two. A sub-interface is listed and its parents aren't, though `implements` is true for them. A conditional conformance (`where T: …`) is listed when this instantiation meets the condition (R5).

## Enum Variants

| Function | Signature | Description |
|----------|-----------|-------------|
| `variants<T>()` | `-> Vec<VariantInfo>` | All variants of an enum (compile error if not an enum) |

<!-- surface -->
```rask
extend reflect {
    func variants<T>() -> Vec<VariantInfo>
}

struct VariantInfo {
    name: string
    has_fields: bool
    field_count: usize
    fields: Vec<FieldInfo>       // payload fields (empty for unit variants)
    serial_name: string       // @rename value, or same as name
}
```

A payload field is a `FieldInfo` like a struct field's, read from the same layout. A positional payload is named by its index (`_0`, `_1`). `offset` counts from the start of the enum value, the same origin a struct field's does, so it lands after the tag. A payload has no visibility or wire annotations of its own: `is_public` is true and `serial_name` is the field's name. A generic enum reports the instantiation: `Answer<string>`'s `Yes` holds a 16-byte `string` where `Answer<i64>`'s holds 8.

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
| `implements<T, Interface>()` | R2 | Checks T against the interface, not codebase-wide |
| `implements<T, NotAnInterface>()` | — | Compile error |
| `has_field<T>()` on a non-struct | — | `false` |
| `methods<T>()` on a generic instantiation | R5 | Only the methods whose `extend` header and `where` clause T meets |
| `interfaces<T>()` with derived conformances | — | Not listed; `implements` answers |

---

## Appendix (non-normative)

### Rationale

**R1 (comptime only):** No runtime reflection keeps binaries small and avoids the metadata bloat of languages like Java/C#.

**R2 (local analysis):** I chose a stdlib module over language-level syntax because it keeps the language small. The compiler provides the intrinsics; the stdlib wraps them in a stable API. "Find all types implementing Interface X" would require whole-program knowledge, breaking local analysis (`CORE_DESIGN.md` Principle 5).

### Patterns & Guidance

**Comptime field iteration** — the primary use case. Uses `comptime for` + field access (`ctrl.comptime/CT48`–`CT54`):

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
            field.is_public
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
- `std.encoding` — Comptime field iteration and serialization
- `type.interfaces` — Interface definitions and structural typing
- `type.structs` — Struct field layout and visibility
- `mem.relocatable` — Flat type constraint, `is_flat<T>()` usage (`mem.relocatable/FL4`)
