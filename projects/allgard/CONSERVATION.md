<!-- id: allgard.conservation -->
<!-- status: proposed -->
<!-- summary: The seven conservation laws of the federation -->

# Conservation Laws

Seven invariants the federation enforces unconditionally. No application, no domain admin, no script can violate them. If a state transition violates a law, it's rejected.

These are the physics of the federation.

## Law 1: Conservation of Supply

> For every asset type, `total_minted - total_burned = total_existing`.

Every unit of every asset is accounted for. The ledger always balances. Minting and burning are explicit, auditable operations with defined authority.

### Implications

- Every `create` Transform that introduces a new asset must be authorized by a minting authority
- Every `destroy` Transform is permanent and logged
- The total supply of any asset type is always computable from the log
- Duplication is impossible: you can't create value without the minting authority

### Verifiable Minting

Every `create` and `destroy` Transform must be backed by a [Raido](../raido/) script. The script is content-addressed — any domain can fetch it and re-execute to verify the mint or burn independently.

This is not optional. General transforms (transfer, mutate, split, merge) can use trust-based Proofs or optionally verifiable Raido Proofs. But minting and burning — the operations that change total supply — are always verifiable. If you can't re-execute the minting logic, you can't audit supply. And if you can't audit supply, Law 1 is just a claim.

A domain still writes its own minting logic. Sovereignty is over *policy* (what to mint, when, how much), not over *auditability* (whether the policy is verifiable). You can mint whatever you want. You just can't hide how.

### Per-Domain Sovereignty

Each domain mints independently. Cross-domain value is market-determined through bilateral exchange. No shared mint authority — that would reintroduce centralization.

Commodity money emerges naturally: assets with intrinsic utility become de facto currencies. The [founding cluster](README.md#founding-cluster) seeds the first commodity money by convention, giving it initial liquidity. The protocol provides auditability (this law), asset type registration (catalog observation), and bilateral exchange. Convention handles the rest.

See [Domain Sovereignty over Supply](README.md#domain-sovereignty-over-supply) for full rationale.

## Law 2: Singular Ownership

> Every Object has exactly one Owner at every point in time. Ownership transfers are atomic.

No duplication. No orphans. An Object is in at most one Domain's inventory at any point in time — never two.

Within a domain, transfer is atomic: remove from A, add to B, in one transaction. Across domains, the bilateral escrow protocol maintains the same invariant under network failure — see [Cross-Domain Transfer](#cross-domain-transfer).

### Cross-Domain Transfer

Across domains, atomicity requires a bilateral escrow protocol. The source domain escrows the object (locks it), the destination validates and accepts, then the source commits irrevocably by persisting a Departure Proof. The protocol guarantees that at no point do two domains simultaneously hold the same object in their inventories.

There is a brief window during transfer where the object is "in transit" — the source has committed but the destination hasn't registered. During this window the object is in zero inventories, not two. Ownership is unambiguous (determined by the Transfer Intent), and recovery is guaranteed by the source's persistent Departure Proof and the owner's wallet.

See [TRANSFER.md](TRANSFER.md) for the full protocol specification, failure modes, timeout semantics, and the formal proof that Law 2 holds under network partition.

### Implications

- No concurrent mutation (only the owner can authorize Transforms)
- No CRDTs needed for the base case
- Cross-domain transfer is a bilateral escrow protocol — the object must leave one domain before entering another, with the source domain as escrow authority
- Shared access is through Grants, not shared ownership

## Law 3: Conservation of Exchange

> In any transaction, the sum of value leaving participants equals the sum of value entering participants, minus explicit fees and sinks.

You can't conjure value. If a transfer gives Owner A an asset, something of declared value leaves Owner A.

The "minus sinks" clause is critical — fees, depreciation, processing costs are *designed entropy*. Without value sinks, economies inflate to meaninglessness. But sinks must be declared in the transaction type, not hidden.

### Designed Entropy

Without value sinks, supply only grows. Every mint adds, nothing subtracts, and the economy inflates to meaninglessness. Sinks are the counterweight — planned destruction that keeps the system in equilibrium.

Categories of sinks:

- **Processing fees**: operations that consume value (crafting, refining, combining)
- **Maintenance costs**: upkeep drains on long-lived assets (repair, storage, hosting)
- **Transaction fees**: cross-domain transfers cost something. Small, but bounds spam and drains supply.
- **Decay**: some asset types degrade over time (consumables, temporary grants, perishable goods)

The specific sinks are domain policy — a game domain has crafting loss, a compute domain has CPU credits. The Conservation Law doesn't dictate which sinks exist. It requires that sinks are declared in the transform type, not hidden. A domain that claims "free repairs" and quietly destroys inventory is violating the law.

Sinks should be tunable per domain. The law enforces that declared sinks match actual destruction.

## Law 4: Causal Ordering

> Every state mutation references a prior valid state. No state can be derived from a state that doesn't exist yet.

This prevents replay attacks, fork-based duplication, and state corruption. Every Transform has a parent hash. The system maintains a DAG, not just current state.

### Implications

- Every Transform includes a reference to the state it operates on
- Out-of-order Transforms are rejected (they reference a state that's already been superseded)
- History is append-only: you can add corrections, but not erase entries
- Domain rollbacks are permitted (for bug fixes) but the rollback itself is logged as a new entry

### Not Immutability

I considered making history immutable but rejected it. It conflicts with domain sovereignty. If a domain admin needs to roll back a bug, they should be able to. The constraint is weaker but sufficient: history is *append-only*. Corrections are new entries, not edits to old ones.

## Law 5: Bounded Rates

> Every operation type has a maximum frequency per entity per time window.

Even if a Transform is technically valid, you can't execute 10,000 transfers per second. Rate bounds are per-operation-type, configurable per domain, but always present.

Physics has the speed of light. We have rate limits.

### Why Not Gas

I considered a gas model (like Ethereum) but rejected it. Gas is UX poison — it makes every action cost something, which kills casual interaction. Rate limits cover the abuse case (no infinite loops, no spam) without burdening normal use.

## Law 6: Authority Scoping

> An operation can only affect Objects the initiator has authority over. Authority is non-transitive by default.

A process in Domain X can't touch Objects in Domain Y. An Owner's script can't modify another Owner's inventory. Authority must be explicitly granted and is always scoped — to a domain, a session, a specific object set.

### Non-Transitivity

If Owner A grants Owner B authority over Object X, Owner B does *not* automatically gain authority to grant that authority to Owner C. Delegation requires explicit permission from the grantor (the `delegatable` flag on a Grant).

Deliberate departure from some capability systems where capabilities are freely transferable. Free transferability makes revocation nearly impossible — once you hand out a capability, it can spread uncontrollably. Non-transitive-by-default keeps the authority graph manageable.

### Implications

- Every Transform is checked against the initiator's authority
- Authority is always traceable back to its source
- Revocation is practical because the delegation graph is controlled
- Cross-domain operations require explicit Grants, not ambient authority

## Law 7: Conservation of Introduction Capacity

> Each domain has a finite, non-transferable budget of introductions per time window. Capacity regenerates over time but cannot be borrowed, pooled, or delegated.

Law 5 bounds operation rates. Law 7 is the same principle applied to the specific operation that grows the trust graph: introduction. The thing being conserved is **introduction capacity** — each domain's ability to vouch for newcomers is a scarce resource, consumed on use, regenerated by time.

Without this, a single hub domain can vouch for unlimited newcomers per day and become the sole gateway to the network. That's how Gmail ate email and centralized platforms ate XMPP.

Sybil resistance depends on this being a law, not a suggestion.

### Parameters

| Parameter | Description | Default |
|-----------|-------------|---------|
| `window` | Time period for rate measurement | 30 days |
| `base_rate` | Introductions per window for `known` domains | 5 |
| `allied_rate` | Introductions per window for `allied` domains | 20 |
| `stranger_rate` | Introductions per window for `stranger` domains | 0 |

Rates are **per-introducer, not per-owner.** A domain with 100 owners doesn't get 100× the introduction budget. The domain itself is the entity being rate-limited — it's the domain's reputation on the line for every introduction it makes.

### Trust Level Scaling

Introduction capacity scales with the introducer's trust level (see [Trust](TRUST.md)):

| Trust Level | Budget | Rationale |
|-------------|--------|-----------|
| Stranger | 0 | Can't introduce others until you're introduced yourself |
| Known | `base_rate` | Earned basic trust, limited introduction capacity |
| Trusted | 2× `base_rate` | Sustained clean track record, more vouching capacity |
| Allied | `allied_rate` | Deep bilateral relationship, highest capacity |

A domain's effective introduction rate is what the *receiving* domain assigns based on trust level. Domain A might consider B "allied" (20/month budget) while Domain C considers B merely "known" (5/month budget). Same introducer, different budgets — because trust is bilateral.

### Interaction with Law 5

Law 5 (Bounded Rates) covers operation frequency. Law 7 covers introduction frequency. They're complementary:

- Law 5 prevents a domain from spamming transforms. Law 7 prevents it from spamming the trust graph.
- A domain hitting its Law 5 rate limit on introductions is also hitting Law 7 — but Law 7 is stricter because the window is longer (30 days vs per-second operation limits).
- Law 7 doesn't replace Law 5's per-operation rate limits on `Introduce` messages. Both apply. Law 5 prevents bursts (can't send 5 introductions in 1 second). Law 7 prevents sustained flooding (can't send 50 introductions in a month).

### Enforcement

Same as Law 5 — bilateral verification, no central enforcer.

The receiving domain tracks how many introductions it has accepted from each source domain within the current window. Excess introductions are rejected with `RateLimitExceeded`.

**Specifics:**

1. **Receiving domain enforces.** The introducer can *attempt* more introductions than its budget allows. The receiving domain rejects the excess. No honor system.
2. **Receiver's clock governs the window.** The rolling window is measured by the receiving domain's wall clock at time of acceptance, not the introducer's timestamp. The introducer's timestamp is relevant for causal ordering (Law 4), not rate limiting. This avoids clock-skew gaming — a domain with a fast clock can't get introductions to age out early.
3. **Window is rolling, not fixed.** A 30-day rolling window, not calendar months. Prevents gaming by timing introductions at month boundaries.
4. **Rejected introductions are logged.** A domain that consistently exceeds its introduction budget is either misconfigured or probing. Persistent over-attempts feed into the trust model as a negative signal.
5. **Budget recovery is immediate.** When an introduction exits the rolling window (older than 30 days), that slot is freed. No penalty for using your full budget.
6. **Introducer identity is the domain, not the owner.** In Leden, the `Introduce` message is sent by an endpoint (owner) over a session. The receiving domain maps the sender to its source domain via the session's domain identity. All owners on Domain A share Domain A's introduction budget. A domain can't circumvent the limit by routing introductions through different owners.

### Bootstrap Exception

The [founding cluster](README.md#founding-cluster) bootstraps with pre-negotiated bilateral trust. Founding members start at `allied` trust level with each other by convention — they don't go through the stranger→known→trusted progression. This gives them the `allied_rate` introduction budget from day one, which is necessary to seed the network with enough introductions to reach critical mass.

This is not a protocol-level exception. It's a configuration choice: founding members configure each other as `allied` in their initial trust state. Any domain can do this for any other domain — the founding cluster just does it before the network exists.

### Why This Works

- **Caps hub dominance.** A domain limited to 5 introductions per month can't become the sole gateway.
- **Forces distributed introduction.** Newcomers must find multiple introduction paths, diversifying the graph.
- **Quality over quantity.** Limited budget means each introduction is more valuable and worth more due diligence.
- **Sybil bottleneck.** Creating 10,000 Sybil domains is easy. Getting them introduced at 5 per month per introducer is slow. The attacker needs many compromised introducers or years of patience.

## Enforcement

Conservation laws are *local invariants with bilateral verification*. They're not global invariants — no one has a complete view. This is deliberate, and it matters to understand what each level actually guarantees.

### Level 1: Within a Domain (Strong)

The domain's runtime validates every Transform against all seven laws before applying it. This is the strongest guarantee: if the runtime is correct, violations are structurally impossible within a single domain. Same as a database enforcing its own constraints.

### Level 2: Across Domains (Bilateral)

Each domain checks the other's Proofs during cross-domain operations. Domain B re-verifies every Proof that Domain A produces. If the Proof violates a law, B rejects it. This catches:

- Invalid transfers (Law 1 — supply doesn't balance)
- Ownership conflicts (Law 2 — object claimed by two owners)
- Unbalanced exchanges (Law 3 — value conjured from nothing)
- Broken causal chains (Law 4 — references non-existent prior state)
- Rate violations (Law 5 — too many operations too fast)
- Authority overreach (Law 6 — operating on objects without proper Grants)
- Introduction flooding (Law 7 — too many introductions from one source)

Bilateral verification is strong for operations that cross boundaries. It's the equivalent of a bank verifying a wire transfer.

### Level 3: Network-Wide (Structural + Probabilistic)

There is no global enforcer. But [verifiable minting](#verifiable-minting) closes the biggest gap: every mint and burn is a Raido script that any trading partner can re-execute. A domain can't secretly inflate supply — the minting logic is content-addressed and independently verifiable.

What remains undetectable: internal activity that never crosses a boundary *and* doesn't involve minting or burning. A domain that mutates objects internally without exporting them is opaque. That's fine — if it never crosses a boundary, it doesn't affect anyone else.

What makes remaining fraud detectable:

- **Overlapping partial views.** Every domain that trades with A accumulates a partial view of A's economy. These views overlap. If A's self-reported numbers don't reconcile with what its trading partners have independently witnessed, the discrepancy surfaces through [audit gossip](README.md#supply-audit).
- **Proof chain inclusion.** Supply audits include Proof chains. A verifying domain can check that the events it witnessed appear in A's published chain. Missing events mean a fraudulent audit.
- **Verifiable minting scripts.** Mint and burn operations are Raido scripts. A supply audit now includes not just totals but the scripts that produced them. Re-execute the scripts, check the totals. This is mechanical, not trust-based.

### What This Means in Practice

| Guarantee | Strength | Mechanism |
|-----------|----------|-----------|
| Violations within a domain | Prevented | Runtime enforcement |
| Violations in cross-domain ops | Detected and rejected | Bilateral Proof verification |
| Supply inflation/deflation | Mechanically verifiable | Raido-backed mint/burn (required) |
| Hidden internal mutations | Undetectable | Accepted — doesn't affect other domains |
| Computational fraud (general) | Mechanically verifiable | Raido re-execution (optional for non-mint transforms) |

The enforcement model has no "trust me" gap for supply. Minting is verifiable by construction. General transforms can optionally be verifiable too (domains negotiate this bilaterally). Internal mutations that never cross boundaries are the only blind spot, and they're harmless.
