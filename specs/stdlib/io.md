<!-- id: std.io -->
<!-- status: decided -->
<!-- summary: Reader/Writer traits, buffered wrappers, standard streams, IoError -->
<!-- depends: memory/resource-types.md, types/error-types.md -->

# I/O

Trait-based I/O: `Reader` and `Writer` as foundation, buffered wrappers for efficiency, linear standard stream handles, single `IoError` enum.

## IoError

| Rule | Description |
|------|-------------|
| **E1: Single error type** | All I/O operations return `T or IoError` |
| **E2: OS mapping** | Variants map to OS-level error categories; `Other(string)` is the catch-all |

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

## Reader Trait

| Rule | Description |
|------|-------------|
| **R1: Read** | `read(buf)` reads up to `buf.len()` bytes. Returns bytes read (0 = EOF) |
| **R2: Read all** | `read_all()` reads all remaining bytes. Allocates |
| **R3: Read text** | `read_text()` reads all as UTF-8 string. Fails on invalid UTF-8 |
| **R4: Read exact** | `read_exact(buf)` fills `buf` completely or returns `UnexpectedEof` |

<!-- test: skip -->
```rask
trait Reader {
    func read(self, buf: []u8) -> usize or IoError
    func read_all(self) -> []u8 or IoError
    func read_text(self) -> string or IoError
    func read_exact(self, buf: []u8) -> () or IoError
}
```

## Writer Trait

| Rule | Description |
|------|-------------|
| **W1: Write** | `write(data)` writes up to `data.len()` bytes. Returns bytes written |
| **W2: Write all** | `write_all(data)` writes entire buffer, retrying partial writes internally |
| **W3: Flush** | `flush()` flushes internal buffers to the underlying sink |

<!-- test: parse -->
```rask
trait Writer {
    func write(self, data: []u8) -> usize or IoError
    func write_all(self, data: []u8) -> () or IoError
    func flush(self) -> () or IoError
}
```

## Buffered Wrappers

| Rule | Description |
|------|-------------|
| **B1: BufReader** | Wraps `Reader` with internal buffer (default 8KB) for efficient small reads and line access |
| **B2: BufWriter** | Wraps `Writer` with internal buffer (default 8KB). Flushes on full buffer, explicit `flush()`, or drop (best-effort) |
| **B3: Linearity inherited** | Buffered wrappers inherit linearity from their inner type |

<!-- test: skip -->
```rask
const reader = BufReader.new(file)              // default 8KB
const reader = BufReader.with_capacity(4096, file)

reader.read_line() -> string or IoError         // strips trailing newline
reader.lines() -> Iterator<string or IoError>   // lazy line iterator
```

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

## Standard Streams

| Rule | Description |
|------|-------------|
| **S1: Linear handles** | `Stdin`, `Stdout`, `Stderr` are `@resource` — must be consumed via `close()` or `ensure` |
| **S2: Send not Sync** | Standard streams are `Send` but not `Sync`. Transfer via channel or protect with mutex |
| **S3: Close semantics** | `close()` releases the handle, not the underlying fd (OS manages that at exit) |

| Type | Implements | Linear |
|------|------------|--------|
| `Stdin` | `Reader` | Yes |
| `Stdout` | `Writer` | Yes |
| `Stderr` | `Writer` | Yes |

<!-- test: skip -->
```rask
import io

const stdout = io.stdout()
ensure stdout.close()
try stdout.write_all("Hello, world!\n".as_bytes())
try stdout.flush()
```

## Buffer Type

| Rule | Description |
|------|-------------|
| **B4: In-memory** | `Buffer` implements both `Reader` and `Writer`. Not linear (no OS resource) |

<!-- test: skip -->
```rask
const buf = Buffer.new()
try buf.write_all("hello ".as_bytes())
try buf.write_all("world".as_bytes())
const result = string.from_utf8(buf.as_bytes())  // "hello world"
```

| Method | Returns | Description |
|--------|---------|-------------|
| `Buffer.new()` | `Buffer` | Empty buffer |
| `Buffer.from(data: []u8)` | `Buffer` | Initialized with data, position at 0 |
| `as_bytes(self)` | `[]u8` | View of all written bytes |
| `len(self)` | `usize` | Total bytes written |
| `reset(self)` | `()` | Reset read position to 0 |

## Convenience Functions

| Rule | Description |
|------|-------------|
| **C1: No handles** | `io.read_line()` and `io.copy()` avoid ceremony of acquiring stream handles |

| Function | Signature | Description |
|----------|-----------|-------------|
| `io.read_line()` | `-> string or IoError` | Read one line from stdin (strips newline) |
| `io.copy(reader, writer)` | `-> usize or IoError` | Copy all bytes, returns total copied |

## Trait Implementations Summary

| Type | Reader | Writer | Linear |
|------|--------|--------|--------|
| `Stdin` | Yes | -- | Yes |
| `Stdout` | -- | Yes | Yes |
| `Stderr` | -- | Yes | Yes |
| `File` (from `fs`) | Yes | Yes | Yes |
| `Buffer` | Yes | Yes | No |
| `BufReader<R>` | Yes | -- | Inherits from R |
| `BufWriter<W>` | -- | Yes | Inherits from W |

## Error Messages

```
ERROR [std.io/S1]: standard stream not consumed
   |
3  |  const stdout = io.stdout()
   |        ^^^^^^ `Stdout` is a @resource that must be closed

WHY: Standard streams are linear resources to prevent accidental close or leak.

FIX: Add `ensure stdout.close()` after acquiring the handle.
```

```
ERROR [std.io/R3]: invalid UTF-8
   |
5  |  const text = try file.read_text()
   |                   ^^^^^^^^^^^^^^^^ stream contains invalid UTF-8

WHY: read_text() requires valid UTF-8. Use read_all() for raw bytes.
```

## Edge Cases

| Case | Behavior | Rule |
|------|----------|------|
| Read from closed stream | `IoError.Other("stream closed")` | E1 |
| Write to broken pipe | `IoError.BrokenPipe` | E1 |
| `read_text` with invalid UTF-8 | `IoError.Other("invalid UTF-8")` | R3 |
| `read_exact` on short stream | `IoError.UnexpectedEof` | R4 |
| `io.copy` with aliased reader/writer | Undefined (caller must not alias) | C1 |
| `BufWriter` flush on drop fails | Error silently discarded | B2 |
| `Stdout` not closed | Compile error | S1 |
| `Buffer` overflow | Grows like `Vec` (fallible allocation) | B4 |
| Zero-length read/write | Returns `Ok(0)`, no-op | R1, W1 |

---

## Appendix (non-normative)

### Rationale

**S1 (linear streams):** Standard streams are process-global resources. Linear typing prevents accidentally closing stdout mid-program, multiple closers racing, or silent leaks in long-running processes.

**B2 (flush on drop):** Best-effort flush on drop avoids silent data loss in the common case. For critical writes, call `flush()` explicitly and handle the error.

**C1 (convenience functions):** `io.read_line()` covers the "ask user for input" case without acquiring a Stdin handle. Most simple programs don't need direct stream access.

### Thread Safety

| Type | `Send` | `Sync` |
|------|--------|--------|
| `Stdin` | Yes | No |
| `Stdout` | Yes | No |
| `Stderr` | Yes | No |
| `Buffer` | Yes | No |
| `BufReader<R>` | if R: Send | No |
| `BufWriter<W>` | if W: Send | No |

### See Also

- `std.fs` — `File` type implementing Reader/Writer
- `std.net` — `TcpConnection` will implement Reader/Writer
- `mem.resource-types` — `@resource` and `ensure` semantics
- `type.errors` — `T or E` result pattern
