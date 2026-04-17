<!-- id: std.encoding -->
<!-- status: decided -->
<!-- summary: Comptime field iteration and auto-derived Encode/Decode for format-agnostic serialization -->
<!-- depends: control/comptime.md, stdlib/reflect.md, types/generics.md, types/traits.md -->

# Encoding

Two language primitives — `comptime for` over struct fields and comptime field access — plus auto-derived `Encode`/`Decode` traits. Format libraries (JSON, TOML, MessagePack) use these to serialize any compatible struct with zero user ceremony.

## Core Mechanism

Encoding uses `comptime for` and comptime field access — see `ctrl.comptime/CT48–CT54` for the full rules. In brief:

- `comptime for field in reflect.fields<T>()` unrolls at compile time, each iteration monomorphized per-field type
- `value.(field.name)` accesses a field by comptime-known string, resolving to direct field access
- `comptime if` inside the loop body enables per-field conditional code generation

Visibility rules apply: private fields are accessible only in the defining module.

<!-- test: skip -->
```rask
import std.reflect

func print_fields<T>(value: T) {
    comptime for field in reflect.fields<T>() {
        print("{field.name} = {value.(field.name)}")
    }
}

struct Point { public x: f64, public y: f64 }

// print_fields(Point { x: 1.0, y: 2.0 }) unrolls to:
//   print("x = {value.x}")
//   print("y = {value.y}")
```

## Encode and Decode Traits

| Rule | Description |
|------|-------------|
| **E11: Marker traits** | `Encode` and `Decode` are marker traits with no methods. They signal that a type's structure is serialization-compatible |
| **E12: Auto-derive** | The compiler auto-derives `Encode` for any struct where all public fields have `Encode` types, unless the struct is marked `@no_encode`. Same for `Decode` |
| **E13: Public fields only** | Auto-derived encoding covers public fields only. Non-public fields are invisible to external serialization code (`std.reflect/R4`) |
| **E14: Base types** | `bool`, `i8`–`i64`, `u8`–`u64`, `f32`, `f64`, `string` are `Encode` and `Decode` |
| **E15: Collection types** | `Vec<T>` where `T: Encode`, `Map<string, T>` where `T: Encode`, `T?` where `T: Encode` — all auto-implement `Encode`. Same for `Decode` |
| **E16: Opt-out** | `@no_encode` on a struct prevents auto-derive of `Encode`. `@no_decode` prevents `Decode`. For types where automatic serialization is semantically wrong |
| **E17: Enum auto-derive** | Enums auto-derive `Encode`/`Decode` when all variant payloads are `Encode`/`Decode` types |

<!-- test: parse -->
```rask
trait Encode { }
trait Decode { }
```

<!-- test: skip -->
```rask
struct User {
    public name: string
    public age: i32
    public email: string?
}
// Auto-derives Encode and Decode — all public fields are compatible types

@no_encode
struct InternalState {
    public data: Vec<u8>
    secret: string
}
// Opted out — won't satisfy Encode bound
```

### Generic Bounds

<!-- test: skip -->
```rask
func send_json<T: Encode>(endpoint: string, value: T) -> () or HttpError {
    const body = json.encode(value)
    return http.post(endpoint, body)
}

func load_config<T: Decode>(path: string) -> T or ConfigError {
    const text = try fs.read_string(path)
    return try toml.decode<T>(text)
}
```

### Manual Implementation

For types where auto-derive doesn't apply (private fields, custom invariants), implement encoding in the same module where the type is defined:

<!-- test: skip -->
```rask
@no_encode
struct DateTime {
    year: i32
    month: u8
    day: u8
    hour: u8
    minute: u8
    second: u8
}

// In the same module — can access private fields
extend DateTime {
    public func to_json(self) -> JsonValue {
        return JsonValue.String(self.to_iso8601())
    }

    public func from_json(value: JsonValue) -> DateTime or JsonError {
        const s = value.as_string() is Some else {
            return Err(JsonError.TypeError("expected string for DateTime"))
        }
        return try DateTime.parse_iso8601(s)
    }
}
```

## Field Annotations

| Rule | Description |
|------|-------------|
| **E18: @rename** | `@rename("name")` on a struct field overrides the serialized key name. Reflected as `FieldInfo.serial_name` |
| **E19: @skip** | `@skip` excludes a field from serialization and deserialization. Reflected as `FieldInfo.is_skipped` |
| **E20: @default** | `@default(expr)` provides a comptime value used when the field is missing during deserialization. The field becomes optional in the input. Reflected as `FieldInfo.has_default` |
| **E21: Comptime expressions** | `@rename` takes a string literal. `@default` takes a comptime expression. Both validated at compile time |
| **E28: @skip zero values** | `@skip` without `@default` requires a type with a known zero value. During decode, skipped fields are initialized to this value |

**Types with known zero values (E28):**

| Type | Zero value |
|------|-----------|
| `bool` | `false` |
| `i8`–`i64`, `u8`–`u64` | `0` |
| `f32`, `f64` | `0.0` |
| `string` | `""` (empty) |
| `T?` (optionals) | `None` |
| `Vec<T>` | empty vec |
| `Map<K,V>` | empty map |
| Structs, enums, other types | **No zero value** — `@skip` requires `@default(expr)` or compile error |

<!-- test: skip -->
```rask
struct ApiUser {
    @rename("user_name")
    public name: string

    @skip
    public cache_key: string

    @default(0)
    public login_count: i32

    @default("unknown")
    public role: string
}

// JSON: {"user_name": "alice", "login_count": 5, "role": "admin"}
// Missing login_count → defaults to 0
// Missing role → defaults to "unknown"
// cache_key never appears in output, never expected in input
```

### @skip and Auto-Derive

A `@skip` field doesn't need to be `Encode`/`Decode`. A struct with a non-serializable private field that's also `@skip` still auto-derives `Encode`:

<!-- test: skip -->
```rask
struct CachedUser {
    public name: string
    public age: i32

    @skip
    internal_id: u64          // Not Encode, but skipped — doesn't block auto-derive
}
// CachedUser: Encode — the compiler ignores @skip fields for auto-derive eligibility
```

## Enum Serialization

| Rule | Description |
|------|-------------|
| **E22: External tagging** | Default: variant name is the key. `{"Circle": {"radius": 1.0}}` for struct payloads, `"Point"` for unit variants |
| **E23: Single payload** | Variants with one unnamed field: `{"Circle": 1.0}` — payload directly as the value |
| **E24: Internal tagging** | `@tag("field")` on the enum: tag is a field inside the object. `{"type": "Circle", "radius": 1.0}` |
| **E25: Variant rename** | `@rename` on individual variants overrides the serialized variant name |

<!-- test: skip -->
```rask
enum Shape {
    Circle { radius: f64 }
    Rectangle { width: f64, height: f64 }
    Point
}
// External (default):
//   Circle    → {"Circle": {"radius": 1.0}}
//   Rectangle → {"Rectangle": {"width": 2.0, "height": 3.0}}
//   Point     → "Point"
```

<!-- test: skip -->
```rask
@tag("type")
enum Event {
    Click { x: i32, y: i32 }

    @rename("key_press")
    KeyPress { code: u32 }
}
// Internal:
//   Click    → {"type": "Click", "x": 10, "y": 20}
//   KeyPress → {"type": "key_press", "code": 65}
```

## Format Library Pattern

Format libraries use `comptime for` + `reflect` to implement encoding generically. Each format is self-contained.

### Encoding

<!-- test: skip -->
```rask
import std.reflect

// Type dispatch — monomorphizes per concrete type
func encode_value<T: Encode>(value: T, w: mutate JsonWriter) -> () or JsonError {
    comptime if T == bool {
        return w.write_bool(value)
    } else if T == string {
        return w.write_string(value)
    } else if reflect.is_integer<T>() {
        return w.write_number(value as f64)
    } else if reflect.is_float<T>() {
        return w.write_number(value)
    } else if reflect.is_optional<T>() {
        if value is Some(v) {
            return encode_value(v, mutate w)
        } else {
            return w.write_null()
        }
    } else if reflect.is_vec<T>() {
        try w.begin_array()
        for item in value {
            try encode_value(item, mutate w)
        }
        return w.end_array()
    } else if reflect.is_map<T>() {
        try w.begin_object()
        for key, val in value {
            try w.key(key)
            try encode_value(val, mutate w)
        }
        return w.end_object()
    } else if reflect.is_struct<T>() {
        try w.begin_object()
        comptime for field in reflect.fields<T>() {
            comptime if !field.is_skipped {
                try w.key(field.serial_name)
                try encode_value(value.(field.name), mutate w)
            }
        }
        return w.end_object()
    } else if reflect.is_enum<T>() {
        return encode_enum(value, mutate w)
    }
}

// Top-level entry point
public func encode<T: Encode>(value: T) -> string or JsonError {
    mut w = JsonWriter.new()
    try encode_value(value, mutate w)
    return w.finish()
}
```

### Decoding

<!-- test: skip -->
```rask
func decode_value<T: Decode>(parser: mutate JsonParser) -> T or JsonError {
    comptime if T == bool {
        return parser.read_bool()
    } else if T == string {
        return parser.read_string()
    } else if reflect.is_integer<T>() {
        const n = try parser.read_number()
        return n as T
    } else if reflect.is_optional<T>() {
        if parser.peek_null() {
            parser.skip()
            return None
        }
        return Some(try decode_value(parser))
    } else if reflect.is_vec<T>() {
        mut result = Vec.new()
        try parser.begin_array()
        while !parser.end_array() {
            result.push(try decode_value(parser))
        }
        return result
    } else if reflect.is_struct<T>() {
        return decode_struct<T>(mutate parser)
    }
}

func decode_struct<T: Decode>(parser: mutate JsonParser) -> T or JsonError {
    try parser.begin_object()
    mut fields = Map<string, JsonValue>.new()
    while !parser.end_object() {
        const key = try parser.read_key()
        fields.insert(key, try parser.read_value())
    }

    return T {
        comptime for field in reflect.fields<T>() {
            comptime if !field.is_skipped {
                (field.name): comptime if field.has_default {
                    match fields.get(field.serial_name) {
                        Some(v) => try decode_from_value(v),
                        None => field.default_value,
                    }
                } else {
                    try decode_from_value(
                        fields.get(field.serial_name) is Some else {
                            return Err(JsonError.MissingField(field.serial_name))
                        }
                    )
                },
            }
        }
    }
}
```

### Struct Literal Construction

| Rule | Description |
|------|-------------|
| **E26: Comptime for in struct literal** | `comptime for` inside `T { ... }` produces field initializers. Each iteration must produce exactly one `(field.name): value` pair |
| **E27: All fields required** | The compiler verifies every non-skipped field is initialized. Missing fields are a compile error |

## Error Messages

**Non-encodable field [E12]:**
```
ERROR [std.encoding/E12]: struct `Connection` is not Encode
   |
5  |  json.encode(conn)
   |               ^^^^ field `socket` has type `Socket` which is not Encode
   |
3  |  struct Connection {
4  |      public socket: Socket    ← Socket is not Encode
   |

WHY: Auto-derive requires all public fields to be Encode types.

FIX: Mark the field @skip, mark the struct @no_encode and implement custom
     encoding, or make Socket implement Encode.
```

**Opted-out type used as Encode [E16]:**
```
ERROR [std.encoding/E16]: `InternalState` does not implement Encode
   |
8  |  json.encode(state)
   |               ^^^^^ type marked @no_encode

WHY: @no_encode prevents auto-derive of Encode.

FIX: Remove @no_encode, or implement a custom encoding method.
```

**Runtime string in field access [CT53]:**
```
ERROR [ctrl.comptime/CT53]: runtime string in comptime field access
   |
5  |  const v = point.(name)
   |                   ^^^^ `name` is not comptime-known

WHY: Comptime field access resolves at compile time. The field name must be
     a comptime-known string.

FIX: Use a comptime-known string:

  const v = point.("x")              // string literal
  const v = point.(field.name)       // inside comptime for
```

**Unknown field [CT54]:**
```
ERROR [ctrl.comptime/CT54]: no field "z" on type `Point`
   |
5  |  const v = point.("z")
   |                    ^^^ Point has fields: x, y
```

**@skip without default on type without zero value [E28]:**
```
ERROR [std.encoding/E28]: @skip field has no default value
   |
4  |  @skip
5  |  public state: GameState
   |                ^^^^^^^^^ GameState has no known zero value

WHY: Skipped fields are initialized to a zero value during decode.
     GameState is a struct — no automatic zero value exists.

FIX: Add @default with an explicit value:

  @skip @default(GameState.initial())
  public state: GameState
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Struct with no public fields | E13 | Encodes as empty object `{}` |
| Recursive types (`struct Node { children: Vec<Node> }`) | E15 | Works — `Vec<Node>` where `Node: Encode` |
| All fields `@skip` | E19, E27 | Encodes as `{}`; decode produces struct with all defaults |
| `@default` on non-optional required field | E20 | Field becomes optional in input, required in struct definition. Default fills the gap |
| `@rename` collision (two fields same serial name) | E18 | Compile error: duplicate serial name |
| `@skip` field without `@default` during decode | E19, E28 | Field must have a known zero value (E28) or `@default`. Compile error otherwise |
| Nested comptime for (struct within struct) | CT48 | Works — `encode_value` recursively monomorphizes |
| Private field with `@rename` | — | Annotation accepted but ineffective — field not encoded externally. Useful for same-module custom encoding |
| Generic struct `Wrapper<T>` | E12 | `Encode` if `T: Encode`. Checked at monomorphization |
| Enum with non-Encode payload | E17 | Enum is not `Encode`. Error points to the non-Encode variant |

---

## Appendix (non-normative)

### Rationale

**E11 (marker traits):** I wanted Encode/Decode for generic bounds (`T: Encode`) but Rask doesn't have associated types in MVP. A Serializer trait hierarchy (like serde) would need them. Marker traits are the simplest option that enables compile-time checked generic bounds. Format libraries use `comptime for` directly instead of dispatching through trait methods — each format writes ~100 lines, which is acceptable since formats differ genuinely in how they handle nulls, numbers, nesting.

**E12-E13 (auto-derive, public fields only):** Matches Go's exported-fields-only behavior. I chose auto-derive with opt-out because the zero-ceremony path should be the common path. Adding a `File` field to a struct naturally breaks `Encode` — good. The error message tells you exactly which field is the problem.

**E16 (@no_encode):** The opt-out exists for types where automatic serialization is semantically wrong — connection pools, caches, types with invariants that can't survive a round-trip. The compiler won't silently serialize something you've explicitly excluded.

**E18-E20 (field annotations):** Field-level annotations keep customization at the point of use. I chose format-agnostic annotations (`@rename`, not `@json_rename`) because format-specific renaming is rare enough to handle with custom encoding. The annotations are typed and compiler-checked, unlike Go's stringly-typed struct tags.

**E22-E24 (enum serialization):** Externally tagged is the simplest default — unambiguous, requires no configuration. `@tag("type")` for internal tagging covers the common API pattern. I intentionally left out adjacently tagged and untagged — they add complexity and the escape hatch (custom encoding) covers the rare cases.

### Patterns & Guidance

**Second format (TOML):**

```rask
import std.reflect

func encode_value<T: Encode>(value: T, w: mutate TomlWriter, key: string?) -> () or TomlError {
    comptime if T == bool {
        return w.write_bool(key, value)
    } else if T == string {
        return w.write_string(key, value)
    } else if reflect.is_integer<T>() {
        return w.write_integer(key, value as i64)
    } else if reflect.is_struct<T>() {
        try w.begin_table(key)
        comptime for field in reflect.fields<T>() {
            comptime if !field.is_skipped {
                try encode_value(value.(field.name), mutate w, Some(field.serial_name))
            }
        }
        return w.end_table()
    }
    // TOML has no null — optionals with None are omitted
    // TOML arrays, inline tables, etc.
}
```

Each format library is self-contained. The comptime dispatch handles format-specific differences naturally (TOML omits null, MessagePack uses binary tags, etc.).

**Full round-trip example:**

```rask
import json

struct Config {
    @rename("server_host")
    public host: string

    @default(8080)
    public port: i32

    @skip
    public cached_at: i64
}

func main() -> () or Error {
    // Encode
    const config = Config { host: "localhost", port: 3000, cached_at: 0 }
    const text = json.encode(config)
    // → {"server_host": "localhost", "port": 3000}

    // Decode (port defaults to 8080 if missing)
    const loaded = try json.decode<Config>("{\"server_host\": \"example.com\"}")
    // → Config { host: "example.com", port: 8080, cached_at: 0 }
}
```

**HTTP JSON API server (validation target #1):**

```rask
import json
import http

struct CreateUserRequest {
    public name: string
    public email: string
    public age: i32?
}

struct UserResponse {
    public id: i64
    public name: string
    public email: string
}

func handle_create_user(req: http.Request) -> http.Response {
    const body = req.body() is Some else {
        return http.Response.bad_request("missing body")
    }
    const input = json.decode<CreateUserRequest>(body) is Ok else {
        return http.Response.bad_request("invalid JSON")
    }
    const user = create_user(input.name, input.email, input.age)
    const response = UserResponse { id: user.id, name: user.name, email: user.email }
    return http.Response.ok(json.encode(response))
}
```

Zero serialization boilerplate. Comparable to Go.

### See Also

- `ctrl.comptime` — Compile-time execution, `comptime if` (`ctrl.comptime/CT5`)
- `std.reflect` — Field reflection, type introspection (`std.reflect/R1`)
- `std.json` — JSON format library using this mechanism (`std.json/J6`)
- `type.generics` — Trait bounds, auto-derive pattern (`type.generics/CL1`)
- `type.traits` — Trait definitions, structural matching (`type.traits/TR1`)
- `mem.relocatable` — Pool binary serialization using Encode/Decode (`mem.relocatable/PB1`)
