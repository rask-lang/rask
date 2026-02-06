# JSON — Parsing and Serialization

## The Question

How does JSON parsing and serialization work? How do we convert between JSON and Rask structs without proc macros or a derive ecosystem?

## Decision

Built-in `json` module with two layers: untyped `JsonValue` enum for dynamic JSON, and zero-ceremony struct encoding/decoding where the compiler auto-generates conversion code.

## Rationale

**Why built-in instead of Rust's serde approach?**
- In Rust, JSON requires a 3-crate dependency chain: `serde` (framework) + `serde_json` (format) + `serde_derive` (proc macro). This is powerful and extensible, but it's heavy for "I just want to parse some JSON"
- JSON is the lingua franca of web APIs. A systems language targeting web services needs it in stdlib
- The compiler already knows struct field names and types — it can auto-generate encode/decode without macros

**Why `JsonValue` enum instead of just typed encode/decode?**
- Not all JSON has a known schema. APIs return dynamic shapes, config files vary
- `JsonValue` lets you explore unknown JSON: `data["users"][0]["name"]`
- Typed encode/decode is sugar on top of `JsonValue`, not a replacement

**Why auto-encode/decode without attributes?**
- If a struct has all JSON-compatible fields, it should just work: `json.encode(my_struct)`
- No `#[derive(Serialize, Deserialize)]`, no `#[serde(rename = "...")]`, no `use serde::Serialize`
- Field name = JSON key. Struct nesting = JSON nesting. It's the obvious mapping
- `@json(rename = "fieldName")` available for the 10% case (deferred)

**Why `json.encode` / `json.decode` instead of trait methods?**
- `json.encode(value)` reads as "convert this to JSON" — the module makes the format explicit
- Avoids polluting every struct with `.to_json()` methods
- If you want trait-based, implement `JsonEncodable` explicitly

## Specification

### Types

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
    ParseError(string)         // malformed JSON
    TypeError(string)          // wrong type (expected string, got number)
    MissingField(string)       // required field not in object
}
```

### Parsing — String to JsonValue

```rask
json.parse(input: string) -> JsonValue or JsonError
```

Parses an RFC 8259 JSON string into a `JsonValue` tree.

### Serialization — JsonValue to String

```rask
json.stringify(value: JsonValue) -> string            // compact, no whitespace
json.stringify_pretty(value: JsonValue) -> string     // indented with 2 spaces
```

### JsonValue Access Methods

```rask
value.is_null() -> bool
value.as_bool() -> bool?
value.as_number() -> f64?
value.as_string() -> string?
value.as_array() -> Vec<JsonValue>?
value.as_object() -> Map<string, JsonValue>?
```

### JsonValue Indexing

```rask
value["key"]    // index by string key (for objects), returns JsonValue?
value[index]    // index by integer (for arrays), returns JsonValue?
```

### JsonValue Construction

```rask
// Enum constructors
JsonValue.Null
JsonValue.Bool(true)
JsonValue.Number(42.0)
JsonValue.String("hello")
JsonValue.Array(vec)
JsonValue.Object(map)
```

### Typed Encoding — Struct to JSON String

```rask
json.encode(value: T) -> string                    // struct -> JSON string
json.encode_pretty(value: T) -> string             // struct -> pretty JSON string
json.to_value(value: T) -> JsonValue               // struct -> JsonValue tree
```

Works for any struct where all fields are JSON-compatible types:
- Primitives: `bool`, `i32`, `i64`, `u32`, `u64`, `f32`, `f64`, `string`
- Collections: `Vec<T>`, `Map<string, T>` where T is JSON-compatible
- Optionals: `T?` — `None` becomes `null` (or field omitted)
- Nested structs: recursively encoded as JSON objects

### Typed Decoding — JSON String to Struct

```rask
json.decode<T>(input: string) -> T or JsonError    // JSON string -> struct
json.from_value<T>(value: JsonValue) -> T or JsonError  // JsonValue -> struct
```

Field mapping:
- Struct field name = JSON object key (snake_case preserved)
- Missing field with type `T?` → `None`
- Missing required field → `JsonError.MissingField`
- Wrong type → `JsonError.TypeError`

### Access Pattern

```rask
import json

// Untyped — explore unknown JSON
const data = try json.parse(input)
match data {
    JsonValue.Object(obj) => {
        const name = obj["name"]?.as_string() ?? "unknown"
        println("Name: {name}")
    }
    _ => println("Expected object")
}

// Typed — known schema
struct User {
    name: string
    age: i64
    email: string?
}

const user = try json.decode<User>(input)
println("Hello, {user.name}")

const output = json.encode(user)
```

## Examples

### HTTP API Server — JSON Request/Response

```rask
import json

struct CreateUserRequest {
    name: string
    email: string
}

struct UserResponse {
    id: i64
    name: string
    email: string
    created: bool
}

func handle_create_user(body: string) -> string or JsonError {
    const req = try json.decode<CreateUserRequest>(body)

    const resp = UserResponse {
        id: 42,
        name: req.name,
        email: req.email,
        created: true,
    }

    return json.encode(resp)
}
```

### Config File Parsing

```rask
import json
import fs

struct Config {
    host: string
    port: i64
    workers: i64?
    debug: bool
}

func load_config(path: string) -> Config or string {
    const content = try fs.read_file(path)
    const config = json.decode<Config>(content).map_err(|e| e.to_string())
    return try config
}
```

### Dynamic JSON Manipulation

```rask
import json

func add_timestamp(input: string) -> string or JsonError {
    const value = try json.parse(input)
    const obj = value.as_object() ?? return Err(JsonError.TypeError("expected object"))

    obj["timestamp"] = JsonValue.Number(1234567890.0)

    return json.stringify(JsonValue.Object(obj))
}
```

### Building JSON from Scratch

```rask
import json

func build_response(status: string, data: Vec<string>) -> string {
    const items = Vec.new()
    for item in data {
        try items.push(JsonValue.String(item))
    }

    const obj = Map.new()
    obj["status"] = JsonValue.String(status)
    obj["count"] = JsonValue.Number(data.len().to_float())
    obj["items"] = JsonValue.Array(items)

    return json.stringify(JsonValue.Object(obj))
}
```

## Edge Cases

- `json.parse("")` → `JsonError.ParseError`
- `json.parse("null")` → `JsonValue.Null`
- `json.parse("123")` → `JsonValue.Number(123.0)` (all JSON numbers are f64)
- Large integers (>2^53) lose precision when stored as f64 — this is a JSON limitation
- Duplicate keys: last value wins (matches JavaScript behavior)
- `json.decode<T>` where T has extra fields not in JSON: extra fields get default/zero values
- `json.decode<T>` where JSON has extra keys not in T: extra keys are ignored

## JSON Number Precision

All JSON numbers are stored as `f64`. This means:
- Integers up to 2^53 (9,007,199,254,740,992) are exact
- Larger integers lose precision
- This matches JavaScript's `JSON.parse()` behavior

If exact large integer handling is needed, use `json.parse()` and extract number strings manually (future: `JsonValue.NumberStr(string)` variant).

## Deferred

- **Field renaming**: `@json(rename = "fieldName")` attribute for non-snake_case JSON keys
- **Custom serialization**: `JsonEncodable` / `JsonDecodable` traits for manual control
- **Streaming parser**: `json.Parser` for large files without loading into memory
- **Integer preservation**: `JsonValue.Integer(i64)` variant for lossless integer round-trips
- **Date/time**: No special handling — dates are strings, parse with `time` module

## References

- specs/stdlib/collections.md — `Vec`, `Map` used in JsonValue
- specs/types/error-types.md — `JsonError` follows standard error pattern
- examples/http_api_server.rask — Primary consumer of json module

## Status

**Specified** — ready for implementation in interpreter.
