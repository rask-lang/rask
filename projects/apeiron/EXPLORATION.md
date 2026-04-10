# Exploration
<!-- id: apeiron.exploration --> <!-- status: proposed --> <!-- summary: How galaxy knowledge works — sky data from the seed, geology from beacon-gated claim surveys -->

The galaxy is a seed. The seed determines what's POSSIBLE. What's ACTUAL is determined by who shows up.

## Two Layers of Knowledge

### Sky — Free, From the Seed

Star positions, spectral classes, planet counts. Computable by anyone from the public seed. This is your sky map — the same for everyone, always.

```
sky = generate_star(seed, star_id)
// → position, spectral class, planet count
// Public. Permanent. No beacon required.
```

The sky generation function IS public. Anyone can read it, run it, map all 10,000 stars. That's fine — you're looking at the sky through a telescope. The sky is free. But it doesn't tell you what's on the ground.

Sky data carries rough implications. A G-type star with rocky planets is more likely to have iron. A system near the dense core probably has common elements in quantity. A red dwarf with a single gas giant might be hydrogen-rich. These are informed guesses from spectral analysis, not measurements. Enough to pick a direction. Not enough to commit hosting costs.

### Survey — Claim Time Only

Specific deposit data. Element types, quantities, qualities, extraction sites per body. Created once at claim time. Permanent.

```
geology = survey(seed, star_id, body_id, beacon_value)
// → specific deposits, quantities, quality, extraction sites
// Requires beacon. Created at claim time. Permanent.
```

The geology is permanent once created. It's published as part of the domain's metadata. Verifiable by anyone: re-run the function with (seed, star_id, body_id, beacon_value_at_claim), get the same result. The beacon value is in the public beacon log. The claim Transform proof timestamps when it happened.

### Why No Prospect Layer

I considered a prospect layer — beacon-gated scanning that produces probability ranges, costing fuel. Scouts would sell prospect data. But the beacon value is public after the tick. Anyone can run the prospect function offline with a known beacon value, for free, no fuel. The beacon prevents pre-computation but not post-computation. For any function where the output has value AS INFORMATION, post-computation makes it free.

Crafting is protected because you still need real materials. Surveying is protected because you've already committed to a claim. A pure-information prospect layer would have neither protection — the information IS the product, and it's free to compute.

Expeditions (see below) solve this differently. Their primary output is PHYSICAL — samples are Allgard objects that can't be post-computed into existence. The information (scan data) is a secondary output that's post-computable but backed by physical samples. The scout's value isn't exclusive access to information — it's being the first to produce verified samples and reports, and the physical effort required to do so.

## Why the Beacon Is the Key, Not Noise

The beacon value isn't a perturbation on readable data. It's a required input to a cryptographic construction. The function mixes seed and beacon such that neither alone produces meaningful output.

The seed constrains the distribution — what elements are POSSIBLE, their probability ranges. The beacon collapses the distribution into specific values. Like a keyed hash: the message (seed) and the key (beacon) are both required. Remove either and the output is garbage.

This means:
- **Reading the script gives you nothing.** The function is public Raido bytecode. Understanding every line doesn't help because the output depends on a beacon value that doesn't exist until someone claims.
- **Running the script without a beacon gives garbage.** Not "slightly wrong" — meaningless. The beacon is load-bearing, not decorative.
- **After the claim, the result is the truth.** Not an approximation, not a noisy sample. The actual, permanent, verifiable geology.
- **Post-computation is fine for surveys.** The beacon value is public. Anyone can verify a domain's geology. That's the point — geology is published, verifiable, permanent. There's nothing to game because the claimer committed before the beacon.

## Collapse on Claim

Geological collapse is tied to claiming (deploying a domain).

**Why not collapse on first scan?** Race conditions. In a federated system with no global state, "who scanned first" is a distributed consensus problem. Two scanners at different domains, different epochs, same location — who wins? Solvable, but unnecessary complexity.

Collapse on claim avoids it entirely. Claiming is a heavyweight operation — you're deploying real hosting infrastructure. There's no race because domain deployment is visible in the network (Leden gossip). Two domains can't claim the same star because the network resolves competing claims through bilateral trust (who has more relationships, who got introduced first — see [README.md](README.md#no-central-map-authority)).

**What this means:**
- Before anyone claims a star: only sky data exists. Rough probabilities, not facts.
- The moment someone claims: geology manifests. Specific, permanent, verifiable.
- The claimer doesn't choose their geology. They committed to the claim before the beacon tick. The beacon (unpredictable) determines what manifests.
- Good sky data ("probably titanium") might collapse into great geology or mediocre geology. Risk and reward from the same mechanism.

## The Claiming Decision

Without a prospect layer, claiming is a bet. You have sky data — spectral analysis, planet types, rough composition models. You pick a star that looks promising and commit real hosting costs. The beacon determines what you get.

This is honest. Real prospectors spend money on claims based on geological models, not ground truth. Sometimes the model is right and you hit a rich deposit. Sometimes it's wrong and you eat the cost. The sky data makes it an informed gamble, not a blind one.

The information that narrows risk is SOCIAL, not computational:
- **Domain owners who've claimed nearby systems** know their own geology. A neighbor with rich iron might share that the region is generally iron-rich — or might not (competitive advantage).
- **Trade data** reveals what resources are abundant or scarce in a region. If every system in sector 7 exports titanium, the sector probably has more.
- **Faction intelligence** pools members' survey data. A faction with 20 claimed systems has a much better model of regional geology than a solo player.

None of this is beacon-gated. It's human knowledge, traded bilaterally, valued by trust. The kind of information advantage that can't be computed on a laptop.

## Verification

Anyone can verify a claimed system's geology:

1. Check the claim Transform proof (timestamps, beacon epoch, domain signature)
2. Look up the beacon value for that epoch (public beacon log)
3. Re-run `survey(seed, star_id, body_id, beacon_value)` locally
4. Compare output to the domain's published geology

If it doesn't match, the domain fabricated its geology. Trust flag. The proof is cryptographic — no judgment call, no reputation heuristic. The math either checks out or it doesn't.

## Domain Owner Knowledge

A domain owner knows their own system's complete geology. They ran the survey. They have the data. This is a real advantage — they know exactly what's extractable, where, and how much.

This is intentional. Owning a system SHOULD mean knowing it intimately. The knowledge advantage is part of what makes claiming valuable. It also creates information asymmetry in trade — a domain owner selling "iron-rich system" might be underselling a tungsten deposit they haven't disclosed. Buyer beware. Due diligence means checking the published survey against the verification function.

## Expeditions

Sky data tells you what a system MIGHT have. Claim survey tells you what it DOES have. Expeditions fill the gap: go to an unclaimed system, learn more than sky data reveals, bring back something physical. Without deploying a domain.

### The Architecture Problem

No domain at the destination means no authority, no Transforms, no minting. Your ship is hosted on the departure domain. The unclaimed system is math. How do you interact with math?

**The departure domain is the authority.** Your ship never physically leaves. The expedition is an operation the departure domain performs: it runs the expedition script against the target system's seed data, using your ship's scanner equipment as an input. The departure domain mints any physical outputs.

Narratively, your ship flew to star 4822, scanned three planets, collected samples, and came home. Mechanically, the departure domain ran a computation that consumed your fuel and scanner-time, and produced outputs.

This is consistent with the existing model. Line from the README: "your ship object stays hosted on the last domain you visited." Expeditions make that static waiting period into active gameplay.

### Expedition Flow

1. **Depart.** Player requests an expedition to star 4822 from their current domain. Domain checks: does the ship have enough fuel for the round trip? Does it have a scanner? If yes, the domain locks the ship in expedition state and records a **departure proof** (timestamped, signed, committed before the next beacon tick).

2. **Scan.** After the beacon tick, the departure domain runs the expedition script:

```
results = expedition_scan(
    seed,
    target_star_id,
    body_id,
    beacon_value,          // beacon that ticked after departure commitment
    scanner_quality,       // from ship's scanner component
    fuel_allocated         // how much scanning fuel the player committed
)
// → element_estimates[], confidence_ranges[], sample_manifest
```

The departure commitment means the player can't pick a beacon value that produces favorable results. They committed to the target before the beacon ticked.

3. **Produce outputs.** The departure domain mints two things:

**Scan data** — an expedition report (knowledge object per [KNOWLEDGE.md](KNOWLEDGE.md)) containing element estimates with confidence ranges. This is information. It IS post-computable — anyone with the departure proof and beacon value can re-run the function. But the explorer generated it first, and the report is a verified knowledge object with proof chain.

**Physical samples** — small Allgard objects representing material collected from the system's surface. A few units of probable elements, minted by the departure domain.

4. **Return.** Expedition timer expires (proportional to distance — farther systems take longer). Ship returns to normal state with scan data and samples in cargo. Fuel consumed.

### Why Samples Resist Post-Computation

Information is free to copy. Objects aren't. Anyone can re-run the expedition script and learn what samples the expedition WOULD produce. But they can't mint the sample objects without their own expedition (fuel, scanner, departure proof, departure domain willing to run the script).

A sample of iron ore from star 4822 is an Allgard object with mass, a proof chain, and economic value. It's a few units — not enough to build anything, but enough to analyze. Run experiments on it to confirm properties, test it as a crafting input, verify it matches expected deposits. The sample is physical proof you went there.

Samples are minted at reduced quantity (surface collection, not extraction). The expedition script produces maybe 5-20 units per body scanned, depending on scanner quality and fuel spent. Full extraction (thousands of units) still requires a domain claim and proper mining infrastructure.

### Scan Accuracy

Expedition scans don't reveal the true geology — that collapses at claim time. They reveal ESTIMATES with confidence ranges, shaped by scanner quality (deterministic noise model per [PHYSICS.md](PHYSICS.md)):

| Scanner quality | Element detection | Quantity estimate | Confidence |
|----------------|-------------------|-------------------|------------|
| Basic | Common elements only | ±60% | Low |
| Standard | Common + strategic | ±35% | Moderate |
| High-precision | All elements including exotic | ±15% | High |

A basic scanner at star 4822 might report: "Body 3: iron detected (25K-75K units), carbon detected (10K-35K units), possible strategic element (unidentified)."

A high-precision scanner: "Body 3: iron (38K-52K units, high confidence), carbon (18K-24K units, high confidence), titanium (6K-8K units, moderate confidence), possible trace gold (low confidence)."

The claim survey later reveals: "Body 3: 48,200 iron, 21,000 carbon, 7,100 titanium, 300 gold."

The high-precision expedition was close. The basic expedition was directionally correct but vague. Both were worth doing before committing hosting costs to claim.

### What Expedition Scans DON'T Reveal

- **Extraction site count and locations.** Claim-level detail.
- **Exact quantities.** Always a range, never a number.
- **Deposit quality.** Quality collapses at claim time.
- **Exotic element certainty.** Exotic elements (5-15% availability) may appear as "possible trace" even with good scanners. Confirming exotics requires either a very expensive expedition (lots of fuel, top scanner) or claiming.

This preserves the claiming gamble. Expeditions reduce the gamble from "blind bet based on spectral class" to "informed bet based on physical samples and detailed scans." But uncertainty remains — especially for the valuable rare deposits.

### Multi-System Expeditions

An expedition can target multiple stars in sequence. Each additional star costs fuel (round-trip distance from the previous target). The expedition script runs once per body scanned. The player allocates their fuel budget across targets — scan 3 stars shallowly or 1 star deeply.

The departure domain runs the full sequence and mints all outputs at the end. The expedition timer is the sum of all travel times plus scan times. Long multi-system expeditions lock the ship for many ticks — risk of the departure domain changing policy while you're out.

### Expedition Economics

**Cost.** Fuel for the round trip (proportional to distance × ship mass), scanner wear (deterministic degradation per scan cycle), and opportunity cost (ship locked for the expedition duration).

**Revenue.** Sell the expedition report as a knowledge object. Sell the samples to researchers or prospective claimers. Take expedition contracts from factions that want regions scouted.

**Profession.** A player with a lightweight ship (small hull, big scanner, big fuel tank, minimal weapons) makes a good scout. Low travel cost (light ship), high scan quality (good scanner), long range (big tank). Scouts are the exploration profession — they don't claim systems, they sell the information that helps others decide what to claim.

Scouts compete with each other on quality (better scanner = tighter estimates), coverage (more systems per expedition = broader reports), and speed (reaching frontier systems before other scouts). A faction's scout fleet is a strategic asset.

### Anomalies

The expedition script includes anomaly detection. Anomalies are hidden features of the galaxy — not in the sky data, only detectable through physical scanning.

```
anomalies = detect_anomalies(seed, star_id, scanner_quality, fuel_spent, beacon_value)
// → list of anomaly types and approximate locations, or empty
```

Anomaly types (examples — the founding cluster defines these and can add more via updated scripts):

- **Dense deposits.** Unusually concentrated resource nodes. Much richer than normal for the star type. Increase the value of claiming this system.
- **Gravitational anomalies.** Affect jump fuel costs to/from this system. Some reduce cost (natural shortcuts). Some increase it (gravity wells that trap ships). Affect navigation planning.
- **Derelict structures.** Remnants from a previous era (lore) or from abandoned player domains (gameplay). Contain salvageable components and materials. A domain that claims the system can salvage the derelict — free starting materials.
- **Phenomena.** Unusual physical conditions that affect crafting. A system with a specific radiation environment might produce different interaction function results for certain element pairs. Research value.

Anomaly detection chance depends on scanner quality and fuel spent. Basic scanner: detect only dense deposits (obvious). High-precision scanner with heavy fuel investment: detect all types. The anomaly function is beacon-gated (committed before beacon, evaluated after), so results are unpredictable at commitment time.

**Anomaly data is the highest-value expedition output.** A system with a gravitational shortcut or a derelict structure is significantly more valuable to claim. Scouts who find anomalies can sell the information for premium prices. The information is post-computable (anyone can re-run the function), but the scout found it first and can sell it before others compute it.

### Verification

Expedition outputs are verifiable through the standard proof chain:

1. Check the departure proof (timestamp, beacon epoch, departure domain signature)
2. Check that the beacon value is the one that ticked after the departure commitment
3. Re-run the expedition script with (seed, star_id, body_id, beacon_value, scanner_quality, fuel_allocated)
4. Confirm the scan data and sample manifest match

If the departure domain fabricated results (minted samples that the expedition script wouldn't produce), the verification fails. Reputation damage for the departure domain. Same trust model as crafting verification per [TRANSFORMATION.md](TRANSFORMATION.md).

### Stage 1 Testing

Expeditions are testable in the monolith:

- Player at founding system 1 requests expedition to unclaimed star 4822. Verify: fuel deducted, ship locked, departure proof recorded.
- After beacon tick, verify: scan data produced with correct confidence ranges for the ship's scanner quality.
- Verify: samples minted, mass conserved (samples are small objects with proof chains).
- Multi-system expedition: scan 3 stars in one trip. Verify: fuel budget split correctly, all outputs produced.
- Anomaly detection: verify detection probability scales with scanner quality and fuel.
- Bad scanner: verify wide confidence ranges. Good scanner: verify tight ranges. Both should bracket the true geology (which collapses at claim time).
- Expedition report traded as knowledge object. Verify format matches [KNOWLEDGE.md](KNOWLEDGE.md).

### Why Not Just Claim?

An expedition to star 4822 costs maybe 100 fuel and scanner-time. Deploying an outpost (claiming) costs hosting infrastructure indefinitely, plus materials for the outpost itself, plus ongoing bandwidth. The expedition is 10-100x cheaper than claiming.

Expeditions let players answer "is this system worth claiming?" without committing to the claim. A faction that sends scouts to 50 systems and claims the best 5 has spent much less than a faction that claims 50 and abandons 45.

The expedition doesn't replace claiming. It makes claiming a better decision.

## Interaction With Material Science

The same beacon model applies to both geology and crafting. Both use the beacon as a required key, not noise:

- **Geology:** `survey(seed, star_id, body, beacon)` — the beacon determines which specific deposits manifest from the seed's probability distribution
- **Crafting:** `interact(elements, fractions, energy, seed, beacon)` — the beacon determines which specific material properties manifest from the interaction landscape

The seed constrains what's possible. The beacon determines what is. The commitment pattern (lock resources before the beacon tick) prevents pre-computation. The verification pattern (re-execute with known inputs) ensures honesty.

See: [TRANSFORMATION.md](TRANSFORMATION.md), [../allgard/BEACON.md](../allgard/BEACON.md)
