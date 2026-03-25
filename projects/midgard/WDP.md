World Description Protocol
<!-- id: midgard.wdp --> <!-- status: proposed --> <!-- summary: Content schema for describing virtual world state over Leden -->

WDP is Midgard's "HTML" — the content schema that tells a client what the world looks like and how to interact with it. Domains send structured descriptions. Clients decide how to render them. A text client shows room descriptions. A 2D client draws tiles. A 3D client builds a scene. Same data, different presentations.

WDP is not a protocol. It's a content schema that rides on Leden, the same way HTML rides on HTTP. Leden handles sessions, capabilities, observation, and content delivery. WDP defines what's inside the payloads.
Why This Exists

The Midgard stack has logic (Raido), trust (Allgard), and transport (Leden). It doesn't have a way to describe what the world looks like.

Without WDP, every domain invents its own scene format. Clients need per-domain rendering code. Cross-domain travel becomes "load a completely different game." The federation model works at the object level (swords transfer) but fails at the experience level (the player sees nothing coherent).

WDP is the common language between domain and client. A domain describes its world in WDP. A client that speaks WDP can render any domain, the same way a browser that speaks HTML can render any website.
Design Principles

These come from studying what worked and what didn't across 30 years of world description formats.
1. Ignore What You Don't Understand

The single most important rule. If a client receives a property it doesn't recognize, it skips it. If a domain sends an entity kind the client hasn't seen before, the client falls back to the kind's category. Unknown fields are preserved, never rejected.

This is how HTML survived 30 years of evolution. Browsers that don't know <video> render the fallback content inside the tags. WDP clients that don't know a "flamebrand" render it as a generic item. Forward compatibility is non-negotiable.
2. Description Is Not Behavior

WDP describes what exists — structure, properties, appearance. Raido scripts define what happens — combat, crafting, physics. VRML tried to combine scene description with behavioral wiring and paid for it with implementation complexity that killed adoption. X3D inherited the same mistake.

The boundary is sharp: WDP says "there's a goblin here with 30 health." Raido says "when attacked, subtract damage from health." The client renders the description. The domain runs the behavior. They don't mix.
3. Description Is Not Rendering

WDP carries semantic descriptions, not rendering instructions. "A weathered oak door with iron bands" — not vertex buffers, not sprite coordinates, not CSS. The client decides how to present it. A text client prints the description. A 3D client assembles a door from its built-in asset library. A client with GenAI generates one on the fly.

Second Life got this right with parametric prims — 50 bytes of shape parameters instead of megabytes of mesh data. glTF got it right for transmission (GPU-ready buffers). WDP sits one level higher: semantic description that any renderer can interpret.
4. Progressive Enhancement, Not Multiple Representations

The domain sends one description. Richer clients extract more from it. A text client uses name and description. A 2D client adds position and appearance.shape. A 3D client adds appearance.assets for custom models. Same payload, different extraction depths.

This is HTML's model. One document, many renderers. Not "here's the text version, here's the 2D version, here's the 3D version." That doesn't scale — domains would need to author three descriptions for every entity.
5. Optimize for the Consumer

glTF's tagline is "the JPEG of 3D." It succeeded by optimizing for the renderer, not the authoring tool. WDP optimizes for the client, not the domain. Descriptions are structured for fast parsing, progressive extraction, and incremental updates. The domain does extra work once; thousands of clients benefit.
6. The Simpler System Wins

HTTP beat CORBA. JSON beat XML. HTML beat SGML. In every case, the simpler format won because more people could implement it correctly. A WDP client should be buildable in a weekend. The minimum viable client is a text renderer that prints names, descriptions, and affordance menus. Everything beyond that is progressive enhancement.
Core Model

Four concepts: regions, entities, affordances, and appearance.
Regions

A region is a spatial container. It's the "page" — the top-level context that a client loads and renders. A dungeon room, a forest clearing, a city block, a spaceship interior. Regions connect to other regions through portals.

A region is a Leden object. Observing it gives you WDP content — a snapshot of the region's current state, followed by a delta stream of changes.

Region:
  name: "The Rusty Anchor"
  description: "A cramped tavern. Smoke hangs low. The bar runs the
    length of the far wall, bottles glinting in candlelight. A staircase
    in the corner leads up."
  spatial: grid_2d { width: 12, height: 8 }
  ambient:
    lighting: dim
    sound: tavern_murmur
    temperature: warm
  entities: [...]

Region fields:
Field	Required	Purpose
name	Yes	Display name
description	Yes	Text description — the universal fallback
spatial	No	How positions work (see below)
ambient	No	Environmental properties
entities	Yes	Things in the region

Spatial models:
Model	Positions	Use case
continuous_3d { bounds }	[x, y, z] floats	Full 3D worlds
continuous_2d { bounds }	[x, y] floats	Top-down or side-view
grid_2d { width, height }	[col, row] integers	Tile-based games
graph	Named locations	Text adventures, node-based maps
abstract	None	Inventories, conversations, menus

No spatial model means abstract — entities exist but have no spatial relationship. A text client can always render any spatial model as a list of entities with their descriptions.

The domain picks the spatial model. The client renders what it can. A text client ignores continuous_3d coordinates and lists entities. A 2D client projects continuous_3d down to a top view. The spatial model is a hint about the domain's intended presentation, not a rendering requirement.
Entities

An entity is a thing in a region. A goblin, a sword, a door, a campfire, a portal to another region. Entities have a kind, properties, affordances, and appearance.

Entity:
  ref: <leden_object_ref>
  kind: creature
  name: "Goblin Scout"
  description: "A small, wiry goblin crouches behind a rock, watching
    you with sharp yellow eyes."
  position: [12, 3]
  properties:
    health: 30
    hostile: true
    level: 5
  affordances: [...]
  appearance: {...}

Entity fields:
Field	Required	Purpose
ref	Yes	Leden object reference (identity + capability)
kind	Yes	Category from the vocabulary
name	Yes	Display name
description	Yes	Text description — always the fallback
position	No	Location within the region's spatial model
properties	No	Key-value extensible data
affordances	No	What you can do with/to this entity
appearance	No	Rendering hints and asset references

Entity kinds (the initial vocabulary — extensible):
Kind	Meaning	Fallback rendering
creature	Living entity — player, NPC, monster, animal	Name + description
item	Portable object — sword, potion, tool, material	Name + "you could pick this up"
structure	Fixed construction — wall, door, chest, table, building	Name + part of environment
terrain	Ground/environment — grass, water, stone, lava	Background/floor
portal	Navigation point — door, gate, path, teleporter	Directional prompt
effect	Transient phenomenon — fire, mist, spell, sound	Flavor text
marker	Abstract point — spawn, waypoint, boundary	Hidden or minimal
container	Holds other entities — chest, bag, shelf	Name + "contains things"
vehicle	Rideable/enterable transport	Name + description

Kinds work like HTML elements — a small fixed set that clients know how to render by default. Unknown kinds fall back to their closest match (a client that doesn't know vehicle treats it as structure). This is HTTP's status code class trick: the first part tells you the category even if you don't know the specific value.

Domains don't need new kinds for domain-specific entities. A "Flamebrand of the Seventh Circle" is kind=item. The name and description carry the specifics. Kind tells the client the category of thing; everything else is in the description and properties.
Properties

Key-value pairs on entities. Properties are the extensible data channel — anything a domain wants to communicate about an entity that doesn't fit the fixed fields.

Properties have typed values:
Type	Example
int	health: 30, level: 5
float	weight: 2.5, speed: 1.3
bool	hostile: true, locked: false
string	faction: "Iron Guard", mood: "suspicious"
ref	owner: <object_ref>, wielding: <object_ref>
list	tags: ["undead", "boss"]

Unknown property keys are ignored by clients that don't recognize them. This is the forward-compatibility mechanism. A domain can add enchantment_level: 7 and old clients just skip it.

Well-known properties (not required, but clients may render them specially):
Property	On	Meaning
health	creature	Current hit points
health_max	creature	Maximum hit points
level	creature	Power/experience level
hostile	creature	Whether this entity is hostile to the observer
locked	structure, container	Whether this requires a key/action to open
weight	item	How heavy the item is
quantity	item	Stack count
price	item	Trade value
owner_name	any	Display name of the owner

Well-known properties let clients build smart UIs without domain-specific code. A client sees health and health_max on a creature and renders a health bar. No domain-specific plugin needed.
Affordances

Affordances are what make WDP interactive. They answer: "what can I do here?"

This is HATEOAS applied to virtual worlds. In REST, the server response tells you what actions are available via links and forms. In WDP, the entity description tells you what actions are available via affordances. The client doesn't need compiled-in knowledge of what's possible — the domain tells it, per entity, right now.

LambdaMOO discovered this in 1990: verbs live on objects and are discovered at runtime. "Take ball" works because the ball has a take verb. WDP formalizes the same pattern.

Affordance:
  verb: "attack"
  label: "Attack"
  category: combat
  mode: instant
  params:
    - name: "weapon"
      type: entity_ref
      label: "With what?"
      optional: true
  range: 2.0
  method: <leden_method_ref>

Affordance fields:
Field	Required	Purpose
verb	Yes	Action identifier (domain-defined)
label	Yes	Human-readable label for display
category	Yes	Client rendering hint (from vocabulary)
mode	Yes	Interaction pattern
params	No	Inputs the action needs
range	No	Maximum distance for this action
method	Yes	Leden method reference to call
conditions	No	Client-side hints about requirements

Categories (how the client groups/renders affordances):
Category	Rendering hint	Examples
navigate	Directional controls, map markers	Go north, enter building, climb ladder
interact	Context menu, action button	Open, close, pull lever, read sign
combat	Action bar, targeting UI	Attack, defend, cast spell
trade	Trade dialog	Buy, sell, barter, give
communicate	Dialog/chat interface	Talk to, ask about, persuade
craft	Crafting UI	Forge, brew, enchant, repair
inspect	Info panel	Examine, identify, appraise
use	Quick action	Eat, drink, equip, activate

A text client renders all categories the same way — a numbered menu. A graphical client renders them differently — combat affordances in an action bar, navigate affordances as directional arrows, trade affordances in a dialog window. The category is a suggestion, not a command.

Interaction modes:
Mode	Pattern	Example
instant	One action, immediate result	Take item, attack, go north
targeted	Select a target first	Attack which enemy, give to whom
dialog	Multi-step structured interaction	Trade negotiation, crafting recipe selection
continuous	Ongoing action with a duration	Channel spell, hold position, follow

For dialog mode, the affordance includes a schema — a structured form definition that the client renders as UI:

Affordance:
  verb: "trade"
  label: "Trade"
  category: trade
  mode: dialog
  schema:
    offer:
      type: entity_ref_list
      label: "Your offer"
      source: inventory
    request:
      type: text
      label: "What do you want?"
  method: <leden_method_ref>

The client renders this as whatever UI fits — a split-pane trade window, a series of text prompts, a drag-and-drop interface. The schema defines the data shape. The client decides the presentation.

When the player completes the interaction, the client packages the inputs and calls the Leden method. The domain validates and executes. The result comes back through the observation stream as entity updates.
Appearance

Appearance is layered. Every layer is optional except the first (which is the entity's description, a required field).

Layer 1: Semantic (always available)
Kind + name + description. Every client can render this. A creature named "Goblin Scout" with a description — that's enough for a text game.

Layer 2: Hints (optional, structured)
Rendering suggestions that help graphical clients without requiring custom assets:

appearance:
  shape: humanoid
  scale: [0.8, 0.8, 1.2]
  palette: [green, brown, gray]
  material: leather
  posture: crouching
  emitting: null

Hint vocabulary:
Hint	Values (examples)	Purpose
shape	humanoid, quadruped, serpentine, avian, tree, rock, box, sphere, blade, ...	Base geometry suggestion
scale	[w, h, d] or single float	Relative size
palette	List of color names or hex values	Primary colors
material	stone, wood, metal, cloth, leather, crystal, bone, water, fire, ice, ...	Surface appearance
posture	standing, crouching, prone, sitting, flying, swimming, ...	Current pose
emitting	fire, smoke, light, sparks, mist, ...	Particle/effect hint

A 2D client uses shape + palette to pick a sprite from its library. A 3D client uses shape + scale + material to assemble a procedural model. A text client ignores hints entirely. Hints are optimization — the description is always truth.

Layer 3: Assets (optional, content-addressed)
Custom visuals for domains that want specific look:

appearance:
  ...hints...
  assets:
    sprite: sha256:a1b2c3...    # 2D sprite sheet
    model: sha256:d4e5f6...     # 3D model (glTF)
    icon: sha256:789abc...      # UI icon
    portrait: sha256:def012...  # Dialog portrait
    sound: sha256:345678...     # Associated sound

Assets are content-addressed blobs in Leden's content store. The client fetches what it needs based on its rendering capability. A text client fetches nothing. A 2D client fetches the sprite. A 3D client fetches the model. Asset format is encoded in the blob's metadata (stored with the content hash).

If the client can't fetch an asset (network issue, unsupported format), it falls back to hints. If no hints, it falls back to kind + description. The layers degrade gracefully. Always.
Fidelity Negotiation

The client declares what it can handle. The domain uses this to tailor its descriptions.

This is HTTP's Accept header applied to world description. The client says what it supports. The domain picks the best match.

Fidelity is declared once at session start, during Leden bootstrap:

client_fidelity:
  rendering: [text, tiles_2d, scene_3d]
  max_entities: 200
  asset_formats: [png, gltf, ogg]
  interaction: [keyboard, mouse]
  audio: true
  spatial_preference: grid_2d

Field	Purpose
rendering	What rendering modes the client supports (ordered by preference)
max_entities	How many entities the client can handle at once
asset_formats	What asset formats the client can load
interaction	Input methods available
audio	Whether the client can play audio
spatial_preference	Preferred spatial model (domain may override)

The domain uses fidelity to:

    Choose spatial model. If the domain supports multiple layouts, pick the one that matches the client.
    Filter appearance layers. Don't send 3D asset hashes to a text client.
    Limit entity count. Send the most relevant entities within the client's budget. A text client gets the 20 most important things. A 3D client gets 200.
    Pick asset formats. If the client supports glTF, reference glTF assets. If only PNG, reference sprites.

Fidelity is a declaration, not a negotiation. The domain reads it and adapts. No back-and-forth. If the domain can't serve the client's capabilities at all (a 3D-only domain with a text-only client), it says so at bootstrap and the client can disconnect gracefully.
Integration with Leden

WDP is a content schema. Leden is the protocol. Here's how they compose.
Session Setup

    Client connects to domain's bootstrap address (Leden Layer 0-1)
    Client authenticates with the domain's greeter (Leden Layer 2-3)
    Client declares client_fidelity as part of the greeter handshake
    Greeter returns: a capability for the domain's region directory and the player's initial region

The greeter is the only public capability. Everything else flows from it.
Region Entry

    Client receives a region object reference (from greeter, from a portal, from another region)
    Client calls Observe(region_ref) (Leden observation)
    Domain responds with a region snapshot — the full WDP region description
    Client renders the region
    Observation stream begins — client receives deltas

The region observation composes with promise pipelining: "resolve this portal, then observe the destination region" is one round trip.
Observation Flow

The domain sends updates through Leden's observation stream:
Update	Payload	When
entity_enter	Full entity description	Entity appears in region
entity_exit	Entity ref	Entity leaves region
entity_update	Ref + changed fields	Entity properties change
affordance_update	Ref + new affordance list	Available actions change
ambient_update	Changed ambient fields	Environment changes

These map directly to Leden observation deltas. The region object is the publisher. Subscribed clients are the observers. Leden handles fan-out, backpressure, sequence numbering, and reconnection.

For high-frequency updates (entity movement), the client can observe individual entities with filtered properties:

Observe(entity_ref, filter: [position])

This gives position-only updates at high frequency without the overhead of full entity deltas. The region observation handles add/remove (low frequency). Individual observations handle property changes (high frequency).

Coalescing strategy: Game entities should use coalesce-on-backpressure. The client wants the latest position, not every position along the path. This is configured on the domain side — Leden's observation backpressure model handles it.
Interaction Execution

When the player selects an affordance:

    Client reads the affordance's method field (a Leden method reference)
    Client packages any params the player provided
    Client calls the method on the entity's object reference
    Domain validates and executes (Raido script, domain logic, whatever)
    Results arrive through the observation stream as entity/region updates

The client never calls domain-specific APIs. It calls the method that the affordance told it to call. New domain features are new affordances on entities — the client discovers and renders them without code changes. This is the HATEOAS guarantee.

Error handling: If the method call fails (insufficient permissions, out of range, invalid state), the domain returns a structured error through the Leden promise. The client displays it. The affordance's conditions field helps the client avoid common errors proactively (graying out "attack" when out of range), but the domain always validates server-side.
Progressive Rendering Example

The same region data, three clients:
Text Client

=== The Rusty Anchor ===
A cramped tavern. Smoke hangs low. The bar runs the length of the
far wall, bottles glinting in candlelight. A staircase in the corner
leads up.

You see:
  Barkeep Marta (creature) — A stout woman polishing glasses,
    watching the room with tired eyes.
  Dusty Bottle (item) — An unlabeled bottle, thick with dust.
  Staircase (portal) — Creaky wooden stairs leading to rooms above.

Actions:
  1. Talk to Barkeep Marta [communicate]
  2. Take Dusty Bottle [interact]
  3. Go upstairs [navigate]
  4. Look around [inspect]

Uses: name, description, kind, affordances.label, affordances.category
2D Tile Client

Renders a 12x8 grid. Barkeep Marta is a humanoid shape sprite in her palette colors at position [6, 2]. The dusty bottle is an item icon at [8, 5]. The staircase is a portal tile at [11, 7]. Ambient dim lighting applies a dark overlay. Clicking an entity shows its affordances as a context menu.

Uses: everything above + position, appearance.shape, appearance.palette, ambient
3D Client

Builds a tavern interior from built-in assets (shape hints for walls, bar, stools). Loads custom model for Barkeep Marta if the domain provides one via appearance.assets.model. Spatial audio for tavern_murmur ambient. Particle system for candlelight. Player right-clicks Marta to see affordances in a radial menu.

Uses: everything above + appearance.assets, appearance.material, appearance.scale, spatial audio

Same WDP payload. Zero domain-specific client code.
The Vocabulary

WDP defines mechanisms (kinds, shapes, materials, categories). The initial terms are listed above in their respective sections. The vocabulary is extensible without protocol changes — new terms are just new strings. Clients that don't recognize a term fall back to the category or ignore it.

Over time, commonly-used terms will become de facto standards. When 200 domains all use shape: humanoid, that's a standard. No committee needed. The same way HTML elements standardized through browser adoption, not W3C edicts (the edicts came after).

Domain unions (from the existing Midgard design) accelerate vocabulary convergence. A union of 50 domains that all agree on the same entity types, appearance hints, and affordance verbs creates a pocket of perfect interop. WDP doesn't need to know unions exist — it just sees consistent vocabulary use.
What This Doesn't Cover

UI layout. WDP describes the world, not the client's interface. Health bars, minimaps, quest trackers, inventory screens — these are client concerns. The client builds UI from entity data (health from properties, minimap from region layout, inventory from a container entity's contents).

Physics. WDP doesn't describe collision volumes, rigid body properties, or physics constraints. If a domain needs physics-aware clients, it uses well-known properties (solid: true, mass: 5.0) and the client interprets them. Full physics simulation is domain-side (Raido).

Animation. WDP doesn't describe skeletal rigs or animation state machines. The posture hint covers coarse state ("crouching", "attacking", "idle"). Smooth animation is the client's problem, driven by posture changes in the observation stream.

Audio design. WDP carries ambient properties and sound asset references. Spatial audio mixing, music systems, and sound design are client-side. The domain says "there's a fire here." The client decides what fire sounds like.

Scripting. No behavior in the description. Ever. Raido handles scripting. WDP handles description. The boundary is load-bearing.

Entity internals. WDP describes what an entity looks like from outside. Its internal state machine, its Raido scripts, its capabilities graph — all opaque. The domain exposes what it wants through properties and affordances.
Resolved

Regions are not entities. A region is a container. Entities are contents. Regions have metadata (name, description, ambient, spatial model). Entities have affordances and appearance. Mixing them creates ambiguity about what "observing an entity" means vs. "observing a region." Clean separation.

Portals are entities (kind=portal) that reference target regions. Navigation is: entity (portal) → region → entities. This gives you cross-region links (like HTML hyperlinks) without nesting regions inside regions.

One spatial model per region. A region doesn't present itself differently to different clients. It has one spatial model. Clients adapt. A text client can render continuous_3d as a list — it just ignores coordinates. The alternative (per-client spatial models) requires the domain to maintain multiple representations, which doesn't scale.

Affordances over methods. The client doesn't call entity methods directly. It discovers affordances, which contain method references. This indirection is the key to client-domain decoupling. A domain can change its internal method structure without breaking clients — it just updates the affordance's method field. Clients never hardcode method names.

No inheritance in the entity model. USD and Roblox use class hierarchies. ECS uses composition. WDP uses composition — an entity is a bag of kind + properties + affordances + appearance. No "class GenericSword with subclass Flamebrand." Inheritance creates coupling between entity definitions that breaks across domain boundaries. Composition lets two domains agree on individual properties without agreeing on a type hierarchy.

Content-addressed assets, not URLs. Assets are identified by content hash, not location. This means: deduplication across domains is free, integrity verification is free, and caching is trivial. Two domains that independently use the same goblin sprite share the content hash. The client fetches it once. This falls directly out of Leden's content store.
Open Questions

Entity relationships. An NPC might be "in a group with" other NPCs. A sword might be "equipped by" a character. A chair might be "part of" a table set. Should WDP express relationships between entities, or is that just properties (equipped_by: <ref>)? Flecs-style entity relationships are powerful but add complexity. Properties might be enough for v1.

Region transitions. When a player enters a portal, what does the client experience? Instant swap (region A disappears, region B appears)? Gradual transition (both regions visible briefly)? The domain might want to control this. Should portals have transition hints (transition: fade, transition: walk, transition: instant)?

Observation granularity. The current model is: observe the region for add/remove, observe individual entities for property changes. Is this the right split? For a region with 500 entities, the client might want "observe all entities with position changes" as a single subscription instead of 500 individual ones. Leden's ObserveBatch handles the wire-level cost, but the conceptual model might need a "region-level property filter."

Domain-specific UI. Some domains need UI that doesn't map to entities — a skill tree, a faction reputation screen, a build mode toolbar. These aren't world description. Should WDP punt on these entirely (domain provides a custom UI layer, client renders it as a webview or ignores it)? Or should there be a lightweight "panel" concept in WDP for structured non-spatial information?
Deferred

    Wire format. Binary vs text. Depends on Leden's wire format decision. WDP is a schema — the encoding is separate.
    Vocabulary registry. A formal list of kinds, shapes, materials, and categories with semantic definitions. Needed before implementation, not before design.
    LOD (Level of Detail). Distant entities could be sent with less detail. The mechanism exists (fidelity negotiation + filtered observation), but the specific LOD policy is implementation-level.
    Accessibility. Screen reader hints, colorblind palettes, motor-impairment interaction modes. Important, but a layer on top of the base protocol, not a change to it.
    Versioning. WDP will evolve. Version negotiation should follow Leden's model (version handshake at session start, backward-compatible additions don't require version bumps). Details after v1 is stable.
