<!-- id: std.url -->
<!-- status: decided -->
<!-- summary: URL parsing and percent-encoding — one Url type, ports default from the scheme -->
<!-- depends: stdlib/strings.md, stdlib/collections.md -->

# URL

Take a URL apart, put one back together, percent-encode the pieces. RFC 3986.

`http` keeps taking plain `string` URLs (`std.http/T1`) — most code passes a URL
through without ever looking inside it, and a required parse step at every call
would be ceremony. `Url` is for when you actually need the parts.

## Core Rules

| Rule | Description |
|------|-------------|
| **U1: One type** | `Url` holds the parsed parts as fields. No separate `Authority`, `Host`, `QueryString` types. Path, query, fragment and user are kept as written, still percent-encoded |
| **U2: Ports default from the scheme** | `port()` answers what you'd connect to — 443 for `https`, 80 for `http`, 22 for `ssh` — not what was literally written. An unknown scheme with no explicit port gives `none` |
| **U3: Percent-encoding lives here** | `url.encode`/`url.decode` are the one place that handles `%XX`. It isn't a serialization format, so it doesn't belong beside `base64`/`hex` |
| **U4: Parsing is fallible, lookup is absent-shaped** | `url.parse` returns `Url or UrlError` — a bad URL is a real error with a reason, and that includes a bad escape anywhere in it. `u.param(key)` returns `string?` — a missing parameter is a non-answer (`std.api/SD3`). Because parse checked every escape, reading parameters can't fail |

## Type

<!-- surface -->
```rask
struct Url {
    scheme: string          // "https"
    host: string            // "example.com"
    explicit_port: u16?     // 8080 — only what was written; port() is what to connect to
    path: string            // "/api/users" — "/" when empty
    query: string?          // "page=1&limit=10" — raw, undecoded
    fragment: string?       // "section"
    user: string?           // rare, but it's in the grammar
}
```

Fields are public and plain. Anything that needs computing is a method.

The written port is a field like every other part, so the URL can be put back
together exactly. It's called `explicit_port` rather than `port` because the name
`port` belongs to the method, which is the one callers want (U2).

## Parsing

<!-- surface -->
```rask
extend url {
    func parse(s: string) -> Url or UrlError
}
```

<!-- test: skip -->
```rask
let u = try url.parse("https://example.com:8080/path?page=1#top")
u.scheme        // "https"
u.host          // "example.com"
u.port()        // 8080
u.path          // "/path"
u.param("page") // "1"
```

## Methods

<!-- surface -->
```rask
extend Url {
    func port(self) -> u16?                             // explicit port, else the scheme's default (U2)
    func param(self, key: string) -> string?            // one parameter, percent-decoded
    func params(self) -> Vec<(string, string)>          // all of them, in order, decoded
    func with_query(self, pairs: Vec<(string, string)>) -> Url
}
Url implements Displayable {
    func display(self) -> string                        // reassembles the URL
}
Url implements Div<string> {
    func div(self, segment: string) -> Url              // append a path segment, encoding it
}
```

`param`/`params` are one name in two numbers. The parameter lookup used to be
`query(key)`, which collided with the `query` field: `u.query` and `u.query("x")`
would have been two different things under one name.

`params()` hands back a `Vec`, the same shape `url.parse_query` returns. A query
string is a handful of pairs, and the old `Iterator` return named a protocol
that's gone (`type.sequence/SEQ20`).

`/` appends a path segment the same way `Path` does (`std.path`), so the two
read alike. Query and fragment stay where they were:

<!-- test: skip -->
```rask
let api = try url.parse("https://api.example.com/v1")
let endpoint = api / "users" / user_id      // ".../v1/users/12%20a"
```

## Percent-Encoding

<!-- surface -->
```rask
extend url {
    func encode(s: string) -> string            // "hello world" -> "hello%20world"
    func decode(s: string) -> string or UrlError

    func encode_query(pairs: Vec<(string, string)>) -> string   // "a=1&b=2"
    func parse_query(s: string) -> Vec<(string, string)> or UrlError
}
```

`+` means a space in a query string and nowhere else, so `parse_query`,
`param` and `params` decode it and `url.decode` leaves it alone. `encode` always
writes a space as `%20`, which both read.

`encode` escapes everything outside the unreserved set — correct for a path
segment, a query value, or a form field, which is every case that comes up. A URL
you assembled yourself is already encoded; don't run it through again.

`encode_query`/`parse_query` work on a bare query string, which is also the
`application/x-www-form-urlencoded` body format. That's why they exist next to
`u.query()` rather than being folded into it — a form body has no URL around it.

Pairs, not a `Map`: query strings can repeat a key (`?tag=a&tag=b`) and order can
matter for signing.

## Errors

<!-- surface -->
```rask
enum UrlError {
    NoScheme                // "example.com/path" — relative URLs aren't parsed
    BadScheme(string)       // scheme has characters the grammar doesn't allow
    BadPort(string)         // ":99999" or ":http"
    BadEscape(string)       // "%zz", or "%" at the end of the input
    BadHost(string)
}
extend UrlError {
    func message(self) -> string
}
```

## Error Messages

```
ERROR [std.url/U4]: URL has no scheme
   |
3  |  let u = try url.parse("example.com/api")
   |                         ^^^^^^^^^^^^^^^^ no "scheme:" prefix

WHY: url.parse takes absolute URLs. Without a scheme there's no way to
     know whether "example.com" is a host or the first path segment.

FIX: Add the scheme:

  let u = try url.parse("https://example.com/api")
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| No path — `"https://example.com"` | U1 | `path` is `"/"` |
| Empty query — `"?"` | U1 | `query` is `none`, not `Some("")` |
| Repeated key — `"?tag=a&tag=b"` | — | `params()` has both; `param("tag")` gives the first |
| Valueless key — `"?debug"` | — | `param("debug")` is `""`, not `none`. It's present |
| Bad escape anywhere — `"http://x/%zz"` | U4 | `BadEscape("%zz")` from `parse` |
| Escape that isn't UTF-8 — `url.decode("%FF")` | U3 | `BadEscape` with the whole input. The result is a `string` |
| `+` in a query | — | A space for `param`/`params`/`parse_query`. `url.decode` keeps it |
| Empty port — `"http://x:/"` | U2 | No port written; `port()` is the default |
| No authority — `"mailto:a@b.c"` | U1 | `host` is `""`, `path` is `"a@b.c"`. `display()` writes no `//` |
| Explicit default port — `"https://x:443/"` | U2 | `port()` is 443. Round-tripping through `display()` drops the redundant `:443` |
| Unknown scheme, no port — `"myapp://x/"` | U2 | `port()` is `none` |
| Uppercase scheme or host | — | Lowercased on parse. Path and query keep their case |
| IPv6 host — `"http://[::1]:80/"` | U1 | `host` is `"::1"`, brackets stripped. `display()` puts them back |
| Non-ASCII host (IDN) | — | Kept as-is, not punycoded. IDNA is a package (`std.strings/U3` reasoning — it needs the full Unicode tables) |
| `%2F` inside a path segment | U3 | Preserved as an escape through parse and `display()` — decoding it would change the path structure |

---

## Appendix (non-normative)

### Rationale

**U2 (ports default from the scheme):** the earlier sketch had `port: u16?` as a
bare field, which pushes `?? 443` to every call site, and half of them will get the
default wrong for a scheme they didn't think about. What callers want is the port
to connect to — that's a question with a real answer, so it's a method that answers
it.

**U3 (percent-encoding here, not in `encoding`):** percent-encoding is a URL
syntax rule, not a binary-to-text format. Putting it beside `base64` and `hex`
would suggest it's interchangeable with them, and it would leave `url` depending on
another module for its own grammar.

**Why `Url` doesn't replace `string` in `http`:** almost every HTTP call site
passes a URL straight through — from config, from a link, from a previous response.
Requiring `try url.parse(...)` at those sites buys nothing and adds an error branch
nobody reads. Parsing is for programs that manipulate URLs, and those reach for it
deliberately.

### See Also

- `std.http` — takes URLs as `string`; this is the type for taking them apart
- `std.path` — same `/` join operator, same "wrapper over a string" shape
- `std.strings` — percent-decoding produces `string`, so the result is UTF-8 validated
