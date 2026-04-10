# Reputation
<!-- id: apeiron.reputation --> <!-- status: proposed --> <!-- summary: Bilateral attestation system — how domains track and share trust history -->

Allgard provides structural trust — conservation laws, capabilities, proof chains. That prevents cheating. It doesn't tell you whether someone is a good trading partner. A domain that never cheats can still be slow to deliver, overcharge for services, or abandon contracts.

Reputation fills the gap between "provably honest" and "worth doing business with."

## The Problem

The founding cluster starts with pre-established bilateral trust. The five operators know each other. But when player #6 shows up, nobody knows them. They have a valid identity (Conservation Law 7 — introduction-based), a starter ship, and zero history. How does anyone decide whether to trade with them? How does the player build credibility?

Without reputation, every interaction starts from zero. New players face a cold start. Established players have no way to evaluate strangers. Factions can't assess potential recruits. The network has trust (cryptographic) but not reputation (behavioral).

## Design Principles

**Bilateral, not global.** There is no galaxy-wide reputation score. Each domain tracks its own experience with each entity. Reputation is a collection of bilateral records, not a ranking.

**Attestation, not aggregation.** Domains attest to specific facts: "this entity completed 47 courier contracts with me." They don't compute a score. The recipient of an attestation decides what it means.

**Verifiable where possible.** Attestations about on-chain events (completed transfers, fulfilled contracts, combat outcomes) are backed by proof chains. Anyone can verify. Attestations about subjective experience ("reliable partner," "slow to respond") are opinions — weighted by the attester's own reputation.

**No protocol enforcement.** The protocol doesn't know what reputation is. It's a pattern built on Allgard objects and Leden observation. Domains that ignore reputation can. They'll just have fewer trading partners.

## Attestation Format

An attestation is an Allgard object, owned by the attesting domain, containing structured claims about another entity.

```
attestation:
  subject: <owner_id>           # Who this is about
  attester: <domain_id>         # Who is making the claim
  epoch: <beacon_epoch>         # When (anchored to beacon timeline)
  claims:
    contracts_completed: 47
    contracts_failed: 2
    contracts_abandoned: 0
    trade_volume_credits: 125000
    first_interaction_epoch: 1042
    last_interaction_epoch: 1891
    combat_encounters: 3
    combat_cheating_detected: 0
    custom: {}                  # Domain-specific claims
  proof_refs: [<proof_id>, ...]  # Optional: proof chain references for verifiable claims
  signature: <domain_signature>
```

### Claim Categories

**Verifiable claims.** Backed by proof chain references. "Completed 47 contracts" points to 47 contract completion proofs. Anyone can check. These are the hard currency of reputation — unfakeable unless the attesting domain fabricates proof chains (which requires fabricating an entire transaction history, detectable by bilateral audit).

**Observable claims.** Based on Leden session data that the attester witnessed but that isn't a full Allgard proof. "Responded to trade queries within 5 seconds on average." Hard to verify independently, but the attester stakes their own reputation on the claim.

**Subjective claims.** Opinions. "Good trading partner." "Trustworthy in combat situations." Unfalsifiable. Weighted entirely by trust in the attester.

The founding cluster publishes a **standard claims vocabulary** — the field names and semantics in the `claims` block above. Domains can extend it with custom claims. Standard vocabulary means attestations are comparable across domains. Custom vocabulary means domains aren't limited to what the founders thought of.

## Sharing Reputation

Attestations are shared through normal Allgard/Leden mechanisms:

**Direct request.** Entity A asks domain B for attestations about entity C. Domain B shares (or doesn't — sharing is voluntary). Standard Leden observation query.

**Proactive sharing.** A domain publishes attestation summaries in its Leden peer metadata. "I've traded with 200 entities, here are aggregate stats." Not individual attestations — summaries that signal the domain's activity level.

**Faction pooling.** A faction (group Owner) aggregates member attestations about external entities. The faction hub collects attestations from members and makes the pooled data available through faction Grants. This is a significant information advantage — a 20-member faction has 20x the bilateral history of a solo player.

**Negative attestations.** A domain attests that an entity cheated, abandoned a contract, or behaved badly. These propagate through the same mechanisms. A domain that receives a negative attestation weighs it by trust in the attester. Negative attestations from untrusted sources are noise. From trusted trading partners, they're actionable.

### Asymmetry Is the Point

Different entities see different reputations for the same subject. Domain A trusts domain B's attestations. Domain C doesn't trust domain B. So A and C evaluate the same subject differently. This is correct — reputation SHOULD be subjective. A global score is gameable. Distributed bilateral assessment is robust.

The cost: new players face a cold start. They have no attestation history with anyone. The founding cluster mitigates this through the introduction chain — every new identity was introduced by someone. The introducer's reputation backs the introduction. "I introduced this player" is an implicit attestation.

## Reputation and Access

Domains can gate access based on reputation. Not enforced by protocol — enforced by the domain's own logic.

**Facility access.** A shipyard that only serves entities with 10+ completed contracts from trusted attesters. The domain checks attestations before issuing a Grant.

**Trade terms.** Better prices for established traders. Higher credit limits for entities with long history. Standard bilateral negotiation, informed by reputation.

**Faction recruitment.** Factions can set reputation requirements for membership. "Must have 50+ contracts completed across 3+ domains in the founding cluster." The faction hub verifies against collected attestations.

**Zone access.** A route domain that denies entry to entities with combat cheating attestations from trusted sources. Consent-on-entry can include reputation checks.

None of this is mandatory. A domain that accepts everyone can. A domain that trusts nobody can gate everything. Sovereignty.

## Gaming Reputation

### Sybil Attestations

A domain creates fake entities, trades with itself, attests to the fake entities' reliability. Then presents the attestations as proof of good reputation.

**Mitigation:** Conservation Law 7 (can't mass-produce trusted identities). Every identity needs an introduction from an existing trusted entity. Creating fake identities requires burning real introduction capacity. The fake entities also need real proof chains for verifiable claims — fabricating a history of 50 completed contracts means fabricating 50 sets of transfer proofs, which means the fake entities must actually transact. At that point, the "fake" reputation is partly real — the entity actually did things.

**Deeper mitigation:** Attestation weight depends on trust in the attester. Attestations from unknown domains carry little weight. A player can create a domain and attest to themselves — but nobody trusts the new domain's attestations until the domain itself builds reputation. Circular bootstrapping doesn't work because trust flows from established to new, not the other way.

### Reputation Bombing

A coalition of domains issues coordinated negative attestations against a target. The target's reputation is damaged across the network.

**Mitigation:** Bilateral assessment. Each domain evaluates attestations individually, weighted by their own trust in each attester. A coalition of domains you don't trust can't damage your reputation with domains that DO trust you. The attack only works within the coalition's trust network — which is also where the coalition was already powerful.

**Real risk:** A major faction issues negative attestations against a solo player. If the faction is widely trusted, the attestations are widely believed. This is real social power, and it's intentional. Reputation systems reflect power structures. The mitigation is the same as in real life — build relationships outside the faction's sphere, join a counter-faction, or move to a region where the faction has less influence.

### Attestation Staleness

An entity behaved well for 100 epochs, then went rogue. Old attestations are positive. New ones are negative.

**Mitigation:** Attestations have epochs. Recent attestations override older ones from the same attester. Consumers of attestations can apply time-weighting — recent data matters more. The standard claims vocabulary includes `last_interaction_epoch` specifically for this.

## The Introduction Chain as Seed Reputation

Every identity in the network was introduced by someone. The introduction is the first attestation — "I vouch for this entity enough to introduce them." The introducer's reputation backs it.

For founding cluster players: introduced by a founding domain. High initial trust, because the founding domains have the most history and the most bilateral relationships.

For frontier players introduced by a solo operator: lower initial trust, because the introducer has less reputation to stake. The frontier player builds reputation through their own activity, starting from a lower baseline.

This creates a natural gradient. Core players (near the founding cluster, introduced by established domains) start with better reputation access. Frontier players start with less. The gradient isn't a gate — it's a consequence of trust being built from bilateral experience. Frontier players who build strong trading records develop strong reputations. It just takes longer because they start further from the trust center.

## Stage 1 Testing

The monolith tracks reputation internally. Same logic, not distributed:

- Each logical domain maintains attestation records for entities it interacts with.
- AI traders build reputation through completed trades. Verify attestations accumulate correctly.
- New player starts with zero reputation. Completes courier contracts. Verify attestation records grow.
- Test reputation gating: a facility requires 5+ completed contracts. New player can't access it. After completing 5 contracts, they can.
- Test negative attestations: an entity fails a contract. Verify the attesting domain records it. Verify other domains weight it appropriately.
- Test staleness: entity has old positive and new negative attestations. Verify time-weighted evaluation reflects the change.
- Simulate Sybil attack: a domain tries to inflate reputation through self-trading. Verify the resulting attestations carry low weight from external domains.

## Interaction With Other Systems

**Factions.** Faction reputation is separate from member reputation (per [FACTIONS.md](FACTIONS.md)). This spec adds the mechanism: faction hubs aggregate member attestations, building a pooled knowledge base. Faction membership itself becomes a reputation signal — "member of the Iron Compact" carries the Compact's collective reputation.

**Contracts.** Every completed or failed contract generates attestation data. Contracts (see [CONTRACTS.md](CONTRACTS.md)) are the primary source of verifiable reputation claims. The reputation system gives contracts persistent economic meaning beyond the immediate transaction.

**Knowledge.** Survey data, recipes, and intel (see [KNOWLEDGE.md](KNOWLEDGE.md)) have trust requirements. You need to trust the source to value the knowledge. Reputation is how you evaluate sources. A recipe sold by a domain with 200 verified trades is more trustworthy than one from an unknown entity.

**Combat.** Combat outcomes are verifiable events. "Provably cheated in combat" is the most damaging attestation possible — backed by deterministic proof (re-run the combat script, see the divergence). Combat reputation is binary: either you've been caught cheating or you haven't.

## What This Spec Doesn't Cover

**Reputation display.** How clients present reputation to players. UI design, not protocol design. But the founding cluster should publish a reference implementation — probably a trust-weighted summary showing recent history from the player's own bilateral network.

**Reputation markets.** Buying and selling attestations. This will happen — it's just bilateral trading of Allgard objects. Whether it's healthy or corrosive depends on the ecosystem. Probably both. Not worth prescribing rules for.

**Identity reputation vs. domain reputation.** An Owner can operate multiple domains. Does reputation attach to the Owner or the domain? Both — an Owner builds reputation through all their domains, and each domain has its own operational history. The attestation format tracks both (`subject` can be an Owner ID or a domain ID). How consumers aggregate across an Owner's domains is their choice.
