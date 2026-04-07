# Element Table
<!-- id: apeiron.elements --> <!-- status: proposed --> <!-- summary: The founding cluster's element definitions — property vectors, abundances, and starter recipes -->

Thirteen elements. Twelve natural, one synthetic. Three tiers plus manufactured. This is the founding cluster's standard physics table — content-addressed, deterministic, published to every domain.

## Why Real Names

I named these after real-world elements because it gives players an on-ramp. "Iron + carbon" producing something steel-like *feels right*. That's immersion you get for free. The interaction function is still seed-determined and opaque — two galaxies produce different optimal ratios for "steel." The names are handles for intuition, not constraints on physics.

What this buys: known alloys (steel, titanium alloys, tungsten carbide) work as natural starter recipes. Players bring real-world intuitions. Most of those intuitions are roughly correct for simple combinations — and completely wrong for exotic multi-element, high-energy phases where the physics diverges from reality entirely.

## Why Thirteen

Other games land on 8-12 base resources (EVE: 8 minerals, Factorio: 6, Space Engineers: 11, Minecraft: 11). The sweet spot is enough variety for meaningful choices without inventory tedium. Our complexity comes from the interaction function's combinatorial space — 78 element pairs, each with 3-5 stoichiometric peaks — not from a long element list.

Every element earns its slot by occupying a unique region in the 6-property space. No two are interchangeable. I conflated similar elements aggressively: nickel, cobalt, and chromium are similar enough that chromium alone covers the "trace hardening modifier" role. Tin and zinc are both "soft alloying metal" — cut both. If playtesting reveals a gap, the expansion mechanism is built in.

## The Elements

Properties on a 0.00–1.00 scale. These are *base element properties* — the weighted-average floor when elements combine. The interaction function's stoichiometric peaks push material properties well beyond any single element's values.

### Common — Every star system, bulk quantities

| # | Element | Den | Hard | Cond | React | Stab | Rad | Role |
|---|---------|-----|------|------|-------|------|-----|------|
| 0 | **Iron** | 0.62 | 0.60 | 0.32 | 0.40 | 0.42 | 0.05 | Bulk structural. Dense, hard, moderate everything else. The backbone. |
| 1 | **Carbon** | 0.14 | 0.72 | 0.12 | 0.70 | 0.78 | 0.03 | Universal bonder. Hard, reactive, stable — the paradox element. Tiny additions transform other elements. |
| 2 | **Silicon** | 0.22 | 0.52 | 0.42 | 0.32 | 0.72 | 0.06 | Semiconductor. Moderate everything with good stability. Electronics, sensors, ceramics. |
| 3 | **Copper** | 0.56 | 0.22 | 0.88 | 0.28 | 0.48 | 0.04 | The conductor. Soft and heavy, but nothing else comes close in conductivity until gold. |
| 4 | **Aluminum** | 0.20 | 0.28 | 0.62 | 0.55 | 0.45 | 0.03 | Lightweight. Low density, decent conductor, reactive. The strength-to-weight enabler in alloys. |
| 5 | **Hydrogen** | 0.01 | 0.00 | 0.05 | 0.95 | 0.08 | 0.70 | Fuel base. Nearly zero everything except reactivity and radiance. The energy element — hydrogen compounds are the natural starting point for fuel. Dangerous in concentration. |

### Strategic — Variable distribution, two consumption patterns

Strategic elements split into two gameplay roles. **Bulk strategic** (titanium) — you need lots of it, you build structures from it. **Trace strategic** (chromium, tungsten, gold) — you need less of it, you add small amounts to alloys or use it for specialized applications.

| # | Element | Den | Hard | Cond | React | Stab | Rad | Role |
|---|---------|-----|------|------|-------|------|-----|------|
| 6 | **Titanium** | 0.35 | 0.82 | 0.14 | 0.15 | 0.88 | 0.04 | Aerospace structural. Hard, stable, light, poor conductor. Where you need strength without weight. Bulk strategic — you build hulls from this. |
| 7 | **Chromium** | 0.55 | 0.80 | 0.30 | 0.25 | 0.85 | 0.18 | Trace hardener. Very hard, very stable. Small additions to other elements dramatically boost hardness and corrosion resistance. The 18% that makes steel stainless. |
| 8 | **Tungsten** | 0.92 | 0.92 | 0.28 | 0.06 | 0.94 | 0.22 | Extreme metal. Hardest, densest, most stable natural element. Poor conductor, nearly inert. Armor, weapons, reactor containment, heat shields. |
| 9 | **Gold** | 0.82 | 0.15 | 0.85 | 0.04 | 0.78 | 0.15 | Inert conductor. Dense, soft, excellent conductor, nearly zero reactivity. The only element combining high conductivity with high stability and chemical inertness. Space electronics, connectors, radiation shielding. |

### Exotic — Few systems, high strategic value

| # | Element | Den | Hard | Cond | React | Stab | Rad | Role |
|---|---------|-----|------|------|-------|------|-----|------|
| 10 | **Uranium** | 0.88 | 0.38 | 0.10 | 0.38 | 0.22 | 0.92 | Nuclear fuel source. Dense, radioactive, unstable. Extreme radiance but low stability — the energy wants to escape. Valuable as fuel feedstock and as parent material for plutonium synthesis. |
| 11 | **Platinum** | 0.82 | 0.42 | 0.52 | 0.05 | 0.90 | 0.08 | The catalyst. Nearly inert, extremely stable, good conductor. Worth more as a catalyst than as material input — its presence during transforms lowers activation energy for stoichiometric peaks that would otherwise require vastly more fuel. Controlling platinum deposits is the single largest strategic advantage in the galaxy. |

### Synthetic — Not found naturally

| # | Element | Den | Hard | Cond | React | Stab | Rad | Role |
|---|---------|-----|------|------|-------|------|-----|------|
| S | **Plutonium** | 0.85 | 0.28 | 0.08 | 0.48 | 0.05 | 0.98 | Manufactured nuclear fuel. Zero natural deposits — must be synthesized from uranium via high-energy reactor transform. Extreme radiance, extreme instability. The highest energy density of any element, but must be contained. The progression: find uranium → build reactor → produce plutonium → access the best fuel compounds. |

Plutonium fits cleanly into the existing transformation system. It's just a transform whose output happens to be an element rather than a material. The physics script knows about plutonium from day one — the interaction function includes peaks for all plutonium pairs. But nobody has any until someone builds the reactor to make it.

## Abundance

| Tier | Elements | Galaxy mass share | System availability | Deposit scale |
|------|----------|-------------------|--------------------|----|
| Common | Fe, C, Si, Cu, Al, H | ~85% | 90-100% of systems | 100K–1M units |
| Strategic (bulk) | Ti | ~8% | 60-80% of systems | 10K–100K units |
| Strategic (trace) | Cr, W, Au | ~5% | 30-50% of systems | 1K–10K units |
| Exotic | U, Pt | ~2% | 5-15% of systems | 100–1K units |
| Synthetic | Pu | 0% | Manufactured only | — |

Total extractable mass per star varies ~10x from poorest to richest. But exotic element placement is independent of total richness — a resource-poor star might have platinum while a resource-rich one doesn't.

### What Scarcity Creates

Common elements create no trade pressure. Everyone has iron.

Strategic elements create the first trade routes. A titanium-rich system becomes an industrial hub. A system with chromium and tungsten becomes an alloy center. Gold deposits make a system valuable for electronics manufacturing. The consumption patterns are different: you burn through titanium by the ton building hulls, but a chromium deposit lasts because you only add 18% to steel batches.

Exotic elements create alliances and conflicts. Uranium deposits mean energy independence and plutonium synthesis capability. Platinum deposits mean catalyst access — categorically different research capability, not marginal improvement. A system with both uranium AND platinum is the most strategically valuable location in the galaxy.

Plutonium creates manufacturing depth. You can't shortcut the chain. Find uranium, build a reactor, synthesize plutonium, compound it into fuel. Each step requires the previous step's output. This is the only element that can't be found — it must be made.

## Starter Recipes

Published by the founding cluster. Known stoichiometric peaks, verified in the founding galaxy seed. Wide, forgiving peaks with low energy thresholds. Every new player can reproduce these on day one.

| Recipe | Inputs | Ratio | Peak | What it does |
|--------|--------|-------|------|-------------|
| **Structural Steel** | Iron + Carbon | 97:3 | Gaussian, wide | +hardness, +stability over base iron |
| **Chromium Steel** | Iron + Chromium | 82:18 | Plateau | +hardness, +stability, corrosion resistance |
| **Light Alloy** | Aluminum + Silicon | 90:10 | Gaussian | +hardness, maintains low density |
| **Hull Plate** | Titanium + Aluminum | 90:10 | Ridge | +hardness, +stability, low density |
| **Hydrocarbon Fuel** | Hydrogen + Carbon | 80:20 | Gaussian, wide | +radiance, moderate stability — baseline fuel |
| **Conductor Wire** | Copper + Gold | 95:5 | Narrow gaussian | +stability over pure copper, retains conductivity |
| **Silicon Carbide** | Silicon + Carbon | 70:30 | Plateau | Extreme hardness, low density — ceramic armor, abrasives |

Seven recipes. All recognizable real-world analogues. The founding cluster doesn't gatekeep these — they're the floor. The first thing every researcher does is try to beat them.

### What's Not Published

Everything else. 78 element pairs minus 7 published = 71 unmapped pairs. Plus all ternary, quaternary, and quinary combinations. Plus high-energy phases. Plus catalyst effects. Plus plutonium chemistry (nobody has any yet).

Specific unknowns that define the research game:

- **Stainless steel.** Iron + Carbon + Chromium at the right ternary ratio. Three-element research is significantly harder — the search space is 3D, not 2D.
- **Advanced fuel.** Hydrogen + something unexpected at high energy. The published hydrocarbon fuel is intentionally mediocre. Better fuel recipes are the first major research prize.
- **Tungsten carbide.** Tungsten + Carbon at a narrow peak ratio. Extreme hardness, extreme density. The weapons/armor material — but tungsten is scarce, so production is limited.
- **Gold electronics.** Gold + Silicon at specific ratios. What happens to conductivity when you combine the inert conductor with the semiconductor? The interaction function knows. Nobody's tested it.
- **Plutonium compounds.** Everything. When someone finally synthesizes plutonium and starts combining it with other elements, they're exploring 12 new element pairs that nobody in the galaxy has data on. Extreme radiance + other properties = unknown territory.
- **Platinum-catalyzed reactions.** What peaks unlock when platinum is present as a catalyst during high-energy transforms? The physics script includes these peaks. They're waiting for someone with platinum access and enough fuel to test systematically.

## Property Space Analysis

Each element occupies a unique region. No two are interchangeable:

| Unique signature | Element | Why nothing else fills this role |
|------------------|---------|----------------------------------|
| Extreme reactivity + radiance, zero mass | Hydrogen | Only element that's pure energy potential |
| High hardness + high reactivity + high stability | Carbon | The only element that bonds with everything yet remains stable |
| High hardness + high stability + low density | Titanium | The aerospace sweet spot — nothing else is this strong and this light |
| Max density + max hardness + max stability | Tungsten | Every structural extreme simultaneously |
| High conductivity + near-zero reactivity | Gold | The only inert conductor — copper conducts but corrodes |
| Extreme radiance + moderate reactivity + low stability | Uranium | Dense energy that wants to escape |
| Near-zero reactivity + extreme stability | Platinum | The catalyst signature — participates without being consumed |
| Extreme radiance + extreme instability | Plutonium | Maximum energy density, maximum danger |

### Gaps Only Alloys Can Fill

The table is deliberately incomplete. No single element provides:

- **High conductivity + high hardness.** Best conductor (copper) is soft. Hardest elements (tungsten, chromium) are poor conductors. An alloy that achieves both is extremely valuable for high-performance electronics.
- **High radiance + high stability.** Uranium has radiance but low stability. Tungsten has stability but moderate radiance. A stable high-energy material requires the right combination at the right energy level.
- **Low density + high hardness + high conductivity.** Nothing natural is light, hard, AND conductive. This is the holy grail material — lightweight structural electronics.
- **High reactivity + high stability.** A material that bonds readily but doesn't degrade. Contradiction in element terms. Achievable only through specific stoichiometric peaks.

These gaps are the discovery game. The interaction function contains peaks in these impossible regions. Finding them requires exploration.

## Catalyst Mechanics

Three elements are natural catalyst candidates — low reactivity, high stability, not consumed during transforms:

| Catalyst | Availability | Breadth | Strength | What it unlocks |
|----------|-------------|---------|----------|-----------------|
| **Platinum** | Exotic (~10% of systems) | Broad — affects peaks across many element pairs | Strong — 40-60% activation energy reduction | Mid-to-high energy peaks. The general-purpose catalyst. Makes expensive reactions affordable. |
| **Gold** | Strategic (~40% of systems) | Narrow — affects peaks for a few specific pairs | Moderate — 20-30% activation energy reduction | Specific low-to-mid energy peaks. More accessible than platinum, less powerful. |
| **Tungsten** | Strategic (~40% of systems) | Very narrow — only affects highest-energy peaks | Variable — seed-dependent, sometimes dramatic | Extreme-energy peaks only. Useless for normal chemistry, critical for endgame materials. |

Catalyst effects are seed-determined. In one galaxy, platinum might dramatically catalyze iron-chromium peaks. In another, it might favor titanium-aluminum. The mapping is discoverable only through experimentation.

## Future Expansion

The table starts at 13. The physics script supports expansion. Future updates add elements that "were always there" — the seed placed them, the physics script evaluates them, but earlier scripts didn't include them in the published table.

Expansion candidates (not committed):

- **Lithium** — ultra-light, extremely reactive, high radiance. An alternative fuel pathway.
- **Nickel** — the alloy stabilizer. Moderate everything, shines in combination with chromium and iron.
- **Silver** — superconductor. Even better than copper, but scarce and soft.
- **Cobalt** — magnetic catalyst. Selective catalyst for iron-family peaks.
- **Helium-3** — fusion fuel. Near-zero everything except extreme radiance.

Each expansion adds new element pairs to the interaction landscape without breaking existing recipes. "Element 14 was always in those asteroids. We finally know what to do with it."
