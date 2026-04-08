# Exploration
<!-- id: apeiron.exploration --> <!-- status: proposed --> <!-- summary: How galaxy knowledge works — coarse data from the seed, detailed geology from beacon-gated claim surveys -->

The galaxy is a seed. The seed determines what's POSSIBLE. What's ACTUAL is determined by who shows up.

## Two Layers of Knowledge

### Coarse Layer — Public, From the Seed

Star positions, spectral classes, planet counts, rough composition. Computable by anyone from the public seed. This is your sky map — the same for everyone, always.

The coarse layer tells you probabilities. "This G-type star with 4 rocky planets is likely iron-rich." "That red dwarf with a single gas giant probably has hydrogen." Enough to make informed decisions about where to explore. Not enough to plan extraction.

```
coarse = generate_star(seed, star_id)
// → position, spectral class, planet count, probable composition
// Public. Permanent. No beacon required.
```

The coarse generation function IS public. Anyone can read it, run it, map all 10,000 stars. That's fine — you're looking at the sky. The sky is free.

### Detailed Layer — Beacon-Gated, Created at Claim Time

Specific deposit data — element types, quantities, qualities, locations on each body. This doesn't exist until someone claims the system.

Claiming a star means deploying a domain. The claim includes a comprehensive geological survey — a scan Transform that takes the seed, the star ID, and the current beacon value. The beacon is half the key. Without it, the function produces garbage. With it, the true geology manifests.

```
geology = survey(seed, star_id, body_id, beacon_value)
// → specific deposits, quantities, quality, extraction sites
// Requires beacon. Created once at claim time. Permanent.
```

The geology is permanent once created. It's published as part of the domain's metadata. Verifiable by anyone: re-run the function with (seed, star_id, body_id, beacon_value_at_claim), get the same result. The beacon value is in the public beacon log. The claim Transform proof timestamps when it happened.

## Why the Beacon Is the Key, Not Noise

The beacon value isn't a perturbation on readable data. It's a required input to a cryptographic construction. The function mixes seed and beacon such that neither alone produces meaningful output.

The seed constrains the distribution — what elements are POSSIBLE, their probability ranges. The beacon collapses the distribution into specific values. Like a keyed hash: the message (seed) and the key (beacon) are both required. Remove either and the output is garbage.

This means:
- **Reading the script gives you nothing.** The function is public Raido bytecode. Understanding every line doesn't help because the output depends on a beacon value that doesn't exist until someone commits fuel and waits for the tick.
- **Running the script without a beacon gives garbage.** Not "slightly wrong" — meaningless. The beacon is load-bearing, not decorative.
- **After the beacon, the result is the truth.** Not an approximation, not a noisy sample. The actual, permanent, verifiable geology.

## Collapse on Claim

I chose to tie geological collapse to claiming (deploying a domain) rather than to individual scans. 

**Why not collapse on first scan?** Race conditions. In a federated system with no global state, "who scanned first" is a distributed consensus problem. Two scanners at different domains, different epochs, same location — who wins? This is the same class of problem as double-spend detection. Solvable, but unnecessary complexity.

Collapse on claim avoids it entirely. Claiming is a heavyweight operation — you're deploying real hosting infrastructure. There's no race because domain deployment is visible in the network (Leden gossip). Two domains can't claim the same star because the network resolves competing claims through bilateral trust (who has more relationships, who got introduced first — see [README.md](README.md#no-central-map-authority)).

**What this means:**
- Before anyone claims a star: only coarse data exists. Probabilities, not facts.
- The moment someone claims: geology manifests. Specific, permanent, verifiable.
- The claimer doesn't choose their geology. They committed to the claim before the beacon tick. The beacon (unpredictable) determines what manifests.
- Good coarse data (probable titanium) might collapse into great geology or mediocre geology. Risk and reward from the same mechanism.

## Scouts

Scouts don't discover ground truth — they narrow probabilities.

A scout visits unclaimed systems and analyzes coarse data in detail. They can't run the detailed survey (no domain = no beacon-gated Transform). But they can compute the coarse generation function for every body in the system, cross-reference spectral analysis with known element correlations, and produce an estimate.

A scout report says: "Star 4822 has 4 rocky planets. Body 2 and 4 show iron-class spectral signatures. Based on planet mass and composition model, estimated 30K-70K iron across the system. Body 3 is an asteroid belt with trace heavy-element signatures — possible tungsten or chromium. Recommend claiming."

This is real value. The probability narrowing saves other players from wasting claim costs on bad systems. But it's not ground truth — the actual geology depends on a beacon value that won't exist until someone claims.

Scout reports are cheaper to produce (just fuel for travel + local computation) and useful for decision-making. They're honest about what they are: informed estimates, not surveys.

## What the Script Reveals vs. What Requires Commitment

| Layer | Source | Cost | What you learn |
|---|---|---|---|
| Sky map | `generate_star(seed, star_id)` | Free | Position, type, planets |
| Scout estimate | Coarse data + analysis | Travel fuel | Probability ranges for elements |
| Geological survey | `survey(seed, star_id, body, beacon)` | Claim (hosting + fuel) | Exact deposits, permanent |

Each layer is strictly more informative and strictly more expensive. No shortcuts.

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
