<!-- id: gdl.spatial -->
<!-- status: proposed -->
<!-- summary: Conventions and extension for real-time spatial presence in gards -->

# Spatial Protocol

How 50 people in the same room see each other move in real-time.

GDL defines regions, entities, and positions. Leden handles observation, deltas, and backpressure. Allgard handles federation and presence across domains. None of them answer the operational question: when a tavern has 50 players moving simultaneously, what does the domain send, to whom, at what rate?

Without this spec, every spatial gard reinvents interest management, movement prediction, and update prioritization. Clients built for one domain's movement model break on another's. The federation works at the object level (swords transfer) but fails at the experience level (movement feels different everywhere).

## Scope

This spec has two parts:

1. **Motion conventions** — standard property names for entity movement. Any domain can use them. Clients that recognize them get smooth interpolation. Clients that don't see position jumps. No negotiation, no new protocol machinery.

2. **Spatial awareness extension** — a fidelity-negotiated capability for interest management. The domain adapts what it sends based on observer distance and relevance. The client declares it can handle variable update rates per entity.

A turn-based hex game can use motion conventions on a thrown projectile without opting into the extension. An MMO tavern opts into the extension to make 50 simultaneous players feasible.

## Part 1: Motion Conventions

Standard property names for entity movement. These are additions to the [property convention registry](GDL.md#initial-convention-registry) — same status, same rules. Domains use them. Clients that recognize them dead-reckon between updates. Clients that don't skip them.

### Motion Properties

| Property | Type | Meaning |
|----------|------|---------|
| `velocity` | float list | Movement vector in region units/second. `[vx, vy]` for 2D, `[vx, vy, vz]` for 3D. |
| `acceleration` | float list | Acceleration vector in region units/second². Same dimensionality as velocity. |
| `grounded` | bool | Whether the entity is on a surface. When true, client ignores vertical acceleration for prediction. |
| `move_target` | float list | Where the entity is moving toward. Client can interpolate along the projected path instead of straight-line extrapolation. |

Four properties. Three for physics prediction, one for pathfinding.

**`velocity`** is the essential one. It enables linear dead reckoning — predicting position between server updates. Without it, clients render position snapshots at tick_rate, which stutters at any frame rate above the tick rate.

**`acceleration`** gives parabolic prediction — jumping, falling, thrown objects, any non-linear motion. `predicted = position + velocity * t + 0.5 * acceleration * t²`. This covers most natural movement. A client that doesn't understand acceleration falls back to linear extrapolation from velocity alone.

**`grounded`** distinguishes "walking on a slope" from "falling." A grounded entity doesn't need vertical acceleration prediction — the client can assume it stays on the surface. An airborne entity does. Without this, the client can't dead-reckon a walking character correctly in a 3D region with gravity.

**`move_target`** is optional. A domain with pathfinding includes the destination so clients draw smoother curves instead of straight-line extrapolation. A domain without pathfinding skips it.

These are the mechanical minimum for client-side prediction without extensions. A domain that wants richer physics (friction, drag, collision) uses the [physics parameters extension](GDL-extensions.md#physics-parameters) or ships a [prediction script](GDL-extensions.md#client-scripts). The conventions cover the 90% case — entities that move in straight lines or parabolic arcs.

**What's intentionally excluded:** `speed` (velocity magnitude — redundant), `heading` (use the existing `orientation` field on entities), `angular_velocity` (niche), `move_state` (this is animation state, already covered by GDL's [animation vocabulary](GDL.md#animation-state)). Domains can use any of these as regular properties — the extensible property system handles them. They don't need protocol-level convention status.

### Dead Reckoning

A client that sees `position: [10, 5]` and `velocity: [2, 0]` on an entity can predict the entity's position between server updates. At tick_rate 20 (50ms between updates), this eliminates the stutter that comes from rendering position-only snapshots at 60fps.

```
// Linear (velocity only)
predicted = position + velocity * t

// Parabolic (velocity + acceleration)
predicted = position + velocity * t + 0.5 * acceleration * t²
```

The client uses whichever properties are present. Position only → no prediction, snap between updates. Velocity → linear. Velocity + acceleration → parabolic. Each level up produces smoother interpolation between server updates.

For grounded entities in 3D regions, the client zeroes vertical acceleration — the entity follows the surface. This prevents grounded characters from sinking through floors or floating during prediction.

When an authoritative position update arrives, the client has three choices:

1. **Snap.** Set position to authoritative value. Simple, visually jarring.
2. **Blend.** Interpolate from predicted to authoritative over a short window (50-150ms). Smooth, standard approach.
3. **Ignore small corrections.** If the difference is below a threshold (e.g., 0.1 region units), keep the predicted position. Reduces micro-jitter.

The choice is the client's. The domain sends truth. The client makes it feel good. This is the same principle as GDL's existing [client-side prediction](GDL.md#client-side-prediction).

### Movement Input

The [input streams extension](GDL-extensions.md#input-streams) already defines how continuous client→server data works. This spec standardizes the movement-specific stream:

```
input_streams:
  - id: movement
    type: movement_2d    # [dx, dy, speed]
    rate: 20
```

Movement input types:

| Type | Data | Use |
|------|------|-----|
| `movement_2d` | `[dx, dy, speed]` — direction vector + speed scalar | Top-down, side-view |
| `movement_3d` | `[dx, dy, dz, speed]` — direction + speed | 3D worlds |
| `position_2d` | `[x, y]` — absolute position | Click-to-move, touch |
| `position_3d` | `[x, y, z]` — absolute position | Click-to-move in 3D |

The direction types (`movement_2d`, `movement_3d`) carry intent: "I'm pressing left at walk speed." The domain decides the authoritative position. The position types carry desired destination: "I clicked here." The domain pathfinds and validates.

Direction input is preferred for continuous movement. Position input is for discrete click-to-move. Both are valid — the domain declares which it accepts.

For clients that don't support input streams (text clients, simple 2D clients), movement affordances remain the fallback:

```
Affordance:
  verb: "move"
  label: "Go north"
  category: navigate
  mode: instant
  predicted: true
  method: <leden_method_ref>
```

Input streams and movement affordances coexist. The domain provides both. The client uses what it supports.

### When Conventions Alone Suffice

A gard with fewer than ~20 moving entities in a client's viewport doesn't need the spatial awareness extension. The numbers:

- 20 entities × position update at 20Hz = 400 deltas/second
- Each delta is ~40 bytes (entity ref + position + velocity)
- 16 KB/second total

Leden handles this without breaking a sweat. Backpressure, coalescing, and filtered observation cover the rest. The motion conventions give clients what they need for smooth rendering.

The extension becomes necessary when the entity count or update rate makes "send everything to everyone" untenable.

## Part 2: Spatial Awareness Extension

Fidelity-negotiated. The client declares support:

```
client_fidelity:
  spatial_awareness: true
```

When both sides support it, the domain gains the ability to vary update rates per entity per observer based on spatial relevance. The client knows to expect this and handles entities appearing at different update frequencies.

### The Problem at Scale

A tavern with 50 players. Each player needs to see the others. At 20Hz tick rate:

- 50 entities × 49 observers × 20 updates/second = 49,000 deltas/second outbound

That's just position. Add properties, affordance changes, effects — it multiplies. And this is a *small* room. A city district with 500 players is 100x worse.

The viewport mechanism from [GDL fidelity](GDL.md#fidelity-negotiation) helps — you only see entities in your viewport. But in a tavern, everyone IS in your viewport. The viewport doesn't help when the problem is density, not extent.

### Relevance Zones

The space around each observer is partitioned into zones. Entities in closer zones get more frequent updates. Entities in farther zones get less. The question is who decides the zone boundaries.

Both sides have information the other doesn't. The domain knows its spatial density, server capacity, and visibility rules (fog of war, walls, stealth). The client knows its camera — FOV, zoom level, screen size, rendering budget. A VR headset at 110 degrees sees different spatial density than a minimap zoomed out to the whole region.

So it's negotiated.

**Step 1: Domain declares constraints.**

The region advertises what it can provide — maximum rates and minimum radii. This is part of the region snapshot:

```
Region:
  name: "The Rusty Anchor"
  spatial: continuous_2d { bounds: [20, 15] }
  properties:
    tick_rate: 20
    spatial.constraints:
      max_zones: 3
      max_rate: 20
      min_radius: 2.0
      visibility: circle
    spatial.defaults:
      - { radius: 5,  rate: 20 }
      - { radius: 15, rate: 5 }
      - { radius: 40, rate: 1 }
```

Constraint fields:

| Field | Type | Purpose |
|-------|------|---------|
| `max_zones` | int | Maximum number of zones the domain will track per observer |
| `max_rate` | int | Highest update rate the domain provides (Hz) |
| `min_radius` | float | Smallest allowed zone radius (region units). Prevents "give me 0.1 radius at 60Hz" abuse. |
| `visibility` | string | How visibility is computed (see below) |

Visibility modes:

| Mode | Meaning |
|------|---------|
| `circle` | Distance only. Entity within zone radius = visible. Simple, predictable, cheap. |
| `line_of_sight` | Walls and obstacles block visibility. An entity 3 units away but behind a wall gets no updates. The domain computes occlusion server-side. |
| `domain_controlled` | The domain applies arbitrary visibility rules — fog of war, stealth, instancing, phase shifts. The client can't predict what it'll receive. |

`spatial.defaults` are the zone configuration used for clients that don't send their own. Most clients accept these. The domain picks defaults appropriate for its typical spatial density.

**Step 2: Client requests zones.**

Zone configuration and viewport position are separate messages. The viewport center changes constantly (every time the player moves). Zone config changes rarely — when the client zooms, switches rendering mode, or enters a dense area. Bundling them wastes parsing on every position update.

```
// Sent once at session start, updated when rendering mode changes
spatial_zones:
  region: <region_ref>
  capacity: 100
  zones:
    - { radius: 3,  rate: 20, label: "near" }
    - { radius: 12, rate: 5,  label: "mid" }
    - { radius: 25, rate: 1,  label: "far" }

// Sent at movement rate, unchanged from existing GDL
client_viewport:
  center: [10, 7]
  radius: 25
```

A VR client requests tight zones — it renders close-up detail at high fidelity, distant entities as silhouettes. A minimap client requests wide zones at low rates — it renders dots and doesn't need 20Hz position for anyone. A mobile client with limited bandwidth requests fewer zones with lower rates.

The outermost zone radius should match the viewport radius. Zones beyond the viewport are nonsensical — you can't receive updates for entities you're not observing. If the viewport radius changes (zoom), the client sends an updated zone config.

`capacity` is the number of entities the observer can meaningfully track. Defaults to `max_entities` from fidelity. The domain uses it to prioritize: if 200 entities are in the viewport but capacity is 100, the domain sends the 100 most relevant (nearest first, plus any the observer is interacting with). Zones control *rate*. Capacity controls *count*.

If the client declares `spatial_awareness: true` but never sends `spatial_zones`, the domain uses its default zone configuration from `spatial.defaults` (see constraints above). This is the common case — most clients accept the domain's defaults.

**Step 3: Domain applies.**

The domain clamps the client's request to its constraints (max_rate, min_radius, max_zones) and applies server load adjustments. If the server is overloaded, it can reduce rates below what the client requested — the client handles whatever rates it receives.

No confirmation message. The client adapts to actual delivery rates, same as Leden backpressure — the server sends less, the client notices. Adding a confirmation round-trip would mean the client waits for the domain to acknowledge before adapting, which is worse than measuring actual rates.

**Zone semantics:**

- Zones are ordered by radius. An entity falls into the smallest zone that contains it.
- Entities beyond the outermost zone follow the viewport rules — they enter/exit the observation stream as they cross the viewport boundary.
- The `rate` is a *maximum*. An entity that isn't moving doesn't generate updates regardless of zone.
- Zone config is per-region. The client sends different configs for different regions if it has multiple regions observed simultaneously (portal preview, minimap of a different area).
- Visibility applies before zones. With `visibility: line_of_sight`, an entity 3 units away but behind a wall gets no position updates — it's not visible, regardless of zone. With `visibility: circle`, distance is the only factor. With `visibility: domain_controlled`, the domain applies arbitrary visibility rules (fog of war, stealth, instancing) and the client can't predict what it'll receive.

**What changes between zones:**

Only the *observation rate* for position and motion properties. Other entity data (affordance changes, health, status) continues at normal delta frequency — these are event-driven, not tick-driven. An entity in the "far" zone still instantly shows a health change or a new affordance. What drops to 1Hz is position streaming.

### Update Tiers

Within each zone, the domain applies update tiers — priority ordering for what gets sent when bandwidth is constrained.

| Tier | Data | Priority |
|------|------|----------|
| 1 | `position` | Always sent at zone rate |
| 2 | `velocity`, `acceleration`, `grounded` | Sent at zone rate, coalesced under backpressure |
| 3 | `orientation` | Sent at zone rate, dropped under heavy backpressure |
| 4 | Other properties | Event-driven, normal observation |

Under normal conditions, all tiers flow. Under backpressure, the domain drops lower tiers first. A client getting only tier 1 can still render entities — they pop to new positions each update instead of interpolating smoothly. Tier 1 + 2 gives smooth dead reckoning. Tier 1 + 2 + 3 adds facing direction. Degradation is graceful at each level.

The tier structure is a domain implementation concern — the spec defines the priority order, but the domain decides when to shed tiers. The client doesn't negotiate tiers. It receives what the domain sends and renders accordingly.

### Spatial Events

The extension formalizes two events that the core spec leaves implicit:

**`entity_nearby`** — An entity crossed into the observer's near zone. Different from `entity_enter` (which fires at the viewport boundary). Nearby is semantically closer — "this entity is now close enough to matter."

```
Event:
  type: "entity_nearby"
  source: <entity_ref>
  data:
    zone: "near"
    distance: 4.2
```

**`entity_distant`** — An entity crossed out of the near zone into a farther zone.

```
Event:
  type: "entity_distant"
  source: <entity_ref>
  data:
    zone: "mid"
    distance: 16.1
```

These are rendering hints. A client might:
- Show nameplates only for nearby entities
- Load high-detail models for nearby, low-detail for distant
- Enable spatial audio falloff based on zone
- Show interaction prompts only for nearby entities

The domain fires these events. The client uses them however it wants, or ignores them.

### How Observation Works

The recommended observation pattern for spatial awareness uses region-level filtered observation, not per-entity subscriptions:

```
Observe(region_ref, entity_filter: [position, velocity, acceleration, grounded])
```

This gives position and motion updates for all entities in the region as a single subscription. The domain applies zone-based throttling server-side — the client doesn't need one observation per entity. Combined with the region observation for structural changes (entity_enter/exit), this is two observations total regardless of entity count.

Per-entity observations are still available for detailed tracking (a targeted entity's full property set, for example). But bulk position streaming should go through the region-level filter to avoid multiplexing overhead.

### Interaction Override

An entity the observer is directly interacting with (targeting, trading with, in combat with, observing individually) always gets near-zone update rates, regardless of actual distance. The domain tracks interaction state — when an observer calls an affordance on an entity, that entity gets promoted to near-zone priority for that observer until the interaction ends.

This doesn't need protocol support. It's a domain implementation convention: interacted entities override zone-based priority. I'm documenting it because every domain that implements spatial awareness will need this rule, and getting it wrong produces the jarring experience of your trade partner's avatar stuttering at 1Hz because they walked 20 meters away mid-trade.

### Without the Extension

A client that doesn't declare `spatial_awareness: true` gets the existing behavior:

- All entities in the viewport at the region's tick_rate
- Standard observation backpressure (coalesce under load)
- Entity enter/exit at viewport boundary

This works for small entity counts. For large counts, the domain's options are limited — it can only use the existing `max_entities` fidelity field to cap how many entities the client receives, dropping the rest. Without spatial awareness, the domain can't do distance-based prioritization because the client hasn't declared it understands variable update rates.

## Worked Example: 50 Players in a Tavern

Region: `continuous_2d`, tick_rate: 20, constraints: max_rate 20, min_radius 2.0, defaults: near(5, 20Hz), mid(15, 5Hz), far(40, 1Hz).

Player A is at position [8, 6]. Their client declared `spatial_awareness: true`, `max_entities: 200`, and sent a zone config: near(5, 20Hz), mid(15, 5Hz), far(25, 1Hz).

The domain clamps (all within constraints) and computes for Player A:
- 4 players within 5 units → near zone, 20Hz position updates
- 38 players within 15 units → mid zone, 5Hz position updates
- 7 players beyond 15 units (near the door) → far zone, 1Hz

Outbound for Player A:
- Near: 4 × 20Hz = 80 position deltas/second
- Mid: 38 × 5Hz = 190 position deltas/second
- Far: 7 × 1Hz = 7 position deltas/second
- Total: 277 position deltas/second

Compare to naive: 49 × 20Hz = 980 deltas/second. The zone model cuts it to 28% of the naive rate.

Player A's client dead-reckons mid-zone entities between 5Hz updates using velocity. The visual result: nearby players move smoothly, distant players move almost as smoothly (4 frames of interpolation between updates at 60fps), and players near the door move in noticeable steps.

As Player A walks toward a group, those entities transition from mid to near zone. The domain fires `entity_nearby` events. The client loads high-detail models and shows nameplates. Update rate increases to 20Hz. Smooth transition.

Meanwhile, Player B is on a mobile client. They requested zones: near(3, 10Hz), mid(10, 2Hz), capacity 30. Same tavern, same 50 players, but Player B receives fewer updates for fewer entities. The domain sends the 30 nearest players and drops the rest from the observation stream. Player B's client renders a simpler scene — it asked for what it can handle.

## Worked Example: Region Transition

Player A walks toward a portal in the tavern. The portal is an entity with kind `portal` and a reference to the destination region.

1. Player A enters the portal's proximity (affordance with `mode: proximity` fires automatically or client prompts).
2. Client calls the portal's method. Domain validates.
3. Domain returns the destination region reference.
4. Client calls `Observe(destination_region_ref)`. Gets a snapshot.
5. Client calls `Unobserve(tavern_region_ref)`.

Steps 3-4 compose with promise pipelining — one round trip. The client can begin rendering the destination region while fading out the tavern. The domain handles the entity bookkeeping: Player A's entity gets `entity_exit` in the tavern, `entity_enter` in the destination.

If both regions are on the same domain, this is a local operation. If the destination is on a different domain, this is a [leased transfer](../allgard/TRANSFER.md) — Allgard handles the ownership mechanics, GDL handles what the client sees. The spatial protocol doesn't add anything here — region transitions are already covered.

## What This Doesn't Cover

- **Server-side spatial indexing.** How the domain efficiently computes "which entities are within 5 units of this observer" is an implementation concern. Spatial hashing, quadtrees, sweep-and-prune — the domain picks what works. The spec says what to send, not how to compute it.

- **Physics simulation.** Collision detection, rigid body dynamics, projectile trajectories. These are domain logic (Raido scripts or native code). Motion conventions carry the *result* of physics, not the simulation itself. The [physics parameters extension](GDL-extensions.md#physics-parameters) covers client-side simulation hints.

- **Anti-cheat for movement.** Validating that a player's input stream isn't teleporting them across the map is domain validation logic. The domain is authoritative — it processes movement input and publishes the validated position. Input streams don't bypass domain authority.

- **Pathfinding.** The `move_target` property tells the client where an entity is heading. How the domain computed the path is not the client's concern.

## Relationship to Existing Specs

| Spec | Relationship |
|------|-------------|
| [GDL core](GDL.md) | Motion conventions extend the property registry. Spatial awareness builds on viewport, tick_rate, and observation flow. |
| [GDL extensions](GDL-extensions.md) | Movement input standardizes input stream types. Spatial awareness is a new extension alongside client scripts and spatial layers. |
| [Leden observation](../leden/observation.md) | Zone-based update rates are implemented through filtered observation and coalescing. No new observation primitives. |
| [Allgard presence](../allgard/PRESENCE.md) | Presence says "Owner is on Domain X." Spatial awareness says "Owner's entity is at position [8, 6] in the tavern." Different layers. |
| [Allgard transfer](../allgard/TRANSFER.md) | Region transitions that cross domains use leased transfer. Spatial protocol doesn't change the transfer mechanics. |

## Resolved

**Zone shape.** Circles. They're rotationally invariant, match human perception of "nearby," and match the existing viewport shape (center + radius). A client with a wide rectangular display circumscribes it with a circle. The wasted updates at corners are marginal compared to the complexity of supporting rectangles, oriented boxes, and frustums.

**Domain pushback.** No explicit confirmation. The client adapts to actual delivery rates, same as Leden backpressure. Measuring actual update frequency is trivial — count updates per second per zone. Adding a confirmation round-trip would mean the client waits for acknowledgment before adapting, which is worse.

**Observation multiplexing.** Region-level filtered observation (`Observe(region_ref, entity_filter: [position, velocity, ...])`) handles bulk position streaming. The domain applies zone-based throttling server-side within this single observation. Per-entity observations reserved for detailed tracking of specific entities. See [How Observation Works](#how-observation-works).

## Open Questions

**Cross-domain spatial adjacency.** Two domains sharing a physical border — walking seamlessly from one domain into another without a portal. This requires two domains to agree on a shared coordinate system at their boundary, shared visibility across the border, and synchronized entity handoff. The leased transfer model handles discrete transitions (portals). Continuous borders are a harder problem that this spec doesn't attempt. Worth solving eventually — it would enable open-world federation where domains tile together geographically.
