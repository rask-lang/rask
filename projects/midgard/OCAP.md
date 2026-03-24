# Midgard's Use of Capabilities

How Midgard applies Allgard's federation model and Leden's capability protocol to virtual world communication.

The federation primitives (Object, Owner, Domain, Transform, Proof, Grant) live in [Allgard](../allgard/PRIMITIVES.md). The protocol mechanics (layers, operations, persistence) live in [Leden](../leden/). This document covers Midgard-specific decisions on top.

## Trust Model

Object capability security is the trust model. Holding a reference to an object IS your permission to interact with it. No ACLs, no identity checks, no blockchain.

This fits virtual worlds naturally: a player holding a reference to a sword can use it. A domain hosting objects is authoritative for them. Cross-domain trade is bilateral, not a global ledger update.

## How Midgard Maps to the Stack

| Allgard primitive | Midgard meaning |
|-------------------|----------------|
| Object | A game entity (sword, character, region, currency) |
| Owner | A player, NPC, or automated system |
| Domain | A world region or server shard |
| Transform | A game action (attack, trade, craft, move) |
| Proof | Evidence of a valid game action for cross-domain trust |
| Grant | Permission to interact with another player's items or enter a region |

| Leden concept | Midgard meaning |
|---------------|----------------|
| Endpoint | A domain (world region) |
| Capability | Authority over a game object |
| Introduction | Player A giving Player B access to an item on Domain C |
| Revocation | Revoking a trade permission, banning a player from a region |
| Bootstrap/greeter | Domain's public entry point — authenticate, get scoped capabilities |

## Cross-Domain Interaction Example

1. **Owner A** on **Domain X** holds an **Object** (a sword)
2. Owner A creates a **Grant** giving **Owner B** transfer authority
3. Owner B submits a **Transform** (transfer sword to me) to **Domain X**
4. Domain X validates against the Grant and Conservation Laws
5. Domain X produces a **Proof** of the transfer
6. Owner B's home **Domain Y** receives the Proof and registers the sword

Every step uses Allgard primitives over Leden protocol operations.

## Midgard-Specific Concerns

These are application policy, not federation model:

- **Rate limiting across domains.** Conservation Law 5 is per-domain. Coordinated abuse from multiple domains is a harder problem — cross-domain rate limiting needs application-level policy.
- **Raido snapshot migration.** Raido VM state travels inside the protocol as opaque object content. The protocol doesn't know it's a VM snapshot — it's just bytes. Determinism guarantees bitwise-identical replay on the other end.
- **Non-transitive delegation by default.** If Owner A grants Owner B authority, B can't re-delegate to C without explicit permission. Departure from some capability systems — keeps the authority graph manageable for game economies.
