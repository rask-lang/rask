# File System (fs)

Two-tier API: convenience functions handle open/close internally, `File` handles have explicit lifecycle. `File` is a `@resource` type — must be consumed (closed) before scope exit. All fallible operations return `T or IoError`. `File` implements `Reader` and `Writer` traits.

## Specification

### File Type

<!-- test: skip -->
```rask
@resource
struct File {
    // Opaque handle -- wraps OS file descriptor
}
```

`File` is linear: creating one starts a resource obligation, `file.close()` (which takes ownership) is the only way to satisfy it. Compiler rejects code paths where a `File` might go unconsumed.

### File Methods

<!-- test: skip -->
```rask
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
    // Consumes the resource (takes ownership)
    func close(take self) -> () or IoError

    // Read helpers
    func read_to_string(self) -> string or IoError
    func lines(self) -> Vec<string> or IoError

    // Metadata
    func metadata(self) -> Metadata or IoError
}
```

### Convenience Functions

These open, operate, and close internally. No `File` handle is exposed, so no resource obligation is created.

| Function | Signature | Description |
|----------|-----------|-------------|
| `fs.read_file` | `(path: string) -> string or IoError` | Read entire file to string |
| `fs.read_lines` | `(path: string) -> Vec<string> or IoError` | Read file as line vector |
| `fs.write_file` | `(path: string, content: string) -> () or IoError` | Write string to file (create/truncate) |
| `fs.append_file` | `(path: string, content: string) -> () or IoError` | Append string to file (create if missing) |
| `fs.exists` | `(path: string) -> bool` | Check if path exists (infallible) |

### File Handle Functions

These return a `File` that must be consumed.

| Function | Signature | Description |
|----------|-----------|-------------|
| `fs.open` | `(path: string) -> File or IoError` | Open existing file for reading |
| `fs.create` | `(path: string) -> File or IoError` | Create file for writing (truncates if exists) |
| `fs.open_with` | `(path: string, opts: OpenOptions) -> File or IoError` | Open with explicit options |

### OpenOptions

<!-- test: skip -->
```rask
struct OpenOptions {
    read: bool
    write: bool
    append: bool
    create: bool
    truncate: bool
}

extend OpenOptions {
    func new() -> OpenOptions {
        return OpenOptions {
            read: false
            write: false
            append: false
            create: false
            truncate: false
        }
    }

    func read(self, val: bool) -> OpenOptions
    func write(self, val: bool) -> OpenOptions
    func append(self, val: bool) -> OpenOptions
    func create(self, val: bool) -> OpenOptions
    func truncate(self, val: bool) -> OpenOptions
}
```

Builder pattern -- each method returns a new `OpenOptions` for chaining:

<!-- test: skip -->
```rask
const file = try fs.open_with("log.txt",
    OpenOptions.new().write(true).append(true).create(true))
ensure file.close()
```

### Metadata

<!-- test: skip -->
```rask
struct Metadata {
    size: u64           // File size in bytes
    modified: u64       // Last modified (Unix timestamp, seconds)
    accessed: u64       // Last accessed (Unix timestamp, seconds)
    is_file: bool
    is_dir: bool
    is_symlink: bool
}
```

Accessible via `fs.metadata(path)` (no open required) or `file.metadata()` (on open handle).

### Directory Operations

| Function | Signature | Description |
|----------|-----------|-------------|
| `fs.read_dir` | `(path: string) -> Vec<DirEntry> or IoError` | List directory contents |
| `fs.create_dir` | `(path: string) -> () or IoError` | Create single directory |
| `fs.create_dir_all` | `(path: string) -> () or IoError` | Create directory tree (mkdir -p) |
| `fs.remove` | `(path: string) -> () or IoError` | Remove file |
| `fs.remove_dir` | `(path: string) -> () or IoError` | Remove empty directory |
| `fs.remove_dir_all` | `(path: string) -> () or IoError` | Remove directory tree recursively |

### Path Operations

| Function | Signature | Description |
|----------|-----------|-------------|
| `fs.rename` | `(from: string, to: string) -> () or IoError` | Rename or move file/directory |
| `fs.copy` | `(from: string, to: string) -> u64 or IoError` | Copy file, returns bytes copied |
| `fs.canonicalize` | `(path: string) -> string or IoError` | Resolve to absolute path |
| `fs.metadata` | `(path: string) -> Metadata or IoError` | Get metadata without opening |

### DirEntry

<!-- test: skip -->
```rask
struct DirEntry {
    name: string        // File name (no path)
    path: string        // Full path
}
```

### Usage Examples

**Simple read (convenience, no resource management):**

<!-- test: skip -->
```rask
const content = try fs.read_file("config.txt")
println(content)
```

**File handle with ensure (explicit lifecycle):**

<!-- test: skip -->
```rask
const file = try fs.open("data.txt")
ensure file.close()

const data = try file.read_to_string()
process(data)
```

`ensure` block guarantees `file.close()` runs when scope exits, even if `read_to_string()` or `process()` fails. Without `ensure` and without calling `close()`, compiler rejects the code.

**Write a file:**

<!-- test: skip -->
```rask
try fs.write_file("output.txt", "hello world")
```

**Append to a log:**

<!-- test: skip -->
```rask
try fs.append_file("app.log", format("{}: request handled\n", timestamp()))
```

**Copy with byte count:**

<!-- test: skip -->
```rask
const bytes = try fs.copy("original.dat", "backup.dat")
println(format("{} bytes copied", bytes))
```

**Directory listing:**

<!-- test: skip -->
```rask
const entries = try fs.read_dir(".")
for entry in entries {
    const meta = try fs.metadata(entry.path)
    const kind = if meta.is_dir: "dir" else: "file"
    println(format("  {} ({})", entry.name, kind))
}
```

**Create directory tree:**

<!-- test: skip -->
```rask
try fs.create_dir_all("output/reports/2026")
try fs.write_file("output/reports/2026/q1.txt", report)
```

**Transaction pattern (explicit close before ensure):**

<!-- test: skip -->
```rask
const file = try fs.create("data.tmp")
ensure file.close()

try file.write(serialize(data))
file.close()                        // explicit close before rename
try fs.rename("data.tmp", "data.json")
```

Calling `close()` explicitly before `ensure` runs is safe — `ensure` block's `close()` silently succeeds on already-closed file.

### Error Type

All fs operations use `IoError` from the io module:

<!-- test: skip -->
```rask
enum IoError {
    NotFound(string)        // path
    PermissionDenied(string)
    AlreadyExists(string)
    IsADirectory(string)
    NotADirectory(string)
    DirectoryNotEmpty(string)
    BrokenPipe
    ConnectionRefused
    TimedOut
    Other(string)
}
```

The interpreter currently returns `string` errors. The typed `IoError` enum is planned for the compiled backend.

### Implementation Status

| Feature | Interpreter | Compiler |
|---------|:-----------:|:--------:|
| `fs.read_file` | Done | Planned |
| `fs.read_lines` | Done | Planned |
| `fs.write_file` | Done | Planned |
| `fs.append_file` | Done | Planned |
| `fs.exists` | Done | Planned |
| `fs.open` | Done | Planned |
| `fs.create` | Done | Planned |
| `fs.open_with` | -- | Planned |
| `fs.canonicalize` | Done | Planned |
| `fs.metadata` | Done | Planned |
| `fs.remove` | Done | Planned |
| `fs.remove_dir` | Done | Planned |
| `fs.remove_dir_all` | -- | Planned |
| `fs.create_dir` | Done | Planned |
| `fs.create_dir_all` | Done | Planned |
| `fs.rename` | Done | Planned |
| `fs.copy` | Done | Planned |
| `fs.read_dir` | -- | Planned |
| `file.close()` | Done | Planned |
| `file.read_all()` / `read_to_string()` | Done | Planned |
| `file.write()` | Done | Planned |
| `file.lines()` | Done | Planned |
| `file.metadata()` | -- | Planned |
| Reader/Writer traits | -- | Planned |
| `OpenOptions` | -- | Planned |
| `DirEntry` | -- | Planned |
| Typed `IoError` | -- | Planned |
| Linear resource enforcement | Done (runtime) | Planned (compile-time) |
