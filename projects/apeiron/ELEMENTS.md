# Element Table
<!-- id: apeiron.elements --> <!-- status: proposed --> <!-- summary: The founding cluster's element definitions — property vectors, abundances, and starter recipes -->

The founding cluster's element table. Sixteen elements, six properties each. Published as part of the standard physics script.

## Design Choice: Real Names

The elements are named after real-world elements and their property vectors are inspired by real-world intuitions. Iron is dense and hard. Copper conducts. Lithium is light and reactive. This isn't simulating real chemistry — the interaction function is still seed-determined, opaque, and entirely its own physics. But the names give players an intuitive starting point. When "iron + carbon" produces something steel-like at the founding cluster's published ratio, that feels *right*. That's immersion, not simulation.

The benefit: known alloys (steel, bronze, titanium alloys) work as natural starter recipes. Players bring real-world intuitions about what might combine well. Most of those intuitions will be roughly correct for simple combinations — and completely wrong for exotic multi-element, high-energy phases where Apeiron's physics diverges from reality entirely. The familiar on-ramp makes the alien interior more surprising.

What this is NOT: a lookup table of real metallurgy. The interaction function doesn't know about crystal lattices or electron shells. Two players in different galaxies (different seeds) will find different optimal ratios for "steel" because the stoichiometric peaks are seed-determined. The names are handles for intuition, not constraints on behavior.

## The Sixteen Elements

Properties on a 0.00–1.00 scale. These are *base element properties* — the weighted-average floor when elements combine. The interaction function's stoichiometric peaks can push material properties well beyond any single element's base values.

### Common — Found in every star system

| # | Element | Den | Hard | Cond | React | Stab | Rad | Character |
|---|---------|-----|------|------|-------|------|-----|-----------|
| 1 | **Iron** | 0.72 | 0.65 | 0.35 | 0.45 | 0.60 | 0.10 | The structural workhorse. Dense, hard, moderate in everything else. Backbone of early-game construction. |
| 2 | **Carbon** | 0.15 | 0.80 | 0.12 | 0.75 | 0.85 | 0.08 | The universal bonder. Extreme hardness (diamond), extreme reactivity (bonds with nearly everything), high stability. Tiny amounts transform other elements. |
| 3 | **Silicon** | 0.25 | 0.60 | 0.45 | 0.30 | 0.75 | 0.15 | The semiconductor. Moderate across the board with a conductivity sweet spot — not a great conductor, not an insulator. Electronics and sensors. |
| 4 | **Copper** | 0.65 | 0.20 | 0.92 | 0.35 | 0.45 | 0.05 | The conductor. Soft, heavy, superb conductivity. The gap between copper's conductivity and everything else is enormous. |
| 5 | **Aluminum** | 0.18 | 0.22 | 0.70 | 0.50 | 0.55 | 0.05 | The lightweight. Low density, decent conductivity, moderate reactivity. The strength-to-weight enabler in alloys. |

### Industrial — Present in most systems, varying quantities

| # | Element | Den | Hard | Cond | React | Stab | Rad | Character |
|---|---------|-----|------|------|-------|------|-----|-----------|
| 6 | **Titanium** | 0.38 | 0.70 | 0.10 | 0.15 | 0.90 | 0.05 | The balanced performer. Hard, light, extraordinarily stable, poor conductor. Where you need strength without weight and don't care about carrying current. |
| 7 | **Chromium** | 0.60 | 0.85 | 0.30 | 0.25 | 0.88 | 0.12 | The hardener. Very hard, very stable. Small additions to other elements dramatically boost hardness and corrosion resistance. |
| 8 | **Nickel** | 0.68 | 0.45 | 0.40 | 0.20 | 0.72 | 0.08 | The alloy partner. Unremarkable alone — moderate everything. Shines in combination. Stabilizes other elements without dominating their character. |
| 9 | **Tin** | 0.55 | 0.12 | 0.28 | 0.40 | 0.50 | 0.03 | The soft one. Low hardness, low radiance, moderate reactivity. Useful as a mixing element — it yields to the character of its partners. |
| 10 | **Zinc** | 0.52 | 0.18 | 0.32 | 0.55 | 0.35 | 0.05 | The reactive protector. Moderate density, notable reactivity, poor stability. Sacrifices itself to protect other elements (galvanization analogue). |

### Scarce — Found in specific regions, worth trading for

| # | Element | Den | Hard | Cond | React | Stab | Rad | Character |
|---|---------|-----|------|------|-------|------|-----|-----------|
| 11 | **Tungsten** | 0.95 | 0.95 | 0.25 | 0.08 | 0.92 | 0.20 | The extremophile. Hardest element, densest practical element, nearly inert. Extreme environments — reactor containment, armor piercing, heat shields. |
| 12 | **Silver** | 0.72 | 0.15 | 0.98 | 0.18 | 0.52 | 0.25 | The superconductor. Even better conductivity than copper, plus meaningful radiance. The premium conductor — but scarce and soft. |
| 13 | **Lithium** | 0.04 | 0.05 | 0.35 | 0.95 | 0.15 | 0.70 | The energetic lightweight. Lightest element, most reactive, highest radiance of any common-ish element, terrible stability. The fuel element. High radiance makes lithium compounds the natural starting point for energy-dense fuel. Dangerous to handle — reactivity and low stability mean lithium-rich materials degrade. |

### Rare — Found in few systems, primary catalyst candidates

| # | Element | Den | Hard | Cond | React | Stab | Rad | Character |
|---|---------|-----|------|------|-------|------|-----|-----------|
| 14 | **Cobalt** | 0.68 | 0.50 | 0.38 | 0.30 | 0.65 | 0.45 | The magnetic catalyst. Moderate properties with notable radiance. Primary catalyst for mid-tier stoichiometric peaks — dramatically lowers activation energy for iron-family alloys. |
| 15 | **Platinum** | 0.92 | 0.30 | 0.55 | 0.05 | 0.95 | 0.30 | The noble catalyst. Nearly inert, extremely stable. Premier catalyst for high-energy peaks — its presence enables reactions that would otherwise require vastly more fuel. Worth more as a catalyst than as a material input. |
| 16 | **Iridium** | 0.98 | 0.55 | 0.42 | 0.03 | 0.98 | 0.60 | The exotic. Densest, most stable, significant radiance. Extremely rare. The gateway element — its presence as a catalyst unlocks the highest-energy stoichiometric peaks where latent properties (spatial distortion, field coherence, phase stability) first become non-zero. No iridium access means no path to tier 3 materials at reasonable energy levels. |

## Abundance Distribution

Relative abundance across the galaxy. The seed distributes actual quantities per star with high variance around these means — some stars are iron-rich deserts, others are balanced. The distribution creates geographic scarcity.

| Tier | Elements | Galaxy share | Per-star variance | Gameplay role |
|------|----------|-------------|-------------------|---------------|
| Common | Iron, Carbon, Silicon, Copper, Aluminum | ~82% of extractable mass (each ~14-20%) | Low variance. Every star has meaningful deposits. | Day-one building. No scarcity gates. |
| Industrial | Titanium, Chromium, Nickel, Tin, Zinc | ~15% of extractable mass (each ~2-4%) | Moderate variance. Most stars have some, quantities differ 5x. | Mid-game alloys. Trade begins when someone needs chromium and you have excess. |
| Scarce | Tungsten, Silver, Lithium | ~2.5% of extractable mass (each ~0.5-1.2%) | High variance. Many stars have none. Concentrated in specific regions. | Geographic value. Trade routes form around scarce deposits. |
| Rare | Cobalt, Platinum | ~0.4% of extractable mass (each ~0.15-0.25%) | Very high variance. Found in ~20% of stars. | Catalyst access. Controlling rare deposits is a strategic advantage — not marginal, categorical. |
| Exotic | Iridium | ~0.1% of extractable mass | Extreme variance. Found in ~5% of stars. Often in hostile/remote systems. | Endgame gating. The key to latent-property materials. Worth building alliances — or wars — over. |

The total extractable mass per star varies ~10x from the poorest to the richest. A rich star might have 500K mass units of iron. A poor star might have 50K. But rare elements don't scale proportionally — a rich star isn't more likely to have iridium. Rare element placement is independent of total richness.

## Starter Recipes

The founding cluster publishes these recipes — known stoichiometric peaks, verified in the founding galaxy seed. They're the on-ramp. Wide, forgiving peaks with low energy thresholds. Every new researcher can reproduce them day one.

| Recipe | Inputs | Ratio | Peak type | What it produces | Real-world analogue |
|--------|--------|-------|-----------|------------------|---------------------|
| **Structural steel** | Iron + Carbon | 97:3 | Wide gaussian | +hardness, +stability over base iron | Carbon steel |
| **Bronze** | Copper + Tin | 88:12 | Plateau | +hardness, +stability over base copper | Tin bronze |
| **Light alloy** | Aluminum + Silicon | 90:10 | Gaussian | +hardness, maintains low density | Cast aluminum alloy |
| **Hull plate** | Titanium + Aluminum | 90:10 | Ridge | +hardness, +stability, low density | Ti-6Al (simplified) |
| **Heat resistant** | Nickel + Chromium | 80:20 | Plateau | +stability, +conductivity, high-temp tolerance | Nichrome |
| **Conductor wire** | Copper + Silver | 95:5 | Narrow gaussian | +stability over pure copper, retains conductivity | Sterling conductor |
| **Basic fuel** | Lithium + Carbon | 70:30 | Wide gaussian | +radiance — the starter fuel compound | Energetic compound |

These seven recipes aren't secrets — they're published in the standard physics script. The founding cluster doesn't gatekeep them. They're the floor, not the ceiling. The first thing every researcher does is try to beat them.

### What's NOT published

Everything else. The stoichiometric peaks for every other element pair. Multi-element combinations. High-energy phases. Catalyst effects. The founding cluster doesn't know most of this — they've explored the gentle regions and published what they found. The vast majority of the interaction landscape is unmapped.

Specific unknowns that players discover:

- **Better steel.** Iron + Carbon + Chromium at the right ternary ratio. Real-world stainless steel, but the exact Apeiron ratio and energy level are seed-dependent.
- **Superalloys.** Nickel + Chromium + Cobalt + Titanium. Four-element combination with interference effects. Requires cobalt as both input AND catalyst.
- **Advanced fuel.** Lithium + something unexpected at high energy. The founding cluster's basic fuel recipe is intentionally mediocre — better fuel recipes are the first major research prize.
- **Tungsten composites.** Tungsten + Carbon at narrow peak ratios. Extreme hardness, extreme density — armor and tooling.
- **The iridium peaks.** Whatever happens when iridium is present as a catalyst during high-energy transforms. Nobody in the founding cluster has enough iridium to test systematically. The peaks exist in the physics script. They're waiting.

## Property Design Rationale

Each property axis creates distinct gameplay tradeoffs:

**Density** creates the weight-vs-strength decision. Low density (aluminum, lithium, carbon) means lighter ships and structures — cheaper to accelerate, less fuel to move. High density (tungsten, iridium, iron) means more mass per volume — better for armor, ballast, kinetic projectiles, and anything where mass is an advantage. You can't have light AND dense. Every structural choice trades one for the other.

**Hardness** is the obvious armor/tooling property, but it trades against workability. Very hard materials (tungsten, chromium, carbon) are harder to machine — higher precision facility requirements. This is implicit in the interaction function: narrow peaks for extremely hard materials, wide peaks for moderate ones.

**Conductivity** spans 0.03 (tin) to 0.98 (silver). The gap matters. Copper at 0.92 is "good enough" and common. Silver at 0.98 is marginally better but scarce. The gameplay question: is 6% more conductivity worth the scarcity premium? For most applications, no. For the highest-performance systems where resonance peaks reward conductivity near 1.0, yes.

**Reactivity** is double-edged. High reactivity (lithium 0.95, carbon 0.75) means the element bonds readily — good for alloys, bad for material longevity. Low reactivity (iridium 0.03, platinum 0.05) means it resists combination — which is exactly why those elements make good catalysts. They participate without being consumed.

**Stability** is corrosion resistance, environmental tolerance, longevity. The highest-stability elements (iridium 0.98, platinum 0.95, tungsten 0.92, titanium 0.90) are all scarce or rare. Common elements have moderate stability at best. This creates a natural progression: early-game materials degrade, late-game materials endure.

**Radiance** is the energy axis. Lithium (0.70) and iridium (0.60) are the standouts. High radiance means the element contributes to energy density in fuel compounds, emission in weapons, sensitivity in sensors. Lithium is the accessible energy element. Iridium is the endgame one. The gap between lithium fuel and iridium-catalyzed fuel is the progression spiral that TRANSFORMATION.md describes.

## Element Interactions at a Glance

Not exhaustive — just the intuition-level relationships that give the table its structure.

**Natural partners** (real-world alloy intuitions that the interaction function rewards):
- Iron + Carbon (steel family)
- Copper + Tin (bronze family)  
- Copper + Zinc (brass family)
- Titanium + Aluminum (aerospace alloys)
- Nickel + Chromium (heat-resistant alloys)
- Iron + Chromium + Nickel (stainless family)

**Natural catalysts** (low reactivity, high stability — they participate without being consumed):
- Platinum catalyzes broadly — lowers activation energy for many peaks across many pairs
- Cobalt catalyzes selectively — dramatic effect on iron-family and nickel-family peaks, little effect elsewhere
- Iridium catalyzes the extremes — only affects the highest-energy peaks, but the effect is enormous

**Natural fuel components** (high radiance, high reactivity):
- Lithium is the primary fuel element — highest radiance among accessible elements
- Carbon adds stability to fuel compounds — lithium alone is too unstable for practical fuel
- Silver adds radiance to non-fuel materials — useful for energy-emitting systems (weapons, sensors)

**Antagonistic pairs** (elements whose peaks tend toward destructive interference):
- Seed-dependent. The founding cluster may discover that zinc interferes with titanium alloys in their galaxy. Another galaxy's seed produces constructive interference for the same pair. This is the part that's NOT predictable from real-world intuition — and it's where Apeiron's physics diverges from reality.

## Future Expansion

The table starts at 16. The physics script and interaction function support expansion. Future updates add elements that "were always there" — the seed placed them, but earlier physics scripts couldn't evaluate them.

Expansion candidates (not committed, just the design space):
- **Vanadium** — would complete the Grade 5 titanium analogue (Ti-6Al-4V)
- **Molybdenum** — high-temperature structural element, fills a gap between chromium and tungsten
- **Beryllium** — extremely light, very hard, toxic-analogue (high facility requirements)
- **Rare earths** (as a group element) — magnetic and electronic applications
- **Helium-3** — fusion fuel analogue, extremely rare, extreme radiance

Each expansion changes the interaction landscape. New element pairs mean new stoichiometric peaks. Existing recipes don't break — they just get competition. "Element 17 was always in those asteroids. We finally know what to do with it."
