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

### Detection Model

Every object in a domain has a **signature** — how detectable it is. Every sensor has a **detection strength** — how well it can pick up signatures. Detection happens when the signal exceeds a threshold at the given distance.

**Signature.** Derived from the target's physical state:

```
signature = k_mass * mass^0.5 + k_power * power_output + k_emit * active_emissions
```

- `mass^0.5`: Bigger objects are more detectable. Square root because mass grows faster than cross-section.
- `power_output`: Running systems emit waste energy. A ship with a hot reactor and firing engines is loud.
- `active_emissions`: Active scanners, communications, jamming — deliberate EM output. Loudest signal.

A cold, drifting, small ship has low signature. A capital ship burning hard with active scanners has high signature.

**Detection strength.** Derived from sensor properties:

```
detection_strength = (sensitivity * resolution) / noise_floor
```

Where sensitivity, resolution, and noise floor come from the sensor component's materials and coupling environment (see Sensor Properties below).

**Detection check.** Signal strength falls off with distance squared:

```
received_signal = target.signature / distance²
detected = received_signal > (threshold / detector.detection_strength)
```

Three thresholds determine what the detector sees, each a constant in the standard physics script:

| Tier | Threshold | What you learn | Sensor cost |
|------|-----------|----------------|-------------|
| **Contact** | `T_contact` (lowest) | Exists, approximate bearing, mass estimate | Passive — zero power |
| **Outline** | `T_outline` | Component categories (has weapons, has cargo), damage state, hull material class | Active — draws power per scan |
| **Inspection** | `T_inspect` (highest) | Full component tree with specs, cargo manifest, fuel state | Deep — high power, close range, conspicuous |

At a given distance, your sensor can achieve the highest tier whose threshold your received signal exceeds. Beyond maximum detection range, you see nothing.

**Maximum detection range** for a tier:

```
max_range = sqrt(target.signature * detector.detection_strength / threshold)
```

A scout ship with detection_strength 500 trying to get a Contact on a hauler with signature 200:

```
max_range = sqrt(200 * 500 / T_contact)
```

If `T_contact = 1.0`, that's range ~316. The same scout trying to Inspect the same hauler (T_inspect = 50.0): range ~44. Much closer.

**Active scan power cost.** Outline and Inspection tiers require active scanning. Each scan cycle costs:

```
scan_power = base_draw / sensor.power_efficiency
```

Drawn from the ship's energy budget (Law 3). Running continuous active scans competes with weapons, shields, and engines for reactor output.

**The target knows.** Active scans are emissions. The target's sensors detect YOUR scan as `active_emissions` in your signature. Deep scanning a stealthy ship reveals you as much as it reveals them.

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

Sensor equipment has three operating modes: detect, jam, and spoof. Switching modes is a power allocation decision in strategic orders — one sensor array can't do all three simultaneously.

**Jamming.** The sensor array emits broadband noise, raising the effective noise floor for opponents within range.

```
jammed_noise_floor = opponent.noise_floor + jam_power / distance²
```

This directly degrades the opponent's `detection_strength` (since detection_strength = sensitivity * resolution / noise_floor). Jamming at close range can blind an opponent's sensors enough to drop them from Outline to Contact, or from Contact to nothing.

Costs power proportional to jam effectiveness. Countered by: better sensors (higher base sensitivity overcoming the raised floor), distance (jam power falls off with distance²), or directional filtering (sensor components with high stability resist broadband noise).

**Spoofing.** The sensor array emits false signatures. Adds phantom contacts to the opponent's detection:

```
ghost.signature = spoof_power * spoofing_array.resolution
ghost.apparent_mass = declared_value  // attacker chooses what the ghost "looks like"
```

Opponents detect ghosts as real contacts at Contact tier. At Outline tier, ghosts hold up if the spoofing array's resolution exceeds the opponent sensor's resolution. At Inspection tier, ghosts fail — there's no real component tree behind them. Deep scanning always breaks spoofing.

Costs power and requires a spoofing array (specialized sensor component). A fleet with one spoofer can make itself look like three fleets — until the opponent gets close enough to inspect.

**EMCON (Emission Control).** Shut down active emissions. Sets `power_output` contribution to signature to zero and `active_emissions` to zero. Free — no power cost, you're just turning things off.

```
emcon_signature = k_mass * mass^0.5  // mass term only, no power/emission terms
```

Dramatically reduces signature. But also: no active scans (can't power the sensor), no jamming, no spoofing. You're blind AND quiet. Useful for ambushes — go dark, wait, power up weapons at close range.

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

**Threshold values.** The specific values for `T_contact`, `T_outline`, `T_inspect`, and the signature constants `k_mass`, `k_power`, `k_emit`. These are tuning parameters in the standard physics script — decided through playtesting.

**Sensor types.** Thermal, EM, gravitational, optical — these are flavor categories, not distinct physics. The six material properties (density, hardness, conductivity, reactivity, stability, radiance) determine what a sensor can detect. Whether players call a high-radiance sensor "thermal" or "optical" is convention.

**Communication interception.** Can you intercept another player's messages? Within a domain, the operator can see all communications (omniscience). Between domains, messages travel over Leden sessions — encrypted, capability-gated. Interception would require breaking Leden's security model. Not covered here; it's a Leden concern.
