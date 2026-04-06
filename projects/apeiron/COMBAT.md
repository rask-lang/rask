# Combat Model
<!-- id: apeiron.combat --> <!-- status: proposed --> <!-- summary: Authority model, execution model, and structural decisions for combat -->

Combat in Apeiron is a federation problem before it's a game design problem. Who computes the fight? Who has authority over the outcome? What prevents cheating? These structural questions constrain what mechanics are even possible. This spec answers the structural questions and defers mechanics to playtesting.

## Principles

**Verifiable, not enforced.** Same as constraint physics. The standard combat script is deterministic Raido bytecode. Any party can re-execute and verify any combat outcome. Non-standard combat scripts are visible, not banned.

**No authority required.** Combat resolution doesn't depend on either combatant's honesty. The outcome is deterministic from public inputs. Disputes are resolved by re-execution, not adjudication.

**War is expensive.** Destroyed objects are gone (Allgard conservation). Damaged components need replacement (material cost). Combat is a consumption sink — the economy's most violent one. Nobody fights casually.

**Counters emerge from physics.** No type chart. No damage multiplier table. The five constraint laws create natural tradeoffs between armor, speed, firepower, and range. Counter-strategies emerge from those tradeoffs. The founding cluster doesn't prescribe a meta — they publish the physics and let players discover it.

## Execution Model

### Hybrid: Orders + Scripts

Two layers of control, one required and one optional:

**Strategic orders** — what every combatant submits each tick. A structured data format: stance, target priority, formation, engagement range, power allocation, retreat conditions. No code required. This is the menu.

**Execution scripts** — Raido bytecode that interprets strategic orders into per-ship tactical decisions within each tick's simulation. The standard execution script (published by the founding cluster) does what the orders say literally. Community scripts are tradeable Allgard objects — combat doctrine as intellectual property. Custom scripts give an edge to factions with combat programmers.

A casual player selects orders from a menu and uses the standard script. A faction combat engineer writes a custom script that implements conditional logic the menu can't express. Both are valid combatants. The custom script player has an edge — like a better-tuned ship, not a different game.

### Multi-Tick Engagement

Combat spans multiple beacon ticks. Each tick:

1. **Observe.** Both sides see the result of the previous tick — positions, visible damage, fleet composition changes.
2. **Commit.** Both sides submit orders (hashed) before the beacon. Can't see the opponent's current orders.
3. **Reveal.** Beacon tick produces the randomness seed.
4. **Execute.** The combat script runs one round of simulation using (fleet states, orders, execution scripts, beacon value).
5. **Verify.** Result is deterministic. Any party can re-run and check.
6. **Repeat.** Until one side is destroyed, retreats, or both disengage.

Between ticks, combatants observe what happened and adapt. This is where strategy evolves — you saw their formation, you counter it, they counter your counter. Campaigns develop dynamics across dozens of ticks. No single strategy dominates because opponents adapt.

### Fleet Scale

A fleet is a set of ships (Allgard objects). Strategic orders apply at fleet level. The execution script translates fleet orders to per-ship actions. A single ship is a fleet of one — same model, no special case.

Fleet composition matters because constraint physics creates tradeoffs at fleet level: the slowest ship limits fleet speed, different ships prefer different engagement ranges, more ship types means more coordination overhead. A specialized fleet is focused but predictable. A mixed fleet is flexible but slower.

## Authority Model

### Who Computes

**The domain always computes.** Combat happens within a domain's jurisdiction — a star system, a route domain, a station. The domain runs the standard combat script. That's what domains do: host objects, run transforms.

**Anyone can verify.** The combat script is content-addressed Raido. The inputs are public: fleet states at engagement start, committed orders (revealed after beacon), beacon values, execution script hashes. Any party — combatant, observer, trading partner — can re-run the script and get the same result. Deterministic. No trust required.

### PvP on Neutral Ground

Two players fighting in a third party's star system or on a route domain. The domain has no stake in the outcome.

The domain is the referee. It receives orders from both sides, runs the script after the beacon, publishes the result with full execution trace. Either player can verify. If the domain lies (favors one player), the other proves it by re-executing — the false result doesn't match the deterministic output from the public inputs.

This is the clean case. Most PvP happens here — contested route domains, arena zones in star systems, border skirmishes.

### Attacking a Domain

The hard case. You're attacking the domain operator's own system. The referee IS the defendant.

**Computation still works.** The inputs are public. The script is deterministic. The attacker re-runs the script locally and knows the real outcome. If the domain claims a different result, the attacker has cryptographic proof: "here are the committed orders, here's the beacon value, here's the script hash — re-run it yourself."

**The real problem is custody.** The attacker's ships are hosted on the domain (they entered via consent-on-entry). Even if the attacker proves they won, the domain can refuse to apply damage transforms to its own ships. The domain physically controls the objects in its memory.

Three enforcement mechanisms:

**Reputation destruction.** A domain that provably cheats at combat is done as a trading partner. The proof is published via Leden gossip. Every bilateral partner can re-run the script and see the cheating. Economic isolation follows. For most domains, this deterrent is sufficient — the long-term cost of lost trade far exceeds the short-term gain of winning one fight.

**Retreat escrow.** The departure domain (where the attacker jumped from) holds a retreat claim on the attacker's fleet objects. The claim is established during the consent-on-entry capability exchange. If the hostile domain refuses to honor a verified combat outcome, the attacker invokes the retreat — ships revert to the departure domain's custody. The hostile domain can block this, but the block is visible in Leden session logs. This limits what the cheating domain gains: they can't keep the attacker's ships AND maintain honest reputation.

**Third-party verification.** Not enforced — voluntary. Mutual trading partners can re-run the combat script from the public inputs and see who's right. They adjust trust accordingly. This is the same mechanism Allgard uses for everything: verification propagates through self-interest, not authority.

### Consent-on-Entry

Entering a PvP-enabled domain means accepting a combat capability grant via Leden. The grant specifies:

- **Combat script hash** — which standard combat script applies. Visible before entry. Non-standard is your choice to accept or avoid.
- **Rules of engagement** — what triggers combat (always-on, flag-based, challenge-based). Domain policy.
- **Retreat rights** — whether retreat is possible and under what conditions. A domain that doesn't allow retreat is visible — most players won't enter.
- **Consequence scope** — what the combat script can do to your objects. Damage and destruction within the script's logic, not arbitrary modification.

The grant is inspectable. A player can see exactly what they're consenting to before entering. "This route domain uses standard combat v2.1, allows retreat after 3 ticks, PvP is always-on." Informed consent.

## Retreat and Disengagement

If combat is always to the death, nobody fights. Retreat is essential.

**Retreat takes time.** Committing a retreat order starts the disengagement — ships turn, accelerate away, still take fire for a number of ticks (domain parameter, visible in consent-on-entry). A fighting retreat is costly.

**Damaged ships may not escape.** A ship with a failed engine can't match fleet speed during retreat. It's left behind — captured or destroyed. "What do you sacrifice?" is a real decision.

**Pursuit costs fuel.** The attacker can pursue, but pursuit extends the engagement and burns fuel. Maybe not worth it for one crippled cruiser.

**Mutual disengagement.** Both sides commit "cease fire." Bilateral agreement. Possible at any tick.

**Retreat escrow protects the retreating party.** Once retreat is committed and the retreat timer expires, the departure domain's claim activates. Ships transfer back. The combat domain can't hold them.

## Counter-Strategies From Physics

No type chart. The five constraint laws create natural tradeoffs:

**Heavy brawler.** Dense armor (high density, hardness materials), short-range high-damage weapons, slow. Law 1: armor mass. Law 2: big hull, structural cost. Devastating up close but takes ticks to close distance.

**Long-range artillery.** Big weapons (high radiance, conductivity), light armor, moderate speed. Law 3: big weapons need big reactors. Fragile if reached. Kills brawlers before they arrive.

**Evasive skirmisher.** Fast, light, hard to hit. Low mass = high thrust-to-weight. Minimal coupling costs (Law 5 — few systems). Can't tank hits. Dances around artillery at range. Gets caught and crushed by brawlers.

These archetypes aren't prescribed — they emerge because the physics rewards specialization (Law 5 coupling costs punish generalists) while constraint interactions create natural weaknesses (heavy = slow, light = fragile, big guns = big reactor = big mass).

Fleet composition adds another dimension: mixed fleets cover weaknesses but pay coordination costs. A fleet of brawlers is predictable. A fleet of brawlers screening for artillery is harder to counter but slower and more complex to command.

## Deterministic Noise in Combat

Equipment quality shapes combat outcomes without introducing randomness:

```
actual_damage = base_damage + noise(weapon.quality, weapon_id, shot_index)
```

High-quality weapons: tight scatter, predictable damage. Crude weapons: wide scatter, volatile. The noise seed is `(weapon_id, shot_index)` — deterministic, not rerollable. You can't retry for a better roll. The shot is the shot.

The beacon seeds the engagement-level noise: initial positioning scatter, environmental conditions. You committed your orders before the beacon, so you can't optimize for specific conditions. Your strategy must be robust across different beacon values.

Combined: equipment quality determines shot-to-shot consistency (per PHYSICS.md). The beacon determines engagement-level conditions. Strategy must handle both.

## What This Spec Doesn't Cover

**Specific orders.** What stances exist, what formations look like, what "engagement range" means in spatial terms. Game design. Founding cluster publishes the first standard combat script with a specific order vocabulary. It evolves through playtesting.

**Damage formulas.** How weapon material properties map to base damage, how shields absorb, how stress accumulates. Game design within the constraint physics framework. The standard combat script encodes these. Content-addressed, versioned, voluntarily adopted.

**Sub-tick resolution.** How many simulation steps happen within each beacon tick's combat round. Tuning parameter in the combat script. More steps = more tactical richness but more compute.

**Spatial model.** 2D or 3D combat space. Range calculation, movement model, line-of-sight. Game design. The model here is agnostic — it works with any spatial model that's deterministic.

**Information model.** How much each side sees of the opponent. Full state, sensor-limited, fog of war. Game design. Sensors as a real system (Law 3 power draw, Law 5 coupling) would add depth. Deferred.

**Loot and capture.** What happens to destroyed ships' cargo, how capture works. Allgard transfer mechanics apply — destroyed objects are gone, captured objects transfer via standard bilateral escrow. The details are game design.

**Multi-party combat.** More than two sides in the same engagement. The bilateral model extends but the commit-reveal flow gets more complex. Deferred until two-party combat is solid.

## Interaction With Other Systems

**Constraint physics.** Ships are component trees. Combat stresses components. Damaged components fail (Law 4). Replacement consumes materials. The five laws constrain what ships can exist and what tradeoffs they face — combat exploits those tradeoffs.

**Beacon.** Commit-reveal per tick provides strategic blindness (can't react to opponent's current move) and engagement-level noise seeding (can't optimize for specific conditions).

**Transformation physics.** Better materials = better weapons, shields, engines. Combat creates demand for the best materials. The material research game feeds the combat game.

**Conservation laws.** Destroyed objects are permanently gone. Combat is a consumption sink. War has real economic cost — the most powerful deterrent against frivolous aggression.

**Leden capabilities.** Combat consent, retreat rights, and order submission all flow through the capability model. Revocation of combat consent = leaving the PvP zone.
