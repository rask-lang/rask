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

## Designed Entropy

Value sinks prevent inflation (Conservation Law 3). Without them, supply only grows. Midgard's sinks:

| Sink | Mechanism |
|------|-----------|
| **Crafting loss** | Combining items consumes some inputs. 3 iron bars → 1 sword, not 3 iron bars ↔ 1 sword. |
| **Repair costs** | Maintaining equipment consumes resources. Skip repairs, item degrades. |
| **Item decay** | Some asset types have a lifespan. Consumables, buffs, temporary enchantments. |
| **Transaction fees** | Cross-domain transfers cost something. Small, but bounds spam and drains supply. |
| **Training costs** | Learning abilities consumes resources. Permanent character progression as a value sink. |

The specific sinks are domain policy — each Midgard domain chooses its own rates. Conservation Law 3 just requires that sinks are declared in the transform type, not hidden. A domain that claims "free repairs" and quietly destroys inventory is violating the law.

Sinks should be tunable, not fixed. A domain running a casual server wants low decay. A hardcore server wants high decay. The protocol doesn't care — it just enforces that declared sinks match actual destruction.

## Deferred

- **Wire format**: shared concern with Leden. Implementation detail.

## Resolved

- **Domain sovereignty over supply**: per-domain minting, bilateral exchange, commodity money emerges. See [Allgard](../allgard/README.md#domain-sovereignty-over-supply).
- **Cross-domain transfer routing**: bilateral with introduction or intermediary chains. See [Allgard](../allgard/README.md#cross-domain-transfer-routing).
- **Bootstrapping**: seed nodes, zero trust, bilateral reputation. See [Allgard](../allgard/README.md#bootstrapping).
