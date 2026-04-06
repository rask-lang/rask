# Beacon Wire Protocol
<!-- id: allgard.beacon-protocol --> <!-- status: proposed --> <!-- summary: How commit-reveal maps to Leden messages, tick timing, commitment locks, gossip propagation -->

[BEACON.md](BEACON.md) defines what the beacon is. This spec defines how it works on the wire — the Leden messages, the timing, and the mechanical interactions with Allgard transforms.

## Tick Timing

**Tick interval: 30 seconds.** Long enough for gossip to propagate across the network. Short enough that crafting and combat don't stall.

Each tick has three phases:

```
|-- commit (0-20s) --|-- reveal (20-27s) --|-- grace (27-30s) --|
                     ^                      ^                    ^
               commit closes          reveal closes         tick boundary
```

**Commit window: 20 seconds.** Contributors publish commitment hashes. Domains lock transform inputs. Long enough for gossip propagation across a healthy network.

**Reveal window: 7 seconds.** Contributors reveal values. Short — reveal is cheap, no reason to wait. If you committed, reveal immediately.

**Grace period: 3 seconds.** Final gossip propagation. Any domain that hasn't received all reveals catches up. The beacon value is computable as soon as all reveals are in — the grace period is for stragglers.

At the tick boundary: the new beacon value is final. Locked transforms evaluate. The next tick's commit window opens immediately.

### Why 30 Seconds

Shorter ticks (5-10s) make combat responsive but stress gossip convergence — a commit might not reach all peers before the window closes. Longer ticks (2-5min) are fine for crafting but make multi-tick combat feel sluggish.

30 seconds is a starting point. The founding cluster publishes this as a constant in the standard physics script. It can change (new script version, voluntary adoption). I expect combat zones might run faster local ticks for sub-tick resolution while the federation beacon stays at 30s.

## Clock Synchronization

No global clock. Domains use the beacon log itself as the clock.

Each beacon log entry has a tick number. Domains count ticks from the beacon chain, not from wall clocks. When a domain receives a beacon entry for tick N, it knows tick N+1's commit window is open.

**Bootstrap:** a domain joining the network fetches the beacon log from any peer (it's public, gossiped via Leden). The latest entry tells them the current tick. They sync to the network's tick cadence, not to any clock authority.

**Drift tolerance:** commit timestamps are judged by the receiving domain's local view of the tick phase. A commitment that arrives during the reveal window (late) is rejected by that domain but might be accepted by others who received it earlier. This is fine — the commitment is valid if ANY peer logged it during the commit window. Bilateral partners who logged the timestamp are the witnesses.

**Partition behavior:** a network partition produces two beacon chains that diverge at the partition point. Each partition produces beacon values from whoever reveals on their side. When the partition heals, domains reconcile by accepting the chain with more contributors (more reveals = stronger unpredictability guarantee). The founding cluster's presence on one side makes that side's chain authoritative in practice.

## Leden Message Types

Beacon messages use the application-defined range (0x80+):

| Tag | Message | Direction |
|-----|---------|-----------|
| 0x80 | `BeaconCommit` | Contributor → gossip |
| 0x81 | `BeaconReveal` | Contributor → gossip |
| 0x82 | `BeaconEntry` | Any → gossip |
| 0x83 | `BeaconLogRequest` | Any → any |
| 0x84 | `BeaconLogResponse` | Any → any |
| 0x85 | `TransformCommit` | Domain → bilateral partners |
| 0x86 | `TransformResult` | Domain → bilateral partners |

### BeaconCommit (0x80)

A contributor announces their commitment for an upcoming tick.

```
{
    type: 0x80,
    tick: uint,              // which tick this commitment is for
    contributor: endpoint_id, // who's committing
    commitment: bytes[32],   // hash(reveal_value)
    signature: bytes         // signs (tick, commitment)
}
```

Gossiped via existing Leden sessions. Every peer that receives it forwards to their peers (standard gossip protocol, same as PeerUpdate). Deduplicated by (tick, contributor).

### BeaconReveal (0x81)

A contributor reveals their value after the commit window closes.

```
{
    type: 0x81,
    tick: uint,
    contributor: endpoint_id,
    reveal_value: bytes[32], // the actual random value
    signature: bytes
}
```

Any receiver verifies: `hash(reveal_value) == commitment` from the corresponding BeaconCommit. Invalid reveals are dropped.

### BeaconEntry (0x82)

The computed beacon value for a completed tick. Any domain can produce this once they have all reveals (or the reveal window closes).

```
{
    type: 0x82,
    tick: uint,
    beacon_value: bytes[32],   // hash(reveal_1 || reveal_2 || ... || reveal_n)
    contributors: [endpoint_id...],  // who revealed, in canonical order
    reveals: [bytes[32]...],   // corresponding reveal values
    prev_hash: bytes[32],      // hash of previous BeaconEntry
    signature: bytes           // producer signs the entry
}
```

Self-verifying: any receiver can recompute `beacon_value` from the reveals and check `prev_hash` against the prior entry. The producer's signature is convenience, not authority — the entry's correctness is independently verifiable.

Contributors are sorted by endpoint_id (canonical order) so every domain computing the beacon value independently gets the same hash.

### BeaconLogRequest (0x83) / BeaconLogResponse (0x84)

For syncing. A domain that missed ticks requests a range:

```
// Request
{
    type: 0x83,
    id: uint,
    from_tick: uint,    // inclusive
    to_tick: uint       // inclusive, max 100 entries per request
}

// Response
{
    type: 0x84,
    id: uint,
    entries: [BeaconEntry...]
}
```

Used during bootstrap and partition recovery. The beacon log is public — any peer can serve it.

### TransformCommit (0x85)

A domain commits to transform parameters before the beacon tick. This is the application-level commitment — "I'm going to craft/fight/mine with these parameters on the next tick."

```
{
    type: 0x85,
    id: uint,
    tick: uint,                // which beacon tick this transform evaluates against
    domain: endpoint_id,       // who's committing
    commitment_hash: bytes[32], // hash(transform_parameters)
    locked_objects: [object_id...], // objects locked for this transform
    signature: bytes
}
```

Sent to bilateral partners (not gossiped to the whole network — transform details are domain business). Partners log the commitment with their local timestamp. This timestamp is the evidence that the commitment preceded the beacon.

### TransformResult (0x86)

After the beacon reveals, the domain evaluates and publishes the result.

```
{
    type: 0x86,
    id: uint,
    tick: uint,
    domain: endpoint_id,
    beacon_value: bytes[32],    // the beacon value used
    parameters: bytes,          // the full transform parameters (hash must match commitment)
    result: bytes,              // the transform output
    proof: bytes,               // Raido execution proof
    signature: bytes
}
```

Partners verify:
1. `hash(parameters) == commitment_hash` from the TransformCommit
2. `beacon_value` matches the beacon log for that tick
3. The TransformCommit timestamp was before the beacon tick
4. Re-execute the transform script with (parameters, beacon_value) and confirm the result matches

All local. No federation interaction needed.

## Commitment Locks

When a domain sends a TransformCommit, the listed objects are **locked**. Locked objects cannot be transferred, mutated, or included in another transform until the lock resolves.

### Lock Lifecycle

```
1. Domain sends TransformCommit listing objects → objects locked
2. Beacon tick completes → beacon value available
3. Domain evaluates transform → produces result
4. Domain sends TransformResult → lock released, transforms applied
5. OR: domain abandons (doesn't send result within 2 ticks) → lock released, no transform
```

### Lock Visibility

Bilateral partners who receive the TransformCommit know which objects are locked. If the domain tries to transfer a locked object, the partner rejects it — the transfer proof would reference an object that's currently committed to a transform.

### Lock Duration

A lock lasts at most 2 tick intervals (60 seconds at the default rate). If the domain doesn't publish a TransformResult within 2 ticks after the committed tick, the lock expires and the objects are free. This prevents griefing — a domain can't lock objects forever by committing and never resolving.

### Interaction With Combat

Combat transforms follow the same pattern:

1. Combatant commits orders (TransformCommit with fleet objects locked)
2. Beacon reveals
3. Domain computes combat round
4. Domain publishes result (TransformResult with damage/destruction outcomes)
5. Both sides verify independently

The fleet objects are locked during the tick — they can't be transferred mid-combat. This is the mechanical "your ships are committed to this fight" enforcement. Retreat releases the lock over multiple ticks (per COMBAT.md retreat timing).

## Gossip Propagation

Beacon messages ride on existing Leden sessions — the same gossip infrastructure used for peer discovery. No special connections needed.

### Propagation Model

Same as PeerDigest/PeerRequest/PeerUpdate but for beacon data:

1. A contributor sends BeaconCommit to all direct peers
2. Each peer forwards to their peers (deduplicate by tick + contributor)
3. Convergence in O(log N) rounds for N endpoints

With 30-second ticks and sub-second gossip rounds, a 1000-domain network converges within the commit window. The 20-second commit window is conservative — most networks converge in under 5 seconds.

### Bandwidth

Per tick, per contributor: one BeaconCommit (~100 bytes) + one BeaconReveal (~100 bytes). With 10 contributors, that's ~2 KB per tick per peer, gossiped. Negligible compared to normal Leden session traffic.

BeaconEntry is larger (~500 bytes with 10 contributors) but only produced once per tick and only needs to reach each domain once.

## Byzantine Behavior

### Non-Reveal

A contributor who commits but doesn't reveal is simply excluded from the beacon value. Their commitment is ignored. The beacon is produced from whoever revealed.

If a contributor repeatedly commits and doesn't reveal, peers stop forwarding their commits (local policy, not protocol enforcement). They're wasting bandwidth. The standard recommendation: after 3 consecutive non-reveals, stop relaying commits from that contributor for 10 ticks.

### Invalid Reveal

A reveal whose hash doesn't match the commitment is dropped. The contributor is treated as a non-revealer for that tick. Same bandwidth penalty applies.

### Late Commit

A commitment that arrives after the commit window is rejected by the receiving domain. Other domains that received it in time may accept it. This is fine — the commitment is valid if at least one bilateral partner timestamped it within the window. A domain trying to use a transform commitment that nobody witnessed as on-time won't find partners willing to verify its proofs.

### Equivocation

A contributor who sends two different commitments for the same tick (to different peers) is detectable: when the reveals propagate, the two different commitment hashes surface. Equivocation proof is published via gossip. The contributor's reputation takes the hit. Both commitments are excluded from the beacon value for that tick.

### Withholding Bias

A contributor who sees other reveals and decides not to reveal (to bias the beacon by exclusion) is possible. This is the known weakness of commit-reveal schemes. Mitigation: the contributor already committed. Not revealing means their commitment is wasted and they gain bandwidth penalties. The beacon value without their contribution is still unpredictable (one honest contributor is enough). The cost of withholding (wasted commitment, reputation) exceeds the benefit (marginal influence on the value) in practice.

## Capability Model

### Contributing

Any domain can contribute to the beacon. No special capability required — just send BeaconCommit to your peers. Contributing is permissionless because adding contributors can only strengthen the beacon (one honest contributor is enough).

### Consuming

Reading the beacon log is permissionless. It's public data, gossiped to everyone. Any domain can fetch any historical beacon value.

### Committing Transforms

TransformCommit/TransformResult use standard Leden capabilities. A domain must have the appropriate capability for the objects it locks (ownership or delegated authority). Partners verify this against the object's capability chain.

No new capability types are needed for the beacon itself. The beacon is infrastructure — like the clock. Everyone can read it. The capabilities govern what you DO with it (transforms on objects you have authority over).

## Constants

| Constant | Initial Value | What it controls |
|----------|---------------|-----------------|
| `tick_interval` | 30s | Total tick duration |
| `commit_window` | 20s | Time for commitments |
| `reveal_window` | 7s | Time for reveals |
| `grace_period` | 3s | Final propagation |
| `lock_timeout` | 2 ticks (60s) | Max commitment lock duration |
| `non_reveal_penalty` | 3 consecutive | Commits ignored after N non-reveals |
| `penalty_duration` | 10 ticks | How long the non-reveal penalty lasts |

Published by the founding cluster in the standard physics script. Changeable via new script version, voluntary adoption.

## Design Notes

I picked 30-second ticks as the initial value because it's the smallest interval where gossip reliably converges across a global network with realistic latency. 10 seconds works for a founding cluster of 5-20 domains in the same region. 30 seconds works when the network spans continents.

The commit-reveal scheme is deliberately simple. I considered threshold signatures (BLS, DRAND-style) which would eliminate the withholding attack entirely. I rejected them because they require a fixed committee, which requires election, which requires governance. Commit-reveal with one-honest-contributor is weaker cryptographically but requires zero governance. That fits Allgard's bilateral model. If the withholding attack becomes a real problem in practice, threshold signatures can be layered on top without changing the message format — just the beacon value computation.

Transform commitments are bilateral (sent to trading partners), not gossiped globally, because transform details are domain business. The beacon itself is public. What you do with it is between you and your partners. This separation keeps the gossip bandwidth low and respects domain sovereignty.
