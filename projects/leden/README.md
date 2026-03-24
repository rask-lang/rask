<!-- id: leden.overview -->
<!-- status: proposed -->
<!-- summary: Leden — capability-based networking and IPC protocol -->

# Leden

Capability-based networking and IPC protocol. Standalone crate. Handles structured communication between isolated endpoints — whether those are Allgard gards, OS processes, microservices, or machines on a network.

Leden has no knowledge of what's on either end. It's a protocol that anyone can use.

The name comes from the old Scandinavian shipping lanes — established routes between settlements.

## Why Leden

TCP moves bytes. HTTP adds request/response semantics. Neither gives you capabilities — fine-grained, delegatable, revocable authority over specific objects across trust boundaries.

ACLs don't scale across trust boundaries (governance nightmare). OAuth tokens are too coarse ("access everything on this server"). Blockchain is overkill for bilateral trust. Capabilities are the Goldilocks model: per-object authority, delegatable, revocable, unforgeable. Proven by 25 years of research (E language, KeyKOS, Spritely Goblins).

I'm not inventing — I'm packaging what works into something standalone and usable.

## What Leden Is

1. **A capability protocol** — four layers from transport to structured object references.
2. **Transport-agnostic** — pluggable: Unix sockets, TCP, QUIC, shared memory, in-process channels.
3. **A standalone crate** — no dependency on Rask's runtime. Embed it anywhere.

## What Leden Is Not

- Not an actor system. Leden is the wire between endpoints, not the endpoints themselves.
- Not tied to Allgard. Allgard uses Leden, but so can anything else.
- Not HTTP. No request/response semantics baked in.

## Protocol Layers

Four layers. Each solves one concern.

```
┌──────────────────────────────────┐
│  3. Object                       │  References, calls, pipelining
├──────────────────────────────────┤
│  2. Capability                   │  Tokens, delegation, revocation
├──────────────────────────────────┤
│  1. Session                      │  Multiplexing, reconnection, identity
├──────────────────────────────────┤
│  0. Transport                    │  TCP, QUIC, Unix socket, whatever
└──────────────────────────────────┘
```

See [protocol.md](protocol.md) for full specification of each layer.

**Layer 0: Transport** — Reliable ordered byte streams. Boring by design.

**Layer 1: Session** — Stateful relationship between two endpoints. Multiplexing, reconnection, backpressure. Sessions survive transport failures — a network blip doesn't destroy application state. Deliberate separation from CapTP, which couples sessions and capabilities.

**Layer 2: Capability** — Token creation, attenuation (narrowing scope), delegation (third-party handoff), revocation (membrane pattern). Capabilities are scoped to sessions but transferable across them.

**Layer 3: Object** — Gives capabilities structure. Object references, method dispatch, promise pipelining, argument serialization. Promise pipelining lives here because it's about call semantics, not access control.

## Key Operations

| Operation | What it does |
|-----------|-------------|
| **Bootstrap** | Cold-start: connect, get a single "greeter" capability, obtain scoped access from there. One public endpoint, everything else through capabilities. |
| **Introduce** | A introduces B to C without becoming a relay. B gets direct capability to C. Critical for scaling. First-class protocol operation. |
| **Revoke** | Switch off a capability via membrane pattern. Eventually consistent — protocol supports optimistic, pessimistic, and synchronous strategies per-capability. |
| **Pipeline** | Send a message to the result of a message that hasn't resolved yet. 3 round trips → 1. |

## Key Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Packaging | Separate crate (`leden`) | Standalone. No runtime dependency. |
| Trust model | Object capabilities | Proven. Fine-grained. Composes across trust boundaries. |
| Protocol | Binary, versioned, length-prefixed | Compact. No parsing ambiguity. Forward-compatible. |
| Transport | Pluggable (trait-based) | Same protocol over Unix sockets, TCP, shared memory, or in-process. |
| Session-capability coupling | **Decoupled** | Unlike CapTP. Network failure ≠ authority loss. Sturdy references for recovery. |
| Backpressure | Built-in | Senders block (or get errors) when receivers are slow. |
| Encryption | Optional TLS layer | Not forced. In-process and localhost don't need it. |

## Prior Art

| Source | What we took | What we skipped |
|--------|-------------|----------------|
| **E language / CapTP** | Promise pipelining, third-party handoff, sturdy refs, membrane revocation | Custom language, single-threaded vats, session-capability coupling |
| **Spritely Goblins** | Validation that OCap works for distributed systems | Guile Scheme, Syrup serialization, OCapN (too slow to standardize) |
| **Cap'n Proto RPC** | Proof that capability RPC is production-viable | Serialization lock-in |
| **Erlang/OTP** | Actor primitives (spawn, send, receive, monitor) | Trusted-network assumption, full mesh |

## Specs

| Spec | What it covers |
|------|----------------|
| [protocol.md](protocol.md) | Layers, operations, persistence, reconnection, capability lifecycle, version negotiation, error model |
| [content.md](content.md) | Content-addressed blob storage, lazy fetching, chunking |
| [observation.md](observation.md) | Push-based observation of object state changes |

## Open Questions

- **Discovery.** How do endpoints find each other? Static config? DNS? Multicast? Or leave that to Allgard?
- **Wire format.** MessagePack, Cap'n Proto, FlatBuffers, Protocol Buffers? Must have schema evolution, compact binary, cross-language support, existing tooling.
