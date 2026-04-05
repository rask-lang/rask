# Constraint Physics
<!-- id: apeiron.physics --> <!-- status: proposed --> <!-- summary: General physical laws that constrain what can exist in the galaxy -->

The [natural laws](README.md#physics-and-natural-laws) in the main spec (distance costs, entropy, scarcity) are consequences of constraint physics applied to travel and economy. This spec defines the physics underneath — general rules about how matter, energy, and structure work. These aren't game rules. They're the universe's physics. Everything else — ship design, station architecture, weapon systems, construction methods — emerges from their interaction.

The design goal: simple laws, emergent complexity. No material tables, no predefined ship classes, no recipe books. Those can emerge from player communities and founding cluster conventions. The physics just says what's possible.

## Principles

**General over specific.** "Structural load scales with volume" is a law. "Ships can't be bigger than 500m" is a rule. Laws compose. Rules accumulate.

**Interact to constrain.** No single law should feel limiting on its own. The constraints emerge from interactions. Mass alone doesn't prevent giant ships. Energy alone doesn't. Structure alone doesn't. All three together create a natural ceiling that nobody prescribed.

**Verifiable, not enforced.** Same principle as the existing natural laws. The standard physics script encodes these laws as formulas. Departure proofs include the results. Non-standard physics is visible, not banned. Domains that ignore structural scaling are transparent to every trading partner.

**Tunable constants, fixed relationships.** The laws define how properties relate. The founding cluster publishes constants (gravity scaling factor, structural efficiency, energy density). Constants can evolve. Relationships don't.

## Object Structure

Objects are compositions. A ship isn't a type with stats — it's a tree of components. Components are built from components or raw materials. The whole inherits properties from its parts.

An object is a tree of components. Properties propagate upward:

- **Mass:** sum of all component masses
- **Volume:** sum of all component volumes (plus packing overhead)
- **Power draw:** sum of all system power draws
- **Power supply:** sum of all source outputs
- **Structural requirement:** function of total volume (Law 2)

This is the data model the constraint laws evaluate. Change the engine, the thrust-to-weight ratio changes. Add more cargo bays, volume increases, structural needs increase, mass increases, agility decreases. Every modification ripples through the physics.

An assembled object (a ship) is one Allgard Object. Its `content` encodes the component tree — what it's made of. When a ship arrives at a new domain, the receiving domain re-derives mass, structural integrity, energy budget from the component tree. If the numbers don't match the claimed properties, trust flag.

What the components ARE is game content, not physics. "Engine" isn't a law. The physics says "something in this composition provides thrust, and thrust requires energy, and the energy source has mass." What players call that thing, how it looks, what tech tree it belongs to — convention, not physics.

## The Five Laws

Five constraint laws govern what can physically exist. No single law is very restrictive. The constraints emerge from their interactions.

## Law 1: Conservation of Mass-Energy

Everything is made of something. Every object has mass. Mass comes from composition — the stuff it's built from. You can't have properties without the matter that provides them.

Not "objects have a mass property that someone fills in." Objects have mass BECAUSE they're made of materials, and materials have mass. The mass is derived, not declared.

**The rule:** An object's mass is the sum of its components' mass. No exceptions. No "this ship weighs 10 tons because I said so." It weighs what its materials weigh.

**What this creates:** A natural floor on how light anything useful can be. Want more cargo capacity? Need more hull. More hull means more mass. More mass means more fuel. You can't cheat this loop — you can only find better materials (lighter, stronger) or accept the tradeoff.

**Interaction with natural laws:** Mass drives fuel cost (distance costs) and trade economics (the trader's dilemma). This law says mass isn't arbitrary — it's derived from physical composition. The fuel cost formula has teeth because mass can't be gamed.

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

## Law 4: Stress and Failure

Objects under stress degrade. Stress comes from exceeding design parameters — overloading cargo, running systems beyond rated capacity, taking damage, operating in hostile environments.

**The rule:** Every component has a stress tolerance. Exceeding it accelerates decay (amplifies the existing entropy law). Operating at the edge is possible but costly. Operating beyond limits causes component failure.

```
effective_decay = base_decay * stress_multiplier(load / tolerance)
```

Where the stress multiplier is 1.0 at normal load, rises gradually as load approaches tolerance, and spikes sharply above it. The exact curve is a tunable — founding cluster publishes a standard one.

**What this creates:** Meaningful risk and engineering margin. A captain who overloads their cargo hold can do it — but the hull degrades faster, and if they push too far, structural failure. This isn't a hard wall. It's a pressure gradient. Safe operation is cheap. Risky operation is expensive. Reckless operation is catastrophic.

**Interaction with entropy:** The natural law says things decay. This law says the rate isn't constant — it responds to how hard you push. A well-maintained ship running within limits lasts a long time. An overloaded hauler cutting corners burns through hull integrity. Same ship, different choices, different outcomes.

## Law 5: Proximity Coupling

Systems in physical proximity exchange energy whether you want them to or not. Heat radiates. Vibration propagates through structure. Electromagnetic fields leak. This isn't an engineering problem to be solved — it's physics. Managing unwanted coupling requires material (insulation, shielding, damping), and that material has mass and volume.

**The rule:** Every pair of systems that share physical proximity has a coupling cost. The cost depends on the pair — how much unwanted energy they exchange. Managing that exchange requires interface material with real mass and volume.

Coupling has two components:

**Unwanted coupling (interference).** A reactor radiates heat. Weapons generate EM pulses. Engines vibrate. Cryo storage must stay cold. These are proximity effects — they happen because systems share the same structure. Mitigating them requires physical material: thermal insulation, EM shielding, vibration damping, radiation barriers. Each interface pair has a coupling intensity based on what the two systems emit and what they're sensitive to.

**Wanted coupling (routing).** Power must travel from reactor to systems via conduits. Coolant must circulate via pipes. Data must flow via lines. Fuel must reach engines. Every connection between systems is a physical conduit with mass, volume, and routing distance. More systems means more routing.

**What this creates:** The interface count between n systems scales as `n*(n-1)/2`. But the cost isn't uniform — it depends on WHAT you're combining. An engine next to a fuel tank is cheap (short fuel line, compatible thermal profile). A reactor next to a medical bay is expensive (heavy radiation shielding). A weapons array next to sensitive sensors is expensive (EM isolation). Some pairs are nearly free. Others dominate the mass budget.

This means specialization emerges from physics, not from a rule. A ship with three compatible systems (engine + fuel + cargo) has cheap interfaces. A ship with fifteen diverse systems has hundreds of interface pairs, many of them expensive. The penalty isn't abstract "complexity overhead" — it's the actual mass of shielding, insulation, conduits, and damping that holds a diverse system together.

**Why this is better than a formula:** There's no single equation. The coupling cost depends on what's next to what. Players who design clever layouts — putting compatible systems adjacent, routing carefully, isolating hostile pairs — build better ships than players who stuff everything in. Ship design becomes spatial problem-solving, not plugging numbers into a formula.

**Interaction with other laws:** Interface material has mass (Law 1). It occupies volume inside the structure (Law 2 — more volume means more structural support). Shielding and active isolation draw power (Law 3). Interface components can fail under stress (Law 4). Every law touches every other. The coupling cost feeds back into the same spirals that constrain everything else.

**What the physics script evaluates:** Given a component tree with spatial layout, compute pairwise coupling costs between adjacent systems. Sum the interface material mass and volume. Verify that shielding meets minimum requirements for each pair. A ship that puts a reactor next to unshielded crew quarters is physically invalid — the radiation flux exceeds survivable limits. Not because a rule says "don't do that" but because the physics says "that crew is dead."

## How The Laws Interact

None of these laws is individually very restrictive. Their power comes from interaction:

| Want this | L1: Mass-Energy | L2: Structural Scaling | L3: Energy Budget | L4: Stress | L5: Proximity Coupling |
|-----------|----------------|----------------------|------------------|-----------|---------------------|
| Bigger ship | More material mass | Superlinear structural cost | More energy for systems | More stress on structure | More internal interfaces to manage |
| More weapons | — | — | More power → bigger reactor → more mass | Higher operational stress | EM/heat shielding against adjacent systems |
| Longer range | More fuel mass | — | Fuel has volume → structure cost | — | Fuel routing to engines has mass |
| More cargo | More hull mass | More volume → more structure | — | Risk of overload | — |
| Do everything | All of the above | All of the above | All of the above | All of the above | Hundreds of expensive interface pairs (reactor↔medical, weapons↔sensors) |

The "10 million km ship" fails not because of one law but because all five compound: unimaginable material mass (L1), superlinear structural cost (L2), reactor mass to power it (L3), extreme stress tolerances needed (L4), and astronomical shielding/routing mass from millions of system interfaces (L5). Each law alone might be surmountable. Together, they create a wall that scales with ambition.

## What About Stations and Structures?

Same laws apply. A space station is a composition of components with mass, volume, structural requirements, and an energy budget. The difference: stations don't need to move. No fuel cost, no thrust-to-weight ratio. This is why stations can be much larger than ships — they only fight the structural scaling law, not the mass-fuel spiral.

But stations still face structural scaling (Law 2), energy budgets (Law 3), and proximity coupling (Law 5). A station the size of a moon is possible — but the structural mass is enormous, the power requirements are vast, and the shielding between thousands of diverse systems is a civilization-level engineering effort. The tiers emerge.

Outposts are small with few systems — coupling costs are minimal. Stations are bigger with more diverse systems — reactor shielding, life support isolation, docking bay EM management. Megastructures are theoretically possible but the interface mass alone rivals the structural mass. Nobody prescribes the tiers. The physics creates them.

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

The constraint laws aren't enforced by any authority. They're verified by every trading partner.

One Raido script — the **standard physics script** — evaluates all five constraint laws plus the natural laws (fuel costs, decay rates) in a single pass. The founding cluster publishes it. It's content-addressed. Any domain can fetch, inspect, and execute it independently.

**On minting:** A domain mints an object (ship, station, component). The physics script evaluates the component tree — derives mass, checks structural requirements, verifies energy budget, evaluates stress tolerances, computes coupling costs. The evaluation result and script hash go into the minting proof.

**On transfer:** The receiving domain re-runs the same script against the component tree in the object's content. If derived properties match claimed properties, the object is physically valid. If they don't — trust flag. The script hash in the departure proof tells the receiver exactly which physics were applied. Fetch the script, re-execute, verify independently.

**On mutation:** Any transform that changes an object's component tree triggers re-evaluation. Adding cargo, swapping an engine, installing weapons — the physics script runs again. The domain includes the new evaluation in the mutation proof.

No domain can force another to run the script. But every domain that trades will verify inbound objects, because accepting physically impossible objects means your own proofs become suspect to YOUR trading partners. Verification propagates through self-interest, not authority.

A domain running non-standard physics (different constants, missing laws, no evaluation) isn't banned. It's transparent — other domains see the non-standard script hash and decide how much trust to extend.

## Constants and Tuning

The laws define relationships. The founding cluster publishes constants:

| Constant | What it controls | Tuning direction |
|----------|-----------------|------------------|
| Structural exponent (`e`) | How fast structural needs grow with size | Higher = smaller ships. Lower = bigger ships. |
| Structural coefficient (`k`) | Base structural cost per volume | Higher = heavier everything. Lower = lighter. |
| Energy density | Power output per unit mass of reactor | Higher = more capable at same mass. Lower = heavier for same capability. |
| Coupling intensity table | Base interference between system type pairs | Higher values = more shielding needed. Determines which combinations are expensive. |
| Stress curve | How fast decay accelerates under load | Steeper = more punishing. Flatter = more forgiving. |

These are knobs, not laws. The founding cluster sets initial values through playtesting. They can publish updated constants (new script hash, voluntary adoption). The laws — the relationships themselves — don't change.

**Critical constraint:** Constants must be published and content-addressed. No secret physics. If the founding cluster changes the structural exponent, every domain can see the old and new scripts, evaluate their objects against both, and decide when to adopt. No forced migrations. No surprise invalidation.

## What This Doesn't Cover

**Materials.** The physics says components have mass, volume, structural efficiency, and energy properties. It doesn't say what materials exist. Steel, carbon fiber, alien alloys — those are game content that the founding cluster and player communities define. The physics just requires that whatever you call your material, it has consistent physical properties that other domains can verify.

**Technology.** The physics doesn't define tech trees, research mechanics, or progression. A "warp drive" is a component that provides thrust with certain mass/energy/volume properties. Whether it requires rare materials, research time, or faction reputation to build — that's game design, not physics.

**Combat.** The physics defines that weapons have mass and power draw, and that damage creates stress. How combat actually works (turn-based, real-time, deterministic) is a separate system. The physics just ensures that whatever combat system exists, the ships involved have physically consistent properties.

**Economy.** Allgard's conservation laws handle economic integrity. This spec handles physical integrity. They compose — an object must be both economically valid (proper minting, balanced exchange) and physically valid (mass checks out, structure sufficient, energy budget balanced).
