# Contracts
<!-- id: apeiron.contracts --> <!-- status: proposed --> <!-- summary: Standard contract templates — escort, bounty, joint venture, insurance, research -->

[ECONOMY.md](ECONOMY.md) defines three job types: courier, mining, and facility rental. All compose from Allgard primitives. This spec extends the pattern to more complex agreements that emerge once an economy has players with different capabilities and goals.

I don't want a contract system. I want standard templates that compose from existing primitives — Grants, conditional Transfers, Transforms, and Objects. A domain that invents a new contract type uses the same primitives. The founding cluster publishes templates so that contracts are interoperable across domains, not because contracts need special mechanics.

## Contract Anatomy

Every contract follows the same structure, regardless of type:

```
contract:
  id: <object_id>
  type: <template_id>              # Standard template hash (e.g., "escort_v1")
  issuer: <domain_id>              # Who posted it
  terms:
    <type-specific terms>
  escrow:
    <locked objects/credits>       # Issuer's commitment, held by issuer's domain
  acceptance:
    <acceptor_id>                  # Null until accepted
    <acceptor_signature>
  status: open | active | completed | failed | expired
  expiry: <beacon_epoch>
  proof_refs: [<proof_id>, ...]    # Completion/failure proofs
```

A contract is an Allgard object. Creating one locks the escrow (conditional Transfer). Accepting one commits the acceptor. Completion triggers escrow release. Failure triggers rollback or penalty. All standard Allgard transfer mechanics.

The `type` field references a standard template by content hash. Templates are Raido scripts that define validation logic: what inputs are required, what constitutes completion, what triggers failure. The founding cluster publishes the initial templates. Anyone can publish new ones.

## Escort Contracts

Protect cargo (or a ship) during transit through a route domain or series of jumps.

### Terms

```
terms:
  cargo_id: <object_id>           # What's being protected
  route: [<domain_id>, ...]       # Waypoints
  threat_level: low | medium | high
  payment: <amount>
  bonus_on_zero_loss: <amount>    # Optional: extra payment if cargo takes no damage
```

### Mechanics

The issuer posts the contract with payment locked in escrow. The escort accepts and receives a conditional Grant from the cargo owner: `scope: combat_authority` over the route domains. This Grant lets the escort engage hostiles near the cargo (consent-on-entry for the escort's combat actions is pre-authorized by the cargo owner's Grant, scoped to the specific route).

**Completion:** Cargo arrives at the final waypoint. The destination domain's TransferComplete proof triggers escrow release. If cargo took no damage and a zero-loss bonus exists, bonus releases too.

**Partial completion:** Cargo arrives damaged. Payment releases (the escort did their job). No bonus.

**Failure:** Cargo is destroyed en route. Escrow returns to the issuer. The escort gets nothing. If the escort abandoned the cargo (jumped away), attestation records it as a contract abandonment.

### Why This Works

Escort is just a conditional transfer (payment) linked to another conditional transfer (cargo delivery), plus a combat Grant. No escort-specific protocol. The complexity is in the template's validation logic, not in new primitives.

## Bounty Contracts

Destroy a specific target. Payment on proof of destruction.

### Terms

```
terms:
  target_id: <object_id>          # The ship/station to destroy
  target_owner: <owner_id>        # Who owns it
  target_last_seen: <domain_id>   # Intelligence: where it was last observed
  payment: <amount>
  evidence_required: destruction_proof | damage_proof
  expiry: <beacon_epoch>
```

### Mechanics

The issuer locks payment in escrow. Any entity can claim the bounty — no acceptance required. This is an open contract.

**Completion:** A claimant presents a destruction proof — a debris field object whose proof chain references the target's object ID, hosted on the domain where the destruction happened. The bounty contract's validation script verifies: (1) the debris derives from the correct target, (2) the destruction event is valid (standard combat script output), (3) the beacon epoch is after the contract's creation epoch. Escrow releases to the claimant.

**Damage bounties.** Variant: payment for verified damage to the target, not destruction. The claimant presents a combat log showing damage dealt to the target. Partial payment proportional to damage. Harder to verify — needs the combat domain's cooperation to attest damage. More suitable for targets that are too well-defended to destroy.

**Expiry.** If nobody claims the bounty by the deadline, escrow returns to the issuer.

### Risks

Bounties can be gamed. The target's ally accepts the bounty, then the target scuttles a decoy ship with the same ID. Mitigation: the destruction proof includes the destroyed object's full component tree. The bounty issuer can verify the destroyed object was the real target (mass, materials, equipment consistent with a real ship, not a empty hull). Sophisticated gaming is possible but expensive — building a convincing decoy costs real materials.

## Joint Venture Contracts

Two or more parties invest resources and share output. Mining operations, research partnerships, construction projects.

### Terms

```
terms:
  parties: [<owner_id>, ...]
  investments:
    <owner_id>: [<object_id>, ...]   # What each party contributes
  output_split:
    <owner_id>: <fraction>           # How output is divided
  operation: <transform_script_hash> # What the venture does
  host_domain: <domain_id>           # Where the operation runs
  duration: <beacon_epochs>
```

### Mechanics

Each party transfers their investment to the host domain via conditional Transfer. All investments must arrive before the operation begins — the contract's validation script checks that all committed objects are present. If any party fails to deliver, all transfers roll back.

**Operation phase.** The host domain runs the Transform specified by the operation script. Inputs: the invested objects. Outputs: whatever the operation produces (materials, components, discoveries). Output objects are created with ownership split per the contract terms — each party receives their fraction directly from the Transform.

**Output split.** For divisible outputs (bulk materials), the split is straightforward — each party gets their percentage by mass. For indivisible outputs (a single component, a discovery), the contract specifies a resolution: auction among parties, round-robin assignment, or the host domain holds it in escrow until parties negotiate.

**Early termination.** If a party wants out, remaining parties can buy out their share (bilateral negotiation) or the venture dissolves. Dissolution returns remaining inputs proportional to investment, minus any consumed inputs. Messy — like real joint ventures dissolving.

### Research Partnerships

A specialization of joint venture. One party contributes materials and fuel (the investor). The other contributes facility access and expertise (the researcher). Output is knowledge — a discovered recipe, a resonance map, a material property insight.

The tricky part: knowledge, once observed, can be copied. The investor funds 500 experiments. The researcher observes all results. How does the investor ensure the researcher shares the findings?

**Sealed knowledge escrow.** The researcher's facility produces output objects for each experiment. The experiment results (input ratios, energy, output properties) are encoded in the output objects. These objects are owned by the joint venture (split ownership). Neither party can access the results without the other's cooperation — the objects are held on the host domain with a Grant that requires both parties to observe.

This isn't cryptographically enforced — it relies on the host domain's honesty. But the host domain has reputation at stake, and the experiment proofs are verifiable. If the researcher secretly uses the results, the investor can prove the experiments happened (proof chain) and that the researcher had access (Grant records). Reputation damage.

Imperfect. But real research partnerships have the same trust problems. The contract reduces risk, it doesn't eliminate it.

## Insurance Contracts

One party pays a premium. If a specified loss event occurs, the insurer pays compensation.

### Terms

```
terms:
  insured: <owner_id>
  insured_object: <object_id>      # What's covered
  premium: <amount>                 # Payment per epoch
  premium_interval: <beacon_epochs>
  coverage: <amount>               # Maximum payout
  covered_events:
    - destruction                  # Combat, stress failure
    - damage_above: <threshold>    # Significant damage
    - theft                        # If provable (failed transfer)
  deductible: <amount>             # Insured pays first N credits of loss
  expiry: <beacon_epoch>
```

### Mechanics

**Premium payment.** The insured transfers credits to the insurer at regular intervals. Standard bilateral transfers. If a premium payment is missed, coverage lapses (the contract's validation script tracks premium timestamps).

**Claim.** The insured presents proof of a covered event — destruction proof (debris field from their ship), damage proof (combat log showing damage to their ship), or theft proof (failed transfer / unauthorized capability exercise). The contract's validation script verifies the event matches a covered category.

**Payout.** The insurer transfers the coverage amount minus deductible to the insured. If the insurer doesn't pay, the contract records a failure — reputation damage to the insurer.

### Why Insurance Matters

Insurance spreads risk. A player with an expensive ship fears losing it in combat. Insurance lets them take risks they otherwise wouldn't. This increases combat participation, trade through dangerous routes, and frontier exploration.

Insurance also creates a role: the insurer. A domain (or faction) with deep reserves can profit by pooling risk across many insured parties. Same economics as real insurance — the insurer profits when claims are rare, loses when they cluster. A faction war that destroys 50 insured ships at once is an insurance crisis.

### Moral Hazard

Insured players take more risks. A fully insured ship charges into combat with less to lose. This is intentional — it makes the game more dynamic. But it creates demand for investigation: did the player deliberately scuttle their ship for the insurance payout? The proof chain helps — a scuttled ship's destruction proof differs from a combat destruction proof (no opposing fleet, no combat log). The insurer's validation script can check.

Sophisticated fraud (arrange to lose a "combat" with an ally) is harder to detect. Same problem real insurers face. The solution is the same: investigation, claims adjustors, and the fact that fraud requires a collaborator (who might talk).

## Hire Contracts

Hire a player (or AI) for a duration to perform a role.

### Terms

```
terms:
  role: miner | guard | researcher | hauler | <custom>
  employer: <domain_id>
  employee: <owner_id>
  duration: <beacon_epochs>
  payment: <amount>
  payment_schedule: per_epoch | on_completion | milestone
  grants: [<grant_scope>, ...]     # What the employee can do
  termination: mutual | employer_only | with_penalty
```

### Mechanics

The employer locks total payment in escrow. On acceptance, the employee receives Grants per the contract terms — facility access, extractor operation, combat authority, whatever the role requires. Payment releases on schedule (per-epoch transfers, milestone triggers, or lump sum on completion).

**Early termination.** If the contract allows employer-only termination, the employer revokes the Grants and pays a pro-rated amount. If termination requires penalty, the penalty releases from escrow to the employee. If mutual only, both must agree.

This generalizes mining contracts from ECONOMY.md. A mining contract is just a hire contract where `role: miner`, `grants: [operate_extractor]`, and `payment_schedule: per_epoch` with output split encoded in the extractor's minting script.

## Contract Discovery

How do players find contracts? No global job board — that's a central service. Instead:

**Domain boards.** Each domain publishes available contracts in its Leden metadata or through observation queries. A player docked at a station can query "what contracts are available here?" The domain responds with its open contracts.

**Faction boards.** Factions aggregate contracts from member domains. A faction member queries the hub and sees contracts across all faction systems. Information advantage from membership.

**Trade hub aggregation.** Trade hub domains can aggregate contracts from nearby systems (bilaterally shared). The hub becomes a job board because it's economically useful to be one. Not prescribed — emergent from the hub's role as information broker.

**Word of mouth.** Player A tells player B about a contract at domain C. Social information. The most interesting contracts (high-value bounties, rare research opportunities) propagate through social networks, not bulletin boards.

## Stage 1 Testing

All contract types testable in the monolith:

- **Escort:** AI escort protects AI hauler through route. Verify payment on delivery. Simulate ambush — verify escort engages, cargo survives or doesn't, payment adjusts correctly.
- **Bounty:** Post bounty on AI ship. AI bounty hunter destroys it. Verify destruction proof validates, escrow releases.
- **Joint venture:** Two AI domains invest materials in a shared mining operation. Verify output split matches contract terms. Test early termination and dissolution.
- **Insurance:** Insure an AI hauler. Destroy it. Verify claim process, payout, and premium history tracking. Test moral hazard detection (scuttling vs. combat destruction proofs).
- **Hire:** AI researcher hired by AI domain. Verify Grant issuance, payment schedule, and termination mechanics.
- **Discovery:** Verify domain boards list open contracts. Test a trade hub aggregating contracts from neighboring systems.

## What This Spec Doesn't Cover

**Auction contracts.** Multiple parties bid on a contract (e.g., multiple escorts competing for a job). The template could support this — acceptance requires beating the current best offer. But auction mechanics add complexity. Defer until basic contracts are working.

**Multi-stage contracts.** A construction project with milestones: deliver materials by epoch X, assemble by epoch Y, deliver finished product by epoch Z. Each milestone triggers partial payment. Composable from sequential contracts, but a unified template would be cleaner. Defer.

**Dispute resolution.** What happens when parties disagree about whether a contract was fulfilled? The validation script handles clear cases (proof chain says yes or no). Edge cases (partial completion, force majeure, ambiguous terms) need human judgment. In Stage 1, the domain operator arbitrates. In federation, bilateral reputation handles it — domains that rule unfairly lose trust.

**Contract law.** Factions may develop internal contract enforcement — courts, arbitration, precedent. This is social infrastructure built on top of contract objects. The protocol provides the primitives. Factions provide the jurisprudence.
