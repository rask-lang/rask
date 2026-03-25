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

Eight concepts: regions, entities, affordances, appearance, panels, themes, spatial layers, and input streams.
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
  theme:
    mood: gritty
    palette: [#2a1a0e, #5c3a1e, #8b6914, #1a1a1a]
    saturation: low
    contrast: high
    epoch: medieval
    density: cluttered
    stylesheet: sha256:ef9a01...
  entities: [...]

Region fields:
Field	Required	Purpose
name	Yes	Display name
description	Yes	Text description — the universal fallback
spatial	No	How positions work (see below)
ambient	No	Environmental properties
theme	No	Visual identity and mood (see WDS)
layers	No	Dense spatial data — terrain, tilemaps, voxels, collision geometry
physics	No	Physics parameters for client-side simulation
comfort	No	Immersive comfort hints (locomotion modes, vignette settings)
tick_rate	No	Server update frequency in Hz (0 = event-driven)
entities	Yes	Things in the region

Spatial models:
Model	Positions	Use case
continuous_3d { bounds }	[x, y, z] floats	Full 3D worlds
continuous_2d { bounds }	[x, y] floats	Top-down or side-view
grid_2d { width, height }	[col, row] integers	Tile-based games
hex { width, height }	[q, r] axial integers	Strategy games, hex-based worlds
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
orientation	No	Facing direction — [qx, qy, qz, qw] quaternion, or degrees for 2D
properties	No	Key-value extensible data
affordances	No	What you can do with/to this entity
appearance	No	Rendering hints and asset references

Entity kinds (the initial vocabulary — extensible):
Kind	Meaning	Fallback rendering
creature	Living entity — player, NPC, monster, animal	Name + description
item	Portable object — sword, potion, tool, material	Name + "you could pick this up"
structure	Fixed construction — wall, door, chest, table, building	Name + part of environment
terrain	Ground/environment — grass, water, stone, lava	Background/floor
portal	Navigation point — door, gate, path, teleporter	Directional prompt (+ transition hint)
effect	Transient phenomenon — fire, mist, spell, sound	Flavor text
marker	Abstract point — spawn, waypoint, boundary	Not rendered. Editor/debug mode may visualize
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

Property semantics convention: numeric properties that represent a bounded value use `X` + `X_max` naming. Values are always absolute, never percentages. `health: 30` means 30 hit points, not 30%. `health` without `health_max` means the maximum is unknown — the client shows the number but can't render a bar. This matters for cross-domain consistency: two domains using `health` to mean different things (absolute vs. percentage vs. armor-adjusted) would break the smart UI promise.
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
predicted	No	Whether the client can apply the result optimistically

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
proximity	Triggered by spatial closeness	Pick up nearby item, open door you're standing at, grab object in reach
batch	One action applied to multiple entities	Command selected units, loot all nearby items

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
Panels

Some domain UI doesn't map to entities in a region — skill trees, faction reputation, crafting grids, build mode toolbars, quest logs. These aren't world description. But punting them to "each domain builds custom UI" defeats the whole point of WDP.

I decided to use HTML. Not invent a new UI description language — the web already has one with 30 years of tooling, accessibility support, and rendering engines. A panel is a sandboxed HTML fragment that a domain sends for non-spatial UI.

Panel:
  id: "skill_tree"
  label: "Skills"
  category: character
  fallback: "Strength: 5, Agility: 3, Magic: 7"
  content_type: text/html
  content: sha256:abc123...

Panel fields:
Field	Required	Purpose
id	Yes	Stable identifier for the panel
label	Yes	Human-readable name
category	Yes	Grouping hint (character, inventory, social, craft, quest, system)
fallback	Yes	Plain text summary — always renderable
content_type	Yes	MIME type of the content (text/html for now)
content	Yes	Content-addressed blob reference

The content is sandboxed HTML/CSS — no JavaScript, no external resources. Think HTML email, not a web app. The domain authors a self-contained fragment. The client renders it in an iframe with sandbox restrictions or shadow DOM. Interaction flows through WDP affordances embedded in the HTML as data attributes, not through JS event handlers.

A text client renders the fallback string. A web-based client renders the HTML natively. A native client can use a lightweight HTML renderer or fall back to the text. Same progressive enhancement as everything else in WDP.

Why HTML and not a structured schema: I considered a custom layout language. But any layout language rich enough for skill trees and crafting grids would end up being a bad version of HTML. CSS already solves layout. HTML already has form elements. Screen readers already understand both. The alternative is years of design work to build something worse than what exists.

Panels are delivered through the observation stream like everything else. A panel_update delta carries a new content hash when the domain changes the panel's contents. The client fetches the new blob and re-renders.

Panels are not entities. They don't have positions, affordances, or appearance. They're a parallel content channel for structured information that lives outside the spatial world. A domain can send zero panels (pure world interaction) or many (complex RPG with character sheets, quest logs, faction standings).

Panel interaction convention: clickable elements in panel HTML use data attributes that the client intercepts and translates to affordance calls:

    <button data-wdp-verb="unlock_skill"
            data-wdp-param-skill="fireball"
            data-wdp-method="<leden_method_ref>">
      Learn Fireball
    </button>

The client listens for click events on elements with `data-wdp-verb`, extracts the parameters (any attribute starting with `data-wdp-param-`), and calls the referenced Leden method. The domain receives the call and validates. Results come back through panel_update in the observation stream.

This means panel HTML is a layout and display concern. Interactivity is WDP's job — the client IS the JavaScript runtime. CSS handles hover states, transitions, and visual feedback. The domain authors HTML the same way you'd author an HTML email with a few clickable buttons.
Theme

Regions carry a theme field for visual identity — the domain's way of saying "this place should feel like this." The full theme system is specified separately in [WDS.md](WDS.md) (World Description Style), the same way CSS is a separate spec from HTML. They evolve independently: WDP's structure is stable, styling evolves fast. A WDP implementation is complete without WDS — it just uses client defaults.

Brief summary of what WDS provides:

- Design tokens. Flat key-value pairs (`color.primary: #2a1a0e`, `atmosphere.fog_density: 0.4`, `entity.hostile_tint: #ff2200`) that every client type can map to its rendering system. Text clients map colors to ANSI. 3D clients map atmosphere tokens to shaders. No selector syntax, no specificity bugs.

- Structured hints. Coarse mood signals (`mood: gritty`, `epoch: medieval`, `saturation: low`) for clients that don't want to parse individual tokens. A simple client picks a preset from mood + epoch.

- CSS stylesheet. Content-addressed CSS blob for panel styling and web client UI theming. Domain stylesheets reference tokens via CSS custom properties (`var(--wdp-color-primary)`). Walking through a portal shifts the entire client's UI palette.

- Three-level cascade. Domain → region → entity. Domain is the brand. Region is the scene. Entity is the individual. Last writer wins.

See [WDS.md](WDS.md) for the full design: token categories, cascade rules, stylesheet constraints, security model, and per-client-type consumption examples.
Fidelity Negotiation

The client declares what it can handle. The domain uses this to tailor its descriptions.

This is HTTP's Accept header applied to world description. The client says what it supports. The domain picks the best match.

Fidelity is declared at session start during Leden bootstrap, and can be renegotiated mid-session via a `fidelity_update` message. A client that switches from windowed to fullscreen, or a mobile client that rotates orientation, sends an updated fidelity declaration. The domain adjusts what it sends going forward — no snapshot reset needed, just different filtering on the delta stream.

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
panels	Whether the client can render HTML panels (bool)
immersive	VR/AR/XR capabilities (see Immersive Capabilities)
physics	Whether the client can run local physics simulation (bool)

The domain uses fidelity to:

    Choose spatial model. If the domain supports multiple layouts, pick the one that matches the client.
    Filter appearance layers. Don't send 3D asset hashes to a text client.
    Limit entity count. Send the most relevant entities within the client's budget. A text client gets the 20 most important things. A 3D client gets 200.
    Pick asset formats. If the client supports glTF, reference glTF assets. If only PNG, reference sprites.

Fidelity is a declaration, not a negotiation. The domain reads it and adapts. No back-and-forth. If the domain can't serve the client's capabilities at all (a 3D-only domain with a text-only client), it says so at bootstrap and the client can disconnect gracefully.

For large regions (cities, open worlds), the client also reports its viewport — the spatial area it's currently displaying. The domain uses the viewport to decide which entities to include in the observation stream. A client showing a 20x15 tile area of a 500x500 city only receives entities within (and slightly beyond) that viewport. The client sends viewport updates as it scrolls or moves the camera.

client_viewport:
  center: [120, 85]
  radius: 25

The viewport is a circle (center + radius) regardless of spatial model. The domain sends entities within the radius, plus a buffer for smooth scrolling. Entity enter/exit deltas fire as entities cross the viewport boundary, not the region boundary. This is how WDP scales to large regions without sending 10,000 entities on initial snapshot.
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
panel_update	Panel id + new content hash	Domain UI changes
theme_update	Changed tokens and/or hints	Visual identity changes (see WDS)
layer_update	Layer id + changed chunk hashes	Terrain/block modifications

These map directly to Leden observation deltas. The region object is the publisher. Subscribed clients are the observers. Leden handles fan-out, backpressure, sequence numbering, and reconnection.

For high-frequency updates (entity movement), the client can observe individual entities with filtered properties:

Observe(entity_ref, filter: [position])

This gives position-only updates at high frequency without the overhead of full entity deltas. The region observation handles add/remove (low frequency). Individual observations handle property changes (high frequency).

For regions with many entities, the client can also filter at the region level:

Observe(region_ref, entity_filter: [position])

This gives position updates for all entities in the region as a single subscription, instead of requiring one observation per entity. The region observation handles structural changes (add/remove), the region-level filter handles bulk property streaming (all positions), and individual entity observations handle detailed per-entity tracking. Three tiers, matching different update frequencies.

Coalescing strategy: Game entities should use coalesce-on-backpressure. The client wants the latest position, not every position along the path. This is configured on the domain side — Leden's observation backpressure model handles it.

Tick rate: The region snapshot includes a `tick_rate` field (updates per second) so the client can calibrate interpolation. A turn-based domain sends `tick_rate: 0` (event-driven, no interpolation needed). A real-time action domain sends `tick_rate: 20` (the client interpolates between position updates at 20Hz). The client renders at its own frame rate — tick rate is about the domain's update frequency, not the client's display refresh.
Interaction Execution

When the player selects an affordance:

    Client reads the affordance's method field (a Leden method reference)
    Client packages any params the player provided
    Client calls the method on the entity's object reference
    Domain validates and executes (Raido script, domain logic, whatever)
    Results arrive through the observation stream as entity/region updates

The client never calls domain-specific APIs. It calls the method that the affordance told it to call. New domain features are new affordances on entities — the client discovers and renders them without code changes. This is the HATEOAS guarantee.

Error handling: If the method call fails (insufficient permissions, out of range, invalid state), the domain returns a structured error through the Leden promise. The client displays it. The affordance's conditions field helps the client avoid common errors proactively (graying out "attack" when out of range), but the domain always validates server-side.
Client-Side Prediction

For real-time interaction (movement, combat), the round-trip through "affordance → Leden method → observation update" adds perceptible latency. Without prediction, everything feels like 200ms input lag.

Affordances with `predicted: true` tell the client it can apply the expected result locally before the server confirms. The client acts on the optimistic result immediately and reconciles when the authoritative update arrives through the observation stream.

What the client predicts is the client's problem. WDP doesn't carry prediction logic — that would violate "description is not behavior." The `predicted` flag is permission: "this action's effect is predictable enough that you should try." A movement affordance is predictable. A "open mysterious chest" affordance is not.

If the server result differs from the prediction, the client snaps to the authoritative state. Smooth reconciliation (interpolation, rollback) is a client rendering concern. The domain sends truth. The client makes it feel good.

This is the same model every multiplayer game uses. The difference is that WDP makes it opt-in per affordance rather than a global client assumption. A domain with deterministic physics marks movement as predicted. A domain with complex server-side logic marks nothing as predicted. The client adapts.
Input Streams

Affordances model discrete actions: "attack", "open door", "move to [5, 3]". Some interactions are continuous high-frequency data that doesn't fit the request-response pattern: player movement (gamepad stick at 60Hz), mouse aim, VR head/hand pose at 90Hz. Issuing an affordance call per input frame is too heavyweight — that's 90 method calls per second per tracked point.

Input streams are a lightweight client→server channel for continuous positional data. The client publishes. The domain subscribes. Other clients observe the result through the normal entity observation stream.

The player entity has input stream endpoints declared by the domain:

input_streams:
  - id: position
    type: pose_3d       # [x, y, z, qx, qy, qz, qw]
    rate: 20            # domain wants 20Hz updates from client
  - id: head
    type: pose_3d
    rate: 90
  - id: left_hand
    type: pose_3d
    rate: 90
  - id: right_hand
    type: pose_3d
    rate: 90
  - id: aim
    type: direction_2d  # [yaw, pitch]
    rate: 60

Input stream fields:
Field	Required	Purpose
id	Yes	Stream identifier
type	Yes	Data type: pose_3d, position_3d, position_2d, direction_2d, float, bool
rate	Yes	Maximum update rate the domain accepts (Hz)

The client sends input at the requested rate (or lower if it can't keep up). The domain processes input server-side and publishes the authoritative result to other observers through entity_update deltas. The client that sent the input applies it locally (predicted) and reconciles on the authoritative update.

Input streams are Leden observation in reverse: the client is the publisher, the domain is the observer. They use the same coalescing and backpressure model — if the domain can't keep up, it gets the latest value, not a queue of stale frames.

A VR client with head + two hand tracking sends three pose streams. The domain receives them, validates (prevent teleportation hacks, enforce physics), and fans out the result to other players through the normal observation stream. Other clients see the VR player's avatar moving its head and hands.

A non-VR client with a gamepad sends one position stream (stick movement) and maybe one aim stream (right stick or mouse). A text client sends no input streams — it uses discrete movement affordances. The domain adapts to what the client provides.

Input streams don't replace affordances. Moving around is an input stream. Attacking is an affordance. Aiming is an input stream. Pulling the trigger is an affordance. Streams handle continuous state. Affordances handle discrete events. They compose.
Spatial Layers

Entities work for sparse worlds — 20 things in a tavern, 200 in a battlefield. Dense worlds break this model. A Minecraft chunk is 65,536 blocks. A platformer level is collision geometry. A terrain system is a heightmap. These aren't entities — they're bulk spatial data.

Spatial layers sit alongside entities in a region. A layer is a typed array of spatial data that the client renders as background/environment. Entities exist ON TOP of layers.

Region:
  name: "Crystal Caverns"
  spatial: continuous_3d { bounds: [256, 64, 256] }
  layers:
    - id: terrain
      type: heightmap
      resolution: [256, 256]
      data: sha256:abc123...
    - id: blocks
      type: voxel_3d
      chunk_size: 16
      palette: sha256:def456...   # block type definitions
      chunks: [sha256:111..., sha256:222..., ...]
    - id: collision
      type: mesh_2d
      data: sha256:789abc...
  entities: [...]

Layer types:
Type	Data	Use case
heightmap	2D grid of elevation values	Terrain in 3D worlds
tilemap_2d	2D grid of tile IDs + tile palette	Platformers, top-down games, pixel art worlds
voxel_3d	3D grid of block IDs + block palette	Minecraft-style voxel worlds
mesh_2d	2D collision polygons (line segments, shapes)	Platformer level geometry, walls, slopes
mesh_3d	3D collision mesh (triangles)	Complex 3D environments
navmesh	Walkable area graph	Pathfinding for NPCs and AI

Layer data is content-addressed, like assets. Large layers (voxel worlds) are chunked — the client fetches chunks within its viewport. Layer updates arrive through the observation stream:

layer_update	Layer id + changed chunk hashes	Terrain/block modifications

A text client ignores layers entirely — it uses entity descriptions. A 2D client renders tilemap_2d layers as background tiles. A 3D client renders heightmaps, voxel chunks, and collision meshes. Progressive enhancement, as always.

Layers also carry physics-relevant data. A platformer's mesh_2d layer defines collision geometry — slopes, one-way platforms, moving platform paths. The client can run local physics against the layer data for predicted movement. The domain validates authoritatively.

Spatial layers don't replace entities. The terrain is a layer. The goblin standing on the terrain is an entity. A tree might be either — a decorative tree in a forest is part of a layer, a specific tree the player can chop down is an entity. The domain decides the boundary.
Physics Parameters

Some domains need clients to run local physics — platformers, racing, VR hand interaction, any game where frame-precise movement matters. "Description is not behavior" means WDP doesn't carry physics logic. But physics parameters (gravity, friction, collision rules) are description — they describe the physical properties of the space, not what happens in it.

Regions can declare physics parameters:

physics:
  gravity: [0, -9.8, 0]
  drag: 0.01
  move_speed: 5.0
  jump_velocity: 8.0
  friction: 0.3
  collision: layers    # collide against spatial layers

Physics fields:
Field	Purpose
gravity	Gravitational acceleration vector
drag	Air/fluid resistance factor
move_speed	Base movement speed (domain-defined units)
jump_velocity	Initial jump velocity (0 = no jumping)
friction	Surface friction coefficient
collision	What the player collides with: layers, entities, both, none

These are parameters, not a physics engine. The client plugs them into whatever physics system it uses — Unity's Rigidbody, a custom Verlet integrator, a simple Euler step. The domain provides the constants. The client provides the simulation. The domain validates the result.

Entities that participate in physics carry physics-relevant properties:

properties:
  solid: true
  mass: 5.0
  friction: 0.8       # surface override
  bouncy: 0.3
  kinematic: true      # moves but isn't pushed by others

A VR client uses physics parameters + spatial layers to simulate hand interaction locally: the hand collides with objects, objects have mass and friction, the client predicts the physical result and sends it to the domain for validation. Without physics parameters, VR interaction would require a server round-trip for every hand movement against every object. That's 200ms input lag on touching a table. Unacceptable.

A text client ignores physics parameters. A 2D client might use gravity + friction for simple character movement. A 3D client uses the full set. A VR client adds hand physics on top. Progressive enhancement.
Immersive Capabilities

VR, AR, and spatial computing clients declare their capabilities through the fidelity system. The domain adapts what it sends.

client_fidelity:
  rendering: [scene_3d]
  immersive:
    type: vr
    tracking: [head, hands]
    room_scale: true
    controllers: [hand_tracking, touch]
    refresh_rate: 90
  max_entities: 200
  asset_formats: [gltf, ogg]

Immersive fidelity fields:
Field	Purpose
type	vr, ar, xr, or spatial — what kind of immersive display
tracking	What the client tracks: head, hands, body, eyes, face
room_scale	Whether the client has room-scale tracking (vs. seated/standing)
controllers	Input types: hand_tracking, touch, gamepad, wand, gaze
refresh_rate	Display refresh rate — affects tick rate and input stream expectations

The domain uses immersive fidelity to:

- Set up input streams. A VR client with hand tracking gets head + left_hand + right_hand input stream endpoints. A seated VR client gets head only.
- Mark proximity affordances. Objects in a VR domain get `mode: proximity` affordances with appropriate ranges (0.3m for grabbing, 1.0m for interacting).
- Send comfort metadata. The region includes comfort hints:

comfort:
  locomotion: [teleport, smooth, snap_turn]
  vignette_on_move: true
  seated_mode: supported

- Choose appropriate spatial model. VR domains use continuous_3d. The domain can reject clients without 3D support at bootstrap.

Comfort is a domain declaration, not a client request. The domain says "I support teleport and smooth locomotion." The client picks which one to use based on user preference. The domain doesn't need to know — both result in the same position input stream, just with different positional patterns (discrete jumps vs. smooth movement).

Haptic feedback: affordances can carry a `haptic` field:

Affordance:
  verb: "grab"
  mode: proximity
  range: 0.3
  predicted: true
  haptic:
    pattern: pulse
    intensity: 0.5
    duration: 50

Haptic fields are hints. VR clients with haptic controllers apply them. All other clients ignore them. A text client rendering a proximity affordance shows it as a regular menu item.

Immersive clients are just clients. They render WDP regions, observe entities, call affordance methods. The immersive extensions (input streams, proximity mode, physics, comfort, haptics) are all progressive enhancements. A domain that sends them works fine with a non-immersive client — the extensions are ignored. A VR client connecting to a non-immersive domain works fine too — it uses standard 3D rendering and falls back to menu-based affordances.
Progressive Rendering Example

The same region data, four clients:
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

VR Client

Same tavern, but you're standing in it. Head tracking renders the scene at 90Hz from your eye position. Barkeep Marta has a 3D model (appearance.assets.model) or a procedural humanoid assembled from shape + scale + material hints. Reaching toward the Dusty Bottle triggers its proximity affordance — your hand enters the 0.3m grab range and the client highlights it. Squeeze to grab (affordance call with predicted: true), the bottle follows your hand locally while the server confirms. Spatial audio: Marta's voice comes from her position, tavern murmur is ambient. Candlelight is a volumetric light source from the effect entity.

Uses: everything above + orientation, input streams (head, hands), physics parameters, proximity affordances, haptic hints, comfort settings

Same WDP payload. Zero domain-specific client code.
The Vocabulary

WDP defines mechanisms (kinds, shapes, materials, categories). The initial terms are listed above in their respective sections. The vocabulary is extensible without protocol changes — new terms are just new strings. Clients that don't recognize a term fall back to the category or ignore it.

Over time, commonly-used terms will become de facto standards. When 200 domains all use shape: humanoid, that's a standard. No committee needed. The same way HTML elements standardized through browser adoption, not W3C edicts (the edicts came after).

Domain unions (from the existing Midgard design) accelerate vocabulary convergence. A union of 50 domains that all agree on the same entity types, appearance hints, and affordance verbs creates a pocket of perfect interop. WDP doesn't need to know unions exist — it just sees consistent vocabulary use.
What This Doesn't Cover

Client UI chrome. WDP describes the world and domain panels, not the client's own interface. Health bars, minimaps, hotkey bindings, settings screens — these are client concerns. The client builds its chrome from entity data (health from properties, minimap from region layout) and its own preferences. Domain-specific UI (skill trees, crafting grids) goes through panels.

Physics simulation. WDP provides physics parameters (gravity, friction, collision rules) and spatial layers (collision geometry). The client runs local physics against these. But the physics engine itself is the client's choice — WDP doesn't specify simulation algorithms, integrator types, or solver iterations. Two clients simulating the same parameters may produce slightly different results. The domain is authoritative; clients predict and reconcile.

Animation. WDP doesn't describe skeletal rigs or animation state machines. The posture hint covers coarse state ("crouching", "attacking", "idle"). Smooth animation is the client's problem, driven by posture changes in the observation stream.

Audio design. WDP carries ambient properties and sound asset references. Spatial audio mixing, music systems, and sound design are client-side. The domain says "there's a fire here." The client decides what fire sounds like.

Scripting. No behavior in the description. Ever. Raido handles scripting. WDP handles description. The boundary is load-bearing.

Entity internals. WDP describes what an entity looks like from outside. Its internal state machine, its Raido scripts, its capabilities graph — all opaque. The domain exposes what it wants through properties and affordances.

Data validation. WDP doesn't specify validation rules. A domain might send `health: 50, health_max: 30` or a position outside the region's bounds. Domains are responsible for consistency. Clients should be tolerant — display what you can, clamp out-of-bounds values, don't crash on contradictions. Postel's law: be conservative in what you send, liberal in what you accept.

Panel security. Panels are sandboxed: no JavaScript, no external resource loading. Clients render panels in sandboxed iframes (`sandbox="allow-same-origin"`) with a Content-Security-Policy that blocks external fetches. A malicious domain cannot use panels to exfiltrate data, track users, or escape the sandbox. For stylesheet security, see [WDS.md](WDS.md).
Resolved

Regions are not entities. A region is a container. Entities are contents. Regions have metadata (name, description, ambient, spatial model). Entities have affordances and appearance. Mixing them creates ambiguity about what "observing an entity" means vs. "observing a region." Clean separation.

Portals are entities (kind=portal) that reference target regions. Navigation is: entity (portal) → region → entities. This gives you cross-region links (like HTML hyperlinks) without nesting regions inside regions.

One spatial model per region. A region doesn't present itself differently to different clients. It has one spatial model. Clients adapt. A text client can render continuous_3d as a list — it just ignores coordinates. The alternative (per-client spatial models) requires the domain to maintain multiple representations, which doesn't scale.

Affordances over methods. The client doesn't call entity methods directly. It discovers affordances, which contain method references. This indirection is the key to client-domain decoupling. A domain can change its internal method structure without breaking clients — it just updates the affordance's method field. Clients never hardcode method names.

No inheritance in the entity model. USD and Roblox use class hierarchies. ECS uses composition. WDP uses composition — an entity is a bag of kind + properties + affordances + appearance. No "class GenericSword with subclass Flamebrand." Inheritance creates coupling between entity definitions that breaks across domain boundaries. Composition lets two domains agree on individual properties without agreeing on a type hierarchy.

Content-addressed assets, not URLs. Assets are identified by content hash, not location. This means: deduplication across domains is free, integrity verification is free, and caching is trivial. Two domains that independently use the same goblin sprite share the content hash. The client fetches it once. This falls directly out of Leden's content store.

Entity relationships are properties for v1. `equipped_by: <ref>`, `contained_in: <ref>`, `group: <ref>`. Flecs-style first-class relationships are powerful but add complexity that isn't justified yet. Properties handle the common cases (equipment, containment, grouping). If the pattern proves too limiting, relationships can be promoted to a first-class concept later. Going the other direction — removing a relationship system — is painful.

Portal transitions are domain-controlled. Portals carry a `transition` hint: `instant` (default), `fade`, `walk`, or `loading`. The client renders what it can — a text client ignores transitions entirely, a graphical client uses the hint to drive its transition animation. The domain decides the experience; the client decides the presentation. Without this, every client guesses differently and cross-domain travel feels jarring.

Observation has three tiers. Region observation for structural changes (entity add/remove). Region-level property filter for bulk streaming (`Observe(region_ref, entity_filter: [position])` gives position updates for all entities as one subscription). Individual entity observation for detailed per-entity tracking. This avoids the 500-subscriptions problem without changing Leden's observation model — region-level filters are just a filtered view over the region's delta stream.

Domain-specific UI uses HTML panels. Domains send sandboxed HTML/CSS fragments for non-spatial UI (skill trees, crafting grids, faction screens). No JavaScript, no external resources. Web clients render natively, text clients show a plain text fallback. I chose HTML over a custom schema because any layout language rich enough for real UI would end up being a bad version of HTML. See the Panels section above.

Visual identity is a separate spec (WDS). Design tokens for world styling, CSS stylesheets for panels and web UI, three-level cascade (domain → region → entity). Separated from WDP because styling evolves faster than structure and has a different implementer audience. A WDP implementation is complete without WDS. See [WDS.md](WDS.md).

VR/AR/XR is supported through general-purpose extensions, not a VR-specific protocol. Entity orientation, input streams (continuous client→server data), proximity affordances, spatial layers (dense geometry), physics parameters, and immersive fidelity fields. All are progressive enhancements — a VR client connecting to a non-VR domain works fine (menu affordances, no hand physics), and a non-VR client connecting to a VR domain works fine (ignores input stream endpoints, uses instant/targeted affordances). The immersive capabilities are the same mechanisms needed for platformers, racing games, and any real-time physics game.

Dense worlds use spatial layers. Tilemaps, voxel chunks, heightmaps, and collision meshes sit alongside entities in a region. Entities are sparse (things you interact with). Layers are dense (the world itself). A Minecraft chunk is a voxel layer. A platformer level is a mesh_2d layer. A terrain system is a heightmap layer. Layers are content-addressed and chunked for viewport-based streaming.

Large regions use viewport filtering. The client reports its viewport (center + radius), and the domain only sends entities within that area. Entity enter/exit deltas fire at the viewport boundary. This scales WDP to open-world regions without dumping 10,000 entities on initial snapshot.
Deferred

    Wire format. Binary vs text. Depends on Leden's wire format decision. WDP is a schema — the encoding is separate.
    Vocabulary registry. A formal list of kinds, shapes, materials, and categories with semantic definitions. Needed before implementation, not before design.
    LOD (Level of Detail). Distant entities could be sent with less detail. The mechanism exists (fidelity negotiation + filtered observation), but the specific LOD policy is implementation-level.
    Accessibility. Screen reader hints, colorblind palettes, motor-impairment interaction modes. Important, but a layer on top of the base protocol, not a change to it.
    Versioning. WDP will evolve. Version negotiation should follow Leden's model (version handshake at session start, backward-compatible additions don't require version bumps). Details after v1 is stable.
    Entity visibility. Fog of war needs a visibility field on entities: visible, last_known (stale data with timestamp), hidden. The domain controls which entities the client knows about. last_known entities carry stale data that the client renders differently (grayed out, question mark). Deferred because most Midgard use cases don't need fog of war, and the viewport filtering mechanism handles the common case of "don't show what's far away."
    Event streams. WDP is state (current properties), not events (what happened). A combat log needs "player X hit boss for 500 damage with Fireball" — that's an event, not a state change. A parallel event channel alongside the observation stream would carry happenings. Deferred because panels can show a combat log (updated via panel_update), which covers the common case without a new concept.
    Time-sequenced content. Rhythm games and cutscenes need pre-loaded event sequences with precise timestamps. The observation model is push-based (server sends updates as they happen), not time-indexed. This is a fundamentally different content type — probably a separate spec rather than a WDP extension. Deferred.
