<!-- id: allgard.beacon -->
<!-- status: proposed -->
<!-- summary: Distributed verifiable randomness for the federation -->

# Beacon

Distributed verifiable randomness. A periodic unpredictable value that any domain can consume but no domain can control. The federation's shared coin flip.

## The Problem

Deterministic systems can't have randomness — every computation is re-executable, every result is predictable. But many federation operations need unpredictability: fair evaluation, unbiased selection, tamper-proof ordering. Without a shared randomness source, any domain can pre-compute outcomes and game the system.

## The Primitive

A Beacon is a periodic value with three properties:

1. **Unpredictable** — no party can predict the value before it's produced, even with full knowledge of the protocol
2. **Verifiable** — any party can verify the value was correctly produced after the fact
3. **Available** — the beacon ticks on schedule as long as at least one contributor participates

A new beacon value is produced every **tick**. The tick interval is a federation parameter — seconds for fast applications, minutes for strategic ones.

## How It Works

### Commit-Reveal

Each tick has two phases:

**Commit phase.** Contributors each generate a random value and publish `hash(value)` to the federation (via Leden gossip). The commitment is binding — the contributor can't change their value after publishing the hash.

**Reveal phase.** After all commits are in (or the commit window closes), contributors reveal their actual values. Anyone can verify `hash(revealed_value) == committed_hash`.

The beacon value is the combined hash of all revealed values:

```
beacon = hash(reveal_1 || reveal_2 || ... || reveal_n)
```

If a contributor commits but doesn't reveal, their contribution is excluded. The beacon is produced from whoever revealed. This means the beacon is available even if some contributors go offline between commit and reveal — degraded but functional.

### Why One Honest Contributor Is Enough

The combined hash is unpredictable if ANY ONE input is unpredictable. A sybil attacker controlling 99 contributors can determine 99 inputs but can't predict the 100th. The combined hash depends on all inputs — one unknown makes the whole thing unknown.

This is the fundamental security property: **the beacon is as strong as its strongest contributor.** Creating sybil contributors doesn't weaken it. Only compromising ALL contributors breaks it.

### Who Contributes

Anyone. There's no committee, no election, no stake requirement. A domain that wants to contribute announces participation (Leden gossip) and starts committing values. A domain that stops contributing just... stops. The beacon continues without them.

The founding cluster always contributes. They're the guaranteed honest participants. Their presence means the beacon is always at least as strong as the founding cluster's honesty — the same trust assumption the federation already makes for the galaxy seed and standard physics script.

Other domains contribute voluntarily. More contributors = harder to compromise (more parties needed for collusion). But even with just the founding cluster contributing, the beacon works.

## Using the Beacon

### Commit-Then-Evaluate

The primary use pattern: a domain commits to action parameters BEFORE the beacon tick, then evaluates AFTER. The beacon value enters the evaluation, making the outcome unpredictable at commit time.

1. Domain chooses parameters and locks inputs
2. Domain publishes commitment hash to bilateral partners
3. Beacon ticks — new value produced
4. Domain evaluates using (parameters + beacon value)
5. Domain publishes result — partners verify commitment, beacon, and evaluation

The commitment is binding — bilateral partners log it. If the domain doesn't follow through, the unfulfilled commitment is visible. Trust consequences via standard Allgard bilateral accountability.

### Verification

A verifier checks:

1. Was the commitment published before the beacon tick? (Timestamp in the commitment, logged by bilateral partners)
2. Does the commitment match the parameters used? (Hash check)
3. Is the beacon value correct? (Check against the beacon's public log)
4. Does the output match the evaluation with those parameters and that beacon? (Re-execute the function)

All local. No federation interaction needed for verification. The beacon log is public — gossiped via Leden, available to anyone.

## Properties

| Property | Guarantee | Assumption |
|----------|-----------|------------|
| Unpredictability | No party can predict the next value | At least one contributor is honest |
| Verifiability | Anyone can check any beacon value | Hash function is secure |
| Availability | Beacon ticks as long as anyone reveals | At least one contributor completes commit-reveal |
| Sybil resistance | Fake contributors can't bias the beacon | At least one contributor is honest |
| Decentralization | No single party controls the value | Multiple contributors |

## Beacon Log

Every beacon value is recorded in a public log — a hash chain where each entry references the previous:

```
entry_n = {
    tick: n,
    beacon_value: hash(reveal_1 || ... || reveal_k),
    contributors: [domain_ids...],
    prev: hash(entry_{n-1})
}
```

The log is gossiped via Leden. Any domain can maintain a copy. The hash chain makes tampering evident — altering a past entry invalidates all subsequent entries.

## Applications

The beacon is a low-level primitive. Applications build on it:

| Application | How it uses the beacon |
|-------------|----------------------|
| Crafting evaluation | Crafter commits params before tick, evaluates after — can't pre-compute optimal params |
| Combat resolution | Combatants commit actions before tick, resolve after — deterministic but unpredictable |
| Deterministic noise | Beacon value seeds the noise function per tick window — object quality affects scatter, beacon prevents pre-computation |
| Fair selection | Beacon selects from a committed set — no party can bias the selection |
| Event timing | Beacon triggers events at unpredictable moments within known windows |

## Interaction With Other Primitives

**Transforms** can reference a beacon value. The proof includes the beacon tick and value. Verifiers check the beacon log.

**Objects** committed before a beacon tick are locked — they can't be modified or transferred until the tick resolves and the transform completes or is abandoned.

**Conservation laws** are unaffected. The beacon doesn't create or destroy value. It provides randomness for evaluation, not for minting.

**Bilateral trust** governs commitment accountability. Partners who witness commitments enforce follow-through. The beacon itself is just a value — trust handles the behavioral layer.

## Constants

| Constant | What it controls |
|----------|-----------------|
| Tick interval | How often a new beacon value is produced |
| Commit window | How long contributors have to commit before reveal |
| Reveal window | How long contributors have to reveal after commit window closes |
| Minimum contributors | How many reveals needed for a valid tick (probably 1) |

The founding cluster publishes initial values. The tick interval is the primary tuning knob — shorter ticks mean faster research cycles but more federation traffic.

## Design Notes

I considered requiring a minimum contributor count (e.g., 5 domains must reveal for a valid tick). I rejected it because it creates an availability dependency — if fewer than 5 are online, the beacon stalls and nobody can craft. One contributor is enough for unpredictability. The founding cluster guarantees at least one. More is better but not required.

I also considered weighted contributions (domains with more trust contribute more). Unnecessary — the hash combination is already fair. One honest contribution makes the whole beacon unpredictable regardless of weight. Weighting adds complexity without security benefit.
