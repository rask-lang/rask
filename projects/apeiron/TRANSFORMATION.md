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

The galaxy has a fixed set of elements — the fundamental building blocks of all materials. Not 118 like the periodic table. More like 12-20, tuned through playtesting. Each element is defined by a small property vector:

| Property | What it governs |
|----------|----------------|
| **Density** | Mass per unit volume |
| **Hardness** | Resistance to deformation |
| **Conductivity** | Thermal and electrical transport |
| **Reactivity** | How readily it bonds with other elements |
| **Stability** | Resistance to decay and environmental stress |
| **Radiance** | Energy emission and absorption characteristics |

Six properties. That's the atomic fingerprint. Everything else — structural efficiency, shielding effectiveness, energy density — derives from these when elements combine into materials.

The founding cluster publishes the element table: names, property vectors, abundances. It's part of the standard physics script. Content-addressed, deterministic, same for everyone.

### Why Not Real Chemistry

Real chemistry is beautiful but computationally unbounded. Protein folding, quantum orbital interactions, reaction kinetics — simulating real chemistry in a Raido script is impossible and unnecessary. What we need is the *character* of chemistry: simple atomic rules producing complex emergent materials. The element system captures that without pretending to be physics.

## Combination Physics

When elements combine, the output material's properties are determined by three things: the input ratios, the energy invested, and the **interaction function** — a deterministic but computationally opaque mapping from inputs to property modifications.

### Base Properties: The Weighted Average

The simplest combination — melt two elements together with no special process — gives you the weighted average:

```
base[p] = sum(element[i].fraction * element[i].property[p])
```

This is boring and expected. An alloy of 70% A and 30% B has properties somewhere between A and B. No surprises, no discoveries. This is what you get for free.

### The Interaction Function: Computational Opacity

The interaction function takes the full input state — element identities, mass fractions, energy level — and the galaxy seed, and produces a property modification vector:

```
modification = interact(element_ids, fractions, energy, galaxy_seed)
material[p] = base[p] + modification[p]
```

The critical design choice: **this function is forward-cheap but backward-hard.** Computing the output from known inputs is fast (one Raido evaluation). Finding inputs that produce a desired output requires searching the input space — there's no analytical shortcut.

The galaxy seed parameterizes the function. Each galaxy has different chemistry. Reading the Raido source code tells you the algorithm, but the seed makes the specific landscape unique. Like knowing SHA-256's algorithm doesn't help you find a preimage.

### Local Smoothness, Global Chaos

Pure pseudorandomness would make experimentation a lottery. Real chemistry has structure — small changes in composition usually produce small changes in properties. The interaction function preserves this:

**Within a phase region**, the function is smooth. Nearby inputs produce nearby outputs. A researcher can hill-climb — try a ratio, adjust slightly, observe improvement, adjust again. Standard gradient-following works. Experiments are informative. Progress is incremental.

**At phase boundaries**, the function is discontinuous. A small change in ratio or energy crosses a boundary and the output jumps to a completely different regime. What was improving suddenly collapses, or something unexpected appears.

The phase regions are a tessellation of the input space — their boundaries are determined by the seed. Within each region, the interaction function is a smooth mapping with region-specific characteristics (also seed-derived). Across boundaries, unrelated.

This creates the right research dynamics:
- **Hill-climbing works locally.** A researcher exploring a phase region can optimize systematically. "More element B improves hardness" — true within this region. Each experiment narrows the search.
- **Boundaries are unpredictable.** You don't know where the next boundary is until you cross it. A ratio change from 0.31 to 0.32 might be smooth. From 0.32 to 0.33 might cross a boundary and produce something completely different.
- **Breakthroughs come from boundary crossings.** The best materials aren't at the peaks of known regions — they're in undiscovered regions on the other side of boundaries nobody has crossed yet.
- **You can't extrapolate across boundaries.** Knowledge of one region tells you nothing about adjacent regions. Each boundary crossing is a fresh discovery.

### Energy as a Dimension

Energy isn't just a scaling factor — it's a full dimension of the input space. The phase tessellation spans the energy axis too. This means increasing energy doesn't just "turn up" existing effects. At certain energy levels, you cross phase boundaries in the energy dimension and enter entirely new regions.

Low energy → one set of phase regions (conventional chemistry).
Medium energy → different regions (advanced metallurgy).
High energy → yet another landscape (exotic physics).

The boundaries in the energy dimension are the natural activation thresholds. You don't need a separate threshold parameter — the phase structure handles it. At low energy, you're in regions where the interaction function produces conventional material properties. Push energy high enough and you cross into regions where the function produces non-zero values for properties that were zero in every low-energy region.

This is how latent physics emerges from the same mechanism. No special case needed.

### Ratio Sensitivity

Within a phase region, the interaction function is smooth but not necessarily gentle. Some regions have steep gradients — a 1% ratio change produces a 20% property change. Others are flat — large ratio changes barely matter. The gradient structure is seed-determined and varies by region.

A researcher mapping a steep region needs precise ratio control (better lab equipment). A researcher in a flat region can be sloppy. The physics doesn't prescribe which regions are steep — the seed determines it. Some galactic chemistries reward precision. Others reward breadth of exploration.

## Multi-Element Combinations

More elements means higher-dimensional input space. The phase tessellation extends naturally — it's defined over the full space of (element fractions × energy). With two elements, the input space is 2D (ratio + energy). With three, it's 3D. With five, it's 5D.

Higher-dimensional spaces have exponentially more phase regions. This is why multi-element research is harder and more rewarding — the landscape is richer but the search cost grows exponentially with the number of input elements. A binary search (2 elements) might find good materials in dozens of experiments. A ternary search (3 elements) might take hundreds. A quinary search (5 elements) could take thousands.

The founding cluster tunes the phase density (how many regions per unit of input space) to control discovery pace. Dense tessellation = lots of boundaries = lots of surprises but hard to optimize within any single region. Sparse tessellation = large smooth regions = easier optimization but fewer breakthrough opportunities.

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

This also means manufacturing efficiency is a genuine competitive axis. Two factions might know the same recipe (same inputs, same output), but the one with better lab equipment (lower base loss, more precise energy control reducing energy_loss) produces more output per input. Industrial advantage from infrastructure, not just knowledge.

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

Mechanically: a catalyst element is an input to the interaction function but is excluded from the mass budget. It appears in the inputs and the outputs (not consumed). Its presence shifts the effective input coordinates — potentially moving the evaluation point across a phase boundary that would otherwise be unreachable at the current energy level.

```
effective_input = shift(base_input, catalyst_element, catalyst_fraction)
```

A catalyst effectively lets you access neighboring phase regions without the energy to reach them directly. The shift function is part of the interaction algorithm — deterministic, seed-dependent, computationally opaque like everything else. Most element-as-catalyst combinations produce negligible shifts. A few produce large shifts that cross boundaries into productive regions.

This is why geographic scarcity matters for research. A rare catalyst element doesn't just make existing processes cheaper — it makes the interaction function evaluate at points that are otherwise inaccessible. A faction controlling rare catalyst deposits can explore regions of the landscape that nobody else can reach. Not from a multiplier. From geometry — the catalyst shifts their position in input space.

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

The energy boundaries that gate latent properties aren't reachable with day-one reactors. Reaching them requires better reactor materials — which require discovering better alloys — which requires reaching intermediate energy boundaries first. Each tier of energy capability unlocks phase regions that contain materials for the next tier.

This cascade isn't prescribed. It emerges from the phase structure of the interaction function. The founding cluster designs the seed and the interaction algorithm such that the energy boundaries fall at levels that create a natural progression. But nobody can predict exactly which path through the cascade is fastest — that depends on which phase regions happen to contain the best intermediate materials, which depends on the seed.

A faction running high-energy experiments with rare elements might notice a tiny non-zero value for "spatial distortion" in their output. What is that? The physics script has performance functions for it. But nobody has ever produced enough to matter. The faction pushes deeper — spends more material, maps the landscape around that anomalous result, finds the local gradient, climbs it. Eventually they produce a material with significant spatial distortion. Plug it into a system. The performance function evaluates — and the system does something nobody has seen before.

That's discovery. Not a tech tree unlock. Not a recipe. The universe had more to offer than anyone knew.

### Computational Opacity Is the Protection

The physics script is public. Anyone can read the algorithm. But the interaction function is seed-parameterized and computationally opaque — knowing the algorithm tells you the *structure* (phases, boundaries, smoothness) but not the *content* (where the boundaries fall, what each region produces). That's determined by the seed, and the only way to learn it is to evaluate the function at specific points. Each evaluation is an experiment that costs materials.

You can read the source code and see that spatial distortion is a property dimension. You can see that the interaction function CAN produce non-zero values for it. But you can't compute WHICH inputs produce it without running the experiments. The function is a black box in practice even though it's transparent in principle. Like knowing that SHA-256 has preimages without being able to find them.

### Progression Tiers

Not prescribed — emergent from the phase landscape. But the founding cluster designs the seed to create natural tiers:

**Tier 0 — Common chemistry.** Low-energy phase regions with common elements. Basic alloys, structural materials, simple conductors. Founding cluster publishes starter recipes that exploit a few known regions. Wide, gentle regions — easy to explore, small improvements everywhere.

**Tier 1 — Advanced materials.** Medium-energy regions. Better property values, some steep gradients rewarding precision. Requires better energy infrastructure (built from tier 0 materials). The "orbital" and "interstellar" phase from PHYSICS.md.

**Tier 2 — Exotic materials.** High-energy regions, often requiring 3+ elements including rare ones. Dense phase tessellation — many boundaries, frequent surprises. First non-zero values in latent property dimensions. The "industrial space" phase.

**Tier 3 — New physics.** Extreme-energy regions in high-dimensional input spaces (4-5 elements with exotics). Significant latent property values. New system types become viable. Cascading prerequisites — tier 2 materials for the reactors that reach tier 3 energy. The "stellar" phase becomes theoretically accessible.

A faction might reach tier 3 in one property dimension while stuck at tier 1 in others. Progress is multidimensional.

### Founding Cluster Design Responsibility

The interaction function algorithm and its relationship to the galaxy seed is the founding cluster's most important creative act. They're designing the universe's chemistry — not as lore, but as math. The algorithm determines:

- Phase region density at each energy level (controls discovery pace)
- Which property dimensions are latent vs. active (controls capability progression)
- How seed variation affects the landscape (controls inter-galaxy uniqueness)
- The smoothness-to-chaos ratio within regions (controls how rewarding systematic research is)
- Where the energy-dimension boundaries fall (controls tier progression)

This is universe design. The founding cluster tunes it through playtesting. The algorithm can evolve (new script version, voluntary adoption) but the seed doesn't change. Factions explore a continent that already exists — the interaction function drew the map but nobody has it.

## Verification

When a domain mints a crafted material, the proof includes:

1. **Input proof** — what elements went in (references to prior object proofs)
2. **Process proof** — the crafting script hash (content-addressed Raido bytecode)
3. **Output claim** — the resulting material's property vector
4. **Physics evaluation** — standard physics script applied to the inputs and process

Any domain can verify by re-executing:

1. Fetch the crafting script (by hash). Run it against the claimed inputs.
2. Fetch the standard physics script. Evaluate the transformation — does the claimed output fall within the bounds that the combination physics compute for those inputs at that energy?
3. Evaluate the finished object against the five constraint laws (existing verification).

If the crafting script claims an output outside the bounds computed by the combination physics, the transformation is invalid. The material can't exist. Trust flag.

This is the same verification pattern as constraint physics — re-executable, deterministic, independent. The transformation physics extends the standard physics script with a `verify_transformation(inputs, process, output) -> bool` function alongside the existing `verify_object(component_tree) -> bool`.

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

**Materials research** — exploring the phase landscape. Opaque. Requires element inputs, energy, catalysts, materials labs.

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

**Labs and infrastructure:** Lab quality determines what energy levels and process types are available. A crude workshop can alloy at low energy. A plasma forge operates at high energy, activating interactions that a workshop can't reach. Lab equipment (from PHYSICS.md) has mass, volume, power draw, structural requirements — constrained by the five laws like everything else. Better labs require better materials, which require better labs. The spiral is intentional.

## Constants and Tuning

| Constant | Layer | What it controls |
|----------|-------|-----------------|
| Element property vectors | Material | Base properties of each element (the weighted average inputs) |
| Interaction function algorithm | Material | The core mapping — how inputs combine to produce property modifications |
| Galaxy seed | Material | Parameterizes the interaction function — determines the specific phase landscape |
| Phase density parameters | Material | How many phase regions per unit of input space at each energy level |
| Smoothness parameters | Material | How gentle the gradients are within phase regions |
| Catalyst efficiency function | Material | How catalysts modify the effective energy level |
| Base loss fraction | Both | Minimum material loss during any transformation |
| Energy loss coefficient | Material | How fast loss scales with energy invested |
| Reference energy | Material | Energy level at which energy_loss equals the coefficient |
| Complexity loss coefficient | Material | Additional loss per input element beyond the first |
| Theoretical property maximums | Material | Hard ceiling for each material property (saturation curve) |
| Performance functions | System | How component properties derive system performance |
| System interaction coefficients | System | Non-linear bonuses from component combinations |
| Theoretical performance maximums | System | Hard ceiling for each performance characteristic |

All constants are part of the standard physics script. Content-addressed, published, verifiable. The founding cluster tunes through playtesting and publishes updates (new script hash, voluntary adoption).

The element count and interaction table size determine the game's discovery depth. More elements = larger search space = longer discovery timeline. The founding cluster starts small (12-16 elements) and can expand the table in future updates — adding new elements that the seed already placed in the galaxy but that prior scripts didn't know how to evaluate. "The elements were always there. We built better instruments."

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
