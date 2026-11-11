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
| Game | Rules, AI, jobs, items, content, art. | Native Rask with the kernel's guarantees, a Raido mod over native systems, or a mix. The game's choice. |

## Decided

- **Native game code gets the kernel's guarantees.** Hot loops are the game in this genre: Factorio's belts, Oxygen Not Included's gases, Noita's materials. A kernel that owned them would impose one belt model on every game; game code outside the kernel's guarantees would desync. So a game's own Rask gets the same guarantees as the kernel: deterministic collections, kernel-run parallelism, and the build plus the game code hashed into the session's identity, so two clients on different builds can't join each other.
- **Where the game's logic lives is the game's choice.** It can be all native Rask. Or it can be Factorio's shape: the game's hot loops are native Rask systems, hooks and integrators, and the core rules and content are a Raido mod on top that drives them, so the base game is a mod like any other. Or any mix. In the mod shape, the base mod reaches native code only through the externs the game registers, the same ones every other mod gets, so whatever the game's designers can do, a modder can do.
- **Mods are Raido.** A game that hosts mods, or is one, uses Raido; a game that doesn't want modding never loads it.
- **Lockstep, co-op first.** The state is far too big to send and the inputs are tiny. Every client holds the full state, so map hacks are possible. That's fine for co-op and wrong for competitive play, which isn't the target.
- **No MMO shape.** Partial world state and sharding is a different business.
- **Pause and speed are inputs.** In lockstep a speed change is just another entry in the log, so multiplayer pauses and changes speed like single player (Paradox's games and Age of Empires do). Who may pause is the game's call. Multiplayer sessions save and resume.
- **The loop steps on a clock or on input.** Real-time games step at a fixed rate; turn-based games step when a turn's inputs are in. Asynchronous multiplayer then falls out of the input log.
- **A slow tick slows the game, it never skips.** In lockstep every machine has to run every tick, so when a tick takes longer than its budget the game runs slower than real time, and the kernel reports where the time went, per system.
- **Render is a view.** It reads the state between steps, interpolates, and never writes it. The kernel runs headless; the renderer is a library, so a game can bring its own, or run as a plugin in another engine.
- **Bind the platform, write the opinions.** SDL3 for windows, GPU and audio devices. stb and dr libraries for codecs. The renderer and the mixer are written in Rask.

## Game code

A game writes rules and nothing else. Falling sand, complete:

```rask
import verdandi

enum Cell { Empty, Sand, Water, Stone }

func fall(mutate cells: verdandi.Cells<Cell>, at: verdandi.At, rng: verdandi.Rng) {
    if cells[at] != Cell.Sand { return }
    let side = if rng.bool() { at.left() } else { at.right() }
    for to in [at.below(), side.below()] {
        if cells[to] == Cell.Empty {
            cells.swap(at, to)
            return
        }
    }
}

func main() {
    verdandi.run(
        space: verdandi.Grid2(width: 1024, height: 512, region: 64),
        cells: Cell.Empty,
        systems: [fall],
    )
}
```

And units:

```rask
struct Pos { x: Fixed, y: Fixed }
struct Vel { x: Fixed, y: Fixed }

func drift(mutate p: Pos, v: Vel) {
    p.x += v.x
    p.y += v.y
}
```

- **A system is a plain function, and its parameters are its declaration.** `mutate cells: Cells` with a position makes a cell rule, run as in-place colour passes. Component parameters make an entity rule, run for every entity that has them: `mutate` ones are written, the rest are read from last tick. An `Rng` parameter comes keyed by (system, subject, draw number). No registration, no scheduler setup, no network or save code.
- **Run order is the `systems` list.** Not inferred from data dependencies, so it's in the code and there's no hidden scheduler.
- **Cells and entities are both first-class.** Dense fields per cell (sand, gas, temperature) and sparse entities with components (units, buildings). The genre needs both, so neither is built on the other.
- **Reach comes from the space and is enforced on access.** A cell rule reaching past half a region is a deterministic panic, never a race.
- **Sleep:** a region where a cell rule changed nothing sleeps until a neighbour changes. An entity that waits calls `verdandi.sleep(until: tick)`.

This needs one thing from the language: the kernel has to see a function's parameter types and modes at compile time. `std.reflect` lists a type's fields and methods, not a function's parameters.

## Determinism

Every source of drift is closed in the kernel, so game code can't reopen it by accident:

- **Floats:** correctly rounded transcendentals, no fused multiply-add, the default FP environment on every thread (`determinism/D12`, done in rask-lang/rask#1752). Raido uses the same f64.
- **Iteration order:** game state uses collections with a deterministic order. Rask's `Map` seeds its order per process on purpose (`determinism/D7`), so the kernel needs an ordered map; `specs/determinism.md` lists it as an open question.
- **Randomness:** lives in the state, keyed by (system, subject, draw number), where the subject is an entity id or a region plus a cell index (a falling-sand cell isn't an entity, and its reaction rolls need keys too). Never by thread or task, so results don't depend on how work is split, and region size can be tuned after release without breaking replays.
- **Merges:** effects that cross tasks apply after the join, ordered by entity id, never by task or completion order.
- **Budgets count work, never time.** "Pathfind until 2 ms pass" gives every machine a different answer; "100 path requests per tick" doesn't.
- **Uninitialised memory:** Rask has none to read.
- **Mods:** Raido has no I/O and a deterministic interpreter.

## Entities

Every game in this genre hand-rolls "my target died": ids with generation counters, checked on every read. Rask's racks make that impossible instead, so entities are built on them.

- **One rack of entity nodes is the identity table.** A node is small and cold: a stable `id`, the entity's kind, and where its components live (region and index). Nodes never move (`mem.racks/RK1`), so an entity moving between regions changes only its node's location fields. That's the indirection table, built in.
- **Hot data lives in region arrays.** Components sit in per-region arrays for streaming; the node says where an entity's data is right now.
- **References are `Link<Entity>?`, stored anywhere.** A unit's target, a belt's next belt, a pawn's job site, in a node or right in a hot component array. Deleting an entity sets every link to it to `none` before the delete returns (RK3), so a stale reference can't be observed and reads need no check (RK4). Storing a link costs a few nanoseconds of bookkeeping at any number of holders (RK10); only a delete walks edges.
- **Spawn, delete and region moves are staged** during the parallel phase and applied after the join, in id order. That's what determinism needs, and it keeps deletes' edge walks out of the parallel phase.
- **Order and identity come from `id`.** Links compare but don't order (RK11), and a slot can be reused by a later node (RK12), so sorted queries and save identity use `id`. Hashes and checkpoints use slot numbers, which are deterministic given the same operations (`determinism/D8`); saves translate links through slot numbers as `mem.relocatable` does.

Render interpolation, sorted query results and saves all follow the entity, so moving between regions or a swap-remove changes nothing a reader can see.

Fields always read together (x and y, velocity and position in a movement pass) go in one array of small structs; fields read rarely (name, history, inventory) go in a cold array. Splitting every field apart costs as much as never splitting. In Rask an array of small structs is a `Vec` of `@layout(C)` structs, packed at natural width (#985), and the same buffer can go to the GPU as instance data.

## Space

I don't assume a grid. Space is a partition into regions, and a region is whatever the game's space type says: a tile, a cube, a hex, an octree cell, a room, a navmesh polygon. The kernel asks the space type three things: which region a position is in, which regions border a region, and how far writes reach.

A regular lattice gives a lot for free: region lookup is one division, neighbours are implicit, the colouring is fixed, field data packs into dense arrays of one shape. Keeping the interface general costs nothing as long as the space type is a generic parameter and declares what it knows ahead of time (colour count, field shape, implicit neighbours); a grid fills those in at compile time. The kernel ships square, hex and cube lattices, graph spaces (Paradox's provinces and star lanes, RimWorld's rooms over its grid), and several separate spaces in one game (Factorio's planets and platforms). Spheres (RimWorld's world map) and moving, rotating sub-grids (ships, vehicles) stay possible through the same interface and get built when a game needs one.

The region is the unit for storage, copy-on-write checkpoints, the state hash, parallel tasks, and streaming. The usual problems and their answers:

- **Crossing a boundary:** an entity leaving its region goes into an outbox and moves after the tick, in entity-id order. A margin around each region stops a unit on the boundary from flipping back and forth.
- **Neighbour queries:** your region and its neighbours, because regions are at least one interaction radius wide. Long reach (a turret's 24 tiles, artillery's 224) uses a coarser level of the same index. Results come back sorted by entity id.
- **Sleep:** per entity, with a scheduled wake tick, as Factorio does. A furnace sleeps until its smelt finishes; a belt segment until a gap reaches its end. A region whose entities all sleep skips entirely. Wake-ups live in one queue ordered by (tick, entity, sequence). Float fields need a settle threshold written into the rules, or diffusion approaches zero forever and nothing sleeps.
- **Fields** (fog, temperature, gas, flow): dense data per region with a ring of copied neighbour cells (a halo) filled at the start of each tick. Domain decomposition, as supercomputer simulations call it.
- **Unbounded worlds:** a region is generated from the world seed and its key, so every machine generates the same one. Which regions simulate depends only on the state (activity, nearness to any player), never on one client's camera.
- **Large coordinates:** positions are relative to their region.
- **Far regions run less often.** A region's rate (every tick, every 4th, every 16th) depends only on state: distance to the nearest player-owned thing or active event, never a camera. A system opts in by taking a `dt: Ticks` parameter and integrating over the elapsed ticks (growth, temperature, a far colony's economy); a system without one (belts, combat) runs every tick, and a region where one is active stays at full rate. A region at rate k runs on ticks where `tick % k` equals a phase derived from its key, which spreads the load. A full-rate region reading a slower neighbour sees data up to k ticks old: deterministic and bounded. An event promotes a slow region to full rate from the next tick. Sleep comes first; reduced rates are opt-in, and the default is every tick. A cheaper model for far-away places (Dwarf Fortress's world simulation, RimWorld's world map) is the game's to write.

## Parallelism

Each tick forks work across cores and joins before the next. Work stealing is fine, because which core runs a task can't change its result. A system picks one of three models:

- **Double-buffered (the default):** a task writes only its own slice and reads last tick's values for everything else. Rask's ownership rules make that checkable.
- **In-place colour passes:** for systems that move matter between cells (falling sand, fluids). The region graph is coloured so no two neighbours share a colour (4 for a square grid, 3 for hex, 8 for cubes); one colour's regions run in parallel, the colours run in a fixed order. This needs writes to reach at most half a region, or two same-colour regions could write the same cell between them. Noita writes at most 32 pixels into 64-pixel chunks for this reason.
- **Sequential:** for a rule whose writes reach arbitrarily far. Dwarf Fortress's water pressure moves water instantly through a connected body to wherever it can escape. Such a body is better kept as a network over cells (the graph-network library's connectivity, applied to cells) so the system works per body; when nothing fits, the system runs on one core, in a fixed order.

Systems run at their own rates (temperature every 4 ticks, path requests spread across ticks), with the schedule part of the state.

What's missing in the language is fork-join that can borrow (#1686) and `kernel` blocks where each lane writes only its own slot (#1654). Those are ranked from main like any other compiler work.

Rendering doesn't join the tick. The simulation runs on its own thread at a fixed rate and publishes each finished state. The renderer draws at the display's rate from the last two published states, interpolating by entity id.

## Testing determinism

A game should learn it's nondeterministic from a failing test, not from a multiplayer bug report.

**The twin run.** `rask test` runs the game twice from the same seed and input log, varying everything that's supposed not to matter, and compares the state hash after every tick:

| Varied | Catches |
|---|---|
| Thread count (one versus all cores) | anything that depends on threads |
| Task order, drawn from a seed (Rask's sim mode) | merges in completion order, races |
| Region size, where the space allows | random draws or merges keyed by region or task |
| Map iteration seed (its own sim-mode stream, `sim/SD1`) | iterating a `Map` in game state |
| Memory layout | an address leaking into logic |

During the test the hash is taken after every system, per region, so the first difference names the tick, the system and the region; diffing the two states then names the entity and the field. The report ends with a replay line, as sim mode's do.

The same machinery gives:

- **Regression tests:** a recorded session (seed, input log, hash trail) replays and must match. When the rules change on purpose, it's re-recorded.
- **Cross-machine runs:** recorded sessions run on x86-64 and on aarch64 under qemu in CI.
- **Seed search:** bot players with random inputs over many seeds (`--seeds N`).

`rask lint` warns when a system iterates a `Map`, reads the clock, or reaches outside its parameters. Information, not enforcement; the twin run enforces. It costs twice the CPU, so it's for tests and CI; a dev build can run a shadow copy every N ticks.

## Performance budget

Estimates for the prototype to confirm or break, not measurements. At 60 ticks a second a tick is 16.7 ms; joining needs headroom, so the simulation gets about 10 ms on the slowest supported machine (8 cores). Rendering runs on its own thread.

| Work | Estimate | Target |
|---|---|---|
| A simple entity system over 100,000 units | about 5 MB of memory traffic, about 0.5 ms on one core | ≤ 0.2 ms on 8 cores |
| Falling sand, 1024×512 cells, 30% awake | about 10 ns per awake cell, about 1.5 ms on one core | ≤ 0.5 ms on 8 cores |
| A flow field over 1 million cells | 10 to 20 ms from scratch | spread over ticks with a count budget, ≤ 1 ms per tick |
| Kernel overhead: staging, merges, rolling hash, link bookkeeping | grows with changes, not world size | ≤ 0.5 ms |
| Mod handlers | unknown | ≤ 2 ms, set by fuel |
| Left for game logic | | about 5 ms |

What that asks of Rask's compiler, as a measured target: a loop over a `Vec` of small structs within 1.5 to 2× of the same loop in C. That means bounds checks hoisted, no refcount traffic in loops, small functions inlined, fields in registers, SIMD where it fits. `rask bench` already compares against a C baseline, so the prototype measures nanoseconds per cell and per entity against C. Misses become compiler work, ranked from main.

## Mods

When a game hosts mods, or its core is one, they're Raido, which makes several hard problems easy:

- **Downloading code from a server is safe.** Raido has no I/O; a mod reaches only the externs the game registers. Images and sounds from a mod go through C decoders, so those run sandboxed or validated.
- **A mod is its hash.** Bytecode is content-addressed, so the mod set is a list of hashes, and together with the build hash it's the session's identity. Mismatched mods, RimWorld multiplayer's main cause of desyncs, can't happen.
- **Mod state is game state.** Raido's VM state is serializable, so checkpoints and replays include it.
- **Patching happens at load time, inside the sandbox.** A mod can wrap or replace any named Raido function of the base game (when the core is a mod) or of another mod: before it, after it, or instead of it, as RimWorld's Harmony patches do. Patches are resolved once, when mods load, in load order, and the composed program is what's hashed into the session, so every machine runs the same one. Native systems can't be patched; mods reach them only through the externs the game registers. Nothing is patched at runtime, and no mod runs outside the sandbox.
- **Handlers are transactions.** Running out of fuel is deterministic, but it unwinds after side effects. An event handler commits its writes only when it finishes, so an over-budget handler leaves nothing half-done.

Two stages, as in Factorio. The data stage runs once at load and declares what exists: entity types, components, recipes, sprites. The control stage runs during play and reacts to events, in a fixed order relative to the tick's systems and in entity-id order within one.

- **Simulation mods** change the rules. They must match on every machine and are part of the session's identity.
- **Client mods** change UI, sounds and visuals. They're local, never hashed, and can read the state but never write it.

Raido is an interpreter, so 100,000 units each running a script every tick won't fit. That's the line between the two halves of the mod shape: per-entity work is native (the game's systems, hooks and integrators, registered as externs), and the mod configures them with data and handles events. A mod that needs a new hot loop asks the game for a system. Compiling Raido to native code later would move that line, provided the compiled code keeps the interpreter's results and its fuel accounting.

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
- **Graph networks:** power, fluids, belts, logistics, circuits, and connected bodies of cells (a lake, a gas pocket), with connectivity kept up to date as things are built and destroyed, across regions.
- **Visibility:** line of sight and fog of war, computed incrementally.
- **Deterministic 2D physics,** including bodies built from cell masks and broken apart by explosions, as Noita's are. That's the hard part, well beyond rigid boxes.
- **Rendering at scale:** instance buffers from component arrays, so the CPU does nothing per entity per frame. Sprites are packed into atlases grouped by layer, streamed into VRAM as 128x128 tiles the way Factorio does (FFF #264), and compressed (BC7, ASTC). Atlas packing, mips and compression happen at build time, in `build.rk`.
- **UI and text:** layout, text shaping, localization.
- **Audio:** the mixer, with voice limiting for thousands of sources. Its reverb tails decay into subnormals, so it flushes them itself.

## Could you build these games on it?

The stress test for the design: rewrite each target game on the kernel and see what breaks.

| Game | What it uses | What it needs that isn't plain |
|---|---|---|
| Factorio | 2D chunks, several surfaces, graph networks for belts, power, fluids, circuits and rails, per-entity sleep, pollution as a field, budgeted biter pathing, the base game as a mod over native systems. | Nothing new. Speed is the risk: years of hand-tuned C++ in the belts. |
| They Are Billions | Continuous movement over a spatial hash, flow fields, crowd avoidance, instanced rendering. | Nothing new. |
| Noita | In-place colour passes over pixel chunks, half-region reach. | Physics bodies built from pixels; random keys per cell; hashing every N ticks. |
| Dwarf Fortress | 3D voxels, temperature field, per-dwarf pathing with invalidation, seeded world generation, century-long saves. | Water pressure writes arbitrarily far: connected bodies as networks, or a sequential system. |
| RimWorld | Multi-rate ticks (normal, rare every 250, long every 2000), rooms as a graph over the grid, few pawns with heavy AI under budgets. | A sphere space for the world map. Its mods patch any method at runtime (Harmony); here they patch any Raido function at load time, so the moddable part keeps that culture and native systems don't. |
| Paradox grand strategy | Graph spaces, daily and monthly ticks, lockstep with pause and speed, hot join and desync recovery, parallel AI per country, script as Raido data and control stages. | Its own 3D map renderer, which the renderer being a library allows. |

## Out of scope

- **Competitive RTS.** Lockstep gives every client the whole state, so fog of war can't be enforced.
- **Rollback for fast action on huge worlds.** Not affordable; see input latency.

## Open

- The space interface in detail: what a lattice declares, whether a hierarchy of region sizes belongs in the kernel or in one space type, and how a space type states its write reach.
- Language items, ranked from main: the ordered map the kernel needs (`determinism` open question), fork-join that can borrow (#1686), compile-time access to a function's parameter types and modes, and staged structural mutation on racks (designed in `mem.racks`, unbuilt).
- The headroom target for joining.
- The transport under Ting: stay on ngtcp2, or move to GameNetworkingSockets, which has relays and NAT traversal.
- A falling-sand prototype first, as a speed benchmark against the performance budget. Every target game is hand-tuned C++ or C#, and throughput decides whether lockstep and fast-forward exist at all. Compiler work it turns up is ranked from main.

- Literal-built number types for simulation units such as `Fixed` and `Meters`: done in rask-lang/rask#1752 (`FromLiteral<T>`).
- Shared-library output from `rask build`, for running as a plugin in another engine.

## Related

- [Ting](../ting/README.md): the transport underneath.
- [Urd](../urd/README.md): log, snapshots and a Merkle state store, which are the checkpoint machinery.
- [Raido](../raido/README.md): deterministic mods.
