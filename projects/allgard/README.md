<!-- id: allgard.overview -->
<!-- status: proposed -->
<!-- summary: Allgard — federation model for networks of gards -->

# Allgard

Federation model for networks of gards. Defines the rules that make cross-domain cooperation trustworthy without central authority.

A gard joins Allgard by speaking [Leden](../leden/) and respecting the [Conservation Laws](CONSERVATION.md). That's it. No registration, no approval, no central server. You're in if you play by the rules; you're out if you don't.

The name is Old Norse: *all* + *garðr*. All the gards, together.

## What Allgard Is

1. **A federation model** — six [primitives](PRIMITIVES.md) and six [conservation laws](CONSERVATION.md) that make cross-domain interaction trustworthy.
2. **Domain-sovereign** — every gard is authoritative for its own state. No global consensus, no master server.
3. **Bilateral trust** — domains verify each other's Proofs directly. Reputation is emergent, not administered.
4. **Protocol-agnostic** — Allgard defines the *model*. [Leden](../leden/) is the wire protocol that carries it.

## What Allgard Is Not

- Not a protocol. Leden is the protocol. Allgard is the model that gives protocol messages meaning.
- Not a registry. Discovery is Leden's gossip layer. Allgard doesn't know who's online.
- Not a blockchain. No global ledger, no consensus mechanism. Trust is bilateral and capability-based.

## The Stack

```
┌──────────────────────────────────┐
│  Applications                    │  Midgard, your app, etc.
├──────────────────────────────────┤
│  Allgard                         │  Federation model: primitives, conservation laws
├──────────────────────────────────┤
│  Leden                           │  Wire protocol: capabilities, sessions, gossip
└──────────────────────────────────┘
```

Applications implement domain logic on top of Allgard's model. Allgard's model rides on Leden's protocol. Each layer is independent — you could use Leden without Allgard, or define a different federation model on top of Leden.

## How a Gard Joins

1. Connect to any existing gard via Leden (bootstrap through a seed endpoint)
2. Learn about other gards through gossip (Leden discovery)
3. Establish bilateral capability relationships
4. Enforce Conservation Laws on hosted objects
5. Accept or reject Proofs from other gards

No step requires permission from a central authority. Federation is peer-to-peer.

## Key Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Trust model | Object capabilities (via Leden) | Fine-grained, delegatable, revocable. Proven. |
| Ownership | Single-owner, atomic transfer | No concurrent mutation, no CRDTs for the base case |
| Conservation | Six invariants, enforced unconditionally | Physics of the federation. Not policy — law. |
| Delegation | Non-transitive by default | Keeps the authority graph manageable. Explicit re-delegation. |
| Sovereignty | Domains are authoritative for hosted objects | No global state to coordinate. Each domain runs its own rules on top of the universal laws. |
| Enforcement | Bilateral verification | No global enforcer. Bad actors get excluded by reputation. |

## Specs

| Spec | What it covers |
|------|----------------|
| [PRIMITIVES.md](PRIMITIVES.md) | The six primitives: Object, Owner, Domain, Transform, Proof, Grant |
| [CONSERVATION.md](CONSERVATION.md) | The six conservation laws every domain must enforce |

## Open Questions

- **Domain sovereignty over supply**: can one domain mint independently of another, or is there a shared mint authority for cross-domain assets?
- **Cross-domain transfer routing**: bilateral or through a clearinghouse?
- **Wire format**: shared concern with Leden — MessagePack, Cap'n Proto, FlatBuffers?
- **Bootstrapping**: how does first capability exchange happen between unknown domains?
