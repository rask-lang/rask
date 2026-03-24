<!-- id: allgard.primitives -->
<!-- status: proposed -->
<!-- summary: The six federation primitives -->

# Primitives

Six primitives. Everything in the federation composes from these.

## Object

An opaque blob with a content-addressed ID, a type tag, and an owner.

An Object is simultaneously three things:
- **Data** — it has state (the blob)
- **Actor** — it receives messages (Transforms), has private state, no shared memory
- **Capability** — holding a reference to it IS your permission to interact with it

Objects are content-addressed: the ID is derived from the content. This means:
- Deduplication is free
- Integrity verification is free
- References are unforgeable (you can't guess a valid ID)

Every Object has exactly one Owner at any point in time (Conservation Law 2). Ownership transfer is atomic.

### Properties

| Property | Description |
|----------|------------|
| `id` | Content-addressed identifier (hash of content + type + metadata) |
| `type` | Tag describing the object's schema/interface |
| `owner` | The Owner identity that has authority over this object |
| `domain` | The Domain currently hosting this object |
| `content` | Opaque bytes. Interpretation determined by `type`. |

## Owner

An identity that holds capabilities — references to Objects.

An Owner can:
- Authorize Transforms on Objects it owns
- Receive Grants from other Owners
- Delegate authority via Grants to other Owners
- Revoke Grants it has issued

Owners are cryptographic identities. The specific scheme (ed25519 keys, DIDs, etc.) is a protocol decision, not a primitive concern.

An Owner is *not* a person. A person may control multiple Owners. An automated system can be an Owner. The federation doesn't care about the entity behind the key.

### Home Domain

Every Owner has a home domain — the domain that is authoritative for their identity and primary inventory. An Owner can operate in other domains, but their home domain is the root of trust for their identity.

## Domain

An authority boundary. A gard that hosts Objects and enforces local rules.

A Domain is:
- **Trust boundary** — code running inside a Domain trusts other code in that Domain. Code across Domains does not trust each other by default.
- **Authority root** — the Domain is the final arbiter for Objects it hosts
- **Rule enforcer** — the Domain enforces its own rules (rate limits, content policies, application logic) on top of the universal Conservation Laws

Domains map to E's concept of a "machine" (trust boundary), not a "vat" (execution unit). A Domain may contain many execution units internally.

### Sovereignty

Each Domain is sovereign over its hosted Objects. It can:
- Define custom object types and rules
- Set rate limits and policies
- Accept or reject incoming transfers
- Run its own application logic

What it cannot do:
- Violate Conservation Laws
- Modify Objects it doesn't host
- Forge capabilities it wasn't granted

### Federation

Domains federate. Any Domain can communicate with any other Domain via Leden's capability protocol. There's no central authority, no global state, no master server. Domains discover each other through gossip and establish bilateral capability relationships.

## Transform

A proposed operation on an Object. A message send.

A Transform hasn't happened yet. It's a request: "I want to do this to this object." The hosting Domain validates and applies it (or rejects it).

### Operations

| Operation | Description |
|-----------|------------|
| `create` | Bring a new Object into existence |
| `mutate` | Change an Object's content |
| `transfer` | Move an Object to a new Owner |
| `split` | Divide an Object into parts (fungible assets) |
| `merge` | Combine Objects into one (fungible assets) |
| `destroy` | Remove an Object from existence (burning) |

### Promise Pipelining

Transforms support promise pipelining: you can send a Transform to the result of a Transform that hasn't resolved yet. This eliminates round-trip latency for chains of operations.

Example: "Transfer asset to winner of auction" doesn't need to wait for auction resolution to queue the transfer. The transfer references the promise of the auction's result.

Stolen directly from E/CapTP. Essential for distributed performance.

### Causal Ordering

Every Transform references the state it's operating on (Conservation Law 4). This means:
- No time-travel exploits
- No replay attacks
- No fork-based duplication
- Every mutation forms a DAG, not just current state

## Proof

Evidence that a Transform is valid.

Within a Domain, Proofs are whatever the Domain's internal validation requires. The interesting case is cross-domain: when Domain A wants to convince Domain B that a Transform is legitimate.

A Proof must establish:
- The Transform was authorized by the Object's Owner (signature)
- The Transform references a valid prior state (causal link)
- Any Domain-specific rules were satisfied

Proofs are the trust-bootstrapping mechanism. When two Domains that have never interacted want to exchange Objects, Proofs are how they verify legitimacy without trusting each other.

### Verifiable Proofs

For Transforms backed by [Raido](../raido/) scripts, a Proof can include the script hash, inputs, and outputs. The receiving Domain fetches the script and re-executes — determinism guarantees identical results. This turns a trust-based Proof into a mechanically verifiable one.

Verifiable Proofs are an optional extension. Both Domains must negotiate "verifiable-transform" as a Leden capability. See [Verifiable Transforms](README.md#verifiable-transforms).

## Grant

Scoped, optionally time-limited authority delegation. An attenuated capability.

A Grant lets an Owner delegate specific authority over specific Objects to another Owner, without transferring ownership. The recipient can exercise the granted authority but cannot escalate it.

### Properties

| Property | Description |
|----------|------------|
| `grantor` | The Owner delegating authority |
| `grantee` | The Owner receiving authority |
| `scope` | What operations are permitted (e.g., read-only, mutate specific fields) |
| `target` | Which Objects the Grant applies to |
| `expiry` | Optional time limit. `None` means revoke-only. |
| `revocable` | Whether the grantor can revoke. Default: yes. |

### Attenuation

Grants can only narrow, never widen. If Owner A grants Owner B read+write on an Object, Owner B can grant Owner C read-only — but not read+write+transfer. Authority flows downhill.

### Revocation

Revocation is built in, not optional. The mechanism is the membrane pattern: the Grant is a wrapper that can be switched off. When revoked, all further Transforms through that Grant are rejected.

Revocation is **eventually consistent** in a distributed system. There's unavoidable latency between "revoke" and "all domains know it's revoked." The protocol must handle the window where a revoked Grant is still being exercised somewhere. Options:

1. **Optimistic**: allow operations during the window, reconcile after
2. **Pessimistic**: require liveness check before honoring a Grant
3. **Hybrid**: optimistic for low-value, pessimistic for high-value

### Third-Party Handoff

A Grant enables third-party introduction: Owner A sends Owner B a reference to an Object on Domain C. Owner B connects directly to Domain C using the Grant. Cross-domain object introduction without a central broker.

This should be a named operation in the protocol, not an implicit consequence of Grant semantics.

## How They Compose

A typical cross-domain interaction:

1. **Owner A** on **Domain X** holds an **Object**
2. Owner A creates a **Grant** giving **Owner B** transfer authority over the Object
3. Owner B submits a **Transform** (transfer Object to Owner B) to **Domain X**
4. Domain X validates the Transform against the Grant and the Conservation Laws
5. Domain X produces a **Proof** of the transfer
6. Owner B's home **Domain Y** receives the Proof and registers the Object in Owner B's inventory

Every step uses only the six primitives. No special cases.
