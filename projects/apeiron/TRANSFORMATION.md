# Transformation Physics
<!-- id: apeiron.transformation --> <!-- status: proposed --> <!-- summary: Rules governing what crafting processes can produce from given inputs -->

The [constraint physics](PHYSICS.md) define what can exist. This spec defines what can be *created* — the rules governing how inputs become outputs during crafting and research. The constraint laws are the judge of finished objects. Transformation physics is the judge of the process that made them.

Transformation operates at two levels:

1. **Material synthesis** — elements combine into materials with emergent properties
2. **System design** — materials and components compose into functional systems with performance characteristics

Both follow the same principle: simple elemental rules, emergent behavior, discovery through experimentation. No recipe books, no tech trees. The search space is too large to brute-force and the interesting behavior comes from interaction effects nobody prescribed.

## The Problem

Without transformation rules, a crafting script can claim anything. "I put in iron and got out a miracle alloy with 100x structural efficiency." The finished object passes the constraint laws — it's internally consistent. But the process is nonsense. The transformation physics closes this gap: given these inputs combined this way, *these* are the bounds on what can come out.

Too loose and anyone declares wonder materials. Too tight and we've built a recipe book. The design target: rules tight enough to prevent fraud, loose enough that genuine discovery is the game.

## Elements

Fourteen elements — thirteen natural, one synthetic — six properties each. Named after real-world elements — not to simulate real chemistry, but because the names carry intuition. Iron is dense and hard. Copper conducts. When "iron + carbon" produces something steel-like, that's immersion. The interaction function is still seed-determined and opaque. The names are handles, not constraints.

| Property | What it governs |
|----------|----------------|
| **Density** | Mass per unit volume |
| **Hardness** | Resistance to deformation |
| **Conductivity** | Thermal and electrical transport |
| **Reactivity** | How readily it bonds with other elements |
| **Stability** | Resistance to decay and environmental stress |
| **Radiance** | Energy emission and absorption characteristics |

Six properties. That's the atomic fingerprint. Everything else — structural efficiency, shielding effectiveness, energy density — derives from these when elements combine into materials.

The founding cluster publishes the element table: names, property vectors, abundances, and seven starter recipes. It's part of the standard physics script. Content-addressed, deterministic, same for everyone. See [ELEMENTS.md](ELEMENTS.md) for the full table, abundance distribution, starter recipes, and design rationale.

### Why Real Names, Not Real Chemistry

Real chemistry is beautiful but computationally unbounded. Protein folding, quantum orbital interactions, reaction kinetics — simulating real chemistry in a Raido script is impossible and unnecessary. What we need is the *character* of chemistry: simple atomic rules producing complex emergent materials. The element system captures that without pretending to be physics.

The real names help with the on-ramp. Players guess that iron + carbon might make something strong (it does — the founding cluster's "structural steel" recipe). They guess hydrogen might be good for fuel (it is — highest radiance among common elements). These intuitions are roughly correct for simple, low-energy combinations. They're completely wrong for exotic multi-element, high-energy phases where Apeiron's seed-determined physics diverges from reality. The familiar entry makes the alien depth more rewarding to discover.

## Grounding: Everything Is Objects

The only things that are real — externally verifiable, auditable, unfakeable — are **Allgard objects** and the **transforms that consume and produce them**. Every input to a transformation must be a real object with a proof chain. No free parameters. No declarations without material backing.

A transformation has four categories of input, all Allgard objects:

**Element inputs** — the materials being combined. Consumed in the transform. Verified via proof chain back to seed-verified extraction.

**Fuel** — the energy source. Energy is NOT a free parameter. It comes from fuel objects consumed in the transform. The energy available is `fuel.energy_density × fuel.mass_consumed`. Fuel quality (energy density) is itself a material property, derived from element composition via the interaction function. You can't claim high energy without burning fuel that contains it. Better fuel requires better material science — the spiral.

**Catalysts** — present but not consumed (or minimally consumed — a small fraction lost per use, tunable constant). Their continued existence is verifiable.

**The facility** — the physical infrastructure that performs the transform. A facility is an Allgard object with a component tree, evaluated by the constraint laws like any other object. Its capabilities derive from its composition:

- Its **reactor components** determine maximum energy throughput — how much fuel energy per transform it can channel. A facility with a crude reactor can burn fuel, but can't efficiently deliver high energy density to the reaction. The reactor's power output limits the energy_per_mass achievable in a single transform, regardless of fuel quality.
- Its **containment components** determine what processes are safe — high-energy or high-reactivity transforms require containment rated for those conditions. The containment's stress tolerance (Law 4) and shielding (Law 5) set the bounds.
- Its **precision instruments** determine ratio control — how finely the facility can target specific element fractions. The physics script adds deterministic noise to the target ratios, scaled by the facility's precision rating (derived from its instrument components). Crude equipment: large scatter. Precision equipment: tight scatter. The noise is seeded by (facility_id, transform_index) — deterministic, re-computable by anyone verifying the proof.

```
actual_ratio[i] = target_ratio[i] + noise(facility.precision, facility_id, transform_index, i)
```

The interaction function evaluates at `actual_ratio`, not `target_ratio`. This means a crude workshop can't reliably target narrow phase regions — the scatter pushes experiments to random nearby points. A precision lab lands where you aim. Both are verifiable.

The crafting proof includes the facility's object ID. Trading partners can verify:
1. The facility exists (proof chain — it was built from real materials)
2. Its component tree produces the claimed capabilities (physics script evaluates it)
3. The transform is consistent with the facility's capabilities (energy within reactor limits, containment sufficient for the process, etc.)

A domain operator who wants to skip building the facility needs to fabricate its entire supply chain — extraction proofs, material transforms, assembly proofs. All auditable. All traceable to the seed.

### Why Facilities Prevent the Singularity

Without facilities, everything collapses to a point. A domain runs scripts, consumes objects, produces objects. No spatial extent. No world.

With facilities, the five constraint laws create physical extent:

- Facilities have **volume** (reaction chambers, containment vessels, reactor mass). Law 2 — structural scaling means bigger facilities cost superlinearly more structure.
- Facilities have **coupling costs** (Law 5 — the reactor radiates heat into the instruments, requiring shielding mass between them). More capable facilities have more internal coupling to manage.
- Facilities have **energy budgets** (Law 3 — the reactor powering the facility is part of the facility, with its own mass and volume).
- Facilities can **fail under stress** (Law 4 — running high-energy transforms risks containment failure).
- Facilities have **mass** (Law 1 — everything is made of something).

A domain that wants to do advanced material research, system design, manufacturing, AND ship maintenance needs facilities for each. Those facilities occupy space, consume energy, require structure, and couple with each other. The five laws prevent cramming infinite capability into a point — same way they prevent the 10-million-km ship.

This is why worlds have spatial structure. A planetary foundry is next to the mines (short material transport). The research lab is isolated from the foundry (vibration and thermal coupling). The reactor is shielded from both. The shipyard is in orbit (different structural scaling in zero-g). The layout isn't decorative. It's physics.

## Combination Physics

When elements combine, the output material's properties are determined by three things: the input ratios, the energy invested (from consumed fuel), and the **interaction function** — a deterministic but computationally opaque mapping from inputs to property modifications.

### Base Properties: The Weighted Average

The simplest combination — melt two elements together with no special process — gives you the weighted average:

```
base[p] = sum(element[i].fraction * element[i].property[p])
```

This is boring and expected. An alloy of 70% A and 30% B has properties somewhere between A and B. No surprises, no discoveries. This is what you get for free.

### The Interaction Function: Computational Opacity

The interaction function takes the full input state — element identities, mass fractions, energy (derived from consumed fuel) — the galaxy seed, and a **beacon value** from the [Allgard beacon](../allgard/BEACON.md):

```
energy = fuel.energy_density * fuel.mass_consumed
modification = interact(element_ids, fractions, energy / output_mass, galaxy_seed, beacon_value)
material[p] = base[p] + modification[p]
```

The critical design choice: **the domain commits to transform parameters BEFORE the beacon tick.** The beacon value enters the function but doesn't exist at commit time. This means the domain can't pre-compute which parameters produce the best output — the optimum shifts with every beacon tick. Each evaluation requires committing real resources (locked inputs, published commitment) before learning the result.

The galaxy seed parameterizes the general landscape. The beacon value perturbs it: peak positions shift by up to ±100% of their width, peak heights modulate 30-170%, peak widths vary 60-140%, and energy windows shift ±10%. The general structure is stable (which element pairs work, which energy ranges, which peak shapes) but the exact optimum moves every tick. General knowledge commoditizes. Exact coordinates don't transfer.

**Verification:** The proof includes the commitment (timestamped before the beacon tick), the beacon value (from the public beacon log), and the output. Any verifier re-executes: check commitment timing, check beacon value against the log, re-evaluate the function, confirm the output matches. Cheap. Local. No federation interaction.

### Stoichiometric Peaks

Pure pseudorandomness would make experimentation a lottery. Real chemistry has structure — small changes in composition usually produce small changes in properties. The interaction function preserves this through **stoichiometric peaks**: each element pair generates a small set of peaks at seed-determined ratios. These are the stable compounds — specific compositions where the elements combine productively.

Each peak has seed-determined characteristics:
- **Height** per property — a peak that boosts hardness might do nothing for conductivity
- **Width** — how forgiving the stoichiometry is (broad peaks tolerate sloppy ratios, narrow peaks require precision)
- **Shape** — some peaks are smooth bells (forgiving), others are flat-topped plateaus that cliff-edge off (stable but brittle), others are needles (demanding exact ratios), others are ridges with a sharp central bonus atop a broad base (rewarding to find, more rewarding to optimize)
- **Energy window** — the activation energy range where the peak is active
- **Interference sign** — whether the peak reinforces or cancels when overlapping with other peaks

**Within a peak's influence**, the function is smooth. A researcher near a stoichiometric ratio can hill-climb toward the peak — adjust ratios slightly, observe improvement, converge on the optimum. The peak's shape determines how rewarding this is.

**Between peaks**, the function produces low-amplitude deterministic noise — rough terrain with occasional micro-spikes. Not zero, but not systematically useful. A researcher exploring desert territory might stumble on a minor anomaly — enough to notice, not enough to be strategic.

**Where peaks overlap**, they can reinforce (both contribute positive modifications) or interfere destructively. This creates the "adding a trace of element C to an A-B alloy completely destroys the hardness" phenomenon — peak C interferes with peak A-B. The interference sign is seed-determined and unpredictable without evaluation.

This creates the right research dynamics:
- **Hill-climbing works locally.** Near a stoichiometric peak, systematic optimization converges on the optimum. "More element B improves hardness" — true near this peak.
- **Boundaries are where peak dominance shifts.** Moving away from one peak's influence toward another's can produce dramatic changes. The researcher doesn't know where the next productive peak is.
- **Breakthroughs come from finding new peaks** — especially ones that other researchers haven't discovered. The best materials are at peaks nobody has mapped, not at the optimum of known ones.
- **Interference is unpredictable.** A multi-element combination that should work (good pair affinities) might fail because of destructive interference between overlapping peaks. Or a combination that shouldn't work might succeed because interference is constructive.
- **You can't extrapolate across peaks.** Knowledge of one peak tells you nothing about others. Each new peak is a fresh discovery.

### Energy as a Dimension

Energy isn't just a scaling factor — it's a full dimension of the input space. Each stoichiometric peak has an **energy window** — a range `[e_lo, e_hi]` where the peak is active. Outside the window, the peak contributes nothing. This means increasing energy doesn't just "turn up" existing effects. Different energy levels activate different sets of peaks — the landscape changes character entirely.

Low energy → one set of active peaks (conventional chemistry).
Medium energy → different peaks activate (advanced metallurgy).
High energy → yet another set of peaks (exotic physics).

**The best peaks tend to have high energy thresholds.** This isn't a rule — it emerges from the peak generation. But the effect is critical: the most valuable materials are energy-gated. Low-tech factions with crude fuel can only access mediocre peaks. High-tech factions with advanced fuel access the best peaks. This creates the natural progression arc without prescribing tiers.

This is how latent physics emerges from the same mechanism. No special case needed — the peaks that produce non-zero values for exotic properties simply have high energy thresholds.

### Ratio Sensitivity

Near a stoichiometric peak, the interaction function is smooth but not necessarily gentle. Different peaks have different shapes — a needle peak has steep gradients where a 1% ratio change produces a 20% property change. A plateau peak is flat across a wide range, then cliff-edges off. The shape is seed-determined per peak.

A researcher near a needle peak needs precise ratio control — small changes matter. A researcher near a plateau can be sloppy. The physics doesn't prescribe which peaks are steep — the seed determines it, through the peak shape selection. This is why facility precision matters for **manufacturing**: once you've found a needle peak, reproducing the exact composition consistently requires high-precision instruments. A crude workshop scatters across the peak, producing inconsistent results. A precision facility hits the stoichiometric ratio reliably.

## Multi-Element Combinations

More elements means more element pairs, and more pairs means more stoichiometric peaks. The peak count grows as n×(n-1)/2 pairs, each with 3-5 peaks. With two elements: 1 pair, ~4 peaks. With three: 3 pairs, ~12 peaks. With five: 10 pairs, ~40 peaks. And the peaks can interfere where they overlap — with more peaks, more overlaps, more interference.

The input space dimensionality also grows. With two elements, the input is 2D (ratio + energy). With three, it's 3D. With five, it's 5D. Higher-dimensional spaces are exponentially harder to search — the same number of experiments covers an exponentially smaller fraction of the space.

This is why multi-element research is harder and more rewarding — more peaks to find, more interference effects to discover, and exponentially more space to search. A binary search (2 elements) might find good peaks in dozens of experiments. A ternary search (3 elements) might take hundreds. A quinary search (5 elements) could take thousands.

The founding cluster tunes the peak density (how many peaks per pair, how wide they are) to control discovery pace. Dense peaks = lots of interference = lots of surprises but hard to optimize. Sparse peaks = large deserts between productive compositions = harder to find anything but clearer to optimize once found.

## Mass Budget

Conservation during transformation:

```
output_mass <= input_mass * (1 - loss_fraction)
```

Every transformation loses material. The loss depends on the **process**, not the **result** — what you put in and how hard you push, not what comes out.

```
loss_fraction = base_loss + energy_loss(energy_per_mass) + complexity_loss(num_elements)
```

**Base loss** — the floor. No process is 100% efficient. Even simple mixing loses material to slag, spillage, incomplete reactions. Tunable constant, probably 3-8%.

**Energy loss** — scales with energy invested per unit mass. Higher energy processes are more violent. More material vaporized, more waste heat carrying away particles, more byproducts. A low-energy alloy might add 2% loss. A plasma-sintered exotic might add 25%.

```
energy_loss = energy_coefficient * (energy_per_mass / reference_energy)
```

**Complexity loss** — scales with the number of distinct input elements. More elements means more reaction pathways, more off-spec byproducts, harder to control. Binary combination: small. Quinary: significant.

```
complexity_loss = complexity_coefficient * (num_elements - 1)
```

The output properties don't affect loss at all. If you find a phase region that produces amazing material at low energy with two common elements, your production cost is low. That's not a bug — it's the reward for discovery. The cost was the search: all the experiments that consumed materials while exploring dead ends. Once you've found an efficient process, you benefit from it.

The lost mass is gone. Destroyed. This feeds into Allgard's Conservation Law 3 — crafting loss is a designed entropy sink.

## Theoretical Limits

No material property can exceed a theoretical maximum defined in the standard physics script. The interaction effects asymptotically approach but never reach these limits:

```
effective_property = theoretical_max * (1 - exp(-raw_value / theoretical_max))
```

As raw computed values get large, the effective property saturates. Diminishing returns are baked into the math. You can always make materials incrementally better, but each increment costs more energy and rarer inputs for less improvement.

The theoretical maximums are constants — part of the standard physics script, set by the founding cluster. They define the ultimate ceiling for each property in the galaxy. A civilization that maxes out structural efficiency has hit the physics wall. No amount of clever crafting gets past it. The founding cluster tunes these ceilings to create the progression arc described in PHYSICS.md — each technology phase corresponds to reaching a certain fraction of theoretical limits.

## Catalysts

Some elements, when present in small quantities during a transformation, modify the interaction function's behavior without being consumed. These are catalysts.

Mechanically: a catalyst element is an input to the interaction function but is excluded from the mass budget. It appears in the inputs and the outputs (not consumed). Its presence **lowers the activation energy threshold** of specific stoichiometric peaks — making them accessible at lower energy levels than they would otherwise require.

```
effective_threshold = peak.energy_threshold * (1 - catalyst_reduction)
```

A catalyst doesn't change what's thermodynamically possible — the same peaks exist with or without it. It changes what's **kinetically accessible** at a given energy level. A peak that normally requires energy level 0.5 might only need 0.2 with the right catalyst. The peak value is the same. The access is different.

Each peak responds to a small number of specific catalyst elements (1-3, seed-determined). Most element-as-catalyst combinations have no effect on a given peak. A few produce large threshold reductions that make otherwise energy-gated peaks accessible. The mapping from catalyst elements to peak effects is seed-determined and computationally opaque — you discover which catalysts work for which reactions through experimentation, not analysis.

Since the best peaks tend to have high energy thresholds, catalysts are disproportionately valuable for accessing the best materials. A faction with a rare catalyst element doesn't get marginally better results — they access peaks that are completely inaccessible to factions without it at the same energy level. That's worth fighting over.

This is why geographic scarcity matters for research. A rare catalyst element doesn't just make existing processes cheaper — it makes high-energy-gated peaks accessible at low energy. A faction controlling rare catalyst deposits can reach materials that other factions would need vastly more advanced fuel to access. Not from a multiplier. From activation energy — the catalyst lowers the barrier.

## Process Parameters

A crafting script specifies:

1. **Input elements** — what goes in, and how much of each
2. **Energy** — how much energy the process invests
3. **Catalysts** — what elements are present but not consumed
4. **Process type** — a discrete parameter selecting the combination mode

Process types represent fundamentally different physical processes:

| Process | Character |
|---------|-----------|
| **Alloying** | Bulk mixing. All interaction effects available. Standard material creation. |
| **Refinement** | Single-element input. Removes impurities, improves base properties toward theoretical element purity. Energy-intensive. |
| **Decomposition** | Breaks a material back into constituent elements. Lossy. Recovers some inputs from existing materials. |

The process type selects which terms of the combination formula apply and with what efficiency. Alloying uses the full interaction model. Refinement operates within a single element's property bounds. Decomposition reverses a previous combination (partially — information and mass are lost).

These aren't arbitrary categories. They correspond to physically distinct operations. The founding cluster can add new process types in future standard physics script updates (sintering, vapor deposition, nuclear transmutation) as the game evolves.

## The Discovery Landscape

The phase model creates specific research dynamics:

**Easy discoveries — hill-climbing in gentle regions.** Two common elements, low energy. The founding cluster publishes starter recipes that land in wide, smooth phase regions. Researchers hill-climb from there — adjust ratios, observe incremental improvements, converge on local optima. This is day-one accessible research. Reliable, low-risk, modest rewards.

**Medium discoveries — finding better regions.** The starter recipes aren't in the best phase regions — they're in the ones the founding cluster chose to publish. Adjacent regions might be significantly better. Finding them means pushing ratios or energy past a phase boundary. The researcher doesn't know where the boundary is. They push incrementally, observing smooth improvement, until suddenly the output jumps — they've crossed a boundary. Now they're in new territory. Maybe better, maybe worse. If better, they hill-climb in the new region.

**Hard discoveries — high-dimensional exploration.** Three or more elements, rare inputs, high energy. The input space is 4D+. Phase regions are plentiful but each experiment costs more (rare materials consumed). A researcher might spend dozens of experiments mapping a single region, only to find its optimum is mediocre. Then stumble across a boundary into a region where hardness values exceed anything previously known. This is where breakthroughs happen — and they can't be predicted or accelerated by reading the source code.

**The search game:** A researcher has a goal — "I want a material with higher structural efficiency than anything known." The theoretical maximum says such materials exist. But in which phase region? With which elements? At what energy? The researcher designs experiments, consumes materials, maps local gradients, crosses boundaries, evaluates new regions. Each experiment is informative locally but says nothing about unexplored regions. This is genuine research gameplay — not "click research button, wait timer."

**Information asymmetry:** A faction that discovers a breakthrough knows the recipe (crafting script — specific inputs, ratios, energy). They can produce the material. They can sell finished goods (trading partners see the output but not the input coordinates). They can sell the recipe (valuable but creates competitors). Reverse-engineering from the component tree narrows the element space but doesn't reveal the ratios or energy — those must be rediscovered experimentally. With the phase model, even knowing the exact composition of the output material doesn't tell you which phase region produced it or how to get there from different starting materials.

## Latent Physics

The element property vector has more dimensions than day-one chemistry can reach. Properties like spatial distortion, field coherence, phase stability exist in the physics script from launch. The performance functions reference them. But at low energy with common elements, the interaction function maps to phase regions where these properties are zero.

This falls out naturally from the phase model — no special mechanism needed. The energy dimension of the input space has its own phase boundaries. Low-energy regions produce conventional properties. Cross an energy boundary and you enter regions where the function produces non-zero values for properties that were zero in every low-energy region.

| Property | Status at launch | What it enables when non-zero |
|----------|-----------------|-------------------------------|
| Hardness | Active — accessible in low-energy phase regions | Structural components, armor, tools |
| Conductivity | Active — accessible in low-energy phase regions | Power routing, sensors, communications |
| Spatial distortion | Latent — only non-zero in high-energy phase regions | Jump drives, gravity manipulation |
| Field coherence | Latent — only non-zero in extreme-energy regions with rare elements | Force fields, containment, directed energy |
| Phase stability | Latent — only non-zero in high-dimensional input spaces (3+ elements including exotics) | Metamaterials, cloaking, sensor dampening |

### Threshold Cascades

The energy boundaries that gate latent properties aren't reachable with day-one fuel. Reaching them requires higher energy density fuel — which requires discovering better materials — which requires reaching intermediate energy boundaries first. Each tier of fuel quality unlocks phase regions that contain materials for the next tier of fuel.

This cascade isn't prescribed. It emerges from the phase structure of the interaction function. The founding cluster designs the seed and the interaction algorithm such that the energy boundaries fall at levels that create a natural progression. But nobody can predict exactly which path through the cascade is fastest — that depends on which phase regions happen to contain the best intermediate materials, which depends on the seed.

A faction running high-energy experiments with rare elements might notice a tiny non-zero value for "spatial distortion" in their output. What is that? The physics script has performance functions for it. But nobody has ever produced enough to matter. The faction pushes deeper — spends more material, maps the landscape around that anomalous result, finds the local gradient, climbs it. Eventually they produce a material with significant spatial distortion. Plug it into a system. The performance function evaluates — and the system does something nobody has seen before.

That's discovery. Not a tech tree unlock. Not a recipe. The universe had more to offer than anyone knew.

### Computational Opacity Is the Protection

The physics script is public. Anyone can read the algorithm. But the interaction function is seed-parameterized and computationally opaque — knowing the algorithm tells you the *structure* (phases, boundaries, smoothness) but not the *content* (where the boundaries fall, what each region produces). That's determined by the seed, and the only way to learn it is to evaluate the function at specific points. Each evaluation is an experiment that costs materials.

You can read the source code and see that spatial distortion is a property dimension. You can see that the interaction function CAN produce non-zero values for it. But you can't compute WHICH inputs produce it without running the experiments. The function is a black box in practice even though it's transparent in principle. Like knowing that SHA-256 has preimages without being able to find them.

### Progression Tiers

Not prescribed — emergent from the phase landscape. But the founding cluster designs the seed to create natural tiers:

**Tier 0 — Common chemistry.** Low-energy phase regions with common elements. Basic alloys, structural materials, simple conductors. Founding cluster publishes starter recipes that exploit a few known regions. Wide, gentle regions — easy to explore, small improvements everywhere.

**Tier 1 — Advanced materials.** Medium-energy regions. Better property values, some steep gradients rewarding precision. Requires better fuel (crafted from tier 0 discoveries). The "orbital" and "interstellar" phase from PHYSICS.md.

**Tier 2 — Exotic materials.** High-energy regions, often requiring 3+ elements including rare ones. Dense phase tessellation — many boundaries, frequent surprises. First non-zero values in latent property dimensions. The "industrial space" phase.

**Tier 3 — New physics.** Extreme-energy regions in high-dimensional input spaces (4-5 elements with exotics). Significant latent property values. New system types become viable. Cascading prerequisites — tier 2 fuel to reach tier 3 energy levels. The "stellar" phase becomes theoretically accessible.

A faction might reach tier 3 in one property dimension while stuck at tier 1 in others. Progress is multidimensional.

### Founding Cluster Design Responsibility

The interaction function algorithm and its relationship to the galaxy seed is the founding cluster's most important creative act. They're designing the universe's chemistry — not as lore, but as math. The algorithm determines:

- Peak density per element pair (controls discovery pace)
- Peak shape distribution — how many needles vs. plateaus vs. ridges (controls how much precision matters)
- Which property dimensions are latent vs. active (controls capability progression)
- How seed variation affects the landscape (controls inter-galaxy uniqueness)
- Interference patterns — reinforcement vs. cancellation frequency (controls how predictable multi-element combinations are)
- Energy window distribution — where activation thresholds fall (controls tier progression)
- Catalyst sensitivity — how many peaks respond to each catalyst element (controls geographic scarcity value)

This is universe design. The founding cluster tunes it through playtesting. The algorithm can evolve (new script version, voluntary adoption) but the seed doesn't change. Factions explore a continent that already exists — the interaction function drew the map but nobody has it.

## Verification and Proof Chains

### The Problem: Verification vs. Secrecy

Transforms must be verifiable — a domain can't just claim "I put in iron and got a miracle alloy." But full transparency leaks the recipe. If every proof chain reveals exact ratios, energy level, and catalyst, buying one sample gives you the complete recipe for free.

The solution: **layered proof chains.** Public verification establishes trust. Private details protect trade secrets.

### Public Proof (visible to anyone inspecting the material)

1. **Output properties** — the material's property vector (the 6 values). This IS the material's identity.
2. **Mass** — total output mass.
3. **Domain attestation** — the producing domain's signed statement: "I verified this transform against standard physics script v{hash}."
4. **Physics script hash** — which version of the standard physics script was used.
5. **Input consumption hashes** — cryptographic hashes proving real objects were consumed. Verifies mass conservation without revealing what those objects were.
6. **Transform index** — monotonic counter preventing replay.

### Private Proof (held by the producing domain, never shared in trade)

1. **Element identities and exact ratios** — what went in and how much of each.
2. **Energy level** — the exact energy per output mass.
3. **Catalyst identity** — which catalyst element was present (if any).
4. **Facility details** — reactor, containment, precision specifics.
5. **Full input object references** — the complete proof chain of every consumed input.

### How Verification Works

**Within the producing domain:** Full verification. The domain re-runs the physics script with all private inputs, confirms the output matches. This is the deterministic re-execution that TRANSFORMATION.md has always described. The domain stakes its reputation on this attestation.

**Between domains (trade):** The buyer sees the public proof. They verify:
- The producing domain is running the standard physics script (hash check).
- Real objects were consumed (input hashes link to valid object IDs in the producing domain's published consumption log).
- The attestation is signed by the producing domain.
- Mass conservation holds (output mass ≤ declared input mass).

They do NOT re-run the physics script themselves — they don't have the private inputs. They trust the producing domain's attestation. This is the same bilateral trust model Allgard already uses for everything else.

**If you don't trust the producing domain:** Don't buy their materials. Or demand full disclosure as a condition of trade (some sellers will accept this for commodity materials). Trust is bilateral, not universal.

### Reverse Engineering

Buying a material tells you the output properties. Not the inputs. Reverse engineering requires work:

**Step 1 — Decomposition.** Break the material back into constituent elements. Now you know WHAT's in it. But: you destroyed the material, decomposition is lossy (30-50% mass loss), and you still don't know the ratios, energy level, or catalyst. Cost: one sample destroyed.

**Step 2 — Experimental sweep.** With known elements, sweep ratios and energy to find the stoichiometric peak. For a 2-element alloy: ~200-500 experiments (ratio × energy grid). For 3-element: thousands. Each experiment consumes real materials.

**Step 3 — Catalyst guessing.** Decomposition doesn't reveal the catalyst — it wasn't consumed and doesn't appear in the output. If the material was catalyst-assisted, you also need to sweep catalyst candidates. You don't even know IF a catalyst was involved.

| Material complexity | Reverse engineering cost |
|--------------------|-----------------------|
| 2-element, no catalyst | ~500 experiments |
| 2-element, with catalyst | ~1,500 experiments (3 catalyst candidates) |
| 3-element, no catalyst | ~5,000+ experiments |
| 3-element, with catalyst | ~15,000+ experiments |
| 4-element | Effectively prohibitive without clues |

This creates a natural economy: **simple recipes commoditize fast, complex recipes stay proprietary.** Binary alloys are reverse-engineered in days. Ternary alloys take weeks of sustained investment. Quaternary alloys are durable trade secrets.

### Recipe Trading

Because reverse engineering is expensive, recipes have independent trade value. A crafter can sell:
- **Materials** — the output. Buyer gets the material, not the recipe. Safe.
- **Recipes** — the full private proof (or a crafting script that encodes it). Buyer pays a premium but skips reverse engineering entirely.
- **Hints** — partial information. "It's iron-based, needs medium energy." Worth something, not everything.

This creates a knowledge economy layered on top of the material economy. A researcher who discovers a good ternary peak can profit from it three ways: produce and sell materials, license the recipe to other producers, or sell hints to competing researchers.

## Material Naming

### Social Names Are Primary

Nobody reads `Fe82-Cr18 @E45` in their inventory. Materials have names. The name is the primary identifier everywhere — inventory, trade, conversation. Composition is metadata you inspect when you need it.

**How naming works:**

- The producing crafter names their material when first registering it.
- Founding cluster pre-registers starter recipe names: "Steel", "Hull Plate", "Hydrocarbon Fuel."
- Players register discoveries: "Kovac's Alloy", "Void Glass", "Sunfire" — whatever they choose.
- Names are tied to a **property range**, not exact composition. Steel is steel whether it's Fe97-C3 or Fe96.5-C3.5. Small ratio variations that land on the same stoichiometric peak produce the same named material.
- Composition notation (`Fe82-Cr18`) exists as a detail view for research and precision work. Not the label.

### Naming Governance

**Founding cluster** maintains the standard registry. Starter recipe names are reserved. Basic profanity filter on new registrations. Light touch — reject the obvious, let everything else through.

**Domains are sovereign.** A domain can display any names they want locally. If the founding cluster rejects a name, the crafter's home domain can still use it internally.

**Standard names are opt-in but sticky.** When a material gets widely adopted, its name becomes a de facto standard. The founding cluster can promote community names to "standard" status — recognition of what players already call it, not a vote.

**No voting system.** Voting creates politics around naming instead of around the game. First-to-register at the founding cluster, domain sovereignty everywhere else.

## Level 2: System Design

Material synthesis produces materials with properties. System design produces *functional systems* — engines, reactors, shields, weapons, sensors — with performance characteristics.

Without design rules, a crafting script can claim "I built a 1kg engine with infinite thrust." The constraint laws check mass and energy budget but don't derive performance from composition. System design physics fills that gap.

System design is deliberately **more predictable** than material synthesis. Material science is exploration of an opaque landscape. System design is optimization under known physics. The difficulty comes from multi-objective tradeoffs and the five constraint laws interacting, not from hidden landscapes. You can mostly *compute* how an engine should perform from its material properties. The surprises are at the edges.

### Base Performance: Analytical and Transparent

Each system type has a **performance function** — an analytical formula that takes component material properties and design parameters, and derives performance:

```
base_thrust = combustion_efficiency(chamber.stability, chamber.radiance)
            * nozzle_expansion(nozzle.hardness, nozzle_area)
            * fuel_energy_density
            * (1 - thermal_loss(chamber.conductivity))
```

These are published in the standard physics script. Analytical. Readable. Anyone can plug in their material properties and compute expected performance. No opacity. A competent engineer can optimize the design parameters for their available materials by studying the formulas.

This is intentional. System design rewards understanding, not blind search. A faction that studies the performance functions deeply and optimizes carefully builds better systems than one that experiments randomly. Knowledge of physics matters.

### Why Analytical Isn't Easy

The performance functions are transparent, but optimizing them is hard because the five constraint laws create **coupled feedback loops**:

- More thrust requires a bigger combustion chamber (more mass, Law 1)
- Bigger chamber requires more structural support (superlinear cost, Law 2)
- More thrust needs more fuel flow needs more power (energy budget, Law 3)
- Higher operating temperature means faster component degradation (stress, Law 4)
- The reactor powering the engine couples thermally with the chamber (shielding mass, Law 5)

Optimizing thrust means solving a system of coupled equations where improving one variable worsens others. The Pareto frontier is high-dimensional and non-convex. There's no single "best engine" — there are tradeoff surfaces. A light fast engine. A heavy efficient engine. A reliable slow engine. Each point on the surface is a different design.

This is genuinely hard optimization even with full transparency. Two engineers with the same materials and the same physics knowledge will produce different designs because they weight the tradeoffs differently.

### Resonance Effects: Where Surprises Live

The base performance is analytical. But specific combinations of materials in specific component roles produce non-linear bonuses that the base formula doesn't capture. These are **resonance effects** — emergent performance gains from material interactions within a system.

```
actual_thrust = base_thrust * (1 + resonance(chamber.properties, nozzle.properties, injector.properties))
```

The resonance function is smoother and sparser than the material interaction function. No phase boundaries. No chaotic discontinuities. Instead: **peaks** at specific material-property combinations, with gradual falloff away from the peak.

```
resonance = sum(
    amplitude[k] * exp(-distance(component_properties, peak_center[k])^2 / width[k])
)
```

Each resonance peak has a center (a specific combination of material properties across components), an amplitude (how much bonus it gives), and a width (how precise the combination needs to be).

The peaks are **not seed-dependent**. They're determined by the resonance function in the standard physics script — fixed for all galaxies. But their locations in material-property space mean they're effectively hidden until someone has the right materials AND tests the right combination. A resonance peak centered at (chamber.stability=0.8, nozzle.conductivity=0.6, injector.hardness=0.9) is invisible to anyone without materials near those property values.

**Why not seed-dependent?** Material synthesis is seed-dependent because each galaxy should have unique chemistry — that's the exploration game. System design is NOT seed-dependent because engineering knowledge should be transferable. An engine design that exploits a resonance should work in any galaxy, given materials with the right properties. Physics is universal. Chemistry (in this model) is local.

### New Materials Shift the Landscape

Here's where the two layers compose. When material synthesis produces a new material with novel property values, it potentially lands near resonance peaks that no previous material could access. The system designer plugs the new material into their performance calculations, and — surprise — the resonance function produces a bonus nobody expected.

This creates a cascade:
1. Material researcher discovers an alloy with unusual property profile
2. System designer tests it in various component roles
3. One combination hits near a resonance peak — performance jumps 15% beyond the analytical base
4. The designer optimizes around the peak — fine-tunes the design parameters for the new material
5. The optimized system enables a new class of ship that wasn't viable before

The material researcher doesn't know they enabled a system breakthrough. The system designer doesn't know what's in the material — they just know it works unusually well in this role. Knowledge flows across the two layers but doesn't automatically transfer. Collaboration between material scientists and system engineers is valuable.

### Design Parameters

System design has **design parameters** — engineering choices about how to arrange and configure components:

- **Geometry** — how components are arranged spatially (affects proximity coupling, Law 5)
- **Operating point** — where on the performance curve the system targets (affects stress, Law 4)
- **Tolerance margins** — how much safety margin the design includes (trading peak performance for reliability)
- **Routing** — how power, fuel, coolant, and data flow between components (affects mass and coupling)

Design parameters create tradeoffs within the physics. Two engines with identical materials can have different thrust if one uses a higher operating point (more thrust, faster degradation) or tighter geometry (lighter, more coupling interference). The physics computes the consequences. The designer chooses the tradeoffs.

Design parameters also affect resonance — the same materials at different operating points might be closer to or further from a resonance peak. Tuning the operating point IS part of finding the resonance.

### Process Types

| Process | Character |
|---------|-----------|
| **Assembly** | Combining components into a system. Standard construction. Performance determined by physics. |
| **Optimization** | Modifying design parameters without changing materials. Cheaper than rebuilding. Searching for resonances. |
| **Reverse engineering** | Studying a system's component tree to learn its materials and design parameters. You see the composition but not the resonance analysis — you know WHAT but not WHY it performs well. |
| **Scaling** | Building a larger or smaller version of a known design. Not free — structural scaling (Law 2) means resonances shift at different scales. Re-optimization required. |

### Research Categories

Research decomposes naturally along the two layers:

**Materials research** — exploring the phase landscape. Opaque. Requires element inputs, energy, catalysts.

**System research** — optimizing performance functions and finding resonances. Semi-transparent. Requires candidate materials, test facilities, and understanding of the physics.

System research further specializes by what you're building:

| Domain | Key material properties | Key constraint interactions |
|--------|------------------------|---------------------------|
| Propulsion | stability, radiance | L1 (fuel mass), L3 (power), L4 (thermal stress) |
| Reactors | radiance, stability, conductivity | L1 (reactor mass), L3 (output vs. draw), L5 (thermal coupling to everything) |
| Weapons | conductivity, radiance, hardness | L3 (power draw), L4 (stress from repeated firing), L5 (EM coupling) |
| Shielding | density, hardness, stability | L1 (shield mass), L2 (coverage area), L5 (absorption vs. re-radiation) |
| Sensors | conductivity, stability | L5 (isolation from noise sources), L3 (power for sensitivity) |

These categories aren't prescribed. They emerge because different performance functions depend on different material properties and different constraint interactions. A faction that wants better engines pursues thermal stability and radiance in their material research. A faction that wants better shields pursues density and hardness. The same interaction function serves both — but they're exploring different regions of it.

## Interaction With Existing Systems

**Constraint physics (PHYSICS.md):** Material synthesis produces materials. System design produces functional components. Constraint physics governs the finished objects built from them. A crafted alloy with excellent structural efficiency feeds into Law 2 — it lowers `k`, making bigger structures viable. A better engine design feeds into Law 3 — more thrust per watt, extending range at the same mass. Transformation physics creates the parts. Constraint physics judges the whole.

**Conservation laws (Allgard):** Mass lost during transformation satisfies Conservation Law 3 (exchange conservation — crafting loss is a declared sink). Inputs consumed are destroyed (minting/burning backed by Raido script). Outputs are new objects. The full chain is auditable.

**Geographic scarcity:** The seed distributes elements and their abundances. Common elements appear everywhere. Rare elements appear in specific systems. Catalyst elements may be extremely scarce. The combination physics makes rare elements valuable not by fiat but because they enable transformations that common elements can't.

**Fuel and facilities as energy gate:** The energy available for a transform comes from consumed fuel, channeled through a facility's reactor. Both are Allgard objects, both verifiable. Better fuel → more energy available. Better facility → more energy deliverable per transform. Both require better materials to build, which require research, which requires fuel and facilities. The spiral is intentional.

## Research Economics

### What's Actually Enforced

Domains are sovereign. Nobody polices how fast you run experiments. But every experiment requires real objects — and those objects are externally verifiable:

1. **Material consumption.** Each experiment consumes real element inputs and real fuel. Allgard objects destroyed via verifiable transforms. Proof chain auditable. Can't conjure materials.

2. **Facility capability.** Each experiment happens in a real facility. The facility's reactor limits energy throughput. Its containment limits process intensity. Its precision limits ratio control. The facility is an Allgard object — its capabilities derive from its component tree, verifiable by anyone.

3. **Output validity.** The claimed output must match the interaction function evaluation for those inputs, at the energy the facility can deliver, with the precision the facility achieves. The producing domain verifies this via full re-execution. Trading partners verify via domain attestation (see Verification and Proof Chains).

### What Constrains Research

Three things, all verifiable:

**Material supply** — every experiment burns inputs. Rich factions research faster. The binding constraint on research *volume*.

**Fuel quality** — energy comes from consumed fuel. Crude fuel means low energy, limited to low-energy phase regions. Advanced fuel (itself a product of material research) means access to high-energy regions where latent properties live. The binding constraint on research *depth*.

**Facility capability** — the facility's reactor determines maximum energy throughput per transform. Its containment determines what processes are safe. Its precision determines how accurately you can target specific ratios in the input space. A crude workshop with a small reactor can't channel the energy from advanced fuel even if you have it. A precision lab built from advanced materials can target narrow phase regions that a crude facility would scatter across. The binding constraint on research *capability*.

All three compose. A faction needs materials (to burn on experiments), fuel (to provide energy), and a facility (to channel that energy into the transform). Building a better facility requires better materials, which requires research, which requires a facility. The spiral is real and every step is verifiable.

### Batch Strategy Still Matters

Even without enforced throughput limits, batch economics is relevant because of **information flow**. A researcher running experiments sequentially can use each result to inform the next — hill-climbing. A researcher committing to many experiments at once (because they have materials to burn) must decide inputs before seeing results.

A wealthy faction dumping 1,000 experiments worth of materials into blind exploration maps the landscape fast but wastefully. A careful faction running 10 experiments, studying results, then running 10 more is slower but more efficient per unit of material. The optimal strategy depends on the ratio of material wealth to search space size.

**Early game (scarce materials, narrow search space):** Sequential hill-climbing dominates. Every experiment is precious. Observe, adjust, repeat.

**Late game (abundant materials, vast search space):** Parallel exploration dominates. The search space grows exponentially with element count and energy range. Sequential exploration of a 5-element landscape would take geological time. Burning thousands of experiments to map regions in bulk is the only viable approach.

This creates a natural economic role for **research services.** A faction rich in common materials but poor in rare elements partners with a faction rich in rare elements but poor in volume. One provides the exotic inputs. The other provides the bulk materials to burn through exploration. The crafting scripts they discover are the shared profit.

### Implications for the Simulation

### What the Simulation Validates

The playtest simulation (`sim/transform_sim.py`) implements the stoichiometric peak model and validates these claims:

**Validated:**
- Interaction function structure: stoichiometric peaks with energy windows and interference produce the right discovery dynamics
- Catalyst mechanism: lowering activation energy thresholds makes specific catalysts dramatically valuable (+100% at low energy for the right catalyst, most catalysts do nothing)
- Energy gating: low-energy researchers are locked out of the best peaks; progression from energy 5→50 produces 5x improvement
- Precision matters for manufacturing: a known recipe at ±0.05 precision produces 78% yield at 90% quality; at ±0.2, only 25% yield
- Element count scales difficulty exponentially: 2 elements is easy, 5 elements is genuinely hard
- Seed variance: different galaxies produce meaningfully different landscapes (30% coefficient of variation across seeds)
- Reverse engineering is possible but expensive: matching a known output at tolerance 2.0 takes ~1000 experiments

**Not modeled (game mechanics, not physics):**
- Mass budget / loss fraction (research economics)
- Process types: alloying, refinement, decomposition
- Deterministic noise seeding for verification proofs
- Latent properties (spatial distortion, field coherence)
- System design layer (performance functions, resonance)

The simulation also informs tuning:
- Vary material budgets (scarce, moderate, abundant)
- Compare sequential vs. parallel exploration at each budget level
- Model energy-capacity constraints (low-energy-only vs. full-range access)
- Test research partnerships (shared inputs, shared discoveries)

## Constants and Tuning

| Constant | Layer | What it controls |
|----------|-------|-----------------|
| Element property vectors | Material | Base properties of each element (the weighted average inputs) |
| Interaction function algorithm | Material | The core mapping — how inputs combine to produce property modifications |
| Galaxy seed | Material | Parameterizes the interaction function — determines the specific phase landscape |
| Peaks per element pair | Material | How many stoichiometric peaks each pair generates (controls discovery density) |
| Peak shape distribution | Material | Relative frequency of gaussian, plateau, needle, ridge shapes |
| Catalyst threshold reduction | Material | How much each catalyst element lowers peak activation energy |
| Base loss fraction | Both | Minimum material loss during any transformation |
| Energy loss coefficient | Material | How fast loss scales with energy invested |
| Reference energy | Material | Energy level at which energy_loss equals the coefficient |
| Complexity loss coefficient | Material | Additional loss per input element beyond the first |
| Theoretical property maximums | Material | Hard ceiling for each material property (saturation curve) |
| Performance functions | System | How component properties derive system performance |
| System interaction coefficients | System | Non-linear bonuses from component combinations |
| Theoretical performance maximums | System | Hard ceiling for each performance characteristic |

All constants are part of the standard physics script. Content-addressed, published, verifiable. The founding cluster tunes through playtesting and publishes updates (new script hash, voluntary adoption).

The element count and interaction table size determine the game's discovery depth. More elements = larger search space = longer discovery timeline. The founding cluster starts with 14 elements (13 natural + 1 synthetic) and can expand the table in future updates — adding new elements that the seed already placed in the galaxy but that prior scripts didn't know how to evaluate. "The elements were always there. We built better instruments." See [ELEMENTS.md](ELEMENTS.md) for expansion candidates.

## What This Creates

| Mechanic | Source |
|----------|--------|
| Material progression | Element interactions unlock better material constants |
| System progression | Component interactions unlock better system performance |
| Research depth | Two independent but composing discovery landscapes |
| Geographic value | Rare elements as catalysts, rare materials for exotic systems |
| Trade value | Elements, materials, components, crafting scripts, finished systems |
| Specialization | Material scientists, system engineers, and builders are distinct roles |
| Knowledge economy | Crafting scripts are tradeable intellectual property at both layers |
| Verification | Every transformation is deterministically re-computable |
| Natural difficulty curve | Common alloys → exotic materials → basic systems → optimized systems → breakthrough designs |

No tech trees. No recipe books. No unlock gates. Elements interact to produce materials. Materials compose into systems. Systems assemble into objects. The constraint laws judge the result. Everything in between is discovery.

## Future Directions

**Biological science.** Genetics, breeding, ecosystem engineering, terraforming, alien life. The transformation model extends naturally — genetic building blocks combine via an interaction function, organisms have trait vectors, ecosystems are system-design problems. Not designed yet. The core material/system science needs to validate first. Nothing in this spec forecloses it.
