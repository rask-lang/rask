# Knowledge Economy
<!-- id: apeiron.knowledge --> <!-- status: proposed --> <!-- summary: Knowledge as tradeable commodity — formats for survey data, recipes, blueprints, doctrine, intel -->

Materials are the economy's blood. Knowledge is its brain. A player who discovers a ternary alloy with 40% better structural efficiency than anything published doesn't just have a good material — they have power. They can sell the output at premium. They can sell the recipe for a fortune. They can keep it secret and dominate a niche for months. The recipe is worth more than any single batch of material.

This spec defines how knowledge objects work — what they contain, how they're traded, what can be verified, and what stays opaque.

## The Problem

Knowledge in Apeiron has a copying problem. Once someone sees a recipe (element ratios, energy level, catalyst), they can reproduce it. Digital goods are infinitely copyable. If I sell you a recipe, you can sell it to everyone. The recipe's value collapses to zero after the first sale.

This isn't a problem to solve — it's a property to design around. Real knowledge economies work the same way (trade secrets, academic publishing, patent licensing). The solution isn't DRM. It's making knowledge valuable in context, not just in isolation.

## Knowledge Categories

### Recipes

A recipe is a set of transformation parameters that produce a known output: element types and ratios, energy level, catalyst (if any), and expected output properties. See [TRANSFORMATION.md](TRANSFORMATION.md) for the interaction function.

**What a recipe object contains:**

```
recipe:
  id: <object_id>
  discoverer: <owner_id>
  discovery_epoch: <beacon_epoch>
  inputs:
    elements: [{element: "Fe", fraction: 0.97}, {element: "C", fraction: 0.03}]
    energy_per_mass: 450
    catalyst: null
  output:
    properties:
      density: 7.85
      hardness: 0.72
      conductivity: 0.15
      reactivity: 0.08
      stability: 0.88
      radiance: 0.05
    name: "Structural Steel"            # Discoverer-assigned name
  facility_requirements:
    min_precision: 0.8                  # Minimum facility precision to reliably produce
    min_containment: 200                # Minimum containment rating
    min_reactor_output: 500             # Minimum reactor power
  verification:
    proof_ref: <proof_id>               # Points to a Transform proof of the first verified production
    script_hash: <hash>                 # Interaction function version
  notes: "Standard backbone material. Ratio is forgiving — ±2% still produces usable output."
```

**Verification.** The recipe includes a proof reference to an actual production Transform. Anyone can verify: re-run the interaction function with the stated inputs, confirm the output matches. The recipe is provably real — not a guess, not a lie.

**Partial recipes.** A discoverer might sell an incomplete recipe — the element pair and approximate energy range, but not the exact ratio. The buyer knows WHAT to research, but still needs to run experiments to find the peak. Partial recipes are cheaper and protect the discoverer's exact knowledge. There's no format difference — just omitted fields.

### Survey Data

Geological data from claimed star systems. Created at claim time per [EXPLORATION.md](EXPLORATION.md). Published as part of domain metadata. But raw survey data from YOUR system is free. The valuable knowledge is survey data from OTHER systems — especially unclaimed ones you've computed from the public beacon log.

```
survey_report:
  id: <object_id>
  surveyor: <owner_id>
  star_id: <int>
  body_id: <int>
  epoch: <beacon_epoch>
  deposits:
    - element: "Fe"
      quantity: 50000
      quality: 0.85
      extraction_sites: 3
    - element: "Ti"
      quantity: 8000
      quality: 0.72
      extraction_sites: 1
  verification:
    beacon_value: <value>
    seed: <galaxy_seed>
    # Anyone can re-run survey(seed, star_id, body_id, beacon_value) and check
```

**Value model.** Survey data for a claimed system is freely verifiable — the beacon value is public, the survey function is public. The value is in AGGREGATION: a report covering 50 systems in a region, cross-referenced with sky data and trade flows. Individual surveys are cheap. Regional intelligence is expensive to assemble and valuable to buyers planning expansion.

**Pre-claim intelligence.** For unclaimed systems, survey data doesn't exist yet (no beacon value). But sky data analysis (spectral class → likely deposits) IS valuable to a player deciding where to claim. A systematic analysis of 200 candidate systems, ranking them by likely resource value, is a knowledge product. Not verifiable (it's probabilistic), so it's priced by the analyst's reputation.

### Blueprints

A ship or facility design. The complete component tree — what components, what materials, what arrangement.

```
blueprint:
  id: <object_id>
  designer: <owner_id>
  design_epoch: <beacon_epoch>
  name: "Ironclad Hauler Mk.III"
  category: ship | facility | station | component
  component_tree:
    hull:
      material: "Hull Plate"
      mass: 2400
      children:
        - engine:
            material: "Silicon Carbide"
            mass: 180
            properties:
              thrust: 5200
              power_draw: 800
        - fuel_tank:
            material: "Light Alloy"
            mass: 120
            capacity: 3000
        - cargo_bay:
            material: "Structural Steel"
            mass: 600
            capacity: 8000
        # ... full tree
  derived_properties:
    total_mass: 4800
    thrust_to_weight: 1.08
    fuel_range: 24                     # jumps at average distance
    cargo_capacity: 8000
    power_budget: {draw: 1200, supply: 1500}
  bill_of_materials:
    "Hull Plate": 2400
    "Structural Steel": 1200
    "Silicon Carbide": 420
    "Light Alloy": 380
    "Conductor Wire": 400
  verification:
    physics_script_hash: <hash>        # Constraint physics version used for derivations
```

**Verification.** Anyone can verify the derived properties by evaluating the component tree against the constraint physics script. If the numbers don't match, the blueprint is wrong. The physics script hash is included so verifiers know which version was used.

**Value model.** The blueprint itself is reproducible. But a GOOD blueprint — one that optimizes the constraint physics tradeoffs cleverly — represents real design work. Players who excel at system design (Level 2 transformation, per TRANSFORMATION.md) produce blueprints that others can't easily replicate even with the same materials. The creativity is in the arrangement, not the components.

**Parametric blueprints.** A Raido script that takes material properties as input and outputs an optimized component tree. More valuable than a static blueprint because it adapts to available materials. "Give me what you have, I'll give you the best hauler I can design from it." These are the designer's real intellectual property.

### Combat Doctrine

Execution scripts for combat. Per [COMBAT.md](COMBAT.md), the standard execution script is published. Custom scripts are tradeable.

```
doctrine:
  id: <object_id>
  author: <owner_id>
  name: "Skirmisher Wolfpack v2"
  script_hash: <hash>                 # Raido bytecode
  compatible_with:
    combat_script: <standard_combat_hash>   # Which standard combat script version
  description: "Coordinated skirmisher tactics. Ships spread to maximize evasion,
                converge for alpha strikes on priority targets, disengage when
                outnumbered 3:1."
  recommended_fleet:
    min_ships: 3
    max_ships: 8
    ship_role: "evasive skirmisher"
  performance_claims:
    vs_brawlers: "favorable at range, disengage if closed"
    vs_artillery: "flanking approach, high casualties expected"
    vs_mixed: "target artillery first, avoid brawler engagement"
  verification:
    test_results: [<proof_id>, ...]    # Optional: combat sim results as proofs
```

**Value model.** Doctrine scripts are executable — you can test them in simulation before buying. But testing against a specific opponent's doctrine requires knowing what that opponent runs. Buying a doctrine that counters a rival faction's known tactics is high-value intelligence.

**Obfuscation.** The buyer receives the script hash and can execute it. But Raido bytecode is readable by anyone who understands the instruction set. A skilled player can reverse-engineer a doctrine from the bytecode. This is intentional — doctrine advantages are temporary. The meta evolves.

### Intelligence Reports

Aggregated knowledge about specific entities, domains, regions, or markets. The most subjective category — valued entirely by the source's reputation.

```
intel_report:
  id: <object_id>
  source: <owner_id>
  epoch: <beacon_epoch>
  category: market | military | diplomatic | geographic
  subject: <description>
  content:
    # Freeform structured data. Examples:
    # Market: price trends, supply/demand shifts, arbitrage opportunities
    # Military: fleet compositions, patrol routes, defense gaps
    # Diplomatic: faction tensions, alliance shifts, upcoming conflicts
    # Geographic: promising unclaimed regions, resource density estimates
  sources_cited: [<proof_id>, ...]     # Optional: backing evidence
  confidence: high | medium | low
  expiry: <beacon_epoch>               # Intel goes stale
```

**Value model.** Intel can't be verified independently (that's what makes it intelligence). It's priced by the source's reputation and the buyer's need. A market report from a trade hub with 500 verified trades is worth more than one from an unknown scout. The reputation system (see [REPUTATION.md](REPUTATION.md)) is what makes intel tradeable.

**Staleness.** Intel has a shelf life. Military disposition from 100 epochs ago is useless. The `expiry` field is self-reported — the source estimates when the intel goes stale. Buyers decide whether to trust that estimate.

## The Knowledge Lifecycle

### Discovery

New knowledge is created through experimentation (recipes, resonance maps), exploration (survey data, regional analysis), design (blueprints, doctrine), or observation (intel reports). Each creation act has a cost — experiments burn materials and fuel, exploration burns fuel, design requires expertise and time, observation requires being present.

### Valuation

Knowledge value depends on:
- **Scarcity.** How many people know this? A ternary alloy recipe known to 3 players is worth more than one known to 300.
- **Utility.** How useful is it? A recipe for a material that's 5% better than published alternatives is worth less than one that's 40% better.
- **Verifiability.** Can the buyer check before paying? Recipes with proof refs are worth more than claims without evidence.
- **Exclusivity.** Is this a one-time sale or a license? A recipe sold exclusively (the seller promises not to sell again) is worth more. But the promise is unenforceable — it's backed by reputation.
- **Recency.** How fresh is this? Intel stales. Recipes don't stale but their economic advantage erodes as more players discover similar recipes independently.

### Trade

Knowledge objects trade like any Allgard object — bilateral Transfer. The founding cluster publishes the standard object formats above so that knowledge objects from different sources are structurally comparable.

**Preview.** The seller can share partial knowledge before the trade. "I have a ternary alloy recipe using Fe, Cu, and one other element. Output hardness is 0.94. Interested?" The buyer evaluates the partial information and decides whether to pay for the full recipe. The preview is just conversation — no protocol support needed.

**Bundled knowledge.** A research domain might sell a "materials starter pack" — 20 binary recipes, 5 ternary recipes, a regional survey analysis, and a hauler blueprint. Bundled as a set of knowledge objects in a single Transfer. Volume discount.

### Propagation

Once sold, knowledge propagates. The buyer can resell. Each resale reduces the remaining scarcity premium. The first buyer pays the most. The hundredth buyer pays the marginal cost of the Transfer.

This is fine. Discoverers profit from early sales. As knowledge spreads, its economic advantage shrinks but the galaxy gets richer overall. The discoverer's incentive is to keep discovering — stay ahead of the commoditization curve.

**Faction knowledge pools.** Factions aggregate member discoveries into a shared library (per [FACTIONS.md](FACTIONS.md)). Faction membership grants access to the pool. This is one of the strongest incentives for joining a faction — the collective knowledge base exceeds what any individual could assemble.

## What Can't Be Copied

Not everything is pure information. Some knowledge is embedded in physical assets:

**Facility precision.** A research domain with high-precision instruments can target narrow phase regions that a crude workshop can't. Knowing the recipe doesn't help if your facility can't produce it. The facility is a physical moat.

**Material stockpiles.** Knowing that platinum catalyzes a reaction is useless if you don't have platinum. Rare elements are physical bottlenecks that knowledge alone can't bypass.

**Reputation.** "Trusted recipe vendor" takes time to build. A new seller with the same recipes but no reputation can't command the same price.

**Tacit expertise.** A player who's run 10,000 experiments has intuition about the interaction function that can't be encoded in a recipe object. They know which regions of phase space are promising, which catalyst hints to follow, which energy levels to explore. This knowledge is in the player, not the objects.

## Stage 1 Testing

Knowledge trading is testable in the monolith:

- **Recipe discovery and trade.** AI researcher discovers a recipe. Posts it for sale. AI buyer purchases. Verify the recipe object transfers correctly and the buyer can use it to craft.
- **Survey data aggregation.** AI scout collects survey data from 10 claimed systems. Packages a regional analysis. Sells to AI expansion planner. Verify data formats are consistent and verifiable.
- **Blueprint trading.** AI shipwright designs a hauler blueprint. Sells copies to multiple buyers. Verify each buyer can build from the blueprint (given materials).
- **Knowledge pricing.** Track recipe prices over time. As more AI agents discover the same recipe independently, verify the price drops. Early discoverers profit, late ones don't.
- **Faction knowledge pool.** AI faction members contribute recipes to the faction library. New member joins and gains access. Verify Grant-based access works.
- **Partial knowledge.** Sell an incomplete recipe (element pair, no exact ratio). Verify the buyer can use it to narrow their own research.

## Interaction With Other Systems

**Transformation physics.** Recipes ARE the output of the transformation discovery process. The knowledge economy gives transformation physics its economic engine — without trade in recipes, discovery is a solitary activity. With trade, it's a competitive research industry.

**Reputation.** Knowledge sources are evaluated by reputation. A recipe from a domain with 200 verified trades is trusted. A recipe from an unknown domain might be a waste of credits. Reputation makes the knowledge market function.

**Contracts.** Research partnerships (see [CONTRACTS.md](CONTRACTS.md)) formalize knowledge creation. The contract defines who funds research, who runs experiments, how discoveries are shared. Knowledge objects are the output.

**Factions.** Faction knowledge pools are a primary membership benefit. The faction's collective R&D effort, encoded as a shared library of knowledge objects, is a competitive advantage that scales with membership.

**Combat.** Doctrine scripts are knowledge objects with direct combat impact. The knowledge economy arms races — factions invest in better doctrine, sell obsolete versions, steal current ones through espionage (observation of combat logs → reverse engineering of opponent doctrine).

## What This Spec Doesn't Cover

**Intellectual property enforcement.** There is none. Knowledge is bilaterally traded. Once sold, the buyer can do whatever they want. Resale, reverse engineering, publication. The seller's protection is economic (sell early, profit before commoditization) and social (reputation for fair dealing). No DRM, no licensing enforcement, no patent system. If factions develop IP norms, that's social convention, not protocol.

**Encryption.** Can a knowledge object be encrypted so only the holder can read it? Yes — standard cryptography, nothing Apeiron-specific. A recipe encrypted with the buyer's public key is unreadable by anyone else. But the buyer can decrypt and re-encrypt for resale. Encryption prevents casual observation, not determined piracy.

**Knowledge destruction.** Can knowledge be "un-known"? No. Once an entity has observed a recipe, they know it. Revoking the knowledge object's Grant doesn't erase the information from the entity's memory. This is realistic and intentional. The value is in the object (verifiable, proof-chain-backed) not in the information alone (which can't be revoked).
