<!-- id: std.fs -->
<!-- status: decided -->
<!-- summary: Two-tier file API — convenience functions and linear File handles with Reader/Writer -->
<!-- depends: stdlib/io.md, memory/resource-types.md -->

# File System

Two tiers: convenience functions (open/close internally) and `File` handles with explicit lifecycle. `File` is `@resource` — must be consumed before scope exit.

## File Type

| Rule | Description |
|------|-------------|
| **F1: Linear resource** | `File` is `@resource`. `file.close()` (takes ownership) is the only way to consume it |
| **F2: Reader/Writer** | `File` implements `Reader` and `Writer` traits |

<!-- test: skip -->
```rask
@resource
struct File {
    // Opaque — wraps OS file descriptor
}

extend File with Reader {
    func read(self, buf: []u8) -> usize or IoError
    func read_all(self) -> []u8 or IoError
}

extend File with Writer {
    func write(self, data: []u8) -> usize or IoError
    func write_all(self, data: []u8) -> () or IoError
    func flush(self) -> () or IoError
}

extend File {
    func close(take self) -> () or IoError
    func read_text(self) -> string or IoError
    func lines(self) -> Vec<string> or IoError
    func metadata(self) -> Metadata or IoError
}
```

## Convenience Functions

| Rule | Description |
|------|-------------|
| **F3: Self-contained** | Convenience functions open, operate, and close internally — no resource obligation |

| Function | Signature |
|----------|-----------|
| `fs.read_file` | `(path: string) -> string or IoError` |
| `fs.read_lines` | `(path: string) -> Vec<string> or IoError` |
| `fs.write_file` | `(path: string, content: string) -> () or IoError` |
| `fs.append_file` | `(path: string, content: string) -> () or IoError` |
| `fs.exists` | `(path: string) -> bool` |

<!-- test: parse -->
```rask
const content = try fs.read_file("config.txt")
try fs.write_file("output.txt", "hello world")
```

## File Handle Functions

| Rule | Description |
|------|-------------|
| **F4: Must consume** | Returned `File` must be closed via `close()` or `ensure`. Compiler rejects unconsumed handles |

| Function | Signature |
|----------|-----------|
| `fs.open` | `(path: string) -> File or IoError` |
| `fs.create` | `(path: string) -> File or IoError` |
| `fs.open_with` | `(path: string, opts: OpenOptions) -> File or IoError` |

<!-- test: skip -->
```rask
const file = try fs.open("data.txt")
ensure file.close()
const data = try file.read_text()
process(data)
```

## OpenOptions

| Rule | Description |
|------|-------------|
| **F5: Builder pattern** | Each method returns a new `OpenOptions` for chaining |

<!-- test: skip -->
```rask
struct OpenOptions {
    read: bool
    write: bool
    append: bool
    create: bool
    truncate: bool
}

const file = try fs.open_with("log.txt",
    OpenOptions.new().write(true).append(true).create(true))
ensure file.close()
```

## Directory and Path Operations

| Function | Signature | Description |
|----------|-----------|-------------|
| `fs.read_dir` | `(path: string) -> Vec<DirEntry> or IoError` | List directory contents |
| `fs.create_dir` | `(path: string) -> () or IoError` | Create single directory |
| `fs.create_dir_all` | `(path: string) -> () or IoError` | Create directory tree (mkdir -p) |
| `fs.remove` | `(path: string) -> () or IoError` | Remove file |
| `fs.remove_dir` | `(path: string) -> () or IoError` | Remove empty directory |
| `fs.remove_dir_all` | `(path: string) -> () or IoError` | Remove directory tree recursively |
| `fs.rename` | `(from: string, to: string) -> () or IoError` | Rename/move file or directory |
| `fs.copy` | `(from: string, to: string) -> u64 or IoError` | Copy file, returns bytes copied |
| `fs.canonicalize` | `(path: string) -> string or IoError` | Resolve to absolute path |
| `fs.metadata` | `(path: string) -> Metadata or IoError` | Get metadata without opening |

## Metadata and DirEntry

<!-- test: parse -->
```rask
struct Metadata {
    size: u64
    modified: u64       // Unix timestamp, seconds
    accessed: u64       // Unix timestamp, seconds
    is_file: bool
    is_dir: bool
    is_symlink: bool
}

struct DirEntry {
    name: string        // File name (no path)
    path: string        // Full path
}
```

## Error Messages

```
ERROR [std.fs/F1]: file handle not consumed
   |
3  |  const file = try fs.open("data.txt")
   |        ^^^^ `File` is a @resource that must be closed

WHY: File is a linear resource. Every code path must call close() or use ensure.

FIX: Add `ensure file.close()` after opening.
```

```
ERROR [std.fs/F4]: file not found
   |
5  |  const file = try fs.open("missing.txt")
   |                   ^^^^^^^^^^^^^^^^^^^^^^^^ IoError.NotFound

WHY: The path does not exist on the filesystem.
```

## Edge Cases

| Case | Behavior | Rule |
|------|----------|------|
| `close()` on already-closed file | No-op (supports ensure + explicit close) | F1 |
| `ensure` + explicit `close()` before scope end | Safe — ensure's close silently succeeds | F1 |
| File handle not closed | Compile error | F4 |
| `fs.exists` on symlink to missing target | Returns `false` | F3 |
| `fs.write_file` to existing file | Truncates and overwrites | F3 |
| `fs.append_file` to nonexistent file | Creates it | F3 |

---

## Appendix (non-normative)

### Rationale

**F1 (linear File):** Prevents resource leaks by construction. The compiler catches every code path where a file might go unconsumed. The `ensure` pattern makes this ergonomic.

**F3 (convenience functions):** For the common case of "read a file and move on," there's no reason to expose a handle. Convenience functions cover 80% of file operations.

**F5 (builder pattern):** `OpenOptions` avoids a proliferation of `fs.open_read_write_create` variants. Each setter returns a new value for chaining.

### Patterns & Guidance

**Transaction pattern** — write to temp file, close, then rename:

<!-- test: skip -->
```rask
const file = try fs.create("data.tmp")
ensure file.close()
try file.write(serialize(data))
file.close()
try fs.rename("data.tmp", "data.json")
```

### See Also

- `std.io` — `Reader`/`Writer` traits, `IoError` enum
- `mem.resource-types` — `@resource` and `ensure` semantics
- `type.errors` — `T or E` result pattern
