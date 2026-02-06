# Solution: I/O Module

## The Question
How do byte-level I/O operations work? What traits unify file, network, and buffer I/O?

## Decision
Trait-based I/O with `Reader` and `Writer` as the foundation. Buffered wrappers (`BufReader`, `BufWriter`) for efficient line-oriented access. Standard streams (`Stdin`, `Stdout`, `Stderr`) are linear resource handles. A single `IoError` enum covers all I/O errors.

## Rationale
Trait-based I/O enables generic functions (`io.copy`, pipe, tee) that work across files, sockets, and in-memory buffers. Linear handles for standard streams prevent accidental closing of stdout/stderr. Buffered wrappers are separate types (not implicit) to keep cost transparent (TC >= 0.90). A single error enum avoids proliferation while remaining matchable.

## Specification

### IoError Enum

<!-- test: parse -->
```rask
enum IoError {
    NotFound(string)
    PermissionDenied(string)
    AlreadyExists(string)
    BrokenPipe
    ConnectionReset
    TimedOut
    UnexpectedEof
    Other(string)
}
```

All I/O operations return `T or IoError`. The variants map to OS-level error categories. `Other(string)` is the catch-all for platform-specific errors.

### Reader Trait

<!-- test: parse -->
```rask
trait Reader {
    func read(self, buf: []u8) -> usize or IoError
    func read_all(self) -> []u8 or IoError
    func read_to_string(self) -> string or IoError
    func read_exact(self, buf: []u8) -> () or IoError
}
```

| Method | Description |
|--------|-------------|
| `read(buf)` | Read up to `buf.len()` bytes. Returns bytes read (0 = EOF) |
| `read_all()` | Read all remaining bytes into a new `[]u8`. Allocates |
| `read_to_string()` | Read all remaining bytes as UTF-8 string. Allocates. Fails if not valid UTF-8 |
| `read_exact(buf)` | Fill `buf` completely. Returns `UnexpectedEof` if stream ends early |

### Writer Trait

<!-- test: parse -->
```rask
trait Writer {
    func write(self, data: []u8) -> usize or IoError
    func write_all(self, data: []u8) -> () or IoError
    func flush(self) -> () or IoError
}
```

| Method | Description |
|--------|-------------|
| `write(data)` | Write up to `data.len()` bytes. Returns bytes written |
| `write_all(data)` | Write entire buffer. Retries partial writes internally |
| `flush()` | Flush internal buffers to the underlying sink |

### BufReader\<R: Reader\>

Wraps any `Reader` with an internal buffer for efficient small reads and line-oriented access.

<!-- test: parse -->
```rask
struct BufReader<R: Reader> {
    inner: R
    buf: []u8
}
```

**Construction:**

| Constructor | Description |
|-------------|-------------|
| `BufReader.new(reader)` | Wrap reader with default 8KB buffer |
| `BufReader.with_capacity(cap, reader)` | Wrap reader with `cap`-byte buffer |

**Methods:**

| Method | Returns | Description |
|--------|---------|-------------|
| `read_line(self)` | `string or IoError` | Read one line (strips trailing newline) |
| `lines(self)` | `Iterator<string or IoError>` | Lazy line iterator |
| `inner(self)` | `R` | Access underlying reader |

`BufReader` implements `Reader`, so it can be passed to any function expecting `Reader`.

<!-- test: skip -->
```rask
const file = try fs.open("data.txt")
ensure file.close()
const reader = BufReader.new(file)

for line in reader.lines() {
    const text = try line
    println(text)
}
```

### BufWriter\<W: Writer\>

Wraps any `Writer` with an internal buffer to reduce system calls.

<!-- test: parse -->
```rask
struct BufWriter<W: Writer> {
    inner: W
    buf: []u8
}
```

**Construction:**

| Constructor | Description |
|-------------|-------------|
| `BufWriter.new(writer)` | Wrap writer with default 8KB buffer |
| `BufWriter.with_capacity(cap, writer)` | Wrap writer with `cap`-byte buffer |

`BufWriter` implements `Writer`. Flushes automatically when the buffer is full or when `flush()` is called explicitly. Also flushes on drop (best-effort; errors during drop-flush are silently discarded).

### Standard Streams

Standard streams are accessed through the `io` module. Each returns a linear resource handle.

```rask
import io

const stdin = io.stdin()     // Stdin (@resource, implements Reader)
const stdout = io.stdout()   // Stdout (@resource, implements Writer)
const stderr = io.stderr()   // Stderr (@resource, implements Writer)
```

| Type | Implements | Linear? | Notes |
|------|------------|---------|-------|
| `Stdin` | `Reader` | Yes | Must close or use with `ensure` |
| `Stdout` | `Writer` | Yes | Must close or use with `ensure` |
| `Stderr` | `Writer` | Yes | Must close or use with `ensure` |

**Why linear?** Standard streams are process-global resources. Making them linear prevents:
- Accidentally closing stdout mid-program
- Multiple parts of code racing to close the same stream
- Silent resource leaks in long-running processes

**Typical usage with `ensure`:**

<!-- test: skip -->
```rask
func main() -> () or IoError {
    const stdout = io.stdout()
    ensure stdout.close()

    try stdout.write_all("Hello, world!\n".as_bytes())
    try stdout.flush()

    return ()
}
```

**Close semantics:** Calling `close()` on a standard stream releases the handle. It does NOT close the underlying file descriptor (the OS manages that at process exit). This satisfies the linear requirement without surprising behavior.

### Buffer Type

In-memory byte buffer implementing both `Reader` and `Writer`. Not linear (no OS resource).

<!-- test: parse -->
```rask
struct Buffer {
    data: Vec<u8>
    pos: usize
}
```

| Constructor | Description |
|-------------|-------------|
| `Buffer.new()` | Empty buffer |
| `Buffer.from(data: []u8)` | Buffer initialized with data, read position at 0 |

| Method | Returns | Description |
|--------|---------|-------------|
| `as_bytes(self)` | `[]u8` | View of all written bytes |
| `len(self)` | `usize` | Total bytes written |
| `reset(self)` | `()` | Reset read position to 0 |

Buffer is useful for testing (mock I/O) and for building data in memory before writing.

<!-- test: skip -->
```rask
const buf = Buffer.new()
try buf.write_all("hello ".as_bytes())
try buf.write_all("world".as_bytes())

const result = string.from_utf8(buf.as_bytes())  // "hello world"
```

### Convenience Functions

The `io` module provides shortcuts for common operations.

| Function | Returns | Description |
|----------|---------|-------------|
| `io.read_line()` | `string or IoError` | Read one line from stdin (strips newline) |
| `io.copy(reader, writer)` | `usize or IoError` | Copy all bytes from reader to writer. Returns total bytes copied |

These avoid the ceremony of acquiring and managing standard stream handles for simple programs.

<!-- test: skip -->
```rask
// Simple interactive program - no handle management needed
const name = try io.read_line()
println("Hello, {name}")
```

`io.copy` reads from `reader` in chunks (default 8KB) and writes to `writer`:

<!-- test: skip -->
```rask
// Copy file to stdout
const file = try fs.open("output.txt")
ensure file.close()
const stdout = io.stdout()
ensure stdout.close()

const bytes_copied = try io.copy(file, stdout)
```

### Trait Implementations Summary

| Type | Reader | Writer | Linear |
|------|--------|--------|--------|
| `Stdin` | Yes | -- | Yes |
| `Stdout` | -- | Yes | Yes |
| `Stderr` | -- | Yes | Yes |
| `File` (from `fs`) | Yes | Yes | Yes |
| `Buffer` | Yes | Yes | No |
| `BufReader<R>` | Yes | -- | Inherits from R |
| `BufWriter<W>` | -- | Yes | Inherits from W |

### Edge Cases

| Case | Handling |
|------|----------|
| Read from closed stream | Returns `IoError.Other("stream closed")` |
| Write to broken pipe | Returns `IoError.BrokenPipe` |
| `read_to_string` with invalid UTF-8 | Returns `IoError.Other("invalid UTF-8")` |
| `read_exact` on short stream | Returns `IoError.UnexpectedEof` |
| `io.copy` with same reader and writer | Undefined (caller must not alias) |
| `BufWriter` flush on drop fails | Error silently discarded (use explicit `flush()`) |
| `Stdout` not closed | Compile error (linear resource) |
| `Buffer` overflow | Grows like `Vec` (fallible allocation) |
| Zero-length read/write | Returns `Ok(0)`, no-op |

### Thread Safety

| Type | `Send` | `Sync` |
|------|--------|--------|
| `Stdin` | Yes | No (single reader) |
| `Stdout` | Yes | No (single writer) |
| `Stderr` | Yes | No (single writer) |
| `Buffer` | Yes | No |
| `BufReader<R>` | if R: Send | No |
| `BufWriter<W>` | if W: Send | No |

Standard streams are `Send` but not `Sync`. To share across threads, transfer ownership via channel or protect with a mutex.

## Examples

### Read All Lines from File
<!-- test: skip -->
```rask
func count_lines(path: string) -> usize or IoError {
    const file = try fs.open(path)
    ensure file.close()
    const reader = BufReader.new(file)

    let count = 0
    for line in reader.lines() {
        try line  // propagate read errors
        count += 1
    }

    return count
}
```

### Interactive Input Loop
<!-- test: skip -->
```rask
func repl() -> () or IoError {
    loop {
        print("> ")
        const line = try io.read_line()

        if line == "quit" {
            return return ()
        }

        const result = evaluate(line)
        println(result)
    }
}
```

### Generic Copy with Progress
<!-- test: skip -->
```rask
func copy_with_progress(
    source: Reader,
    dest: Writer,
    total: usize,
) -> () or IoError {
    const buf = [0u8; 8192]
    let copied = 0

    loop {
        const n = try source.read(buf)
        if n == 0 {
            return return ()
        }
        try dest.write_all(buf[0..n])
        copied += n
        print_progress(copied, total)
    }
}
```

### In-Memory Testing
<!-- test: skip -->
```rask
func test_serializer() {
    const buf = Buffer.new()
    const record = Record { name: "test", value: 42 }

    try record.serialize(buf)  // buf implements Writer

    const expected = [0x74, 0x65, 0x73, 0x74, 0x00, 0x2A]
    assert_eq(buf.as_bytes(), expected)
}
```

## Implementation Status

| Feature | Status | Notes |
|---------|--------|-------|
| `io.read_line()` | Implemented | In interpreter (`rask-interp/src/stdlib/io.rs`) |
| `println()` / `print()` | Implemented | Builtin functions |
| `Reader` trait | Planned | |
| `Writer` trait | Planned | |
| `BufReader` | Planned | |
| `BufWriter` | Planned | |
| `Buffer` | Planned | |
| `Stdin` / `Stdout` / `Stderr` | Planned | Currently implicit in `io.read_line()` and `println()` |
| `io.copy()` | Planned | |
| `IoError` enum | Planned | Currently uses `string` for errors |

## Integration Notes

- **Resource Types:** `Stdin`, `Stdout`, `Stderr`, and `File` are `@resource` types. Must be consumed via `close()` or `ensure`. See [resource-types.md](../memory/resource-types.md).
- **File System:** The `fs` module builds on `Reader`/`Writer`. `File` implements both traits. See [fs.md](fs.md) (planned).
- **Concurrency:** Standard stream handles are `Send` but not `Sync`. For concurrent logging, use channels or a logging abstraction.
- **Error Handling:** `IoError` is a regular enum. Use `try` to propagate, `match` to handle specific variants. See [error-types.md](../types/error-types.md).
- **C Interop:** Raw file descriptors accessible via unsafe blocks on `File`. Standard streams map to fd 0/1/2.
- **Networking:** Future `net` module types (`TcpStream`, `UdpSocket`) will implement `Reader`/`Writer`.

## See Also

- [Resource Types](../memory/resource-types.md) -- Must-consume types and `ensure`
- [Error Types](../types/error-types.md) -- `T or E` results and `try`
- [Collections](collections.md) -- `Vec<u8>` used as byte buffers
- [Strings](strings.md) -- `string` type and UTF-8 handling
- [Concurrency](../concurrency/) -- Thread-safe I/O patterns
