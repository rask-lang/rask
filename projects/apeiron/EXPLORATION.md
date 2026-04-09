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

### Why No Middle Layer

I considered a prospect layer — beacon-gated scanning that produces probability ranges, costing fuel. Scouts would sell prospect data. But the beacon value is public after the tick. Anyone can run the prospect function offline with a known beacon value, for free, no fuel. The beacon prevents pre-computation but not post-computation. For any function where the output has value AS INFORMATION, post-computation makes it free.

Crafting is protected because you still need real materials. Surveying is protected because you've already committed to a claim. A prospect layer would have neither protection — the information IS the product, and it's free to compute.

Two layers. Sky and survey. No middle ground that holds up.

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

## Interaction With Material Science

The same beacon model applies to both geology and crafting. Both use the beacon as a required key, not noise:

- **Geology:** `survey(seed, star_id, body, beacon)` — the beacon determines which specific deposits manifest from the seed's probability distribution
- **Crafting:** `interact(elements, fractions, energy, seed, beacon)` — the beacon determines which specific material properties manifest from the interaction landscape

The seed constrains what's possible. The beacon determines what is. The commitment pattern (lock resources before the beacon tick) prevents pre-computation. The verification pattern (re-execute with known inputs) ensures honesty.

See: [TRANSFORMATION.md](TRANSFORMATION.md), [../allgard/BEACON.md](../allgard/BEACON.md)
