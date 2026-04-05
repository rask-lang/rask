# Tiwaz — Step-by-Step Build Guide

**Tiwaz** (ᛏ) — a reverse proxy written in Rask. Lives at `examples/tiwaz/`.

## Reference files to keep open

- [http_api_server.rk](examples/http_api_server.rk) — closest existing pattern (accept loop, `Shared<T>`, routing)
- [lsm_database/](examples/lsm_database/) — multi-file package structure to copy
- [stdlib/http.rk](stdlib/http.rk) — all HTTP types: `HttpServer`, `Responder`, `Request`, `Response`, `send_request`, `parse_url`
- [stdlib/net.rk](stdlib/net.rk) — TCP primitives

---

## Phase 0 — Passthrough proxy

Goal: listen on `:8080`, forward everything to one backend, return the response.

### Step 1: Scaffold the package

Create `examples/tiwaz/build.rk` — copy the pattern from `lsm_database/build.rk`.

### Step 2: Write the accept loop (`main.rk`)

1. `HttpServer.listen("0.0.0.0:8080")` — bind
2. `ensure server.close()`
3. `using Multitasking` block wrapping a `loop`
4. Inside loop: `server.accept()` → gives you `(Request, Responder)`
5. `spawn` per request, `.detach()` it
6. Inside the spawn: call your `forward()` function, then `responder.respond(response)`
7. Handle the error case — if forwarding fails, respond with `Response.internal_error()`

### Step 3: Write the forwarding function (`proxy.rk`)

`forward(req: Request, upstream: string) -> Response or HttpError`

1. Build the upstream URL: `"http://{upstream}{req.path()}"`
2. Grab the method string from `req.method`
3. Call `http.send_request(method_str, url, req.body, req.headers)` — check what `send_request` actually accepts (read `stdlib/http.rk` line ~585)
4. Return the response

### Step 4: Test it

1. Run the `http_api_server.rk` as your backend on `:3000` (or pick a port)
2. Run tiwaz: `rask run examples/tiwaz/main.rk`
3. `curl http://localhost:8080/health` — should proxy to the backend and return `ok`
4. `curl -X POST http://localhost:8080/users -d '{"name":"test","email":"test@test.com"}'`

### Gaps you'll likely hit

- Does `send_request` forward original headers? Or does it build its own?
- What error do you get when the upstream is unreachable?
- Hop-by-hop headers (Connection, Keep-Alive) — should strip them before forwarding

---

## Phase 1 — Path-based routing

Goal: route `/api/*` to one backend, `/static/*` to another, etc.

### Step 5: Define routes (`config.rk`)

```
struct Route {
    prefix: string
    upstream: string
}
```

Write a `routes() -> Vec<Route>` function that returns your route table. Hardcoded — that's the "compiled config" pitch.

### Step 6: Write the router (`router.rk`)

`match_route(path: string, routes: Vec<Route>) -> Route?`

1. Iterate routes, find first where `path.starts_with(route.prefix)`
2. Return it (or None)

### Step 7: Path rewriting

When `/api/users` matches prefix `/api`, the upstream should receive `/users`. Strip the prefix from the path before forwarding.

### Step 8: Wire it into main.rk

Replace the hardcoded upstream with: match route → forward to matched upstream with rewritten path → 404 if no route matches.

### Step 9: Test with multiple backends

Run two instances of `http_api_server.rk` on different ports. Configure routes pointing to each. Verify requests go to the right place.

### Gaps you'll likely hit

- String slicing for path rewriting (`path[prefix.len()..]`) — does it work cleanly?
- Passing the rewritten path to `forward()` — you'll need to modify the Request or pass the path separately

---

## Phase 2 — Middleware + observability

Goal: wrap handlers with logging and metrics.

### Step 10: Define the middleware pattern (`middleware.rk`)

A middleware takes a handler and returns a new handler:
```
func logging(next: func(Request) -> Response) -> func(Request) -> Response
```

Start with logging: print method, path, status, and duration for each request.

### Step 11: Add metrics (`metrics.rk`)

- `Metrics` struct: request count, error count, bytes proxied
- `Shared<Metrics>` at module level
- Metrics middleware: increment counters on each request
- Add a `/metrics` endpoint that returns the counts (intercept before routing)

### Step 12: Error types (`error.rk`)

`ProxyError` enum with variants for: upstream unreachable, upstream timeout, bad response, no matching route. Convert to appropriate HTTP responses (502, 504, 404).

### Step 13: Chain middleware in main.rk

Build the handler by wrapping: `logging(with_metrics(route_and_forward))`.

### Gaps you'll likely hit

- Can you store `func(Request) -> Response` as a variable and pass it around?
- Closures capturing `Shared<Metrics>` across spawn boundaries
- `time.Instant.now()` + `.elapsed()` — does duration formatting work?

---

## Phase 3 — Health checks + failover

Goal: multiple backends per route, automatic health checking, round-robin.

### Step 14: Backend pools (`backend.rk`)

`BackendPool` with `Vec<Backend>` and a `Mutex<usize>` for round-robin index. `next_healthy() -> string?` picks the next backend that's marked up.

### Step 15: Health checker (`health.rk`)

Background `spawn` that loops forever: for each backend, `http.get(health_url)`. Update `Shared<Map<string, bool>>` with up/down status. Sleep between checks.

### Step 16: Update config and router

Routes now point to `BackendPool` instead of a single address string. Router calls `pool.next_healthy()` and returns 503 if all backends are down.

### Step 17: Test failover

Start two backends. Verify round-robin. Kill one. Verify traffic goes to the healthy one. Restart it. Verify it comes back.

### Gaps you'll likely hit

- `time.sleep()` — does it exist?
- Graceful shutdown of the health checker when the proxy exits
- Shared state contention under load

---

## Testing approach

For all phases: run a backend with `rask run examples/http_api_server.rk`, run tiwaz, and curl through it. You can also use a simple `python3 -m http.server 3000` as a static backend.
