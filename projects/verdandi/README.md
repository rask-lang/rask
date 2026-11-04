<!-- id: verdandi.overview -->
<!-- status: proposed -->
<!-- summary: Verdandi -- a deterministic simulation engine for colony, factory and falling-sand games, with checkpoints, fast-forward and co-op multiplayer from one design. -->

# Verdandi

Working name. Urd keeps the past; Verdandi runs the present tick.

An engine for the games other engines don't serve: colony sims, factory builders, falling sand, tens of thousands of units. They Are Billions, Factorio, Noita, Dwarf Fortress, RimWorld. Those games all ended up on custom engines, or used Unity only to draw, because a scene tree of 200,000 nodes doesn't work.

## The pitch

Players of this genre ask for three things: checkpoints, fast-forward, and multiplayer. They Are Billions players have asked for all three for years. RimWorld and Dwarf Fortress never shipped multiplayer. Fans bolted it on anyway:

- **RimWorld:** the community Multiplayer mod uses lockstep. Its standing problem is desyncs, mostly from mods that aren't multiplayer-safe, and they get worse as the colony grows.
- **Dwarf Fortress:** DFPlex runs one copy of the game and streams each player a screen over a websocket. That's remote desktop, not netcode.

So the demand is proven. What's missing is an engine where the simulation is deterministic by construction rather than patched into shape afterwards.

I think the three wishes are one feature. If the game is a deterministic step over its state and the inputs:

- a checkpoint is a snapshot plus the input log after it;
- fast-forward is running the step without drawing, as fast as the CPU allows;
- multiplayer is every machine running the same steps on the same inputs (lockstep);
- joining a game in progress is all three at once: receive a checkpoint, then fast-forward to catch up.

The game writes `step(state, inputs)`. The engine owns time, inputs, snapshots and the network. Single player is the same engine with one peer, so a game written for it gets multiplayer by changing configuration, not code.

## Decided

- **Lockstep, co-op first.** The state is far too big to send and the inputs are tiny. Every client holds the full state, so map hacks are possible. That's fine for co-op and wrong for competitive play, which isn't the target.
- **No MMO shape.** Partial world state and sharding is a different business.
- **No pause or speed-up in multiplayer.** It doesn't make sense with several players, and few games do it. Checkpoints and fast-forward are single-player features, and the same machinery as joining.
- **Render is a view.** It reads the state between steps, interpolates, and never writes it.
- **Bind the platform, write the opinions.** SDL3 for windows, GPU and audio devices. stb and dr libraries for codecs. The renderer (instanced draws straight from component arrays) and the mixer are written in Rask.

## What it has to solve

**Determinism across machines.** Every source of drift has to be closed at the engine level, not left to game code:
- floats (IEEE basic operations, correctly rounded `sin`/`cos`/`exp`, no fused multiply-add, NaNs canonicalized where their bits show: #1683, on every machine, so Raido can use f64 too: #1735);
- map iteration order;
- parallel scheduling;
- randomness, which lives in the state, seeded per system;
- uninitialised memory;
- mods.

Raido runs mods, because a mod that can't be nondeterministic can't desync.

**Mods are the game.** The engine is a kernel with no content. The game itself is a mod on top, the same way Factorio's base game is a mod called `base`. Whatever a game's own designers can do, a modder can do, because the game used no private door. Mods are Raido, which is what makes the rest of this section possible:
- **Downloading code from a server is safe.** Raido has no I/O; a mod reaches only the externs the engine registers. A server can send a joining player its mods, and running them can't touch the player's machine. Assets are the remaining attack surface: images and sounds from a mod go through C decoders, so those run sandboxed or validated, not trusted.
- **A mod is its hash.** Bytecode is content-addressed, so the mod set is a list of hashes. A client joins only with the exact set and fetches what it's missing from the server, from a local cache first. Mismatched mods, RimWorld multiplayer's main cause of desyncs, can't happen.
- **Mod state is game state.** Raido's VM state is serializable, so checkpoints, saves, replays and the desync hash include every mod's state with no extra work. Running out of fuel mid-tick is deterministic too, the same on every machine.

Two stages, as in Factorio. The data stage runs once at load and declares what exists: entity types, their components, recipes, sprites. The engine lays out its arrays and atlases from that. The control stage runs during play and reacts to events.

Mods split into two kinds:
- **Simulation mods** change the rules. They must match on every machine and are part of the hash.
- **Client mods** change UI, sounds and visuals. They're local, never hashed, and can read the state but never write it, so one player's UI mod can't desync anyone.

The cost is speed. Raido is an interpreter, and 100,000 units each running a script every tick won't fit in the budget. Factorio has the same constraint with Lua: heavy per-tick scripts are what kills its tick rate. So the per-entity hot loops (movement, flow fields, combat resolution, rendering) are engine systems written in Rask. Mods configure them with data and handle events: a unit died, a building finished, a wave starts. A mod that needs a new hot loop gets it by asking for an engine system, not by running per entity per tick. Compiling Raido to native code later would move that line, provided the compiled code keeps the interpreter's exact results.

**Finding a desync.** A hash of the full state every tick is too slow at this size. The state lives in chunks hashed incrementally, as a merkle tree (Urd's store), so the per-tick hash is cheap. When two machines disagree, comparing the trees walks straight to the chunk, then the entity, that drifted. With the seed and the input log, the desync replays on one machine.

**Throughput is correctness.** Lockstep runs at the speed of the slowest machine, and 4x fast-forward costs 4x the CPU. So simulation speed decides whether the features exist at all:
- component arrays, not objects;
- regions that sleep when nothing in them changes, like Noita's dirty rectangles;
- parallel systems with a fixed merge order. Falling sand updates chunks in a checkerboard pattern, as Noita does, which keeps the parallelism deterministic.

This is also where Rask's Cranelift backend gets stress-tested: SIMD, inlining, layout.

**Data layout.** Entities aren't objects. A zombie is an index into arrays: one for positions, one for targets, one for health. A system that moves units streams through the position and target arrays and never touches the rest, so the cache holds only what the loop reads. Two refinements matter at this scale:
- **Group by access, not by field.** Fields always read together (x and y, velocity and position in a movement pass) go in one array of small structs; fields read rarely (name, history, inventory) go in a cold array. Splitting every field apart costs as much as never splitting.
- **The layout serves checkpoints and hashing too.** Arrays chunk naturally, so the same chunks are what copy-on-write copies and what the merkle tree hashes.

Layout alone doesn't make 100,000 units cheap. What they look at does: a spatial grid so a unit only checks its neighbours, and one shared path field per target, a flow field, instead of a path per unit. Those are algorithms, not engine plumbing, but the kernel should ship them, because every game in the genre writes them.

In Rask, an array of small structs is a `Vec` of `@layout(C)` structs, packed at natural width (#985), and the same buffer can go to the GPU as instance data. Splitting a struct into parallel arrays is written by hand today. A struct-of-arrays layout the compiler generates from one declaration would make it read like the object version. Worth a spec once the first game shows how often it's needed.

**Parallelism without losing determinism.** Each tick forks work across cores and joins before the next: one task per region of the map, per horde's flow field, per fog-of-war pass. Work stealing is fine, because which core runs a task can't change its result, under three rules:
- **A task writes only its own slice.** Reads of shared state see last tick's values. Two tasks never write the same array range, so there's nothing to race on and nothing to lock.
- **Cross-task effects merge in a fixed order.** A unit leaving one region for another, or damage from two hordes on one wall, goes into per-task output buffers that are applied after the join in task order, never in completion order. Sums over floats follow the same rule, since float addition isn't associative.
- **Randomness is seeded by (tick, task, entity), never by thread.**

Rask's ownership rules make the first one checkable: a slice handed to a task can't be touched by another. What's missing is the fork-join itself. `spawn` moves its captures into the task, so splitting one `Vec` across cores means moving pieces out and back. Fork-join that can borrow (#1686) and `kernel` blocks where each lane writes only its own slot (#1654) are the language work this needs.

Rendering doesn't join the tick. The simulation runs on its own thread at a fixed rate and publishes each finished state. The renderer draws at the display's rate from the last two published states, interpolating between them. A slow frame never stalls the simulation, and a heavy tick never drops a frame.

**Rendering at this scale.** The CPU should do nothing per entity per frame. Instance buffers come straight from the component arrays, so the GPU gets position, sprite and animation frame for every visible unit in one upload. The vertex shader interpolates between the last two states and picks the animation frame from the tick. The CPU's work per frame is culling by chunk and issuing a handful of draws.

Textures are the other bottleneck, and Factorio's history shows both halves of it:
- **Batching needs atlases.** One draw per texture doesn't scale, so sprites are packed into atlases and grouped by what's drawn together (all belts, all of one horde). A layer then draws in one call per atlas. Draw order (layers, depth within a layer) and atlas grouping pull against each other, so the grouping follows the layers.
- **Atlases outgrow VRAM.** Factorio's sprites needed about 2.4 GB of VRAM by 2018 (FFF #227). Its answer was virtual texturing (FFF #264): the atlas is cut into 128x128 tiles, and only tiles in use live in a smaller physical texture in VRAM, with a lookup table from virtual to physical tile. In 2D the renderer knows exactly which sprites it draws this frame, so it knows which tiles to make resident without a GPU feedback pass. Evict least recently used. Prefetch along the camera's motion, and draw a lower mip of the tile until the full one arrives, so streaming never shows as pop-in.
- **Compressed textures** (BC7 on desktop, ASTC on mobile) cut VRAM and bandwidth by four or more, before any streaming. SDL3's GPU API supports them.

Atlas packing, mip generation and compression happen at build time, in `build.rk`, not at load. Shaders are written once and cross-compiled for Vulkan, Metal and D3D12 with SDL's shader tooling.

**Cheap checkpoints.** Copying a large world every few seconds is too slow. Chunks are copy-on-write, so a checkpoint costs what changed since the last one, not the world size. A save file is a checkpoint plus a log.

**Saves across versions.** A replay needs the exact simulation that recorded it. Factorio's replays break across versions. A save carries the version of its rules, and an old log replays on old rules or not at all. Raido's content-addressed bytecode helps with mods.

**Joining a game in progress.** The joiner downloads a checkpoint while the game keeps running, then simulates faster than real time to catch up. Factorio's catch-up shows this works, and that a slow machine can take a long time to get there.

**Input latency.** In lockstep, an input takes effect a round trip later. That's fine for They Are Billions or Dwarf Fortress, and noticeable for Noita's hands-on action. Rolling back and re-running ticks, as fighting games do, isn't affordable on a million-cell world. The answer is input delay plus cosmetic prediction for the player's own avatar, which Factorio also does. Fast action on a huge world is the genre's hardest case.

**A slow machine.** One player's laptop stalls everyone. The options are adapting the tick rate, letting that player fall behind and catch up, or dropping them. Each costs something.

**Conflicting commands.** Two players order the same dwarf in the same tick. The engine orders inputs within a tick (by player, then sequence), so the result is the same everywhere. What the game does with conflicts is its own design.

## Open

- The transport under Ting: stay on ngtcp2, or move to GameNetworkingSockets, which has relays and NAT traversal? Lockstep bandwidth is trivial, so this is mostly about NAT and relays.
- The first flagship game. Something with many small things following local rules on a grid (falling sand, factory belts), because it hits entity count and determinism on day one.
- Literal-built number types for simulation units, such as `Fixed` and `Meters` (#1682).
- Shared-library output from `rask build`, for a Godot plugin later.

## Related

- [Ting](../ting/README.md): the transport underneath.
- [Urd](../urd/README.md): log, snapshots and a merkle state store, which are the checkpoint machinery.
- [Raido](../raido/README.md): deterministic mods.
