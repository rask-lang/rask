<!-- id: std.http -->
<!-- status: decided -->
<!-- summary: HTTP/1.1 client and server with typed requests, linear server handles -->
<!-- depends: stdlib/net.md, stdlib/io.md, stdlib/time.md, memory/resource-types.md -->

# HTTP

HTTP/1.1 client and server. Convenience functions for simple requests, `HttpClient` for configuration, `HttpServer` for accept loops with linear response handles.

## Types

| Rule | Description |
|------|-------------|
| **T1: Request** | Method, URL, headers, body. Constructable for client, received from server |
| **T2: Response** | Status code, headers, body. Constructable for server, received from client |
| **T3: Headers** | Case-insensitive header map |
| **T4: Method** | Standard HTTP method enum |

<!-- test: skip -->
```rask
struct Request {
    public method: Method
    public url: string
    public headers: Headers
    public body: string
}

struct Response {
    public status: u16
    public headers: Headers
    public body: string
}

enum Method { Get, Head, Post, Put, Delete, Patch, Options }
```

## Headers

| Rule | Description |
|------|-------------|
| **H1: Case-insensitive** | Header name lookup is case-insensitive (`"Content-Type"` matches `"content-type"`) |
| **H2: Multi-value** | Multiple values for the same header are comma-joined per RFC 7230 |

<!-- test: skip -->
```rask
struct Headers { }

extend Headers {
    func new() -> Headers
    func get(self, name: string) -> string?
    func set(mutate self, name: string, value: string)
    func has(self, name: string) -> bool
    func remove(mutate self, name: string) -> string?
    func len(self) -> usize
    func iter(self) -> Iterator<(string, string)>
}
```

## Request API

<!-- test: skip -->
```rask
extend Request {
    func new(method: Method, url: string) -> Request
    func with_header(take self, name: string, value: string) -> Request
    func with_body(take self, body: string) -> Request

    func path(self) -> string                      // URL path before '?'
    func query_param(self, name: string) -> string? // single query parameter
    func query_params(self) -> Map<string, string>  // all query parameters
}
```

## Response API

| Rule | Description |
|------|-------------|
| **R1: Status helpers** | Named constructors for common status codes |
| **R2: Status categories** | `is_ok()`, `is_error()` etc. check status code ranges |

<!-- test: skip -->
```rask
extend Response {
    // Constructors
    func new(status: u16, body: string) -> Response
    func ok(body: string) -> Response                  // 200
    func json(body: string) -> Response                // 200 + Content-Type: application/json
    func created(body: string) -> Response             // 201
    func no_content() -> Response                      // 204
    func not_found() -> Response                       // 404
    func bad_request(message: string) -> Response      // 400
    func internal_error(message: string) -> Response   // 500
    func redirect(url: string) -> Response             // 302

    // Builder
    func with_status(take self, status: u16) -> Response
    func with_header(take self, name: string, value: string) -> Response

    // Status checks
    func is_ok(self) -> bool            // 200-299
    func is_redirect(self) -> bool      // 300-399
    func is_client_error(self) -> bool  // 400-499
    func is_server_error(self) -> bool  // 500-599
}
```

## Client

| Rule | Description |
|------|-------------|
| **C1: Module convenience** | `http.get()`, `http.post()` etc. for one-off requests. Uses a shared internal client |
| **C2: Full request** | `http.request(req)` for custom method, headers, body |
| **C3: HttpClient** | Configurable client for timeouts, default headers, connection reuse |
| **C4: Not a resource** | `HttpClient` is not `@resource` — pooled connections close on drop |

<!-- test: skip -->
```rask
http.get(url: string) -> Response or HttpError
http.post(url: string, body: string) -> Response or HttpError
http.put(url: string, body: string) -> Response or HttpError
http.delete(url: string) -> Response or HttpError
http.request(request: Request) -> Response or HttpError
```

<!-- test: skip -->
```rask
import http

const resp = try http.get("http://api.example.com/users")
if resp.is_ok() {
    println(resp.body)
}
```

### HttpClient

<!-- test: skip -->
```rask
struct HttpClient { }

extend HttpClient {
    func new() -> HttpClient
    func with_timeout(take self, timeout: Duration) -> HttpClient
    func with_header(take self, name: string, value: string) -> HttpClient
    func with_max_redirects(take self, n: u32) -> HttpClient

    func get(self, url: string) -> Response or HttpError
    func post(self, url: string, body: string) -> Response or HttpError
    func put(self, url: string, body: string) -> Response or HttpError
    func delete(self, url: string) -> Response or HttpError
    func request(self, request: Request) -> Response or HttpError
}
```

<!-- test: skip -->
```rask
import http
import time

const client = HttpClient.new()
    .with_timeout(Duration.seconds(30))
    .with_header("Authorization", "Bearer token123")

const resp = try client.get("http://api.example.com/users")
```

## Server

| Rule | Description |
|------|-------------|
| **S1: HttpServer** | TCP listener that parses HTTP/1.1. Linear resource — must close |
| **S2: Responder** | Linear handle for sending exactly one response per request. Compiler guarantees every accepted request gets a response |
| **S3: Accept loop** | Server yields `(Request, Responder)` pairs. Spawn tasks for concurrent handling |

<!-- test: skip -->
```rask
@resource
struct HttpServer { }

extend HttpServer {
    func listen(addr: string) -> HttpServer or HttpError
    func accept(self) -> (Request, Responder) or HttpError
    func local_addr(self) -> string
    func close(take self)
}

@resource
struct Responder { }

extend Responder {
    func respond(take self, response: Response) -> void or HttpError
}
```

<!-- test: skip -->
```rask
import http

func main() -> void or Error {
    using Multitasking {
        const server = try HttpServer.listen("0.0.0.0:8080")
        ensure server.close()

        loop {
            const (req, responder) = try server.accept()
            spawn(|| {
                ensure responder.respond(Response.internal_error("unhandled"))
                const response = handle(req)
                responder.respond(response)
            }).detach()
        }
    }
}
```

### Convenience Server

| Rule | Description |
|------|-------------|
| **S4: listen_and_serve** | One-line server for simple cases. Spawns a green task per request. Requires `using Multitasking` |

<!-- test: skip -->
```rask
http.listen_and_serve(
    addr: string,
    handler: func(Request) -> Response,
) -> void or HttpError
```

<!-- test: skip -->
```rask
import http

func main() -> void or Error {
    using Multitasking {
        try http.listen_and_serve("0.0.0.0:8080", handle)
    }
}

func handle(req: Request) -> Response {
    return match req.path() {
        "/health" => Response.ok("ok"),
        "/users" => handle_users(req),
        _ => Response.not_found(),
    }
}

func handle_users(req: Request) -> Response {
    if req.method is Method.Get {
        const users = get_all_users()
        return Response.json(json.encode(users))
    }
    if req.method is Method.Post {
        const user = json.decode<User>(req.body) is Ok else {
            return Response.bad_request("invalid json")
        }
        save_user(user)
        return Response.json(json.encode(user)).with_status(201)
    }
    return Response.new(405, "method not allowed")
}
```

## HttpError

<!-- test: skip -->
```rask
enum HttpError {
    ConnectionFailed(string)
    Timeout
    InvalidUrl(string)
    InvalidResponse(string)
    TooManyRedirects
    Io(IoError)
}
```

## Error Messages

```
ERROR [std.http/S2]: responder not consumed
   |
5  |  const (req, responder) = try server.accept()
   |              ^^^^^^^^^ `Responder` is a @resource that must respond

WHY: Every accepted HTTP request must get a response. Responder is linear.

FIX: Add `ensure responder.respond(Response.internal_error("unhandled"))` after accepting.
```

```
ERROR [std.http/C1]: request failed
   |
3  |  const resp = try http.get("http://down.example.com")
   |                   ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ HttpError.ConnectionFailed("connection refused")

WHY: Could not establish TCP connection to the remote host.
```

## Edge Cases

| Case | Behavior | Rule |
|------|----------|------|
| Responder not consumed | Compile error | S2 |
| `ensure` + explicit `respond()` | Safe — ensure's respond is a no-op if already sent | S2 |
| Redirect loop | `HttpError.TooManyRedirects` (default limit: 10) | C1 |
| Invalid URL | `HttpError.InvalidUrl` | C1 |
| Response body too large | Reads fully into memory (no streaming in v1) | T2 |
| Connection timeout | `HttpError.Timeout` (default: 30s for module functions) | C1 |
| Malformed HTTP response | `HttpError.InvalidResponse` | C1 |
| Server accept on closed server | `HttpError.Io(IoError.Other("server closed"))` | S1 |
| Request with no Content-Type | Header absent, body sent as-is | T1 |

---

## Appendix (non-normative)

### Rationale

**S2 (linear Responder):** I wanted the compiler to guarantee every request gets a response. Forgetting to respond is a common server bug — the client hangs, the connection leaks. Making Responder `@resource` catches this at compile time. The `ensure` pattern provides a fallback response for error paths.

**C4 (HttpClient not @resource):** Connection pools are an optimization, not a correctness obligation. Forcing `.close()` on every client adds ceremony for the 99% case. Pooled connections close on drop — this is the one place I'm OK with implicit cleanup because there's no data loss risk.

**C1 (shared internal client):** Module-level functions (`http.get`, etc.) reuse a shared internal client for connection pooling. Without this, every call does a fresh TCP+TLS handshake. Use `HttpClient.new()` when you need isolation or custom settings.

**S4 (listen_and_serve):** Covers the common pattern of "listen, accept in a loop, spawn a task per request." Three lines instead of ten. For anything more complex (graceful shutdown, connection limits), use the `HttpServer` accept loop directly.

### Deferred

- HTTP/2
- WebSockets
- Streaming request/response bodies
- Server-sent events
- Cookie jar
- Proxy support
- Client certificate authentication

### Validation

This spec enables validation target #1 (HTTP JSON API server):

<!-- test: skip -->
```rask
import http
import json

struct User {
    public id: u64
    public name: string
    public email: string
}

func main() -> void or Error {
    using Multitasking {
        try http.listen_and_serve("0.0.0.0:8080", handle)
    }
}

func handle(req: Request) -> Response {
    return match req.path() {
        "/health" => Response.ok("ok"),
        "/users" => match req.method {
            Method.Get => {
                const users = db_list_users()
                Response.json(json.encode(users))
            },
            Method.Post => {
                const user = json.decode<User>(req.body) is Ok else {
                    return Response.bad_request("invalid json")
                }
                const saved = db_create_user(user)
                Response.json(json.encode(saved)).with_status(201)
            },
            _ => Response.new(405, "method not allowed"),
        },
        _ => Response.not_found(),
    }
}
```

### See Also

- `std.net` — TCP/UDP transport layer
- `std.io` — `IoError`, `Reader`/`Writer` traits
- `std.json` — JSON encoding/decoding for request/response bodies
- `std.time` — `Duration` for timeouts
- `mem.resource-types` — `@resource` and `ensure` semantics
