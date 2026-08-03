<!-- id: std.json -->
<!-- status: decided -->
<!-- summary: Untyped JsonValue enum plus zero-ceremony struct encoding/decoding -->
<!-- depends: stdlib/collections.md, types/error-types.md, stdlib/encoding.md -->

# JSON

Two layers: untyped `JsonValue` enum for dynamic JSON, compiler-generated struct encoding/decoding for known schemas.

## Types

| Rule | Description |
|------|-------------|
| **J1: JsonValue** | All JSON values represented as a six-variant enum: Null, Bool, Number, String, Array, Object |
| **J2: f64 numbers** | All JSON numbers stored as `f64`. Integers up to 2^53 are exact; larger lose precision |
| **J3: JsonError** | Parse, type, and missing-field errors reported via `JsonError` enum |

<!-- test: parse -->
```rask
enum JsonValue {
    Null
    Bool(bool)
    Number(f64)
    String(string)
    Array(Vec<JsonValue>)
    Object(Map<string, JsonValue>)
}

enum JsonError {
    ParseError(string)
    TypeError(string)
    MissingField(string)
}
```

## Encoding and Decoding

One verb pair for both layers: `decode` (string → value) and `encode` (value → string), per the serialization convention in canonical-patterns.md. The untyped path is just `decode` with `JsonValue` as the target — no separate `parse`/`stringify` family.

| Rule | Description |
|------|-------------|
| **J4: RFC 8259** | `json.decode` accepts any valid RFC 8259 JSON string |
| **J5: Duplicate keys** | Last value wins (matches JavaScript behavior) |
| **J11: Nesting limit** | Arrays and objects nest at most 256 deep; past that the input is a `ParseError` |

<!-- test: skip -->
```rask
json.decode<JsonValue>(input) -> JsonValue or JsonError    // untyped/dynamic JSON
json.encode(value)                                          // works for JsonValue too
json.encode_pretty(value)
```

## JsonValue Access

| Method | Returns |
|--------|---------|
| `value.is_null()` | `bool` |
| `value.as_bool()` | `bool?` |
| `value.as_number()` | `f64?` |
| `value.as_string()` | `string?` |
| `value.as_array()` | `Vec<JsonValue>?` |
| `value.as_object()` | `Map<string, JsonValue>?` |
| `value["key"]` | `JsonValue?` (object index) |
| `value[index]` | `JsonValue?` (array index) |

## Typed Encoding/Decoding

| Rule | Description |
|------|-------------|
| **J6: Auto-encode** | Any struct satisfying `Encode` can be encoded without manual implementation. Uses `comptime for` + field access (`std.encoding/E1`–`E3`) |
| **J7: Compatible types** | `bool`, `i32`, `i64`, `u32`, `u64`, `f32`, `f64`, `string`, `Vec<T>`, `Map<string, T>`, `T?`, nested structs |
| **J8: Field mapping** | Struct field `serial_name` = JSON key. Defaults to field name (snake_case). Override with `@rename` (`std.encoding/E18`) |
| **J9: Optional fields** | `T?` fields decode `null` or missing as `none`; missing required fields produce `MissingField`. `@default` fields (`std.encoding/E20`) also tolerate missing keys |
| **J10: Extra keys ignored** | JSON keys not matching a serialized struct field are silently skipped. This includes a key naming a field that is excluded from the wire form (`std.encoding/E13b`) — the field is never filled from input |

<!-- test: skip -->
```rask
json.encode(value: T) -> string
json.encode_pretty(value: T) -> string
json.to_value(value: T) -> JsonValue
json.decode<T>(input: string) -> T or JsonError
json.from_value<T>(value: JsonValue) -> T or JsonError
```

<!-- test: skip -->
```rask
import json

struct User {
    name: string
    age: i64
    email: string?
}

const user = try json.decode<User>(input)
const output = json.encode(user)
```

With field annotations:

<!-- test: skip -->
```rask
struct ApiUser {
    @rename("user_name")
    name: string

    age: i64

    @default("user")
    role: string

    @no_serialize
    cache_key: string = ""
}
// encode → {"user_name": "alice", "age": 30, "role": "admin"}
// decode with missing role → role defaults to "user"
```

## Error Messages

```
ERROR [std.json/J3]: missing required field
   |
5  |  const user = try json.decode<User>(body)
   |                   ^^^^^^^^^^^^^^^^^^^^^^^ field "email" not found in JSON object

WHY: Required (non-optional) struct fields must be present in the JSON input.

FIX: Add the field to the JSON, or change the struct field to `email: string?`.
```

```
ERROR [std.json/J7]: incompatible type for JSON encoding
   |
3  |  json.encode(my_struct)
   |              ^^^^^^^^^ field `data` has type `File` which is not JSON-compatible

WHY: Only primitive, collection, optional, and nested-struct types can be encoded.
```

## Edge Cases

| Case | Behavior | Rule |
|------|----------|------|
| `json.decode<JsonValue>("")` | `JsonError.ParseError` | J4 |
| `json.decode<JsonValue>("null")` | `JsonValue.Null` | J4 |
| `json.decode<JsonValue>("123")` | `JsonValue.Number(123.0)` | J2 |
| Large integers (>2^53) | Precision loss in f64 | J2 |
| Duplicate keys in object | Last value wins | J5 |
| JSON has extra keys not in struct | Ignored | J10 |
| Struct has extra fields not in JSON | Required fields error; optional fields get `none` | J9 |
| Number where an integer field is declared | Truncated toward zero | J2 |
| Excluded field on decode | Its declared default, or `@default` | `std.encoding/E13a` |
| Excluded field with no default | Compile error — the type is not `Decode` | `std.encoding/E13a` |
| JSON key naming an excluded field | Ignored, like any unknown key | J10, `std.encoding/E13b` |
| `private` field on encode | Never emitted | `std.encoding/E13` |
| Nesting past 256 levels | `JsonError.ParseError` | J11 |

---

## Appendix (non-normative)

### Rationale

**J2 (f64 numbers):** Matches JavaScript's `JSON.parse()` behavior. Exact large integers would need a `JsonValue.Integer(i64)` variant — deferred until there's a real use case.

**J6 (auto-encode):** Uses `comptime for` over `reflect.fields<T>()` to generate per-field encoding at monomorphization time. No derive macro needed — any struct satisfying `Encode` (`std.encoding/E12`) works automatically.

**J8 (field mapping):** `@rename` (`std.encoding/E18`) overrides the serialized key name. Default is the field name (snake_case). Format-agnostic — works for TOML, MessagePack, etc.

**How decode reaches the type:** the native backend has no reflection at runtime, so the call site describes the target instead — a small tree of field names, byte offsets, and kinds handed to the decoder, which fills the value in one pass. Nesting, lists, and maps recurse in the runtime rather than unrolling at the call site. The interpreter walks the same struct declarations directly. Both are compiler-side; nothing in `stdlib/json.rk` implements `decode`.

**J11 (nesting limit):** the parser recurses, so an input of nothing but `[[[[…` would otherwise walk off the stack. 256 is well past anything a real document nests.

### Deferred

- `json.Parser` — streaming parser for large files
- `JsonValue.Integer(i64)` — lossless integer round-trips
- Date/time handling — dates are strings, parse with `time` module
- `json.to_value` / `json.from_value` — the typed↔`JsonValue` pair; `decode`/`encode` cover the string ends today
- `@default(expr)` with anything but a literal — an arbitrary comptime expression needs CTFE at the field
- Enums as JSON — only structs, collections, and scalars decode

### Resolved (by std.encoding)

- ~~`@json(rename = "fieldName")`~~ → `@rename("fieldName")` — format-agnostic field annotation (`std.encoding/E18`)
- ~~`JsonEncodable` / `JsonDecodable`~~ → `Encode` / `Decode` marker traits (`std.encoding/E11`)

### See Also

- `std.encoding` — Encode/Decode traits, comptime field iteration, field annotations
- `std.collections` — `Vec`, `Map` used in JsonValue
- `type.errors` — `JsonError` follows standard error pattern
