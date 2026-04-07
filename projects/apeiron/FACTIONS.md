# Factions
<!-- id: apeiron.factions --> <!-- status: proposed --> <!-- summary: Faction mechanics — group Owners, membership Grants, territory as convention -->

Factions are groups of domain operators who cooperate. In Allgard terms, a faction is a `group` Owner — the primitive already exists in [PRIMITIVES.md](../allgard/PRIMITIVES.md). No new mechanisms. Just composition.

## What a Faction Is

A faction is a `group` Owner hosted on a domain. That's it.

The group Owner:
- Lives on a home domain (the faction hub)
- Owns shared Objects — treasury, blueprints, strategic intel
- Issues Grants to members defining what they can access
- Has a name, profile, and reputation — visible to outsiders via Leden observation
- Builds bilateral trust with other domains and factions through normal Allgard mechanisms

A faction is not a protocol construct. It's a pattern of Owners, Grants, Objects, and Domains composed in a specific way. The protocol doesn't know what a "faction" is. It knows what a group Owner with Grants is.

## Membership

Membership is a Grant from the faction Owner to a member Owner.

**Join:** The faction issues a Grant to the new member. The Grant's scope defines what the member can do — access the treasury, use faction facilities, view strategic data, vote on decisions (if the faction uses voting). Different members can receive different Grants. A founding member might have broader scope than a new recruit.

**Leave:** The member stops exercising the Grant. Optionally, the faction revokes it. Clean separation — no shared state to untangle. The member's domain is still sovereign. Their objects are still theirs.

**Kicked:** The faction revokes the Grant. The member loses access to faction resources immediately (subject to revocation propagation delay — see [PRIMITIVES.md](../allgard/PRIMITIVES.md#revocation)). Objects owned by the member stay with the member. Objects owned by the faction stay with the faction.

**Membership is visible.** Member domains advertise faction affiliation in Leden peer metadata:

```
metadata:
  faction: "Iron Compact"
  faction_owner: "iron_compact@hub.ironcompact"
```

Outsiders can verify membership by checking whether the claimed faction Owner has an active Grant to that domain's operator. The Grant is the proof. No self-declaration without backing.

## Multiple Factions

A domain can belong to multiple factions. Sovereignty means you choose your alliances. Nothing prevents a trading hub from joining both a mining consortium and a defense pact.

Some factions may require exclusivity — "revoke our Grant if you join a rival." That's a social rule enforced through Grant conditions or faction governance. Not a protocol constraint. If a faction discovers a member is also in a rival, it revokes the Grant. The member's choice: which faction to keep.

## Territory

Territory is social convention, not protocol state.

A faction's territory is the set of star systems whose domain operators are faction members. There's no protocol-level land claim, no invisible fence, no ownership of empty space. You can't "own" an unclaimed star — you can only claim it by deploying a domain.

Territory is advertised through Leden peer metadata. A faction might claim a region of the galaxy by having its members tag their systems:

```
metadata:
  faction: "Iron Compact"
  faction_territory: "core-sector-7"
```

This is a claim, not enforcement. Another faction can claim the same sector. Conflicting claims are resolved socially — through trade, diplomacy, or conflict. The protocol provides the primitives. The players provide the politics.

**Border systems** are domains at the edge of a faction's territory. They're strategically important — the first point of contact for outsiders, the first line of defense. A faction that loses its border systems loses its territorial claim in practice, regardless of metadata tags.

**Unclaimed space between factions** is genuinely unclaimed. No faction controls it. Anyone can deploy an outpost there. Route domains through contested space become strategically valuable — the faction (or independent operator) that controls the trade lane between two factions controls a chokepoint.

## Shared Resources

The faction Owner owns Objects. Members access them through Grants.

**Treasury.** Credits, materials, fuel — Objects owned by the faction Owner, held on the faction hub domain. Members with treasury Grants can withdraw (Transfer from faction to member, authorized by the Grant). Deposit is just a Transfer to the faction Owner. The faction domain's governance logic controls withdrawal limits and approval.

**Shared facilities.** The faction hub (or member domains) host facilities that faction members can use. Access is through Grants — same as facility rental in [ECONOMY.md](ECONOMY.md#facility-rental), but the Grant comes from the faction instead of being purchased per-use. Faction membership IS the payment — you contribute to the faction, the faction provides facility access.

**Intelligence.** Scout reports, trade data, resource maps — Objects owned by the faction Owner, observable by members through Grants. A faction that pools its members' scout data has a significant information advantage over solo operators. The faction hub aggregates reports and makes them available to all members.

**Shared blueprints.** Crafting scripts and ship blueprints developed by faction researchers, owned by the faction Owner. Members with the right Grant can use them. The faction can choose to keep blueprints exclusive (competitive advantage) or sell them (revenue).

## Governance

Internal to the faction. The protocol doesn't prescribe governance models.

The faction Owner's keys are controlled by whoever the members agree should control them. Three natural models:

**Autocratic.** One person holds the faction Owner's master key. They make all decisions — who joins, who's kicked, how the treasury is spent. Simple, fast, fragile. If the leader goes offline or goes rogue, the faction is stuck.

**Council.** A small group shares authority through the faction Owner's key hierarchy. The master key is held by M-of-N council members (social recovery pattern from [PRIMITIVES.md](../allgard/PRIMITIVES.md#recovery)). Council members have Device Grants with different scopes. Major decisions (admitting members, spending treasury) require M-of-N approval. Day-to-day operations are delegated.

**Democratic.** Members vote on decisions. The faction hub runs voting logic — a Raido script that counts Grants exercised as votes. The faction Owner's keys execute the outcome. This requires trust in the faction hub operator to run the voting script honestly. Verifiable if the voting script is content-addressed Raido bytecode — any member can re-execute and verify the count.

Most factions will start autocratic (one founder with a vision) and evolve toward council as they grow. I don't think many will go fully democratic — the overhead isn't worth it for most decisions. But the option exists.

The governance model is a social choice. The protocol provides the tools (Grants, key hierarchy, Raido scripts). What the faction builds with them is up to the members.

## Faction Reputation

The faction Owner builds its own bilateral reputation, separate from individual members.

When the faction (as the group Owner) trades with other domains, those transactions build the faction's reputation. A faction with a history of honest trades and verified Proofs is trusted. A faction that cheats loses trust.

Member reputations are separate. A faction member acting badly damages their own reputation, not the faction's directly. But a faction that consistently has bad-actor members loses reputation through association — trading partners notice patterns.

A faction's reputation is valuable. It takes time to build and is hard to replace. This creates a natural incentive for factions to police their own members — kicking bad actors protects the faction's collective reputation.

## Faction-Wide Trade Agreements

The faction Owner can negotiate bilateral agreements with external domains or other factions. These agreements can be delegated to members through Grants.

Example: The Iron Compact negotiates a trade agreement with the Free Traders guild. The agreement specifies exchange rates and transfer terms. The Iron Compact's faction Owner issues Grants to its members that include the scope to exercise this agreement. Now every Iron Compact member can trade with Free Traders under the negotiated terms.

This is Grant attenuation. The faction negotiates once, delegates to many. The external party sees the Grant chain: member → faction Owner → agreement. Verifiable. No per-member negotiation needed for standard terms.

Members can still negotiate their own bilateral deals outside the faction agreement. Sovereignty is preserved. The faction agreement is a floor, not a ceiling.

## What This Doesn't Cover

**War mechanics.** Factions will fight. Combat is domain-hosted, but coordinated multi-domain warfare — fleet operations, system sieges, blockades — needs the combat model specced first. The faction primitives (shared Grants, coordinated authority) provide the organizational structure. The combat spec provides the mechanics.

**Faction dissolution.** What happens when a faction falls apart? The faction Owner's Grants are revoked. Shared Objects go to whoever controls the faction Owner's keys at dissolution time. Members keep their own Objects. It's messy — like any real organization dissolving. The protocol doesn't make it clean because real dissolution isn't clean.

**Faction creation costs.** Creating a group Owner is creating an Owner — same process as any identity. The "cost" is social: convincing other domain operators to join and accept your governance. No protocol-level fee or approval.
