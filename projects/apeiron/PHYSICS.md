# Constraint Physics
<!-- id: apeiron.physics --> <!-- status: proposed --> <!-- summary: General physical laws that constrain what can exist in the galaxy -->

The natural laws in the main spec (distance costs, mass, entropy, scarcity) create economic friction. Constraint physics goes deeper — general rules about how matter, energy, and structure work. These aren't game rules. They're the universe's physics. Everything else — ship design, station architecture, weapon systems, construction methods — emerges from their interaction.

The design goal: simple laws, emergent complexity. No material tables, no predefined ship classes, no recipe books. Those can emerge from player communities and founding cluster conventions. The physics just says what's possible.

## Principles

**General over specific.** "Structural load scales with volume" is a law. "Ships can't be bigger than 500m" is a rule. Laws compose. Rules accumulate.

**Interact to constrain.** No single law should feel limiting on its own. The constraints emerge from interactions. Mass alone doesn't prevent giant ships. Energy alone doesn't. Structure alone doesn't. All three together create a natural ceiling that nobody prescribed.

**Verifiable, not enforced.** Same principle as the existing natural laws. The standard physics script encodes these laws as formulas. Departure proofs include the results. Non-standard physics is visible, not banned. Domains that ignore structural scaling are transparent to every trading partner.

**Tunable constants, fixed relationships.** The laws define how properties relate. The founding cluster publishes constants (gravity scaling factor, structural efficiency, energy density). Constants can evolve. Relationships don't.

## Law 1: Conservation of Mass-Energy

Everything is made of something. Every object has mass. Mass comes from composition — the stuff it's built from. You can't have properties without the matter that provides them.

This is the foundation. Not "objects have a mass property that someone fills in." Objects have mass BECAUSE they're made of materials, and materials have mass. The mass is derived, not declared.

**The rule:** An object's mass is the sum of its components' mass. No exceptions. No "this ship weighs 10 tons because I said so." It weighs what its materials weigh.

**What this creates:** A natural floor on how light anything useful can be. Want more cargo capacity? Need more hull. More hull means more mass. More mass means more fuel. You can't cheat this loop — you can only find better materials (lighter, stronger) or accept the tradeoff.

**Interaction with existing laws:** Mass already affects fuel cost (Natural Law 2). This law says mass isn't arbitrary — it's derived from physical composition. The fuel cost formula now has teeth because mass can't be gamed.

## Law 2: Structural Scaling

Bigger things need proportionally more structure to hold together. This is the square-cube law — surface area grows as the square, volume grows as the cube. A structure twice as wide needs more than twice the structural support.

**The rule:** Structural mass required scales superlinearly with enclosed volume. The exact relationship:

```
structural_mass = k * volume^e
```

Where `k` is a material-dependent structural efficiency constant and `e > 1` (the scaling exponent, probably around 1.2–1.4 — tunable, decided through playtesting). The key property: `e > 1` means every doubling in size costs MORE than double the structure.

**What this creates:** Natural size tiers. Small ships are efficient — structure is a small fraction of total mass. As ships get bigger, structural mass dominates. There's a practical ceiling where adding more volume costs so much structural mass that you can't carry useful payload. Nobody sets this ceiling. It emerges from the exponent.

A 10 million km ship isn't forbidden. It just needs galaxy-scale structural mass, which needs galaxy-scale fuel, which doesn't exist. The math says no without anyone writing a rule.

**What this doesn't do:** Prescribe HOW structure works. Structural mass is abstract — it could be steel beams, carbon nanotubes, force fields, alien bone. The law constrains the relationship between size and structural cost. The materials that fill that cost are game content that can evolve.

## Law 3: Energy Budget

Everything that does something needs energy. Engines, shields, weapons, life support, sensors, communications — every system draws from an energy budget. Energy comes from sources (fuel, reactors, solar) which have mass and occupy volume.

**The rule:** Every functional system has a power draw. Every energy source has a power output and a mass. Total draw cannot exceed total supply. Total supply adds mass (and volume) to the object.

```
sum(system.power_draw) <= sum(source.power_output)
total_mass += sum(source.mass)
```

**What this creates:** The capability-mass spiral. Want more weapons? Need more power. More power means more reactor mass. More mass means more fuel. More fuel means more mass. Every capability has weight. Literally.

This is why ships specialize. A ship that's good at everything is too heavy to move. A light scout sacrifices weapons for speed. A heavy warship sacrifices range for firepower. Nobody mandates ship classes — they emerge because you can't have everything at finite mass.

**Interaction with structural scaling:** Energy sources occupy volume. Volume requires structure. Structure has mass. Mass requires more energy to move. The two spirals compound — big ships with big reactors need big structures to hold the reactors, which need more energy to move.

## Law 4: Composition

Objects are built from components. Components are built from components or raw materials. The whole inherits properties from its parts. Mass is additive. Volume is additive. Capabilities come from what's inside.

**The rule:** An object is a tree of components. Properties propagate upward:

- **Mass:** sum of all component masses
- **Volume:** sum of all component volumes (plus packing overhead)
- **Power draw:** sum of all system power draws
- **Power supply:** sum of all source outputs
- **Structural requirement:** function of total volume (Law 2)

**What this creates:** Modular construction. A ship isn't a type with stats — it's a composition of hull, engines, reactor, cargo bays, weapons, shields, life support. Change the engine, change the ship's thrust-to-weight ratio. Add more cargo bays, increase volume, increase structural needs, increase mass, decrease agility. Every modification ripples through the physics.

**How this relates to Allgard objects:** An assembled object (a ship) is one Allgard Object. Its `content` encodes the component tree — what it's made of. The component tree is what the physics script evaluates. When a ship arrives at a new domain, the receiving domain can re-derive mass, structural integrity, energy budget from the component tree. If the numbers don't match the claimed properties, trust flag.

**What this doesn't prescribe:** What the components ARE. "Engine" isn't a law — it's game content. The physics says "something in this composition provides thrust, and thrust requires energy, and the energy source has mass." What players call that thing, how it looks, what tech tree it belongs to — that's all convention, not physics.

## Law 5: Stress and Failure

Objects under stress degrade. Stress comes from exceeding design parameters — overloading cargo, running systems beyond rated capacity, taking damage, operating in hostile environments.

**The rule:** Every component has a stress tolerance. Exceeding it accelerates decay (amplifies the existing entropy law). Operating at the edge is possible but costly. Operating beyond limits causes component failure.

```
effective_decay = base_decay * stress_multiplier(load / tolerance)
```

Where the stress multiplier is 1.0 at normal load, rises gradually as load approaches tolerance, and spikes sharply above it. The exact curve is a tunable — founding cluster publishes a standard one.

**What this creates:** Meaningful risk and engineering margin. A captain who overloads their cargo hold can do it — but the hull degrades faster, and if they push too far, structural failure. This isn't a hard wall. It's a pressure gradient. Safe operation is cheap. Risky operation is expensive. Reckless operation is catastrophic.

**Interaction with entropy:** The existing entropy law says things decay. This law says the rate isn't constant — it responds to how hard you push. A well-maintained ship running within limits lasts a long time. An overloaded hauler cutting corners burns through hull integrity. Same ship, different choices, different outcomes.

## Law 6: Conservation of Complexity

Building complex things requires proportionally more effort than simple things. This isn't just "more components = more mass" (that's Law 4). This is: integrating more systems has overhead beyond their individual costs.

**The rule:** Assembly complexity grows with the number of distinct systems. Each additional system type adds integration overhead — mass, energy, volume — beyond what the system itself costs.

```
integration_overhead = c * n_systems * ln(n_systems)
```

Where `c` is a constant and `n_systems` is the count of distinct system types. The n*ln(n) relationship means: a few systems integrate cheaply. Many systems compound. A ship with 3 system types (engine, hull, cargo) has low overhead. A ship with 20 system types (engine, hull, cargo, weapons, shields, sensors, cloak, medical, hangar, mining, refinery, ...) has substantial integration cost.

**What this creates:** Specialization pressure. A jack-of-all-trades ship pays heavy integration overhead. A focused ship (pure hauler, pure fighter, pure miner) is lean. This isn't because we declared ship classes — it's because complexity has a cost. Player-designed "do everything" ships exist but they're expensive, fragile, and inefficient compared to specialists.

**Why n*ln(n) and not n²?** n² would make anything beyond 5-6 systems nearly impossible. n*ln(n) is gentler — it allows ambitious designs but taxes them. A capital ship with 15 systems is viable but expensive. One with 30 is technically possible but probably not worth it. The curve creates soft boundaries that players discover through experience.

## How The Laws Interact

None of these laws is individually very restrictive. Their power comes from interaction:

| Want this | Law 1 says | Law 2 says | Law 3 says | Law 4 says | Law 5 says | Law 6 says |
|-----------|-----------|-----------|-----------|-----------|-----------|-----------|
| Bigger ship | More material mass | More structural mass (superlinear) | More energy for systems | More components | More stress on structure | — |
| More weapons | — | — | More power draw → bigger reactor → more mass | More components → more volume → more structure | Higher operational stress | More system types → integration overhead |
| Longer range | More fuel mass | — | Fuel has volume → structure cost | — | — | — |
| More cargo | More hull mass | More volume → more structure | — | Bigger bays, more mass | Risk of overload | — |
| Do everything | All of the above | All of the above | All of the above | All of the above | All of the above | Integration overhead on top |

The "10 million km ship" fails not because of one law but because all six compound: unimaginable material mass (L1), superlinear structural cost (L2), reactor mass to power it (L3), millions of integrated components (L4), extreme stress tolerances needed (L5), integration overhead for all those systems (L6). Each law alone might be surmountable. Together, they create a wall that scales with ambition.

## What About Stations and Structures?

Same laws apply. A space station is a composition of components with mass, volume, structural requirements, and an energy budget. The difference: stations don't need to move. No fuel cost, no thrust-to-weight ratio. This is why stations can be much larger than ships — they only fight the structural scaling law, not the mass-fuel spiral.

But stations still face structural scaling (Law 2), energy budgets (Law 3), and complexity overhead (Law 6). A station the size of a moon is possible — but the structural mass is enormous, the power requirements are vast, and maintaining it is a civilization-level effort. Again, the physics creates natural tiers without prescribing them.

Outposts are small, simple (few systems, low complexity overhead), and cheap. Stations are bigger, moderate complexity. Megastructures are theoretically possible but require economic empires. The tiers emerge.

## What About Spaceport Design? How Things Look?

Physics constrains what can EXIST. GDL describes what things LOOK LIKE. These are separate.

A ship's component tree determines its physical properties (mass, volume, capability). The ship's GDL appearance determines its visual representation. They're linked but not identical — a ship with two engines could look like anything. The physics says "two thrust units of mass X." GDL says "two nacelles on swept-back pylons."

**How others see your ship:** GDL appearance (content-addressed assets). Your ship's visual model travels with it. Every client renders it. Text clients describe it. 2D clients sprite it. 3D clients model it.

**How others verify your ship:** Physics script re-evaluation. Your ship arrives at a new domain. The domain reads the component tree from the object's content. Runs the standard physics script. Checks that claimed mass matches derived mass, that structural requirements are met, that energy budget balances. If something doesn't add up, the ship is flagged.

The visual can be anything. The physics must check out. A ship that looks like a tiny fighter but has capital-ship stats is physically inconsistent — the component tree that produces those stats has a volume and mass that don't match a tiny hull. Domains notice.

## Blueprints and Design

This spec deliberately doesn't define a blueprint system, crafting recipes, or construction mechanics. Those are game content — conventions that emerge from the physics. The founding cluster will publish starter blueprints (standard ship designs as component trees). Player communities will design new ones. Faction engineers will optimize. That's all game-layer activity, not physics.

What the physics provides is the evaluation function: given a component tree, derive all physical properties. Any proposed design can be evaluated. "Will this ship fly? How much fuel does it need? Can the structure hold? Does the power budget balance?" These are physics questions with deterministic answers. The standard physics script answers them. Anyone can check.

## Verification

Same model as existing natural laws. The standard physics script is content-addressed Raido bytecode. Domains that evaluate objects against it include the script hash in their proofs. Trading partners verify. Non-standard physics is visible.

A domain that mints a ship whose component tree doesn't produce valid physics (negative mass, structure below requirement, energy budget in deficit) is minting physical impossibilities. Any receiving domain can re-run the script and see the violation. Trust collapses.

A domain that uses different constants (lower structural scaling exponent, higher energy density) is running non-standard physics. Not broken — but transparent. Other domains decide how much trust to extend. A domain where ships are suspiciously light for their capability will face questions.

## Constants and Tuning

The laws define relationships. The founding cluster publishes constants:

| Constant | What it controls | Tuning direction |
|----------|-----------------|------------------|
| Structural exponent (`e`) | How fast structural needs grow with size | Higher = smaller ships. Lower = bigger ships. |
| Structural coefficient (`k`) | Base structural cost per volume | Higher = heavier everything. Lower = lighter. |
| Energy density | Power output per unit mass of reactor | Higher = more capable at same mass. Lower = heavier for same capability. |
| Integration coefficient (`c`) | Complexity overhead per system type | Higher = more specialist. Lower = more generalist. |
| Stress curve | How fast decay accelerates under load | Steeper = more punishing. Flatter = more forgiving. |

These are knobs, not laws. The founding cluster sets initial values through playtesting. They can publish updated constants (new script hash, voluntary adoption). The laws — the relationships themselves — don't change.

**Critical constraint:** Constants must be published and content-addressed. No secret physics. If the founding cluster changes the structural exponent, every domain can see the old and new scripts, evaluate their objects against both, and decide when to adopt. No forced migrations. No surprise invalidation.

## What This Doesn't Cover

**Materials.** The physics says components have mass, volume, structural efficiency, and energy properties. It doesn't say what materials exist. Steel, carbon fiber, alien alloys — those are game content that the founding cluster and player communities define. The physics just requires that whatever you call your material, it has consistent physical properties that other domains can verify.

**Technology.** The physics doesn't define tech trees, research mechanics, or progression. A "warp drive" is a component that provides thrust with certain mass/energy/volume properties. Whether it requires rare materials, research time, or faction reputation to build — that's game design, not physics.

**Combat.** The physics defines that weapons have mass and power draw, and that damage creates stress. How combat actually works (turn-based, real-time, deterministic) is a separate system. The physics just ensures that whatever combat system exists, the ships involved have physically consistent properties.

**Economy.** Allgard's conservation laws handle economic integrity. This spec handles physical integrity. They compose — an object must be both economically valid (proper minting, balanced exchange) and physically valid (mass checks out, structure sufficient, energy budget balanced).
