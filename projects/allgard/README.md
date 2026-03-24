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
        ↕ Raido (optional extension: verifiable transforms)
```

Applications implement domain logic on top of Allgard's model. Allgard's model rides on Leden's protocol. Each layer is independent — you could use Leden without Allgard, or define a different federation model on top of Leden.

[Raido](../raido/) is a cross-cutting extension, not a layer. Domains that both support Raido can verify each other's transform logic mechanically instead of relying on bilateral trust alone. See [Verifiable Transforms](#verifiable-transforms).

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
| Supply | Per-domain sovereignty | No shared mint authority. Cross-domain value is market-determined. |
| Transfer routing | Bilateral, with introduction or intermediary chains | No clearinghouse. Reach distant domains through mutual contacts. |
| Bootstrapping | Seed nodes, zero trust | No approval, no registry, no trust anchor. Reputation is emergent. |

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
| Supply audit | Verify the domain's self-reported minting and supply (optional) |

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

## Domain Sovereignty over Supply

Each domain mints its own assets independently. No shared mint authority, no protocol-level currency, no central bank.

Cross-domain value is market-determined. If Domain A's "iron ingot" is worth 3 of Domain B's "gold coins," that's between A and B. The protocol doesn't enforce exchange rates — bilateral trade agreements do.

**Why not a shared currency?** Because "shared minting authority" is centralization. Who runs it? Who sets issuance rates? Every answer reintroduces a central party that the architecture explicitly rejects.

**What about fragmentation?** It happens. That's the honest cost of sovereignty. In practice, commodity money emerges — assets with intrinsic utility (crafting materials, fuel, compute credits) become de facto currencies because they're useful, scarce, and fungible. Nobody decrees it. Markets discover it.

What the protocol provides:
- **Asset type registration** — domains publish what they mint and its properties (via catalog observation from bootstrapping)
- **Bilateral exchange** — two domains agree on rates for a specific trade
- **Conservation Law 1** — every domain's supply is auditable (`total_minted - total_burned = total_existing`)
- **Supply audit** — verifiable supply reports, cross-checked by gossip (see below)

Convention handles the rest.

### Supply Audit

A domain that wants others to trust its currency offers a **supply audit capability** through its greeter. The audit returns per-asset-type totals: minted, burned, circulating. Self-reported — the domain controls its own ledger.

Self-reported numbers alone are just claims. Three mechanisms make them trustworthy:

**Bilateral verification.** Every cross-domain transfer already carries a Proof (Conservation Law 4). Domain B accumulates a partial view of Domain A's economy — every object that crossed the boundary, every mint and burn B witnessed firsthand. B can check A's self-reported numbers against its own records. If A claims 1000 total minted but B alone has received 1200 through verified transfers, A is lying. No new protocol needed — this falls out of existing Proof mechanics.

**Audit gossip.** Domains share their bilateral observations with each other. B tells C: "I've verified 500 units from A." C says: "I've verified 600." D says: "I've verified 300." Together they've witnessed 1400 — if A claims 1000 total supply, the stories don't add up.

This is village reputation. Nobody has complete information. Everyone has overlapping partial information. Liars get caught because the numbers from independent observers don't reconcile. The more domains that trade with you, the harder it is to inflate undetected.

Audit gossip rides on existing Leden gossip infrastructure. It's not a new protocol — it's a new message type in the peer gossip exchange. Domains that care about supply integrity participate. Domains that don't, ignore it.

**Proof chain inclusion.** The audit includes a Proof chain — the causal ordering mechanism from Law 4 applied to mint and burn events. A verifying domain can't check the complete chain (it doesn't have the issuer's full history), but it can verify that the events it witnessed are included. If a transfer B received from A doesn't appear in A's published audit chain, the audit is fraudulent.

### What This Can't Do

Prevent fraud. A sovereign domain can lie about internal activity that never crosses a boundary. If A mints 10000 units and only circulates 100 externally, no one can see the other 9900.

That's fine. It's the same limitation real economies have. The point isn't omniscience — it's that fraud at scale is detectable. A domain running a major currency has many trading partners. Each one holds a piece of the picture. The gossip network assembles those pieces. Discrepancies surface.

## Cross-Domain Transfer Routing

Bilateral. Direct domain-to-domain transfers over Leden sessions.

If Domain A wants to send an object to Domain C and they've never met, two paths:

1. **Introduction.** A asks a mutual contact B to introduce it to C (Leden's `Introduce` operation). A and C establish a direct relationship, then transfer. B is out of the loop after the introduction.

2. **Intermediary chain.** A transfers the object to B, B transfers to C. B holds the object briefly during transit. This requires an **escrow transform** — A transfers to B with a condition: "forward to C within N seconds, or it returns to me." This composes from existing primitives: Transform + Grant + expiry.

No routing protocol. No clearinghouse. If you need to reach a distant domain, you go through domains you both know. The gossip layer (Leden discovery) tells you who knows whom.

## Verifiable Transforms

Conservation Law 4 requires every Transform to carry a Proof. For simple operations (transfer ownership, burn), the Proof is a signature and a causal link. The receiving domain checks the signature, verifies the chain, done.

But some Transforms have *logic* — crafting recipes, damage calculations, economic formulas. Today those are trust-based: Domain A says "I ran this transform and here's the result." Domain B checks the Proof structure but can't independently verify the computation. B trusts A or doesn't.

[Raido](../raido/) changes this. A verifiable transform includes:

1. A Raido script (content-addressed bytecode chunk)
2. The inputs to the script
3. The outputs (the claimed result)

The receiving domain fetches the script (Leden content store), re-executes with the same inputs. Raido's determinism guarantees identical output. If the result matches, the transform is mechanically verified. If not, the Proof is fraudulent.

### What This Gives You

- **Law 4 (Causal Ordering)** goes from "check the receipt" to "re-run the computation." Proofs for scripted transforms become independently verifiable.
- **Bilateral trust** becomes **bilateral verification** for any transform backed by a Raido script.
- **Supply audits** can include verifiable minting/burning logic — not just "I claim these totals" but "here's the script that computes them, run it yourself."

### What This Doesn't Change

- Simple transforms (transfer, burn) don't need Raido. Signature + causal link is sufficient.
- Raido is an optional extension. Domains negotiate "verifiable-transform" as a Leden capability. Domains that don't support it fall back to trust-based Proofs.
- Domain sovereignty is preserved. A domain can still run private logic internally. Verifiable transforms only apply to cross-domain Proofs where both sides opt in.

### Capability Negotiation

Two domains agree to verifiable transforms during Leden capability negotiation. They agree on a Raido chunk format version. From then on, cross-domain Proofs for scripted transforms include the script hash, inputs, and outputs. The receiving domain re-executes to verify.

This is the same pattern as observation in Leden — negotiated, optional, composable. No domain is forced to support it. But domains that do can offer stronger trust guarantees, which matters for reputation.

## Prior Art and Differentiation

Allgard draws from existing work. Here's what I took, what I didn't, and why.

| System | What Allgard shares | Where Allgard diverges |
|--------|--------------------|-----------------------|
| **Blockchains** (Ethereum, Solana) | Conservation of supply, append-only history, auditable state transitions | No global ledger, no consensus mechanism. Trust is bilateral, not majority-vote. No gas fees — rate limits instead. Domains are sovereign; there's no "the chain." |
| **CRDTs** (Automerge, Yjs) | Distributed state without central coordination | CRDTs solve concurrent mutation via merge semantics. Allgard prevents concurrent mutation entirely — single owner, atomic transfer (Law 2). No merge conflicts because there's nothing to merge. CRDTs are the right tool when you need shared mutable state. Allgard says: don't share mutable state. |
| **Object capability systems** (E/CapTP, Spritely, Cap'n Proto) | Capability-based authority, fine-grained delegation, revocation | Allgard uses capabilities (via [Leden](../leden/)) but adds conservation laws on top. Pure ocap systems don't constrain *what* capabilities can do economically — you can mint infinite tokens if you hold the minting cap. Allgard's laws constrain the physics. Capabilities control *who can act*; conservation laws control *what actions are valid*. |
| **Federation protocols** (ActivityPub, Matrix) | Decentralized, no central authority, domain sovereignty | ActivityPub federates *content* (posts, follows). Allgard federates *objects with ownership and value*. The conservation laws have no equivalent in social federation — there's no "supply" of posts to conserve. Matrix is closer (federated state), but uses eventual consistency and state resolution. Allgard uses single-owner semantics to avoid state conflicts entirely. |
| **Game economies** (EVE Online, WoW) | Conservation of supply, value sinks, anti-inflation mechanics | These are single-domain systems with a central authority. Allgard is multi-domain with no authority. The conservation laws are what a single game server enforces internally, generalized to work across sovereign domains that don't trust each other. |

### The Core Difference

Most distributed systems choose between **consistency** (blockchains — everyone agrees on one truth) and **availability** (CRDTs — everyone can write, merge later). Allgard sidesteps the tradeoff by restricting the data model: single owner per object, atomic transfers, no shared mutable state. This makes consistency trivial (only one writer) and availability a domain-local concern (your domain is always available to you).

The cost: no shared mutable state. You can't have two domains simultaneously editing the same object. If you need that, use Grants to give temporary scoped access, and accept that the Grant holder operates under the owning domain's authority. This is a real limitation, and it's intentional — shared mutable state across trust boundaries is where distributed systems complexity explodes.

## Open Questions

- **Wire format**: shared concern with Leden — deferred. Implementation detail that doesn't affect the model. See [Leden](../leden/README.md).
