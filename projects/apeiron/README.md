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

A ship is a gard. The player's ship-domain has authority over itself — its interior, its cargo, its crew. GDL nested spaces handle the interior: the bridge, the cargo hold, crew quarters. When the ship is docked at a station, it's a sub-domain within that station's space. When it's in open space, it's a standalone domain.

Ships make the "your home travels with you" pattern literal. Your ship IS your home domain. Your stuff is always with you because your domain is always with you.

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

To unclaimed systems, you can jump in your ship (your ship is its own domain — it doesn't need a receiving domain). You arrive at an empty procedural system. You can look around. You can't extract without claiming.

### Route Domains

The space between stars matters. A trade lane between Star A and Star B can be a lightweight domain — a long, thin GDL region with procedural starfield. Encounters happen here: pirates, traders, anomalies, salvage. Route domains are cheap to host (mostly empty space + occasional events).

Route domains are optional. Two stars can connect directly via jump. But route domains add gameplay: chokepoints, ambush zones, patrol routes, escort missions. A player or faction that hosts the route between two major trade hubs controls that corridor.

### The Void

Stars with no route domains between them connect via direct jump. The void between them is cosmetic — the client renders a procedural starfield during the jump animation. No gameplay happens in the void. No authority, no entities, no encounters.

This is honest. Don't simulate what you can't host. The void is a skybox, not a world.

## The Founding Cluster

The first 5-20 star systems. Operated by the project. Shared standard asset types, shared currency, seamless travel between all of them. This is the product on day one.

The founding cluster picks their stars from the seed — probably dense-core systems with good connectivity. They establish the baseline: what a ship looks like, what common resources are, how currency works, what the standard crafting recipes are.

New players start at a founding cluster system. They get a ship (their first domain). They can trade, explore, mine, build. When they're ready, they claim their own star and join the network.

The founding cluster is the seed of trust. Every domain in the galaxy traces its introduction chain back to the founding cluster. The cluster members start as each other's trusted partners. Everyone else earns trust through introduction.

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

## Open Questions

**Galaxy size.** 10,000 is the working number. Big enough for exploration, small enough for every star to potentially matter. Needs playtesting. Could be 5,000 or 50,000.

**Star system scale.** How detailed is the procedural generation per system? Just planet count and resources? Or full orbital mechanics, atmospheric composition, terrain seeds? More detail = richer gameplay but heavier seed script.

**Ship-to-ship combat.** Two ships meet in a route domain or at a system edge. Who has authority? The route domain operator? Lockstep between the two ship-domains? Needs design.

**Faction mechanics.** Alliances of star systems. Probably just bundles of bilateral grants (Midgard's "unions" pattern). But do we need more structure? Shared defense, territory claims, faction wars?

**Economy bootstrapping.** What do founding cluster systems mint? How does the initial currency get distributed? What's the first thing a new player can trade?

**Client experience.** What does the minimum viable Apeiron client look like? A text client showing star names and jump menus? A 2D galaxy map with docking screens? A full 3D space flight sim? GDL supports all of these, but which is the target?

**The name "Apeiron" for the galaxy.** Is Apeiron the game, or the galaxy within the game? If the galaxy, the game might need its own name. Or they're the same thing — the game IS the galaxy.
