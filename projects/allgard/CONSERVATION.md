<!-- id: allgard.conservation -->
<!-- status: proposed -->
<!-- summary: The six conservation laws of the federation -->

# Conservation Laws

Six invariants the federation enforces unconditionally. No application, no domain admin, no script can violate them. If a state transition violates a law, it's rejected.

These are the physics of the federation.

## Law 1: Conservation of Supply

> For every asset type, `total_minted - total_burned = total_existing`.

Every unit of every asset is accounted for. The ledger always balances. Minting and burning are explicit, auditable operations with defined authority.

### Implications

- Every `create` Transform that introduces a new asset must be authorized by a minting authority
- Every `destroy` Transform is permanent and logged
- The total supply of any asset type is always computable from the log
- Duplication is impossible: you can't create value without the minting authority

### Decision: Per-Domain Sovereignty

Each domain mints independently. Cross-domain value is market-determined through bilateral exchange. No shared mint authority — that would reintroduce centralization.

Commodity money emerges naturally: assets with intrinsic utility become de facto currencies. The protocol provides auditability (this law), asset type registration (catalog observation), and bilateral exchange. Convention handles the rest.

See [Domain Sovereignty over Supply](README.md#domain-sovereignty-over-supply) for full rationale.

## Law 2: Singular Ownership

> Every Object has exactly one Owner at every point in time. Ownership transfers are atomic.

No duplication. No orphans. An Object is in exactly one inventory, hosted by exactly one Domain.

Transfer is atomic: remove from A, add to B, in one transaction.

### Implications

- No concurrent mutation (only the owner can authorize Transforms)
- No CRDTs needed for the base case
- Cross-domain transfer requires a bilateral protocol (the Object must leave one Domain before entering another)
- Shared access is through Grants, not shared ownership

## Law 3: Conservation of Exchange

> In any transaction, the sum of value leaving participants equals the sum of value entering participants, minus explicit fees and sinks.

You can't conjure value. If a transfer gives Owner A an asset, something of declared value leaves Owner A.

The "minus sinks" clause is critical — fees, depreciation, processing costs are *designed entropy*. Without value sinks, economies inflate to meaninglessness. But sinks must be declared in the transaction type, not hidden.

### Designed Entropy

Planned value sinks prevent inflation:

- **Processing fees**: operations that consume value
- **Maintenance costs**: upkeep drains on long-lived assets
- **Transaction fees**: cross-domain transfers cost something
- **Decay**: some asset types degrade over time

The specific sinks are domain policy. The Conservation Law just says they must be explicit and auditable.

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

## Enforcement

Conservation Laws are enforced at two levels:

1. **Within a domain**: the domain's runtime validates every Transform against all six laws before applying it
2. **Across domains**: bilateral verification during cross-domain operations. Each domain checks the other's Proofs.

There's no global enforcer. Trust is bilateral and capability-based. If a domain consistently violates Conservation Laws, other domains stop accepting its Proofs. Reputation is emergent, not administered.
