# Standard Library Overview

Foundational types and modules for systems programming.

---

## Design Philosophy

**Batteries included.** HTTP servers, JSON parsing, CLI tools — all built-in.

**Pay for what you use.** Dead code elimination is aggressive — unused modules don't bloat binaries.

**Timeless standards.** JSON (RFC 8259), HTTP (RFC 7230), Base64 (RFC 4648) — stable protocols only.

**Mechanical, not opinionated.** Implements protocols and formats, not frameworks. `http.Server` handles requests; routing/middleware live in packages.

**Linear resources for I/O.** File handles, sockets, system resources are linear types — must be consumed exactly once. Prevents leaks by construction.

**Fallible operations.** Operations that can fail return `Result`. No hidden exceptions.

**Transparent costs.** Allocations, I/O, syscalls — visible in code.

---

## Module Organization

### Core & Collections
| Module | Purpose | Status |
|--------|---------|--------|
| [core](#core) | Primitives, traits, Option, Result | Specified |
| [collections](collections.md) | Vec, Map, Pool | Specified |
| [string](strings.md) | String types | Specified |
| [iteration](iteration.md) | Collection iteration | Specified |

### I/O & Filesystem
| Module | Purpose | Status |
|--------|---------|--------|
| [io](io.md) | Reader, Writer, Buffer traits | Specified |
| [fs](fs.md) | File operations | Specified |
| [path](path.md) | Path manipulation | Specified |

### Networking & Web
| Module | Purpose | Status |
|--------|---------|--------|
| [net](#net) | TCP/UDP sockets | Planned |
| [http](#http) | HTTP client and server | Planned |
| [tls](#tls) | TLS/SSL connections | Planned |
| [url](#url) | URL parsing | Planned |

### Data Formats
| Module | Purpose | Status |
|--------|---------|--------|
| [json](json.md) | JSON parsing and serialization | Specified |
| [csv](#csv) | CSV parsing and writing | Planned |
| [encoding](#encoding) | Base64, hex, URL encoding | Planned |

### Utilities
| Module | Purpose | Status |
|--------|---------|--------|
| [cli](cli.md) | Command-line argument parsing | Specified |
| [time](#time) | Duration, Instant, timestamps | Specified |
| [os](os.md) | Platform-specific operations | Specified |
| [fmt](fmt.md) | String formatting | Specified |
| [math](math.md) | Mathematical functions | Specified |
| [random](random.md) | Random number generation | Specified |
| [hash](#hash) | SHA256, MD5, CRC32 | Planned |
| [bits](#bits) | Bit manipulation utilities | Planned |
| [unicode](#unicode) | Unicode utilities | Planned |
| [terminal](#terminal) | ANSI colors, terminal detection | Planned |

### Concurrency & Testing
| Module | Purpose | Status |
|--------|---------|--------|
| [sync](#sync) | Synchronization primitives | Specified ([concurrency/sync.md](../concurrency/sync.md)) |
| [testing](testing.md) | Test framework | Specified |

---

## Prelude (Built-in)

Always available without import:

### Primitives

| Type | Description |
|------|-------------|
| `i8`, `i16`, `i32`, `i64`, `i128` | Signed integers |
| `u8`, `u16`, `u32`, `u64`, `u128` | Unsigned integers |
| `isize`, `usize` | Pointer-sized integers |
| `f32`, `f64` | Floating point |
| `bool` | Boolean |
| `char` | Unicode scalar value |

### Core Types

| Type | Description |
|------|-------------|
| `Option<T>` | Optional value (`some(v)` or `none`) |
| `Result<T, E>` | Success or error |
| `Error` | Error trait |

### Collections

| Type | Description |
|------|-------------|
| `Vec<T>` | Growable array |
| `Map<K, V>` | Key-value map |
| `Pool<T>` | Handle-based sparse storage |
| `Handle<T>` | Opaque identifier into Pool |

### Strings

| Type | Description |
|------|-------------|
| `string` | UTF-8 owned string |
| `string_view` | Stored indices into string |
| `string_builder` | Growable string buffer |

### Functions

| Function | Description |
|----------|-------------|
| `print(...)` | Print to stdout |
| `panic(msg)` | Terminate with message |

### Traits

| Trait | Description |
|-------|-------------|
| `Copy` | Implicitly copyable (≤16 bytes) |
| `Clone` | Explicitly cloneable |
| `Display` | Human-readable formatting |
| `Debug` | Debug formatting |
| `Eq`, `Ord` | Equality, ordering |
| `Hash` | Hashable |
| `Iterator` | Iteration protocol |

---

## Requires Import

All other modules require explicit import:

```rask
import fs
import net
import time
import io

const file = try fs.open("data.txt")
```

---

## Core

Fundamental types and traits. Everything in core is in the prelude.

See:
- [types/primitives.md](../types/primitives.md) — Primitive types
- [types/optionals.md](../types/optionals.md) — Option<T>
- [types/error-types.md](../types/error-types.md) — Result, Error trait
- [types/traits.md](../types/traits.md) — Trait definitions

---

## IO

Traits for reading and writing byte streams. See [io.md](io.md).

### Types

| Type | Description | Linear? |
|------|-------------|---------|
| `Reader` | Trait for reading bytes | -- |
| `Writer` | Trait for writing bytes | -- |
| `Buffer` | In-memory byte buffer | No |
| `BufReader<R>` | Buffered reader wrapper | Inherits from R |
| `BufWriter<W>` | Buffered writer wrapper | Inherits from W |
| `Stdin` | Standard input handle | Yes |
| `Stdout` | Standard output handle | Yes |
| `Stderr` | Standard error handle | Yes |

### Core Traits

```rask
trait Reader {
    func read(self, buf: []u8) -> usize or IoError
    func read_all(self) -> []u8 or IoError
    func read_to_string(self) -> string or IoError
    func read_exact(self, buf: []u8) -> () or IoError
}

trait Writer {
    func write(self, data: []u8) -> usize or IoError
    func write_all(self, data: []u8) -> () or IoError
    func flush(self) -> () or IoError
}
```

### Standard Streams

```rask
import io

const stdin = io.stdin()     // Stdin (@resource, implements Reader)
const stdout = io.stdout()   // Stdout (@resource, implements Writer)
const stderr = io.stderr()   // Stderr (@resource, implements Writer)
```

### Convenience

```rask
const line = try io.read_line()             // Read line from stdin
const n = try io.copy(reader, writer)       // Copy all bytes between streams
```

**Status:** Specified — see [io.md](io.md). Interpreter has `io.read_line()` only.

---

## FS

File system operations.

### Types

| Type | Description | Linear? |
|------|-------------|---------|
| `File` | Open file handle | Yes |
| `DirEntry` | Directory entry | No |
| `Metadata` | File metadata | No |
| `OpenOptions` | File open configuration | No |

### File Operations

| Function | Signature | Description |
|----------|-----------|-------------|
| `fs.open(path)` | `(read string) -> File or IoError` | Open for reading |
| `fs.create(path)` | `(read string) -> File or IoError` | Create/truncate |
| `fs.open_with(path, opts)` | `(...) -> File or IoError` | Open with options |

### File Handle (Linear)

```rask
// File is linear — must be closed
const file = try fs.open("data.txt")
ensure file.close()

const data = try file.read_all()
try process(data)
```

| Method | Signature | Description |
|--------|-----------|-------------|
| `.read(buf)` | `([]u8) -> usize or IoError` | Read bytes |
| `.read_all()` | `() -> []u8 or IoError` | Read entire file |
| `.write(data)` | `(read []u8) -> usize or IoError` | Write bytes |
| `.close()` | `() -> () or IoError` | Close handle (consumes) |
| `.metadata()` | `() -> Metadata or IoError` | Get file info |

### Directory Operations

| Function | Description |
|----------|-------------|
| `fs.read_dir(path)` | List directory contents |
| `fs.create_dir(path)` | Create directory |
| `fs.create_dir_all(path)` | Create directory tree |
| `fs.remove_file(path)` | Delete file |
| `fs.remove_dir(path)` | Delete empty directory |
| `fs.remove_dir_all(path)` | Delete directory tree |
| `fs.rename(from, to)` | Rename/move |
| `fs.copy(from, to)` | Copy file |
| `fs.exists(path)` | Check existence |
| `fs.metadata(path)` | Get metadata without opening |

**Status:** Specified — see [fs.md](fs.md).

---

## Net

Networking primitives.

### Types

| Type | Description | Linear? |
|------|-------------|---------|
| `TcpListener` | TCP server socket | Yes |
| `TcpStream` | TCP connection | Yes |
| `UdpSocket` | UDP socket | Yes |
| `IpAddr` | IP address (v4/v6) | No |
| `SocketAddr` | IP address + port | No |

### TCP Server

```rask
import net

const listener = try net.tcp_listen("0.0.0.0:8080")
ensure listener.close()

loop {
    const (stream, addr) = try listener.accept()
    spawn {
        ensure stream.close()
        try handle_connection(stream)
    }.detach()
}
```

### TCP Client

```rask
const stream = try net.tcp_connect("example.com:80")
ensure stream.close()

try stream.write_all(request)
const response = try stream.read_all()
```

**Status:** Specified — see [io.md](io.md).

---

## Time

Time-related types and functions.

### Types

| Type | Description | Size |
|------|-------------|------|
| `Duration` | Time span (nanoseconds) | 8 bytes |
| `Instant` | Monotonic timestamp | 8 bytes |
| `SystemTime` | Wall-clock timestamp | 16 bytes |

### Duration

```rask
const d = Duration.seconds(5)
const d = Duration.millis(100)
const d = Duration.nanos(1_000_000)

d.as_secs()    // -> u64
d.as_millis()  // -> u64
d.as_nanos()   // -> u128
```

### Instant (Monotonic Clock)

```rask
const start = time.now()
expensive_operation()
const elapsed = time.now() - start

if elapsed > Duration.seconds(1) {
    log("slow operation")
}
```

### Sleep

```rask
time.sleep(Duration.millis(100))
```

**Status:** Specified — see [time.md](time.md) for full specification.

---

## Path

Cross-platform path manipulation.

### Type

| Type | Description |
|------|-------------|
| `Path` | Immutable path (owned string internally) |

### Operations

```rask
import path

const p = Path.new("/home/user/file.txt")
p.parent()      // -> Option<Path>  "/home/user"
p.file_name()   // -> Option<string>  "file.txt"
p.extension()   // -> Option<string>  "txt"
p.stem()        // -> Option<string>  "file"
p.is_absolute() // -> bool

const p2 = p.join("subdir")  // "/home/user/file.txt/subdir"
```

**Status:** Specified — see [path.md](path.md) for full specification.

---

## OS

Platform-specific operations.

### Environment

```rask
import os

os.env("HOME")              // -> Option<string>
os.env_or("PORT", "8080")   // -> string
os.set_env("KEY", "value")
os.args()                   // -> []string
```

### Process

```rask
os.exit(0)
os.getpid()  // -> u32
```

**Status:** Specified — see [os.md](os.md) for full specification.

---

## FMT

String formatting.

### Format Macro

```rask
const s = format!("Hello, {}!", name)
const s = format!("{} + {} = {}", a, b, a + b)
const s = format!("{:08x}", value)  // Hex with padding
```

### Format Specifiers

| Specifier | Description |
|-----------|-------------|
| `{}` | Display trait |
| `{:?}` | Debug trait |
| `{:x}` | Hex lowercase |
| `{:X}` | Hex uppercase |
| `{:b}` | Binary |
| `{:e}` | Scientific |
| `{:>10}` | Right-align, width 10 |
| `{:0>10}` | Zero-pad, width 10 |

**Status:** Specified — see [fmt.md](fmt.md).

---

## Math

Mathematical functions.

### Functions

| Function | Description |
|----------|-------------|
| `math.abs(x)` | Absolute value |
| `math.min(a, b)` | Minimum |
| `math.max(a, b)` | Maximum |
| `math.clamp(x, lo, hi)` | Clamp to range |
| `math.sqrt(x)` | Square root |
| `math.pow(x, n)` | Power |
| `math.log(x)` | Natural log |
| `math.sin(x)`, `cos`, `tan` | Trigonometry |
| `math.floor(x)`, `ceil`, `round` | Rounding |

### Constants

| Constant | Value |
|----------|-------|
| `math.PI` | 3.14159... |
| `math.E` | 2.71828... |
| `math.INF` | Infinity |
| `math.NAN` | Not a number |

**Status:** Specified — see [math.md](math.md) for full specification.

---

## Random

Random number generation.

### Types

| Type | Description |
|------|-------------|
| `Rng` | Random number generator |

### Usage

```rask
import random

const rng = random.new()           // System-seeded
const rng = random.from_seed(42)   // Deterministic

rng.u64()           // -> u64
rng.range(0, 100)   // -> i64 in [0, 100)
rng.f64()           // -> f64 in [0.0, 1.0)
rng.bool()          // -> bool
rng.shuffle(vec)    // In-place shuffle
rng.choice(vec)     // -> Option<T>
```

**Status:** Specified — see [random.md](random.md) for full specification.

---

## JSON

JSON parsing and serialization (RFC 8259).

### Functions

| Function | Signature | Description |
|----------|-----------|-------------|
| `json.parse(s)` | `(read string) -> JsonValue or JsonError` | Parse JSON string |
| `json.stringify(v)` | `(read JsonValue) -> string` | Serialize to JSON |
| `json.stringify_pretty(v)` | `(read JsonValue) -> string` | Serialize with indentation |

### Types

```rask
enum JsonValue {
    Null,
    Bool(bool),
    Number(f64),
    String(string),
    Array(Vec<JsonValue>),
    Object(Map<string, JsonValue>),
}
```

### Usage

```rask
import json

const data = try json.parse(input)
match data {
    JsonValue.Object(obj) => {
        const name = try obj["name"]
    }
    _ => return Err(InvalidFormat)
}

const output = json.stringify(data)
```

### Typed Serialization

```rask
// Types implementing Serialize/Deserialize traits
const user: User = try json.decode(input)
const output = json.encode(user)
```

**Status:** Specified — see [json.md](json.md) for full specification.

---

## HTTP

HTTP client and server (RFC 7230-7235).

### Types

| Type | Description | Linear? |
|------|-------------|---------|
| `Request` | HTTP request (method, path, headers, body) | No |
| `Response` | HTTP response (status, headers, body) | No |
| `Server` | HTTP server listener | Yes |
| `Client` | HTTP client | No |
| `Headers` | Header collection | No |

### Server

```rask
import http

const server = try http.Server.listen(":8080")
ensure server.close()

loop {
    const (req, resp) = try server.accept()

    if req.method == "GET" && req.path == "/health" {
        try resp.status(200).body("OK").send()
    } else {
        try resp.status(404).send()
    }
}
```

### Client

```rask
import http

const client = http.Client.new()

const resp = try client.get("https://api.example.com/data")
const body = try resp.body_string()

// With headers
const resp = try client.post("https://api.example.com/submit")
    .header("Content-Type", "application/json")
    .body(json_data)
    .send()
```

### Request/Response

| Field | Type | Description |
|-------|------|-------------|
| `req.method` | `string` | GET, POST, etc. |
| `req.path` | `string` | Request path |
| `req.headers` | `Headers` | Request headers |
| `req.body` | `[]u8` | Request body |
| `resp.status` | `u16` | Status code |
| `resp.headers` | `Headers` | Response headers |

**Status:** Planned — detailed specification TODO.

---

## TLS

TLS/SSL connections (wraps system TLS library).

### Types

| Type | Description | Linear? |
|------|-------------|---------|
| `TlsStream` | Encrypted TCP stream | Yes |
| `TlsListener` | TLS server listener | Yes |
| `TlsConfig` | TLS configuration | No |

### Client Connection

```rask
import tls

const stream = try tls.connect("example.com:443")
ensure stream.close()

try stream.write_all(request)
const response = try stream.read_all()
```

### Server

```rask
import tls

const config = tls.Config.new()
const config = try config.cert_file("server.crt")
const config = try config.key_file("server.key")

const listener = try tls.listen(":443", config)
ensure listener.close()

loop {
    const stream = try listener.accept()
    // handle encrypted connection
}
```

**Status:** Planned — detailed specification TODO.

---

## CLI

Command-line argument parsing.

### Basic Usage

```rask
import cli

const args = cli.parse()

const verbose = args.flag("verbose", "v")      // --verbose or -v
const output = try args.option("output", "o")     // --output=file or -o file
const files = args.positional()                // remaining args
```

### Structured Parsing

```rask
import cli

struct Args {
    verbose: bool,
    output: Option<string>,
    files: Vec<string>,
}

let args: Args = try cli.parse_into()
```

### Help Generation

```rask
const parser = cli.Parser.new("myapp")
    .version("1.0.0")
    .description("My application")
    .flag("verbose", "v", "Enable verbose output")
    .option("output", "o", "Output file")
    .positional("files", "Input files")

const args = try parser.parse()
```

**Status:** Specified — see [cli.md](cli.md) for full specification.

---

## Encoding

Common encodings (RFC 4648).

### Base64

```rask
import encoding

const encoded = encoding.base64.encode(data)      // -> string
const decoded = try encoding.base64.decode(encoded)  // -> []u8

// URL-safe variant
const encoded = encoding.base64url.encode(data)
```

### Hex

```rask
const hex = encoding.hex.encode(data)      // -> string "48656c6c6f"
const data = try encoding.hex.decode(hex)     // -> []u8
```

### URL Encoding

```rask
const encoded = encoding.url.encode("hello world")  // "hello%20world"
const decoded = try encoding.url.decode(encoded)       // "hello world"
```

**Status:** Planned — detailed specification TODO.

---

## Hash

Hash functions for integrity (not security).

### Functions

| Function | Output | Use Case |
|----------|--------|----------|
| `hash.sha256(data)` | `[32]u8` | Content addressing, integrity |
| `hash.sha1(data)` | `[20]u8` | Git compatibility (legacy) |
| `hash.md5(data)` | `[16]u8` | Checksums (legacy) |
| `hash.crc32(data)` | `u32` | Fast checksums |

### Usage

```rask
import hash

const digest = hash.sha256(file_contents)
const hex = encoding.hex.encode(digest)

// Incremental hashing
const hasher = hash.Sha256.new()
hasher.update(chunk1)
hasher.update(chunk2)
const digest = hasher.finish()
```

**Note:** For cryptographic security (HMAC, signatures), use the `crypto` package.

**Status:** Planned — detailed specification TODO.

---

## URL

URL parsing (RFC 3986).

### Types

```rask
struct Url {
    scheme: string,      // "https"
    host: string,        // "example.com"
    port: Option<u16>,   // 443
    path: string,        // "/api/users"
    query: Option<string>, // "page=1&limit=10"
    fragment: Option<string>, // "section"
}
```

### Parsing

```rask
import url

const u = try url.parse("https://example.com:8080/path?query=1")

u.scheme    // "https"
u.host      // "example.com"
u.port      // Some(8080)
u.path      // "/path"
u.query     // Some("query=1")
```

### Query Parameters

```rask
const params = try url.parse_query("name=Alice&age=30")
params["name"]  // Some("Alice")

const query = url.encode_query([("name", "Alice"), ("age", "30")])
// "name=Alice&age=30"
```

### Construction

```rask
const u = Url {
    scheme: "https",
    host: "api.example.com",
    path: "/users",
    ..Url.default()
}
u.to_string()  // "https://api.example.com/users"
```

**Status:** Planned — detailed specification TODO.

---

## Unicode

Unicode utilities beyond basic string operations.

### Character Properties

```rask
import unicode

unicode.is_letter('A')      // true
unicode.is_digit('5')       // true
unicode.is_whitespace(' ')  // true
unicode.is_uppercase('A')   // true
unicode.is_lowercase('a')   // true
```

### Case Conversion

```rask
unicode.to_uppercase('a')   // 'A'
unicode.to_lowercase('A')   // 'a'
unicode.to_titlecase('a')   // 'A'
```

### Normalization

```rask
const nfc = unicode.normalize_nfc(text)   // Canonical composition
const nfd = unicode.normalize_nfd(text)   // Canonical decomposition
```

### Categories

```rask
unicode.category('A')  // Category.UppercaseLetter
unicode.category('5')  // Category.DecimalNumber
unicode.category(' ')  // Category.SpaceSeparator
```

**Status:** Planned — detailed specification TODO.

---

## Terminal

Terminal utilities and ANSI styling.

### Colors

```rask
import terminal

print(terminal.red("Error: ") + message)
print(terminal.green("Success"))
print(terminal.bold(terminal.blue("Header")))
```

### Styles

| Function | Description |
|----------|-------------|
| `terminal.bold(s)` | Bold text |
| `terminal.dim(s)` | Dimmed text |
| `terminal.italic(s)` | Italic text |
| `terminal.underline(s)` | Underlined text |

### Colors

| Function | Description |
|----------|-------------|
| `terminal.red(s)` | Red foreground |
| `terminal.green(s)` | Green foreground |
| `terminal.blue(s)` | Blue foreground |
| `terminal.yellow(s)` | Yellow foreground |
| `terminal.rgb(s, r, g, b)` | Custom RGB color |

### Detection

```rask
if terminal.is_tty() {
    print(terminal.green("colored"))
} else {
    print("plain")
}

terminal.width()   // -> Option<u16>
terminal.height()  // -> Option<u16>
```

**Status:** Planned — detailed specification TODO.

---

## CSV

CSV parsing and writing (RFC 4180).

### Reading

```rask
import csv

const reader = csv.Reader.from_string(data)
for row in reader {
    const name = row[0]
    const age = row[1]
}

// With headers
const reader = csv.Reader.from_string(data).with_headers()
for row in reader {
    const name = try row["name"]
    const age = try row["age"]
}
```

### Writing

```rask
const writer = csv.Writer.new()
try writer.write_row(["name", "age"])
try writer.write_row(["Alice", "30"])
const output = writer.finish()
```

### Options

```rask
const reader = csv.Reader.from_string(data)
    .delimiter(';')
    .quote('"')
    .has_headers(true)
```

**Status:** Planned — detailed specification TODO.

---

## Bits

Bit manipulation utilities and binary parsing helpers.

See [bits.md](bits.md) for full specification.

### Bit Operations

Methods on integer types: `popcount()`, `leading_zeros()`, `trailing_zeros()`, `reverse_bits()`, `rotate_left(n)`, `rotate_right(n)`, `swap_bytes()`.

### Byte Order

Methods for endianness: `to_be_bytes()`, `to_le_bytes()`, `from_be_bytes()`, `from_le_bytes()`.

### Binary Parsing

For one-off parsing without `@binary` structs:

```rask
let (magic, version, length, rest) = try data.unpack(u32be, u8, u16be)
```

See also [Binary Structs](../types/binary.md) for declarative binary layouts.

**Status:** Specified — see [bits.md](bits.md).

---

## Explicitly Out of Scope

The following are **not** part of stdlib — use packages:

| Category | Reason |
|----------|--------|
| Web frameworks | Routing, middleware, templates are opinionated |
| XML/YAML/TOML | Format opinions (JSON covers web interchange) |
| Database drivers | External dependencies (SQLite, PostgreSQL) |
| Full cryptography | AES, RSA, ECDSA need expert maintenance |
| GUI | Platform-specific, large |
| Regex | Complex engine, multiple implementations |
| Compression | gzip, zstd, lz4 — specialized |
| Serialization frameworks | MessagePack, Protocol Buffers — opinionated |
| Image/Audio/Video | Media processing — large, specialized |

**Distinction:** Stdlib provides **protocols and formats** (HTTP, JSON, TCP). Packages provide **frameworks and solutions** (web routers, ORMs, media codecs).

---

## See Also

- [collections.md](collections.md) — Vec, Map
- [strings.md](strings.md) — String types
- [iteration.md](iteration.md) — Collection iteration
- [testing.md](testing.md) — Test framework
- [memory/pools.md](../memory/pools.md) — Pool and Handle
- [memory/resource-types.md](../memory/resource-types.md) — Resource type semantics (linear resources)
- [control/ensure.md](../control/ensure.md) — Cleanup mechanism
- [concurrency/README.md](../concurrency/README.md) — Concurrency primitives

---

## Remaining Issues

### High Priority — Core Functionality
1. **IO module** — Reader/Writer traits
2. **FS module** — File operations
3. **HTTP module** — Client and server
4. **JSON module** — Parse/stringify

### Medium Priority — Networking & Data
5. **Net module** — TCP/UDP sockets
6. **TLS module** — Secure connections
7. **URL module** — URL parsing
8. **CLI module** — Argument parsing
9. **Encoding module** — Base64, hex

### Lower Priority — Utilities
10. **Time module** — Duration, Instant
11. **Path module** — Path manipulation
12. **Hash module** — SHA256, CRC32
13. **Math module** — Math functions
14. **Random module** — RNG
15. **FMT module** — Format strings
16. **CSV module** — Tabular data
17. **Bits module** — Bit manipulation
18. **Unicode module** — Unicode utilities
19. **Terminal module** — ANSI colors
