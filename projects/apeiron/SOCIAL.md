# Social Tools
<!-- id: apeiron.social --> <!-- status: proposed --> <!-- summary: Coordination conventions — fleets, bookmarks, shared intelligence, faction operations -->

[FACTIONS.md](FACTIONS.md) defines what a faction IS — a group Owner with Grants. This spec defines what factions DO day-to-day: coordinate fleets, share navigation data, run operations, and build the social fabric that makes multiplayer work.

These are conventions, not protocol. The founding cluster publishes standard object formats and interaction patterns. Factions adopt and extend them. A faction that invents better coordination tools has a real advantage.

## The Problem

Allgard gives you Grants, Objects, Transfers, and Transforms. You CAN coordinate a 20-ship fleet operation with raw primitives. You'd issue 20 individual Grants, negotiate 20 bilateral transfers for fleet fuel, manually share sensor data through observation queries, and track formation through per-ship position updates.

Nobody will do this. The primitives are necessary but not sufficient for coordination. Standard tools built on top of the primitives make the difference between "technically possible" and "actually done."

EVE Online learned this: corps need hangars, fleet invites, shared bookmarks, alliance standings, and chat channels. Without them, the corps that COULD exist in the protocol wouldn't exist in practice. Apeiron needs the same — standard tools, not special protocol.

## Fleet Mechanics

A fleet is a temporary coordination structure. Ships that move, fight, and scout together.

### Fleet Object

```
fleet:
  id: <object_id>
  commander: <owner_id>
  name: "Trade Convoy Bravo"
  members:
    - ship_id: <object_id>
      owner: <owner_id>
      role: commander | wing_lead | member | scout
      joined_epoch: <beacon_epoch>
  formation:
    type: convoy | screen | dispersed | custom
    params: {}                          # Formation-specific parameters
  orders:
    destination: <domain_id>
    waypoints: [<domain_id>, ...]
    rules_of_engagement: defensive | aggressive | hold_fire
    retreat_policy: commander_calls | individual_choice
  status: forming | in_transit | engaged | disbanded
  comms_channel: <object_id>           # Reference to a shared observation object for fleet chat
```

A fleet is an Allgard object owned by the commander. Members receive Grants to observe the fleet object (see formation, orders, member list) and to submit updates (position reports, status changes).

### Forming a Fleet

1. Commander creates the fleet object on their current domain.
2. Commander issues **fleet invitation Grants** to prospective members. The Grant includes: fleet object reference, role assignment, expected duration.
3. Prospective members inspect the Grant — they see the fleet's purpose, commander, current members, and orders. Informed consent.
4. Accepting the Grant adds the member to the fleet object (Transform: update member list).
5. Fleet is active when enough members join (commander decides when to depart).

### Fleet Communication

The fleet object includes a reference to a **comms channel** — a Leden observation object. All fleet members observe it. Messages are GDL chat events scoped to the group (per GDL spec). Fleet chat travels through whatever domain currently hosts the fleet object.

**Command channel.** The commander can push orders to the fleet through a separate observation stream — read-only for members. Orders update in real-time. Members see "new waypoint: system 4822" or "all ships: defensive posture" without the commander manually messaging each one.

### Fleet Movement

When a fleet jumps between domains, the fleet object transfers alongside the ships. All members' ships transfer via standard Allgard bilateral escrow. The fleet object coordinates the jump:

1. Commander commits fleet jump order.
2. Fleet object records intended destination and member list.
3. Each member's ship transfers individually (standard cross-domain transfer).
4. Destination domain receives ships and verifies they're fleet members.
5. Fleet object updates member positions as arrivals confirm.

Ships that fail to arrive (destroyed en route, left behind, out of fuel) are marked in the fleet object. The commander sees the fleet's actual composition in real-time.

### Fleet Combat

In combat, the fleet object carries the fleet-level strategic orders per [COMBAT.md](COMBAT.md). The commander submits fleet orders; the combat script translates them to per-ship actions via the execution script.

**Delegation.** Wing leads can have Grant-delegated authority over their wing's orders. The commander sets fleet strategy; wing leads set wing tactics. This is Grant attenuation — the commander's fleet Grant delegates a subset to each wing lead.

### Fleet Dissolution

Commander disbands the fleet. The fleet object is either destroyed (cleanup) or archived (record of the operation). Members' Grants are revoked. Ships return to individual operation.

Alternatively, members can leave by ceasing to exercise their Grant. A member who jumps away from the fleet and doesn't follow orders is effectively gone. The commander can revoke their Grant to make it official.

## Shared Bookmarks

A bookmark is a reference to a location — a star, a specific domain, a point of interest within a domain. Personal bookmarks are trivial (client-side data). Shared bookmarks are the useful ones.

### Bookmark Object

```
bookmark:
  id: <object_id>
  creator: <owner_id>
  name: "Rich titanium belt, system 4822"
  location:
    star_id: <int>                     # Galaxy-level reference
    domain_id: <domain_id>             # Optional: specific domain
    position: [x, y, z]               # Optional: position within domain (GDL coordinates)
  category: navigation | resource | danger | trade | social | custom
  notes: "Three extraction sites, quality 0.85. Contested — Iron Compact claims it."
  created_epoch: <beacon_epoch>
  expiry: <beacon_epoch>               # Optional: intel goes stale
  visibility: private | faction | alliance | public
```

### Sharing Bookmarks

**Personal.** Stored on the player's client. Not shared. Not an Allgard object — just local data.

**Faction bookmarks.** Allgard objects owned by the faction, accessible through membership Grants. The faction hub maintains a bookmark collection. Members can read all faction bookmarks and add new ones (if their Grant includes write access). A 50-member faction with shared bookmarks has 50x the explored knowledge of a solo player.

**Temporary sharing.** A player can share a bookmark with another player by transferring a copy of the bookmark object. One-time bilateral sharing. The recipient gets the data; the sender keeps their copy.

**Published bookmarks.** A domain can publish bookmarks in its metadata — points of interest, navigation hazards, recommended routes. This is domain-provided intelligence, weighted by trust in the domain.

## Standing System

Factions need to know how to treat outsiders. Who's friendly, who's hostile, who's neutral. Standings are the convention.

### Standing Levels

```
standings:
  owner: <faction_id or domain_id>     # Who sets these standings
  entries:
    - subject: <owner_id or faction_id>
      level: allied | friendly | neutral | unfriendly | hostile
      set_epoch: <beacon_epoch>
      notes: "Trade partner since epoch 1200"
```

**Allied.** Full cooperation. Shared intelligence, mutual defense, fleet access. Usually between faction members or close partners.

**Friendly.** Preferred treatment. Better trade terms, access to restricted facilities, priority docking. Built through positive reputation history.

**Neutral.** Default. Standard trade terms, standard access, standard rules.

**Unfriendly.** Restricted access. Higher prices, reduced facility access, increased scrutiny. Result of negative reputation or political tension.

**Hostile.** Active opposition. Combat on sight, no trade, no docking (or docking as a trap). War footing.

### How Standings Work

Standings are domain/faction policy, not protocol. A domain that sets standing "hostile" toward a faction chooses what that means:

- Deny docking rights (refuse the player's session connection, or refuse to issue a visit Grant)
- Engage on sight (consent-on-entry Grant includes auto-combat)
- Raise prices (market prices adjusted based on buyer's faction affiliation)
- Share intelligence about the hostile entity with allies

The standing object is published in the domain's metadata. Visiting players can check standings before entering — "this domain is hostile to my faction" is visible information. Informed consent.

**Mutual standings.** Standings are unilateral by default. Faction A sets Faction B to hostile; Faction B might still have A at neutral. Bilateral agreement on standings (both sides agree to allied status) is a diplomatic act — a treaty. The treaty is a contract per [CONTRACTS.md](CONTRACTS.md), with standings as the terms.

## Faction Operations

Coordinated multi-domain activities. These compose from fleet mechanics, shared bookmarks, standings, and contracts.

### Trade Convoy

1. Faction posts convoy contract (escort + hauler coordination).
2. Haulers load cargo from faction trade agreements.
3. Escort fleet forms with commander.
4. Convoy fleet object coordinates movement and communication.
5. Escort protects haulers through route domains.
6. On arrival, cargo delivers to destination contracts. Payment flows.

### System Defense

1. Faction sets hostile standings against an aggressor.
2. Defense fleet forms at the threatened system.
3. Sensor ships (scout role) patrol approach routes, feeding data to fleet object.
4. Command channel pushes updates to all defenders.
5. On contact, fleet commander coordinates response through fleet orders.

### Research Expedition

1. Faction posts research joint venture contract.
2. Research ship (high-precision sensors, lab facility) plus escort fleet.
3. Fleet moves to target region (unclaimed or allied systems).
4. Research ship runs experiments; results shared through faction knowledge pool.
5. Fleet provides security and logistics (fuel resupply, cargo hauling for materials).

### Intelligence Gathering

1. Scout ships with high-quality sensors enter target domain.
2. Scouts observe: ship traffic, fleet compositions, market prices, domain defenses.
3. Scouts compile intel reports (see [KNOWLEDGE.md](KNOWLEDGE.md)).
4. Reports shared to faction bookmark/intelligence pool.
5. Faction command uses intel to plan operations.

## Shared Facilities

Faction members pooling infrastructure.

### Faction Hangar

A storage location on a faction-controlled domain. Objects owned by the faction (treasury, shared equipment, strategic reserves) stored here. Members access through Grants with varying permissions:

- **Deposit:** Any member can add objects. Standard Transfer to faction Owner.
- **Withdraw:** Restricted. Treasurer role (specific Grant) can withdraw. Prevents embezzlement.
- **View:** All members can observe hangar contents. Transparency within the faction.

### Shared Shipyard

A faction member's domain hosts a shipyard. Faction members get facility access through faction Grants (same as facility rental, but the Grant comes from faction membership, not per-use payment). Faction members build ships at member cost (below market rate). Outsiders pay full price.

### Communication Hub

The faction hub domain hosts persistent communication channels:

- **General channel.** All members. Observation object, GDL group-scoped events.
- **Officer channel.** Restricted Grant. Strategy discussion.
- **Trade channel.** Market prices, contract postings, arbitrage opportunities.
- **Intel channel.** Scout reports, threat alerts, sensor data.

These are just Leden observation objects with different Grant scopes. The convention is in the naming and usage patterns, not the protocol.

## Stage 1 Testing

Social tools are testable in the monolith with AI agents:

- **Fleet formation.** AI commander creates fleet, AI ships join. Verify Grant issuance, member list, formation tracking.
- **Fleet movement.** Fleet jumps between logical domains. Verify all members transfer, fleet object updates, stragglers are tracked.
- **Fleet combat.** Fleet enters combat. Verify commander's orders propagate to all ships, wing leads delegate correctly.
- **Shared bookmarks.** AI faction maintains bookmark collection. New member joins, verify they see existing bookmarks.
- **Standings.** Domain sets hostile standing toward an AI faction. Verify hostile ships are denied docking or engaged on sight.
- **Faction operations.** Run a full trade convoy: cargo pickup, escort formation, route transit, delivery. Verify all contracts, transfers, and fleet mechanics compose correctly.

## What This Spec Doesn't Cover

**Diplomacy protocol.** Formal treaties between factions (non-aggression pacts, trade agreements, mutual defense). These are contracts with standings as terms. The patterns are simple but the political dynamics are emergent. No spec can prescribe diplomacy.

**Alliance structure.** An alliance of factions — a higher-level group Owner that issues Grants to faction group Owners. Technically possible with current primitives. Not specced because it's premature — see if factions need alliances before designing them.

**Leaderboards / Rankings.** Public displays of faction wealth, territory, combat record. Domain-provided UI, not protocol. A trade hub that publishes faction rankings is providing entertainment, not infrastructure.

**Offline coordination.** When players aren't in-game, they coordinate on Discord, forums, wikis. External tools. Nothing Apeiron can or should control. The in-game tools should be good enough that players don't NEED external tools for basic coordination. But they'll use them anyway.
