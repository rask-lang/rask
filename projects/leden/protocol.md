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
2. A sends `Introduce(capability, recipient=B, attenuation)` to C (the issuer)
3. C records the delegation (A → B) in its tree, computes the new chain link, and responds with an introduction token (a sturdy ref for B)
4. A forwards the token to B over their existing session
5. B presents the token to C, establishing a direct session
6. C validates the token (see Delegation Chain Cryptography, Verification Procedure)
7. B now has a direct capability to the object on C

A is out of the loop after step 4. B and C communicate directly. The issuer (C) always learns about the delegation before B can use it — no gap between delegation and tree recording.

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

#### Revocation Message Types

| Message | Direction | Fields | Purpose |
|---------|-----------|--------|---------|
| `Revoke` | Revoker → Issuer | `capability_id` | "I want this capability dead" |
| `RevocationNotice` | Issuer → All holders | `capability_id`, `reason` | "This capability is no longer valid" |
| `RevocationAck` | Holder → Issuer | `capability_id` | "Acknowledged, I've stopped using it" |
| `CheckRevocation` | Holder → Issuer | `capability_id` | "Is this still valid?" |
| `RevocationStatus` | Issuer → Holder | `capability_id`, `valid: bool` | "Yes/no" |

The issuer is the authoritative source for revocation status. It maintains a revocation log — an append-only record of which capabilities were revoked and when.

#### Optimistic Flow

```
A (revoker)              Issuer              B (holder)
    |                      |                     |
    |  Revoke(cap_id)      |                     |
    |─────────────────────>|                     |
    |                      | mark revoked        |
    |                      |                     |
    |                      | RevocationNotice    |
    |                      |────────────────────>|
    |                      |                     | stop using cap
    |                      |                     |
```

No synchronization. B might use the capability between "mark revoked" and receiving the notice. That's accepted — the application reconciles later (e.g., undo the operation, compensate).

Best for: read operations, low-value writes, anything where "oops, they saw one more update" is fine.

#### Pessimistic Flow

```
B (holder)              Issuer
    |                      |
    | (wants to use cap)   |
    | CheckRevocation(id)  |
    |─────────────────────>|
    |                      |
    | RevocationStatus     |
    |<─────────────────────|
    |                      |
    | (proceed or abort)   |
```

Extra round trip every time B wants to use the capability. Expensive but safe. No window of vulnerability.

The cost is real: one additional RTT per operation. For a 100ms link, that's 100ms added to every call. Use this only when the consequence of using a revoked capability is worse than the latency.

Best for: financial operations, permission changes, anything where "used a revoked capability" is a serious problem.

**Caching:** To reduce the cost, B can cache the status for a short TTL. The TTL trades freshness for latency. A 1-second cache on a 100ms link means at most 1 second of stale status instead of checking every time. The issuer can also push `RevocationNotice` to invalidate caches.

#### Synchronous Flow

```
A (revoker)              Issuer              B (holder)
    |                      |                     |
    |  Revoke(cap_id)      |                     |
    |─────────────────────>|                     |
    |                      | RevocationNotice    |
    |                      |────────────────────>|
    |                      |                     |
    |                      |    RevocationAck    |
    |                      |<────────────────────|
    |                      |                     |
    |  Revoked(confirmed)  | all acks received   |
    |<─────────────────────|                     |
```

The revoker blocks until all holders have acknowledged. No operations on the capability succeed during this window — the issuer queues them until revocation is confirmed.

This is the slowest strategy. If one holder is unreachable, the revoker blocks. Needs a timeout: if not all acks arrive within the deadline, the revocation is forced and any in-flight operations from unresponsive holders will get `CapabilityRevoked` when they eventually arrive.

Best for: security-critical revocation (revoking a compromised key), administrative operations.

#### Delegated Revocation

When A delegates a capability to B, and B delegates to C, revocation must cascade:

```
A revokes the original capability
→ B's derived capability is automatically revoked
  → C's derived capability is automatically revoked
```

The issuer tracks the delegation tree. `RevocationNotice` is sent to all nodes in the tree. A holder doesn't need to explicitly revoke derived capabilities — revoking the parent revokes all descendants.

This is why the delegation chain matters for introduction (Layer 2). The issuer must track "who delegated to whom" to propagate revocation correctly.

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

## Session Reconnection

The concrete flow for "transport dropped, reconnect and get my capabilities back."

### Sturdy Reference Structure

A sturdy reference is what you store to prove you held a capability. It contains:

| Field | Type | Purpose |
|-------|------|---------|
| `issuer` | endpoint identity | Who created the original capability |
| `object_id` | opaque | Which object this grants access to |
| `permissions` | bitfield | What operations are allowed (after attenuation) |
| `token` | 256-bit | Unique, unguessable — identifies this specific delegation event |
| `expiry` | optional timestamp | When this sturdy ref becomes invalid (if time-limited) |

The `token` is unique per delegation event — not per capability. When the issuer creates a capability, the first holder gets a token. When that holder delegates, the new holder gets a different token. Each token maps to exactly one node in the issuer's delegation tree.

### Delegation Verification

The delegation tree is the security boundary. Each node in the tree records: recipient identity, permissions, weight, revocation status, and parent node. The token is a lookup key into this tree.

#### Verification Procedure

When an endpoint receives a sturdy reference (during re-attachment, introduction, or any capability claim), it performs these steps in order. Any failure rejects the reference.

**Step 1: Token lookup.** Look up the token in the issuer's persistent store. If not found → reject with `InvalidToken`. This proves the delegation event actually happened.

**Step 2: Check presenter.** The tree node's `recipient` must match the endpoint presenting the sturdy reference. If mismatch → reject with `HolderMismatch`.

**Step 3: Check revocation.** Walk from the tree node up to the root, checking each ancestor's revocation status. If any ancestor is revoked → reject with `CapabilityRevoked`. (Revocation cascades — revoking a parent revokes all descendants.)

**Step 4: Permission and expiry checks.** Verify the permissions in the sturdy ref are no broader than the tree node's permissions (can only narrow). Check expiry.

**Step 5: Accept.** Re-issue a live capability with the verified permissions and a fresh lease.

#### Example

```
Issuer I creates capability for object O, grants to A (permissions=0x07, weight=512):
  I generates token_A, stores tree node: {recipient=A, permissions=0x07, weight=512, parent=root}
  A's sturdy ref: {issuer=I, object_id=O, token=token_A, expiry=none}

A delegates to B (attenuates to permissions=0x03, weight=256):
  A sends Introduce to I. I generates token_B, stores tree node:
    {recipient=B, permissions=0x03, weight=256, parent=node_A}
  B's sturdy ref: {issuer=I, object_id=O, token=token_B, expiry=none}

B delegates to C (no attenuation, weight=128):
  B sends Introduce to I. I generates token_C, stores tree node:
    {recipient=C, permissions=0x03, weight=128, parent=node_B}
  C's sturdy ref: {issuer=I, object_id=O, token=token_C, expiry=none}
```

When C presents their sturdy ref:
1. Look up token_C → find tree node (recipient=C, permissions=0x03, parent=node_B)
2. Presenter is C, recipient is C → match
3. Walk up: node_B not revoked, node_A not revoked, root not revoked → ok
4. Permissions 0x03 ≤ 0x03 → ok
5. Accept, issue live capability

#### Security Properties

**Forgery resistance.** Tokens are 256-bit, randomly generated by the issuer. An attacker can't guess a valid token. A holder's token maps to one specific tree node — knowing it reveals nothing about other tokens.

**Path substitution resistance.** Each delegation event has its own token. A holder with a low-privilege delegation can't claim a high-privilege path because they don't have the token for it. Unlike a shared nonce + chain design, there's no shared secret that could be used to forge claims about other paths.

**Impersonation resistance.** Step 2 checks that the presenter matches the tree node's recipient. Stealing someone's sturdy ref requires obtaining their token, which is only transmitted once (from issuer to delegator to recipient).

**Permission escalation resistance.** Step 4 checks the claimed permissions against the tree. Widening fails. The tree records the exact permissions for each delegation event.

**Replay after revocation.** Step 3 walks the tree upward, checking every ancestor. Revoking any node in the chain invalidates all descendants. No window — the tree is the authoritative state.

**Link-dropping resistance.** Not applicable. There's no chain to drop. The tree itself encodes the full delegation path. The holder doesn't carry path information — the issuer reconstructs it from the tree node's parent pointers.

#### Why Not HMAC Chains

An earlier design used a shared nonce per capability with HMAC-SHA256 chains linking delegation steps. That approach had compounding problems:

1. Every holder had the nonce, so any holder could compute valid-looking chain links. The chain alone didn't prevent forgery — the tree had to do that too.
2. The chain was opaque hashes, so verification needed a parallel plaintext path array for tree lookup — two fields doing one job.
3. Path substitution attacks required the HMAC to bind each link to its specific delegation parameters. The HMAC was compensating for the shared nonce's weakness.

Per-delegation tokens eliminate all three problems. The token is secret (only the issuer and the specific recipient know it), maps directly to a tree node (no search needed), and can't be used to claim a different delegation (no shared secrets between delegation events).

The cost: one 256-bit token stored per delegation event, instead of one per capability. Since the issuer already stores a tree node per delegation event, this adds 32 bytes per node — negligible.

### Re-attachment Flow

```
Client                                Server (Issuer)
   |                                     |
   |  (transport reconnects)             |
   |                                     |
   |  Hello(...)                         |
   |────────────────────────────────────>|
   |                                     |
   |  Welcome(...)                       |
   |<────────────────────────────────────|
   |                                     |
   |  Reattach(sturdy_refs=[...])        |
   |────────────────────────────────────>|
   |                                     |
   |  ReattachResult(results=[           |
   |    {ref: r1, cap: <live cap>},      |
   |    {ref: r2, cap: <live cap>},      |
   |    {ref: r3, err: Revoked},         |
   |  ])                                 |
   |<────────────────────────────────────|
   |                                     |
   |  (resume operations with live caps) |
```

### Re-attachment Message Types

| Message | Direction | Fields | Purpose |
|---------|-----------|--------|---------|
| `Reattach` | Client → Server | `sturdy_refs[]` | "Here are my claims, give me live capabilities" |
| `ReattachResult` | Server → Client | `results[]` | Per-ref: live capability or error |

Each sturdy ref is validated independently. Some may succeed while others fail. Possible errors per ref:

| Error | Meaning |
|-------|---------|
| `CapabilityRevoked` | Revoked while you were disconnected |
| `CapabilityExpired` | Time limit passed |
| `ObjectNotFound` | Object was destroyed |
| `InvalidToken` | Token doesn't match records — forged or corrupted ref |
| `HolderMismatch` | Presenter doesn't match the token's recorded recipient |
| `IssuerMismatch` | This server didn't issue this ref |

### Batching

`Reattach` takes an array, not a single ref, deliberately. A client reconnecting to a game server might need to re-attach 50 capabilities (world objects, chat channels, inventory). One message, one response. No per-ref round trip.

### Edge Cases

**Revoked during disconnection.** The client gets `CapabilityRevoked` in the reattach result. Clean — you find out immediately on reconnect, not when you try to use it.

**Server restarted too.** The server reconstructs its capability table from its persistent store. Sturdy refs are designed for this — the token maps to a persisted tree node, no in-memory state needed.

**Partial re-attachment.** Some refs succeed, some fail. The client gets a complete picture in one response and decides how to handle failures (request new capabilities from another source, degrade gracefully, etc.).

**Observations.** Active observations are re-established automatically after successful re-attachment (see observation.md). The server sends a snapshot for each to resync state.

**Pending promises.** Handled by the error model's resolution policies (see Error Model). `fail` promises were already rejected. `retry` promises are re-sent. `expire` promises are checked against their deadlines.

---

## Capability Lifecycle

How capabilities are born, shared, and cleaned up. The full picture.

### Creation

Capabilities are created by the endpoint that hosts the object. When the greeter (bootstrap) grants a capability, or when an object method returns a reference to another object, a new capability is minted:

1. Issuer generates a unique 256-bit `token` for this delegation event
2. Issuer assigns a `weight` (for reference counting — see GC below)
3. Issuer records the holder, permissions, weight, and token in the delegation tree
4. Holder receives a live capability (in-session) + a sturdy reference containing the token (for persistence)

### Delegation

Delegation uses the same `Introduce` / `IntroduceResult` message pair regardless of whether A and B share a session with the issuer or not:

1. A sends `Introduce(capability, recipient=B)` to the issuer
2. The issuer splits A's weight — A keeps half, B gets half
3. The issuer records the delegation (A → B) in its delegation tree
4. The issuer computes the new chain link and builds B's sturdy ref
5. The issuer responds with `IntroduceResult(token=B's sturdy ref)`
6. A forwards the token to B

Weight splitting is how the issuer tracks outstanding references without centralized reference counting. The original weight (e.g., 1024) is halved at each delegation. When delegations return their weight (via `Release`), the issuer can determine when all references are accounted for.

### Release and GC

When a holder no longer needs a capability:

| Message | Direction | Fields | Purpose |
|---------|-----------|--------|---------|
| `Release` | Holder → Issuer | `capability_id`, `weight` | "I'm done, here's my weight back" |

```
A creates cap with weight 1024, gives to B
B delegates to C: B keeps 512, C gets 512
C delegates to D: C keeps 256, D gets 256

D is done: Release(weight=256) → Issuer, total returned = 256
C is done: Release(weight=256) → Issuer, total returned = 512
B is done: Release(weight=512) → Issuer, total returned = 1024 = original

All weight returned → capability can be garbage collected.
```

When all weight has been returned, no one holds a reference. The issuer cleans up the capability — removes it from the delegation tree, frees the tokens, reclaims any associated resources.

### Lease-Based Expiry

Weight-based GC has a problem: what if a holder crashes and never sends `Release`? The weight is lost. The capability is pinned forever.

Solution: **leases**. Every capability has a lease duration. The holder must periodically renew the lease. If the lease expires, the issuer reclaims the weight and treats the capability as released.

| Message | Direction | Fields | Purpose |
|---------|-----------|--------|---------|
| `Renew` | Holder → Issuer | `capability_id` | "I'm still here, extend my lease" |
| `LeaseExpired` | Issuer → Holder | `capability_id` | "Your lease expired, capability reclaimed" (best-effort) |

Lease duration is set by the issuer when the capability is created. Short leases (seconds) for high-churn objects. Long leases (minutes) for stable references. The holder renews at half the lease interval to avoid races.

If a session reconnects after a lease expired, re-attachment with the sturdy ref will re-issue the capability with a fresh lease — no permanent loss.

### Cycle Detection

The hard problem. A holds a cap to B, B holds a cap to A. Neither releases. Weights never return. Without intervention, both are pinned forever.

This is rare in practice — most capability graphs are trees, not cyclic. But "rare" isn't "impossible," and memory leaks in long-running servers are serious.

**Strategy: trial deletion.**

Adapted from distributed garbage collection literature. Periodically, the issuer suspects a capability might be in a cycle (heuristic: lease renewed many times but never released, low-weight reference that hasn't been delegated further). The issuer initiates a probe:

1. Issuer sends `GCProbe(capability_id, probe_id)` to the holder
2. Holder checks if it holds any capabilities back to the issuer (or the issuer's objects)
3. If yes, it forwards the probe along those capabilities
4. If the probe returns to the issuer, a cycle is confirmed
5. The issuer can then break the cycle by forcibly reclaiming one of the capabilities

This is expensive and only triggered by heuristics, not on every GC pass. For most applications, leases alone handle the problem — a leaked cycle that renews leases is a memory leak, but a bounded one that the application can monitor.

**Pragmatic reality:** Most deployments won't hit cycles. Leases are the primary GC mechanism. Weight-based counting is the fast path. Cycle detection is a safety net for long-running servers with complex capability graphs.

---

## Version Negotiation

The first thing two endpoints do. Before capabilities, before bootstrap, before anything — agree on what protocol version to speak.

### Handshake

Happens at Layer 1 (Session establishment), immediately after transport connects.

```
Client                                Server
   |                                     |
   |  Hello(format=1, min=1, max=3,     |
   |        ext=[...])                   |
   |────────────────────────────────────>|
   |                                     |
   |  Welcome(format=1, version=3,      |
   |          ext=[...])                 |
   |<────────────────────────────────────|
   |                                     |
   |  (session established, proceed      |
   |   to bootstrap)                     |
```

Or if incompatible:

```
Client                                Server
   |                                     |
   |  Hello(format=2, min=4, max=5,     |
   |        ext=[...])                   |
   |────────────────────────────────────>|
   |                                     |
   |  Incompatible(server_min=1,         |
   |    server_max=3, formats=[1])       |
   |<────────────────────────────────────|
   |                                     |
   |  (connection closed)                |
```

The `format` field carries the wire format version, separate from the protocol version (see [wire-format.md](wire-format.md)). The Hello message is always encoded using format version 1 rules to bootstrap negotiation.

The server picks the highest protocol version both sides support. If there's no overlap, the connection fails with a clear error that tells the client what versions the server does support. No guessing.

### Version Semantics

**Major versions** — breaking changes. New major = new protocol. Old and new cannot interoperate without explicit support from both sides.

**Minor versions** — additive only. New message types, new optional fields, new extensions. Old endpoints ignore what they don't understand. A v1.3 endpoint can talk to a v1.1 endpoint — the v1.1 side just won't use the new features.

This means: deploy new servers first (they support old + new minor), clients upgrade at their pace.

### Extensions

Not everything belongs in the core protocol. Extensions are optional features negotiated during the handshake.

```
Hello(min=1, max=1, ext=[content_store, observation, compression_lz4])
Welcome(version=1, ext=[content_store, observation])
```

Both sides advertise what they support. The session uses the intersection. The content store (content.md) and observation (observation.md) are extensions — not every endpoint needs them.

Extensions have their own versioning. `content_store_v2` is a different extension from `content_store_v1`. Keeps the negotiation flat and simple.

### What This Prevents

- **Silent incompatibility.** You find out immediately, not three messages in when something doesn't parse.
- **Forced lockstep upgrades.** Minor versions are backward-compatible. You don't need to upgrade the entire fleet at once.
- **Feature creep in core.** Extensions keep optional features out of the base protocol. An embedded device running bare Leden doesn't pay for observation support it doesn't need.

---

## Error Model

Every protocol needs to answer: what happens when things go wrong?

### Error Levels

| Level | Where | Example | Handled by |
|-------|-------|---------|------------|
| Transport | Layer 0 | Connection dropped | Session reconnection (Layer 1) |
| Protocol | Layer 1-2 | Malformed message, invalid session token | Session termination or reset |
| Capability | Layer 2 | Revoked capability, permission denied | Error response to caller |
| Application | Layer 3 | Method failed, object not found | Error response with details |

Transport and protocol errors are infrastructure — the endpoint handles them or dies. Capability and application errors are the interesting ones because they propagate to the caller and interact with promises.

### Error Structure

Every error has three parts:

| Field | Type | Purpose |
|-------|------|---------|
| `code` | enum | Machine-readable category. Protocol-defined set + application extension. |
| `message` | string | Human-readable explanation. For logs and debugging, not for branching on. |
| `data` | optional bytes | Structured detail. Application-specific. Opaque to the protocol. |

### Protocol Error Codes

These are the errors the protocol itself defines. Applications can extend with their own codes in the `Application` range.

| Code | Meaning |
|------|---------|
| `CapabilityRevoked` | The capability was revoked between issuance and use |
| `CapabilityExpired` | Time-limited capability past its expiry |
| `PermissionDenied` | Capability doesn't grant this operation |
| `ObjectNotFound` | The referenced object doesn't exist (or was destroyed) |
| `MethodNotFound` | The object doesn't support this method |
| `RateLimited` | Too many requests. Backpressure at the application level. |
| `EndpointUnavailable` | The hosting endpoint is unreachable |
| `Timeout` | No response within the deadline |
| `VersionMismatch` | Incompatible protocol version (from handshake) |
| `MalformedMessage` | Message doesn't parse |
| `Application(u32)` | Application-defined. The protocol routes it but doesn't interpret it. |

### Promise Rejection

This is the critical part. Promises are first-class in the protocol (promise pipelining). When a promise can't be fulfilled, it's **rejected** — and rejection propagates.

```
A calls B.method1() → promise P1
A calls P1.method2() → promise P2  (pipelined)
A calls P2.method3() → promise P3  (pipelined)

B.method1() fails with PermissionDenied:
  P1 = Rejected(PermissionDenied)
  P2 = Rejected(PermissionDenied)   ← automatic
  P3 = Rejected(PermissionDenied)   ← automatic
```

The error from the root cause propagates through the entire pipeline. The caller gets the original error, not "P2 failed because P1 failed." No wrapping, no nesting — the root cause flows through.

This is E's "broken promise" semantics. A broken promise infects everything that depends on it. Simple and correct.

### Promise Resolution on Endpoint Failure

The open question from before, now answered. When an endpoint goes down while holding pending promises, the behavior depends on the promise's **resolution policy**, set at creation time:

| Policy | Behavior | Use when |
|--------|----------|----------|
| `fail` | Reject with `EndpointUnavailable` after timeout | Default. Fast feedback. Most RPCs. |
| `retry` | Hold pending, retry when session reconnects | Idempotent operations where you'd rather wait than fail. |
| `expire` | Reject with `Timeout` after a deadline | Time-sensitive operations. "If this doesn't complete in 5s, I don't want it." |

The default is `fail`. You opt into `retry` or `expire` when you know the semantics justify it. No silent hangs.

### Error Handling at Session Boundaries

When a session drops and reconnects:

1. Capabilities survive (sturdy references, already specified).
2. Pending promises with `fail` policy are rejected immediately.
3. Pending promises with `retry` policy are re-sent automatically on reconnect.
4. Pending promises with `expire` policy are rejected if past their deadline.
5. Active observations (see observation.md) are re-established from the last known state.

The caller doesn't need to track which promises were in-flight. The session handles it.

---

## Serialization

MessagePack with integer-keyed maps. See [wire-format.md](wire-format.md) for framing, message schemas, schema evolution rules, and format version negotiation.

---

## Open Design Problems

None currently. All previously open problems (session-capability decoupling, distributed revocation flows, capability GC) have been specified above.
