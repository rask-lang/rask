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
| **E12: Auto-derive** | The compiler auto-derives `Encode` for any struct where every serialized field (E13) has an `Encode` type, unless the struct is marked `@no_encode`. Same for `Decode` |
| **E13: Non-private fields** | Auto-derived encoding covers `public` and package-default fields. `private` fields never auto-serialize, in either direction |
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
    name: string
    age: i32
    email: string?
}
// Auto-derives Encode and Decode — nothing to annotate, no `public` needed

struct Account {
    email: string
    private password_hash: string      // never on the wire, either direction
}
// Encodes as {"email": "…"} — see E13a for what decode does with the hash

@no_encode
struct InternalState {
    data: Vec<u8>
}
// Opted out — won't satisfy Encode bound
```

`public` means one thing: cross-package code visibility. It has no say in the wire format.

### Private Fields and Decode [E13a]

| Rule | Description |
|------|-------------|
| **E13a: Excluded fields need a default** | A field left out of the wire form — `private` or `@no_serialize` — takes its **declared default** (`type.structs/FD1`, FD6) on decode, or its `@default(expr)` override. A field with neither means the type is not auto-`Decode`; the compile error names the field |
| **E13b: Never read from input** | An excluded field is never filled from the input, even when a key of that name is present. Such keys are ignored under the general unknown-key policy (`std.json/J10`) |

<!-- test: skip -->
```rask
struct Account {
    email: string
    private password_hash: string = ""     // declared default → Account: Decode
}

struct Session {
    token: string
    private started: Instant               // no default → Session is not Decode
}
```

E13b is the mass-assignment rail: an attacker who puts `"password_hash": "…"` in the request body cannot reach the field, because decode never looks for it.

### Generic Bounds

<!-- test: skip -->
```rask
func send_json<T: Encode>(endpoint: string, value: T) -> void or HttpError {
    const body = json.encode(value)
    return http.post(endpoint, body)
}

func load_config<T: Decode>(path: string) -> T or ConfigError {
    const text = try fs.read_text(path)
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
        const s = value.as_string() else { return JsonError.TypeError("expected string for DateTime") }
        return try DateTime.parse_iso8601(s)
    }
}
```

## Field Annotations

| Rule | Description |
|------|-------------|
| **E18: @rename** | `@rename("name")` on a struct field overrides the serialized key name. Reflected as `FieldInfo.serial_name` |
| **E19: @no_serialize** | `@no_serialize` excludes a non-private field from the serialized form in **both** directions. Decode value comes from the declared default (E13a). Reflected as `FieldInfo.serialized == false` |
| **E20: @default** | `@default(expr)` provides a comptime value used when the field is missing during deserialization. The field becomes optional in the input. A decode-only override of the declared default (`type.structs/FD6`). Reflected as `FieldInfo.has_default` |
| **E21: Comptime expressions** | `@rename` takes a string literal. `@default` takes a comptime expression. Both validated at compile time |

`@no_serialize` covers both directions on purpose. One-directional exclusion — off the wire on encode, still filled on decode — is how mass-assignment holes get built.

<!-- test: skip -->
```rask
struct ApiUser {
    @rename("user_name")
    name: string

    @no_serialize
    cache_key: string = ""

    @default(0)
    login_count: i32

    @default("unknown")
    role: string
}

// JSON: {"user_name": "alice", "login_count": 5, "role": "admin"}
// Missing login_count → defaults to 0
// Missing role → defaults to "unknown"
// cache_key never appears in output, never read from input — decodes to ""
```

### Excluded Fields and Auto-Derive

An excluded field doesn't need to be `Encode`/`Decode`. It's out of the wire form, so it gets no say in whether the type qualifies:

<!-- test: skip -->
```rask
struct CachedUser {
    name: string
    age: i32

    @no_serialize
    internal_id: Connection = Connection.stub()   // not Encode, but excluded
}
// CachedUser: Encode and Decode
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
func encode_value<T: Encode>(value: T, w: mutate JsonWriter) -> void or JsonError {
    comptime if T == bool {
        return w.write_bool(value)
    } else if T == string {
        return w.write_string(value)
    } else if reflect.is_integer<T>() {
        return w.write_number(value as f64)
    } else if reflect.is_float<T>() {
        return w.write_number(value)
    } else if reflect.is_optional<T>() {
        if value? as v {
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
            try w.write_key(key)
            try encode_value(val, mutate w)
        }
        return w.end_object()
    } else if reflect.is_struct<T>() {
        try w.begin_object()
        comptime for field in reflect.fields<T>() {
            comptime if field.serialized {
                try w.write_key(field.serial_name)
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
    return w.build()
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
            return none
        }
        return try decode_value(parser)
    } else if reflect.is_vec<T>() {
        mut result = Vec.new()
        try parser.begin_array()
        while !parser.is_array_end() {
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
    while !parser.is_object_end() {
        const key = try parser.read_key()
        fields.insert(key, try parser.read_value())
    }

    return T {
        comptime for field in reflect.fields<T>() {
            comptime if field.serialized {
                (field.name): comptime if field.has_default {
                    if fields.get(field.serial_name)? as v {
                        try decode_from_value(v)
                    } else {
                        field.default_value
                    }
                } else {
                    try decode_from_value(
                        fields.get(field.serial_name) ?? return JsonError.MissingField(field.serial_name)
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
ERROR [E0333]: `Connection` cannot be decoded
   |
5  |  json.decode<Connection>(body)
   |  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ field `socket` has type `Socket`, which can't be decoded

FIX: mark `socket` with `@no_serialize` (and give it a default), make it
     `private`, or hold it in a serializable type

WHY: `Decode` isn't implemented by hand — a type has it when its fields do,
     all the way down (std.encoding/E12)
```

The field is named through nesting, so a struct-in-a-struct reports `inner.socket`. Excluding the field really does resolve it: it's out of the wire form, so it gets no say in whether the type qualifies (E19).

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

**Excluded field with no default [E13a]:**
```
ERROR [std.encoding/E13a]: `Session` cannot be decoded
   |
3  |  private started: Instant
   |          ^^^^^^^ left out of the wire form, and has no default

WHY: Decode never reads a private or @no_serialize field from the input, so it
     needs a value from somewhere else. Rask has no universal zero — a value
     the author didn't choose is the Go mistake (type.structs/FD4).

FIX: Declare a default:

  private started: Instant = Instant.epoch()

  or a decode-only one:

  @default(Instant.now()) private started: Instant
```

**`@skip` [migration]:**
```
ERROR [std.encoding/E19]: `@skip` is now `@no_serialize`
   |
4  |  @skip
   |  ^^^^^ skip from what? the name didn't say

FIX: @no_serialize
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Struct with only `private` fields | E13 | Encodes as empty object `{}` |
| Recursive types (`struct Node { children: Vec<Node> }`) | E15 | Works — `Vec<Node>` where `Node: Encode` |
| All fields excluded | E19, E27 | Encodes as `{}`; decode produces the struct from declared defaults |
| `@default` on non-optional required field | E20 | Field becomes optional in input, required in struct definition. Default fills the gap |
| `@rename` collision (two fields same serial name) | E18 | Compile error: duplicate serial name |
| Excluded field without a default | E13a | Compile error naming the field — the type is not auto-`Decode` |
| Input carries a key matching an excluded field | E13b | Ignored, like any unknown key (`std.json/J10`) |
| `@no_serialize` on a `private` field | E19 | Accepted but redundant; lint suggests dropping it |
| `@rename` on a `private` field | E13 | Accepted but ineffective — the field isn't on the wire. Useful for same-module custom encoding |
| Nested comptime for (struct within struct) | CT48 | Works — `encode_value` recursively monomorphizes |
| Generic struct `Wrapper<T>` | E12 | `Encode` if `T: Encode`. Checked at monomorphization |
| Enum with non-Encode payload | E17 | Enum is not `Encode`. Error points to the non-Encode variant |

---

## Appendix (non-normative)

### Rationale

**E11 (marker traits):** I wanted Encode/Decode for generic bounds (`T: Encode`) but Rask doesn't have associated types in MVP. A Serializer trait hierarchy (like serde) would need them. Marker traits are the simplest option that enables compile-time checked generic bounds. Format libraries use `comptime for` directly instead of dispatching through trait methods — each format writes ~100 lines, which is acceptable since formats differ genuinely in how they handle nulls, numbers, nesting.

**E12 (auto-derive with opt-out):** The zero-ceremony path should be the common path. Adding a `File` field to a struct naturally breaks `Encode` — good. The error message tells you exactly which field is the problem.

**E13 (non-private, not public-only).** This used to be public-fields-only, copied from Go's exported-fields rule. It had the polarity backwards. The common case is "serialize the whole value", and it was the one paying per-field ceremony: every DTO field wore a `public` it didn't need for code access — the validation example's `dto.rk` was columns of `public` doing wire-work, not visibility-work — to guard against an uncommon accident.

The visibility tiers already mean the right things, so the gate just had to move to the tier that means "protected":

- `private` — guarded by the type's own code: invariants, secrets. `password_hash` is `private` because it must be, and is thereby off the wire.
- package-default — plain data. Every function in the package can already read it; serialization now agrees with the language's own position on what that tier means.

`public` goes back to meaning exactly one thing: cross-package code visibility. Under E13 `dto.rk` needs zero `public`, zero annotations, and produces the identical wire format.

The leak rail is stronger than before, not weaker. Under the old rule, protecting a field meant *remembering* not to write `public` on it — an omission, invisible in review. Now it's `private`, which states the intent and is enforced by the compiler for code access too.

**E13a/E28 (declared defaults, not invented zeros).** E28 used to carry a "known zero values" table — `0`, `false`, `""`, empty vec — purely to have something to put in a skipped field on decode. That's a shadow `Default` trait, the same disease that got `Default` removed (`type.generics`): values the author never chose, appearing in their struct. Declared field defaults (FD1/FD6) already do this job properly, with the value written where the reader can see it. So E28 and its table are gone; an excluded field without a default is a compile error that names the field. `@default(expr)` stays as the decode-only override.

**E16 (@no_encode):** The opt-out exists for types where automatic serialization is semantically wrong — connection pools, caches, types with invariants that can't survive a round-trip. The compiler won't silently serialize something you've explicitly excluded.

**E18-E20 (field annotations):** Field-level annotations keep customization at the point of use. I chose format-agnostic annotations (`@rename`, not `@json_rename`) because format-specific renaming is rare enough to handle with custom encoding. The annotations are typed and compiler-checked, unlike Go's stringly-typed struct tags.

**E19 (`@skip` → `@no_serialize`).** `@skip` failed the guess test (`std.api/SD2`) — skip from what? Iteration? Validation? The field-level concept is "not part of this type's serialized form, either direction", and `@no_serialize` says that. It also joins the `@no_X` directive family already in the language (`@no_encode`, `@no_decode`), so the shape is familiar.

The umbrella verb is deliberate. Separate `@no_encode`/`@no_decode` at field level would let you write a field that's hidden from output but still filled from input — which is a mass-assignment hole with extra steps.

**Deferred: serializing `private` fields.** Persistence and checkpoint snapshots genuinely want the whole value, secrets included. That's not this rule — it belongs to durable-state work with its own author-consent design. Manual implementations cover it meanwhile.

**E22-E24 (enum serialization):** Externally tagged is the simplest default — unambiguous, requires no configuration. `@tag("type")` for internal tagging covers the common API pattern. I intentionally left out adjacently tagged and untagged — they add complexity and the escape hatch (custom encoding) covers the rare cases.

### Patterns & Guidance

**Second format (TOML):**

```rask
import std.reflect

func encode_value<T: Encode>(value: T, w: mutate TomlWriter, key: string?) -> void or TomlError {
    comptime if T == bool {
        return w.write_bool(key, value)
    } else if T == string {
        return w.write_string(key, value)
    } else if reflect.is_integer<T>() {
        return w.write_integer(key, value as i64)
    } else if reflect.is_struct<T>() {
        try w.begin_table(key)
        comptime for field in reflect.fields<T>() {
            comptime if field.serialized {
                try encode_value(value.(field.name), mutate w, field.serial_name)
            }
        }
        return w.end_table()
    }
    // TOML has no null — optionals that are `none` are omitted
    // TOML arrays, inline tables, etc.
}
```

Each format library is self-contained. The comptime dispatch handles format-specific differences naturally (TOML omits null, MessagePack uses binary tags, etc.).

**Full round-trip example:**

```rask
import json

struct Config {
    @rename("server_host")
    host: string

    @default(8080)
    port: i32

    @no_serialize
    cached_at: i64 = 0
}

func main() -> void or Error {
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
    name: string
    email: string
    age: i32?
}

struct UserResponse {
    id: i64
    name: string
    email: string
}

func handle_create_user(req: http.Request) -> http.Response {
    if req.body.is_empty() { return http.Response.bad_request("missing body") }
    const input = json.decode<CreateUserRequest>(req.body) else { return http.Response.bad_request("invalid JSON") }
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
- `type.generics` — Trait conformance, structural opt-in (`type.generics/G1`)
- `mem.relocatable` — Pool binary serialization using Encode/Decode (`mem.relocatable/PB1`)
