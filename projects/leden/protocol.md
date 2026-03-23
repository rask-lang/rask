<!-- id: leden.protocol -->
<!-- status: proposed -->
<!-- summary: Leden protocol layers, operations, persistence, and serialization -->

# Protocol Specification

Four layers. Each solves one concern. The protocol is transport-agnostic and endpoint-agnostic — it doesn't care what's on either end.

## Layer 0: Transport

Reliable ordered byte streams. TCP, TLS, QUIC, Unix sockets, WebSocket. This layer is boring and should stay boring.

The only requirement: ordered, reliable delivery. Leden handles everything else.

## Layer 1: Session

A stateful relationship between two endpoints.

A session handles:
- **Multiplexing** — multiple logical streams over one transport connection
- **Reconnection** — session survives transport failures
- **Message ordering** — delivery guarantees within a stream
- **Backpressure** — receivers can slow down senders

Sessions have cryptographic identity that survives reconnection. When a transport drops and reconnects, the session resumes — capabilities remain valid, pending promises still pending.

### Session-Capability Decoupling

This is a deliberate departure from CapTP, which couples sessions and capabilities. That coupling means a network blip invalidates all your authority. Wrong tradeoff.

In Leden, sessions and capabilities are separate concerns. You receive capabilities through a session, and the session is how you exercise them. But capabilities can be transferred across sessions (third-party handoff), and a session failure doesn't revoke capabilities — you reconnect and re-attach using sturdy references.

## Layer 2: Capability

Where authority lives.

This layer handles:
- **Token creation and validation** — unguessable, unforgeable
- **Attenuation** — narrowing: a capability with reduced scope. Can only narrow, never widen. Authority flows downhill.
- **Delegation** — passing capabilities to third parties
- **Revocation** — membrane pattern: wrap a capability so it can be switched off
- **Authority verification** — check before applying an operation

Capabilities are scoped to sessions — you receive them through a session, exercise them through a session. But they can be transferred across sessions via introduction (third-party handoff). That's the critical distinction from CapTP's coupling.

## Layer 3: Object

Gives capabilities structure. Without this layer, capabilities are opaque tokens — useful but low-level.

This layer handles:
- **Object references** — capability + type/interface description
- **Method dispatch** — translating "call method X on object Y" into messages
- **Promise pipelining** — calling methods on not-yet-resolved results
- **Argument serialization** — encoding/decoding method arguments and return values

Promise pipelining lives here because it's about call semantics, not access control. "Send message to the result of a message I haven't gotten back yet" requires understanding method signatures and return types.

---

## Operations

### Bootstrap (Cold Start)

How two endpoints that have never met establish their first capability exchange.

Adapted from E's sturdyrefs:

1. Endpoint A publishes a **bootstrap address** — a well-known URL or socket path
2. Endpoint B connects and establishes a Session (Layer 1)
3. The Session provides a single bootstrap capability: a reference to A's "greeter" object
4. B invokes the greeter, providing credentials or proof of identity
5. The greeter returns capabilities scoped to what B is authorized for

The greeter is the only "public" capability. Everything else is obtained by exercising capabilities you already hold. The attack surface is exactly one object.

### Introduction (Third-Party Handoff)

The most important distributed operation. Without this, every cross-endpoint interaction requires a central broker.

Scenario: A wants to give B access to an object on C.

1. A holds a capability to the object (from C)
2. A creates an **introduction** — a new capability scoped for B, referencing the object
3. A sends the introduction to B over their existing session
4. B presents the introduction to C, establishing a direct session
5. C validates the introduction (checks it chains back to a valid delegation from A)
6. B now has a direct capability to the object on C

A is out of the loop. B and C communicate directly. Introductions fan out — the introducer doesn't become a bottleneck.

This is a **named protocol operation** (`Introduce`), not an implicit side effect of delegation. Important enough for first-class status.

### Revocation

When a capability is revoked:

1. Revoker marks the capability as revoked
2. Revocation notice propagated to all endpoints that have seen it
3. Endpoints receiving the notice stop honoring it

The hard part: **revocation is eventually consistent.** Unavoidable latency between "revoke" and "every endpoint knows." During this window, the revoked capability might still be used.

Strategies by risk level (per-capability policy, not global):

| Strategy | When | Behavior |
|----------|------|----------|
| Optimistic | Low-value operations | Allow during window, log, reconcile later |
| Pessimistic | High-value operations | Liveness check before honoring |
| Synchronous | Critical operations | Don't complete until revocation status confirmed |

### Promise Pipelining

Send a message to the result of a message that hasn't resolved yet.

Without pipelining:
```
1. A → B: "Give me the inventory"           (round trip 1)
2. B → A: [reference]
3. A → B: "Get item from inventory"          (round trip 2)
4. B → A: [reference]
5. A → B: "Get item's property"              (round trip 3)
6. B → A: 42
```

With pipelining:
```
1. A → B: "Give me the inventory,
           then get the item from it,
           then get the item's property"      (one round trip)
2. B → A: 42
```

Three round trips → one. Over 100ms latency, that's 300ms vs 100ms. For chains of 10 operations, the difference between usable and unusable.

The protocol represents promise references — placeholders for not-yet-resolved values — as first-class message targets.

---

## Persistence

Capabilities must survive endpoint restarts.

**Sturdy references** (adapted from E/Spritely): a serializable, cryptographic token that can be stored and later used to re-establish a capability. When an endpoint restarts, clients reconnect and present their sturdy references to recover their capabilities.

A sturdy reference is NOT a capability — it's a *claim* that you once held one. The endpoint validates the claim and either re-issues the capability or rejects it (if revoked while down).

| Thing | Persisted? | How |
|-------|-----------|-----|
| Capabilities | Yes | Sturdy references, stored by holder |
| Sessions | No | Rebuilt on reconnection |
| Promises | Depends | Resolved = just values. Pending may be lost on restart. |

---

## Serialization

The wire format is undecided. Requirements:

- Schema evolution (add fields without breaking existing code)
- Compact binary representation (real-time performance matters)
- Existing ecosystem tooling (not a custom format — learned from Spritely's Syrup mistake)
- Cross-language support (the protocol shouldn't require a specific implementation language)

Candidates: MessagePack, Cap'n Proto, FlatBuffers, Protocol Buffers. Decision deferred.

---

## Open Design Problems

1. **Session-capability decoupling mechanics.** Sessions and capabilities are separate (unlike CapTP). But the protocol flow for "re-attach capability to new session after reconnect" via sturdy references needs concrete message types.

2. **Distributed revocation latency.** The optimistic/pessimistic/synchronous strategy per capability is right in principle. Needs concrete message types and flows for each.

3. **Promise resolution on endpoint failure.** If an endpoint goes down while holding pending promises, what happens? Timeout and error, retry on reconnect, or propagate failure. Probably context-dependent.

4. **Capability GC.** When no one holds a reference, the capability should be cleaned up. Distributed reference counting with cycle detection (from E). Needs specification.
