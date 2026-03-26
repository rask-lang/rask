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
├──────────────────────────────────┤
│  Raido                           │  Required for mint/burn, optional for general transforms
└──────────────────────────────────┘
```

Applications implement domain logic on top of Allgard's model. Allgard's model rides on Leden's protocol. Raido provides the verification layer — required for minting and burning ([Conservation Law 1](CONSERVATION.md#verifiable-minting)), optional for general transforms.

Each layer above Raido is independent — you could use Leden without Allgard, or define a different federation model on top of Leden. But every Allgard domain must run Raido for minting verification. See [Verifiable Transforms](#verifiable-transforms).

## How a Gard Joins

1. Connect to a seed endpoint via Leden (transport address from default seed list)
2. Hit the greeter — receive observation capabilities (catalog, peers, transfer inbox)
3. Learn about other gards through gossip (Leden discovery)
4. Start transacting — small transfers, verified Proofs, bilateral reputation builds

Steps 5 and 6 are runtime concerns — the software handles them:

5. Enforce Conservation Laws on hosted objects
6. Accept or reject Proofs from other gards based on own trust heuristics
7. Participate in audit gossip with trading partners

A domain operator writes game logic. The federation infrastructure — conservation law enforcement, proof verification, gossip participation, reputation tracking — runs automatically. Like TCP congestion control: it's a protocol duty, but nobody "operates" it.

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
| Bootstrapping | Founding cluster + seed currency | Curated seed network with pre-negotiated assets. Zero protocol-level trust anchor. Reputation is emergent. |

## Specs

| Spec | What it covers |
|------|----------------|
| [PRIMITIVES.md](PRIMITIVES.md) | The six primitives: Object, Owner, Domain, Transform, Proof, Grant |
| [CONSERVATION.md](CONSERVATION.md) | The six conservation laws every domain must enforce |
| [TRANSFER.md](TRANSFER.md) | Cross-domain transfer protocol: escrow, timeouts, partition recovery, Law 2 proof |
| [TRUST.md](TRUST.md) | Adversarial trust model: introductions, reputation, Sybil resistance |

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

### Founding Cluster

The network doesn't launch as "anyone can join an empty protocol." It launches as a curated cluster of domains run by people who know and trust each other.

The founding cluster is 5-20 domains with:
- **Pre-negotiated bilateral agreements.** Asset types, exchange rates, trust levels — all decided before launch. These domains start at Allied trust level with each other.
- **Standard asset types.** A common items package: basic materials, currency, equipment, character types. Domains in the cluster recognize these automatically. No per-domain negotiation needed for the standard set.
- **A seed currency.** One domain mints the first commodity money. Not protocol-privileged — it has no special status in the Conservation Laws. It's just the first currency that every founding domain agrees to recognize, giving it immediate liquidity. New domains price things relative to it until they have enough bilateral history to price directly. Over time, other currencies emerge and may replace it. Markets decide.

The founding cluster IS the product. A player joins, travels across 5-20 gards with seamless transitions and a shared currency. No compatibility dialogs, no conversion menus, no friction. You travel, you arrive, your stuff works. That's the experience from day one. Federation to gards outside the cluster is the advanced feature you grow into.

**Why this isn't centralization.** The founding domains have no protocol-level authority. They're ordinary domains with pre-negotiated conventions. Any domain can replicate the same setup — start a cluster, agree on asset types, mint a currency. The protocol doesn't privilege the first cluster. Network effects do, temporarily, until the network grows past them.

**Early adopter benefit.** Founding domains get established reputation, introduction capacity, and hub status before the network grows. They are the trusted introducers for the next wave. That's the incentive to build the cluster well — the founders' long-term position depends on the network they seed being worth joining.

### Seed List

The default seed list ships with the software. It points to domains in the founding cluster. These are ordinary domains — same protocol, same Conservation Laws, no special authority. But they have pre-negotiated bilateral agreements that make the first experience seamless.

If the seed domains go down, existing domains still talk to each other. Only brand-new domains with no other contacts are affected. Alternative seed lists are a config change, not a protocol change.

### What This Explicitly Avoids

- **No approval step.** A new domain doesn't need permission to join. It connects, it gossips, it transacts.
- **No trust anchor.** Seed operators don't vouch for anyone. Their endorsement carries no protocol-level weight.
- **No registry.** There is no list of "approved" domains. The network is the set of domains that speak Leden and respect Conservation Laws.
- **No certificate authority.** No domain's identity is validated by a central party. Identity is cryptographic keys. Reputation is transaction history.

### Reputation Is Emergent

Domains decide for themselves who to trust. But the federation provides structural incentives that make honesty the dominant strategy. See [TRUST.md](TRUST.md) for the full adversarial trust model — introduction-based trust, introducer accountability, Sybil resistance, and why high-trust networks are self-reinforcing.

The Conservation Laws give domains something concrete to verify: did this Proof check out? Did this transfer balance? That's the raw signal. [Audit gossip with Proofs](TRUST.md#audit-gossip-with-proofs) gives domains a way to pool that signal without trusting each other's summaries.

## Invisible Federation

Rask's design principle: safety is a property, not an experience. The same applies here: **federation is a property, not an experience.**

Every federation protocol that lost to a centralized competitor lost on UX. Email to Gmail, XMPP to Google Talk, ActivityPub to Twitter. The federated version optimized for operator freedom. The centralized version optimized for user experience. Users outnumber operators 1000:1.

I'm not repeating that mistake. The federation infrastructure — gossip, audit verification, reputation tracking, proof verification, conservation law enforcement — is runtime machinery. Domain operators don't think about it. End users never see it. It's like HTTPS encryption: it's there, it matters, and it's not in your face.

### What's Invisible

| Concern | Who handles it | User/operator sees |
|---------|---------------|-------------------|
| Gossip participation | Runtime (automatic) | Nothing |
| Audit verification | Runtime (automatic) | Nothing |
| Reputation tracking | Runtime (automatic) | Nothing (operators can inspect via admin tools) |
| Proof verification | Runtime (automatic) | Nothing |
| Conservation law enforcement | Runtime (automatic) | Nothing |
| Standard asset transfer | Runtime (bilateral agreements) | One-click crossing between domains with established agreements |
| Currency conversion | Runtime (bilateral rates) | Balance shown in local currency |

### What Surfaces

Within the founding cluster and between gards with established agreements: nothing. Travel is seamless. The player travels, arrives, and plays. No reports, no dialogs, no friction.

Friction only appears at the edges — when a player ventures to an unaffiliated gard or carries exotic items the destination doesn't recognize:

- **Exotic items** travel [sealed](../midgard/README.md#asset-fidelity) — safe, intact, unusable on the destination. A brief notification, not a dialog.
- **Unaffiliated gards** show what transfers at full fidelity and what travels sealed. This is rare within any established network.
- **Low-reputation gards** trigger a trust warning.
- **Changed compatibility** shows a delta since the last visit.

Nothing is ever lost or downgraded. The worst case is sealed transfer.

### Progressive Disclosure

The information is always available — compatibility reports, proof chains, bilateral agreement details. But it's not the primary flow. Players who want to inspect the federation machinery can. Players who don't never see it. Three levels:

1. **Default:** everything works, show nothing
2. **Edge case:** brief notification about sealed items or trust warnings
3. **Deep inspection:** full details on request

## Domain Sovereignty over Supply

Each domain mints its own assets independently. No shared mint authority, no protocol-level currency, no central bank.

Cross-domain value is market-determined. If Domain A's "iron ingot" is worth 3 of Domain B's "gold coins," that's between A and B. The protocol doesn't enforce exchange rates — bilateral trade agreements do.

**Why not a shared currency?** Because "shared minting authority" is centralization. Who runs it? Who sets issuance rates? Every answer reintroduces a central party that the architecture explicitly rejects.

**What about fragmentation?** It happens. That's the honest cost of sovereignty. In practice, commodity money emerges — assets with intrinsic utility (crafting materials, fuel, compute credits) become de facto currencies because they're useful, scarce, and fungible. The [founding cluster](#founding-cluster) seeds the first commodity money by convention, giving it initial liquidity. After that, markets discover the rest.

What the protocol provides:
- **Asset type registration** — domains publish what they mint and its properties (via catalog observation from bootstrapping)
- **Bilateral exchange** — two domains agree on rates for a specific trade
- **Conservation Law 1** — every domain's supply is auditable (`total_minted - total_burned = total_existing`)
- **Supply audit** — verifiable supply reports, cross-checked by gossip (see below)

Convention handles the rest.

### Supply Audit

A domain that wants others to trust its currency offers a **supply audit capability** through its greeter. The audit returns per-asset-type totals: minted, burned, circulating. Self-reported — the domain controls its own ledger.

Self-reported totals alone are just claims. But with [verifiable minting](CONSERVATION.md#verifiable-minting), the supply audit becomes mechanical:

**Verifiable minting scripts.** Every mint and burn is a Raido script. The audit includes not just totals but the content-addressed scripts that produced them. A verifying domain fetches the scripts, re-executes with the declared inputs, and checks the outputs against the claimed totals. If the math doesn't work, the audit is fraudulent. No trust required.

**Bilateral verification.** Every cross-domain transfer already carries a Proof (Conservation Law 4). Domain B accumulates a partial view of Domain A's economy — every object that crossed the boundary, every mint and burn B witnessed firsthand. B can check A's self-reported numbers against its own records. If A claims 1000 total minted but B alone has received 1200 through verified transfers, A is lying. No new protocol needed — this falls out of existing Proof mechanics.

**Audit gossip.** Domains share their bilateral observations with each other. B tells C: "I've verified 500 units from A." C says: "I've verified 600." D says: "I've verified 300." Together they've witnessed 1400 — if A claims 1000 total supply, the stories don't add up.

Audit gossip rides on existing Leden gossip infrastructure. It's not a new protocol — it's a new message type in the peer gossip exchange. Domains that care about supply integrity participate. Domains that don't, ignore it.

**Proof chain inclusion.** The audit includes a Proof chain — the causal ordering mechanism from Law 4 applied to mint and burn events. A verifying domain can't check the complete chain (it doesn't have the issuer's full history), but it can verify that the events it witnessed are included. If a transfer B received from A doesn't appear in A's published audit chain, the audit is fraudulent.

### What This Can't Do

See internal mutations. A domain that mutates objects internally without exporting them is opaque. But internal mutations don't change supply (minting is verifiable) and don't affect other domains. The blind spot is harmless.

## Cross-Domain Transfer Routing

Bilateral. Direct domain-to-domain transfers over Leden sessions.

If Domain A wants to send an object to Domain C and they've never met, two paths:

1. **Introduction.** A asks a mutual contact B to introduce it to C (Leden's `Introduce` operation). A and C establish a direct relationship, then transfer. B is out of the loop after the introduction.

2. **Intermediary chain.** A transfers the object to B, B transfers to C. B holds the object briefly during transit. This requires an **escrow transform** — A transfers to B with a condition: "forward to C within N seconds, or it returns to me." This composes from existing primitives: Transform + Grant + expiry.

No routing protocol. No clearinghouse. If you need to reach a distant domain, you go through domains you both know. The gossip layer (Leden discovery) tells you who knows whom.

Both direct transfers and intermediary chains use the bilateral escrow protocol defined in [TRANSFER.md](TRANSFER.md). The escrow transform for intermediary chains is a conditional transfer with a forwarding deadline — forward within N seconds, or it returns to the sender.

## Verifiable Transforms

[Raido](../raido/) provides mechanical verification for transforms. A verifiable transform includes:

1. A Raido script (content-addressed bytecode chunk)
2. The inputs to the script
3. The outputs (the claimed result)

The receiving domain fetches the script (Leden content store), re-executes with the same inputs. Raido's determinism guarantees identical output. If the result matches, the transform is mechanically verified. If not, the Proof is fraudulent.

### Required: Verifiable Minting

Every `create` and `destroy` Transform must be backed by a Raido script. This is the structural enforcement mechanism for [Conservation Law 1](CONSERVATION.md#verifiable-minting).

A domain's minting logic is a content-addressed Raido script. Any trading partner can fetch it and re-execute. This means:

- **Supply audits are mechanical.** Not "I claim these totals" but "here's the script that produces them, run it yourself."
- **Hidden inflation is impossible.** You can't mint without a script, and the script is verifiable.
- **Sovereignty is preserved.** You write whatever minting logic you want. You just can't hide it.

A domain that runs only internal objects and never exports anything doesn't need trading partners to verify its minting. But the moment it tries to export value, the receiving domain will demand the minting script. No script, no acceptance.

### Optional: Verifiable General Transforms

For transforms beyond minting — crafting recipes, damage calculations, economic formulas — Raido verification is optional. Domains negotiate "verifiable-transform" as a Leden capability.

Without it: Domain A says "I ran this transform and here's the result." Domain B checks the Proof structure (signature, causal link) but trusts A on the computation. This is fine for simple operations like transfer and split.

With it: Domain B re-executes the computation. Bilateral trust becomes bilateral verification. Domains that support verifiable transforms can offer stronger guarantees, which matters for reputation.

### Capability Negotiation

Two domains agree to verifiable transforms during Leden capability negotiation. They agree on a Raido chunk format version. From then on, cross-domain Proofs for scripted transforms include the script hash, inputs, and outputs. The receiving domain re-executes to verify.

This is the same pattern as observation in Leden — negotiated, optional, composable. No domain is forced to support general verification. But every domain must support verifiable minting — that's not negotiable.

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
