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
- floats (same build, our own `sin`/`cos`/`sqrt`, no fused multiply-add: #1683);
- map iteration order;
- parallel scheduling;
- randomness, which lives in the state, seeded per system;
- uninitialised memory;
- mods.

Raido runs mods, because a mod that can't be nondeterministic can't desync.

**Finding a desync.** A hash of the full state every tick is too slow at this size. The state lives in chunks hashed incrementally, as a merkle tree (Urd's store), so the per-tick hash is cheap. When two machines disagree, comparing the trees walks straight to the chunk, then the entity, that drifted. With the seed and the input log, the desync replays on one machine.

**Throughput is correctness.** Lockstep runs at the speed of the slowest machine, and 4x fast-forward costs 4x the CPU. So simulation speed decides whether the features exist at all:
- component arrays, not objects;
- regions that sleep when nothing in them changes, like Noita's dirty rectangles;
- parallel systems with a fixed merge order. Falling sand updates chunks in a checkerboard pattern, as Noita does, which keeps the parallelism deterministic.

This is also where Rask's Cranelift backend gets stress-tested: SIMD, inlining, layout.

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
