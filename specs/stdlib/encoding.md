<!-- id: std.encoding -->
<!-- status: decided -->
<!-- summary: Comptime field iteration and auto-derived Encode/Decode for format-agnostic serialization -->
<!-- depends: control/comptime.md, stdlib/reflect.md, types/generics.md, types/interfaces.md -->

# Encoding

Two language primitives — `comptime for` over struct fields and comptime field access — plus auto-derived `Encode`/`Decode` interfaces. Format libraries (JSON, TOML, MessagePack) use these to serialize any compatible struct with zero user ceremony.

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

struct Point { x: f64, y: f64 }

// print_fields(Point { x: 1.0, y: 2.0 }) unrolls to:
//   print("x = {value.x}")
//   print("y = {value.y}")
```

## Encode and Decode Interfaces

| Rule | Description |
|------|-------------|
| **E11: Marker interfaces** | `Encode` and `Decode` are interfaces with no required methods. They signal that a type's structure is serialization-compatible. A type may add one method to each block: its own `decode` to `Decode` (E12b), its own `encode` to `Encode` (E12d) |
| **E12: Auto-derive** | The compiler auto-derives `Encode` for any struct where every serialized field (E13) has an `Encode` type, unless the struct is marked `@no_encode`. Same for `Decode`, with one more condition: E12a. And `Encode` has one of its own: E12e |
| **E12a: Checking constructors** | A type with a checking constructor is not auto-`Decode`. That is a static function the author wrote in an `extend` block whose return type is `Self or E`, with `E` a real error (`Self?` doesn't count), at any visibility. The type, and any `Vec`, `Map`, field or payload that holds it, fails a `Decode` bound with an error that names the constructor. Two things lift the rule for a type, both written where `T` is declared: its own `decode` (E12b), which is how the constructor gets called, and `T implements Decode { }` with nothing in it, which says a raw decode of the fields is intended. `Encode` is never affected |
| **E12b: Own decode** | `T implements Decode { func decode(wire: W) -> Self or E { … } }` makes every decoder read the input as `W`, decode that the normal way, and hand the value to `decode`. It applies wherever `T` appears: a field, a `Vec` element, a `Map` value, a `T?`, an enum payload, the top level. `decode` is static, takes one plain parameter and returns `Self or E`. `W` is any `Decode` type, itself possibly one with its own `decode`, and `E` is any error (a union of errors too). `T` is concrete: a generic type can't write one. The fields of `T` are never consulted, so they need not be decodable |
| **E12c: A refusal is a decode error** | When `decode` returns an error, the decode fails with that error's `message()` behind where in the input the value was. `json.decode` returns `JsonError.TypeError("field ports[1]: port 0 is reserved")`, and the other decoders word it their own way (below). A claim's inputs are built through `decode` too, and an input it refuses is discarded, not given to the claim (`type.claims/CL5`, IN1) |
| **E12d: Own encode** | `T implements Encode { func encode(self) -> W { … } }` makes every encoder write `W` in `T`'s place, the normal way, wherever `T` appears: a field, a `Vec` element, a `Map` value, a `T?`, an enum payload, the top level. `encode` takes a plain `self` and nothing else. `W` is any `Encode` type, itself possibly one with its own `encode`. It can't fail, because writing a value out can't make an invalid one, so `W` isn't a `T or E`. `T` is concrete, and its fields are never consulted, so they need not be encodable |
| **E12e: Read through `decode`, written through `encode`** | A type with its own `decode` is not auto-`Encode`. The fields a derived `Encode` would write aren't what `decode` reads, so a value written out and read back fails at run time, in whoever reads the output. The type, and any `Vec`, `Map`, field or payload that holds it, fails an `Encode` bound with an error that names it (E0424). Three things lift the rule, all written where `T` is declared: its own `encode` (E12d), `T implements Encode { }` with nothing in it, which says the fields are the wire form, and `@no_encode`, which says it isn't written at all |
| **E13: Non-private fields** | Auto-derived encoding covers `public` and package-default fields. `private` fields never auto-serialize, in either direction |
| **E14: Base types** | `bool`, `i8`–`i64`, `u8`–`u64`, `f32`, `f64`, `string` are `Encode` and `Decode` |
| **E15: Collection types** | `Vec<T>` where `T: Encode`, `Map<string, T>` where `T: Encode`, `T?` where `T: Encode` — all auto-implement `Encode`. Same for `Decode` |
| **E16: Opt-out** | `@no_encode` on a struct prevents auto-derive of `Encode`. `@no_decode` prevents `Decode`. For types where automatic serialization is semantically wrong |
| **E17: Enum auto-derive** | Enums auto-derive `Encode`/`Decode` when all variant payloads are `Encode`/`Decode` types |

<!-- surface -->
```rask
interface Encode { }
interface Decode { }
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

### Types With a Checking Constructor [E12a, E12b]

<!-- test: run | 2 ports\n{"ports":[80,8080]}\ntype error: field ports[1]: port 0 is reserved -->
```rask
import json

struct PortError { reason: string }

extend PortError {
    public func message(self) -> string { return self.reason }
}

struct Port { value: u16 }

extend Port {
    func new(v: u16) -> Port or PortError {
        if v == 0 { return PortError(reason: "port 0 is reserved") }
        return Port(value: v)
    }
}

// How a Port is decoded: as a u16, then through `new`.
Port implements Decode {
    func decode(raw: u16) -> Port or PortError {
        return Port.new(raw)
    }
}

// And written: as the u16 it was read as.
Port implements Encode {
    func encode(self) -> u16 {
        return self.value
    }
}

struct Config { ports: Vec<Port> }

func main() {
    let good = json.decode<Config>("{{\"ports\": [80, 8080]}") catch e => {
        println(e.message())
        return
    }
    println("{good.ports.len()} ports")
    println(json.encode(good))
    let bad = json.decode<Config>("{{\"ports\": [80, 0]}") catch e => {
        println(e.message())
        return
    }
    println("{bad.ports.len()} ports")
}
```

Each `Port` in `ports` is read as a `u16` and goes through `Port.decode`, and written as the `u16` `Port.encode` answers. `Config` needs nothing of its own, and neither does a `Vec<Port>` or a `Map<string, Port>` somewhere else: the type says how it is read and written once, and every place it appears follows.

The two are a pair. A type that reads itself through `decode` and said nothing about writing would be written as an object of its fields, which `decode` can't read, so that is an error (E12e) and not a default. `Port implements Encode { }` and `@no_encode` are the other two answers.

`decode`'s parameter can be anything `Decode`, including a type with a `decode` of its own, and the decoder follows the chain. A type that reads a `string` and hands a parsed value on, behind a type that reads a struct, works the same way. A circle (a type whose wire type is itself) is an error (E0423).

When the function that returns `Self or E` isn't a check on decoded data, say so where the type is declared:

<!-- test: skip -->
```rask
extend Settings {
    func load(path: string) -> Settings or IoError { … }
}

Settings implements Decode { }     // a raw decode is fine
```

The empty block and the one with `decode` are the same declaration with different amounts written down. Empty says the fields decode the way they would anyway. With `decode`, you say how. Anything else in the block is still E0893, and `decode` itself has to be the shape E12b gives, or the error says which part isn't (E0423).

The error for a decode that would skip the constructor names it and shows the `decode` that calls it:

```
ERROR [E0418]: `Port` cannot be decoded: `Port.new` checks its input
   |
12 |  json.decode<Port>(text)
   |  ^^^^^^^^^^^^^^^^^^^^^^^ `Port.new` returns `Port or PortError`, and a decode builds `Port` without calling it

WHY: a type with a checking constructor promises its values passed the check,
     and a derived `Decode` would break that promise without a word.

FIX: write the `decode` that runs the check. It takes what the input holds and returns the checked value:

       Port implements Decode {
           func decode(raw: /* what the input holds */) -> Port or PortError {
               return Port.new(raw)
           }
       }

     or, if `new` isn't a check on decoded data: `Port implements Decode { }`
```

The error for a write that would produce something `decode` can't read:

```
ERROR [E0424]: `Port` cannot be encoded: `Port` is read as a `u16` and would be written as its fields
   |
18 |  json.encode(port)
   |  ^^^^^^^^^^^^^^^^^ `Port.decode` reads a `u16`, and an encode would write `Port` as an object of its fields, which `decode` doesn't read back

WHY: a value that is written out and read back should come back the same.

FIX: write the `encode` that is `decode` the other way round:

       Port implements Encode {
           func encode(self) -> u16 { return /* the u16 that `decode` reads */ }
       }

     or, if the fields really are what gets written: `Port implements Encode { }`
     or keep it from being written at all with `@no_encode`
```

### How Each Codec Honours It [E12b, E12d]

| Decoder | What it does |
|---------|--------------|
| `json.decode` (native and interpreter) | Reads the input as `W` and calls `decode`. A refusal is a `JsonError.TypeError` whose message is `field path: message`, the path worded like every other decode error (`field ports[1]`, `field by_name.db`, `the value` at the top). A `W` of `T?` reads a missing member as `none` |
| `csv.decode` | Reads a cell as `W`, so a `Port` taking a `u16` is a number in its column, empty is `0`, and `abc` is rejected as a bad number before `decode` sees it. `decode` runs on the result, and a refusal is a `CsvError.BadValue` for the row with the same message. `FieldInfo.wire_type_name` (`std.reflect`) is what tells a format what to read a field as |
| `json.encode`, `encode_pretty` (native and interpreter) | Calls `encode` for each value of the type and writes what it returns the normal way: `80`, not `{"value":80}`. Compact and pretty agree |
| `csv.encode` | Writes the cell the encode gave, a number or an unquoted string, so what it writes is what `csv.decode` reads |
| claim inputs | Builds a `W` from the claim's choices, calls `decode`, and discards the input when it refuses. A type with its own `decode` is always sampled, like one with its own generator (`type.claims/IN2`), because what it refuses isn't known before it runs |

### Generic Bounds

<!-- test: skip -->
```rask
func send_json<T: Encode>(endpoint: string, value: T) -> void or HttpError {
    let body = json.encode(value)
    return http.post(endpoint, body)
}

func load_config<T: Decode>(path: string) -> T or ConfigError {
    let text = try fs.read_text(path)
    return try toml.decode<T>(text)
}
```

### Manual Implementation

For a type whose wire form isn't its fields (a date written as a string, a port that checks its number), write `decode` and `encode`. One of each serves every format that reads or writes the type: json, csv and a claim's inputs call the same functions. There is no per-format hook to write, and none to forget.

<!-- test: skip -->
```rask
struct DateTime {
    year: i32
    month: u8
    day: u8
    hour: u8
    minute: u8
    second: u8
}

struct ParseError { reason: string }

extend ParseError {
    public func message(self) -> string { return self.reason }
}

extend DateTime {
    func parse_iso8601(s: string) -> DateTime or ParseError { … }
    func to_iso8601(self) -> string { … }
}

// Read as a string, then parsed. `{"at": "2026-11-15T09:30:00"}` decodes, and
// `{"at": "yesterday"}` fails with `field at: not an ISO 8601 date`.
DateTime implements Decode {
    func decode(s: string) -> DateTime or ParseError {
        return DateTime.parse_iso8601(s)
    }
}

// Written as the same string, so the output reads back.
DateTime implements Encode {
    func encode(self) -> string {
        return self.to_iso8601()
    }
}
```

Both can reach the type's private fields, since they are written where the type is. That's what a custom invariant needs.

Writing only one of the two is not a half measure the compiler lets through quietly. A type with `decode` and nothing for `Encode` is E0424. The other direction is allowed: a type with `encode` and no `decode` is simply never read back, which is a choice about the type, not a mismatch.

## Field Annotations

| Rule | Description |
|------|-------------|
| **E18: @rename** | `@rename("name")` on a struct field overrides the serialized key name. Reflected as `FieldInfo.serial_name` |
| **E19: @no_serialize** | `@no_serialize` excludes a non-private field from the serialized form in **both** directions. Decode value comes from the declared default (E13a). Reflected as `FieldInfo.serialized == false` |
| **E20: @default** | `@default(expr)` provides a comptime value used when the field is missing during deserialization. The field becomes optional in the input. A decode-only override of the declared default (`type.structs/FD6`). Reflected as `FieldInfo.has_default` |
| **E21: Comptime expressions** | `@rename` takes a string literal. `@default` takes a comptime expression, checked against the field where it's written and folded while compiling under the same rule as a declared default (`type.structs/FD1`). Any constant works, not only a literal: `@default(2 + 3)`, `@default(Status.Closed)` |

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
| **E24a: @tag needs names** | `@tag` requires every payload to be named. A variant with one unnamed field has no key to pair its value with, and the tag already holds the object's only unnamed slot — compile error (`E0841`) |
| **E24b: @tag can't shadow a field** | `@tag("k")` on an enum where a variant has a field named `k` would write `"k"` twice in one object — compile error (`E0842`) |

<!-- test: skip -->
```rask
enum Shape {
    Circle(radius: f64)
    Rectangle(width: f64, height: f64)
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
    Click(x: i32, y: i32)

    @rename("key_press")
    KeyPress(code: u32)
}
// Internal:
//   Click    → {"type": "Click", "x": 10, "y": 20}
//   KeyPress → {"type": "key_press", "code": 65}
```

Both ways `@tag` can be malformed are caught at the declaration, not when a value reaches `json.encode` — the shape is wrong for every value, so there's nothing a runtime check would learn (`ctrl.panic/S7`):

<!-- test: skip -->
```rask
@tag("type")
enum Shape {
    Circle(f64)              // E24a/E0841: unnamed payload, no key to tag it with
}

@tag("kind")
enum Event {
    Click(kind: string)   // E24b/E0842: `"kind"` would be written twice
}
```

Externally tagged enums have neither problem: the variant name is the key, so an unnamed payload goes directly under it (E23) and a field named `kind` is just a field.

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

// Top-level entry point. Infallible: E12 requires every serialized field to be
// Encode, so encode_value can't hit an unsupported type at runtime.
public func encode<T: Encode>(value: T) -> string {
    mut w = JsonWriter.new()
    encode_value(value, mutate w)!
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
        let n = try parser.read_number()
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
        let key = try parser.read_key()
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
                    let raw = fields.get(field.serial_name)
                        ?? return JsonError.MissingField(field.serial_name)
                    try decode_from_value(raw)
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
ERROR [E0388]: `Connection` cannot be decoded
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

FIX: Remove @no_encode, or write the type's `encode` (E12d).
```

**Runtime string in field access [CT53]:**
```
ERROR [ctrl.comptime/CT53]: runtime string in comptime field access
   |
5  |  let v = point.(name)
   |                   ^^^^ `name` is not comptime-known

WHY: Comptime field access resolves at compile time. The field name must be
     a comptime-known string.

FIX: Use a comptime-known string:

  let v = point.("x")              // string literal
  let v = point.(field.name)       // inside comptime for
```

**Unknown field [CT54]:**
```
ERROR [ctrl.comptime/CT54]: no field "z" on type `Point`
   |
5  |  let v = point.("z")
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

  or a decode-only override (comptime expression, E21):

  @default(Instant.epoch()) private started: Instant

  A decode-TIME value (e.g. "now") can't come from @default — it's evaluated
  at compile time. Make the field `Instant?` and fill it after decode.
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

**E11 (marker interfaces):** I wanted Encode/Decode for generic bounds (`T: Encode`) and a Serializer interface hierarchy (like serde) needs associated types, which Rask didn't have when this was written. It has them now (`type.associated-types`), so the hierarchy is writable — but the markers work and the rewrite isn't free, so that's its own call, not a consequence of this one. Marker interfaces are the simplest option that enables compile-time checked generic bounds. Format libraries use `comptime for` directly instead of dispatching through interface methods — each format writes ~100 lines, which is acceptable since formats differ genuinely in how they handle nulls, numbers, nesting.

**E12 (auto-derive with opt-out):** The zero-ceremony path should be the common path. Adding a `File` field to a struct naturally breaks `Encode` — good. The error message tells you exactly which field is the problem.

**E12a (a constructor that checks is a promise).** `Port.new` rejects 0, so everyone who receives a `Port` assumes it isn't 0. Auto-derived `Decode` broke that without a sound: `json.decode<Port>` built the value straight from the wire. I kept `Decode` auto-derived for plain data, because that's most types and the zero-ceremony path should stay the common one, and withheld it from a type whose author wrote a constructor that can fail. `Encode` stays derived for everyone, since writing a value out can't make an invalid one.

The trigger is the shape of the declaration, not the name: a static function in an `extend` block returning `Self or E`. I count it at any visibility, because a package-private constructor guards the package's own values as much as a public one does. I don't count `Self?`: absence carries no rejection reason, and the rule is about a check that fails with one. The cost is a false positive when a function returns `Self or E` without validating anything (`Settings.load(path)`). `T implements Decode { }` is the escape for that, and it is visible at the declaration, where a reader of the type will see it. The `{ }` form means "raw decode is intended". It doesn't call anything, so it can't stand in for the check.

**E12b (a type says how it decodes, once).** A constructor that checks is only a promise if the decoder calls it, and the decoders build structs field by field. The first answer was a plain struct of the raw fields, decoded and handed to the constructor by hand. That works for one type at the top. It doesn't compose: `struct Config { ports: Vec<Port> }` would need a second `Config` of raw ports and a loop to convert, for every struct that holds a `Port`. The check has to live with the type, and the decoders have to find it.

I wanted one method and no new syntax (`RULINGS` test 7: a feature pays rent in the language, not the compiler). `decode` goes in the `Decode` block, which already exists and already means "how this type decodes", and `Decode` stays an interface with nothing required, so nearly every type pays nothing. The empty block and the block with `decode` are one declaration at two levels of detail, so there isn't a second keyword to learn.

What `decode` reads is its parameter's type. I considered an associated type, `type Wire = u16` beside the method, and decided against it. The signature already says it, and an associated type would make every author write it twice and keep the two in step. It would also earn nothing: `Decode` can't declare `decode`'s signature either way, because the parameter differs per type, so the compiler checks the method against the one shape E12b describes whichever way the type is named. Reading it off the parameter adds nothing to the language.

`decode` is concrete-only. A generic type's wire type could mention its parameters, and the decoders would need that substitution carried into the runtime's description of the type. Nothing needs it yet, and a generic container can already hold a concrete type with a `decode`.

The refusal is a message and a path because that is what a decode error is. `E` can be a union of errors, so the compiler writes one function beside `decode` that turns any of them into `message()`, and the decoders call that. A decoder never takes an error apart.

**E12d, E12e (written the way it is read).** The first cut of E12b left `Encode` alone, on the ground that this change was about the decoders calling user code. That left a defect: a `DateTime` read from a string and written as an object. Every value that went out came back as an error, and nothing said so until a round trip failed in someone else's process. So `encode` is the same idea turned round: it returns the type written in `T`'s place, the encoders write that the normal way, and a type can chain through another (a `Tag` written as a `Name`, a `Name` as a string) without any encoder knowing.

It takes `self` and returns `W`, with no error, because the other half of E12a's argument holds here: writing a value out can't make an invalid one.

The pairing is a question about what the default does when it's wrong. Without a rule, a type with a `decode` and nothing else would derive `Encode` from its fields, and the one who pays is whoever reads the output, at run time, with no pointer back to the type (`RULINGS` test 8). The way out that costs least is `@no_encode`, which the `DateTime` example used to wear: an escape hatch, which is how you can tell the default was wrong. So a type with its own `decode` gets no derived `Encode`, and the error at the use names the type, the same way E12a treats a constructor. Each answer says what you mean: `encode`, `T implements Encode { }` for fields that really are the wire form, `@no_encode` for a type that is never written. The reverse pairing isn't an error. A type that is only ever written, like a log record, has no reason to read itself back.

The JSON text of a value with its own `encode` comes from a method the compiler writes beside it, `__encode_json`, which is `json.encode(self.encode())`. The native encoder unrolls most shapes into straight-line code, but a `Map` is walked by the runtime, which has no unrolled code to put a call in. It asks the type for its JSON text instead.

**E13 (non-private, not public-only).** This used to be public-fields-only, copied from Go's exported-fields rule. It had the polarity backwards. The common case is "serialize the whole value", and it was the one paying per-field ceremony: every DTO field wore a `public` it didn't need for code access — the validation example's `dto.rk` was columns of `public` doing wire-work, not visibility-work — to guard against an uncommon accident.

The visibility tiers already mean the right things, so the gate just had to move to the tier that means "protected":

- `private` — guarded by the type's own code: invariants, secrets. `password_hash` is `private` because it must be, and is thereby off the wire.
- package-default — plain data. Every function in the package can already read it; serialization now agrees with the language's own position on what that tier means.

`public` goes back to meaning exactly one thing: cross-package code visibility. Under E13 `dto.rk` needs zero `public`, zero annotations, and produces the identical wire format.

The leak rail is stronger than before, not weaker. Under the old rule, protecting a field meant *remembering* not to write `public` on it — an omission, invisible in review. Now it's `private`, which states the intent and is enforced by the compiler for code access too.

**E13a/E28 (declared defaults, not invented zeros).** E28 used to carry a "known zero values" table — `0`, `false`, `""`, empty vec — purely to have something to put in a skipped field on decode. That's a shadow `Default` interface, the same disease that got `Default` removed (`type.generics`): values the author never chose, appearing in their struct. Declared field defaults (FD1/FD6) already do this job properly, with the value written where the reader can see it. So E28 and its table are gone; an excluded field without a default is a compile error that names the field. `@default(expr)` stays as the decode-only override.

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
    let config = Config(host: "localhost", port: 3000, cached_at: 0)
    let text = json.encode(config)
    // → {"server_host": "localhost", "port": 3000}

    // Decode (port defaults to 8080 if missing)
    let loaded = try json.decode<Config>("{\"server_host\": \"example.com\"}")
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
    let input = json.decode<CreateUserRequest>(req.body)
        catch _ => return http.Response.bad_request("invalid JSON")
    let user = create_user(input.name, input.email, input.age)
    let response = UserResponse(id: user.id, name: user.name, email: user.email)
    return http.Response.ok(json.encode(response))
}
```

Zero serialization boilerplate. Comparable to Go.

### See Also

- `ctrl.comptime` — Compile-time execution, `comptime if` (`ctrl.comptime/CT5`)
- `std.reflect` — Field reflection, type introspection (`std.reflect/R1`)
- `std.json` — JSON format library using this mechanism (`std.json/J6`)
- `type.generics` — Interface bounds, auto-derive pattern (`type.generics/CL1`)
- `type.generics` — Interface conformance, structural opt-in (`type.generics/G1`)
- `mem.relocatable` — Rack binary serialization using Encode/Decode (`mem.relocatable/RB1`)
