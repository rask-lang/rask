# Standard Library Overview

Foundational types and modules for systems programming.

---

## Design Philosophy

**Batteries included.** HTTP servers, JSON parsing, CLI tools — all built-in.

**Pay for what you use.** Dead code elimination is aggressive — unused modules don't bloat binaries.

**Timeless standards.** JSON (RFC 8259), HTTP (RFC 7230), Base64 (RFC 4648) — stable protocols only.

**Mechanical, not opinionated.** Implements protocols and formats, not frameworks. `http.Server` handles requests; routing/middleware live in packages.

**Linear resources for I/O.** File handles, sockets, system resources are linear types — must be consumed exactly once. Prevents leaks by construction.

**Fallible operations.** Operations that can fail return `T or E`. No hidden exceptions.

**Transparent costs.** Allocations, I/O, syscalls — visible in code.

**Bytes for machines, graphemes for humans.** Every index and length is a byte
offset; anything a person sees or counts — `width`, `truncate`, `graphemes`, format
padding — works in display columns. Unicode scalars are not a unit and never appear
as one. For ASCII text all three coincide, so correct text handling costs a flag
test ([strings.md](strings.md) `U1`–`U5`).

---

## Module Organization

### Core & Collections
| Module | Purpose | Status |
|--------|---------|--------|
| [core](#core) | Primitives, traits, optionals (`T?`), results (`T or E`) | Specified |
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
| [net](net.md) | TCP/UDP sockets, DNS | Specified |
| [http](http.md) | HTTP client and server | Specified |
| [tls](tls.md) | TLS/SSL connections | Specified |
| [url](url.md) | URL parsing, percent-encoding | Specified |

### Data Formats
| Module | Purpose | Status |
|--------|---------|--------|
| [json](json.md) | JSON encoding and decoding | Specified |
| [encoding](encoding.md) | Encode/Decode traits, field annotations | Specified |
| [csv](csv.md) | CSV reading and writing | Specified |
| [base64](base64.md) | Base64 encoding | Specified |
| [hex](hex.md) | Hex encoding | Specified |

### Utilities
| Module | Purpose | Status |
|--------|---------|--------|
| [cli](cli.md) | Command-line argument parsing | Specified |
| [time](time.md) | Duration, Instant, SystemTime | Specified |
| [os](os.md) | Process, env, subprocess, signals | Specified |
| [fmt](fmt.md) | String formatting | Specified |
| [math](math.md) | Mathematical functions | Specified |
| [random](random.md) | Random number generation | Specified |
| [digest](digest.md) | SHA-256, SHA-1, MD5, CRC32 | Specified |
| [bits](bits.md) | Bit manipulation, byte order, binary pack/unpack | Specified |
| [reflect](reflect.md) | Compile-time type introspection | Specified |
| [terminal](terminal.md) | ANSI styling, terminal detection | Specified |

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
| `T?` | Optional value (present or `none`) |
| `T or E` | Success value or error |
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
| `StringBuilder` | Growable string buffer |

### Functions

| Function | Description |
|----------|-------------|
| `print(...)` | Print to stdout (no newline) |
| `println(...)` | Print to stdout with newline |
| `panic(msg)` | Terminate with message |

### Traits

| Trait | Description |
|-------|-------------|
| `Copy` | Implicitly copyable (≤16 bytes) |
| `Cloneable` | Explicitly cloneable |
| `Equal` | Equality (`==`, `!=`) |
| `Comparable` | Ordering (`<`, `>`, `<=`, `>=`) |
| `Hashable` | Hash-based collections |
| `Displayable` | Human-readable formatting |
| `Debug` | Debug formatting |
| `Default` | Default values |
| `Numeric` | Arithmetic operations |
| `Sequence` | Iteration protocol |

---

## Requires Import

All other modules require explicit import:

```rask
import fs
import net
import time
import io

let file = try fs.open("data.txt")
```

---

## Core

Fundamental types and traits. Everything in core is in the prelude. See [types/primitives.md](../types/primitives.md), [types/optionals.md](../types/optionals.md), [types/error-types.md](../types/error-types.md), [types/traits.md](../types/traits.md).

---

## IO

Reader/Writer traits, buffered I/O, standard streams. See [io.md](io.md).

---

## FS

File operations (open, read, write, directory listing, metadata). `File` is a linear resource. See [fs.md](fs.md).

---

## Net

Networking primitives.

### Types

| Type | Description | Linear? |
|------|-------------|---------|
| `TcpListener` | TCP server socket | Yes |
| `TcpConnection` | TCP connection | Yes |
| `UdpSocket` | UDP socket | Yes |

Addresses are plain strings — no `SocketAddr`/`IpAddr` types.

### TCP Server

```rask
import net

let listener = try net.tcp_listen("0.0.0.0:8080")
ensure listener.close()

loop {
    let conn = try listener.accept()
    spawn {
        ensure conn.close()
        try handle_connection(conn)
    }.detach()
}
```

### TCP Client

```rask
let conn = try net.tcp_connect("example.com:80")
ensure conn.close()

try conn.write_text(request)
let response = try conn.read_text()
```

**Status:** Specified — see [net.md](net.md).

---

## Time

Duration, Instant (monotonic), SystemTime (wall-clock), Duration arithmetic. See [time.md](time.md).

---

## Path

Cross-platform path manipulation (parent, extension, join). See [path.md](path.md).

---

## OS

Environment variables, process exit, args, subprocess spawning, signal handling. See [os.md](os.md).

---

## FMT

String formatting with format specifiers. See [fmt.md](fmt.md).

---

## Math

Mathematical functions (abs, sqrt, sin, etc.) and constants (PI, E). See [math.md](math.md).

---

## Random

Random number generation (seeded, range, shuffle). See [random.md](random.md).

---

## JSON

JSON parsing and serialization (RFC 8259), typed encode/decode. See [json.md](json.md).

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

let server = try http.Server.listen(":8080")
ensure server.close()

loop {
    let (req, resp) = try server.accept()

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

let client = http.Client.new()

let resp = try client.get("https://api.example.com/data")
let body = try resp.body_string()

// With headers
let resp = try client.post("https://api.example.com/submit")
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

**Status:** Specified — see [http.md](http.md).

---

## TLS

Encrypted connections over TCP. `tls.connect(addr)` to dial, `tls.wrap(own conn, host:)`
to upgrade an existing connection. Verification is on by default and turning it off
is explicit at the call site. See [tls.md](tls.md).

---

## CLI

Command-line argument parsing (flags, options, positional args, help generation). See [cli.md](cli.md).

---

## Encoding

`Encode`/`Decode` traits and comptime field iteration — how a struct becomes any
format. See [encoding.md](encoding.md).

Binary-to-text encodings are their own modules, because they do a different job that
happens to share the word: [base64.md](base64.md), [hex.md](hex.md).
Percent-encoding lives in [url.md](url.md), where the grammar it serves is.

---

## Digest

Content hashing — SHA-256, SHA-1, MD5, CRC32, one-shot or incremental. Named
`digest` rather than `hash` because `Hashable.hash()` already means the other kind
of hashing. See [digest.md](digest.md).

---

## URL

Taking a URL apart and putting it back together, plus percent-encoding. `http` keeps
taking plain `string` URLs; `Url` is for when you need the pieces. See [url.md](url.md).

---

## Terminal

ANSI styling as a value — build a `Style`, apply it where you print, and it gates
itself on whether output is a terminal. Plus terminal size and tty detection. See
[terminal.md](terminal.md).

---

## CSV

Typed decode into structs first (`csv.decode<Sale>(text)`), raw rows as the fallback,
streaming for files that don't fit in memory. See [csv.md](csv.md).

---

## Bits

Bit manipulation, byte order, binary parsing. See [bits.md](bits.md).

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
| Unicode collation, locale, IDNA, bidi | Needs the full character database and a locale table. `width`, `graphemes` and `normalized()` cover what ordinary programs hit |
| TUI frameworks | Cursor control, raw mode, event loops — opinions about rendering. `terminal` does styling and size |
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

