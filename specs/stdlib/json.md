<!-- id: std.json -->
<!-- status: decided -->
<!-- summary: Untyped JsonValue enum plus zero-ceremony struct encoding/decoding -->
<!-- depends: stdlib/collections.md, types/error-types.md -->

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

## Parsing and Serialization

| Rule | Description |
|------|-------------|
| **J4: RFC 8259** | `json.parse` accepts any valid RFC 8259 JSON string |
| **J5: Duplicate keys** | Last value wins (matches JavaScript behavior) |

<!-- test: skip -->
```rask
json.parse(input: string) -> JsonValue or JsonError
json.stringify(value: JsonValue) -> string
json.stringify_pretty(value: JsonValue) -> string
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
| **J6: Auto-encode** | Any struct with JSON-compatible fields can be encoded without manual implementation |
| **J7: Compatible types** | `bool`, `i32`, `i64`, `u32`, `u64`, `f32`, `f64`, `string`, `Vec<T>`, `Map<string, T>`, `T?`, nested structs |
| **J8: Field mapping** | Struct field name = JSON key (snake_case preserved) |
| **J9: Optional fields** | `T?` fields decode `null` or missing as `None`; missing required fields produce `MissingField` |
| **J10: Extra keys ignored** | JSON keys not matching any struct field are silently skipped |

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
| `json.parse("")` | `JsonError.ParseError` | J4 |
| `json.parse("null")` | `JsonValue.Null` | J4 |
| `json.parse("123")` | `JsonValue.Number(123.0)` | J2 |
| Large integers (>2^53) | Precision loss in f64 | J2 |
| Duplicate keys in object | Last value wins | J5 |
| JSON has extra keys not in struct | Ignored | J10 |
| Struct has extra fields not in JSON | Required fields error; optional fields get `None` | J9 |

---

## Appendix (non-normative)

### Rationale

**J2 (f64 numbers):** Matches JavaScript's `JSON.parse()` behavior. Exact large integers would need a `JsonValue.Integer(i64)` variant — deferred until there's a real use case.

**J6 (auto-encode):** Compiler generates conversion code for any struct with compatible fields. No derive macro or trait implementation needed.

**J8 (snake_case):** Field renaming attributes (`@json(rename = "fieldName")`) deferred. Snake_case is the default; most JSON APIs use it.

### Deferred

- `@json(rename = "fieldName")` — field renaming attributes
- `JsonEncodable` / `JsonDecodable` — custom serialization traits
- `json.Parser` — streaming parser for large files
- `JsonValue.Integer(i64)` — lossless integer round-trips
- Date/time handling — dates are strings, parse with `time` module

### See Also

- `std.collections` — `Vec`, `Map` used in JsonValue
- `type.errors` — `JsonError` follows standard error pattern
