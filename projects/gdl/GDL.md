Gard Description Language
<!-- id: gdl --> <!-- status: proposed --> <!-- summary: Content schema for describing gard state over Leden -->

GDL is the content schema that tells a client what a gard looks like and how to interact with it. Gards send structured descriptions. Clients decide how to render them. A text client shows room descriptions. A 2D client draws tiles. A 3D client builds a scene. Same data, different presentations.

GDL is not a protocol. It's a content schema that rides on Leden, the same way HTML rides on HTTP. Leden handles sessions, capabilities, observation, and content delivery. GDL defines what's inside the payloads.
Why This Exists

The stack has logic (Raido), trust (Allgard), and transport (Leden). It doesn't have a way to describe what a gard looks like.

Without GDL, every gard invents its own scene format. Clients need per-gard rendering code. Cross-gard travel becomes "load a completely different game." The federation model works at the object level (swords transfer) but fails at the experience level (the player sees nothing coherent).

GDL is the common language between gard and client. A gard describes its world in GDL. A client that speaks GDL can render any gard, the same way a browser that speaks HTML can render any website.
Design Principles

These come from studying what worked and what didn't across 30 years of world description formats.
1. Ignore What You Don't Understand

The single most important rule. If a client receives a property it doesn't recognize, it skips it. If a domain sends an entity kind the client hasn't seen before, the client falls back to the kind's category. Unknown fields are preserved, never rejected.

This is how HTML survived 30 years of evolution. Browsers that don't know <video> render the fallback content inside the tags. GDL clients that don't know a "flamebrand" render it as a generic item. Forward compatibility is non-negotiable.
2. Description Is Not Behavior

GDL describes what exists — structure, properties, appearance. Raido scripts define what happens — combat, crafting, physics. VRML tried to combine scene description with behavioral wiring and paid for it with implementation complexity that killed adoption. X3D inherited the same mistake.

The boundary is sharp: GDL says "there's a goblin here with 30 health." Raido says "when attacked, subtract damage from health." The client renders the description. The domain runs the behavior. They don't mix.
3. Description Is Not Rendering

GDL carries semantic descriptions, not rendering instructions. "A weathered oak door with iron bands" — not vertex buffers, not sprite coordinates, not CSS. The client decides how to present it. A text client prints the description. A 3D client loads the domain's custom door model, or assembles one from its base library, or generates one on the fly.

Second Life got this right with parametric prims — 50 bytes of shape parameters instead of megabytes of mesh data. glTF got it right for transmission (GPU-ready buffers). GDL sits one level higher: content-addressed assets as the primary path, with semantic hints as fallback for any renderer.
4. Progressive Enhancement, Not Multiple Representations

The domain sends one description. Richer clients extract more from it. A text client uses name and description. A 2D client adds position and appearance.shape. A 3D client adds appearance.assets for custom models. Same payload, different extraction depths.

This is HTML's model. One document, many renderers. Not "here's the text version, here's the 2D version, here's the 3D version." That doesn't scale — domains would need to author three descriptions for every entity.
5. Optimize for the Consumer

glTF's tagline is "the JPEG of 3D." It succeeded by optimizing for the renderer, not the authoring tool. GDL optimizes for the client, not the domain. Descriptions are structured for fast parsing, progressive extraction, and incremental updates. The domain does extra work once; thousands of clients benefit.
6. The Simpler System Wins

HTTP beat CORBA. JSON beat XML. HTML beat SGML. In every case, the simpler format won because more people could implement it correctly. A GDL client should be buildable in a weekend. The minimum viable client is a text renderer that prints names, descriptions, and affordance menus. Everything beyond that is progressive enhancement.
Core Model

Seven core concepts that every GDL client must handle:

1. **Regions** — spatial containers. The "page."
2. **Entities** — things in regions, with extensible properties. The "elements."
3. **Affordances** — what you can do. The "links and forms."
4. **Appearance** — what things look like. Content-first, with hints as fallback.
5. **Bonds** — visual relationships between entities. Ropes, beams, chains, links.
6. **Events** — things that happened. Fire-and-forget happenings.
7. **Panels** — domain UI as sandboxed web apps.

Six extensions that clients negotiate through fidelity:

- **Spatial layers** — dense data (voxels, heightmaps, tilemaps)
- **Input streams** — continuous client→server data (movement, tracking, media input)
- **Output streams** — continuous server→client data (bone poses, blend shapes, physics, deformation)
- **Media streams** — audio/video from entities (voice, live performance, video)
- **Nested spaces & reference frames** — sub-spaces in entities, relative positioning
- **Theme** — visual identity system (separate spec: [GDL-style](GDL-style.md))

A minimal GDL client handles the core: render regions, display entities with names and descriptions, show affordance menus, display events as log lines, show panel fallback text. That's a text adventure client. Buildable in a weekend.

Each extension adds a capability. A 2D tile client adds spatial layers. A VR client adds input streams, output streams, and media streams. A client with vehicles adds nested spaces. None require the others. A client that doesn't understand an extension ignores it — the core still works.
Regions

A region is a spatial container. It's the "page" — the top-level context that a client loads and renders. A dungeon room, a forest clearing, a city block, a spaceship interior. Regions connect to other regions through portals.

A region is a Leden object. Observing it gives you GDL content — a snapshot of the region's current state, followed by a delta stream of changes.

Region:
  name: "The Rusty Anchor"
  description: "A cramped tavern. Smoke hangs low. The bar runs the
    length of the far wall, bottles glinting in candlelight. A staircase
    in the corner leads up."
  spatial: grid_2d { width: 12, height: 8 }
  properties:
    tick_rate: 0
  theme:
    mood: gritty
    palette: [#2a1a0e, #5c3a1e, #8b6914, #1a1a1a]
    saturation: low
    contrast: high
    epoch: medieval
    density: cluttered
    tokens:
      atmosphere.lighting: dim
      atmosphere.ambient_sound: tavern_murmur
      atmosphere.temperature: warm
    stylesheet: sha256:ef9a01...
  entities: [...]

Region fields:
Field	Required	Purpose
name	Yes	Display name
description	Yes	Text description — the universal fallback
spatial	No	How positions work (see below)
properties	No	Key-value extensible data (tick_rate, physics params, comfort, domain-defined)
theme	No	Visual identity and mood (see GDL-style)
layers	No	Dense spatial data — terrain, tilemaps, voxels, collision geometry
entities	Yes	Things in the region
bonds	No	Visual relationships between entities (see Bonds)

Regions use the same extensible properties as entities. Environmental properties (lighting, temperature, ambient sound) live in theme tokens — GDL-style already has the `atmosphere.*` namespace for exactly this. Operational parameters (tick_rate, physics, comfort) live in properties.

Properties on regions follow the same rules as entity properties: typed values, unknown keys ignored, extensible. The convention namespaces are:

Namespace	Purpose	Examples
physics.*	Client-side simulation parameters	physics.gravity: [0, -9.8, 0], physics.friction: 0.3
comfort.*	Immersive client hints	comfort.locomotion: [teleport, smooth], comfort.seated_mode: true
tick_rate	Server update frequency in Hz (0 = event-driven)	tick_rate: 20

These aren't special — they're just property keys that clients recognize. A domain can add `weather.wind_speed: 5.0` or `music.bpm: 120` and the protocol doesn't change. Physics was previously a top-level field. It doesn't deserve that status — it's one of many possible region property namespaces.

Spatial models:
Model	Positions	Use case
continuous_3d { bounds? }	[x, y, z] floats	Full 3D worlds
continuous_2d { bounds? }	[x, y] floats	Top-down or side-view
grid_2d { width?, height? }	[col, row] integers	Tile-based games
hex { width?, height? }	[q, r] axial integers	Strategy games, hex-based worlds
graph	Named locations	Text adventures, node-based maps
abstract	None	Inventories, conversations, menus

Bounds are optional. Omitting them means the world is unbounded — it extends as far as the domain can generate. An infinite procedural terrain, an endless ocean, a fractal explorer — all use unbounded spatial models. The viewport mechanism (see Fidelity Negotiation) handles content delivery: the client reports where it's looking, the domain generates content around the viewport. No bounds means no edge.

Bounded worlds declare their extent upfront: `continuous_3d { bounds: [256, 64, 256] }`. The client knows the world's size. Unbounded worlds declare nothing: `continuous_3d {}`. The client discovers the world's extent by moving through it.

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
appearance	No	What it looks like — assets, hints, or both (see Appearance)
appearance_ref	No	Content-addressed reference to a shared appearance definition

Entity kinds tell the client what category of thing this is — how to render it by default, how to present it in lists, what UI patterns apply. Kinds are like HTML elements: a small core set that every client knows, extensible with new terms that degrade gracefully.

Core kinds:
Kind	Meaning	Fallback rendering
agent	Autonomous entity — person, NPC, robot, AI, sensor	Name + description
object	Portable thing — tool, document, item, package	Name + "you could pick this up"
structure	Fixed construction — wall, furniture, building, sign	Part of environment
terrain	Ground/environment — floor, water, surface	Background
portal	Navigation point — door, link, path, teleporter	Directional prompt
effect	Transient phenomenon — notification, animation, particle	Flavor text / toast
marker	Abstract point — waypoint, anchor, boundary	Not rendered
container	Holds other entities — folder, bag, shelf, room	Name + "contains things"
vehicle	Transport with interior — car, ship, elevator	Name + description

Unknown kinds fall back to their closest match. A client that doesn't know `vehicle` treats it as `structure`. A domain can define `sensor`, `widget`, `avatar` — the client falls back to the best-matching core kind.

Kinds are semantic, not cosmetic. A person in a meeting room is `agent`. A sensor on an IoT dashboard is `agent`. A goblin in a dungeon is `agent`. A shared document in a workspace is `object`. A sword in an RPG is `object`. Kind tells the client the category. Name and description carry the specifics. Properties carry the data.
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

Property Patterns

Properties are free-form, but clients can render them smartly if they recognize certain patterns. The patterns matter more than the specific names — a client that understands the bounded numeric pattern can render a bar for `health/health_max`, `energy/energy_max`, `progress/progress_max`, or any other X/X_max pair without knowing what they mean.

Pattern	Convention	Client behavior
Bounded numeric	`X` + `X_max` (int or float)	Render as a bar (X out of X_max)
State enum	string value	Show as status badge or label
Boolean flag	bool value	Toggle UI element visibility or behavior
Entity reference	ref value	Render as a link or association indicator
Tagged	`tags: [...]` (string list)	Filter, search, category display

`health: 30, health_max: 100` → a bar at 30%. `energy: 7.5, energy_max: 10.0` → a bar at 75%. `progress: 3, progress_max: 5` → a bar at 60%. The client doesn't need to know the word "health" — it sees the X/X_max pattern and renders a bar.

Values are always absolute, never percentages. `health: 30` means 30 units, not 30%. `health` without `health_max` means the maximum is unknown — the client shows the number but can't render a bar. This matters for cross-domain consistency.

Specific property names (`health`, `level`, `hostile`, `locked`, `price`) are **conventions**, not protocol. A game domain uses `health`. An IoT domain uses `temperature`. A project tracker uses `progress`. The protocol defines patterns. Communities define conventions.
Affordances

Affordances are what make GDL interactive. They answer: "what can I do here?"

This is HATEOAS applied to virtual worlds. In REST, the server response tells you what actions are available via links and forms. In GDL, the entity description tells you what actions are available via affordances. The client doesn't need compiled-in knowledge of what's possible — the domain tells it, per entity, right now.

LambdaMOO discovered this in 1990: verbs live on objects and are discovered at runtime. "Take ball" works because the ball has a take verb. GDL formalizes the same pattern.

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

Categories are free-form strings — rendering hints that tell the client how to group and present affordances. The client decides what to do with them. A text client ignores categories and renders a numbered menu. A graphical client might render `navigate` as directional arrows and `edit` as a toolbar.

Initial conventions (not exhaustive — domains define their own):
Category	Rendering hint	Examples
navigate	Directional controls, map markers	Go north, enter building, climb ladder
interact	Context menu, action button	Open, close, pull lever, read sign
communicate	Dialog/chat interface	Talk to, ask about, persuade
inspect	Info panel	Examine, identify, appraise
use	Quick action	Eat, drink, equip, activate

A game domain might add `combat`, `trade`, `craft`. A music app adds `playback`. A collaboration tool adds `edit`. A dashboard adds `configure`. Unknown categories get default rendering — the client doesn't need to know every possible category to function.

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

Appearance describes what an entity looks like. The system is designed for domains that create real content — custom models, animations, effects. Hints exist as fallback for lightweight prototyping and graceful degradation, not as the primary authoring path.

Three layers, each optional except the first. A domain that invests in content uses Layer 3 (assets). A domain prototyping uses Layer 2 (hints). A text client uses Layer 1 (semantic). When a higher layer fails (asset not found, unsupported format), the client falls back down. Always.

Layer 1: Semantic (always available)
Kind + name + description. Every client can render this. A creature named "Goblin Scout" with a description — that's enough for a text game.

Layer 2: Hints (optional, structured)
Rendering suggestions that help graphical clients without requiring custom assets. Hints describe *what something is*, not how to render it. The client maps hints to its own rendering system.

appearance:
  shape: humanoid
  scale: [0.8, 0.8, 1.2]
  palette: [green, brown, gray]
  surface:
    roughness: 0.8
    metallic: 0.0
    base: leather
  skeleton: humanoid
  animation: idle
  parts: [horned_helmet, plate_torso, cloth_legs]
  effects:
    - type: fire
      anchor: right_hand
      scale: 0.5
      palette: [#ff4400, #ffaa00]

Hint vocabulary:

**Geometry**

Hint	Values (examples)	Purpose
shape	humanoid, quadruped, serpentine, avian, tree, rock, box, sphere, blade, ...	Base geometry suggestion
scale	[w, h, d] or single float	Relative size
parts	List of part names	Modular attachments/equipment on the base shape

A 2D client uses shape + palette to pick a sprite. A 3D client uses shape + scale to select a base mesh and snaps parts onto it.

Parts can be simple names or structured:

    # Simple — client resolves attachment point from vocabulary
    parts: [horned_helmet, plate_torso, cloth_legs]

    # Structured — explicit attachment and per-part surface
    parts:
      - name: horned_helmet
        attach: head
        surface: { base: bone, roughness: 0.6 }
      - name: plate_torso
        attach: torso
        surface: { base: metal, metallic: 1.0, roughness: 0.3 }
      - name: cloth_legs
        attach: legs
        surface: { base: cloth, roughness: 0.9 }

Simple part names are shorthand — the client resolves the attachment point from part vocabulary conventions. Structured parts give domains explicit control over where parts attach and how they look, including per-part surface properties that override the entity-level surface.

Unknown parts are rendered as the base shape without that part — the entity still appears, just without the attachment. This isn't silent failure; it's the same degradation as a web page with a missing image. The entity is recognizable. The detail is missing. A domain that needs guaranteed visuals ships custom part assets in Layer 3.

**Surface**

Hint	Values (examples)	Purpose
palette	List of color names or hex values	Primary colors (applied to base shape zones)
surface.base	leather, stone, wood, metal, cloth, crystal, bone, water, fire, ice, ...	Material category
surface.roughness	0.0–1.0	PBR roughness (0 = mirror, 1 = matte)
surface.metallic	0.0–1.0	PBR metallic factor (0 = dielectric, 1 = metal)
surface.emissive	hex color or null	Self-illumination color and presence
surface.opacity	0.0–1.0	Transparency (1.0 = opaque, default)

`palette` maps colors to the base shape's zones — a humanoid might map [skin, clothing, accent]. How the client maps colors to zones is client-defined; the convention is index-order from most prominent to least. `surface` provides PBR-compatible material hints for 3D clients. A 2D client uses `palette` and ignores `surface`. A text client ignores both.

Previous flat `material` key is now `surface.base` — same purpose, but surface is a namespace that supports PBR properties alongside the category name. Clients that only understand `surface.base` as a material keyword get the same behavior as before.

**Skeleton and Animation**

Hint	Values (examples)	Purpose
skeleton	humanoid, quadruped, avian, serpentine, arachnid, ...	Which bone rig this entity uses
animation	See below	Current animation state(s)

A skeleton is a named rig type that the client's base library provides. `skeleton: humanoid` tells the client this entity is driven by a humanoid bone hierarchy — two arms, two legs, a head, a spine. The client maps animation state names to animation clips in its library for that skeleton.

Skeleton names are vocabulary conventions. Unknown skeleton names fall back to `shape` for rendering — a client that doesn't know `centaur` ignores the skeleton and renders based on the shape hint. Domains that need custom rigs ship them as Layer 3 assets (see below).

**Animation states** can be a single string or a layered list:

    # Simple — one animation drives the whole skeleton
    animation: walk

    # Layered — different body regions animate independently
    animation:
      - state: run
        layer: full        # default: drives the whole skeleton
      - state: attack
        layer: upper       # upper body only
      - state: look_left
        layer: head         # head only

Animation layer names are skeleton-dependent conventions. A `humanoid` skeleton might define `full`, `upper`, `lower`, `head`, `left_arm`, `right_arm`. A `quadruped` might define `full`, `front`, `rear`, `head`. Unknown layers are ignored — the `full` layer always works as a fallback. Single-string animation (`animation: walk`) is shorthand for `[{state: walk, layer: full}]`.

Core animation state conventions:

State	Meaning
idle	Default stance
walk	Moving at normal speed
run	Moving fast
attack	Melee/ranged attack (one-shot, returns to previous)
cast	Channeling/spellcasting
die	Death animation (one-shot)
sit	Seated
crouch	Low stance
swim	In water
fly	Airborne movement
carry	Holding/transporting something
emote_*	Social animations (emote_wave, emote_dance, emote_bow, ...)

Animation state changes arrive through the observation stream as property updates on the entity. The client transitions between animation clips — blending, crossfading, or snapping depending on its capability. A simple client snaps. A capable client blends. A text client prints "The goblin attacks."

Skeletons also define **attachment points** — named bones where parts mount. A `humanoid` skeleton has `head`, `torso`, `left_hand`, `right_hand`, `back`, `waist`, `feet`. Parts in the `parts` list snap to their conventional attachment point. A `horned_helmet` attaches to `head`. A `plate_torso` attaches to `torso`. Part vocabulary defines which attachment point each part uses.

Clients that don't support skeletal animation fall back to `posture` — a legacy hint that maps to a static pose:

Hint	Values (examples)	Purpose
posture	standing, crouching, prone, sitting, flying, swimming, ...	Static pose fallback

`posture` is derivable from `animation` (idle → standing, crouch → crouching, swim → swimming). Domains can send both for backward compatibility, but `animation` is preferred. If both are present, `animation` wins for clients that support it.

**Effects**

Effects replace the flat `emitting` hint with structured particle/visual effect descriptions:

appearance:
  effects:
    - type: fire
      anchor: right_hand
      scale: 0.5
      palette: [#ff4400, #ffaa00]
    - type: glow
      anchor: self
      palette: [#4488ff]
      intensity: 0.8
    - type: trail
      anchor: feet
      palette: [#aaddff]

Effect fields:

Field	Required	Purpose
type	Yes	Effect type from vocabulary (fire, smoke, sparks, glow, trail, mist, rain, snow, lightning, bubbles, ...)
anchor	No	Where the effect originates — an attachment point name, or `self` for entity center (default: self)
scale	No	Relative size of the effect (default: 1.0)
palette	No	Colors for the effect (overrides client defaults for this effect type)
intensity	No	Brightness/density (0.0–1.0, default: 1.0)
rate	No	Emission rate multiplier (0.0 = paused, 1.0 = normal, 2.0 = double)

Effects are additive — an entity can have multiple simultaneous effects. A flaming sword has `[{type: fire, anchor: blade}, {type: glow, anchor: blade}]`. Effects are hints — the client renders them however it can. A text client prints "the sword is wreathed in flame." A 2D client draws a fire sprite overlay. A 3D client runs a particle system. A client that doesn't recognize an effect type ignores it.

The flat `emitting: fire` shorthand still works as sugar for `effects: [{type: fire}]`. Clients should accept both.

Layer 3: Assets (content-addressed)
The primary authoring path for domains that invest in their world. Content-addressed blobs in Leden's content store — models, skeletons, animations, sprites, effects, materials, sounds.

appearance:
  assets:
    model: sha256:d4e5f6...      # 3D model (glTF, may include rig)
    skeleton: sha256:aab123...   # Custom rig (glTF skeleton, defines bones + attachment points)
    sprite: sha256:a1b2c3...     # 2D sprite sheet
    icon: sha256:789abc...       # UI icon
    portrait: sha256:def012...   # Dialog portrait
    animations:                  # Animation clips keyed by state name
      idle: sha256:aaa111...
      walk: sha256:bbb222...
      attack: sha256:ccc333...
    parts:                       # Custom part meshes keyed by part name
      horned_helmet: sha256:ddd444...
      plate_torso: sha256:eee555...
    effects:                     # Custom particle/effect assets keyed by type
      fire: sha256:fff666...
    materials:                   # Custom material definitions
      enchanted_steel: sha256:ggg777...
    sound: sha256:345678...      # Associated sound
  # Layer 2 hints alongside assets — used as fallback if assets fail
  shape: humanoid
  skeleton: humanoid
  animation: idle
  palette: [green, brown]

Assets are the domain's content. Hints are the safety net. A domain that ships a custom model, skeleton, animations, and parts has full control over how its entities look. The hints describe the *same entity* in terms the client's base library can approximate — so if the model fails to load, the client falls back to a hint-driven humanoid instead of an invisible entity.

The client fetches what it needs based on its rendering capability. A text client fetches nothing. A 2D client fetches the sprite. A 3D client fetches models, animations, parts. Asset format is encoded in the blob's metadata (stored with the content hash).

Assets compose — a domain can ship a custom skeleton with base library animations for standard states, custom animations only for unique ones, and a mix of custom and base library parts. Unspecified states and parts fall back to the base library. A custom `model` replaces the entire shape-based assembly for that entity.

A custom skeleton asset is a glTF file containing the bone hierarchy, attachment point names (as named nodes), and optionally a rest pose. This is how a domain ships a centaur, a six-armed deity, or any creature that doesn't fit the base skeleton types. The skeleton asset defines the bones. The animation assets animate them. The part assets attach to them.

If the client can't fetch an asset (network issue, unsupported format), it falls back to hints. If no hints, it falls back to kind + description. The layers degrade gracefully. Always.

**Appearance references.** Many entities share the same appearance — a forest of 500 trees, a regiment of guards. Sending the same appearance block 500 times wastes bandwidth and makes the domain responsible for keeping them in sync.

An entity can reference a shared appearance definition instead of inlining one:

Entity:
  ref: <leden_object_ref>
  kind: terrain
  name: "Oak Tree"
  position: [45, 12]
  appearance_ref: sha256:tree_oak_01...

The `appearance_ref` is a content-addressed blob containing the full appearance definition (assets + hints). The domain publishes appearance definitions to Leden's content store. Entities reference them by hash. The client fetches the definition once and applies it to every entity that references it.

Appearance references and inline appearance can coexist — an entity with `appearance_ref` can override specific fields inline:

Entity:
  ref: <leden_object_ref>
  kind: terrain
  name: "Dead Oak Tree"
  position: [47, 13]
  appearance_ref: sha256:tree_oak_01...
  appearance:
    palette: [gray, brown]           # override: dead tree is gray
    animation: sway_slow             # override: different animation
    effects:
      - type: mist
        anchor: self
        intensity: 0.3

Inline appearance fields override the referenced definition. Unspecified fields inherit from the reference. This is the same merge-not-replace pattern as theme token inheritance — the reference is the base, inline is the override.

For instanced rendering: when the client sees 500 entities with the same `appearance_ref` and no inline overrides, it can instance them — one draw call, 500 transforms. The content hash makes this optimization trivial to detect. This is how forests, crowds, and particle-heavy scenes stay performant.
Bonds

A bond is a visual relationship between two entities. A rope connecting a boat to a dock. A chain between a prisoner and a wall. A beam of light between a crystal and a receiver. A tether between a player and a grappling hook. These aren't entities — they don't have independent existence, affordances, or ownership. They exist because two entities are connected, and the connection has visual representation.

Without bonds, you'd fake it: create an invisible entity between the two endpoints and update its position/scale every frame. That's a hack that breaks when either endpoint moves, is computationally expensive, and doesn't express the actual relationship.

Bond:
  id: "mooring_rope"
  type: rope
  from: <boat_ref>
  from_anchor: bow
  to: <dock_ref>
  to_anchor: cleat
  appearance:
    surface: { base: fiber, roughness: 0.9 }
    palette: [#8b7355]
  properties:
    tension: 0.7
    sag: 0.3
    thickness: 0.05

Bond fields:

Field	Required	Purpose
id	Yes	Stable identifier within the region
type	Yes	Visual type from vocabulary (rope, chain, beam, tether, lightning, bridge, pipe, wire, ...)
from	Yes	Source entity ref
from_anchor	No	Attachment point on source (default: self)
to	Yes	Target entity ref
to_anchor	No	Attachment point on target (default: self)
appearance	No	Visual properties — surface, palette, assets (same structure as entity appearance)
properties	No	Extensible key-value data (tension, sag, thickness, energy, ...)

Bonds live in the region alongside entities. They arrive in the region snapshot and update through the observation stream:

Update	Payload	When
bond_add	Full bond description	New bond created
bond_remove	Bond id	Bond destroyed
bond_update	Bond id + changed fields	Bond properties change

When either endpoint entity moves, the client re-renders the bond between the new positions. The client decides the visual — a text client prints "a rope connects the boat to the dock." A 2D client draws a line. A 3D client renders catenary physics for rope, rigid links for chain, a particle beam for lightning. The `type` drives the rendering style; `properties` like `sag` and `tension` parameterize it.

Bond types are vocabulary conventions, like entity kinds. Unknown types get a default rendering (a simple line between endpoints). Core conventions:

Type	Rendering hint
rope	Catenary curve with sag, affected by gravity
chain	Rigid linked segments
beam	Straight line of light/energy, may pulse
tether	Elastic connection, stretches with distance
lightning	Jagged, animated electrical arc
bridge	Solid walkable surface between points
pipe	Cylindrical tube, may carry flowing content
wire	Thin, taut line

Bonds can reference appearance assets — a custom chain model, a particle effect for the beam. Same content-addressed asset model as entity appearance. Same fallback chain.

Bonds also support `appearance_ref` for shared definitions — 50 chains in a dungeon reference one chain appearance.

Panels

Some domain UI doesn't map to entities in a region — skill trees, faction reputation, crafting grids, build mode toolbars, quest logs, card game tables. These aren't world description. But punting them to "each domain builds custom UI" defeats the whole point of GDL.

Panels are web apps. A domain authors a standard HTML/CSS/JS application. The client loads it in a sandboxed iframe. The app runs like any web app — it manages its own DOM, handles its own events, maintains its own local state. The only difference from a normal web app: instead of `fetch()` for data and WebSocket for real-time updates, the panel uses `postMessage` to talk to the client, which relays to the domain through Leden.

That's the mental model. postMessage replaces the network. Everything else is normal web development.

Panel:
  id: "skill_tree"
  label: "Skills"
  category: character
  fallback: "Strength: 5, Agility: 3, Magic: 7"
  content: sha256:abc123...

Panel fields:
Field	Required	Purpose
id	Yes	Stable identifier for the panel
label	Yes	Human-readable name
category	Yes	Grouping hint (character, inventory, social, craft, quest, system)
fallback	Yes	Plain text summary — always renderable
content	Yes	Content-addressed blob reference (the web app bundle)
Lifecycle

The content blob is the **application code**. It loads once. It stays running. It manages its own state.

Data changes arrive through `postMessage` — the client pushes observation deltas to the panel as they arrive. The panel updates its own DOM. No re-render, no state loss, no flicker. Scroll position, form inputs, drag state, animations — all preserved across updates.

`panel_update` in the observation stream means the **application code changed** — the domain deployed a new version of the panel. The client fetches the new blob and reloads the iframe. This is rare — a redeploy, not a data update.

    domain sends observation delta → client receives it →
    client posts to panel via postMessage → panel updates its DOM
Sandbox

The security model is `<iframe sandbox="allow-scripts">`:

- JavaScript runs. Full DOM, events, drag-and-drop, keyboard, canvas, WebGL.
- No `allow-same-origin` — can't access client storage or APIs.
- No `allow-top-navigation` — can't navigate the client.
- No `allow-popups` — can't open windows.
- No network. CSP: `default-src 'self' blob: data:; connect-src 'none'`. No phone home, no exfiltrate, no external scripts.

Same model as Stripe payment forms, YouTube embeds. Browser-enforced, battle-tested.
postMessage Protocol

Panel → Client:

    // Trigger an affordance
    parent.postMessage({
      type: "affordance",
      verb: "unlock_skill",
      params: { skill: "fireball" },
      method: "<leden_method_ref>"
    }, "*")

Client → Panel:

    // State update (relayed from observation stream)
    panel.postMessage({
      type: "state",
      data: { skills: [...], xp: 450, level: 7 }
    }, "*")

    // Theme tokens (palette changes, mood shifts)
    panel.postMessage({
      type: "theme",
      tokens: { "color.primary": "#2a1a0e" }
    }, "*")

    // Affordance result
    panel.postMessage({
      type: "result",
      verb: "unlock_skill",
      success: true,
      data: { new_skill: "fireball" }
    }, "*")

The domain decides what state each panel needs. When observation deltas arrive that are relevant to a panel, the client relays them. The panel doesn't subscribe to entities — the domain pushes what it knows the panel needs. Simple.

For panels that don't need JavaScript — simple stat displays, read-only information — the `data-gdl-verb` click convention works as a low-complexity alternative:

    <button data-gdl-verb="unlock_skill"
            data-gdl-param-skill="fireball"
            data-gdl-method="<leden_method_ref>">
      Learn Fireball
    </button>

A text client renders the fallback string. A web client renders the full app. A native client embeds a webview or falls back to text. Progressive enhancement.

Panels are not entities. They don't have positions, affordances, or appearance. They're a parallel content channel for structured information outside the spatial world. A domain can send zero panels (pure world interaction) or many (complex RPG, card game, dashboard).
Theme

Regions carry a theme field for visual identity — the domain's way of saying "this place should feel like this." The full theme system is specified separately in [GDL-style.md](GDL-style.md), the same way CSS is a separate spec from HTML. They evolve independently: GDL's structure is stable, styling evolves fast. A GDL implementation is complete without GDL-style — it just uses client defaults.

Brief summary of what GDL-style provides:

- Design tokens. Flat key-value pairs (`color.primary: #2a1a0e`, `atmosphere.fog_density: 0.4`, `entity.hostile_tint: #ff2200`) that every client type can map to its rendering system. Text clients map colors to ANSI. 3D clients map atmosphere tokens to shaders. No selector syntax, no specificity bugs.

- Structured hints. Coarse mood signals (`mood: gritty`, `epoch: medieval`, `saturation: low`) for clients that don't want to parse individual tokens. A simple client picks a preset from mood + epoch.

- CSS stylesheet. Content-addressed CSS blob for panel styling and web client UI theming. Domain stylesheets reference tokens via CSS custom properties (`var(--gdl-color-primary)`). Walking through a portal shifts the entire client's UI palette.

- Three-level cascade. Domain → region → entity. Domain is the brand. Region is the scene. Entity is the individual. Last writer wins.

See [GDL-style.md](GDL-style.md) for the full design: token categories, cascade rules, stylesheet constraints, security model, and per-client-type consumption examples.
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
  media_codecs: [opus, vp9]
  spatial_preference: grid_2d

Field	Purpose
rendering	What rendering modes the client supports (ordered by preference)
max_entities	How many entities the client can handle at once
asset_formats	What asset formats the client can load
interaction	Input methods available
audio	Whether the client can play audio
media_codecs	Audio/video codecs the client supports (for media streams)
spatial_preference	Preferred spatial model (domain may override)
panels	Whether the client can render sandboxed web panels (bool)
immersive	VR/AR/XR capabilities (see Immersive Capabilities)

Fidelity fields are extensible — like everything else in GDL. A client that supports physics declares `physics: true`. A client that handles nested spaces declares `nested_spaces: true`. The domain reads what it recognizes and ignores the rest. No fidelity field requires a spec change to add.

The domain uses fidelity to:

    Choose spatial model. If the domain supports multiple layouts, pick the one that matches the client.
    Filter appearance layers. Don't send 3D asset hashes to a text client.
    Limit entity count. Send the most relevant entities within the client's budget. A text client gets the 20 most important things. A 3D client gets 200.
    Pick asset formats. If the client supports glTF, reference glTF assets. If only PNG, reference sprites.
    Pick media codecs. Match the client's codec support for media streams.

Fidelity is a declaration, not a negotiation. The domain reads it and adapts. No back-and-forth. If the domain can't serve the client's capabilities at all (a 3D-only domain with a text-only client), it says so at bootstrap and the client can disconnect gracefully.

For large regions (cities, open worlds), the client also reports its viewport — the spatial area it's currently displaying. The domain uses the viewport to decide which entities to include in the observation stream. A client showing a 20x15 tile area of a 500x500 city only receives entities within (and slightly beyond) that viewport. The client sends viewport updates as it scrolls or moves the camera.

client_viewport:
  center: [120, 85]
  radius: 25

The viewport is a circle (center + radius) regardless of spatial model. The domain sends entities within the radius, plus a buffer for smooth scrolling. Entity enter/exit deltas fire as entities cross the viewport boundary, not the region boundary. This is how GDL scales to large regions without sending 10,000 entities on initial snapshot.
Integration with Leden

GDL is a content schema. Leden is the protocol. Here's how they compose.
Session Setup

    Client connects to domain's bootstrap address (Leden Layer 0-1)
    Client authenticates with the domain's greeter (Leden Layer 2-3)
    Client declares client_fidelity as part of the greeter handshake
    Greeter returns: a capability for the domain's region directory and the player's initial region

The greeter is the only public capability. Everything else flows from it.
Region Entry

    Client receives a region object reference (from greeter, from a portal, from another region)
    Client calls Observe(region_ref) (Leden observation)
    Domain responds with a region snapshot — the full GDL region description
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
bond_add	Full bond description	New bond created
bond_remove	Bond id	Bond destroyed
bond_update	Bond id + changed fields	Bond properties change
ambient_update	Changed ambient fields	Environment changes
panel_update	Panel id + new content hash	Domain UI changes
theme_update	Changed tokens and/or hints	Visual identity changes (see GDL-style)
layer_update	Layer id + changed chunk hashes	Terrain/block modifications
event	Event type + scope + data	Something happened (see Events)

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

What the client predicts is the client's problem. GDL doesn't carry prediction logic — that would violate "description is not behavior." The `predicted` flag is permission: "this action's effect is predictable enough that you should try." A movement affordance is predictable. A "open mysterious chest" affordance is not.

If the server result differs from the prediction, the client snaps to the authoritative state. Smooth reconciliation (interpolation, rollback) is a client rendering concern. The domain sends truth. The client makes it feel good.

This is the same model every multiplayer game uses. The difference is that GDL makes it opt-in per affordance rather than a global client assumption. A domain with deterministic physics marks movement as predicted. A domain with complex server-side logic marks nothing as predicted. The client adapts.
Events

The observation stream carries state — "health IS 30", "position IS [5, 3]." But gards also need happenings — "took 10 damage from Fireball", "Kira says: watch out!", "a door slams shut." These are events: fire-and-forget messages about things that occurred. They're not state. They don't persist in the region snapshot. They happen and they're gone.

Without events, chat is impossible. Combat logs are impossible. Sound triggers, visual effects, announcements — all impossible without hacking them through entity property changes. "The goblin's `last_chat_message` property changed to 'die, intruder'" is not how chat should work.

Events ride alongside observation deltas on the region's observation stream. Same backpressure, same sequencing, same session. No new transport mechanism.

Event:
  type: "chat"
  source: <entity_ref>
  position: [12, 3]
  data:
    message: "Watch your back."

Event:
  type: "damage"
  source: <attacker_ref>
  target: <target_ref>
  position: [8, 5]
  data:
    amount: 10
    element: fire
    skill: "Fireball"

Event:
  type: "sound"
  position: [4, 7]
  data:
    sound: door_slam
    volume: 0.8

Event fields:
Field	Required	Purpose
type	Yes	Event type (from vocabulary or domain-defined)
source	No	Entity that caused the event
target	No	Entity the event happened to
position	No	Where it happened (for spatial rendering)
scope	No	Who should see this (see Event Scope)
data	No	Type-specific payload

Event types are free-form strings, extensible like entity kinds. Unknown types are ignored by clients that don't recognize them. A few core types that clients should handle:

Type	Data	Rendering hint
chat	message	Chat bubble, chat log, speech synthesis
sound	sound, volume	Positional or ambient sound trigger
effect	effect, duration, scale	Particle/visual overlay, transient animation
announce	message, priority	Banner, notification, toast

These four are generic — every gard type might use them. Beyond these, domains define their own: a game adds `damage`, `heal`, `loot`. A collaboration tool adds `edit`, `join`, `leave`. An IoT system adds `alert`, `reading`. The protocol doesn't privilege any event type over another.

Chat is just an event. A client renders chat events however it wants — chat bubbles in 3D, a scrolling log in 2D, inline text in a text client. No special chat protocol, no separate channel, no panel hack. The domain sends a chat event, the client shows it.
Event Scope

Not every event should go to every observer. A whisper is for one player. Party chat is for the party. A nearby sound fades with distance. The `scope` field controls targeting:

Scope	Meaning	Example
region	Everyone in the region (default)	Region announcement, ambient sound
proximity	Entities within range of the event position	Local chat, footstep sounds, explosion
target	Specific entity only	Whisper, personal notification
group	Observers of a Leden object	Party chat, guild chat, team channel

Examples:

Event:
  type: "chat"
  source: <player_ref>
  scope: { type: "proximity", radius: 10 }
  data:
    message: "Anyone nearby?"

Event:
  type: "chat"
  source: <player_ref>
  scope: { type: "target", entity: <other_player_ref> }
  data:
    message: "Meet me at the bridge."

Event:
  type: "chat"
  source: <player_ref>
  scope: { type: "group", ref: <party_object_ref> }
  data:
    message: "Ready to pull?"

Scope is enforced by the domain — the domain only sends the event to qualifying observers. The scope field is a rendering hint so the client knows how to present it: whispers in italic, proximity chat fades with distance, party chat in a separate color. The client doesn't filter — the domain already did.

For **cross-region channels** (guild chat, global announcements), the channel is a Leden object. Members observe it. Events arrive on the channel observation, not the region observation. A guild chat panel subscribes to the guild's Leden object and receives chat events from guild members in any region. This composes from existing Leden observation — no new mechanism.

A text client renders all events as log lines:
```
[Goblin Scout] Watch your back.
* You take 10 fire damage from Fireball *
[LOOT] Picked up: Iron Key
[whisper from Kira] Meet me at the bridge.
[party] Ready to pull?
```

A 3D client renders them as floating text, particles, spatial audio, and toast notifications. Progressive enhancement.
Event Reliability

Events are ephemeral. They're not part of the region snapshot. If the client disconnects and reconnects, any events during the gap are lost. The snapshot resync restores state (entity positions, properties) but not events.

For damage numbers and sound triggers, this is fine — stale events are useless. For chat, losing messages matters. The answer: events for real-time delivery, panels for persistent history. A domain that cares about chat history maintains a log and pushes it to a chat panel. The panel is a web app that receives new messages via postMessage and maintains its own scrollback. Events handle "show this now." Panels handle "show what happened." Same architecture as Discord — real-time WebSocket events plus a REST API for history.

---

Extensions

Everything above is the core — what every GDL client handles. Everything below is optional. A client declares which extensions it supports through fidelity negotiation. A domain that uses extensions works fine with clients that don't support them — the core still renders.
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
type	Yes	Data type: pose_3d, position_3d, position_2d, direction_2d, float, bool, audio, video, bytes
rate	Yes	Maximum update rate the domain accepts (Hz)

The client sends input at the requested rate (or lower if it can't keep up). The domain processes input server-side and publishes the authoritative result to other observers through entity_update deltas. The client that sent the input applies it locally (predicted) and reconciles on the authoritative update.

Input streams are Leden observation in reverse: the client is the publisher, the domain is the observer. They use the same coalescing and backpressure model — if the domain can't keep up, it gets the latest value, not a queue of stale frames.

A VR client with head + two hand tracking sends three pose streams. The domain receives them, validates (prevent teleportation hacks, enforce physics), and fans out the result to other players through the normal observation stream. Other clients see the VR player's avatar moving its head and hands.

A non-VR client with a gamepad sends one position stream (stick movement) and maybe one aim stream (right stick or mouse). A text client sends no input streams — it uses discrete movement affordances. The domain adapts to what the client provides.

Input streams don't replace affordances. Moving around is an input stream. Attacking is an affordance. Aiming is an input stream. Pulling the trigger is an affordance. Streams handle continuous state. Affordances handle discrete events. They compose.

The `audio` and `video` input types carry the client's microphone and camera data. The domain declares what media inputs it accepts:

input_streams:
  - id: voice
    type: audio
    rate: 50         # 50 packets/sec (20ms frames, typical for Opus)
  - id: camera
    type: video
    rate: 30

A VR meeting room declares voice input. A streaming theater declares video input. A text adventure declares neither. The client provides what it can. Codec negotiation happens through fidelity (the client declares supported codecs, the domain picks).

The `bytes` input type is an escape hatch for domain-specific continuous data — drawing strokes, sensor readings, custom controller data. The domain defines the format. The client sends raw bytes at the declared rate. Use this sparingly — typed streams are better when they fit.
Output Streams

Input streams are client→server continuous data. Output streams are the mirror: server→client continuous data on entities. Bone poses, blend shapes, physics-driven transforms, terrain deformation, procedural animation, facial expressions — anything that changes too fast for property deltas.

The observation stream handles discrete state: "animation changed to `attack`", "health is now 25." But a ragdolling body, a cloth simulation, or a motion-captured performance produces continuous transform data that doesn't fit the property delta model. A humanoid skeleton has ~20 bones × 7 floats (position + quaternion) = 140 floats per update. At 30Hz, that's 4200 values per second. Property deltas aren't designed for this.

Output streams are declared on entities:

Entity:
  ref: <leden_object_ref>
  kind: agent
  name: "Dancer Yuki"
  output_streams:
    - id: pose
      type: skeleton_pose    # bone transforms for the entity's skeleton
      rate: 30               # 30Hz updates
    - id: face
      type: blend_shapes     # facial expression blend weights
      rate: 15
    - id: cloth
      type: vertex_deltas    # per-vertex displacement for cloth/hair
      rate: 20

Output stream fields:

Field	Required	Purpose
id	Yes	Stream identifier
type	Yes	Data type (see below)
rate	Yes	Update rate in Hz

Output stream types:

Type	Data	Use case
skeleton_pose	Array of bone transforms [bone_id, x, y, z, qx, qy, qz, qw]	Motion capture, procedural animation, ragdoll, IK
blend_shapes	Array of [shape_name, weight] pairs	Facial expressions, morph targets, deformation
vertex_deltas	Array of [vertex_id, dx, dy, dz]	Cloth simulation, soft body, hair, fluid surface
transform	[x, y, z, qx, qy, qz, qw]	High-frequency single-object movement (smoothly interpolated)
floats	Array of named float values	Generic continuous parameters (gauge needles, dials, procedural shaders)
bytes	Raw bytes	Domain-specific continuous data

The client subscribes to output streams through Leden observation. The domain publishes frames at the declared rate. Same backpressure and coalescing as input streams — if the client can't keep up, it gets the latest frame, not a queue of stale ones. The client renders what it receives; interpolation between frames is a client concern.

Output streams compose with skeletal animation. The `animation` hint tells the client which clip to play from its library. An output stream with `type: skeleton_pose` overrides the clip with live bone data — the domain drives the skeleton directly. This is how motion capture, physics ragdoll, and procedural animation work. When the output stream stops (entity goes back to scripted behavior), the client falls back to clip-based animation from the `animation` state.

A client that doesn't support output streams ignores the `output_streams` field. It plays animation clips from the `animation` hint. The dancer does a canned dance animation instead of the motion-captured performance. Graceful degradation.

For **client-side prediction with continuous physics**: the domain sends physics parameters (gravity, friction) and the client runs local simulation. When the domain's authoritative output stream arrives, the client reconciles. This is the same predict-and-reconcile loop every multiplayer game uses, but expressed through GDL's existing mechanisms: physics parameters describe the rules, output streams carry the authoritative state, and the client interpolates between predictions and corrections.

Media Streams

Input streams are client→server. Media streams are entity→observer: audio, video, or data that an entity publishes for observers to consume.

A bard singing in a tavern. A projector showing a video in a theater. A radio tower broadcasting. An NPC with voice lines. Players talking to each other. These are entities that emit media.

Entity:
  ref: <leden_object_ref>
  kind: creature
  name: "Bard Elara"
  position: [6, 3]
  streams:
    - id: voice
      type: audio
      spatial: true

Entity:
  ref: <leden_object_ref>
  kind: structure
  name: "Projection Screen"
  position: [10, 2]
  streams:
    - id: display
      type: video
      surface: true

Stream fields:
Field	Required	Purpose
id	Yes	Stream identifier
type	Yes	Data type: audio, video, data
spatial	No	Whether the stream is positioned at the entity (3D spatial audio, etc.)
surface	No	Whether the video is projected onto the entity's surface
Direct Streams via Leden Introduction

Voice chat between two players should not route through the domain server. Every audio frame taking a server round trip adds 50-200ms of latency. For VR, for music, for any real-time conversation — unacceptable.

The solution is already in Leden: Introduction. The domain is the signaling server, not the relay.

How voice chat works:

1. Player A and Player B are in the same region
2. The domain decides they should hear each other (proximity, party, whatever policy)
3. The domain introduces A to B's voice stream endpoint via Leden Introduction
4. A and B exchange audio frames directly, peer-to-peer
5. The domain is out of the data path. Audio goes A↔B with no server hop.

The domain keeps control:
- It decides who gets introduced (proximity, capabilities, muting)
- It can revoke the introduction at any time (mute, leave range, leave region)
- It never touches the audio data itself

This is WebRTC's architecture: signaling server sets up the connection, media flows peer-to-peer. But using Leden Introduction instead of ICE/STUN/TURN. Simpler — the session already exists, the protocol already handles introduction, and Leden's NAT traversal (relay through a public endpoint) covers the hard cases.

For **broadcast** (concert, lecture, announcement), peer-to-peer doesn't work — one performer can't maintain 500 direct connections. Broadcast streams go through the domain, which fans out to observers. The domain decides routing: direct introduction for small groups, fan-out for broadcasts.

The domain expresses this through stream capabilities. A performer's stream is observable by the region (broadcast). A player's voice stream is observable only by entities the domain has introduced (direct).

Clients don't need to know the routing. They subscribe to an entity's stream. Whether the audio arrives via direct connection or domain relay is transparent — Leden handles it. The only visible difference is latency.

Pre-recorded audio (a jukebox playing a song) is a content-addressed asset, not a stream. The client fetches it from Leden's content store and plays it locally. Live audio is a stream. GDL describes both — `appearance.assets.sound` for pre-recorded, `streams` for live.

A client that doesn't support media streams ignores the `streams` field entirely. The bard is still there, you just can't hear them sing. Text clients render: "Bard Elara strums a melody on her lute." The description carries the experience for clients that can't play audio.
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

Some domains need clients to run local physics — platformers, racing, VR hand interaction. "Description is not behavior" means GDL doesn't carry physics logic. But physics parameters are description — they describe the physical properties of the space.

Physics parameters are region properties in the `physics.*` namespace:

properties:
  physics.gravity: [0, -9.8, 0]
  physics.drag: 0.01
  physics.move_speed: 5.0
  physics.jump_velocity: 8.0
  physics.friction: 0.3
  physics.collision: layers    # collide against spatial layers

Common physics properties:
Property	Purpose
physics.gravity	Gravitational acceleration vector
physics.drag	Air/fluid resistance factor
physics.move_speed	Base movement speed (domain-defined units)
physics.jump_velocity	Initial jump velocity (0 = no jumping)
physics.friction	Surface friction coefficient
physics.collision	What the player collides with: layers, entities, both, none

These are parameters, not a physics engine. The client plugs them into whatever physics system it uses. The domain provides the constants. The client provides the simulation. The domain validates the result. A domain that doesn't need physics simply doesn't set any `physics.*` properties.

Entities that participate in physics carry physics-relevant properties:

properties:
  solid: true
  mass: 5.0
  friction: 0.8       # surface override
  bouncy: 0.3
  kinematic: true      # moves but isn't pushed by others

A VR client uses physics parameters + spatial layers to simulate hand interaction locally: the hand collides with objects, objects have mass and friction, the client predicts the physical result and sends it to the domain for validation. Without physics parameters, VR interaction would require a server round-trip for every hand movement against every object. That's 200ms input lag on touching a table. Unacceptable.

A text client ignores physics parameters. A 2D client might use gravity + friction for simple character movement. A 3D client uses the full set. A VR client adds hand physics on top. Progressive enhancement.
Nested Spaces

A ship on an ocean. A building in a city. A chest in a dungeon room. These are entities that contain other entities in their own spatial coordinate system. Without nested spaces, entering a ship means a region transition through a portal — you can't see the ship's deck and the ocean simultaneously. That's wrong for any game where vehicles, buildings, or containers have interiors visible from outside.

An entity can declare a `space` field — an interior spatial model that contains other entities:

Entity:
  ref: <leden_object_ref>
  kind: vehicle
  name: "The Wavecutter"
  position: [150, 80]
  space:
    spatial: grid_2d { width: 8, height: 4 }
    entities:
      - ref: <crew_ref>
        kind: creature
        name: "First Mate Bjorn"
        position: [2, 1]      # relative to the ship
      - ref: <helm_ref>
        kind: structure
        name: "Ship's Wheel"
        position: [7, 2]

Entities inside a sub-space have positions relative to the containing entity. When the ship at [150, 80] moves to [151, 80], every entity inside moves with it — the domain doesn't update each one individually.

The sub-space's spatial model defines a coordinate system with an origin at the containing entity's position. A `grid_2d { width: 8, height: 4 }` sub-space inside a ship at [150.5, 80.3] maps grid cell [2, 1] to absolute position [150.5 + 2, 80.3 + 1] = [152.5, 81.3]. Grid cells are 1 unit in the parent's coordinate system. If the sub-space needs a different scale, it declares `scale: 0.5` — grid cell [2, 1] maps to [150.5 + 1.0, 80.3 + 0.5].

Sub-spaces can nest. A ship contains a cargo hold. The cargo hold contains crates. Positions compose up the chain: crate position is relative to hold, hold is relative to ship, ship is absolute in the region.

A client that doesn't understand sub-spaces treats the ship as an opaque entity — it renders the ship but not its interior. A capable client renders both: the ship sailing across the ocean, and the crew walking around the deck. Same progressive enhancement as everything else.

Sub-spaces are not regions. There's no region transition to enter a sub-space. The entities inside are part of the same observation stream as the containing region. When a player boards the ship, their entity moves from the region's coordinate system into the ship's sub-space — an `entity_update` that changes their position and adds a `space_parent` reference, not a portal transition.

Entity entering a sub-space:
  entity_update:
    ref: <player_ref>
    position: [3, 1]           # now relative to ship
    space_parent: <ship_ref>   # inside the ship's space

Entity leaving a sub-space:
  entity_update:
    ref: <player_ref>
    position: [150, 81]        # back to region coordinates
    space_parent: null

Observation still works per-region. Sub-space entities are part of the region's entity set. The domain decides which sub-space entities to include based on relevance — a ship on the far side of the ocean might only send the ship entity, not its 20 crew members. A ship the player is standing on sends everything. This is viewport filtering applied to sub-spaces.

When to use sub-spaces vs portals: Sub-spaces are for containers where inside and outside coexist visually — vehicles, open buildings, transparent containers. Portals are for transitions where inside and outside are different contexts — entering a dungeon, teleporting to another region, walking through a door into a separate interior. If you can see both sides at once, sub-space. If you transition between contexts, portal.
Reference Frames

A player standing on a moving platform. A bird perched on a ship's mast. An arrow embedded in a creature. These entities are attached to another entity — their position is relative to it — but they're not inside a sub-space. They're visible in the region, not contained in an interior.

Reference frames handle attachment without containment:

Entity:
  ref: <player_ref>
  kind: creature
  name: "Player"
  position: [2, 0]
  frame: <platform_ref>

The player's position [2, 0] is relative to the platform entity. When the platform moves from [10, 5] to [12, 5], the player's absolute position changes from [12, 5] to [14, 5] — without a position update on the player. The client resolves the absolute position: entity_position + frame_position.

Reference frame fields on an entity:
Field	Required	Purpose
frame	No	Entity ref this entity is attached to. Null = positioned in the region directly.

Reference frames compose with sub-spaces: an entity inside a sub-space is implicitly in the containing entity's frame. The `frame` field is for entities that are ON something without being INSIDE it — a player riding on top of a moving platform, not inside the platform's interior.

The domain sets the `frame` field via `entity_update` when an entity steps onto a platform, mounts a vehicle, or gets picked up. The domain clears it when the entity dismounts. The client handles the position math — frame changes are rare, position updates within the frame are the same as any other movement.

A text client ignores reference frames — it lists the entity wherever it is. A graphical client resolves the frame chain and renders at the computed absolute position. A physics client uses the frame for local simulation — the player's movement is relative to the platform, not the world.
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
- Send comfort metadata. The region includes comfort hints as properties:

properties:
  comfort.locomotion: [teleport, smooth, snap_turn]
  comfort.vignette_on_move: true
  comfort.seated_mode: supported

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

Immersive clients are just clients. They render GDL regions, observe entities, call affordance methods. The immersive extensions (input streams, proximity mode, physics, comfort, haptics) are all progressive enhancements. A domain that sends them works fine with a non-immersive client — the extensions are ignored. A VR client connecting to a non-immersive domain works fine too — it uses standard 3D rendering and falls back to menu-based affordances.
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

Uses: everything above + position, shape, palette, ambient
3D Client

Builds a tavern interior from built-in assets (shape hints for walls, bar, stools). Barkeep Marta is `skeleton: humanoid` + `animation: idle` + `parts: [apron, cloth_shirt]` + `surface.base: cloth` — the client assembles a humanoid from base mesh + parts, plays the idle animation from its library, applies cloth material with the palette colors. Candlelight entity has `effects: [{type: fire, scale: 0.3}]` — the client runs a small fire particle system. Custom model for Marta via appearance.assets.model overrides the procedural assembly if the domain provides one. Spatial audio for tavern_murmur ambient. Player right-clicks Marta to see affordances in a radial menu.

Uses: everything above + skeleton, animation, parts, surface, effects, appearance.assets, spatial audio

VR Client

Same tavern, but you're standing in it. Head tracking renders the scene at 90Hz from your eye position. Barkeep Marta's humanoid skeleton drives a full animation blend — her idle animation shifts weight, blinks, polishes a glass. The `surface.roughness: 0.7` on her apron catches the candlelight with soft diffuse reflection. Reaching toward the Dusty Bottle triggers its proximity affordance — your hand enters the 0.3m grab range and the client highlights it. Squeeze to grab (affordance call with predicted: true), the bottle follows your hand locally while the server confirms. Spatial audio: Marta's voice stream positioned at her location. Candlelight effects render as volumetric particles with the fire palette. Another player says "nice place" — a chat event renders as a speech bubble above their head. Your voice input stream carries your response back.

Uses: everything above + orientation, input streams (head, hands, voice), media streams (Marta's voice), events (chat, sound), physics parameters, proximity affordances, haptic hints, comfort settings

Same GDL payload. Zero domain-specific client code.
The Vocabulary

GDL defines mechanisms (kinds, shapes, materials, categories). The initial terms are listed above in their respective sections. The vocabulary is extensible without protocol changes — new terms are just new strings. Clients that don't recognize a term fall back to the category or ignore it.

Over time, commonly-used terms will become de facto standards. When 200 gards all use shape: humanoid, that's a standard. No committee needed. The same way HTML elements standardized through browser adoption, not W3C edicts (the edicts came after).

Gard unions (from the Allgard federation model) accelerate vocabulary convergence. A union of 50 gards that all agree on the same entity types, appearance hints, and affordance verbs creates a pocket of perfect interop. GDL doesn't need to know unions exist — it just sees consistent vocabulary use.
What This Doesn't Cover

Client UI chrome. GDL describes the world and domain panels, not the client's own interface. Health bars, minimaps, hotkey bindings, settings screens — these are client concerns. The client builds its chrome from entity data (health from properties, minimap from region layout) and its own preferences. Domain-specific UI (skill trees, crafting grids) goes through panels.

Physics simulation. GDL provides physics parameters (gravity, friction, collision rules) and spatial layers (collision geometry). The client runs local physics against these. But the physics engine itself is the client's choice — GDL doesn't specify simulation algorithms, integrator types, or solver iterations. Two clients simulating the same parameters may produce slightly different results. The domain is authoritative; clients predict and reconcile.

Animation blending. GDL describes skeleton types, animation states (including layered body-region animation), custom animation assets, and output streams for continuous bone data. How the client transitions between states — blend trees, crossfade duration, IK solvers — is the client's choice. Two clients playing the same `walk` → `attack` transition may blend differently. GDL describes which state to be in, not how to get there. Output streams carry authoritative bone transforms when the domain drives the skeleton directly (ragdoll, motion capture, procedural).

Audio design. GDL carries theme tokens for atmosphere, sound asset references, media streams, and sound events. Spatial audio mixing, music systems, and sound design are client-side. The domain says "there's a fire here" and optionally publishes a crackling audio stream. The client decides the mix.

Scripting. No behavior in the description. Ever. Raido handles scripting. GDL handles description. The boundary is load-bearing.

Entity internals. GDL describes what an entity looks like from outside. Its internal state machine, its Raido scripts, its capabilities graph — all opaque. The domain exposes what it wants through properties and affordances.

Data validation. GDL doesn't specify validation rules. A domain might send `health: 50, health_max: 30` or a position outside the region's bounds. Domains are responsible for consistency. Clients should be tolerant — display what you can, clamp out-of-bounds values, don't crash on contradictions. Postel's law: be conservative in what you send, liberal in what you accept.

Panel security. Panels are sandboxed web apps: `<iframe sandbox="allow-scripts">` without `allow-same-origin`. CSP blocks all network access. No phone home, no exfiltration, no external scripts. The panel talks only to the client via `postMessage`, and the client validates every affordance call through Leden. A malicious domain can run whatever JS it wants inside the sandbox — it can't escape. For stylesheet security, see [GDL-style.md](GDL-style.md).
Resolved

Regions are not entities. A region is a container. Entities are contents. Regions have metadata (name, description, spatial model, properties, theme). Entities have affordances and appearance. Mixing them creates ambiguity about what "observing an entity" means vs. "observing a region." Clean separation.

Vocabulary is convention, not protocol. Entity kinds, affordance categories, property names, event types, appearance hints — these are initial conventions that communities extend. The protocol defines mechanisms (entities have a `kind` string, affordances have a `category` string). The specific terms (`agent`, `navigate`, `health`) are conventions that emerge from use. Like HTTP content types — `text/html` is not in the HTTP spec. GDL's `creature` kind should not be in the GDL spec either. I kept a small core set for bootstrapping, but they're conventions, not protocol.

Skeletons and animation are description, not behavior. A skeleton is a named rig type — it describes the bone structure an entity has, not how to animate it. Animation states describe what the entity is doing (`walk`, `attack`), not how the client transitions between states (blend trees, crossfade curves, IK). The domain sends state names. The client maps them to its animation library. This keeps the "description is not rendering" boundary clean while giving 3D clients enough information to produce smooth, skeleton-driven animation. The alternative — sending only `posture` strings — meant capable clients had nothing to work with and every goblin was a static posed mesh.

Modular parts over monolithic models. An entity's visual is assembled from a base shape + attached parts, not selected from a catalog of complete models. `shape: humanoid` + `parts: [plate_torso, horned_helmet]` produces a unique-looking entity from reusable components. This is how character creators, Roblox avatars, and modular asset pipelines work. It gives domains combinatorial variety from a bounded part vocabulary without requiring unique models per entity. Parts can be simple names (client resolves attachment) or structured with explicit attachment points and per-part surface properties — a knight's metal armor and cloth undergarments need different material properties. Custom models (Layer 3) still work for domains that want total control — parts are the middle ground between "tinted base mesh" and "full custom asset."

Animation layers over single state. A single `animation: attack` can't express "attacking while running" — real animation is layered, with different body regions doing different things simultaneously. Animation states can be a single string (simple case, drives the whole skeleton) or a list of `{state, layer}` pairs. Layer names are skeleton-dependent conventions (`upper`, `lower`, `head` for humanoids). This is how game engines work internally — GDL exposes the same model. Simple clients ignore layers and play the first state. Capable clients blend across body regions. The single-string shorthand keeps the simple case simple.

Custom skeletons for novel creatures. Base skeleton types (`humanoid`, `quadruped`) cover common cases. But a centaur, a six-armed deity, or any novel creature needs a custom rig. Layer 3 assets include an optional `skeleton` asset — a glTF file defining the bone hierarchy and attachment points. Animation and part assets compose with custom skeletons the same way they compose with base skeletons. A domain that invents a new creature type ships the full asset package (rig + animations + parts); a domain using standard creatures uses the base library. Unknown skeleton names in Layer 2 fall back to shape-based rendering — graceful degradation, not failure.

PBR surface hints over material keywords. `material: metal` told the client what *category* of surface but nothing about its properties. Metal can be polished chrome or rusty iron — wildly different rendering. The `surface` namespace adds PBR-compatible hints (roughness, metallic, emissive, opacity) that 3D clients can map directly to shader uniforms. The `surface.base` keyword preserves the old behavior for simple clients. This isn't specifying rendering — it's describing the physical properties of a surface, which is description.

Structured effects over flat emitting strings. `emitting: fire` was insufficient — where on the entity? How big? What color? Effects are now a list of typed, anchored, parameterized descriptions. An effect anchored to `right_hand` with a fire palette is precise enough for a 3D client to place a particle system, and simple enough for a text client to print "flames lick from the goblin's hand." The vocabulary of effect types is extensible like everything else — unknown types are ignored.

Bonds are not entities. A rope between two entities is a visual relationship, not an independent thing. It has no affordances, no owner, no identity beyond its endpoints. Making it an entity would mean it needs a kind, properties, an existence lifecycle separate from its endpoints — all wrong. Bonds are a distinct concept: typed, visual connections between entities that the client renders based on endpoint positions. They live in the region alongside entities and update through the same observation stream. When either endpoint moves or is removed, the bond updates or disappears.

Appearance references for shared definitions. A forest of 500 trees shouldn't send the same appearance block 500 times. `appearance_ref` points to a content-addressed appearance definition. Entities reference it by hash. The client fetches it once, caches it, and applies it to every entity with the same ref. Inline appearance fields override the reference (merge, not replace). When the client sees many entities with the same `appearance_ref` and no overrides, it can instance them — one draw call, many transforms. The content hash makes instancing trivial to detect.

Assets are the primary path, hints are fallback. The old spec framed custom assets as "Layer 3" on top of hints — implying hints were the normal path and assets were the premium feature. That's backwards. Domains that invest in their world create content — custom models, animations, effects. Hints exist for prototyping, graceful degradation when assets fail to load, and lightweight domains. The appearance system supports both, but the spec should be clear: the path to a good-looking world is content creation, not hope that hint-driven procedural assembly looks good enough.

Output streams for continuous server→client data. The observation stream handles discrete state changes. Output streams handle continuous high-frequency data — bone poses, blend shapes, vertex deformation, physics state. They're the mirror of input streams (client→server). Same backpressure, same coalescing (latest frame wins). Output streams compose with animation: the `animation` hint drives clip playback, an output stream overrides it with live data. When the stream stops, the client falls back to clips. This keeps the observation model clean — property deltas for state, output streams for continuous data, events for happenings. Three channels, three update patterns, no mixing.

Region schema is minimal. Regions have: name, description, spatial model, extensible properties, theme, layers, entities, bonds. Physics parameters, comfort hints, tick rate, and environmental description all live in properties or theme tokens — not as first-class fields. This keeps the region schema stable as new use cases emerge. A VR fitness app adds `comfort.*` properties. A music visualizer adds `audio.*` properties. Neither requires a spec change.

Portals are entities (kind=portal) that reference target regions. Navigation is: entity (portal) → region → entities. This gives you cross-region links (like HTML hyperlinks) without nesting regions inside regions.

One spatial model per region. A region doesn't present itself differently to different clients. It has one spatial model. Clients adapt. A text client can render continuous_3d as a list — it just ignores coordinates. The alternative (per-client spatial models) requires the domain to maintain multiple representations, which doesn't scale.

Affordances over methods. The client doesn't call entity methods directly. It discovers affordances, which contain method references. This indirection is the key to client-domain decoupling. A domain can change its internal method structure without breaking clients — it just updates the affordance's method field. Clients never hardcode method names.

No inheritance in the entity model. USD and Roblox use class hierarchies. ECS uses composition. GDL uses composition — an entity is a bag of kind + properties + affordances + appearance. No "class GenericSword with subclass Flamebrand." Inheritance creates coupling between entity definitions that breaks across domain boundaries. Composition lets two domains agree on individual properties without agreeing on a type hierarchy.

Content-addressed assets, not URLs. Assets are identified by content hash, not location. This means: deduplication across domains is free, integrity verification is free, and caching is trivial. Two domains that independently use the same goblin sprite share the content hash. The client fetches it once. This falls directly out of Leden's content store.

Entity data relationships are properties. `equipped_by: <ref>`, `contained_in: <ref>`, `group: <ref>`. Properties handle the common cases (equipment, containment, grouping). Visual relationships between entities (ropes, chains, beams) are bonds — a first-class concept with their own rendering and observation updates. Data relationships remain properties because they don't have visual representation or lifecycle concerns. If data relationships prove too limiting for complex entity graphs, they can be promoted to a first-class concept later.

Portal transitions are domain-controlled. Portals carry a `transition` hint: `instant` (default), `fade`, `walk`, or `loading`. The client renders what it can — a text client ignores transitions entirely, a graphical client uses the hint to drive its transition animation. The domain decides the experience; the client decides the presentation. Without this, every client guesses differently and cross-domain travel feels jarring.

Observation has three tiers. Region observation for structural changes (entity add/remove). Region-level property filter for bulk streaming (`Observe(region_ref, entity_filter: [position])` gives position updates for all entities as one subscription). Individual entity observation for detailed per-entity tracking. This avoids the 500-subscriptions problem without changing Leden's observation model — region-level filters are just a filtered view over the region's delta stream.

Domain-specific UI uses sandboxed web panels. Panels are long-lived web apps, not re-rendered blobs. The content hash is the application code — loaded once. Data changes arrive through postMessage from the client. `panel_update` means the app was redeployed, not that data changed. JavaScript runs inside `<iframe sandbox="allow-scripts">` — the browser enforces the security boundary. postMessage replaces fetch/WebSocket. Everything else is normal web development. See the Panels section above.

Media streams are peer-to-peer via Leden Introduction. The domain is the signaling server, not the relay. For voice between two players: the domain introduces them, audio flows directly, the domain can revoke to mute. Broadcast (concerts, lectures) goes through the domain for fan-out. This is WebRTC's architecture — signaling server + peer-to-peer — using Leden Introduction instead of ICE/STUN/TURN. The domain keeps control (it decides who gets introduced) without sitting in the data path.

Event targeting via scope. Events carry a `scope` field: region (default, everyone), proximity (within radius), target (one entity, whisper), group (Leden object observers, party/guild chat). Cross-region channels are Leden objects that members observe — guild chat events arrive on the guild observation, not the region observation. The domain enforces scope server-side. The scope field is a rendering hint for the client.

Unbounded spatial models. Bounds are optional on all spatial models. Omitting bounds means the world extends indefinitely — the domain generates content around the client's viewport. Infinite procedural terrain, endless oceans, fractal explorers — all first-class. The viewport mechanism handles content delivery: the client reports where it's looking, the domain generates around it. Bounded worlds declare extent upfront. Unbounded worlds are discovered by moving through them.

Events alongside observation. The observation stream carries both state updates (entity_update, entity_enter, etc.) and events (chat, damage, sound triggers). State is durable — it persists in the snapshot. Events are ephemeral — they happen and they're gone. Chat is an event, not a property change. This was originally deferred ("panels can show a combat log"), but that was a hack. Events are a first-class concept in the observation stream.

Media streams on entities. Entities can publish audio, video, or data streams that clients subscribe to. Voice chat, live performances, video projection, data feeds — all described as entity streams, transported through Leden. Input streams get audio/video/bytes types for microphone, camera, and custom data. The domain is authoritative over who hears what — it controls stream observation capabilities.

Nested spaces over portals-only. Entities can contain sub-spaces — interior spatial models with their own entities and coordinates. A ship on an ocean, a building in a city, a chest with contents. Positions inside a sub-space are relative to the containing entity. No region transition required. Inside and outside coexist in the same observation stream. Portals remain for context transitions (entering a dungeon, teleporting). Sub-spaces handle spatial containment (vehicles, buildings, containers).

Reference frames for attachment. Entities can declare a `frame` — a reference entity their position is relative to. A player on a moving platform, a bird on a mast, an arrow in a creature. The frame entity moves, the attached entity moves with it. No per-frame position updates for every passenger. The client resolves absolute positions by composing frame transforms.

Visual identity is a separate spec (GDL-style). Design tokens for world styling, CSS stylesheets for panels and web UI, three-level cascade (domain → region → entity). Separated from GDL because styling evolves faster than structure and has a different implementer audience. A GDL implementation is complete without GDL-style. See [GDL-style.md](GDL-style.md).

VR/AR/XR is supported through general-purpose extensions, not a VR-specific protocol. Entity orientation, input streams (continuous client→server data), proximity affordances, spatial layers (dense geometry), physics parameters, and immersive fidelity fields. All are progressive enhancements — a VR client connecting to a non-VR domain works fine (menu affordances, no hand physics), and a non-VR client connecting to a VR domain works fine (ignores input stream endpoints, uses instant/targeted affordances). The immersive capabilities are the same mechanisms needed for platformers, racing games, and any real-time physics game.

Dense worlds use spatial layers. Tilemaps, voxel chunks, heightmaps, and collision meshes sit alongside entities in a region. Entities are sparse (things you interact with). Layers are dense (the world itself). A Minecraft chunk is a voxel layer. A platformer level is a mesh_2d layer. A terrain system is a heightmap layer. Layers are content-addressed and chunked for viewport-based streaming.

Large regions use viewport filtering. The client reports its viewport (center + radius), and the domain only sends entities within that area. Entity enter/exit deltas fire at the viewport boundary. This scales GDL to open-world regions without dumping 10,000 entities on initial snapshot.
Deferred

    Wire format. Binary vs text. Depends on Leden's wire format decision. GDL is a schema — the encoding is separate.
    Vocabulary registry. A formal list of kinds, shapes, materials, and categories with semantic definitions. Needed before implementation, not before design.
    LOD (Level of Detail). Distant entities could be sent with less detail. The mechanism exists (fidelity negotiation + filtered observation), but the specific LOD policy is implementation-level.
    Accessibility. Screen reader hints, colorblind palettes, motor-impairment interaction modes. Important, but a layer on top of the base protocol, not a change to it.
    Versioning. GDL will evolve. Version negotiation should follow Leden's model (version handshake at session start, backward-compatible additions don't require version bumps). Details after v1 is stable.
    Entity visibility. Fog of war needs a visibility field on entities: visible, last_known (stale data with timestamp), hidden. The domain controls which entities the client knows about. last_known entities carry stale data that the client renders differently (grayed out, question mark). Deferred because most use cases don't need fog of war, and the viewport filtering mechanism handles the common case of "don't show what's far away."
    Time-sequenced content. Rhythm games and cutscenes need pre-loaded event sequences with precise timestamps. The observation model is push-based (server sends updates as they happen), not time-indexed. This is a fundamentally different content type — probably a separate spec rather than a GDL extension. Deferred.
    Media stream transport details. GDL describes what media streams exist (type, spatial, surface). The actual transport — codec negotiation, packet framing, jitter buffers, bandwidth estimation — is Leden's concern. Leden's content store explicitly notes "live audio/video is not content-addressed" as a gap. That gap needs filling at the Leden layer, not in GDL.
    Sub-space observation granularity. Currently, sub-space entities are part of the containing region's observation stream. For very large sub-spaces (a carrier with 500 rooms), this might need its own observation scope — observe the sub-space independently of the region. Deferred until someone actually needs a 500-room ship.
