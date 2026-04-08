# Exploration
<!-- id: apeiron.exploration --> <!-- status: proposed --> <!-- summary: How galaxy knowledge works — coarse data from the seed, detailed geology from beacon-gated claim surveys -->

The galaxy is a seed. The seed determines what's POSSIBLE. What's ACTUAL is determined by who shows up.

## Three Layers of Knowledge

### Sky — Free, From the Seed

Star positions, spectral classes, planet counts. Computable by anyone from the public seed. This is your sky map — the same for everyone, always.

```
sky = generate_star(seed, star_id)
// → position, spectral class, planet count
// Public. Permanent. No beacon required.
```

The sky generation function IS public. Anyone can read it, run it, map all 10,000 stars. That's fine — you're looking at the sky through a telescope. The sky is free. But it doesn't tell you what's on the ground.

### Prospect — Beacon-Gated, Scout's Work

Probability ranges for element types and quantities per body. Beacon-gated, costs committed fuel. This is what scouts do.

```
estimate = prospect(seed, star_id, body_id, beacon_value)
// → probability ranges: "likely 30K-70K iron", "trace heavy-element signatures"
// Requires beacon. Costs fuel. Not ground truth.
```

Without the beacon, the prospect function produces garbage — same cryptographic construction as everything else. With it, you get a useful narrowing of the probability distribution. Not specific values — ranges. Enough to decide whether a system is worth claiming.

Multiple prospects across different beacon epochs narrow the ranges further. Each gives a different window on the same underlying truth (which only manifests at claim time). A thorough scout with 5 epochs of prospect data has significantly tighter estimates than a single-pass visit.

### Survey — Claim Time Only

Specific deposit data. Element types, quantities, qualities, extraction sites per body. Created once at claim time. Permanent.

```
geology = survey(seed, star_id, body_id, beacon_value)
// → specific deposits, quantities, quality, extraction sites
// Requires beacon. Created at claim time. Permanent.
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

Scouts run prospects. That's the job.

A scout travels to unclaimed systems and runs beacon-gated prospect Transforms from a nearby domain. Each prospect costs committed fuel and one beacon tick. The result is probability ranges — not ground truth, but enough to decide whether a system is worth claiming.

A scout report says: "Star 4822, 5 epochs of prospect data. Body 2: iron 30K-70K (high confidence). Body 4: iron 15K-40K, trace tungsten (moderate confidence). Body 3 asteroid belt: heavy-element signatures consistent with chromium or gold (low confidence, needs more epochs). Recommend claiming if you need iron. Tungsten is a bonus gamble."

This has real value. The prospect data is beacon-gated — you can't compute it from the script. Each epoch of data cost real fuel. The tighter the ranges, the more epochs the scout invested. Buying a report is cheaper than prospecting yourself.

### Why Not Just Claim Blind?

Claiming costs hosting — real money, real infrastructure. A system that prospects as "probably iron-rich" might survey as mediocre. Prospect data doesn't eliminate risk, but it narrows it. A scout who says "5 epochs of data, high confidence iron, moderate confidence tungsten" gives you better odds than rolling the dice on sky data alone.

## What Each Layer Costs

| Layer | Function | Cost | What you learn | Beacon? |
|---|---|---|---|---|
| Sky | `sky(seed, star_id)` | Free | Position, type, planet count | No |
| Prospect | `prospect(seed, star_id, body, beacon)` | Fuel per tick | Probability ranges | Yes |
| Survey | `survey(seed, star_id, body, beacon)` | Claim (hosting) | Ground truth, permanent | Yes |

Each layer is strictly more informative and strictly more expensive. No shortcuts.

## Beacon Overhead

Every prospect and survey requires the beacon: commit parameters + fuel, wait for tick, execute. This has latency cost.

**Batching amortizes it.** A scout commits 20 prospects in one tick — different bodies, different systems. One fuel commitment, one beacon tick, 20 evaluations. The cost is one tick of latency, not twenty. Most gameplay batches naturally: a scout visiting a sector prospects everything in one pass.

**Tick interval is a tuning knob.** Short ticks (seconds) for fast gameplay. Longer ticks (minutes) for strategic weight. Stage 1 (monolith) has a local beacon — near-zero overhead. Federation adds one network round trip per tick, not per operation.

**Verification is cheap.** Re-running a Raido function to verify a prospect or survey result: microseconds to milliseconds. Fetch beacon value from the public log, re-execute locally, compare output. No network call needed beyond the initial log fetch.

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
