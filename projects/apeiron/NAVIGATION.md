# Navigation
<!-- id: apeiron.navigation --> <!-- status: proposed --> <!-- summary: Standard metadata for route planning — what domains publish, how clients compute routes -->

A galaxy of 10,000 stars is useless without a way to plan routes through it. The client can compute star positions from the seed. But which stars are claimed? What fuel costs? Which domains are hostile? Where are the trade hubs? This information lives on domains and propagates through the network.

This spec defines what navigation-relevant data domains publish and how clients use it to plan routes.

## The Data Model

Three layers of navigation data, from most universal to most local:

### Layer 1: Galaxy Topology (From Seed)

Computed locally by every client. Zero network cost.

- **Star positions.** 3D coordinates for all 10,000 stars.
- **Distances.** Euclidean distance between any pair of stars. The distance matrix is the galaxy's road network.
- **Nearest neighbors.** Which stars are within reasonable jump range. This defines connectivity — a ship with range R can reach stars within distance R.
- **Clusters and bridges.** Dense regions (many reachable neighbors), sparse regions (few neighbors), bridge stars (connecting clusters). Emergent from the position distribution.

Every client computes the same topology. The seed guarantees it.

### Layer 2: Domain Metadata (From Network)

Published by claimed domains through Leden peer metadata. Propagates through gossip. Partial — you only see data from domains you know about (through gossip reach or direct connection).

This is the **canonical peer metadata schema**. All domain-published information lives here — navigation, market, faction, standings. One metadata object per domain. Other specs ([MARKET.md](MARKET.md), [SOCIAL.md](SOCIAL.md), [FACTIONS.md](FACTIONS.md)) define the semantics for their sections. This spec defines the structure.

```
peer_metadata:
  # Identity
  star_id: 4822
  domain_type: system | station | outpost | route
  operator: <owner_id>

  # Navigation
  navigation:
    jump_cost_modifier: 1.0            # Multiplier on standard fuel formula
    docking_policy: open | restricted | faction_only | closed
    docking_fee: 50                    # Credits, 0 = free
    connects: [4800, 4850]             # Route domain only: which stars it links
    route_safety: safe | patrolled | contested | dangerous
    route_toll: 0                      # Credits per transit

  # Safety
  safety:
    combat_zone: false
    combat_script_hash: <hash>
    recent_combat_events: 3            # In last N epochs

  # Economy (see MARKET.md for semantics)
  market:
    has_market: true
    fuel_available: true
    fuel_price: 2.5
    has_shipyard: false
    has_refinery: true
    has_public_facilities: true
    commodities:                       # Summary only — full order book requires docking
      - type: "structural_steel"
        buy_price: 3.2
        sell_price: 3.5
        volume_24h: 5000
        supply: 12000
    order_count: 47
    updated_epoch: <beacon_epoch>

  # Faction (see FACTIONS.md for semantics)
  faction:
    name: "Iron Compact"
    faction_owner: <owner_id>
    territory_claim: "core-sector-7"   # Social convention, not enforcement

  # Standings (see SOCIAL.md for semantics)
  standings:
    hostile_to: [<faction_id>, ...]
    allied_with: [<faction_id>, ...]
```

One object, published via Leden peer metadata, gossiped through the network. Clients parse whichever sections they need — the route planner reads `navigation` and `safety`, the trade planner reads `market`, the diplomacy view reads `standings` and `faction`.

**All fields are self-reported.** The domain decides what to publish. A pirate haven might lie about combat events. A trade hub might advertise lower fuel prices than it actually charges. The data is informational, not contractual.

**Trust calibration.** Clients weight metadata by trust in the reporting domain per [REPUTATION.md](REPUTATION.md).

### Layer 3: Player Knowledge (Local)

Personal navigation data accumulated through play. Stored on the client. Not published unless the player chooses to share (via bookmarks or intel reports).

- **Visited domains.** Which stars you've actually been to. Verified data — you saw it firsthand.
- **Bookmarks.** Points of interest. See [SOCIAL.md](SOCIAL.md).
- **Price history.** Market prices at domains you've visited, timestamped. Valuable for arbitrage.
- **Route history.** Paths you've taken, fuel costs you actually paid, incidents encountered.
- **Threat assessment.** Personal notes on dangerous domains, hostile factions, pirate ambush points.

## Route Planning

The client computes routes from the three data layers. Standard algorithm: weighted shortest path (Dijkstra or A*) with player-defined priorities.

### Cost Function

Each edge (jump between two stars) has a cost. The client computes:

```
edge_cost(A, B) = w_fuel * fuel_cost(A, B, ship)
                + w_safety * danger_score(B)
                + w_time * jump_time(A, B)
                + w_cost * monetary_cost(B)           # Docking fees, fuel markup, tolls
```

Where:
- `fuel_cost` = standard formula × jump cost modifier × ship mass
- `danger_score` = f(combat_zone, recent_combat, hostile standings, player threat data)
- `jump_time` = distance-based (farther jumps take longer in the transfer protocol)
- `monetary_cost` = sum of docking fees, fuel purchase at markup, route tolls

The player sets weights. "Cheapest route" emphasizes fuel. "Safest route" emphasizes danger avoidance. "Fastest regardless of cost" minimizes time. A hauler with valuable cargo cranks up safety weight. A scout in a fast ship minimizes fuel weight.

### Route Types

**Direct route.** Jump star-to-star, no route domains. Cheapest (no tolls), fastest (fewer hops). But no gameplay between stars — just transfer protocol.

**Routed path.** Use route domains where they exist. More hops, possible tolls. But route domains offer: encounters (trade, combat), services (refueling stops), and protection (patrolled routes). The scenic route.

**Fuel-constrained route.** Ship can't make the jump in one hop. Route through intermediate systems where refueling is available. The routing algorithm must check fuel availability at each waypoint. Getting stranded because you planned a route through a system with no fuel is a real failure.

### Multi-Hop Planning

For routes spanning many jumps, the client needs to solve a constrained shortest path:

1. Start with galaxy topology (Layer 1).
2. Filter to stars within ship's maximum jump range.
3. Overlay domain metadata (Layer 2) for claimed systems.
4. Apply cost function to each edge.
5. Check fuel constraints: does the ship have enough fuel to reach the next refueling point?
6. Return: ordered list of waypoints, estimated fuel cost, estimated monetary cost, estimated danger level.

**Unknown gaps.** For stars with no domain metadata (unclaimed, or out of gossip range), the client uses defaults: no services, unknown danger, standard jump cost. The route planner should flag these: "no data for systems 4800-4815 on this route."

## Fuel Planning

Fuel is the hard constraint. Running out of fuel means stranding. The navigation system must make fuel state visible and planning reliable.

### Fuel Display

The client always shows:
- **Current fuel.** How much fuel the ship has right now.
- **Range ring.** On the galaxy map, a circle (sphere in 3D) showing how far the ship can travel with current fuel. Stars outside the ring are unreachable without refueling.
- **Route fuel budget.** For a planned route: fuel at each waypoint, total fuel consumed, fuel remaining at destination. Warnings if any leg exceeds capacity or if a refueling stop has uncertain fuel availability.

### Fuel Stops

The route planner knows which domains sell fuel (from metadata). It routes through fuel stops when necessary:

```
Planned route: A → B → C → D → E
Fuel at A: 500 units
Fuel to B: 120
Fuel to C: 180  (cumulative: 300, remaining: 200)
Fuel to D: 250  ← EXCEEDS REMAINING
→ Refuel at C: buy 300 units (price: 2.5 credits/unit = 750 credits)
Fuel to D: 250  (remaining: 250)
Fuel to E: 100  (remaining: 150)
```

If C doesn't sell fuel, the planner tries alternative routes. If no route with adequate fuel stops exists, the planner says so: "destination unreachable with current fuel capacity."

## Hazard Warnings

The navigation system surfaces known hazards:

**Active combat zones.** Domains with `combat_zone: true` and recent combat events. Highlighted on the galaxy map. Route planner avoids by default (overridable).

**Hostile territories.** Domains whose standings include the player's faction in `hostile_to`. Color-coded on the map. Route planner routes around unless the player explicitly permits hostile space.

**Unknown space.** Stars with no domain metadata. Marked differently from claimed/safe and claimed/hostile. Unknown is not dangerous — it's uncertain. But fuel stops are absent, and rescue options are limited.

**Player warnings.** Bookmarks with category "danger" from the player's personal or faction data. Threats the player or their faction has identified but that the domain won't advertise (pirate ambush points, domains that have cheated, deceptive market prices).

## Domain-Provided Navigation Aids

Domains can offer navigation services beyond static metadata:

**Fuel price feeds.** A domain publishes real-time fuel prices through Leden observation. Clients subscribe and get live updates. A trade hub that aggregates fuel prices from nearby systems provides real value to haulers planning routes.

**Route advisories.** A faction or domain publishes route safety assessments: "Route between systems 4800 and 4900 is currently patrolled by the Iron Compact. Safe for allied traffic." Updated regularly. Shared through faction channels or domain metadata.

**Traffic reports.** A domain publishes recent traffic data: which systems have heavy hauler traffic (good for piracy, bad for pirates), which route domains are congested, where combat is happening. Useful for traders (avoid congested markets, find underserved ones) and for combat players (find fights or avoid them).

These are knowledge products per [KNOWLEDGE.md](KNOWLEDGE.md). Domains provide them because they attract traffic. A trade hub with the best navigation data in the sector gets more visitors.

## Stage 1 Testing

Route planning is core to the Stage 1 experience (text-based trading game):

- **Galaxy map.** Client computes and displays all 10,000 stars. Claimed systems (founding 5) are highlighted with metadata.
- **Route planner.** Player selects destination. Client computes cheapest/safest/fastest route through the founding cluster. Verify fuel constraints are checked.
- **Fuel display.** Range ring visible on map. Fuel budget shown for planned routes. Warnings for insufficient fuel.
- **Hazard display.** If one founding system is set to PvP-enabled, verify it's highlighted on the route planner.
- **Multi-hop routes.** Plan a route that requires refueling at an intermediate system. Verify the planner includes the fuel stop.
- **Dynamic updates.** AI domain changes fuel price. Verify client's navigation data updates (through observation or metadata poll).
- **Unknown space.** Plan a route through unclaimed systems. Verify the planner flags "no data" and warns about no fuel availability.

## What This Spec Doesn't Cover

**Jump animation.** What the player sees during transit. Client presentation. The void between stars with no route domain is cosmetic — a skybox, not gameplay (per [README.md](README.md#the-void)).

**Autopilot.** Automatic execution of a planned route — jumping, docking, refueling, continuing. Convenient but dangerous (what if a pirate intercepts?). Client feature, not protocol. Whether the founding cluster client ships with autopilot is a game design choice.

**Real-time positioning.** Where exactly a ship is within a domain's space. That's GDL spatial protocol, not navigation. Navigation gets you to the domain. GDL handles position within it.

**Map visualization.** How the galaxy map looks — 2D projection, 3D rotatable, color schemes, zoom levels. Client UI, not spec. But the founding cluster should publish a reference implementation that's at least functional for Stage 1.
