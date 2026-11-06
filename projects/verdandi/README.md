<!-- id: verdandi.overview -->
<!-- status: proposed -->
<!-- summary: Verdandi -- a deterministic simulation kernel for colony, factory, falling-sand and strategy games: checkpoints, fast-forward and co-op multiplayer from one design, and the hard boring problems solved so the game doesn't have to. -->

# Verdandi

Working name. Urd keeps the past; Verdandi runs the present tick.

An engine for the games other engines don't serve: colony sims, factory builders, falling sand, tens of thousands of units, grand strategy, turn-based simulations. They Are Billions, Factorio, Noita, Dwarf Fortress, RimWorld, Paradox's games. Those all ended up on custom engines, or used Unity only to draw, because a scene tree of 200,000 nodes doesn't work.

People write their own engines for two reasons: they need full control, or their game fits nothing that exists. So Verdandi imposes no game design. It solves the hard, boring problems every game in this space solves again (determinism, time, space, saves, the network) and leaves the game to the developer.

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

Determinism also pays for itself in development: rewind debugging is a checkpoint plus the log, a regression test replays an old log and compares hashes, and a rare bug can be found by searching seeds.

## Three layers

| Layer | What's in it | Rule |
|---|---|---|
| Kernel | The step loop; the input log and command ordering; stable entity ids; deterministic collections and random streams; the space partition, sleep and wake; parallel scheduling with a fixed merge order; snapshots, saves with migrations, replays; lockstep networking with joining, desync detection and recovery; the state hash. | Every game needs it, it's hard, and getting it wrong breaks determinism. Not replaceable. |
| Libraries | Pathfinding, crowd avoidance, graph networks, visibility and fog of war, deterministic 2D physics, the renderer, UI and text, the audio mixer, the mod host (Raido). | Optional and replaceable. None uses a door the game can't. |
| Game | Rules, AI, jobs, items, content, art. | Native Rask, with the kernel's guarantees. Mods are optional. |

## Decided

- **The game is native Rask.** Hot loops are the game in this genre: Factorio's belts, Oxygen Not Included's gases, Noita's materials. A kernel that owned them would impose one belt model on every game; a game that wrote them outside the kernel's guarantees would desync. So game code is ordinary Rask and gets the same guarantees as the kernel: deterministic collections, kernel-run parallelism, and the build plus the game code hashed into the session's identity, so two clients on different builds can't join each other.
- **Mods are optional, and they're Raido.** A game that wants modding hosts Raido; one that doesn't, doesn't.
- **Lockstep, co-op first.** The state is far too big to send and the inputs are tiny. Every client holds the full state, so map hacks are possible. That's fine for co-op and wrong for competitive play, which isn't the target.
- **No MMO shape.** Partial world state and sharding is a different business.
- **Pause and speed are inputs.** In lockstep a speed change is just another entry in the log, so multiplayer pauses and changes speed like single player (Paradox's games and Age of Empires do). Who may pause is the game's call. Multiplayer sessions save and resume.
- **The loop steps on a clock or on input.** Real-time games step at a fixed rate; turn-based games step when a turn's inputs are in. Asynchronous multiplayer then falls out of the input log.
- **Render is a view.** It reads the state between steps, interpolates, and never writes it. The kernel runs headless; the renderer is a library, so a game can bring its own, or run as a plugin in another engine.
- **Bind the platform, write the opinions.** SDL3 for windows, GPU and audio devices. stb and dr libraries for codecs. The renderer and the mixer are written in Rask.

## Determinism

Every source of drift is closed in the kernel, so game code can't reopen it by accident:

- **Floats:** correctly rounded transcendentals, no fused multiply-add, the default FP environment on every thread (`determinism/D12`, done in rask-lang/rask#1752). Raido uses the same f64.
- **Iteration order:** game state uses collections with a deterministic order. Rask's `Map` seeds its order per process on purpose (`determinism/D7`), so the kernel needs an ordered map; `specs/determinism.md` lists it as an open question.
- **Randomness:** lives in the state, keyed by (system, entity, draw number). Never by thread or task, so results don't depend on how work is split, and region size can be tuned after release without breaking replays.
- **Merges:** effects that cross tasks apply after the join, ordered by entity id, never by task or completion order.
- **Budgets count work, never time.** "Pathfind until 2 ms pass" gives every machine a different answer; "100 path requests per tick" doesn't.
- **Uninitialised memory:** Rask has none to read.
- **Mods:** Raido has no I/O and a deterministic interpreter.

## Entities

An entity is a stable id. Its components live in arrays inside its region, and an indirection table maps the id to where it is now. References between entities, sorted query results, saves and render interpolation all use the id, so moving between regions or a swap-remove changes nothing a reader can see.

Fields always read together (x and y, velocity and position in a movement pass) go in one array of small structs; fields read rarely (name, history, inventory) go in a cold array. Splitting every field apart costs as much as never splitting. In Rask an array of small structs is a `Vec` of `@layout(C)` structs, packed at natural width (#985), and the same buffer can go to the GPU as instance data.

## Space

I don't assume a grid. Space is a partition into regions, and a region is whatever the game's space type says: a tile, a cube, a hex, an octree cell, a room, a navmesh polygon. The kernel asks the space type three things: which region a position is in, which regions border a region, and how far writes reach.

A regular lattice gives a lot for free: region lookup is one division, neighbours are implicit, the colouring is fixed, field data packs into dense arrays of one shape. Keeping the interface general costs nothing as long as the space type is a generic parameter and declares what it knows ahead of time (colour count, field shape, implicit neighbours); a grid fills those in at compile time. The kernel ships square, hex and cube lattices, plus several separate spaces in one game (Factorio's planets and platforms). Moving and rotating sub-grids (ships, vehicles), spheres, and graph worlds stay possible through the same interface and get built when a game needs one.

The region is the unit for storage, copy-on-write checkpoints, the state hash, parallel tasks, and streaming. The usual problems and their answers:

- **Crossing a boundary:** an entity leaving its region goes into an outbox and moves after the tick, in entity-id order. A margin around each region stops a unit on the boundary from flipping back and forth.
- **Neighbour queries:** your region and its neighbours, because regions are at least one interaction radius wide. Long reach (a turret's 24 tiles, artillery's 224) uses a coarser level of the same index. Results come back sorted by entity id.
- **Sleep:** per entity, with a scheduled wake tick, as Factorio does. A furnace sleeps until its smelt finishes; a belt segment until a gap reaches its end. A region whose entities all sleep skips entirely. Wake-ups live in one queue ordered by (tick, entity, sequence). Float fields need a settle threshold written into the rules, or diffusion approaches zero forever and nothing sleeps.
- **Fields** (fog, temperature, gas, flow): dense data per region with a ring of copied neighbour cells (a halo) filled at the start of each tick. Domain decomposition, as supercomputer simulations call it.
- **Unbounded worlds:** a region is generated from the world seed and its key, so every machine generates the same one. Which regions simulate depends only on the state (activity, nearness to any player), never on one client's camera.
- **Large coordinates:** positions are relative to their region.

## Parallelism

Each tick forks work across cores and joins before the next. Work stealing is fine, because which core runs a task can't change its result. A system picks one of two models:

- **Double-buffered (the default):** a task writes only its own slice and reads last tick's values for everything else. Rask's ownership rules make that checkable.
- **In-place colour passes:** for systems that move matter between cells (falling sand, fluids). The region graph is coloured so no two neighbours share a colour (4 for a square grid, 3 for hex, 8 for cubes); one colour's regions run in parallel, the colours run in a fixed order. This needs writes to reach at most half a region, or two same-colour regions could write the same cell between them. Noita writes at most 32 pixels into 64-pixel chunks for this reason.

Systems run at their own rates (temperature every 4 ticks, path requests spread across ticks), with the schedule part of the state.

What's missing in the language is fork-join that can borrow (#1686) and `kernel` blocks where each lane writes only its own slot (#1654). Those are ranked from main like any other compiler work.

Rendering doesn't join the tick. The simulation runs on its own thread at a fixed rate and publishes each finished state. The renderer draws at the display's rate from the last two published states, interpolating by entity id.

## Mods

When a game hosts mods, they're Raido, which makes several hard problems easy:

- **Downloading code from a server is safe.** Raido has no I/O; a mod reaches only the externs the game registers. Images and sounds from a mod go through C decoders, so those run sandboxed or validated.
- **A mod is its hash.** Bytecode is content-addressed, so the mod set is a list of hashes, and together with the build hash it's the session's identity. Mismatched mods, RimWorld multiplayer's main cause of desyncs, can't happen.
- **Mod state is game state.** Raido's VM state is serializable, so checkpoints and replays include it.
- **Handlers are transactions.** Running out of fuel is deterministic, but it unwinds after side effects. An event handler commits its writes only when it finishes, so an over-budget handler leaves nothing half-done.

Two stages, as in Factorio. The data stage runs once at load and declares what exists: entity types, components, recipes, sprites. The control stage runs during play and reacts to events, in a fixed order relative to the tick's systems and in entity-id order within one.

- **Simulation mods** change the rules. They must match on every machine and are part of the session's identity.
- **Client mods** change UI, sounds and visuals. They're local, never hashed, and can read the state but never write it.

Raido is an interpreter, so 100,000 units each running a script every tick won't fit. Mods configure native systems with data and handle events; a mod that needs a new hot loop asks the game for a system. Compiling Raido to native code later would move that line, provided the compiled code keeps the interpreter's results and its fuel accounting.

## Saves, replays and versions

These are different things:

- **A replay** is a checkpoint plus a log, frozen to the rules that recorded it. It replays on those rules or not at all. Raido's content-addressed bytecode helps.
- **A save** has to survive game and mod updates halfway through a hundred-hour colony, as players update Workshop mods mid-game. So a save carries versioned schemas, and loading one runs migrations. Saved coroutines suspended inside old bytecode can't resume across a mod update; a mod that keeps coroutines across ticks has to say what happens to them.

Chunks are copy-on-write, so a checkpoint costs what changed since the last one.

## Networking

**Finding a desync.** The state lives in chunks hashed as a Merkle tree (Urd's store). In falling sand every awake chunk changes every tick, so the full hash runs every N ticks, with a cheaper rolling hash of writes in between. Mod state is split per mod and per handle so a mismatch narrows past "some mod". When two machines disagree, comparing trees walks to the chunk, then the entity. With the seed and the log, the desync replays on one machine.

**Recovering from one.** The drifted peer drops its state, takes a checkpoint from the others, and catches up, the same path as joining.

**Joining a game in progress.** The joiner downloads a checkpoint while the game runs, then simulates faster than real time. That needs headroom: the game should run at most at, say, 60% of a tick's budget on its slowest supported machine, or a joiner on that machine may never catch up with a growing colony. The exact target is open.

**A headless server** is a peer that only simulates, for hosting and persistence.

**Input latency.** An input takes effect a round trip later. That's fine for They Are Billions or Dwarf Fortress, and noticeable for Noita's hands-on action. Rolling back a million-cell world isn't affordable, so it's input delay plus cosmetic prediction for the player's own avatar, as Factorio does.

**A slow machine** stalls everyone. The options are adapting the tick rate, letting that player fall behind and catch up, or dropping them.

**Conflicting commands.** Two players order the same dwarf in the same tick. The kernel orders inputs within a tick (by player, then sequence), so the result is the same everywhere. What the game does with the conflict is its own design.

## Libraries

Every game in the genre writes these, and none is the game, so they ship as optional libraries:

- **Pathfinding:** flow fields for hordes (one shared field per target, as They Are Billions does); hierarchical paths with invalidation for colonists who each go somewhere different and repath when a wall is mined (Dwarf Fortress's known frame-rate killer).
- **Crowd avoidance:** units pushing past each other without overlapping.
- **Graph networks:** power, fluids, belts, logistics, circuits, with connectivity kept up to date as things are built and destroyed, across regions.
- **Visibility:** line of sight and fog of war, computed incrementally.
- **Deterministic 2D physics,** for physics sandboxes.
- **Rendering at scale:** instance buffers from component arrays, so the CPU does nothing per entity per frame. Sprites are packed into atlases grouped by layer, streamed into VRAM as 128x128 tiles the way Factorio does (FFF #264), and compressed (BC7, ASTC). Atlas packing, mips and compression happen at build time, in `build.rk`.
- **UI and text:** layout, text shaping, localization.
- **Audio:** the mixer, with voice limiting for thousands of sources. Its reverb tails decay into subnormals, so it flushes them itself.

## Out of scope

- **Competitive RTS.** Lockstep gives every client the whole state, so fog of war can't be enforced.
- **Rollback for fast action on huge worlds.** Not affordable; see input latency.

## Open

- The space interface in detail: what a lattice declares, whether a hierarchy of region sizes belongs in the kernel or in one space type, and how a space type states its write reach.
- The ordered map the kernel needs (`determinism` open question), and fork-join that can borrow (#1686).
- The headroom target for joining.
- The transport under Ting: stay on ngtcp2, or move to GameNetworkingSockets, which has relays and NAT traversal.
- The first flagship game. Something with many small things following local rules (falling sand, factory belts), because it hits entity count and determinism on day one.
- Literal-built number types for simulation units such as `Fixed` and `Meters`: done in rask-lang/rask#1752 (`FromLiteral<T>`).
- Shared-library output from `rask build`, for running as a plugin in another engine.

## Related

- [Ting](../ting/README.md): the transport underneath.
- [Urd](../urd/README.md): log, snapshots and a Merkle state store, which are the checkpoint machinery.
- [Raido](../raido/README.md): deterministic mods.
