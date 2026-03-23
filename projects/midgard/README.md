# Midgard

Virtual world architecture. A concrete example of Raido, Leden, and Allgard working together.

Midgard is an application — it uses the infrastructure projects, it doesn't define them.

## How It Uses the Stack

| Project | Role in Midgard |
|---------|----------------|
| **Allgard** | Each world region is a gard. Isolation, supervision, location transparency. |
| **Leden** | Transport between gards — whether same machine or across a network. |
| **Raido** | User-generated content. Entity scripts, modding, NPC AI. Sandboxed, deterministic, serializable. |

## Core Model

Six primitives. Everything composes from these.

| Primitive | What it is |
|-----------|-----------|
| **Object** | Opaque blob with content-addressed ID, type tag, owner. Also an actor and a capability. |
| **Owner** | Identity that holds capabilities (references to objects). Cryptographic — not necessarily a person. |
| **Domain** | A gard that hosts objects and enforces local rules. Authority boundary. |
| **Transform** | Proposed operation on an object. A message. Hasn't happened yet — the hosting domain validates and applies it. |
| **Proof** | Evidence that a transform is valid. For cross-domain trust bootstrapping. |
| **Grant** | Scoped, optionally time-limited authority delegation. An attenuated capability with built-in revocation. |

## Architecture

**Identity and inventory** — federated, like Matrix. You own your data on your home server (or self-host). Portable between domains.

**Real-time interaction** — deterministic lockstep between peers for small groups (2-16 participants). Raido's fixed-point arithmetic and seedable PRNG guarantee bitwise-identical results across machines.

**Persistent world state** — single-owner model. Only the owning domain can mutate an object. Sidesteps concurrent mutation entirely.

**UGC sandboxing** — Raido. Scripts get only the references the host hands them. Fuel-limited. Full VM state is serializable — scripts can be checkpointed, migrated, replayed.

**Cross-domain communication** — object capability protocol. Holding a reference to an object IS your permission to interact with it. No ACLs, no identity checks, no blockchain.

## Conservation Laws

Six invariants enforced unconditionally. No script, no server, no admin tool can violate them.

1. **Conservation of Supply** — `total_minted - total_burned = total_existing`
2. **Singular Ownership** — every object has exactly one owner at every point in time
3. **Conservation of Exchange** — value in equals value out (minus declared sinks)
4. **Causal Ordering** — every mutation references a prior valid state
5. **Bounded Rates** — every operation type has a max frequency per entity per time window
6. **Authority Scoping** — operations can only affect objects the initiator has authority over

See [CONSERVATION.md](CONSERVATION.md) for details.

## Object Capabilities

Cross-domain communication uses [Leden's](../leden/) capability protocol. See [OCAP.md](OCAP.md) for how Midgard applies it.

## What This Doesn't Need

- **Blockchain.** Capabilities are the trust model. No global consensus required.
- **CRDTs.** Single-owner-at-a-time eliminates concurrent mutation.
- **ACLs.** Capability possession is permission.

## Open Questions

- **Domain sovereignty over supply**: can one domain mint independently of another, or is there a global mint authority?
- **Cross-domain transfer routing**: bilateral or through a clearinghouse?
- **Designed entropy**: what are the value sinks? Without them, economies inflate.
- **Wire format**: MessagePack, Cap'n Proto, FlatBuffers, or something else?
- **Bootstrapping**: how does first capability exchange happen between unknown domains?
