# Apeiron

A federated space game built on [Midgard](../midgard/), [Allgard](../allgard/), [Leden](../leden/), [GDL](../gdl/), and [Raido](../raido/).

The name comes from Anaximander's ἄπειρον — the boundless, the infinite undefined source from which all things emerge. The galaxy is a seed. What exists in it is shaped by those who claim it.

## The Idea

A galaxy of 10,000 stars. Every star is real — generated from a deterministic seed, visible to everyone, always. Some stars are claimed. A claimed star has a domain behind it: a station, a planet, a civilization. The rest are dark — visible, visitable, full of potential, but empty. No authority, no resources, no life. Just math waiting to become something.

There is no central server. No company runs the galaxy. Every claimed star is hosted by someone — a player, a group, a service. They're sovereign over their domain. Their rules, their economy, their physics. The galaxy is the network of these sovereign domains, connected by bilateral trust.

You can't cheat because the math won't let you. You can't inflate because your trading partners verify your minting scripts. You can't dupe because conservation laws are structural. You can't steal because capabilities are unforgeable. The safety is invisible. The galaxy just works.

## The Galaxy

### Seed Generation

One Raido script + one integer seed = the entire galaxy. Every client runs the same script, gets the same stars. Deterministic fixed-point arithmetic guarantees bitwise-identical output on every machine. The script hash is the galaxy's identity.

The seed generates:

| Property | From seed | Example |
|----------|-----------|---------|
| Position | 3D coordinates in galaxy space | `[1204.3, -892.1, 340.7]` |
| Spectral class | Star type | G-class, red dwarf, binary |
| Planet count | How many orbital bodies | 4 planets, 2 moons, 1 asteroid belt |
| Planet properties | Size, composition, atmosphere, orbit | Rocky, iron-rich, thin atmo, 1.2 AU |
| Resource deposits | What's extractable and how much | 50K iron, 200 rare earth, 10 exotic |
| Name | Procedural name | "Korvan," "Tesh-4," "Brightwell" |

The galaxy has structure — not uniform random positions. Dense cores with many neighbors (easy federation, rich trade). Sparse arms with isolated systems (frontier, harder to reach, richer resources). Bridge stars connecting clusters (strategic chokepoints). The topology creates natural gameplay before anyone claims a single star.

### The Map Is Knowledge

You look at the sky and see 10,000 stars. They're all real. But your map has layers:

1. **Seed layer.** Every star, every position. Computed locally. Same for everyone.
2. **Network layer.** Claimed stars. Gossiped through Leden peer metadata. You see what the network tells you.
3. **Personal layer.** Stars you've visited, domains you've connected to. Richer detail — faction, trade data, reputation, who introduced you.
4. **Social layer.** Stars friends have visited. Shared through contact grants.

A new player sees the founding cluster and a sky full of possibilities. A veteran explorer sees a rich map with trade routes, danger zones, and personal history. A hermit miner sees a sparse map with a few trusted partners. The galaxy is different for everyone — not because the data differs, but because knowledge differs.

### No Central Map Authority

Nobody owns the map. Nobody manages coordinates. Each domain claims its position as Leden peer metadata:

```
metadata:
  star_id: 4822
  sector.x: 1210.1
  sector.y: -888.4
  sector.z: 342.2
```

Two domains claim the same star? The network decides through trust — who got introduced first, who has more bilateral relationships. No protocol enforcement. Social consensus, like everything else in Allgard.

## Domain Types

Not everything is a star system. The hybrid model:

### Star Systems

A star system is the primary domain type. The operator hosts everything: planets, stations, asteroid belts, orbital space. Planets and stations are GDL regions within the domain. The system operator is sovereign — their rules, their minting policy, their physics.

Star properties are fixed by the seed. You don't choose your star type or planet count. You choose FROM what the seed generated. Want a binary star system? Find one. Want a system rich in rare minerals? Browse the galaxy data.

### Stations and Starbases

A station can be its own domain. A player builds a trading post orbiting planet 3 of star 4822 — that's a domain, hosted by that player. The star system operator might welcome it (more traffic, more trade) or reject it (sovereignty). Bilateral negotiation.

Stations are smaller, cheaper to host than full star systems. A station might be a single GDL region — a docking bay, a market, some corridors. Perfect for players who want to run something without hosting an entire solar system.

### Ships

Most ships are Allgard objects, not domains. Your starter ship is an object with properties — hull, fuel, cargo capacity, equipment slots. It's hosted on whatever domain you're visiting. When you jump, it transfers.

This is simpler and more practical. No infrastructure needed to fly. Your ship is like your character in a traditional MMO — an object that moves between servers.

Capital ships, mobile bases, and AI trader vessels CAN be domains — full gards with interiors, crew, and sovereignty. GDL nested spaces handle the interior: bridge, cargo hold, crew quarters. A capital ship docked at a station is a sub-domain within that station's space. This is advanced gameplay for invested players, not the default.

### Outposts

A mining outpost, a sensor array, a fuel depot. Lightweight domains that exist to claim resources or provide services. Cheap to host. Might be automated — a Raido script running the extraction, no human operator needed.

Outposts are how you extract resources from unclaimed systems. You can't mine without a domain (no authority = no transforms). An outpost IS the minimum viable domain for resource extraction.

## Resources

### Seed as Geology

The galaxy seed generates what resources EXIST at each star — iron deposits, gas giants, rare minerals, exotic materials. These are deterministic numbers. Anyone can compute them. They're not Allgard objects. They're potential.

Resources don't become objects until a domain mints them. Minting requires a Raido script (Conservation Law 1). The script's inputs are the seed data. The output is resource objects. Anyone can verify by re-running the same script against the same seed.

### Extraction Loop

1. **Scout.** Visit an unclaimed system. Read the seed data. Learn what's there. Free — just evaluating a function. No domain needed.
2. **Claim.** Deploy a domain — an outpost, a station, a full system claim. This costs real resources: hosting compute, bandwidth, maintenance.
3. **Extract.** The domain runs a minting script: "star 4822, belt 3 contains X iron. I'm minting Y units." The script references the seed data and previous extraction proofs.
4. **Trade.** Extracted resources are Allgard objects. They transfer, trade, and craft like anything else. Conservation laws apply.
5. **Maintain.** Keep hosting the domain. Stop hosting, the domain goes dark. Your claim lapses. Someone else can claim the star.

### Finite Deposits

The seed encodes total extractable resources per body. A minting script must reference previous extraction proofs:

```
total_iron = galaxy_seed_resource(star: 4822, body: 3, resource: "iron")
// → 50,000 units (deterministic from seed)

already_extracted = sum(verified_extraction_proofs)
// → 32,000 units (from proof chain)

available = total_iron - already_extracted
// → 18,000 units
```

Two domains mining the same belt must exchange extraction proofs (bilateral, via Leden). If they refuse to coordinate and over-extract, their trading partners catch it during audit: the total received exceeds what the seed allows. Trust collapses for the cheater.

### Hosting as Cost

Running a domain costs real compute and bandwidth. This IS the resource cost. A mining outpost in a remote system costs electricity to run. The resources extracted must justify the hosting cost. This creates genuine economic pressure — not an artificial game sink, but a real constraint.

Rich unclaimed systems exist. Everyone can see them in the seed data. But claiming them means running infrastructure far from established trade routes, with few neighbors for trust. Risk and reward from pure architecture.

## Travel

### Jumping

Travel between domains is jumping between star systems. The jump IS the Allgard transfer protocol. Your objects transfer via bilateral escrow. The escrow process is the voyage — narratively, you're in transit. Mechanically, the source domain commits the departure, the destination domain registers the arrival.

Within established networks (founding cluster, alliances), jumping is near-instant. 1-3 Leden round trips with promise pipelining. The jump animation is longer than the actual transfer.

Visiting unclaimed systems is like looking through a telescope. Your ship object stays hosted on the last domain you visited. The client computes the unclaimed system from the seed — you see what's there, evaluate resources, decide whether to claim. To DO anything, you deploy an outpost (a new domain). Your ship transfers to it.

### Route Domains

The space between stars matters. A trade lane between Star A and Star B can be a lightweight domain — a long, thin GDL region with procedural starfield. Encounters happen here: pirates, traders, anomalies, salvage. Route domains are cheap to host (mostly empty space + occasional events).

Route domains are optional. Two stars can connect directly via jump. But route domains add gameplay: chokepoints, ambush zones, patrol routes, escort missions. A player or faction that hosts the route between two major trade hubs controls that corridor.

### The Void

Stars with no route domains between them connect via direct jump. The void between them is cosmetic — the client renders a procedural starfield during the jump animation. No gameplay happens in the void. No authority, no entities, no encounters.

This is honest. Don't simulate what you can't host. The void is a skybox, not a world.

## The Founding Cluster

The first 5-20 star systems. Operated by the project. Shared standard asset types, shared currency, seamless travel between all of them. This is the product on day one.

The founding cluster picks their stars from the seed — probably dense-core systems with good connectivity. They establish the baseline: what a ship looks like, what common resources are, how currency works, what the standard crafting recipes are.

New players start at a founding cluster system. They get a ship (an Allgard object — not a domain). They can trade, explore, mine, build. When they're ready, they claim their own star and join the network.

The founding cluster is the seed of trust. Every domain in the galaxy traces its introduction chain back to the founding cluster. The cluster members start as each other's trusted partners. Everyone else earns trust through introduction.

## Natural Laws

Allgard's seven conservation laws keep the economy honest. Natural laws keep the game interesting. Conservation laws say "you can't cheat." Natural laws say "the universe has friction."

The founding cluster publishes a **standard physics script** — a content-addressed Raido script that encodes these laws as formulas. Domains that run standard physics include the script hash in their departure proofs. Receiving domains verify: standard physics? Trust the proof. Non-standard? Flag it. Not banned — just transparent. A domain running zero-gravity economics is visible to everyone in the proof chain.

### Law 1: Distance Costs

Moving between stars consumes fuel proportional to distance. The galaxy seed determines star positions. Both departure and arrival domains know the exact distance. The departure domain deducts fuel and includes the calculation in the departure proof.

Fuel consumed is destroyed — a value sink (Conservation Law 3). Fuel burning IS the economy's metabolism. Without it, resources accumulate forever and trade stalls.

If you don't have enough fuel to reach any star, you're stranded. Call for rescue (another player delivers fuel), deploy an outpost (if you have materials to start extracting), or wait for an AI trader to pass through. Getting stranded in deep space is a real risk. That's the point — exploration has stakes.

### Law 2: Mass Is Real

Things have weight. Ship + cargo = total mass. Fuel cost scales with mass: a full hauler costs more to move than an empty scout.

```
fuel_cost = base_rate * (distance / unit) * (1 + cargo_mass / ship_capacity)
```

This creates the trader's dilemma: full cargo = expensive trip, empty = wasted trip. Optimal load is a real calculation. Heavier cargo means higher margins must justify higher fuel. Lightweight luxury goods might beat bulky raw materials on profit-per-fuel.

Standard types have standard mass. A domain that mints "weightless titanium" is visible — other domains compare against standard type definitions. Non-standard mass = non-standard trust.

### Law 3: Entropy

Things decay. Hull degrades per jump. Equipment degrades per use. The domain applies decay when processing transforms. Object properties track cumulative wear.

Decay creates demand. Without it, a ship built once lasts forever and the maintenance economy doesn't exist. With it, every player needs repair materials, creating a constant resource flow.

A domain that doesn't apply decay is visible in proof chains — "500 jumps, 100% hull" is obvious. Trust flag.

Tuning matters: decay too fast is tedious, too slow is meaningless. The standard physics script sets rates. Founding cluster stations offer cheap repairs for starter ships — new players shouldn't death-spiral from decay before they learn to trade.

### Law 4: Scarcity Is Geographic

The seed distributes resources unevenly. No single star has everything. This isn't enforced by domains — it's math. The seed is deterministic and public. Everyone can verify what resources exist where.

Geographic scarcity makes trade necessary. A fuel-rich system needs metals. A metal-rich system needs organics. Nobody is self-sufficient. The topology of need IS the trade network.

### What Natural Laws Create

| Law | Creates | Without it |
|-----|---------|-----------|
| Distance costs | Trade routes, logistics, geography matters | Everyone is everywhere, no spatial game |
| Mass is real | Trader's dilemma, ship specialization, escort missions | Infinite cargo, no tradeoffs |
| Entropy | Maintenance demand, resource flow, economic heartbeat | Accumulate forever, economy dies |
| Geographic scarcity | Trade necessity, interdependence, exploration value | Self-sufficient hermits, no network |

Every law forces interaction. Distance creates traders. Mass creates specialists. Entropy creates demand. Scarcity creates partners. A game without friction is a sandbox without purpose.

### Verification, Not Enforcement

Natural laws aren't enforced by a central authority. They're verified bilaterally — the same way conservation laws work. The standard physics script is public. Departure proofs include the script hash. Any domain can re-execute and verify.

A domain that runs non-standard physics isn't banned. It's transparent. Other domains decide whether to trust ships arriving from a zero-fuel-cost domain. Usually they won't — the same way you wouldn't trust someone who claims they walked across the ocean.

## AI Agents

### The Galaxy Runs Itself

The galaxy doesn't need human critical mass to be alive. AI agents ARE the network at launch.

Allgard doesn't care what an Owner is. It's a cryptographic identity. Human, bot, LLM — conservation laws constrain everyone equally by structure, not policy. An AI agent running a mining outpost is a real domain operator. It holds real objects. It trades bilaterally. It builds real trust. The network can't tell it's not human, and doesn't need to.

At launch, AI operates everything:

- AI runs founding cluster systems — managing prices, spawning NPCs, handling visitors
- AI operates mining outposts, extracting real resources from seed-generated deposits
- AI traders haul cargo between systems, creating real trade routes and real demand
- AI stations set prices based on actual supply and demand
- AI agents build trust through real introductions and real transaction history

Human players don't join an empty galaxy. They join a functioning economy. The first trade a new player makes is with an AI merchant who has real inventory, real prices, and a real reputation.

### No Special Constraints

The seven conservation laws constrain AI the same way they constrain humans:

1. Can't mint without a verifiable script
2. Can't own something in two places
3. Can't create value from nothing
4. Can't skip causality
5. Can't spam beyond rate limits
6. Can't act beyond granted authority
7. Can't mass-produce trusted identities

No AI-specific rules. No policy layer. No "please be nice." The math doesn't care what you are.

An AI that plays by the rules can run 100 outposts, trade optimally across 50 systems, and operate 24/7. None of that is cheating. Humans respond the way humans always respond to automation — move up the value chain. AI mines commodities. Humans build places worth visiting.

Domain operators who want to restrict AI on their domain can. Sovereignty. Others welcome AI traders. Players choose.

### Galaxy Evolution

| Phase | State |
|-------|-------|
| Launch | AI runs everything. 50-200 AI domains. Functioning economy. |
| Early | Humans join. Trade with AI. Start claiming stars. |
| Growth | Humans displace AI in high-value systems. AI retreats to frontier and commodity work. |
| Mature | Humans run the interesting parts. AI runs the infrastructure. |

### AI as Test Suite

The AI network tests the full stack before a single human plays. Conservation laws, transfer protocol, trust model, audit gossip — all exercised by AI agents trading, mining, and occasionally cheating (to verify detection works). If the system can constrain autonomous agents with no moral compass, it can constrain anything.

## Events and Evolution

### The Seed Evolves Without Changing

The galaxy seed is immutable. One number, one galaxy, forever. But the scripts that READ it can be updated.

A new Raido script published by the founding cluster reads the same seed and reveals new information — a hidden asteroid belt, a wormhole, a new resource type. The data was always "in" the math. Nobody computed it until now. Like astronomy — the stars were always there, we built better telescopes.

Content updates are new reader scripts, not new seeds. The founding cluster publishes the script hash. Domains adopt it voluntarily.

Time-gated reveals are also possible — the generation script takes an epoch parameter:

```
// Stars 0-8000: visible from epoch 0
// Stars 8001-9000: visible after epoch 1
// Stars 9001-10000: visible after epoch 2
func generate_star(id, epoch) {
    if id > epoch_threshold(epoch) {
        return nil
    }
    // ...
}
```

New regions of the galaxy light up on a schedule. Deterministic. No server push. Every client computes the same reveal.

### Coordinated Events

The founding cluster is the closest thing to "the developers." They can publish new standard types (a new ship class, a new resource), run coordinated events across all founding systems (an invasion, a trade crisis), or update standard scripts (new crafting recipes, rebalanced formulas). Other domains adopt voluntarily — like web standards, not forced patches.

Any alliance of domains can do the same. The founding cluster just goes first.

### Emergent Events

The most interesting events aren't designed. They emerge:

- A faction blockades a bridge star, cutting trade between clusters
- A miner discovers a rich unclaimed system and word spreads
- An alliance goes to war — route domains become contested
- A pirate group raids a trade lane, merchants hire escorts
- A domain operator goes rogue, trust collapses, refugees flee
- Someone builds something extraordinary and people come to see it

These aren't features. They're what happens when sovereign actors with different interests share a galaxy.

## What This Stack Gives Apeiron

Everything below is infrastructure. Apeiron doesn't build these — it uses them.

| Need | Provided by |
|------|-------------|
| Deterministic galaxy generation | Raido (fixed-point, content-addressed) |
| Object ownership and transfer | Allgard (conservation laws, bilateral escrow) |
| Verifiable resource extraction | Allgard Law 1 + Raido minting scripts |
| Cross-domain travel | Midgard patterns (asset fidelity, sealed transfer, leased transfer) |
| Trust and reputation | Allgard (introduction-based, audit gossip) |
| Capability-based access | Leden (sessions, delegation, revocation) |
| World rendering | GDL (regions, entities, affordances, progressive enhancement) |
| Real-time spatial | GDL spatial protocol (motion, zones, interest management) |
| Styling and atmosphere | GDL-style (tokens, themes, mood) |
| Client-side prediction | GDL client scripts (Raido bytecode) |
| Asset storage and delivery | Leden content store (content-addressed, chunked) |
| Peer discovery | Leden gossip (no registry) |
| Lockstep combat | Midgard (deterministic, verifiable) |
| Player creation / scripting | Midgard (Raido UGC, fuel-limited) |
| AI agents | Midgard (conservation-constrained, capability-scoped) |

## Roadmap

Each stage is playable and fun on its own.

**Stage 1: A space trading game.** 5 founding cluster systems, hand-designed. AI economy. Fly, trade, mine. Text or simple 2D client. One operator runs everything. This is Elite (1984) — and that was a great game.

**Stage 2: Player stations.** Players deploy stations in founding cluster systems via managed hosting. Set prices, sell goods, attract visitors. First taste of sovereignty.

**Stage 3: Player star systems.** Players claim stars beyond the founding cluster. Federation activates — bilateral trust, introductions, the Allgard model. The galaxy expands.

**Stage 4: The full vision.** Self-hosting. Route domains. Factions. The 10,000-star galaxy. Player-created content. Full AI agent ecosystem.

## Open Questions

**Galaxy size.** 10,000 is the working number. Big enough for exploration, small enough for every star to potentially matter. Needs playtesting. Could be 5,000 or 50,000.

**Star system scale.** How detailed is the procedural generation per system? Just planet count and resources? Or full orbital mechanics, atmospheric composition, terrain seeds? More detail = richer gameplay but heavier seed script.

**Combat model.** Combat happens within domain jurisdiction (systems, route domains). Domain runs Raido combat scripts, both sides verify. Looting works through consent-on-entry — entering a PvP domain grants the domain limited authority over combat consequences. Needs detailed design.

**Faction mechanics.** Alliances of star systems. Probably just bundles of bilateral grants (Midgard's "unions" pattern). But do we need more structure? Shared defense, territory claims, faction wars?

**Economy bootstrapping.** What do founding cluster systems mint? How does the initial currency get distributed? What's the first thing a new player can trade?

**Client experience.** What does the minimum viable client look like? A text client showing star names and jump menus? A 2D galaxy map with docking screens? GDL supports all of these — which is the Stage 1 target?

**Managed hosting.** Players who can't self-host need easy domain deployment. Click-button hosting with migration to self-hosted hardware later. Critical for adoption. Contradicts nothing — sovereignty is about control, not where the server physically runs.
