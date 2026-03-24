# Midgard

Virtual world architecture. A concrete example of Raido, Allgard, and Leden working together.

Midgard is an application — it uses the infrastructure projects, it doesn't define them.

## How It Uses the Stack

| Project | Role in Midgard |
|---------|----------------|
| **Leden** | Wire protocol between gards — sessions, capabilities, object references. Gossip discovery lets new regions join and find each other. |
| **Allgard** | Federation model — the six primitives (Object, Owner, Domain, Transform, Proof, Grant) and Conservation Laws. Midgard adds game-specific rules on top. |
| **Raido** | User-generated content. Entity scripts, modding, NPC AI. Sandboxed, deterministic, serializable. |

## Architecture

**Identity and inventory** — federated, like Matrix. You own your data on your home domain (or self-host). Portable between domains.

**Real-time interaction** — deterministic lockstep between peers for small groups (2-16 participants). Raido's fixed-point arithmetic and seedable PRNG guarantee bitwise-identical results across machines.

**Persistent world state** — single-owner model (Allgard Conservation Law 2). Only the owning domain can mutate an object. Sidesteps concurrent mutation entirely.

**UGC sandboxing** — Raido. Scripts get only the references the host hands them. Fuel-limited. Full VM state is serializable — scripts can be checkpointed, migrated, replayed.

**Cross-domain communication** — Allgard's model over Leden's protocol. Holding a reference to an object IS your permission to interact with it. No ACLs, no identity checks, no blockchain.

## What Midgard Adds to Allgard

Allgard defines the federation model. Midgard adds game-specific concerns:

- **Game object types**: swords, characters, regions — concrete types with game semantics
- **Game-specific value sinks**: crafting loss, repair costs, item decay (designed entropy per Conservation Law 3)
- **Raido integration**: VM snapshots travel as opaque Object content. Determinism guarantees bitwise-identical replay on the receiving end.
- **Lockstep simulation**: real-time interaction model for small groups, built on Raido's deterministic execution
- **Cross-domain rate limiting policy**: Conservation Law 5 is per-domain. Coordinated abuse from multiple domains needs application-level policy.
- **Non-transitive delegation policy**: if Owner A grants Owner B authority, B can't re-delegate to C without explicit permission. Keeps the authority graph manageable for game economies.

## What This Doesn't Need

- **Blockchain.** Allgard's capability model is the trust model. No global consensus required.
- **CRDTs.** Single-owner-at-a-time (Conservation Law 2) eliminates concurrent mutation.
- **ACLs.** Capability possession is permission.

## Open Questions

- **Domain sovereignty over supply**: can one domain mint independently of another, or is there a global mint authority?
- **Cross-domain transfer routing**: bilateral or through a clearinghouse?
- **Designed entropy**: what are the value sinks? Without them, economies inflate.
- **Wire format**: shared concern with Leden — MessagePack, Cap'n Proto, FlatBuffers?
- **Bootstrapping**: ✅ Resolved — seed nodes, zero trust, bilateral reputation. See [Allgard bootstrapping](../allgard/README.md#bootstrapping).
