# Net Module

Networking primitives. I kept this minimal—just TCP for now, with string addresses instead of SocketAddr structs. Most servers only need listen/accept/read/write/close. More complex networking (UDP, multicast, raw sockets) can come later without breaking anything.

**Design Metrics:**

**Transparency of Cost (TC ≥ 0.80).** Every network operation returns `Result` — errors are visible. TCP connections are linear resources — forgetting to close is a compile-time error.

**Ergonomic Delta (ED ≤ 1.2).** Comparable to Go's `net` package. String addresses, no builder patterns, no trait bounds.

**Use Case Coverage (UCC ≥ 0.80).** Covers TCP servers, TCP clients, HTTP request/response. Enough for the HTTP JSON API server validation program.

## Types

| Type | Description | Linear? |
|------|-------------|---------|
| `TcpListener` | TCP server socket | Yes — must close |
| `TcpConnection` | TCP connection (read/write) | Yes — must close |

No `SocketAddr` or `IpAddr` types — addresses are strings. Simpler, and you can always parse later if needed.

## TCP Server

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

## TCP Client

<!-- test: skip -->
```rask
import net

const conn = try net.tcp_connect("example.com:80")
ensure conn.close()

try conn.write_all("GET / HTTP/1.1\r\nHost: example.com\r\n\r\n")
const response = try conn.read_all()
```

## API

### Module Functions

<!-- test: skip -->
```rask
// Bind and listen on a TCP address.
net.tcp_listen(addr: string) -> TcpListener or IoError

// Connect to a remote TCP address.
net.tcp_connect(addr: string) -> TcpConnection or IoError
```

### TcpListener

<!-- test: skip -->
```rask
extend TcpListener {
    // Accept a new connection. Blocks until one arrives.
    func accept(self) -> TcpConnection or IoError

    // Close the listener. Required — TcpListener is a linear resource.
    func close(take self)
}
```

### TcpConnection

<!-- test: skip -->
```rask
extend TcpConnection {
    // Read all available data as a string.
    func read_all(self) -> string or IoError

    // Write data to the connection.
    func write_all(self, data: string) -> () or IoError

    // Get the remote address as a string (e.g. "192.168.1.1:4321").
    func remote_addr(self) -> string

    // Close the connection. Required — TcpConnection is a linear resource.
    func close(take self)
}
```

### HTTP Convenience Methods

These are on `TcpConnection` directly — no separate HTTP module needed for basic use.

<!-- test: skip -->
```rask
extend TcpConnection {
    // Read and parse an HTTP/1.1 request from the connection.
    func read_http_request(self) -> HttpRequest or IoError

    // Write an HTTP/1.1 response to the connection.
    func write_http_response(self, response: HttpResponse) -> () or IoError
}

struct HttpRequest {
    public method: string    // "GET", "POST", etc.
    public path: string      // "/users/42"
    public headers: Map<string, string>
    public body: string
}

struct HttpResponse {
    public status: i32       // 200, 404, etc.
    public headers: Map<string, string>
    public body: string
}
```

I put HTTP methods on TcpConnection rather than a separate module because the common case is "accept connection, read request, write response, close." A separate `http` module would add ceremony without benefit for simple servers. If someone needs HTTP/2 or websockets, that's a different library.

## Resource Safety

Both `TcpListener` and `TcpConnection` are linear resources. The compiler enforces that they must be consumed (closed) before going out of scope:

<!-- test: skip -->
```rask
func handle(conn: TcpConnection) -> () or Error {
    ensure conn.close()    // guaranteed cleanup
    const req = try conn.read_http_request()
    try conn.write_http_response(response)
}
```

Calling `.close()` on an already-closed connection is a no-op — this supports the `ensure` + explicit close pattern.
