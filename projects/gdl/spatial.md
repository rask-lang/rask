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
| `speed` | float | Scalar speed in region units/second. Redundant with `velocity` magnitude — use when direction comes from `heading`. |
| `heading` | float | Facing direction in degrees (0 = +x, 90 = +y). For 2D spatial models. |
| `angular_velocity` | float | Rotation speed in degrees/second. |
| `move_state` | string | Movement mode: `idle`, `walk`, `run`, `sprint`, `swim`, `fly`, `fall`, `climb`. |
| `move_target` | float list | Where the entity is moving toward. For pathfinding — client can interpolate along the projected path. |
| `grounded` | bool | Whether the entity is on a surface. Affects client-side gravity prediction. |

These compose with the existing `position` and `orientation` fields on entities.

### Dead Reckoning

A client that sees `position: [10, 5]` and `velocity: [2, 0]` on an entity can predict the entity's position between server updates. At tick_rate 20 (50ms between updates), this eliminates the stutter that comes from rendering position-only snapshots at 60fps.

The formula is trivial:

```
predicted_position = last_position + velocity * time_since_update
```

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

The domain partitions space around each observer into zones. Entities in closer zones get more frequent updates. Entities in farther zones get less.

The domain declares the zone configuration as a region property:

```
Region:
  name: "The Rusty Anchor"
  spatial: continuous_2d { bounds: [20, 15] }
  properties:
    tick_rate: 20
    spatial.zones:
      - { radius: 5,  rate: 20, label: "near" }
      - { radius: 15, rate: 5,  label: "mid" }
      - { radius: 40, rate: 1,  label: "far" }
```

Zone fields:

| Field | Type | Purpose |
|-------|------|---------|
| `radius` | float | Distance from observer in region units |
| `rate` | int | Update rate in Hz for entities in this zone |
| `label` | string | Human-readable name (for debugging, client UI) |

Zones are observer-centric circles. Every observer has the same zone radii, but a different set of entities in each zone (because observers are at different positions). The domain computes this per observer.

**Zone semantics:**

- Zones are ordered by radius. An entity falls into the smallest zone that contains it.
- Entities beyond the outermost zone follow the viewport rules — they enter/exit the observation stream as they cross the viewport boundary.
- The `rate` is a *maximum*. An entity that isn't moving doesn't generate updates regardless of zone.
- Zone config is per-region. Different regions can have different zones. A cramped tavern might have tight zones. An open field might have wide ones.

**What changes between zones:**

Only the *observation rate* for position and motion properties. Other entity data (affordance changes, health, status) continues at normal delta frequency — these are event-driven, not tick-driven. An entity in the "far" zone still instantly shows a health change or a new affordance. What drops to 1Hz is position streaming.

### Update Tiers

Within each zone, the domain applies update tiers — priority ordering for what gets sent when bandwidth is constrained.

| Tier | Data | Priority |
|------|------|----------|
| 1 | `position` | Always sent at zone rate |
| 2 | `velocity`, `heading`, `move_state` | Sent at zone rate, coalesced under backpressure |
| 3 | `orientation`, `angular_velocity` | Sent at zone rate, dropped under heavy backpressure |
| 4 | Other properties | Event-driven, normal observation |

Under normal conditions, all tiers flow. Under backpressure, the domain drops lower tiers first. A client getting only tier 1 can still render entities — they pop to new positions each update instead of interpolating smoothly. Degradation is graceful.

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

### Observer Feedback

The standard viewport mechanism (`client_viewport: { center, radius }`) tells the domain *where* the observer is looking. Spatial awareness adds one field:

```
client_viewport:
  center: [10, 7]
  radius: 25
  capacity: 100
```

`capacity` is the number of entities the observer can meaningfully track right now. It defaults to `max_entities` from fidelity, but can change dynamically — a client that's lagging reduces capacity to request fewer updates. The domain uses capacity to prioritize: if 200 entities are in the viewport but capacity is 100, the domain sends the 100 most relevant (nearest first, plus any the observer is interacting with).

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

Region: `continuous_2d`, tick_rate: 20, zones: near(5, 20Hz), mid(15, 5Hz).

Player A is at position [8, 6]. Their client declared `spatial_awareness: true`, `max_entities: 200`.

The domain computes for Player A:
- 4 players within 5 units → near zone, 20Hz position updates
- 38 players within 15 units → mid zone, 5Hz position updates
- 7 players beyond 15 units (near the door) → viewport edge, 1Hz

Outbound for Player A:
- Near: 4 × 20Hz = 80 position deltas/second
- Mid: 38 × 5Hz = 190 position deltas/second
- Far: 7 × 1Hz = 7 position deltas/second
- Total: 277 position deltas/second

Compare to naive: 49 × 20Hz = 980 deltas/second. The zone model cuts it to 28% of the naive rate.

Player A's client dead-reckons mid-zone entities between 5Hz updates using velocity. The visual result: nearby players move smoothly, distant players move almost as smoothly (4 frames of interpolation between updates at 60fps), and players near the door move in noticeable steps.

As Player A walks toward a group, those entities transition from mid to near zone. The domain fires `entity_nearby` events. The client loads high-detail models and shows nameplates. Update rate increases to 20Hz. Smooth transition.

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

- **Cross-domain spatial adjacency.** Two domains sharing a physical border (walk from one domain into another without a portal). This is an unsolved federation problem — it requires two domains to agree on a shared coordinate system at their boundary. The leased transfer model handles discrete transitions (portals). Seamless adjacency is a future problem.

## Relationship to Existing Specs

| Spec | Relationship |
|------|-------------|
| [GDL core](GDL.md) | Motion conventions extend the property registry. Spatial awareness builds on viewport, tick_rate, and observation flow. |
| [GDL extensions](GDL-extensions.md) | Movement input standardizes input stream types. Spatial awareness is a new extension alongside client scripts and spatial layers. |
| [Leden observation](../leden/observation.md) | Zone-based update rates are implemented through filtered observation and coalescing. No new observation primitives. |
| [Allgard presence](../allgard/PRESENCE.md) | Presence says "Owner is on Domain X." Spatial awareness says "Owner's entity is at position [8, 6] in the tavern." Different layers. |
| [Allgard transfer](../allgard/TRANSFER.md) | Region transitions that cross domains use leased transfer. Spatial protocol doesn't change the transfer mechanics. |

## Open Questions

**Zone shape.** Circles are simple but don't match rectangular viewports well. Should zones support rectangles or oriented boxes? I lean toward circles — they're rotationally invariant and match how humans perceive "nearby." The viewport is already a circle (center + radius). Matching shapes avoids a mismatch between "what I see" and "what updates I get."

**Zone renegotiation.** Can the client request different zone radii? Currently the domain declares zones per-region. A VR client with a narrow FOV might want tighter zones. A minimap client might want wider ones. Letting the client override zones adds complexity. Leaving it domain-only keeps things simple but less adaptive. I lean toward domain-only with the `capacity` field as the client's pressure valve.

**Observation multiplexing cost.** In the current model, the client observes the region (for enter/exit) and individual entities (for property changes). With 50 entities, that's 51 observations over one session. Leden multiplexes these over one connection, but the per-observation bookkeeping on both sides isn't free. Should the extension define a bulk spatial observation mode — "observe all entities in this region, position-only, domain handles filtering"? GDL already hints at this with `Observe(region_ref, entity_filter: [position])`. Might be sufficient.
