# Salvage
<!-- id: apeiron.salvage --> <!-- status: proposed --> <!-- summary: What happens to destroyed mass — debris fields, salvage mechanics, recycling -->

The [combat model](COMBAT.md) says "destroyed objects are gone." That's true for the object — it stops being a ship, a station, a component. But the mass doesn't vanish. Conservation Law 1 (can't create value from nothing) has a corollary: can't destroy value into nothing. Destruction is transformation, not annihilation.

This spec defines what remains after destruction and how it re-enters the economy. The destruction model here extends the combat script ([COMBAT.md](COMBAT.md)) — the combat script determines WHEN objects are destroyed and the total energy of the destruction event. This spec determines WHAT survives. Both run as deterministic Raido bytecode, verifiable by any party.

## The Problem

Without salvage, combat is a pure sink. Every fight deletes mass from the galaxy permanently. That's too aggressive — it makes war economically irrational in almost all cases, which sounds realistic but makes for bad gameplay. Real wars happen because combatants expect to gain something. If everything is incinerated on contact, the calculus never works.

Salvage also closes a physics hole. Law 1 says mass is derived from composition. If a 500-ton ship is "destroyed" and 500 tons of matter vanishes, the universe's mass budget is wrong. The interaction function and conservation laws are built on the premise that matter transforms but doesn't disappear. Destruction without salvage breaks that premise.

## Debris

When an object is destroyed (combat, stress failure, deliberate scuttling), it becomes a **debris field** — a new Allgard object hosted on the domain where the destruction happened.

### Destruction Energy Distribution

A destruction event has a total energy — the sum of all damage dealt in the final combat tick, or a fixed energy for scuttling (proportional to scuttling charge mass × energy density). This energy propagates through the component tree from the outside in.

**Propagation model.** The hull absorbs energy first. Energy that exceeds the hull's absorption capacity passes through to child components, distributed proportionally to their cross-section (volume^(2/3)):

```
hull_absorbed = min(event_energy, hull.tolerance)
penetrating_energy = event_energy - hull_absorbed
for child in hull.children:
    child_share = penetrating_energy * (child.volume^0.67 / total_children_cross_section)
    // recurse: child absorbs up to its tolerance, remainder passes to its children
```

The hull is the outermost component. Deep components (electronics inside a shielded compartment) receive less energy than exposed components (external weapons mounts). Shielding between components (Law 5 coupling mitigation) also absorbs energy before it reaches the shielded component — shielding mass protects during destruction, same as it protects during operation.

### Component Fate

Each component falls into one of three categories based on absorbed energy vs. its stress tolerance (Law 4):

| Condition | Result |
|-----------|--------|
| `absorbed < tolerance` | **Intact.** Survives whole with original properties |
| `tolerance ≤ absorbed < tolerance × catastrophic_factor` | **Damaged.** Survives with degraded properties |
| `absorbed ≥ tolerance × catastrophic_factor` | **Scrapped.** Reduced to constituent materials |

The `catastrophic_factor` is a constant in the physics script (probably 2.0-3.0 — meaning a component needs 2-3x its stress tolerance to completely disintegrate). Between tolerance and catastrophic, damage scales linearly:

```
damage_fraction = (absorbed - tolerance) / (tolerance * (catastrophic_factor - 1))
degraded_property = original_property * (1 - damage_fraction * max_degradation)
```

Where `max_degradation` (probably 0.6) is the worst a damaged component gets before it would be scrapped instead. A component at the boundary of the damaged range has ~60% reduced properties.

### Destruction Loss

Scrapped components don't convert 1:1 into materials. A fraction of their mass is lost — converted to heat, radiation, and fragments too fine to recover:

```
recovered_mass = scrapped_component.mass * (1 - destruction_loss_rate)
```

`destruction_loss_rate` is a constant in the physics script: **0.20** (20%). Not variable — the same rate whether the ship was destroyed by weapons fire or scuttled. This is the real economic sink from combat.

Total mass budget for a destruction event:
```
debris.mass = sum(intact.mass) + sum(damaged.mass) + sum(scrapped.mass * 0.80)
lost.mass = sum(scrapped.mass * 0.20)
// debris.mass + lost.mass = original.mass  (conservation)
```

### Debris Properties

A debris field is a passive object. It has:
- **Mass:** Sum of intact components + damaged components + scrap. Always less than the original object (destruction loss).
- **Location:** The domain where the destruction happened. Debris doesn't drift.
- **Composition manifest:** What's in the debris, derivable from the destruction script. Public — anyone who knows the destroyed object's component tree and the destruction event can compute what survived.
- **Decay timer:** Optional, domain policy. Some domains may despawn debris after N ticks to prevent clutter. Not a physics law — a governance choice. The founding cluster publishes a recommended timer (long enough for salvage, not permanent).

### Cargo Survives

A destroyed ship's cargo hold contents are separate objects. They don't transform — they just need a new host. The debris field contains them. Salvaging the debris transfers cargo to the salvager. This is important: destroying a hauler doesn't destroy its cargo. It makes the cargo available to whoever salvages the wreck.

This creates tactical decisions. Piracy becomes viable — destroy the escort, salvage the hauler's cargo. But also risky — the hauler's cargo might not be worth the fuel and ammo spent destroying the escort.

## Salvage Operations

Salvaging is a Transform. The salvager's domain (or the domain hosting the debris) runs the salvage script. Inputs: debris field object, salvager's equipment. Outputs: recovered components, materials, cargo.

### Equipment Matters

Salvage quality depends on the salvager's equipment — same principle as crafting facility precision:

**Basic tools.** Recover intact components and cargo. Damaged components recovered as-is (can't repair during salvage). Scrap recovered as raw material chunks — not sorted by element, just bulk mass. Cheap to equip, anyone can do it.

**Salvage rig.** Recover and sort scrap into separated materials. Damaged components can be stabilized (prevent further degradation, not repaired). Better material recovery rate from scrap. Requires specialized equipment — a ship fitted for salvage.

**Full recovery platform.** Repair damaged components during salvage (partial — not back to 100%, but better than degraded). Separate scrap into individual elements. Maximum material recovery. Expensive to build and operate — a dedicated salvage vessel or a station with recovery facilities.

Recovery rates (fraction of scrap mass recovered as usable material):

| Equipment | Recovery rate | Notes |
|-----------|--------------|-------|
| Basic tools | 50-60% | Bulk scrap, unsorted |
| Salvage rig | 70-80% | Sorted materials |
| Recovery platform | 85-95% | Element-level separation |

The remaining fraction is true loss — mass that's unrecoverable regardless of equipment. Combined with the destruction loss, total mass loss from destruction + salvage is roughly 30-50% of the original object. Significant but not total.

### Who Can Salvage

The debris is an Allgard object on a domain. The domain controls access.

**Combat domain policy.** The domain where combat happened decides salvage rights. Options:
- **Victor takes all.** Debris ownership transfers to the combat winner. Standard for PvP zones. The consent-on-entry grant specifies this.
- **Open salvage.** Debris is unclaimed. First to salvage takes it. Good for route domains that want to attract salvage operators.
- **Domain claims.** The domain operator claims all debris in their jurisdiction. They can sell salvage rights or operate their own recovery.

**Consent-on-entry covers salvage.** The PvP grant (see [COMBAT.md](COMBAT.md#consent-on-entry)) should specify salvage policy alongside combat rules. Players know before entering what happens to wrecks.

## Recycling

Salvaged materials re-enter the economy through normal crafting. Scrap steel goes back into a smelter. Recovered components go into new ships. The proof chain extends: original extraction → original craft → ship assembly → destruction → salvage → new craft. Every step traceable to the seed.

**Damaged components** are interesting. A cracked engine at 60% thrust is cheaper than building new, but worse. There's a market for damaged goods — budget builds, temporary repairs, emergency replacements. Some players will specialize in repairing damaged components (a Transform that consumes the damaged component plus repair materials, outputs a partially restored one).

**Element recovery** from scrap feeds back into material synthesis. Scrap from exotic alloys might be the cheapest source of rare elements for a system that can't mine them locally. A system near a contested combat zone might build its economy around recycling rather than mining.

## Economic Effects

### New Roles

**Salvager.** Non-combat player who follows fleet engagements and recovers debris. Needs a salvage-fitted ship (cargo space, recovery equipment) but not weapons. A vulture role — morally ambiguous, economically essential. Salvagers keep destroyed mass in circulation.

**Scrap dealer.** Buys unsorted scrap from salvagers, sorts and resells materials. Station-based. Arbitrage between salvage prices and material prices.

**Repair specialist.** Buys damaged components, repairs them, resells. Needs facility access and material science knowledge (repair Transforms are crafting operations — facility precision matters). A niche between salvage and manufacturing.

### Combat Economics Shift

Without salvage, every fight destroys 100% of the loser's mass. With salvage:
- The winner recovers 50-70% of the loser's mass (depending on equipment and destruction severity)
- Combat becomes net-profitable for the winner in many cases
- Piracy has a business model (destroy ship, salvage cargo + components)
- War of attrition has a recycling component (salvage your own losses between engagements)
- Defensive combat near your own facilities is advantageous (salvage your own debris immediately, deny it to the attacker)

### Salvage as Conflict Driver

Debris fields ARE the loot. A major battle leaves a field of recoverable mass. Multiple parties want it — the victor, neutral salvagers, opportunistic third parties. Salvage rights become a negotiation point in cease-fires. "We keep our debris, you keep yours" is a reasonable treaty clause.

Historically valuable debris — wreckage from a famous battle, components from a legendary ship — has provenance through its proof chain. Collector value on top of material value. The proof chain makes provenance unfakeable.

## Scuttling

A domain operator can deliberately destroy their own objects. Why:
- **Deny to enemy.** Scuttle your station rather than let it be captured. You lose the mass, but so does the attacker. Destruction loss applies — you're converting some mass to nothing to prevent all of it from being captured.
- **Downgrade.** Destroy a component to recover materials for rebuilding differently. Cheaper than trying to disassemble cleanly (disassembly is a separate, lossless operation if you have the right facilities — scuttling is the emergency version with destruction loss).
- **Clear space.** Remove obsolete equipment. The debris can be salvaged or despawned.

Scuttling produces debris using the same destruction script. No special case.

## Stage 1 Testing

The monolith can test salvage fully:
- Destroy a ship via combat script. Verify debris field is created with correct mass budget.
- Salvage debris with different equipment tiers. Verify recovery rates.
- Track mass through the full loop: extraction → craft → assembly → destruction → salvage → re-craft. Verify total mass is conserved minus documented losses.
- Test cargo survival: destroy a loaded hauler, verify cargo appears in debris.
- Test salvage economics: is salvage profitable? Does it create the right incentive for the salvager role?
- AI salvagers operating in the economy. Do they affect material prices? Do they keep destroyed mass in circulation?

## Interaction With Other Systems

**Conservation laws.** Debris creation is a Transform. Mass in = mass out + destruction loss. The loss is bounded by a constant in the standard physics script. Verifiable.

**Constraint physics.** Debris components retain their physics properties. A recovered engine still has mass, thrust, power draw. Damaged components have reduced properties but the same physics relationships.

**Combat.** The destruction script is part of the combat script (or called by it). Both are standard Raido bytecode, deterministic, verifiable. The same commit-reveal-verify flow that validates combat validates debris creation.

**Economy.** Salvage is a new source of materials and components, competing with extraction and manufacturing. In steady state, recycled materials should be cheaper than freshly extracted ones (lower energy cost) but lower quality (damage, mixing). The price difference is emergent, not prescribed.

## What This Spec Doesn't Cover

**Disassembly.** Non-destructive teardown of objects into components. This is a facility operation (Transform), not destruction. Should be lossless or near-lossless with the right equipment. Different from salvage — you're carefully taking apart something intact, not recovering wreckage. Needs its own spec or a section in TRANSFORMATION.md.

**Capture.** Taking intact objects from defeated opponents. This is an Allgard transfer, not destruction — the combat script's consent-on-entry grant can authorize forced transfer of intact objects on defeat. Different from salvage because nothing is destroyed. Covered by combat + transfer mechanics.

**Environmental debris.** Asteroid mining tailings, manufacturing waste, abandoned structures. Similar to destruction debris but created by different processes. Same salvage mechanics apply — it's all just objects with mass.
