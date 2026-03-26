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
| `nonce` | 256-bit | Unique, unguessable — proves this was issued, not fabricated |
| `delegation_path` | endpoint identity[] | Plaintext delegation path — who delegated to whom |
| `delegation_chain` | hash chain | HMAC-SHA256 verification of the delegation path |
| `expiry` | optional timestamp | When this sturdy ref becomes invalid (if time-limited) |

The `nonce` is critical. Without it, anyone who knows the object_id could fabricate a sturdy reference. The issuer generates the nonce and stores it — when a client presents a sturdy ref, the issuer checks the nonce against its records.

The `delegation_path` and `delegation_chain` together identify and verify the delegation path. The path provides plaintext identities for tree lookup; the chain provides cryptographic tamper detection. The algorithm and verification procedure are specified below.

### Delegation Chain Cryptography

A sturdy ref carries a plaintext delegation path (for tree lookup) and an HMAC chain (to prevent path substitution). The issuer's delegation tree is the authoritative record; the chain cryptographically binds the sturdy ref to a specific path through that tree.

#### Design Rationale

The issuer maintains a delegation tree — the authoritative record of every delegation event. That tree is the real security boundary. The delegation chain in the sturdy ref serves two purposes:

1. **Path identification.** The chain tells the issuer which delegation path the presenter claims. Without it, the issuer would need to search for a path from root to the presenter's identity. With indexed storage (keyed by nonce + holder) this is fast, but the chain makes the claim explicit — no ambiguity when a holder appears at multiple points in the tree.

2. **Path binding (prevents path substitution).** The plaintext path alone is vulnerable to substitution: if the same capability has been delegated through multiple paths, a low-privilege holder could swap their path for a higher-privilege path that also exists in the tree. The HMAC chain binds each step's parameters (permissions, weight, identities) to the specific delegation event. The issuer recomputes the HMAC using values from the tree, so a substituted path produces a different hash — caught at step 3. This is a real attack, not defense-in-depth.

#### Algorithm: HMAC-SHA256 Chain

Each link in the chain is an HMAC-SHA256 computed over the delegation context, keyed with a secret derived from the capability's nonce. The chain is an ordered array of 32-byte link hashes, one per delegation step.

**Root link** (issuer creates the original capability):

```
link_key  = HMAC-SHA256(key: nonce, data: "leden-delegation-v1")
link_0    = HMAC-SHA256(key: link_key, data: issuer_id || object_id || permissions || weight || holder_id)
```

The root link binds the capability to a specific holder and weight. `holder_id` is the endpoint identity of the first recipient. `weight` is the initial weight assigned at creation.

**Delegation link** (holder delegates to a new recipient):

```
link_n    = HMAC-SHA256(key: link_{n-1}, data: delegator_id || recipient_id || permissions_n || weight_n)
```

Each subsequent link is keyed with the *previous link's hash*. This chains the links cryptographically — you can't produce link_n without knowing link_{n-1}.

The inputs to each delegation link:
- `delegator_id` — endpoint identity of the entity delegating
- `recipient_id` — endpoint identity of the entity receiving
- `permissions_n` — the permission bitfield *after* any attenuation at this step
- `weight_n` — the weight assigned to the recipient at this step

**Concatenation encoding.** The `||` operator means length-prefixed concatenation: each field is encoded as a 2-byte big-endian length followed by the field bytes. Fixed-size fields (permissions as u64, weight as u64) are encoded as 8 bytes big-endian with no length prefix. This prevents ambiguity attacks where field boundaries are shifted.

**The sturdy ref carries two parallel arrays:**
- `delegation_path: [holder_id_0, holder_id_1, ..., holder_id_n]` — plaintext endpoint identities, one per delegation step (including the original holder)
- `delegation_chain: [link_0, link_1, ..., link_n]` — HMAC hashes, one per step

The path tells the issuer which tree nodes to look up. The chain lets the issuer verify the path wasn't tampered with. Both are needed — the chain is opaque (32-byte hashes), so without the plaintext path the issuer can't locate the corresponding delegation records.

#### Nonce Visibility

Every holder has the nonce (it's in the sturdy ref). A holder can compute `link_key` and produce chain links with arbitrary inputs. **The HMAC chain alone does not prevent forgery** — a holder who fabricates links referencing nonexistent delegations is caught by the tree lookup (step 3.2), not by the HMAC comparison.

What the HMAC *does* prevent: path substitution. A holder who modifies `delegation_path` to point to a different (valid) delegation is caught because the HMAC was computed over the original path's parameters.

The nonce proves "the issuer created this capability" (step 1). The path identifies the delegation route (step 3.1). The chain binds those claims together cryptographically (step 3.5). The tree confirms everything actually happened (step 3.2). All four are needed.

#### Introduction and Delegation Recording

When A introduces B to C's object, the delegation chain gains a new link — but the issuer (C) hasn't recorded this delegation yet. This is resolved by a two-phase introduction:

1. A sends `Introduce(capability, recipient=B, attenuation)` to C (the issuer).
2. C validates A's capability, records the delegation (A → B) in its tree, computes the new chain link, and builds a sturdy ref for B.
3. C responds to A with an `IntroduceResult` containing the token (B's sturdy ref).
4. A forwards the token to B.
5. B presents the token to C to establish a direct session.

The issuer computes the chain link — not A. A doesn't need to know the chain construction algorithm. C is the authority on its own tree and the only entity that can produce valid chain links (because it controls which delegations are recorded).

The issuer always learns about the delegation *before* the new holder can use it.

#### Example

```
Issuer I creates capability for object O, nonce N, grants to A (permissions=0x07, weight=512):
  I computes: link_key = HMAC-SHA256(N, "leden-delegation-v1")
  I computes: link_0   = HMAC-SHA256(link_key, I || O || 0x07 || 512 || A)
  A's sturdy ref: delegation_path = [A], delegation_chain = [link_0]

A delegates to B (attenuates to permissions=0x03, weight=256):
  A sends Introduce to I. I computes:
  link_1   = HMAC-SHA256(link_0, A || B || 0x03 || 256)
  B's sturdy ref: delegation_path = [A, B], delegation_chain = [link_0, link_1]

B delegates to C (no attenuation, weight=128):
  B sends Introduce to I. I computes:
  link_2   = HMAC-SHA256(link_1, B || C || 0x03 || 128)
  C's sturdy ref: delegation_path = [A, B, C], delegation_chain = [link_0, link_1, link_2]
```

#### Verification Procedure

When an endpoint receives a sturdy reference (during re-attachment, introduction, or any capability claim), it performs these steps in order. Any failure rejects the reference.

**Step 1: Nonce lookup.** Look up the nonce in the issuer's persistent capability store. If not found → reject with `InvalidNonce`. This proves the capability was actually issued, not fabricated.

**Step 2: Reconstruct the root link.** Compute `link_key` and `link_0` from the stored nonce, the issuer's own identity, the object_id, the original permissions, and the original holder_id (all stored by the issuer when the capability was created). Compare against `delegation_chain[0]`. If mismatch → reject with `InvalidChain`.

**Step 3: Walk the delegation tree.** The issuer maintains a delegation tree recording every delegation event (who delegated to whom, with what permissions and weight). For each delegation step `i` from 1 to N:

1. Use `delegation_path[i-1]` (delegator) and `delegation_path[i]` (recipient) to look up the delegation record in the tree
2. If the record doesn't exist → reject with `InvalidChain` (claimed delegation never happened)
3. If the record is revoked → reject with `CapabilityRevoked`
4. Recompute `link_i = HMAC-SHA256(link_{i-1}, delegator_id || recipient_id || permissions_i || weight_i)` using values from the tree record
5. Compare against `delegation_chain[i]`. If mismatch → reject with `InvalidChain`

**Step 4: Check terminal holder.** `delegation_path[N]` (the last entry) must match the endpoint presenting the sturdy reference. If mismatch → reject with `HolderMismatch` (the ref belongs to someone else).

**Step 5: Permission and expiry checks.** Verify the permissions in the sturdy ref match the most-attenuated permissions in the chain (can only narrow). Check expiry. Check revocation status.

**Step 6: Accept.** Re-issue a live capability with the verified permissions and a fresh lease.

#### Security Properties

Security comes from *three mechanisms working together*: the nonce (proves issuance), the chain (commits to a delegation path), and the tree (authoritative record of what actually happened). No single mechanism is sufficient alone.

**Forgery resistance.** An external attacker (no sturdy ref) cannot guess the 256-bit nonce, so they can't pass step 1. A holder who has the nonce can compute arbitrary chain links, but cannot forge delegation events in the issuer's tree. Any fabricated chain references a path that doesn't exist in the tree — rejected at step 3.

**Link-dropping resistance.** Each link is keyed with the previous link's hash. Removing link_i from the chain means link_{i+1} was keyed with a value the attacker can't reproduce without link_i. Even if the attacker could somehow produce a valid-looking shortened chain, the issuer recomputes the full chain from its tree — the chain length must match the tree depth for this delegation path.

**Impersonation resistance.** Each link encodes the delegator_id and recipient_id. If an attacker substitutes their own identity for either, the HMAC changes. The issuer cross-references against its delegation tree — the identities must match recorded delegation events. Additionally, step 4 checks that the presenter matches the terminal recipient.

**Path substitution resistance.** When the same capability has multiple delegation paths (e.g., A→B with permissions=0x07, A→C with permissions=0x03), a holder could modify their `delegation_path` to point to a higher-privilege path that exists in the tree. The HMAC prevents this: the issuer recomputes each link using the tree's recorded values for the claimed path. A substituted path produces different HMAC inputs → mismatch at step 3.

**Permission escalation resistance.** Each link includes the permission bitfield. Widening permissions changes the HMAC input, producing a different hash that won't match the issuer's recomputation. The issuer also independently enforces that permissions can only narrow along the chain (step 5).

**Replay resistance.** A revoked delegation's entry in the issuer's tree is marked as revoked. Presenting a sturdy ref whose chain passes through a revoked delegation fails at step 3 — the issuer checks revocation status for each delegation record during the tree walk. The chain itself doesn't prevent replay; the tree does.

#### Why HMAC, Not Signatures

Digital signatures (Ed25519, etc.) would let anyone verify the chain without contacting the issuer. That's not what we want. The issuer is *always* the verifier — it's the endpoint hosting the object. Third parties don't need to verify chains independently; they present them to the issuer for verification.

HMAC is simpler, faster, and doesn't require a PKI. Each endpoint already has a persistent identity, but that identity doesn't need to be a signing key for delegation purposes. The nonce-derived key is sufficient.

If a future extension needs third-party-verifiable delegation (e.g., for offline verification in partitioned networks), that's a separate mechanism. Don't over-engineer the common path.

#### Wire Format

The sturdy ref carries two parallel arrays:

```
delegation_path:  [bytes, bytes, ..., bytes]     -- endpoint identities
delegation_chain: [bin(32), bin(32), ..., bin(32)] -- HMAC-SHA256 hashes
```

Both arrays must have the same length, at least 1. The original recipient's sturdy ref has one entry in each. A zero-length array is invalid.

Chain length is bounded by the delegation tree depth. Implementations should reject chains longer than a configurable maximum (default: 32 links). Deeper delegation trees indicate either a design problem or an attack.

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
| `InvalidNonce` | Nonce doesn't match records — forged or corrupted ref |
| `InvalidChain` | Delegation chain doesn't match issuer's delegation tree |
| `HolderMismatch` | Presenter doesn't match the terminal recipient in the chain |
| `IssuerMismatch` | This server didn't issue this ref |

### Batching

`Reattach` takes an array, not a single ref, deliberately. A client reconnecting to a game server might need to re-attach 50 capabilities (world objects, chat channels, inventory). One message, one response. No per-ref round trip.

### Edge Cases

**Revoked during disconnection.** The client gets `CapabilityRevoked` in the reattach result. Clean — you find out immediately on reconnect, not when you try to use it.

**Server restarted too.** The server reconstructs its capability table from its persistent store. Sturdy refs are designed for this — the nonce and delegation chain are verifiable without in-memory state.

**Partial re-attachment.** Some refs succeed, some fail. The client gets a complete picture in one response and decides how to handle failures (request new capabilities from another source, degrade gracefully, etc.).

**Observations.** Active observations are re-established automatically after successful re-attachment (see observation.md). The server sends a snapshot for each to resync state.

**Pending promises.** Handled by the error model's resolution policies (see Error Model). `fail` promises were already rejected. `retry` promises are re-sent. `expire` promises are checked against their deadlines.

---

## Capability Lifecycle

How capabilities are born, shared, and cleaned up. The full picture.

### Creation

Capabilities are created by the endpoint that hosts the object. When the greeter (bootstrap) grants a capability, or when an object method returns a reference to another object, a new capability is minted:

1. Issuer generates a unique `nonce` and stores it
2. Issuer assigns a `weight` (for reference counting — see GC below)
3. Issuer records the holder in the delegation tree
4. Holder receives a live capability (in-session) + a sturdy reference (for persistence)

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

When all weight has been returned, no one holds a reference. The issuer cleans up the capability — removes it from the delegation tree, frees the nonce, reclaims any associated resources.

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
