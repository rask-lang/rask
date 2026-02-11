<!-- id: std.net -->
<!-- status: decided -->
<!-- summary: TCP networking with linear resource handles and string addresses -->
<!-- depends: stdlib/io.md, memory/resource-types.md -->

# Net

TCP networking with minimal API. String addresses, linear resource handles, built-in HTTP/1.1 convenience methods.

## Types

| Rule | Description |
|------|-------------|
| **N1: TcpListener** | TCP server socket. Linear resource — must close |
| **N2: TcpConnection** | TCP read/write connection. Linear resource — must close |
| **N3: String addresses** | All addresses are plain strings (e.g. `"0.0.0.0:8080"`). No `SocketAddr` type |

## Module Functions

| Rule | Description |
|------|-------------|
| **N4: Listen** | `net.tcp_listen(addr)` binds and listens on a TCP address |
| **N5: Connect** | `net.tcp_connect(addr)` connects to a remote TCP address |

<!-- test: skip -->
```rask
net.tcp_listen(addr: string) -> TcpListener or IoError
net.tcp_connect(addr: string) -> TcpConnection or IoError
```

## TcpListener

<!-- test: skip -->
```rask
extend TcpListener {
    func accept(self) -> TcpConnection or IoError
    func close(take self)
}
```

<!-- test: skip -->
```rask
import net

const listener = try net.tcp_listen("0.0.0.0:8080")
ensure listener.close()

loop {
    const conn = try listener.accept()
    spawn {
        ensure conn.close()
        try handle(conn)
    }.detach()
}
```

## TcpConnection

<!-- test: skip -->
```rask
extend TcpConnection {
    func read_all(self) -> string or IoError
    func write_all(self, data: string) -> () or IoError
    func remote_addr(self) -> string
    func close(take self)
}
```

<!-- test: skip -->
```rask
import net

const conn = try net.tcp_connect("example.com:80")
ensure conn.close()
try conn.write_all("GET / HTTP/1.1\r\nHost: example.com\r\n\r\n")
const response = try conn.read_all()
```

## HTTP Convenience

| Rule | Description |
|------|-------------|
| **N6: HTTP on connection** | `read_http_request` and `write_http_response` are methods on `TcpConnection` — no separate HTTP module |

<!-- test: skip -->
```rask
extend TcpConnection {
    func read_http_request(self) -> HttpRequest or IoError
    func write_http_response(self, response: HttpResponse) -> () or IoError
}

struct HttpRequest {
    public method: string
    public path: string
    public headers: Map<string, string>
    public body: string
}

struct HttpResponse {
    public status: i32
    public headers: Map<string, string>
    public body: string
}
```

## Resource Safety

| Rule | Description |
|------|-------------|
| **N7: Must consume** | Both `TcpListener` and `TcpConnection` must be closed before scope exit. Compiler rejects unconsumed handles |
| **N8: Double close** | `close()` on already-closed handle is a no-op (supports ensure + explicit close) |

<!-- test: skip -->
```rask
func handle(conn: TcpConnection) -> () or IoError {
    ensure conn.close()
    const req = try conn.read_http_request()
    try conn.write_http_response(response)
}
```

## Error Messages

```
ERROR [std.net/N7]: connection not consumed
   |
3  |  const conn = try listener.accept()
   |        ^^^^ `TcpConnection` is a @resource that must be closed

WHY: Network connections are linear resources to prevent socket leaks.

FIX: Add `ensure conn.close()` after accepting.
```

```
ERROR [std.net/N4]: bind failed
   |
2  |  const listener = try net.tcp_listen("0.0.0.0:8080")
   |                       ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ IoError.Other("address in use")

WHY: Another process is already listening on this address.
```

## Edge Cases

| Case | Behavior | Rule |
|------|----------|------|
| `close()` on already-closed handle | No-op | N8 |
| Connection not closed | Compile error | N7 |
| Invalid address string | `IoError.Other` | N4, N5 |
| Remote closes during read | `IoError.ConnectionReset` or empty result | N2 |
| Accept on closed listener | `IoError.Other("listener closed")` | N1 |

---

## Appendix (non-normative)

### Rationale

**N3 (string addresses):** No `SocketAddr` or `IpAddr` types. Simpler API, and parsing can be added later without breaking changes.

**N6 (HTTP on TcpConnection):** The common case is "accept, read request, write response, close." A separate `http` module adds ceremony without benefit for simple servers. HTTP/2 or websockets would be a different library.

### Deferred

- UDP sockets
- Multicast
- Raw sockets
- `SocketAddr` / `IpAddr` types
- TLS / HTTPS
- HTTP/2, websockets

### See Also

- `std.io` — `IoError`, `Reader`/`Writer` traits
- `std.json` — JSON encoding for HTTP request/response bodies
- `mem.resource-types` — `@resource` and `ensure` semantics
