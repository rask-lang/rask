# Object Capabilities for Cross-Domain Communication

How domains talk to each other. The protocol layer that makes Midgard work.

## Foundation

Object capability security is the trust model. The core rule: **holding a reference to an object is your permission to interact with it.** No access control lists, no identity checks, no central authority. If you have the reference, you can use it. If you don't, you can't.

This has been independently derived multiple times over 25 years (E language, KeyKOS, Spritely Goblins). It's not speculative — it's the proven answer to "how do mutually suspicious parties share objects safely."

## Why OCap and Not Something Else

**ACLs don't scale across trust boundaries.** An ACL says "these identities can do these things." Cross-domain, you'd need every domain to agree on identities and permissions. That's a governance nightmare.

**Blockchain is overkill.** Global consensus among all domains for every operation? The latency and cost kill real-time interaction. Domains don't need global consensus — they need bilateral trust.

**OAuth/API keys are too coarse.** A bearer token that grants "access to everything on this server" is the wrong granularity for "read this specific field of this specific object."

**Capabilities are the Goldilocks model.** Fine-grained (per-object), delegatable (pass them around), revocable (switch them off), unforgeable (cryptographic). They compose naturally across trust boundaries because they don't depend on shared identity or shared state.

## Protocol Layers

Four layers. Not six, not three. Each solves one concern.

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

### Layer 0: Transport

Reliable ordered byte streams. TCP, TLS, QUIC, Unix sockets, WebSocket. The protocol is transport-agnostic. This layer is boring and should stay boring.

### Layer 1: Session

A stateful relationship between two domains.

A session handles:
- Connection multiplexing (multiple logical streams over one transport)
- Reconnection (session survives transport failures)
- Message ordering and delivery guarantees
- Backpressure

Sessions have cryptographic identity that survives reconnection. Network failures shouldn't destroy application state. When a transport drops and reconnects, the session resumes — capabilities remain valid, pending promises are still pending.

This is a deliberate separation from CapTP, which couples sessions and capabilities. I think that coupling is wrong: a network blip shouldn't invalidate all your authority.

### Layer 2: Capability

Where the six primitives live.

This layer handles:
- Token creation and validation (unguessable, unforgeable)
- Attenuation (narrowing: Grant with reduced scope)
- Delegation (passing capabilities to third parties)
- Revocation (membrane pattern: switch off a Grant)
- Authority verification (check before applying a Transform)

Capabilities are scoped to sessions — you receive capabilities through a session, and the session is how you exercise them. But capabilities can be transferred across sessions (third-party handoff). That's the critical distinction from CapTP's coupling.

### Layer 3: Object

Gives capabilities structure. Without this layer, capabilities are opaque tokens — useful but low-level.

This layer handles:
- Object references (capability + type/interface description)
- Method dispatch (translating "call method X on object Y" into Transforms)
- Promise pipelining (calling methods on not-yet-resolved results)
- Serialization of arguments and return values

Promise pipelining lives here because it's about call semantics, not access control. "Send message to the result of a message I haven't gotten back yet" requires understanding method signatures and return types.

## Key Operations

### Capability Exchange (Bootstrapping)

The cold-start problem: how do two domains that have never met establish their first capability exchange?

Approach (adapted from E's sturdyrefs):

1. Domain A publishes a **bootstrap endpoint** — a well-known URL or address
2. Domain B connects and establishes a Session (Layer 1)
3. The Session provides a single bootstrap capability: a reference to Domain A's "greeter" object
4. Domain B invokes the greeter, providing credentials or proof of identity
5. The greeter returns capabilities scoped to what Domain B is authorized for

The greeter is the only "public" capability. Everything else is obtained by exercising capabilities you already hold. The attack surface is exactly one object.

### Third-Party Handoff (Introduction)

The most important distributed operation. Without this, every cross-domain interaction requires a central broker.

Scenario: Owner A (on Domain X) wants to give Owner B (on Domain Y) access to Object Z (on Domain C).

1. Owner A holds a capability to Object Z (a Grant from Domain C)
2. Owner A creates an **introduction** — a new Grant scoped for Owner B, referencing Object Z
3. Owner A sends the introduction to Owner B over their existing Session
4. Owner B presents the introduction to Domain C, establishing a direct Session
5. Domain C validates the introduction (checks it chains back to a valid Grant from Owner A)
6. Owner B now has a direct capability to Object Z on Domain C

Owner A is out of the loop. Owner B and Domain C communicate directly. This is how the system scales — introductions fan out, the introducer doesn't become a bottleneck.

This should be a **named protocol operation** (e.g., `Introduce`), not an implicit side effect of Grant semantics. It's important enough to deserve first-class status.

### Revocation

When a Grant is revoked:

1. Grantor marks the Grant as revoked in their domain
2. Revocation notice propagated to all domains that have seen the Grant
3. Domains receiving the notice stop honoring the Grant

The hard part: **revocation is eventually consistent.** There's unavoidable latency between "revoke" and "every domain knows." During this window, the revoked Grant might still be used.

Strategies by risk level:
- **Low-value operations**: optimistic. Allow operations during the window, log them, reconcile later.
- **High-value operations**: pessimistic. Require a liveness check ("is this Grant still valid?") before honoring it.
- **Critical operations**: synchronous. Don't complete until revocation status is confirmed with the grantor.

The protocol should support all three. The choice is per-Grant policy, not global.

### Promise Pipelining

Send a Transform to the result of a Transform that hasn't resolved yet.

Without pipelining:
```
1. A → B: "Give me the inventory"           (round trip 1)
2. B → A: [inventory reference]
3. A → B: "Get the sword from inventory"     (round trip 2)
4. B → A: [sword reference]
5. A → B: "Get the sword's damage stat"      (round trip 3)
6. B → A: 42
```

With pipelining:
```
1. A → B: "Give me the inventory,
           then get the sword from it,
           then get the sword's damage stat"  (one round trip)
2. B → A: 42
```

Three round trips become one. Over a 100ms network, that's 300ms vs 100ms. For chains of 10 operations, it's the difference between usable and unusable.

The protocol must represent promise references — placeholders for not-yet-resolved values — as first-class message targets.

## Persistence

Objects and capabilities must survive domain restarts.

**Sturdy references** (adapted from E/Spritely): a serializable, cryptographic token that can be stored and later used to re-establish a capability. When a domain restarts, clients reconnect and present their sturdy references to recover their capabilities.

A sturdy reference is NOT a capability — it's a *claim* that you once held a capability. The domain validates the claim and either re-issues the capability or rejects it (if it was revoked while the domain was down).

### What Gets Persisted

| Thing | Persisted? | How |
|-------|-----------|-----|
| Objects | Yes | Domain's storage (the domain decides how) |
| Ownership | Yes | Part of Object metadata |
| Grants | Yes | Grantor's domain stores active Grants |
| Sessions | No | Rebuilt on reconnection |
| Promises | Depends | Resolved promises are just values. Pending promises may be lost on restart — the protocol must handle this. |

## Serialization

The wire format is undecided. Requirements:

- Schema evolution (fields can be added without breaking existing code)
- Compact binary representation (real-time performance matters)
- Existing ecosystem tooling (not a custom format — learned from Spritely's Syrup mistake)
- Cross-language support (the protocol shouldn't require a specific implementation language)

Candidates: MessagePack, Cap'n Proto, FlatBuffers, Protocol Buffers. Decision deferred.

Note: Raido has its own serialization format for VM state snapshots (versioned, with content-addressed chunks). The protocol wire format is separate — it carries Transforms, Proofs, and Grants between domains. Raido snapshots travel *inside* the protocol as opaque Object content when migrating running scripts across domains.

## What We Stole from Where

| Source | What we took | What we skipped |
|--------|-------------|----------------|
| **E language / CapTP** | Promise pipelining, third-party handoff, sturdy refs, membrane revocation | Custom language, single-threaded vats |
| **Spritely Goblins** | Validation that OCap model works for virtual worlds | Guile Scheme, Syrup serialization, OCapN (too slow to standardize) |
| **Cap'n Proto RPC** | Proof that capability RPC is production-viable | Cap'n Proto serialization lock-in, non-tokio async |
| **Erlang/OTP** | Actor model primitives (spawn, send, receive, monitor) | Trusted-network distribution, full mesh assumption |
| **Matrix** | Federation model, identity portability | Specific sync protocol (different problem domain) |

## Open Design Problems

1. **Session-capability decoupling details.** I've argued sessions and capabilities should be separate (unlike CapTP). But the mechanics of "re-attach capability to new session after reconnect" need working out. Sturdy references are the starting point, but the protocol flow needs specification.

2. **Distributed revocation latency.** The optimistic/pessimistic/synchronous strategy per Grant is right in principle. The protocol needs concrete message types and flows for each.

3. **Promise resolution on domain failure.** If Domain B goes down while holding pending promises from Domain A, what happens? Options: timeout and error, retry on reconnect, or propagate failure. Probably all three depending on context.

4. **Capability GC.** When no one holds a reference to an Object, the capability should be cleaned up. Distributed garbage collection via reference counting with cycle detection (from E). Needs specification.

5. **Rate limiting across domains.** Conservation Law 5 is per-domain. But what about coordinated abuse from multiple domains? Cross-domain rate limiting is a harder problem.
