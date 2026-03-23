<!-- id: leden.overview -->
<!-- status: proposed -->
<!-- summary: Leden — networking and IPC protocol for inter-domain communication -->

# Leden

Networking and IPC protocol. Standalone crate. Handles transport between isolated domains — whether those domains are Allgard gards, OS processes, or machines on a network.

Leden has no knowledge of Allgard or gards. It's a protocol and transport layer that anything can use.

## Why Leden

Rask has `std.net` (TCP/UDP) and `std.http` (HTTP client/server). Those are fine for talking to the outside world. Leden is for structured inter-domain communication — the plumbing between components that need to exchange typed messages reliably.

The name comes from the old Scandinavian shipping lanes — established routes between settlements. That's what this is: the route between domains.

## What Leden Is

1. **A wire protocol** — binary message format, versioned, compact.
2. **Transport layer** — pluggable: Unix sockets, TCP, shared memory, in-process channels.
3. **A standalone crate** — usable without Rask's runtime. Embed it in game engines, microservices, whatever.

## What Leden Is Not

- Not an RPC framework. No code generation, no IDL.
- Not an actor system. Leden moves bytes between endpoints. What those endpoints are (gards, processes, threads) is not Leden's concern.
- Not HTTP. No request/response semantics baked in.

## Key Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Packaging | Separate crate (`leden`) | Most programs won't need it. No stdlib bloat. |
| Protocol | Binary, versioned, length-prefixed | Compact. No parsing ambiguity. Forward-compatible. |
| Transport | Pluggable (trait-based) | Same protocol over Unix sockets, TCP, shared memory, or in-process. |
| Framing | Length-prefixed messages | Simple. No delimiter scanning. |
| Backpressure | Built-in | Senders block (or get errors) when receivers are slow. Prevents unbounded buffering. |
| Serialization | Rask's `Encode`/`Decode` | Zero-copy where possible. Same format as `std.encoding`. |
| Encryption | Optional TLS layer | Not forced. In-process and localhost don't need it. |

## Open Questions

- **Discovery.** How do endpoints find each other? Static config? DNS? Multicast? Or leave that to the layer above (Allgard)?
- **Multiplexing.** Multiple logical streams over one connection? Or one connection per stream?
- **Flow control.** Credit-based or window-based?
- **Reconnection.** Automatic reconnect with message replay? Or surface the failure?
- **Session persistence.** Should sessions survive transport failures? A network blip shouldn't destroy application state — but the mechanics of re-attaching to a session after reconnect need working out.
- **Promise pipelining.** Send a message to the result of a message that hasn't resolved yet. Eliminates round-trip latency for operation chains (3 round trips → 1). Requires first-class promise references as message targets. Proven by E/CapTP — worth considering at the transport level.
