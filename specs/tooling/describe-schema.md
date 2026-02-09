# `rask describe` — JSON Schema

`rask describe` emits a structured summary of a module's public interface. IDE plugins, documentation generators, and AI assistants all consume the same format.

```
rask describe src/server.rk                  # human-readable text
rask describe src/server.rk --format json    # machine-readable JSON
rask describe src/server.rk --all            # include private items
```

---

## Schema (v1)

The JSON output follows this structure. All arrays default to `[]` when empty. Optional fields are omitted when absent.

### Top Level

```json
{
  "version": 1,
  "module": "server",
  "file": "src/server.rk",
  "imports": [ ... ],
  "types": [ ... ],
  "enums": [ ... ],
  "traits": [ ... ],
  "functions": [ ... ],
  "constants": [ ... ],
  "externs": [ ... ]
}
```

| Field | Type | Description |
|-------|------|-------------|
| `version` | `integer` | Schema version. Always `1` for this spec. |
| `module` | `string` | Module name (derived from file name). |
| `file` | `string` | Source file path. |
| `imports` | `Import[]` | Module imports. |
| `types` | `StructType[]` | Struct definitions. |
| `enums` | `EnumType[]` | Enum definitions. |
| `traits` | `TraitType[]` | Trait definitions. |
| `functions` | `Function[]` | Top-level functions. |
| `constants` | `Constant[]` | Top-level constants. |
| `externs` | `ExternFunc[]` | External function declarations. |

---

### Function

```json
{
  "name": "start",
  "public": true,
  "params": [
    { "name": "config", "type": "Config", "mode": "read" }
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
| `name` | `string` | Function name. |
| `public` | `bool` | Whether the function is `public`. |
| `params` | `Param[]` | Parameters (excluding self). |
| `returns` | `Returns` | Return type, split into ok/err for Result types. |
| `self_mode` | `string?` | `"self"`, `"read"`, or `"take"`. Absent for standalone functions. |
| `type_params` | `string[]?` | Generic type parameter names. |
| `context` | `string[]?` | Context clause requirements (`with` clauses). |
| `attrs` | `string[]?` | Attributes (`@inline`, `@entry`, etc.). |
| `unsafe` | `bool?` | True if `unsafe func`. |
| `comptime` | `bool?` | True if `comptime func`. |

### Param

```json
{ "name": "data", "type": "Vec<u8>", "mode": "take" }
```

| Field | Type | Description |
|-------|------|-------------|
| `name` | `string` | Parameter name. |
| `type` | `string` | Type as written in source. |
| `mode` | `string` | `"borrow"` (default), `"read"`, or `"take"`. |

### Returns

For plain return types:
```json
{ "ok": "i32" }
```

For `T or E` result types:
```json
{ "ok": "ProcessResult", "err": "IoError" }
```

For void functions:
```json
{ "ok": "()" }
```

| Field | Type | Description |
|-------|------|-------------|
| `ok` | `string` | Success type. `"()"` for void. |
| `err` | `string?` | Error type. Absent for non-Result returns. |

---

### StructType

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
  "methods": [ ... ]
}
```

| Field | Type | Description |
|-------|------|-------------|
| `name` | `string` | Struct name. |
| `public` | `bool` | Whether the struct is `public`. |
| `type_params` | `string[]?` | Generic type parameter names. |
| `attrs` | `string[]?` | Attributes (`@resource`, etc.). |
| `fields` | `Field[]` | Struct fields. |
| `methods` | `Function[]` | Methods defined in the struct body and extend blocks. |

### Field

```json
{ "name": "port", "type": "u16", "public": true }
```

---

### EnumType

```json
{
  "name": "ServerError",
  "public": true,
  "type_params": [],
  "variants": [
    { "name": "BindFailed", "fields": [{ "name": "0", "type": "string", "public": true }] },
    { "name": "ConfigInvalid", "fields": [{ "name": "0", "type": "string", "public": true }] }
  ],
  "methods": [ ... ]
}
```

| Field | Type | Description |
|-------|------|-------------|
| `name` | `string` | Enum name. |
| `public` | `bool` | Whether the enum is `public`. |
| `type_params` | `string[]?` | Generic type parameter names. |
| `variants` | `Variant[]` | Enum variants. |
| `methods` | `Function[]` | Methods from extend blocks. |

### Variant

Tuple variants use positional field names (`"0"`, `"1"`, ...). Struct variants use named fields.

```json
{ "name": "Move", "fields": [{ "name": "x", "type": "i32", "public": true }, { "name": "y", "type": "i32", "public": true }] }
```

Unit variants have an empty fields array:
```json
{ "name": "Quit", "fields": [] }
```

---

### TraitType

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

---

### Import

```json
{ "path": ["net"], "alias": null, "is_glob": false, "is_lazy": false }
```

```json
{ "path": ["http", "Request"], "alias": "HttpReq", "is_glob": false, "is_lazy": false }
```

---

### Constant

```json
{ "name": "MAX_CONNECTIONS", "type": "u32", "public": true }
```

| Field | Type | Description |
|-------|------|-------------|
| `name` | `string` | Constant name. |
| `type` | `string?` | Type annotation if present. |
| `public` | `bool` | Whether the constant is `public`. |

---

### ExternFunc

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

---

## Full Example

Given this source:

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
        with Pool<Connection>
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
          "params": [{ "name": "config", "type": "Config", "mode": "read" }],
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

---

## Human-Readable Output

Without `--format json`, `rask describe` outputs a compact text summary:

```
server (src/server.rk)

  public struct Server
    public port: u16
    connections: Vec<Connection>

    public func start(take self, config: Config) -> () or ServerError  with Pool<Connection>
    public func stop(take self)

  public enum ServerError
    BindFailed(string)
    ConfigInvalid(string)
```

---

## Visibility Filtering

By default, only `public` items appear. Use `--all` to include private items. Private items are marked in JSON with `"public": false`.

---

## Versioning

The `version` field enables forward compatibility. Consumers should check `version` and handle unknown fields gracefully. Schema changes:

- **Adding optional fields** — minor bump (still v1), consumers ignore unknown keys.
- **Changing field semantics or removing fields** — major bump (v2).

---

## What's Not Included

- Function bodies, expressions, or implementation details.
- Type inference results — only explicitly written types.
- Private items (unless `--all`).
- Documentation comments (future: add `doc` field).

---

## Relationship to Other Tools

| Tool | Purpose |
|------|---------|
| `rask describe` | Public API surface (types, signatures) |
| `rask explain` | Deep analysis of a single function (call graph, resources) |
| `rask check --json` | Compilation errors with structured diagnostics |
| `rask lint --json` | Convention violations |
