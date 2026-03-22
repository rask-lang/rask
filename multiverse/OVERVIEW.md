# Multiverse Architecture

## The Problem

Decentralized virtual worlds need three things: decentralized authority, real-time performance, and user-generated content. Conventional wisdom says pick two.

Blockchain tries to solve this with global consensus. It fails on performance and UX. The actual answer has existed for 25 years in capability-based security research — it just shipped in niche languages nobody used.

## The Insight

Object capabilities *are* the trust model. Holding a reference to an object is your permission to interact with it. No ACLs, no identity checks, no blockchain. Authority flows through object references, scoped by construction, revocable at any time.

This was proven by Electric Communities Habitat (1990s), the E language, and Spritely Goblins. I'm not inventing — I'm distilling what works and delivering it in something mainstream.

## Core Model

Six primitives. Everything else composes from these.

| Primitive | What it is |
|-----------|-----------|
| **Object** | Opaque blob with content-addressed ID, type tag, owner. Also an actor and a capability. |
| **Owner** | Identity that holds capabilities (references to objects). |
| **Domain** | Authority boundary that hosts objects and enforces local rules. Trust boundary. |
| **Transform** | Proposed operation on an object. A message send. Supports promise pipelining. |
| **Proof** | Evidence that a transform is valid. Needed for cross-domain trust bootstrapping. |
| **Grant** | Scoped, optionally time-limited authority delegation. An attenuated capability with built-in revocation. |

See [PRIMITIVES.md](PRIMITIVES.md) for full definitions.

## Architecture

A hybrid. Each component uses the tool that fits.

**Identity and inventory** — federated, like Matrix. You own your data on your home server (or self-host). Portable between domains. No single point of failure.

**Real-time interaction** — deterministic lockstep between peers for small groups (2-16 participants). The host domain acts as arbiter if needed, replaying disputed results headlessly.

**Persistent world state** — single-owner model. Only the owning domain can mutate an object. This sidesteps concurrent mutation entirely — no CRDTs needed for the base case.

**UGC sandboxing** — WASM, capability-scoped. Scripts get only the references they're handed. Gas-limited execution prevents abuse.

**Cross-domain communication** — object capability protocol. OCap principles govern all inter-domain interaction. See [OCAP.md](OCAP.md).

## Conservation Laws

The system enforces six invariants unconditionally. No script, no server, no admin tool can violate them. See [CONSERVATION.md](CONSERVATION.md).

1. **Conservation of Supply** — total minted minus total burned equals total existing
2. **Singular Ownership** — every object has exactly one owner at every point in time
3. **Conservation of Exchange** — value in equals value out (minus declared sinks)
4. **Causal Ordering** — every mutation references a prior valid state
5. **Bounded Rates** — every operation type has a maximum frequency per entity per time window
6. **Authority Scoping** — operations can only affect objects the initiator has authority over

## What This Doesn't Need

- **Blockchain.** Capabilities are the trust model. No global consensus required.
- **CRDTs.** Single-owner-at-a-time eliminates concurrent mutation. If collaborative editing is needed later, add CRDTs for that specific type.
- **ACLs.** Capability possession is permission. No permission lists to maintain or check.
- **Custom serialization.** Use something standard with ecosystem tooling.

## Why Not Blockchain

Blockchain solves global consensus among mutually suspicious parties. But domains don't need global consensus — they need bilateral trust and capability delegation. A domain is authoritative for its own objects. Cross-domain transfer is a bilateral protocol between two domains, not a global ledger update.

The conservation laws are enforced locally by each domain and verified bilaterally during cross-domain operations. This scales because there's no global state — domains are independent shards that interoperate.

## Why Not Just Spritely/OCapN

The model is right. The delivery is wrong.

- Niche language (Guile Scheme) killed adoption
- Custom serialization (Syrup) has no ecosystem tooling
- OCapN standardization is moving too slowly to wait for
- Single-threaded vats prevent parallelism
- Virtual world demos never shipped as products

Steal the model, deliver it in something mainstream.

## Open Questions

- **Domain sovereignty over supply**: can Domain A mint independently of Domain B, or is there a global mint authority?
- **Cross-domain transfer routing**: bilateral or through a clearinghouse?
- **Designed entropy**: what are the value sinks? Without them, economies inflate.
- **Wire format**: what serialization? MessagePack, Cap'n Proto, FlatBuffers, or something else?
- **Bootstrapping**: how does first capability exchange happen between unknown domains?
