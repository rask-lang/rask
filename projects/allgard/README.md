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

1. Connect to a seed endpoint via Leden (transport address from default seed list)
2. Hit the greeter — receive observation capabilities (catalog, peers, transfer inbox)
3. Learn about other gards through gossip (Leden discovery)
4. Start transacting — small transfers, verified Proofs, bilateral reputation builds
5. Enforce Conservation Laws on hosted objects
6. Accept or reject Proofs from other gards based on own trust heuristics

No step requires permission from a central authority. See [Bootstrapping](#bootstrapping) for details.

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

## Bootstrapping

How does the first capability exchange happen between domains that have never met?

**Short answer:** Leden seeds get you connected. The greeter gives strangers a minimal capability set. Trust builds from transactions, not introductions.

### The Layers

1. **Network bootstrap (Leden)** — a new domain connects to a seed endpoint (just a transport address). Seeds are listed in a default config file. Anyone can run a seed; they're not special. See [Leden discovery](../leden/discovery.md).

2. **Capability bootstrap (Allgard)** — after connecting via Leden, the new domain hits the seed's greeter. The greeter gives every stranger the same thing: observation capabilities. You can see what the domain hosts, what asset types exist, what services are offered. No approval, no identity check.

3. **Trust bootstrap (bilateral)** — the new domain starts transacting. Small transfers, verified Proofs. Each successful transaction builds bilateral reputation. Over time, domains that consistently produce valid Proofs get faster processing, higher rate limits, larger transfer caps. Domains that produce invalid Proofs get cut off.

### What the Greeter Gives Strangers

| Capability | Purpose |
|-----------|---------|
| Peer observation | See other domains in the network (gossip accelerator) |
| Catalog observation | See what asset types and services the domain offers |
| Transfer inbox | Submit small cross-domain transfers for verification |

That's it. No write access to hosted objects, no minting authority, no delegation. A stranger can look and transact. Everything else is earned through bilateral reputation.

### Seed List

The default seed list ships with the software. It points to domains run by the project maintainers. These are ordinary domains — same protocol, same Conservation Laws, no special authority. They're just the first nodes in the gossip graph.

If the seed domains go down, existing domains still talk to each other. Only brand-new domains with no other contacts are affected. Alternative seed lists are a config change, not a protocol change.

### What This Explicitly Avoids

- **No approval step.** A new domain doesn't need permission to join. It connects, it gossips, it transacts.
- **No trust anchor.** Seed operators don't vouch for anyone. Their endorsement carries no protocol-level weight.
- **No registry.** There is no list of "approved" domains. The network is the set of domains that speak Leden and respect Conservation Laws.
- **No certificate authority.** No domain's identity is validated by a central party. Identity is cryptographic keys. Reputation is transaction history.

### Reputation Is Emergent

I'm not specifying a reputation system. Domains decide for themselves who to trust based on their own transaction history. A domain that runs a busy marketplace will have different trust heuristics than a domain running a private guild server.

The Conservation Laws give domains something concrete to verify: did this Proof check out? Did this transfer balance? That's the raw signal. What domains do with that signal is their business.

## Open Questions

- **Domain sovereignty over supply**: can one domain mint independently of another, or is there a shared mint authority for cross-domain assets?
- **Cross-domain transfer routing**: bilateral or through a clearinghouse?
- **Wire format**: shared concern with Leden — MessagePack, Cap'n Proto, FlatBuffers?
