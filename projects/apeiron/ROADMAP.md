# Roadmap
<!-- id: apeiron.roadmap --> <!-- status: proposed --> <!-- summary: Build order from specs to playable game -->

Everything below is specs. ~14K lines of markdown across six projects. Zero implementation beyond the Rask compiler and one Python simulation. This roadmap turns specs into a playable game.

## Dependency Chain

```
Rask (working) ──→ Raido VM (Rask) ──→ Galaxy Gen ──→ History Sim ──→ Stage 1 (Rask)
                                                                           │
                                             Leden (Rask) ──→ Allgard (Rask) ──→ Stage 2-3
                                                                                      │
                                                                               GDL ──→ Stage 4
```

Everything is Rask. Raido, Leden, Allgard, the game server — all written in Rask. These are language validation targets. Raido exercises low-level systems code (fixed-point arithmetic, arena allocation, bytecode dispatch). Leden exercises networking. Allgard exercises data structures and verification. The game server exercises everything.

Stage 0 (simulations) validates game mechanics cheaply in Python before committing to implementation. Stage 1 doesn't need federation. One operator, one process, five simulated systems. Raido provides deterministic physics and galaxy generation. The game server enforces conservation laws internally. When you federate in Stage 2, you extract enforcement into Allgard.

Leden and Allgard develop in parallel with Stage 1, not as prerequisites. They converge at Stage 2 when federation becomes real.

Midgard isn't a build phase — it's the patterns that emerge when you federate. Document them as you go.

Combat is a parallel track. Prototype it standalone, integrate when both combat and the trading game are solid.

## Current State

| Project | Lines of spec | Implementation | Status |
|---------|--------------|----------------|--------|
| Rask | — | Working compiler, interpreter, stdlib | Shipping |
| Raido | 1,117 | None | Specs complete |
| Leden | 1,940 | None | Specs complete |
| Allgard | 3,185 | None | Specs complete |
| Midgard | 429 | None | Design docs |
| GDL | 2,649 | None | Specs complete |
| Apeiron | 2,100+ | 1 Python sim | Design docs |

## Stage 0: Simulations

Validate game mechanics before building anything. Python scripts, fast iteration, throw-away code. The goal is to kill bad ideas cheaply.

### Economy Viability

Extend the existing `transform_sim.py`. 5 systems, AI agents (extractors, haulers, stations), 10K ticks. Does the economy circulate? Do prices stabilize? Does trade emerge from geographic scarcity?

**Passes if:** credits don't hyperinflate, at least 3 of 5 systems participate in trade, no single agent accumulates >50% of total supply.

**Fails if:** economy collapses, one agent dominates, prices converge to uniform (no trade incentive).

### Crafting Discovery

Interactive CLI. A player picks elements, ratios, conditions. The interaction function evaluates. Hill-climbing toward stoichiometric peaks — is the search fun? Does the player feel like a scientist or a slot machine?

**Passes if:** players can reason about results ("more carbon made it harder"), peaks are discoverable through experimentation, not random guessing.

**Fails if:** the search space is too flat (everything works roughly the same) or too spiky (no gradient to follow).

### Element Count

5 elements vs 14 elements. Same economy sim, same crafting sim. Does 14 add meaningful depth — more trade routes, more specialization, more interesting recipes? Or is it just noise — more names to memorize, same dynamics?

**Passes if:** 14 elements create trade patterns that 5 don't (specialized systems, multi-hop routes, element scarcity driving conflict).

**Fails if:** reducing to 5 loses nothing noticeable. Simpler is better.

### Galaxy Topology

Generate 10K stars. Visualize clusters, bridges, chokepoints, frontier arms. Does the topology create interesting strategic geography? Are there natural trade corridors, defensible positions, exploration frontiers?

**Passes if:** the galaxy has visible structure — dense cores, sparse arms, bridge stars that matter. A player looking at the map can identify "that cluster is rich but crowded" vs "that arm is remote but resource-dense."

**Fails if:** it looks uniform, or the structure doesn't create meaningful gameplay differences.

## Phase 1: Raido VM

Everything downstream depends on Raido. Galaxy generation, physics evaluation, crafting verification, combat scripts, minting proofs — all Raido bytecode. This is the critical path.

### What to build

A register-based VM that executes Raido bytecode with:
- 32.32 fixed-point arithmetic (bitwise-identical results on every platform)
- Arena-based memory (no GC, deterministic allocation)
- Serializable execution state (pause, resume, migrate)
- Content-addressed bytecode chunks (the script IS its hash)
- Host interop (the game server calls Raido, Raido calls back for object access)
- Fuel metering (bounded execution — scripts can't loop forever)

### What to skip initially
- Coroutines (spec'd but not needed for Phase 2-3)
- Full stdlib (build what galaxy gen needs, expand later)
- Optimization (correctness first — determinism is non-negotiable)

### Validation
- Write the galaxy generation script. If Raido can generate 10,000 stars with positions, spectral classes, planets, and resource deposits from a single seed — and two independent runs produce bitwise-identical output — the VM works.
- Write one crafting script (structural steel: Fe+C at 97:3). If the interaction function evaluates deterministically with fixed-point math, transformation physics works.

### Implementation language
Rask. Raido exercises exactly the kind of systems code Rask needs to prove it handles well — fixed-point arithmetic, arena allocation, bytecode dispatch loops, host interop through FFI. If Rask can't build a register VM cleanly, that's a language problem to fix, not to work around.

## Phase 2: Galaxy Generation

First real Raido program. The galaxy seed script.

### What to build

A Raido script that takes (seed: int, star_id: int) and returns:
- 3D position in galaxy space
- Spectral class
- Planet count and orbital properties
- Resource deposits per body (element type, total extractable quantity)
- Procedural name

The script also takes an epoch parameter for time-gated reveals (stars 8001+ visible after epoch 1, etc.).

Galaxy structure: dense cores, sparse arms, bridge stars. Not uniform random — the topology should create natural clusters, chokepoints, and frontiers.

### Validation
- Generate the full 10,000-star galaxy. Inspect visually (2D/3D scatter plot).
- Verify resource distribution matches the element table: common elements in 90-100% of systems, strategic in 30-80%, exotic in 5-15%.
- Run the same script on two machines. Bitwise-identical output. This is the whole point of Raido.
- Profile performance. Galaxy gen runs once per client session — it can take seconds. But per-star queries (during gameplay) should be fast.

### Deliverable
A working galaxy that anyone can regenerate from a seed integer. The first tangible artifact of the project.

## Phase 2.5: History Simulation

The galaxy needs history before players arrive. This phase generates it.

AI agents run real domains in the generated galaxy. Civilizations emerge, expand, trade, conflict, fall. The simulation uses the same conservation laws, physics scripts, and economy rules that players will use. The history is real — verifiable, consistent, not hand-written.

### What to build

A simulation harness that:
- Instantiates AI domain operators across the galaxy
- Runs the economy (extraction, crafting, trade, consumption) for N epochs
- Simulates civilization rise and fall (expansion pressure, conflict, resource depletion)
- On civilization death: agents release objects, domains publish final [state snapshots](../allgard/PRIMITIVES.md#domain-state-snapshots)
- Outputs a content-addressed history blob: per-star timeline, surviving objects, snapshot references

### Output

The simulation produces:
- **History per star:** who claimed it, what they built, when they fell, what's left
- **Unclaimed objects:** ancient ships, stations, stockpiles scattered across dead domains
- **Resource depletion:** some deposits are partially mined. The galaxy isn't pristine.
- **Ruins and artifacts:** objects with proof chains that reference civilizations that no longer exist

### Validation

- The history is internally consistent — proof chains validate, conservation laws hold
- The history creates interesting exploration content — there are things to find, mysteries to investigate
- The history is reproducible from the seeds (galaxy seed + history seed)

### Open questions

Full agent sim for the entire galaxy, or lighter seed-script for coarse galaxy-wide history with detailed agent sim only in the founding region? The mechanisms support both. The Stage 0 economy sim will inform this — if agents produce interesting history when fast-forwarded, go full agent. If the sim is too expensive or produces boring output, fall back to seed scripts for the periphery.

## Phase 3: Stage 1 — The Trading Game

The first playable thing. Elite (1984) in Apeiron's universe.

### What to build

A single-process game server running 5 founding systems. No federation — one operator, one binary. The server:

- Hosts all 5 systems in-process (separate logical domains, same runtime)
- Runs AI extractors, haulers, stations, shipwrights (see [ECONOMY.md](ECONOMY.md))
- Evaluates physics scripts via embedded Raido (crafting, fuel costs, mass budgets)
- Manages object ownership and transfers (conservation laws enforced internally — same rules as Allgard, just not distributed yet)
- Accepts player connections (simple protocol — doesn't need to be Leden yet)

The client:
- Text-based or simple 2D. Show star map, inventory, market prices, jump menu.
- Compute galaxy data locally from the seed (same Raido script as the server)
- Submit commands: jump, trade, accept job, craft, scout

### What to skip
- Real Leden protocol (use a simpler internal protocol)
- Real Allgard transfer escrow (transfers are local — same process)
- GDL (no spatial representation yet — it's a trading game, not a 3D world)
- Combat (trade, mine, explore first)
- Player-hosted domains (Stage 2)

### What to get right
- **Conservation laws must work from day one.** Even in a monolith, objects should have proof chains. Minting scripts should be verifiable. Mass budgets should balance. This isn't premature — it's the foundation. If conservation doesn't work in a monolith, it won't work distributed.
- **The economy must function.** AI traders should create real supply/demand. Credits should flow. New players should be able to earn through courier jobs, mining, scouting. If the economy is dead or trivially exploitable, fix it before federating.
- **Crafting must feel good.** The interaction function, stoichiometric peaks, facility precision — this is the core discovery loop. If experimentation isn't rewarding, the game doesn't work regardless of infrastructure.

### Validation
- A new player can join, receive a starter ship, take a courier job, earn credits, buy fuel, and jump to another system. The full onboarding loop from [ECONOMY.md](ECONOMY.md).
- AI haulers create visible trade routes. Prices differ between systems. Arbitrage is profitable.
- A player can craft structural steel at a public facility using mined iron and carbon.
- The economy doesn't collapse after 1000 simulated ticks. Credits inflate mildly, not hyperinflate.

### Deliverable
A playable space trading game. Five systems, AI economy, text client. Fun on its own.

## Phase 4: Leden

Federation needs a wire protocol.

### What to build

The networking layer:
- Capability-based sessions (connect, authenticate, exchange capabilities)
- Object operations (create, transfer, observe, query)
- Gossip-based peer discovery (no central registry)
- Content-addressed blob storage (for Raido scripts, blueprints, assets)
- Observation protocol (push-based state change notifications)

### Implementation
Rask. Transport-agnostic — works over TCP, WebSocket, QUIC. MessagePack wire format (already spec'd in [wire-format.md](../leden/wire-format.md)). Leden exercises Rask's networking stack — socket I/O, async patterns, binary serialization.

### Validation
- Two independent processes can establish a Leden session, exchange capabilities, and transfer an object.
- Gossip discovery works: process A knows B, B knows C, A discovers C through gossip.
- Content store: process A publishes a Raido script chunk, process B fetches it by hash.

## Phase 5: Allgard

The federation model becomes runtime code.

### What to build

- Conservation law enforcement as a library (validates Transforms against the 7 laws)
- Cross-domain transfer protocol ([TRANSFER.md](../allgard/TRANSFER.md) — escrow, commit, timeout, recovery)
- Bilateral trust tracking (introduction-based, audit gossip)
- Verifiable minting (Raido script re-execution for supply verification)
- Proof chains (causal ordering, Law 4)

### The key refactor
Extract the Stage 1 monolith's internal conservation enforcement into Allgard. The monolith was enforcing the same rules — now they run across domain boundaries over Leden. The game logic doesn't change. The enforcement boundary does.

### Validation
- Two independent Allgard domains can transfer an object with full escrow protocol.
- A domain that mints recklessly is detected by bilateral audit.
- Partition recovery works: domain goes offline mid-transfer, comes back, transfer completes.
- Conservation laws hold under adversarial conditions (a test harness that tries to violate each law).

## Phase 6: Stage 2-3 — Federation

The monolith splits into sovereign domains.

### What to build

- Each founding system becomes an independent domain process
- Player stations: managed hosting for player-deployed domains within the founding cluster
- Player star systems: players claim stars beyond the founding cluster, run their own domains
- Cross-domain travel using Allgard transfer protocol
- Bilateral trust building through real trading history

### What changes from Stage 1
- Transfers go through Leden escrow instead of in-process moves
- Each domain enforces its own rules on top of universal conservation laws
- Players experience domain boundaries (seamless in the founding cluster, visible at the edges)
- The economy becomes genuinely distributed — no single process sees all state

### Validation
- A player can travel from one founding system to another. Objects transfer correctly.
- A player can deploy an outpost in an unclaimed system and mine resources.
- Two player domains can trade bilaterally without founding cluster involvement.
- A domain going offline doesn't lose objects (partition recovery).

### Deliverable
A federated space trading game. Player sovereignty. The Allgard model working in production.

## Phase 7: Combat Prototype

Parallel track. Can start alongside Phase 3.

### What to build

A standalone combat arena:
- Two fleets, one domain, deterministic Raido combat scripts
- Strategic orders (stance, target, formation, retreat)
- Commit-reveal per beacon tick
- Equipment quality noise ([PHYSICS.md](PHYSICS.md) deterministic noise)
- Damage, stress, component failure (Law 4)
- Retreat mechanics

### Why standalone
Combat is a game design problem that needs rapid iteration. Balancing damage formulas, tuning retreat costs, finding interesting counter-strategies — this is playtesting work. Coupling it to the full economy during development slows both down.

### Integration
When the combat prototype is fun and the trading game is federated, merge them. Ships built from the economy fight using combat scripts. Destroyed ships are gone (conservation). Combat creates material demand. The systems reinforce each other.

## Phase 8: Stage 4 — Full Vision

### What to build

- GDL: spatial representation, regions, entities, progressive enhancement
- Rich clients: 2D galaxy map with docking screens, eventually 3D
- Route domains: space between stars as gameplay space
- Full AI agent ecosystem: AI factions, AI researchers, AI traders at scale
- Player-created content: Raido UGC scripts (fuel-limited)
- Self-hosting: players run their own infrastructure without managed hosting

### This phase is open-ended
Stage 4 is where the game grows beyond what I can prescribe. Player factions, emergent politics, custom physics, player-built worlds. The specs provide the constraints. The players provide everything else.

## What's Not On This Roadmap

**Mobile clients.** The architecture supports any client that can run Raido (for galaxy gen) and speak Leden (for networking). Mobile is a client engineering project, not a protocol project.

**Monetization.** How the founding cluster funds hosting, whether managed hosting charges money, whether there's a real-money economy. Business decisions, not technical ones.
