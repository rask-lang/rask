<!-- id: std.tls -->
<!-- status: decided -->
<!-- summary: TLS client and server over TCP — dial or upgrade, verification on by default, provider behind a C shim -->
<!-- depends: stdlib/net.md, stdlib/io.md, memory/resource-types.md -->

# TLS

Encrypted connections. Two ways in: dial one, or upgrade a TCP connection you
already have.

`TlsConnection` and `TlsListener` are linear resources like their `net`
counterparts (`mem.resource-types`) — `ensure` closes them, and the compiler
catches the leak.

## Client

<!-- test: skip -->
```rask
import tls

let conn = try tls.connect("example.com:443")
ensure conn.close()

try conn.write_text(request)
let response = try conn.read_text()
```

## Upgrading an existing connection

<!-- test: skip -->
```rask
let raw = try net.tcp_connect("proxy.internal:8080")
try negotiate_tunnel(raw)                       // CONNECT, STARTTLS, whatever

let conn = try tls.wrap(own raw, host: "example.com")
ensure conn.close()
```

`wrap` takes the TCP connection (`own`, so the raw one is gone) and the hostname to
verify against — which is the origin server's name, not the proxy's. Every protocol
that upgrades mid-stream needs this, and so does every test that wants a socket
pair without a listener.

## Server

<!-- test: skip -->
```rask
let config = tls.Config.new()
    .with_cert("server.crt", key: "server.key")

let listener = try tls.listen(":443", config)
ensure listener.close()

loop {
    let conn = try listener.accept()
    spawn(|| {
        ensure conn.close()
        try handle(conn)
    }).detach()
}
```

## Config

<!-- test: skip -->
```rask
tls.Config.new() -> Config
config.with_cert(cert: string, key: string) -> Config     // PEM paths, server side
config.with_ca(path: string) -> Config                    // pin a CA instead of the system store
config.with_alpn(protocols: Vec<string>) -> Config        // ["h2", "http/1.1"]
config.with_verification(v: Verification) -> Config
```

<!-- test: parse -->
```rask
enum Verification {
    Full            // default: chain and hostname
    ChainOnly       // hostname mismatch allowed — a pinned CA with a bare IP
    None            // no checking at all
}
```

## Connection

`TlsConnection` has the same surface as `net.TcpConnection` — `read`, `write`,
`read_text`, `write_text`, `close` — so code written against one works against the
other. Plus:

<!-- test: skip -->
```rask
conn.peer_certificate() -> Certificate?
conn.protocol() -> string?       // the ALPN protocol agreed, if any
conn.tls_version() -> string     // "TLSv1.3"
```

## Core Rules

| Rule | Description |
|------|-------------|
| **L1: Dial or upgrade** | `tls.connect(addr)` for the common case; `tls.wrap(own conn, host:)` when the TCP connection already exists. Not two spellings of one operation — one opens a socket, the other takes one over — and without `wrap` proxies, STARTTLS and testing are impossible |
| **L2: Verified by default** | Certificate chain and hostname are checked unless the config says otherwise. There is no "convenience" constructor that skips it |
| **L3: Weakening is named and visible** | Turning verification off is `with_verification(.None)` at the call site. It is not a boolean, not an environment variable, and it produces a compile-time warning naming the line |
| **L4: Same shape as `net`** | `TlsConnection` mirrors `net.TcpConnection`, `tls.listen` mirrors `net.tcp_listen`. TLS is a layer, not a parallel networking API to learn |
| **L5: Modern protocol versions only** | TLS 1.2 and 1.3. SSLv3, TLS 1.0 and 1.1 are not configurable back on — a knob for them is a knob for downgrade attacks |
| **L6: Provider is an implementation detail** | The library underneath is not in the API. No provider handles, no library-specific config, nothing that would change if it were swapped |

## Errors

<!-- test: parse -->
```rask
enum TlsError {
    Handshake(string)
    CertificateExpired(subject: string)
    HostnameMismatch(expected: string, found: string)
    UntrustedIssuer(subject: string)
    NoCertificate
    BadKeyPair(string)
    Protocol(string)
    Io(net.NetError)
}
```

Verification failures are separate variants because they're the ones people
actually debug, and "handshake failed" for all of them is the error message that
sends developers to the internet to find `--insecure`.

## Error Messages

```
ERROR [std.tls/L2]: certificate hostname does not match
   |
8  |  let conn = try tls.connect("192.168.1.10:443")
   |                              ^^^^^^^^^^^^^^^^^ certificate is for "internal.example.com"

WHY: The server's certificate names internal.example.com. You connected
     to an IP, so there's nothing for the hostname check to match.

FIX 1: Connect by name so the certificate can be verified:

  let conn = try tls.connect("internal.example.com:443")

FIX 2: If this is a pinned internal CA reached by IP, say so explicitly:

  let config = tls.Config.new()
      .with_ca("internal-ca.pem")
      .with_verification(.ChainOnly)
```

## Not decided: the provider

The API above is settled. What implements it is not.

The candidates trade the same way they do everywhere: a bundled library
(BearSSL, mbedTLS) gives one code path on every platform at the cost of owning
root-store discovery and tracking CVEs in vendored crypto; the system provider
(SChannel, SecureTransport, OpenSSL) gets cert stores and OS policy for free at the
cost of three shims and OpenSSL ABI drift on Linux; rustls via the C ABI is the
best-maintained stack but puts a Rust toolchain in the path of every program that
links it.

L6 exists so this stays reversible. What the provider must supply:

| Capability | Why |
|------------|-----|
| Handshake over a caller-supplied socket | `wrap` (L1) — the provider must not own the socket or the DNS lookup |
| Chain verification against a supplied or system trust store | L2 |
| Hostname verification, separately controllable | L3, `Verification.ChainOnly` |
| ALPN | HTTP/2 |
| TLS 1.2 and 1.3, nothing older | L5 |
| Server-side cert and key from PEM | `with_cert` |

Anything a provider offers beyond that list stays behind the shim.

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Self-signed certificate | L2 | `UntrustedIssuer`. Pin it with `with_ca`, don't disable verification |
| Expired certificate | L2 | `CertificateExpired` — a distinct variant, because the fix is renewing it |
| Server sends no certificate | L2 | `NoCertificate` |
| `wrap` on a connection with buffered unread data | L1 | Error — the handshake must start at a record boundary |
| Peer closes without `close_notify` | — | Reads return end-of-stream, not an error. Truncation attacks matter to protocols that don't frame their own length; HTTP does |
| Client certificate requested | — | Not supported yet. `with_client_cert` is the obvious extension |
| `listen` with a cert and key that don't pair | — | `BadKeyPair` at `listen`, not at the first connection |
| SNI | L1 | Sent automatically, from the address for `connect` and the `host:` for `wrap` |
| Renegotiation requested by the peer | L5 | Refused. TLS 1.3 removed it and it was a vulnerability in 1.2 |

---

## Appendix (non-normative)

### Rationale

**L1 (both entry points):** the earlier sketch had only `tls.connect`, which makes
TLS a parallel universe to `net` — fine until you need it over something that isn't
a fresh TCP dial. Proxy tunnels, STARTTLS in SMTP and IMAP, and testing against an
in-memory socket pair all need to hand an existing connection over. Two functions
because dialing and upgrading are different operations, not one operation spelled
twice: `connect` opens a socket, `wrap` consumes one.

**L2/L3 (verification on, weakening loud):** the failure mode is well documented in
every other ecosystem — a developer hits a certificate error, finds a flag that
makes it stop, and ships it. Making the weakening explicit at the call site doesn't
prevent that, but it puts the decision in the diff where a reviewer sees it, which
is the same reasoning as `unsafe` (`mem.unsafe`). The separate error variants exist
so that the first thing a developer sees is the actual problem — an expired cert, a
name mismatch — rather than a generic handshake failure that makes disabling
verification look like the only move.

**L6 (provider hidden):** every language that exposed its TLS library in the API
ended up unable to change it. Config objects grew library-specific options, error
types leaked library error codes, and the choice became permanent. Keeping the
provider behind the shim means the decision above can be made — and later remade —
without touching a line of user code.

### See Also

- `std.net` — the TCP layer this wraps; `TlsConnection` mirrors `TcpConnection`
- `std.http` — HTTPS is this plus HTTP; `http.get("https://...")` uses it
- `mem.resource-types` — why connections are linear
- `std.digest` — content hashing, unrelated to the crypto here
