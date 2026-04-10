# Sensors
<!-- id: apeiron.sensors --> <!-- status: proposed --> <!-- summary: Information asymmetry — domain omniscience, player fog of war, sensors as ship systems -->

A domain operator sees everything in their domain. Every ship, every cargo manifest, every movement. That's sovereignty — you run the server, you see the state. Players visiting a domain don't get that. They see what their sensors can detect.

This asymmetry is the information game. A domain operator knows who's carrying what. A visiting player sees blips on a scanner. The gap between those two views creates gameplay: smuggling, ambushes, scouting, intelligence gathering, and the value of being a domain operator in the first place.

## Domain Omniscience

The domain host runs the simulation. All objects in the domain are in its memory. The operator sees:

- Every ship present (identity, component tree, mass, cargo, equipment)
- Every object (debris fields, deployed structures, cargo containers)
- All movement (vectors, acceleration, fuel burn rates)
- All communications within domain scope (per GDL event scoping)
- All active Grants and capabilities in use

This isn't surveillance — it's architecture. The domain IS the server. Hiding from the domain operator is like hiding from the database.

### What operators can do with omniscience

**Customs inspection.** Know exactly what every visiting ship carries. Impose tariffs, ban contraband, tax specific goods. No scanning needed — the cargo manifest is in the domain's object store.

**Traffic control.** See all ships in real-time. Route traffic, enforce speed limits near stations, manage docking queues. Domain policy, not physics.

**Intelligence.** Track which ships visit, when, carrying what. Build trade flow models. Sell this intelligence (see [KNOWLEDGE.md](KNOWLEDGE.md)). Or use it to set better market prices.

**Selective disclosure.** The operator decides what visiting players can see. A friendly trading hub might share detailed scans of all docked ships. A paranoid military system might reveal only ships within weapons range. The operator controls the information aperture.

### Limits on omniscience

**Other domains' internals.** You see ships that enter your domain, but not what's happening in neighboring domains. Cross-domain intelligence requires either visiting (send a scout) or bilateral sharing (an ally tells you).

**Encrypted cargo.** A ship's component tree is readable (it has to be — the domain validates physics). But cargo contents could be opaque containers — Allgard objects whose internal state is encrypted. The domain sees the container's mass and external properties but not the contents. Whether domains ALLOW opaque cargo is domain policy. The founding cluster might require manifest disclosure; a pirate haven might not.

**Player knowledge.** The operator sees the data but can't read the player's mind. A player who visited three systems and memorized trade prices has knowledge the domain can't extract. Social information stays social.

## Player Fog of War

A visiting player sees what their sensors detect plus what the domain chooses to disclose.

### Baseline: Domain Disclosure

Every domain publishes a minimum information set to all visitors. This is the "what you see when you look out the window":

- **Your own ship.** Full state, always. Your component tree, your cargo, your fuel, your position.
- **Domain infrastructure.** Stations, facilities, planets, asteroid belts — the domain's published GDL regions. These are the domain's public face. Always visible.
- **Public market data.** Prices, available goods, posted contracts — whatever the domain chooses to publish in its market interface.
- **Navigation data.** Jump points, fuel costs, domain policy notices. See [NAVIGATION.md](NAVIGATION.md).

Beyond baseline, what you see depends on your sensors and the domain's disclosure policy.

### Sensor Detection

Sensors are ship systems. They obey the five constraint laws like everything else:

**Law 1 (Mass).** Sensors have mass. Better sensors are bigger. A dedicated scanner array is heavy — you're trading cargo capacity or weapons for information.

**Law 3 (Energy).** Sensors draw power. Active scanning draws more than passive. Running full-spectrum active scans drains your energy budget, potentially starving other systems.

**Law 5 (Coupling).** Sensors are sensitive instruments. They couple with nearby systems — engine heat blinds thermal sensors, reactor EM noise drowns out weak signatures. Shielding between sensors and noisy systems costs mass. A pure scanner ship (few other systems, minimal coupling) detects more than a warship with sensors bolted on as an afterthought.

### What Sensors Detect

Sensor output depends on range, sensor quality, and target properties. The domain computes the result — the player's client receives what their sensors would detect, not what the domain knows.

**Passive detection.** Always on, zero power draw. Detects:
- Large objects (stations, capital ships) at long range
- Active ships (engines firing, weapons hot) at medium range
- Mass signatures — approximate total mass of detected objects
- EM emissions — active scanners, communications, reactor output

**Active scanning.** Powered, directed, reveals more detail:
- Component tree outline (what systems a ship has, not exact specs)
- Cargo volume (how full the hold is, not what's in it)
- Material composition hints (what the hull is made of — relevant for combat)
- Damage state (is the target's hull intact? Any failed components?)

**Deep scanning.** High-power, close-range, conspicuous:
- Full component tree with approximate specifications
- Cargo manifest (item types and quantities)
- Fuel state
- Active Grant/capability list (what the target is authorized to do here)

Each tier requires better sensors, more power, and closer range. Deep scanning is essentially "I am staring at you with a spotlight" — the target knows they're being scanned.

### Detection Is Asymmetric

The domain sees everything. Players see based on sensors. This means:

- **The domain can ambush.** An operator who sees a juicy cargo ship can alert pirates (or BE the pirate). The visitor doesn't see the pirates until sensors detect them.
- **Smuggling is about the domain, not scanners.** You can't hide cargo from the domain operator. Smuggling means either: (a) finding a domain that doesn't inspect, (b) using opaque containers on a domain that allows them, or (c) mislabeling cargo and hoping nobody audits.
- **Scouts are valuable.** A player who enters a domain gets sensor data. Reporting that data to allies outside the domain is intelligence work. The domain can see the scout, but can't prevent them from remembering what they saw.
- **Sensor advantage is ship design.** Two players in the same domain — one with a scout ship (big sensors, minimal coupling) and one with a hauler (small sensors, noisy reactor) — see very different pictures. The scout detects the pirate lying in wait. The hauler doesn't.

## Sensor Properties

Sensors are components in the ship's component tree, with properties derived from their materials:

| Property | Derived from | Effect |
|----------|-------------|--------|
| **Sensitivity** | Material conductivity + radiance | Detection range for passive signals |
| **Resolution** | Material hardness + stability | Detail level at a given range |
| **Power efficiency** | Material conductivity | Power draw per scan cycle |
| **Noise floor** | Coupling with adjacent systems (Law 5) | Minimum detectable signal |
| **Scan rate** | Reactor throughput (Law 3) | How often active scans can fire |

Better materials make better sensors. A sensor array built with gold-catalyst conductors (high conductivity, low noise) outperforms one built with basic copper wire. Material science feeds the sensor game just like it feeds the weapons game.

### Stealth

Stealth isn't a cloaking device. It's engineering to minimize your signature:

- **Low-emission engines.** Less thrust, less waste heat. Slow but quiet.
- **Insulated reactor.** Shielding that contains EM output. Costs mass (Law 5).
- **Small profile.** Less mass = less to detect. A tiny scout is harder to spot than a capital ship.
- **Cold running.** Minimize active systems. No active scans, low power draw. You see less because you're using less energy, but you're also harder to see.

Stealth ships sacrifice capability for concealment. A ship that's hard to detect is also a ship that detects poorly (sensors need power), fights weakly (weapons need power), and carries little (small hull). Tradeoffs from constraint physics, not stealth-specific rules.

### Counter-Stealth

The domain always sees stealth ships (omniscience). Counter-stealth is about PLAYERS detecting stealthy opponents:

- **Sensor arrays.** Bigger, more sensitive, higher power. Brute force.
- **Sensor networks.** Multiple ships sharing detection data. A fleet with a dedicated sensor ship feeds data to combat ships. Requires the social tools for fleet coordination (see [SOCIAL.md](SOCIAL.md)).
- **Active probing.** High-power active scan that's hard to hide from. But the probe is loud — everyone knows you're looking.

## Combat Information Model

Sensors directly affect combat per [COMBAT.md](COMBAT.md). The information model was listed as "deferred." Here's how it integrates:

### Pre-Combat

Before engagement, sensor data determines what you know about the opponent:

- **At range:** Mass estimate, ship class guess (from mass + signature), fleet size. Enough to decide whether to engage or flee.
- **Closer:** Component outline, approximate weapons/defenses. Enough to choose initial orders.
- **Deep scan:** Full component tree. Enough to identify specific weaknesses. But deep scanning requires getting close, which the opponent might not allow.

### During Combat

The combat script receives sensor data as an input. Each tick:

- Both sides see what their sensors detect about the opponent's fleet
- Sensor damage degrades detection (a hit to your scanner array means less information next tick)
- Electronic warfare (jamming) is a power allocation choice: spend reactor output to degrade opponent sensors instead of powering weapons
- The domain computes the full fight; each player's client receives their sensor-limited view

### Fog of War in Combat

A player with damaged sensors might not see an enemy flanking maneuver. Their orders are based on incomplete information. The opponent with better sensors sees more and can exploit the gap.

This creates a sensor arms race alongside the weapons/armor race. A fleet with superior sensors has an information advantage that compounds across ticks — they see the opponent's formation changes faster, react with better-informed orders, and can identify damaged ships to focus fire.

## Electronic Warfare

Sensor systems enable electronic warfare — using your systems to degrade the opponent's information:

**Jamming.** Emit noise across sensor frequencies. Costs power (Law 3). Degrades opponent passive detection within range. Countered by better sensors (higher signal-to-noise ratio) or directional filtering.

**Spoofing.** Emit false signatures. Make one ship look like three, or a warship look like a hauler. Costs power and requires specific equipment (a spoofing array). Countered by deep scanning (the real component tree doesn't match the fake signature).

**EMCON (Emission Control).** Go dark — shut down active emissions. Free (costs nothing, you're just turning things off). Reduces your passive detectability. But also reduces YOUR sensor capability — you can't scan while running silent.

These aren't separate systems from sensors — they're alternative uses of sensor-grade equipment. A sensor array can detect or emit. Switching between modes is a power allocation decision in your strategic orders.

## Stage 1 Testing

The monolith simulates fog of war even though it's one process:

- Each logical domain tracks what each visiting player's sensors can detect.
- Two AI ships in the same domain with different sensor quality should see different pictures of the battlefield.
- Test passive detection ranges: a stealthy ship vs. a loud hauler at various distances.
- Test active scanning: power draw, detection detail at different ranges.
- Test coupling: a sensor next to a reactor vs. a sensor with shielding. Verify noise floor difference affects detection.
- Test combat fog of war: two fleets engage, one has sensor advantage. Verify the advantaged fleet gets better information per tick.
- Test electronic warfare: jamming reduces opponent detection. Spoofing creates false contacts. EMCON reduces own signature and own detection simultaneously.
- Test domain disclosure: domain publishes baseline info. Sensors add detail on top. Verify players with no sensors still see baseline.

## Interaction With Other Systems

**Constraint physics.** Sensors ARE constraint physics components. Mass, energy, coupling — all five laws apply. No special cases.

**Combat.** Sensors feed the combat information model. Sensor damage, jamming, and EW are combat mechanics. The combat script takes sensor state as input per tick.

**Exploration.** In unclaimed systems (client computing from seed), there are no domain-hosted objects to detect. Sensors don't apply — you're looking at math, not physics. Sensors matter in CLAIMED domains where other players' ships are present.

**Knowledge.** Sensor data about other ships and domains is intelligence. A scout selling sensor readings of an enemy fleet's composition is knowledge trading per [KNOWLEDGE.md](KNOWLEDGE.md).

**Navigation.** Sensors help validate navigation data. A domain claims "this route is safe." Your sensors say there are three warships on the route. Trust your sensors. See [NAVIGATION.md](NAVIGATION.md).

## What This Spec Doesn't Cover

**Specific detection formulas.** The exact relationship between sensor quality, range, and detection detail. Tuning parameters — decided through playtesting. The standard physics script encodes them.

**Sensor types.** Thermal, EM, gravitational, optical — these are flavor categories, not distinct physics. The six material properties (density, hardness, conductivity, reactivity, stability, radiance) determine what a sensor can detect. Whether players call a high-radiance sensor "thermal" or "optical" is convention.

**Communication interception.** Can you intercept another player's messages? Within a domain, the operator can see all communications (omniscience). Between domains, messages travel over Leden sessions — encrypted, capability-gated. Interception would require breaking Leden's security model. Not covered here; it's a Leden concern.
