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
| **U1: One type** | `Url` holds the parsed parts as fields. No separate `Authority`, `Host`, `QueryString` types |
| **U2: Ports default from the scheme** | `port()` answers what you'd connect to — 443 for `https`, 80 for `http`, 22 for `ssh` — not what was literally written. An unknown scheme with no explicit port gives `none` |
| **U3: Percent-encoding lives here** | `url.encode`/`url.decode` are the one place that handles `%XX`. It isn't a serialization format, so it doesn't belong beside `base64`/`hex` |
| **U4: Parsing is fallible, lookup is absent-shaped** | `url.parse` returns `Url or UrlError` — a bad URL is a real error with a reason. `u.query(key)` returns `string?` — a missing parameter is a non-answer (`std.api/SD3`) |

## Type

<!-- test: skip -->
```rask
struct Url {
    scheme: string          // "https"
    host: string            // "example.com"
    path: string            // "/api/users" — "/" when empty
    query: string?          // "page=1&limit=10" — raw, undecoded
    fragment: string?       // "section"
    user: string?           // rare, but it's in the grammar
}
```

Fields are public and plain. Anything that needs computing is a method.

## Parsing

<!-- test: skip -->
```rask
url.parse(s: string) -> Url or UrlError

let u = try url.parse("https://example.com:8080/path?page=1#top")
u.scheme        // "https"
u.host          // "example.com"
u.port()        // 8080
u.path          // "/path"
u.query("page") // "1"
```

## Methods

<!-- test: skip -->
```rask
u.port() -> u16?                            // explicit port, else the scheme's default (U2)
u.query(key: string) -> string?             // one parameter, percent-decoded
u.params() -> Iterator<(string, string)>    // all of them, in order, decoded
u.with_query(pairs: Vec<(string, string)>) -> Url
u.display() -> string                       // Displayable — reassembles the URL
u / segment -> Url                          // append a path segment, encoding it
```

`/` appends a path segment the same way `Path` does (`std.path`), so the two
read alike:

<!-- test: skip -->
```rask
let api = try url.parse("https://api.example.com/v1")
let endpoint = api / "users" / user_id      // ".../v1/users/12%20a"
```

## Percent-Encoding

<!-- test: skip -->
```rask
url.encode(s: string) -> string             // "hello world" -> "hello%20world"
url.decode(s: string) -> string or UrlError

url.encode_query(pairs: Vec<(string, string)>) -> string   // "a=1&b=2"
url.parse_query(s: string) -> Vec<(string, string)> or UrlError
```

`encode` escapes everything outside the unreserved set — correct for a path
segment, a query value, or a form field, which is every case that comes up. A URL
you assembled yourself is already encoded; don't run it through again.

`encode_query`/`parse_query` work on a bare query string, which is also the
`application/x-www-form-urlencoded` body format. That's why they exist next to
`u.query()` rather than being folded into it — a form body has no URL around it.

Pairs, not a `Map`: query strings can repeat a key (`?tag=a&tag=b`) and order can
matter for signing.

## Errors

<!-- test: parse -->
```rask
enum UrlError {
    NoScheme                // "example.com/path" — relative URLs aren't parsed
    BadScheme(string)       // scheme has characters the grammar doesn't allow
    BadPort(string)         // ":99999" or ":http"
    BadEscape(string)       // "%zz", or "%" at the end of the input
    BadHost(string)
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
| Repeated key — `"?tag=a&tag=b"` | — | `params()` yields both; `query("tag")` gives the first |
| Valueless key — `"?debug"` | — | `query("debug")` is `""`, not `none`. It's present |
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
