<!-- id: allgard.transfer -->
<!-- status: proposed -->
<!-- summary: Cross-domain ownership transfer protocol — escrow, timeouts, partition recovery -->

# Cross-Domain Transfer Protocol

Law 2 says ownership transfer is atomic. Within a single domain, that's trivial — it's a local database transaction. Across domains, it's a distributed systems problem. This spec defines how cross-domain transfers stay safe under network failure.

## The Problem

Two domains. One object. The object needs to move from Domain S (source) to Domain D (destination). During the move, the object must have exactly one owner and must not exist on two domains simultaneously. The network can fail at any point.

This is not 2PC. There's no external coordinator and no voting phase. The source domain holds the object, which makes it the natural authority. It decides when to commit. The destination validates and accepts. The protocol is asymmetric by design — and that asymmetry is what makes it work without global consensus.

Why not 2PC: a coordinator is a single point of failure, and an external coordinator contradicts domain sovereignty. Here the source domain is both authority and participant. It holds the object, so it holds the deciding vote. No coordinator to fail, no voting to deadlock.

## Transfer Intent

Every cross-domain transfer starts with a signed Transfer Intent from the object's Owner:

| Field | Description |
|-------|-------------|
| `object_id` | The object being transferred |
| `from_owner` | Current owner identity |
| `to_owner` | New owner identity |
| `source_domain` | Domain currently hosting the object |
| `dest_domain` | Domain that will host the object |
| `timeout` | Maximum duration for the transfer |
| `conditions` | Optional forwarding conditions for escrow chains |
| `fee` | Optional declared fee — asset type, amount, recipient |
| `signature` | Owner's cryptographic signature over the above fields |
| `causal_ref` | Reference to the object's current state (Law 4) |

The Transfer Intent is the authorization. Without the Owner's signature, no transfer happens. The source domain can't forge this — it doesn't hold the Owner's private key (and [shouldn't](TRUST.md#dying-domain-endgame)).

## Protocol

```
Owner              Source (S)           Destination (D)
  |                     |                      |
  | 1. TransferIntent   |                      |
  |────────────────────>|                      |
  |                     | lock object          |
  |                     | state → Escrowed     |
  |                     |                      |
  |                     | 2. TransferOffer     |
  |                     |─────────────────────>|
  |                     |                      | validate proofs
  |                     |                      | check laws
  |                     |                      | check types
  |                     |                      |
  |                     |   3. TransferAccept  |
  |                     |<─────────────────────|
  |                     |                      |
  |                     | persist Departure    |
  |                     | Proof (write-ahead)  |
  |                     | state → Departed     |
  |                     |                      |
  |                     | 4. TransferCommit    |
  |                     |─────────────────────>|
  |                     |                      | register object
  |                     |                      | state → Arrived
  |                     |                      |
  |                     |  5. TransferComplete |
  |                     |<─────────────────────|
  |                     |                      |
  |                     | clean up escrow      |
```

### Phase 1: Intent

Owner submits a signed Transfer Intent to the source domain. Source validates the signature and the causal reference (Law 4), then locks the object. State transitions to Escrowed. No mutations are permitted while escrowed.

### Phase 2: Offer

Source sends the object content, full proof chain, and Transfer Intent to the destination. This is everything D needs to verify the transfer independently — no trust required, just cryptography.

### Phase 3: Accept or Reject

Destination validates:

- Transfer Intent signature is valid
- Proof chain is intact (Law 4)
- Object type is compatible with destination's type registry
- Conservation laws aren't violated
- Destination is willing to host this object (domain policy — rate limits, content rules, etc.)

If validation passes, D sends TransferAccept. If not, TransferReject with a reason code. On rejection, source unlocks the object and returns to Active.

### Phase 4: Commit

The point of no return. Source persists the Departure Proof to durable storage **before** sending TransferCommit. This is the commit record.

The Departure Proof contains:

| Field | Description |
|-------|-------------|
| `transfer_intent` | The original signed Transfer Intent |
| `source_signature` | Source domain's signature confirming departure |
| `timestamp` | When the departure was committed |
| `causal_ref` | Reference to the object's final state on S |
| `object_content_hash` | Hash of the transferred object content |

Once persisted, the object is no longer in S's inventory. The transfer is committed from S's perspective — irrevocably.

**Why write-ahead?** If source crashes after persisting but before sending TransferCommit, it retries on restart. If it crashes before persisting, the object is still Escrowed and rolls back on timeout. The persist-then-send ordering makes the commit atomic relative to crashes.

### Phase 5: Complete

Destination registers the object, creates an Arrival Proof, and sends TransferComplete. Source cleans up the escrow record. Transfer is done.

## Object States

On the source domain:

| State | Meaning | Mutable? | In inventory? |
|-------|---------|----------|---------------|
| Active | Normal operation | Yes | Yes |
| Escrowed | Locked for transfer | No | Yes (locked) |
| Departed | Transfer committed | N/A | No |

On the destination domain:

| State | Meaning | In inventory? |
|-------|---------|---------------|
| Pending | Offer validated, waiting for commit | No (staged) |
| Arrived | Registered | Yes |

The Escrowed state is the reversible zone. Everything before Departed can be rolled back by timeout. Departed is permanent on the source side.

### Owner-Initiated Cancel

While the object is Escrowed, the owner can actively cancel the transfer without waiting for the timeout. The owner submits a signed CancelTransfer to the source domain. Source unlocks the object, returns to Active.

If the destination already has a Pending record (it received the TransferOffer), source sends TransferAbort to inform it. Destination discards its Pending state. If the abort doesn't arrive, destination's own timeout handles cleanup — the abort is an optimization, not a correctness requirement.

CancelTransfer is only valid while the source is in Escrowed state. Once the source transitions to Departed, cancellation is impossible — the Departure Proof is committed. The owner must wait for the transfer to complete or use wallet recovery if the destination is unreachable.

## Timeout Semantics

Every transfer has a timeout, set in the Transfer Intent. Three timeout levels handle different failure durations.

### Transfer timeout (default: 30s)

Covers the offer/accept/commit cycle. If source hasn't received TransferAccept before this expires, the object returns to Active. Clean rollback — no Departure Proof was created, nothing to undo.

### Completion timeout (default: 5 min)

After source commits (Departed state), this is the window for destination to send TransferComplete. If it doesn't arrive:

1. Source retries sending TransferCommit (the Departure Proof is idempotent)
2. Exponential backoff between retries
3. Source does not roll back — it can't, the Departure Proof is committed

### Extended timeout (default: 24h)

After the completion timeout, source stops actively retrying but keeps the Departure Proof in persistent storage indefinitely. When destination comes back online, it can request the Departure Proof via TransferStatus.

| Timeout | Default | Covers |
|---------|---------|--------|
| Transfer | 30 seconds | Offer → Accept cycle |
| Completion | 5 minutes | Commit → Complete cycle |
| Extended | 24 hours | Active retry window |
| Departure Proof retention | Indefinite | Recovery via query |

All timeouts are configurable per bilateral agreement. Allied domains with low-latency links might use 5-second transfer timeouts. Cross-network transfers through intermediaries might use minutes.

### Destination timeout behavior

If destination sent TransferAccept but doesn't receive TransferCommit before the transfer timeout: destination discards its Pending state.

If TransferCommit arrives later with a valid Departure Proof, the destination can still accept it. The Departure Proof is self-contained — D doesn't need prior Pending state to verify and register the object. This makes the protocol self-healing for late-arriving messages.

## Partition Behavior

Five failure modes, exhaustively.

### 1. Partition during offer

Network fails before D receives TransferOffer.

- **Source:** Escrowed, waiting. Transfer timeout → rollback to Active.
- **Destination:** Never saw anything.
- **Outcome:** Clean rollback. No effect.

### 2. Partition during accept

D validated and sent Accept, but S never receives it.

- **Source:** Escrowed, waiting. Transfer timeout → rollback to Active.
- **Destination:** Pending, waiting. Transfer timeout → discard.
- **Outcome:** Both sides roll back independently. The Accept is lost. No effect.

### 3. Partition after commit — the critical case

Source committed (Departure Proof persisted, state → Departed). Network fails before D receives TransferCommit.

- **Source:** Departed. Cannot roll back. Retries TransferCommit with backoff.
- **Destination:** Pending or already timed out. Waiting.
- **Outcome:** Object is **in transit**.

This is the only case where the object is temporarily not actively hosted by any domain. The transfer is committed on source but not received by destination.

**Recovery paths, in order of preference:**

| # | Scenario | Recovery |
|---|----------|----------|
| 1 | Network recovers | S sends TransferCommit → D registers. Normal completion. |
| 2 | D comes back, S still up | D requests TransferStatus → S resends Commit. |
| 3 | S goes down, D still up | D waits. When S recovers, S retries Commit (Departure Proof is persisted). |
| 4 | D permanently gone | Owner uses [wallet recovery](PRIMITIVES.md#owner-wallet) to land object on a different domain. The Departure Proof in the wallet constitutes proof of ownership. |
| 5 | S permanently gone | Owner uses wallet recovery. The wallet contains the Departure Proof and object content. |
| 6 | Both permanently gone | Owner uses wallet recovery to any new domain. |

In cases 4-6, the wallet is the safety net. The Departure Proof + object content + proof chain in the wallet are sufficient for any domain to independently verify ownership and accept the object.

### 4. Partition after complete

TransferComplete doesn't reach source. Destination has the object. Source still has a dangling escrow record.

- **Source:** Departed, waiting for Complete. Retries TransferCommit (which D already processed).
- **Destination:** Arrived. Receives duplicate TransferCommit, responds with TransferComplete again.
- **Outcome:** Idempotent. Source cleans up escrow on receiving the duplicate Complete.

### 5. Source crashes during commit

Source received Accept, begins persisting Departure Proof, crashes mid-write.

- **If Departure Proof was fully persisted:** On restart, source detects Departed state, retries TransferCommit. Normal recovery.
- **If Departure Proof was NOT persisted:** On restart, source detects Escrowed state. Timeout → rollback to Active. Destination times out independently.
- **Outcome:** The filesystem's atomic write guarantees determine which case applies. The protocol doesn't need to distinguish — it follows the persisted state.

### In-transit duration bounds

| Scenario | Duration |
|----------|----------|
| Normal (no failure) | Single RTT — milliseconds |
| Transient network failure | Seconds to minutes (retry cycle) |
| Extended outage | Up to extended timeout (24h default) |
| Permanent domain failure | Until wallet recovery |

## Proving Law 2 Under Network Failure

Three claims. All three must hold for Law 2 to be satisfied.

### Claim 1: No dual hosting

> At no point do two domains simultaneously have the same object in their inventory.

**Proof:** Source transitions to Departed (removes object from inventory) **before** sending TransferCommit. Destination transitions to Arrived (adds object to inventory) **after** receiving TransferCommit. These events are causally ordered:

```
S persists Departure Proof → S sends TransferCommit → [network] → D receives → D registers
```

The causal chain is unforkable. There is no execution where D registers before S departs. Therefore, at any point in time, the object is in at most one inventory.

There is a window where it's in **zero** inventories (in transit). That's the honest cost of bilateral transfer without global consensus. But zero is not two. Law 2 prohibits duplication, not transit.

### Claim 2: No dual ownership

> At no point does the object have two simultaneous owners.

**Proof:** Ownership is determined by the Transfer Intent, signed by the current owner. The Transfer Intent specifies exactly one `from_owner` and exactly one `to_owner`. The transition is:

1. Before commit: `from_owner` owns the object.
2. After commit: `to_owner` owns the object.
3. The commit is a single atomic event (persisting the Departure Proof).

There is no state where both owners simultaneously have valid ownership claims. The Transfer Intent + Departure Proof form an unambiguous causal chain with exactly one owner at every point.

### Claim 3: Recoverable under all failure modes

> For every failure mode, there exists a recovery path that restores the object to a consistent state with exactly one owner on exactly one domain.

**Proof by exhaustion** (see [Partition Behavior](#partition-behavior)):

| Source state | Dest state | Recovery | Final state |
|---|---|---|---|
| Escrowed | — | Timeout → rollback | Source, original owner |
| Escrowed | Pending | Both timeout → rollback | Source, original owner |
| Departed | Pending | Retry → commit | Dest, new owner |
| Departed | — (timed out) | Late commit or wallet | Dest (or any domain), new owner |
| Departed | Arrived | Complete (or idempotent retry) | Dest, new owner |

Every row terminates with one owner, one domain. The "eventually" qualifier is real — partition recovery takes time. But the protocol guarantees convergence to a consistent state. No failure mode produces a permanent inconsistency.

### What this proof doesn't cover

**Compromised source domain.** If the source domain's signing key is compromised, an attacker could forge Departure Proofs. This is an authentication failure, not a protocol failure. Defense: source domain key management, bilateral Proof verification by the destination, and gossip-based fraud detection (see [TRUST.md](TRUST.md)).

**Byzantine destination.** A destination could claim it never received TransferCommit (lying to keep the object without sending Complete). Source has the Departure Proof proving it committed. The destination's refusal to acknowledge is a bilateral dispute, resolved by the same gossip and reputation mechanisms that handle all inter-domain fraud. The source publishes the Departure Proof, trading partners can verify it, and the destination's reputation takes the hit.

## Conflict Resolution

### Double transfer attempts

Owner tries to transfer the same object to two destinations. Source domain's Escrow lock prevents this — the first Transfer Intent locks the object, the second is rejected with "object locked."

If the first transfer times out and rolls back, the second attempt can proceed. The escrow is a mutex with timeout, not a queue.

### Transfer vs. mutation race

Owner submits a mutation and a transfer concurrently. Source domain serializes them. If the mutation arrives first, it's applied — the transfer's `causal_ref` is now stale (Law 4 rejects it). If the transfer arrives first, the object is escrowed and the mutation is rejected (locked).

No ambiguity. The source domain's serialization order is the ground truth.

### Late-arriving TransferCommit

Destination already discarded its Pending state (timeout expired). TransferCommit arrives with a valid Departure Proof.

The destination **can still accept it.** The Departure Proof is self-contained and independently verifiable — D doesn't need prior state to validate it. D checks:

1. Departure Proof signature is valid
2. Proof chain is intact
3. Object hasn't already been registered from another source (double-spend check)

If all checks pass, D registers the object. The protocol is self-healing.

### Double-spend detection

A compromised source domain forges two Departure Proofs for the same object, sending each to a different destination.

Defense: Law 4. Both Departure Proofs reference the same `causal_ref` (the object's state at departure). Only one state transition from a given `causal_ref` is valid. When the second destination receives its Departure Proof, it checks the proof chain — if another domain has already registered a transition from that `causal_ref`, the second proof is invalid.

Detection mechanism: gossip. When two domains discover conflicting Departure Proofs for the same object, the source domain is flagged for fraud. First-writer-wins — the domain that registered first keeps the object. The second domain reverses and reports.

This is the same principle as [witnessed recovery](PRIMITIVES.md#witnessed-recovery) — conflicting claims are resolved by the witnesses who saw the original state.

## Escrow Transforms (Intermediary Chains)

The basic protocol handles direct S→D transfers. For intermediary chains (A→B→C, where B is a relay), the protocol composes with conditional transfers.

### Conditional Transfer Intent

A Transfer Intent can include forwarding conditions:

| Condition field | Description |
|-----------------|-------------|
| `forward_to` | Owner/domain that B must forward the object to |
| `forward_deadline` | Seconds after arrival that B must complete the forward |
| `on_failure` | `return_to_sender` — pre-authorized return transfer |

This reads as: "Transfer to B, but B must forward to C within the deadline. If B doesn't, the object returns to A."

### Sequence

1. A→B transfer executes using the standard protocol (all 5 phases).
2. B receives the object with the forwarding condition attached.
3. B initiates B→C transfer using the standard protocol.
4. If B→C completes within the deadline: done. Condition satisfied, the forwarding metadata is dropped.
5. If B→C fails at the deadline: B's domain automatically initiates a return transfer to A. The conditional Transfer Intent pre-authorizes the return — B doesn't need A's signature for it.

### Failure during intermediary chain

| Failure | What happens |
|---------|-------------|
| A→B fails | Standard rollback. Object stays with A. |
| A→B succeeds, B→C in progress | B retries B→C until deadline. |
| A→B succeeds, B→C fails at deadline | Automatic return: B→A via pre-authorized return. |
| A→B succeeds, B goes dark before forwarding | A's forwarding condition has a deadline. After expiry, A has cryptographic proof (the conditional Transfer Intent + deadline expiry) that the object should return. A presents this to B's domain when it recovers — or uses wallet recovery. |

The conditional transfer composes from existing primitives: Transfer Intent + Grant (the pre-authorized return) + timeout. No new protocol mechanisms.

### Chaining depth

A→B→C is one intermediary. A→B→C→D is two. Each hop adds a forwarding condition and a deadline. The deadlines must nest: the outermost deadline must be longer than the sum of inner deadlines, or the chain can't complete.

I'd cap this at 3 hops by convention. More than 3 intermediaries is a sign that introduction (Leden's `Introduce` operation) should be used instead. Intermediary chains are for bridging trust gaps, not routing.

## Relationship to Leased Transfer

[Leased transfer](PRIMITIVES.md#leased-transfer) for player visiting is this protocol with a renewable timeout. The lease is a conditional transfer: "host these objects for the visiting player. Renew periodically. If renewal stops, return them."

| Property | Intermediary chain | Leased transfer |
|----------|-------------------|-----------------|
| Purpose | Bridge trust gap | Low-latency game access |
| Duration | Seconds to minutes | Hours to days |
| Renewal | No | Yes (automatic) |
| Expected outcome | Forward | Stay + eventual return |
| Return trigger | Deadline expiry | Lease expiry or revocation |

Under the hood, lease renewal extends the forwarding deadline. No re-transfer — the object stays on the visited domain. The home domain sends a LeaseRenew message that resets the clock.

## Grant Invalidation

When an object transfers to a new domain, all outstanding [Grants](PRIMITIVES.md#grant) targeting that object are revoked.

**Why Grants don't follow the object:**

1. **Authority context changed.** A Grant was issued by the grantor (the old owner or a delegate) on the source domain. The destination domain never approved that delegation. Carrying Grants across domain boundaries would force the destination to honor authority it never vetted.
2. **Sessions changed.** Grants are exercised through Leden sessions. The source domain's sessions are not the destination domain's sessions. The Grant holder would need a session with the destination to exercise the Grant — and that's a new relationship, not a continuation.
3. **Owner may have changed.** If `from_owner ≠ to_owner`, the new owner hasn't authorized the old owner's Grants. Even if the owner is the same (e.g., moving objects between domains), the domain change means the operational context is different.

**When revocation happens:**

| Transfer phase | Grant status |
|----------------|-------------|
| Escrowed | Grants remain valid. Read Grants work. Write Grants are blocked (object is locked). |
| Departed | All Grants are revoked. Source sends RevocationNotice to all Grant holders. |
| Arrived | New owner can issue new Grants on the destination domain. |

During escrow, Grants aren't revoked because the transfer might roll back. Revoking early would break shared access for a transfer that never completes. Revocation happens at commit — when the Departure Proof is persisted and the object leaves the source inventory.

**Leased transfers.** For [player visiting](PRIMITIVES.md#leased-transfer), the home domain can pre-coordinate Grants with the visited domain. The player's home domain issues a Grant to the visited domain as part of the lease setup. The visited domain then issues local Grants for game logic. When the lease ends and objects return, the visited domain's Grants are revoked (same mechanism — transfer triggers revocation). The home domain re-establishes its original Grants.

This is a real cost. A player with Grants shared to 5 friends loses those Grants when visiting another domain. The friends need new Grants after the player returns (or the home domain re-issues automatically based on a stored Grant policy). I think that's the right tradeoff — the alternative is Grants that silently span trust boundaries, which is worse.

## Transfer Fees

Cross-domain transfers can carry fees ([Law 3](CONSERVATION.md) — conservation of exchange, designed entropy). Transaction fees are a value sink that bounds spam and drains supply.

**How fees work in the protocol:**

Fees are declared in the Transfer Intent as an additional field:

| Field | Description |
|-------|-------------|
| `fee` | Optional. Asset type, amount, and recipient (source domain, destination domain, or burned). |

The source domain collects the fee atomically with the escrow in Phase 1. When the object is locked (Escrowed), the fee amount is deducted from the owner's inventory as a separate Transform (burn or transfer to the domain's treasury). Both operations — escrow the object, collect the fee — happen in the same local transaction.

**Rollback refunds the fee.** If the transfer times out or is cancelled, the escrow rolls back and the fee is refunded. The fee collection and escrow are a single local transaction — they commit or roll back together. The fee is only permanently collected when the Departure Proof is persisted (Phase 4). At that point the transfer is committed, and the fee is earned.

**Who sets fees:** Domain policy. The source domain can require a fee for outbound transfers. The destination domain can require a fee for inbound transfers (communicated during the bilateral capability negotiation). Both fees are declared in the Transfer Intent and validated by both sides. If the fees don't match what each domain expects, the transfer is rejected.

**Law 3 compliance:** The Transfer Intent declares the fee explicitly. The Departure Proof includes the fee Transform. Any auditing domain can verify that the declared fee was collected — no hidden sinks. The fee is a designed entropy term in the conservation equation: `value_out = value_in - declared_fees`.

## Third-Party Observability

When a third-party domain (C) queries about an object that's mid-transfer between A and B, what does it see?

**Source domain (A) responds based on object state:**

| Object state | Response to third-party query |
|--------------|-------------------------------|
| Active | Normal object metadata. |
| Escrowed | Object exists, locked for transfer. Transfer destination disclosed. |
| Departed | Object has departed. Departure Proof available (proves where it went). |

**Destination domain (B) responds based on its state:**

| Transfer state | Response |
|----------------|----------|
| Pending | Transfer in progress, not yet committed. |
| Arrived | Normal object metadata (B is the new host). |

The TransferStatus message handles this. Any domain with a valid capability can query either side for the current state of a known transfer. For general object queries (not transfer-specific), the source domain's response includes transfer state if the object is escrowed or departed.

**Gossip implications.** A Departed object creates a visible record. Trading partners that query about the object learn it moved and where it went. This feeds into the bilateral observation mechanism — if A claims an object is still in its inventory but B holds a Departure Proof showing A committed the transfer, the discrepancy is detectable.

## Leden Wire Messages

The transfer protocol maps to Leden Layer 3 (Object) messages:

| Message | Fields | Direction |
|---------|--------|-----------|
| `TransferOffer` | `object_content`, `proof_chain`, `transfer_intent` | S → D |
| `TransferAccept` | `transfer_id`, `dest_signature` | D → S |
| `TransferReject` | `transfer_id`, `reason` | D → S |
| `TransferCommit` | `transfer_id`, `departure_proof` | S → D |
| `TransferComplete` | `transfer_id`, `arrival_proof` | D → S |
| `CancelTransfer` | `transfer_id`, `owner_signature` | Owner → S |
| `TransferAbort` | `transfer_id`, `reason` | S → D |
| `TransferStatus` | `transfer_id` | S ↔ D (query/response) |

`TransferStatus` is the recovery primitive. Either side can query the other for the current state of a transfer. Idempotent, safe to retry, essential for partition recovery.

All messages ride on Leden sessions with capability-based access control. The destination must hold a transfer inbox capability (received from the source domain's greeter or through a Grant). The source must hold a transfer capability for the destination (received through introduction or bilateral agreement).

## What This Doesn't Cover

- **Batch transfers.** Moving N objects at once. The protocol handles one object per transfer. Concurrent transfers are an optimization, not a protocol change.
- **Atomic swaps.** "I give you X, you give me Y, atomically." Requires both objects escrowed simultaneously, swap commits only if both sides accept. Needs its own spec — the coordination is meaningfully different from unidirectional transfer.
- **Cross-domain auctions.** Multi-party coordination with promise pipelining + escrow. Deferred.

These build on the base transfer protocol but add enough coordination complexity to warrant separate specs.
