<!-- id: tool.describe -->
<!-- status: decided -->
<!-- summary: JSON schema for rask describe module API output -->

# Describe Schema

`rask describe` emits a structured summary of a module's public interface in JSON (v1) or human-readable text.

## CLI

| Flag | Effect |
|------|--------|
| **C1: Default** | `rask describe <file>` — human-readable text output |
| **C2: JSON** | `--format json` — machine-readable JSON |
| **C3: Private** | `--all` — include private items (`"public": false`) |

<!-- test: parse -->
```rask
// rask describe src/server.rk
// rask describe src/server.rk --format json
// rask describe src/server.rk --all
```

## Top-Level Schema

| Rule | Description |
|------|-------------|
| **S1: Version field** | Always `1` for this spec; consumers check `version` and ignore unknown fields |
| **S2: Empty arrays** | All arrays default to `[]` when empty |
| **S3: Absent optionals** | Optional fields omitted when absent, never `null` |

```json
{
  "version": 1,
  "module": "server",
  "file": "src/server.rk",
  "imports": [],
  "types": [],
  "enums": [],
  "traits": [],
  "functions": [],
  "constants": [],
  "externs": []
}
```

| Field | Type | Description |
|-------|------|-------------|
| `version` | `integer` | Schema version (always `1`) |
| `module` | `string` | Module name (from file name) |
| `file` | `string` | Source file path |
| `imports` | `Import[]` | Module imports |
| `types` | `StructType[]` | Struct definitions |
| `enums` | `EnumType[]` | Enum definitions |
| `traits` | `TraitType[]` | Trait definitions |
| `functions` | `Function[]` | Top-level functions |
| `constants` | `Constant[]` | Top-level constants |
| `externs` | `ExternFunc[]` | External function declarations |

## Function

| Rule | Description |
|------|-------------|
| **F1: Self mode** | `self_mode` is `"self"`, `"mutate"`, or `"take"`; absent for standalone functions |
| **F2: Result split** | Return type split into `ok`/`err` for `T or E` types |
| **F3: Optional fields** | `type_params`, `context`, `attrs`, `unsafe`, `comptime` omitted when absent/false |

```json
{
  "name": "start",
  "public": true,
  "params": [
    { "name": "config", "type": "Config", "mode": "borrow" }
  ],
  "returns": { "ok": "()", "err": "ServerError" },
  "self_mode": "take",
  "type_params": ["T"],
  "context": ["Pool<Connection>"],
  "attrs": ["inline"],
  "unsafe": false,
  "comptime": false
}
```

| Field | Type | Description |
|-------|------|-------------|
| `name` | `string` | Function name |
| `public` | `bool` | Whether the function is `public` |
| `params` | `Param[]` | Parameters (excluding self) |
| `returns` | `Returns` | Return type |
| `self_mode` | `string?` | `"self"`, `"mutate"`, or `"take"` |
| `type_params` | `string[]?` | Generic type parameter names |
| `context` | `string[]?` | Context clause requirements (`with` clauses) |
| `attrs` | `string[]?` | Attributes (`@inline`, `@entry`, etc.) |
| `unsafe` | `bool?` | True if `unsafe func` |
| `comptime` | `bool?` | True if `comptime func` |

### Param

| Field | Type | Description |
|-------|------|-------------|
| `name` | `string` | Parameter name |
| `type` | `string` | Type as written in source |
| `mode` | `string` | `"borrow"` (default), `"mutate"`, or `"take"` |

### Returns

| Field | Type | Description |
|-------|------|-------------|
| `ok` | `string` | Success type; `"()"` for void |
| `err` | `string?` | Error type; absent for non-Result returns |

```json
{ "ok": "i32" }
{ "ok": "ProcessResult", "err": "IoError" }
{ "ok": "()" }
```

## StructType

| Rule | Description |
|------|-------------|
| **T1: Methods merged** | Methods from struct body and `extend` blocks appear in one `methods` array |

```json
{
  "name": "Server",
  "public": true,
  "type_params": ["T"],
  "attrs": ["resource"],
  "fields": [
    { "name": "port", "type": "u16", "public": true },
    { "name": "connections", "type": "Vec<Connection>", "public": false }
  ],
  "methods": []
}
```

| Field | Type | Description |
|-------|------|-------------|
| `name` | `string` | Struct name |
| `public` | `bool` | Whether the struct is `public` |
| `type_params` | `string[]?` | Generic type parameter names |
| `attrs` | `string[]?` | Attributes (`@resource`, etc.) |
| `fields` | `Field[]` | Struct fields |
| `methods` | `Function[]` | Methods from struct body and extend blocks |

## EnumType

| Rule | Description |
|------|-------------|
| **E1: Positional fields** | Tuple variants use `"0"`, `"1"`, ... as field names |
| **E2: Unit variants** | Unit variants have an empty `fields` array |

```json
{
  "name": "ServerError",
  "public": true,
  "type_params": [],
  "variants": [
    { "name": "BindFailed", "fields": [{ "name": "0", "type": "string", "public": true }] },
    { "name": "Quit", "fields": [] }
  ],
  "methods": []
}
```

| Field | Type | Description |
|-------|------|-------------|
| `name` | `string` | Enum name |
| `public` | `bool` | Whether the enum is `public` |
| `type_params` | `string[]?` | Generic type parameter names |
| `variants` | `Variant[]` | Enum variants |
| `methods` | `Function[]` | Methods from extend blocks |

## TraitType

```json
{
  "name": "Reader",
  "public": true,
  "methods": [
    {
      "name": "read",
      "public": true,
      "self_mode": "self",
      "params": [{ "name": "buf", "type": "[]u8", "mode": "borrow" }],
      "returns": { "ok": "usize", "err": "IoError" }
    }
  ]
}
```

## Import

```json
{ "path": ["net"], "alias": null, "is_glob": false, "is_lazy": false }
{ "path": ["http", "Request"], "alias": "HttpReq", "is_glob": false, "is_lazy": false }
```

## Constant

| Field | Type | Description |
|-------|------|-------------|
| `name` | `string` | Constant name |
| `type` | `string?` | Type annotation if present |
| `public` | `bool` | Whether the constant is `public` |

## ExternFunc

```json
{
  "abi": "C",
  "name": "sqlite3_open",
  "params": [
    { "name": "filename", "type": "RawPtr<u8>", "mode": "borrow" },
    { "name": "db", "type": "RawPtr<RawPtr<sqlite3>>", "mode": "borrow" }
  ],
  "returns": { "ok": "i32" }
}
```

## Visibility Filtering

| Rule | Description |
|------|-------------|
| **V1: Public default** | Only `public` items appear by default |
| **V2: All flag** | `--all` includes private items, marked `"public": false` |

## Versioning

| Rule | Description |
|------|-------------|
| **VR1: Minor additions** | Adding optional fields keeps version at `1`; consumers ignore unknown keys |
| **VR2: Breaking changes** | Changing field semantics or removing fields bumps to `v2` |

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| No public items | V1 | All arrays empty |
| Extend block in different file | T1 | Methods merged into defining type |
| No return type annotation | F2 | `"ok": "()"` |
| Type-inferred constant | — | `type` field omitted |
| No explicit type on param | — | Type as written in source |

## Full Example

```rask
import net
import json

public struct Server {
    public port: u16
    connections: Vec<Connection>
}

public enum ServerError {
    BindFailed(string)
    ConfigInvalid(string)
}

extend Server {
    public func start(take self, config: Config) -> () or ServerError
        using Pool<Connection>
    {
        // ...
    }

    public func stop(take self) {
        // ...
    }
}
```

`rask describe src/server.rk --format json` produces:

```json
{
  "version": 1,
  "module": "server",
  "file": "src/server.rk",
  "imports": [
    { "path": ["net"], "alias": null, "is_glob": false, "is_lazy": false },
    { "path": ["json"], "alias": null, "is_glob": false, "is_lazy": false }
  ],
  "types": [
    {
      "name": "Server",
      "public": true,
      "fields": [
        { "name": "port", "type": "u16", "public": true },
        { "name": "connections", "type": "Vec<Connection>", "public": false }
      ],
      "methods": [
        {
          "name": "start",
          "public": true,
          "self_mode": "take",
          "params": [{ "name": "config", "type": "Config", "mode": "borrow" }],
          "returns": { "ok": "()", "err": "ServerError" },
          "context": ["Pool<Connection>"]
        },
        {
          "name": "stop",
          "public": true,
          "self_mode": "take",
          "params": [],
          "returns": { "ok": "()" }
        }
      ]
    }
  ],
  "enums": [
    {
      "name": "ServerError",
      "public": true,
      "variants": [
        { "name": "BindFailed", "fields": [{ "name": "0", "type": "string", "public": true }] },
        { "name": "ConfigInvalid", "fields": [{ "name": "0", "type": "string", "public": true }] }
      ],
      "methods": []
    }
  ],
  "traits": [],
  "functions": [],
  "constants": [],
  "externs": []
}
```

Human-readable output (without `--format json`):

```
server (src/server.rk)

  public struct Server
    public port: u16
    connections: Vec<Connection>

    public func start(take self, config: Config) -> () or ServerError  using Pool<Connection>
    public func stop(take self)

  public enum ServerError
    BindFailed(string)
    ConfigInvalid(string)
```

---

## Appendix (non-normative)

### Rationale

**S3 (absent optionals):** Omitting absent fields rather than emitting `null` keeps the JSON compact and avoids consumers needing null-checks for every optional.

**F2 (result split):** Splitting `T or E` into `ok`/`err` fields makes it trivial for tools to detect error-returning functions without parsing type strings.

**T1 (methods merged):** Consumers don't care whether a method was in the struct body or an `extend` block. Merging them gives one place to look.

### What's Not Included

- Function bodies, expressions, or implementation details
- Type inference results — only explicitly written types
- Private items (unless `--all`)
- Documentation comments (future: add `doc` field)

### See Also

- `tool.lint` — convention violations
- `tool.warnings` — compiler warnings
- `rask explain` — deep analysis of a single function (call graph, resources)
- `rask check --json` — compilation errors with structured diagnostics
