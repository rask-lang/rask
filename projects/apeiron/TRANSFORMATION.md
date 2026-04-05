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

When elements combine, the output material's properties are determined by three things: the input ratios, the energy invested, and the interaction effects between element pairs.

### Base Properties: The Weighted Average

The simplest combination — melt two elements together with no special process — gives you the weighted average:

```
base[p] = sum(element[i].fraction * element[i].property[p])
```

This is boring and expected. An alloy of 70% A and 30% B has properties somewhere between A and B. No surprises, no discoveries. This is what you get for free.

### Interaction Effects: Where Discovery Lives

Every pair of elements has an interaction coefficient for each property. These coefficients modify the base properties non-linearly:

```
material[p] = base[p] + sum_pairs(
    fraction[i] * fraction[j] * interaction[i][j][p] * energy_factor
)
```

The interaction table is the heart of the system. It's a matrix of element-pair coefficients, one per property. Most entries are small (boring combinations). Some are large positive (synergistic combinations — the alloy is stronger than either component). Some are large negative (antagonistic — the combination is worse than expected).

The key insight: **interaction effects are non-obvious.** The interaction table is part of the standard physics script — deterministic, public, verifiable. But with 20 elements, there are 190 pairs, each with 6 property coefficients. That's 1,140 interaction terms. Add three-element combinations and the space explodes. Nobody can map this analytically. You have to experiment.

### Energy Factor: The Process Matters

The `energy_factor` in the interaction formula scales with energy invested per unit mass of output:

```
energy_factor = min(energy_per_mass / activation_threshold, max_factor)
```

Every interaction has an **activation threshold** — the minimum energy density needed for the interaction to manifest. Below threshold, energy_factor is zero and you get the boring weighted average. Above threshold, the interaction effect kicks in and scales up to a maximum.

This is physically intuitive. Room-temperature mixing gives you one thing. Arc-furnace processing gives you another. Plasma sintering gives you something else. The energy level determines which interaction effects are active.

Low-energy processes are cheap but produce baseline materials. High-energy processes are expensive but unlock interaction effects. Some interactions activate at low energy (easy alloys). Others require extreme energy (exotic materials). The activation thresholds are part of the interaction table.

### Ratio Sensitivity

The interaction effect depends on `fraction[i] * fraction[j]` — it peaks when both elements are present in significant quantities and vanishes when one is trace. But the peak isn't always at 50/50. The interaction table includes an optimal ratio for each pair:

```
ratio_modifier = exp(-((ratio - optimal_ratio) / width)^2)
```

A Gaussian centered on the optimal ratio. Some interactions are broad (work across a range of ratios). Others are sharp (need precise ratios). Finding the optimal ratio for a powerful interaction is part of the discovery.

This means a researcher might know that elements A and B interact well (positive coefficient for hardness) but not know the optimal ratio. Experiments at different ratios map out the peak. Each experiment costs materials.

## Multi-Element Combinations

Two-element interactions are the foundation. Three-element interactions add another layer.

Ternary interaction terms exist but are weaker and rarer than binary ones. The formula extends naturally:

```
material[p] = base[p]
    + sum_binary(f[i] * f[j] * binary[i][j][p] * energy_factor)
    + sum_ternary(f[i] * f[j] * f[k] * ternary[i][j][k][p] * energy_factor)
```

Most ternary terms are zero. A few are significant — these are the "breakthrough" combinations where three specific elements together produce effects that no pair achieves alone. Finding these requires systematic exploration and a lot of material.

Four-element and higher interactions exist in principle but can be negligible for the initial system. The founding cluster can add higher-order terms in later standard physics script updates if the gameplay warrants it.

## Mass Budget

Conservation during transformation:

```
output_mass <= input_mass * (1 - loss_fraction)
```

Every transformation has material loss. The loss fraction has a floor (you can't achieve 100% yield) and scales with how far the output properties deviate from the base weighted average:

```
loss_fraction = base_loss + deviation_loss * property_shift_magnitude
```

Conservative transformations (close to weighted average) waste less. Aggressive transformations (far from average, exploiting strong interactions) waste more. This is the material cost of pushing boundaries — more exotic outputs burn more inputs.

The lost mass is gone. Destroyed. This feeds into Allgard's Conservation Law 3 — crafting loss is a designed entropy sink.

## Theoretical Limits

No material property can exceed a theoretical maximum defined in the standard physics script. The interaction effects asymptotically approach but never reach these limits:

```
effective_property = theoretical_max * (1 - exp(-raw_value / theoretical_max))
```

As raw computed values get large, the effective property saturates. Diminishing returns are baked into the math. You can always make materials incrementally better, but each increment costs more energy and rarer inputs for less improvement.

The theoretical maximums are constants — part of the standard physics script, set by the founding cluster. They define the ultimate ceiling for each property in the galaxy. A civilization that maxes out structural efficiency has hit the physics wall. No amount of clever crafting gets past it. The founding cluster tunes these ceilings to create the progression arc described in PHYSICS.md — each technology phase corresponds to reaching a certain fraction of theoretical limits.

## Catalysts

Some elements, when present in small quantities during a transformation, modify the interaction effects without being consumed. These are catalysts.

Mechanically: a catalyst element contributes to interaction effects but is excluded from the mass budget. It appears in the inputs and the outputs (not consumed). Its presence modifies activation thresholds — lowering the energy required for specific interactions.

```
effective_threshold = base_threshold * catalyst_modifier(catalyst_element, interaction_pair)
```

Most elements are bad catalysts for most interactions (modifier ≈ 1.0, no effect). A few elements are excellent catalysts for specific interactions (modifier << 1.0, dramatically reducing energy requirements). Finding these is high-value research.

Rare elements from the seed often function as catalysts for exotic interactions. This is why geographic scarcity matters for research: a system with rare element deposits enables transformations that are energetically impossible elsewhere. Not because the rare element IS the material — it enables the PROCESS.

A faction controlling rare catalyst deposits has a genuine research advantage. Not from a multiplier. From physics.

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

Here's what this system actually creates for players:

The combination physics defines a landscape over material space. Every possible input combination (elements, ratios, energy, catalysts) maps to a specific output. The landscape is deterministic — everyone has the same physics. But it's high-dimensional and non-linear. Nobody can map it by staring at the formulas.

**Easy discoveries:** Two common elements with a significant binary interaction. Moderate energy. Broad ratio peak. These are the early-game alloys — better than raw elements, accessible with basic infrastructure. The founding cluster publishes starter recipes that exploit a few obvious interactions. Player communities find more through systematic experimentation.

**Hard discoveries:** Three elements, one rare, with a ternary interaction that activates at high energy. Sharp ratio sensitivity. Needs a catalyst from a different star system. These are breakthrough materials — dramatically better constants for specific properties. Finding them requires: access to rare elements (exploration/trade), high-energy labs (infrastructure investment), many experiments (material cost), and sometimes luck.

**The search game:** A researcher has a goal — "I want a material with higher structural efficiency than anything known." The landscape says such materials exist (the theoretical limit is higher than anything discovered). But where? Which elements? What ratios? What energy? Which catalyst? The researcher designs experiments, consumes materials, maps a small region of the landscape, adjusts hypotheses, tries again. This is genuine research gameplay — not "click research button, wait timer."

**Information asymmetry:** A faction that discovers a breakthrough material knows the recipe (crafting script). They can produce the material. They can sell finished goods (trading partners see the output, not the process). They can sell the recipe (valuable but creates competitors). Or they can keep it secret — but the component tree is visible on transfer, so sophisticated rivals can reverse-engineer the composition and narrow down the search space.

## Latent Physics

The interaction table contains terms that evaluate to zero or negligible at normal conditions. They're not hidden — anyone can read the standard physics script. But under extreme conditions — very high energy, specific rare element combinations, unusual catalyst configurations — these dormant terms activate and produce effects that aren't just "better structural efficiency" but qualitatively new capabilities.

This is how real physics works. Relativistic effects are always in the equations. They're negligible at walking speed. At 0.9c they dominate. Nobody unlocked relativity. The math was always there. The conditions to observe it are just extreme.

### How It Works

The interaction formulas already have the mechanism: energy_factor scales interaction effects, and activation thresholds gate when effects manifest. Latent physics extends this with **threshold cascades** — certain interaction terms have activation thresholds so high that they require materials produced by *prior* discoveries to reach.

```
// A normal interaction: activates at energy_per_mass > 100
binary[iron][carbon][hardness] = 2.4, threshold = 100

// A latent interaction: activates at energy_per_mass > 50000
// (unreachable without advanced reactor tech providing the energy)
binary[exotic_A][exotic_B][spatial_compression] = 8.1, threshold = 50000
```

The `spatial_compression` property does nothing at the system design level until a performance function reads it. The founding cluster can publish performance functions that reference properties nobody can produce yet — the function exists, the property evaluates to zero with all known materials, and the system produces no novel effect. When someone eventually produces a material with non-zero spatial_compression, the performance function activates and a new class of system becomes possible.

### New Properties, Not New Rules

The element property vector has a fixed set of dimensions. But nothing requires all dimensions to be useful from day one. The founding cluster can define property dimensions that no known element combination produces in significant quantities:

| Property | Status at launch | What it enables when non-zero |
|----------|-----------------|-------------------------------|
| Hardness | Active — common materials produce it | Structural components, armor, tools |
| Conductivity | Active — common materials produce it | Power routing, sensors, communications |
| Spatial distortion | Latent — no known combination produces it above noise | Jump drives, gravity manipulation, spatial compression |
| Field coherence | Latent — requires exotic catalysts at extreme energy | Force fields, containment, directed energy |
| Phase stability | Latent — requires ternary exotic combinations | Metamaterials, cloaking, sensor dampening |

The latent properties are in the physics script from day one. The formulas that produce them require conditions that day-one technology can't reach. As factions push into extreme regimes — higher energy, rarer elements, more complex combinations — the latent properties start appearing in their experimental outputs. First as noise. Then as signal.

A faction running high-energy experiments with rare elements notices a tiny non-zero value for "spatial distortion" in their output material. What is that? The physics script has formulas for it. The performance functions reference it. But nobody has ever produced enough of it to matter. The faction runs more experiments. Spends more material. Maps the landscape around that anomalous result. Eventually produces a material with significant spatial distortion. Plugs it into a system design. The performance function evaluates — and the system does something nobody has seen before.

That's discovery. Not a tech tree unlock. Not a recipe. A physicist pushing into unknown territory and finding that the universe has more to offer than anyone suspected.

### Why Not Just Hide It

I considered making latent physics truly hidden — obfuscated bytecode, encrypted interaction tables, unknown property dimensions. I rejected that because it contradicts the design philosophy. The physics is public. The standard physics script is readable. Transparency is a core value.

Instead, latent physics relies on **computational opacity**: the formulas are public but the combinatorial search space is too large to map analytically. You can read the code and see that spatial_distortion has non-zero interaction terms for exotic_A + exotic_B at threshold 50000. But you still need to:

1. Find or trade for exotic_A and exotic_B (geographic scarcity)
2. Build a facility capable of energy_per_mass > 50000 (requires prior material breakthroughs for the reactor and containment)
3. Run actual experiments to find the optimal ratio and catalyst (costs real materials each attempt)
4. Design a system that exploits the new property (system design research)

Knowing the math exists doesn't make it free. Theoretical physics and engineering are different disciplines. Both are required.

### Progression Tiers

The latent physics creates a natural progression without prescribing it:

**Tier 0 — Common chemistry.** Standard element interactions at moderate energy. Basic alloys, structural materials, simple conductors. Available from day one with founding cluster starter recipes.

**Tier 1 — Advanced materials.** Strong binary and some ternary interactions. Requires access to uncommon elements and higher-energy facilities. Better structural efficiency, better thermal management, better shielding. What PHYSICS.md calls the "orbital phase" and "interstellar phase."

**Tier 2 — Exotic materials.** Ternary interactions with rare elements and catalysts. Extreme energy thresholds. Materials with unusual property combinations — extremely high values in one dimension, or moderate values in dimensions that are normally zero. The first hints of latent properties appearing. Enables the "industrial space phase."

**Tier 3 — New physics.** Materials with significant values in latent property dimensions. New system types become viable — force fields, jump drives, gravitational manipulation. Requires cascading breakthroughs: tier 2 materials to build the labs that produce tier 3 materials. The "stellar phase" becomes theoretically accessible.

These tiers aren't prescribed. They emerge from the threshold cascade structure of the interaction table. A faction might reach tier 3 in one property dimension while stuck at tier 1 in others. Progress is multidimensional, like PHYSICS.md describes for the technology arc.

### Founding Cluster Design Responsibility

The interaction table and latent physics are the founding cluster's most important creative act. They're designing the universe's chemistry and exotic physics — not as lore, but as math. The constants determine:

- How many meaningful material tiers exist
- How long discovery takes at each tier
- Which rare elements are strategically valuable
- What new capabilities are theoretically possible
- How hard each capability is to unlock

This is universe design. The founding cluster tunes it through playtesting. Constants can evolve (new script version, voluntary adoption) but the relationships don't change. Once the interaction table is published, the discovery landscape is fixed. Factions explore a continent that already exists — the founding cluster drew the map but nobody has it.

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

Material synthesis produces materials with properties. System design produces *functional systems* — engines, reactors, shields, weapons, sensors — with performance characteristics. This is the second transformation layer.

The problem is identical in structure: without design rules, a crafting script can claim "I built a 1kg engine that produces infinite thrust." The finished object passes the constraint laws (it has mass, volume, energy draw). But the performance claim is ungrounded. System design physics constrains the relationship between what goes in and what comes out.

### System Properties Derive From Composition

A system is a component tree. Its performance characteristics derive from its materials and structure — not declared. An engine's thrust derives from its combustion chamber material (thermal tolerance), nozzle geometry (structural properties under heat), fuel injector precision (material conductivity and hardness), and power supply. The physics script computes performance from composition.

Each system type has a **performance function** — a formula that takes the component tree and derives performance:

```
thrust = f(chamber_material.stability, chamber_material.radiance,
           nozzle_material.hardness, nozzle_mass,
           fuel_flow_rate, power_input)
```

The performance functions are part of the standard physics script. They're the system-level equivalent of the element interaction table — deterministic, verifiable, published.

### Design Parameters

Where material synthesis has element ratios and energy, system design has **design parameters**: choices about how to arrange and configure components. These aren't material choices — they're engineering choices.

- **Geometry** — how components are arranged spatially (affects proximity coupling, Law 5)
- **Operating point** — where on the performance curve the system targets (affects stress, Law 4)
- **Tolerance margins** — how much safety margin the design includes (trading peak performance for reliability)
- **Routing** — how power, fuel, coolant, and data flow between components (affects mass and coupling)

Design parameters create tradeoffs within the physics. Two engines with identical materials can have different thrust if one uses a higher operating point (more thrust, faster degradation) or tighter geometry (lighter, more coupling interference). The physics computes the consequences. The designer chooses the tradeoffs.

### The Design Landscape

System design has its own discovery landscape, orthogonal to material discovery:

**Material researchers** explore element combinations to produce better materials. The discovery is: "these elements at this ratio and energy produce a material with excellent thermal stability."

**System designers** explore component arrangements to produce better systems. The discovery is: "this chamber geometry with this nozzle configuration and this material produces 20% more thrust than the standard design."

Both are experiments. Both cost resources (materials consumed, lab time, energy). Both produce knowledge (crafting scripts). Both benefit from the combination landscape being too large to brute-force.

A faction with better materials AND better system designs has a compounding advantage. But they're separable — you can be a material scientist who sells raw alloys, or a system engineer who buys materials and designs engines. Specialization is viable at both levels.

### Interaction Effects in System Design

Like element interactions, component interactions have non-linear effects. An engine with a standard fuel injector performs linearly with fuel flow. But a fuel injector built from a high-conductivity material paired with a precision-machined nozzle might exhibit a resonance effect — fuel atomization improves non-linearly, producing a thrust boost that neither component achieves alone.

These system-level interactions are encoded in the performance functions. They depend on the material properties of the components (tying back to the element interaction layer) and on the design parameters. The interaction table exists at both levels, and they compose.

This is where the real depth lives. A breakthrough material enables a breakthrough system design that enables a new class of ship that changes the strategic landscape. The progression isn't linear — it's a web of interacting discoveries across material and system layers.

### Process Types for System Design

| Process | Character |
|---------|-----------|
| **Assembly** | Combining components into a system. Standard construction. |
| **Optimization** | Modifying an existing system's design parameters without changing materials. Cheaper than rebuilding. |
| **Reverse engineering** | Studying a system's component tree to infer its design parameters. Produces partial knowledge — you learn what's in it, but reconstructing the crafting script requires experiments. |
| **Scaling** | Building a larger or smaller version of a known design. Not free — structural scaling (Law 2) means naive scaling fails. Requires re-solving the design at the new scale. |

### What This Means for Research Categories

Research isn't a single activity. It decomposes naturally along the two transformation layers:

**Materials research** — combining elements, finding interaction effects, producing alloys with specific property profiles. Requires: element inputs, energy, catalysts, materials labs.

**Propulsion research** — designing engines with better thrust-to-weight, fuel efficiency, reliability. Requires: candidate materials, energy, propulsion test facilities.

**Reactor research** — designing power sources with higher energy density, lower mass, better thermal management. Requires: high-energy materials, shielding materials, reactor test labs with heavy coupling isolation.

**Weapons research** — designing systems that concentrate energy effectively at range. Requires: high-conductivity materials, high-stability materials, weapons test ranges.

**Shielding research** — designing systems that absorb, deflect, or dissipate incoming energy. Requires: high-absorption materials, structural materials, shielding test facilities.

**Sensor research** — designing systems that detect faint signals in noisy environments. Requires: high-sensitivity materials, vibration isolation, EM-quiet test environments.

None of these categories are prescribed by the physics. They emerge because different performance functions depend on different material properties, which means different research paths need different inputs and facilities. A faction that wants better engines pursues thermal stability and radiance in their material research. A faction that wants better shields pursues conductivity and stability. The same element interaction table serves both — but they're exploring different regions of it.

## Interaction With Existing Systems

**Constraint physics (PHYSICS.md):** Material synthesis produces materials. System design produces functional components. Constraint physics governs the finished objects built from them. A crafted alloy with excellent structural efficiency feeds into Law 2 — it lowers `k`, making bigger structures viable. A better engine design feeds into Law 3 — more thrust per watt, extending range at the same mass. Transformation physics creates the parts. Constraint physics judges the whole.

**Conservation laws (Allgard):** Mass lost during transformation satisfies Conservation Law 3 (exchange conservation — crafting loss is a declared sink). Inputs consumed are destroyed (minting/burning backed by Raido script). Outputs are new objects. The full chain is auditable.

**Geographic scarcity:** The seed distributes elements and their abundances. Common elements appear everywhere. Rare elements appear in specific systems. Catalyst elements may be extremely scarce. The combination physics makes rare elements valuable not by fiat but because they enable transformations that common elements can't.

**Labs and infrastructure:** Lab quality determines what energy levels and process types are available. A crude workshop can alloy at low energy. A plasma forge operates at high energy, activating interactions that a workshop can't reach. Lab equipment (from PHYSICS.md) has mass, volume, power draw, structural requirements — constrained by the five laws like everything else. Better labs require better materials, which require better labs. The spiral is intentional.

## Constants and Tuning

| Constant | Layer | What it controls |
|----------|-------|-----------------|
| Element property vectors | Material | Base properties of each element |
| Binary interaction table | Material | Pairwise interaction coefficients (per property) |
| Ternary interaction table | Material | Three-element interaction coefficients |
| Activation thresholds | Material | Energy density required for each interaction |
| Optimal ratios | Material | Peak ratio for each interaction pair |
| Ratio widths | Material | How sensitive interactions are to ratio precision |
| Catalyst modifiers | Material | How much catalysts reduce activation thresholds |
| Base loss fraction | Both | Minimum material loss during crafting |
| Deviation loss factor | Material | Additional loss scaling with property shift |
| Theoretical property maximums | Material | Hard ceiling for each material property |
| Max energy factor | Material | Upper bound on energy scaling |
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
