<!-- id: std.net -->
<!-- status: decided -->
<!-- summary: TCP/UDP networking and DNS resolution with linear resource handles and string addresses -->
<!-- depends: stdlib/io.md, memory/resource-types.md -->

# Net

TCP and UDP networking with DNS resolution. String addresses, linear resource handles. HTTP protocol support is in `std.http`.

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

## UDP

| Rule | Description |
|------|-------------|
| **U1: UdpSocket** | Connectionless datagram socket. Linear resource — must close |
| **U2: Bind** | `net.udp_bind(addr)` creates a socket bound to a local address |
| **U3: Connect** | `udp.connect(addr)` sets a default peer for `send`/`recv` — no actual handshake |

<!-- test: skip -->
```rask
@resource
struct UdpSocket { }

net.udp_bind(addr: string) -> UdpSocket or IoError
```

<!-- test: skip -->
```rask
extend UdpSocket {
    func send_to(self, data: []u8, addr: string) -> usize or IoError
    func recv_from(self, buf: []u8) -> (usize, string) or IoError
    func connect(self, addr: string) -> () or IoError
    func send(self, data: []u8) -> usize or IoError     // to connected peer
    func recv(self, buf: []u8) -> usize or IoError      // from connected peer
    func local_addr(self) -> string
    func close(take self)
}
```

<!-- test: skip -->
```rask
import net

const socket = try net.udp_bind("0.0.0.0:9000")
ensure socket.close()

let buf = [0u8; 1024]
const (n, sender) = try socket.recv_from(buf)
try socket.send_to(buf[0..n], sender)
```

## DNS

| Rule | Description |
|------|-------------|
| **D1: Resolve** | `net.resolve(host)` returns IP address strings for a hostname |
| **D2: No caching** | The stdlib does not cache DNS results. OS resolver caching applies |

<!-- test: skip -->
```rask
net.resolve(host: string) -> Vec<string> or IoError
```

<!-- test: skip -->
```rask
const addrs = try net.resolve("example.com")
// ["93.184.216.34", "2606:2800:220:1:248:1893:25c8:1946"]
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
| UDP `send`/`recv` without `connect` | `IoError.Other("not connected")` | U3 |
| UDP packet too large for buffer | Truncated, remaining bytes lost | U1 |
| DNS resolution with no results | Empty `Vec` | D1 |
| DNS resolution for IP literal | Returns the IP itself | D1 |

---

## Appendix (non-normative)

### Rationale

**N3 (string addresses):** No `SocketAddr` or `IpAddr` types. Simpler API, and parsing can be added later without breaking changes.

**U1 (UDP as linear resource):** Same reasoning as TCP — sockets are OS resources that must be explicitly closed. UDP's connectionless nature doesn't change the resource obligation.

**D1 (string results):** DNS results are IP address strings, consistent with N3's string address philosophy. Parsing into structured types can be added later.

### Deferred

- Multicast
- Raw sockets
- `SocketAddr` / `IpAddr` types
- TLS / HTTPS
- Unix domain sockets

### See Also

- `std.http` — HTTP/1.1 client and server (application layer)
- `std.io` — `IoError`, `Reader`/`Writer` traits
- `mem.resource-types` — `@resource` and `ensure` semantics
