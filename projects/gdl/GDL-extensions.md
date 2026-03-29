GDL Extensions
<!-- id: gdl.extensions --> <!-- status: proposed --> <!-- summary: Optional extensions to GDL — streams, spatial layers, physics, nested spaces, immersive -->

Extensions are optional capabilities that clients negotiate through [fidelity](GDL.md#fidelity-negotiation). A domain that uses extensions works fine with clients that don't support them — the [core](GDL.md) still renders.

Each extension is independent. A 2D tile client adds spatial layers. A VR client adds input streams, output streams, and media streams. A client with vehicles adds nested spaces. None require the others. A client that doesn't understand an extension ignores it.

## Specs

| Extension | What |
|-----------|------|
| [Input Streams](#input-streams) | Continuous client→server data (movement, tracking, media input) |
| [Output Streams](#output-streams) | Continuous server→client data (bone poses, blend shapes, physics, deformation) |
| [Media Streams](#media-streams) | Audio/video from entities (voice, live performance, video) |
| [Spatial Layers](#spatial-layers) | Dense data (voxels, heightmaps, tilemaps) |
| [Physics Parameters](#physics-parameters) | Client-side simulation constants |
| [Acoustic Environment](#acoustic-environment) | Physical acoustic properties for spatial audio |
| [Nested Spaces](#nested-spaces) | Sub-spaces in entities, relative positioning |
| [Reference Frames](#reference-frames) | Attachment without containment |
| [Immersive Capabilities](#immersive-capabilities) | VR/AR/XR support |

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

Output streams compose with skeletal animation. The animation state tells the client which clip to play from the scene model. An output stream with `type: skeleton_pose` overrides the clip with live bone data — the domain drives the skeleton directly. This is how motion capture, physics ragdoll, and procedural animation work. When the output stream stops (entity goes back to scripted behavior), the client falls back to clip-based animation from the scene model's animation library.

A client that doesn't support output streams ignores the `output_streams` field. It plays animation clips from the scene model based on the animation state. The dancer does a canned dance animation instead of the motion-captured performance. Graceful degradation.

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

Pre-recorded audio (a jukebox playing a song) is a content-addressed asset, not a stream. The client fetches it from Leden's content store and plays it locally. Live audio is a stream. GDL describes both — `appearance.sound` for pre-recorded, `streams` for live.

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

### Binary Formats

Layer data blobs are content-addressed, like assets. Each type has a defined binary encoding so clients can parse them without format negotiation.

**Heightmap:**
```
Header:
  width  (u16)  — columns
  height (u16)  — rows
  scale  (f32)  — meters per cell (horizontal)
  y_scale (f32) — meters per unit of elevation
Data:
  elevations[width * height] (each f32, row-major)
```
Total size: 12 + (width × height × 4) bytes. A 256×256 heightmap is ~256 KB.

**Tilemap 2D:**
```
Header:
  width      (u16) — columns
  height     (u16) — rows
  tile_bits  (u8)  — bits per tile ID (8 or 16)
  layer_count (u8) — number of tile layers (ground, decoration, etc.)
  _pad       (2 bytes)
Palette ref:
  palette    (32 bytes) — SHA-256 hash of tile palette definition
Data (per layer):
  tiles[width * height] (each u8 or u16 per tile_bits, row-major)
```
Tile ID 0 is always empty/transparent. The palette blob maps tile IDs to names, collision flags, and animation frames — a separate content-addressed asset that multiple tilemaps can share.

**Voxel 3D (chunked):**
```
Chunk header:
  cx, cy, cz (each i16) — chunk coordinates in chunk-space
  size       (u8)       — cells per edge (typically 16)
  _pad       (1 byte)
  palette    (32 bytes) — SHA-256 hash of block palette definition
Data:
  blocks[size^3] (each u16, XZY order — x varies fastest, then z, then y)
```
Block ID 0 is always air. XZY order optimizes for column-based rendering (vertical slices are contiguous). Each chunk is a separate content-addressed blob. A 16³ chunk with u16 blocks is 8 KB + 40 bytes header.

**Mesh 2D:**
```
Header:
  vertex_count   (u16)
  segment_count  (u16)
Vertices:
  positions[vertex_count] (each 2 × f32 — x, y)
Segments:
  indices[segment_count] (each 2 × u16 — start vertex, end vertex)
  flags[segment_count]   (each u8 — 0x01: one-way-up, 0x02: one-way-down)
```
Defines 2D collision geometry as line segments. One-way flags enable platforms you can jump through from below.

**Mesh 3D and Navmesh** use standard glTF binary format (`.glb`) — no custom encoding. glTF is already GPU-ready and well-supported. The content-addressed blob is the `.glb` file.

All integer fields are little-endian. All float fields are IEEE 754 little-endian. Layer blobs carry no version field — the layer `type` in the region description implies the format. If the format ever changes, it becomes a new type name.

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
Acoustic Environment

Sound is half the immersion and the biggest gap in most world description formats. The temptation is to specify reverb presets, attenuation curves, HRTF profiles, and DSP parameters. That's the path to a 2026-era spec that ages like VRML's AudioClip node.

The principle: **describe the physical acoustic properties of spaces and surfaces. Let clients derive the audio processing.** Absorption coefficients, room volume, and surface materials are physics — they don't go out of date. Reverb algorithms, spatial audio engines, and codec implementations change every few years. GDL describes the former. Clients provide the latter.

This is the same pattern as physics parameters (describe gravity and friction, not the integrator) and the same pattern as glTF materials (describe roughness and metallic factor, not the shader).

**Region acoustic properties**

Regions carry acoustic properties in the `acoustic.*` namespace:

properties:
  acoustic.volume: 450.0        # room volume in cubic meters
  acoustic.openness: 0.0        # 0.0 = fully enclosed, 1.0 = open air
  acoustic.absorption: 0.4      # average surface absorption (0.0 = perfect reflection, 1.0 = anechoic)

Three properties. That's it for the region.

Property	Type	Purpose
acoustic.volume	float	Volume of the space in cubic meters. A small room is 30. A cathedral is 20,000. Open air is absent or very large. The client derives reverb time from volume + absorption (Sabine equation or better).
acoustic.openness	float (0–1)	How enclosed the space is. 0.0 = sealed room (full reverb). 1.0 = open field (no reflections). Values between = partial enclosure (covered patio, forest canopy, cave mouth). Clients use this to blend between indoor and outdoor audio models.
acoustic.absorption	float (0–1)	Average acoustic absorption of the space's surfaces. 0.0 = hard reflective surfaces (tile, stone, metal). 1.0 = fully absorptive (recording studio foam, deep snow). Clients use this with volume to derive reverb decay time. A large stone cathedral (volume: 20000, absorption: 0.1) produces long reverb. A carpeted office (volume: 80, absorption: 0.7) produces short, muffled reverb.

Why these three and not more? Because `volume + openness + absorption` is the minimum information a spatial audio engine needs to produce convincing environmental audio. Any modern reverb algorithm (convolution, ray-traced, algorithmic) can take these as inputs. Adding more (surface material breakdown, room dimensions, reflection patterns) would couple GDL to specific propagation models. If a future audio engine needs more detail, it can derive it from the spatial layers (the room geometry is already there in mesh_3d or heightmap data).

A domain that doesn't set acoustic properties gets client defaults. A domain that sets them gets acoustics that match the space. A domain that wants precise acoustic control ships a convolution reverb impulse response as a content-addressed asset in the theme:

theme:
  tokens:
    sound.impulse_response: sha256:abc123...

This is the escape hatch. An impulse response IS the acoustic environment, captured or synthesized. A client that supports convolution reverb uses it directly. A client that doesn't falls back to deriving reverb from the acoustic properties. Progressive enhancement.

**Entity acoustic properties**

Entities that emit or block sound carry acoustic properties:

properties:
  acoustic.occlusion: 0.8     # how much this entity blocks sound (0 = transparent, 1 = solid wall)
  acoustic.emission_radius: 5.0  # how far this entity's sound carries (meters, before falloff)

Property	Type	Purpose
acoustic.occlusion	float (0–1)	How much this entity blocks sound passing through it. A stone wall is 0.95. A curtain is 0.2. A glass window is 0.5. An open doorway is 0.0. Clients use this for occlusion calculations — sound from behind a wall is muffled proportional to occlusion.
acoustic.emission_radius	float	The distance (in region units) at which this entity's sound is at full volume. Beyond this, falloff applies. A whisper is 1.0. A person talking is 5.0. A church bell is 50.0. Clients apply distance falloff beyond this radius using whatever attenuation model they prefer.

That's it. Two properties per entity.

No attenuation curve specification. Distance falloff (inverse square, linear, logarithmic) is a client rendering decision — the same way blend tree transitions are a client animation decision. `emission_radius` tells the client where full volume ends. How fast it falls off from there is the client's audio engine's business. Specifying a falloff curve would couple GDL to a particular spatial audio model. Inverse square is physically correct, but some engines use modified curves for gameplay feel. Let them.

No HRTF specification. Head-related transfer functions are client-side, hardware-dependent, and evolving rapidly (personalized HRTF from ear scans, ML-derived HRTF). GDL provides position and the physical environment. The client's spatial audio engine does the rest.

**Acoustic zones**

Large regions may have different acoustic areas — a cathedral with a side chapel, an outdoor area with a covered walkway. Acoustic zones are entities with `kind: marker` that override the region's acoustic properties within a radius:

Entity:
  ref: <leden_object_ref>
  kind: marker
  name: "Side Chapel"
  position: [15, 8, 3]
  properties:
    acoustic.volume: 80.0
    acoustic.openness: 0.0
    acoustic.absorption: 0.3
    acoustic.radius: 6.0       # zone radius in region units
    acoustic.blend: 2.0        # transition distance (meters) for crossfade

When the listener enters the zone radius, the client blends from the region's acoustic properties to the zone's. `acoustic.blend` controls the crossfade distance — 2.0 means the transition happens over 2 meters. A hard boundary (doorway) uses `blend: 0.5`. A gradual transition (walking from outside into a cave) uses `blend: 5.0`.

Zones are just marker entities with acoustic properties. They arrive and update through the normal observation stream. No new mechanism.

**What this deliberately doesn't cover**

Audio mixing. How loud music is relative to dialogue, how many simultaneous sources to render, whether to duck ambient when combat starts — all client concerns. The domain provides sources and their physical properties. The client mixes.

Music systems. Music cues, crossfades, layered adaptive music, beat-synced transitions — these are domain behavior expressed through events and theme tokens (`sound.music_mood` in GDL-style). Not acoustic environment.

Codec negotiation. What audio format to use for streams — Leden's concern, declared in fidelity. GDL doesn't name codecs.

Propagation simulation. Ray-traced audio, phonon simulation, wave-based diffraction — client-side rendering choices. GDL provides the physical properties. Clients that do ray-traced audio can use spatial layer geometry for reflections. Clients that don't can use the absorption coefficient for a simpler reverb model. Both work from the same data.

A text client ignores all acoustic properties. A 2D client might use `acoustic.openness` to pick between "indoor" and "outdoor" ambient sound mixing. A 3D client derives full spatial reverb from volume + absorption + openness. A VR client adds HRTF spatialization, occlusion ray-casting against spatial layers, and distance-based falloff from emission_radius. Progressive enhancement, as always.
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
