# Midgard

Virtual world architecture. A concrete example of [Raido](../raido/), [Allgard](../allgard/), and [Leden](../leden/) working together.

Midgard is an application — it uses the infrastructure projects, it doesn't define them. For the federation model, see [Allgard](../allgard/README.md). For capabilities and protocol mapping, see [OCAP.md](OCAP.md).

## What Midgard Adds

Game-specific concerns on top of the federation model:

- **Game object types** — swords, characters, regions, currency. Concrete types with game semantics mapped to Allgard Objects.
- **Lockstep simulation** — deterministic lockstep for small groups (2-16 participants). Raido's fixed-point arithmetic and seedable PRNG guarantee bitwise-identical results across machines.
- **UGC sandboxing** — entity scripts, modding, NPC AI via Raido. Scripts get only the references the host hands them. Fuel-limited.
- **Verifiable crafting** — crafting recipes, damage formulas, and economic transforms are Raido scripts. Cross-domain crafting is [verifiable](../allgard/README.md#verifiable-transforms) — the receiving domain re-executes the script to confirm the result.
- **Cross-domain rate limiting** — Conservation Law 5 is per-domain. Coordinated abuse from multiple domains needs application-level policy.

## Domain Crossing

What happens when a player moves between servers (domains)?

### The Model

A player is an Owner. Their character, inventory, and equipment are Objects. Each Object lives on exactly one Domain (Law 2). "Moving to another server" means transferring your Objects from Domain A to Domain B.

This is not seamless. It's a deliberate boundary.

### Pre-Staging

Before anything transfers, the player sees what will happen. Domain A sends a pre-stage query to Domain B: "Here's what's coming — character, inventory, currency. What's your mapping?"

Domain B responds with a compatibility report:

| Object | Result |
|--------|--------|
| Character (Lv 30 Ranger) | Accepted — full fidelity |
| Iron Sword | Accepted — full fidelity |
| Flamebrand of the Seventh Circle | Downgraded → "magic sword (imported)" |
| Cursed Amulet | Rejected — incompatible type |
| 200 gold (Domain A) | Accepted → 180 gold (Domain B), 10% exchange fee |

The player sees this *before committing*. "You'll arrive with these items. The Flamebrand will downgrade. The amulet won't transfer. Proceed?"

No new protocol primitive needed. Domain B already has catalog observation capabilities from Leden. Pre-staging is a query (what do you accept?) followed by a mapping response. The transfer only happens if the player confirms.

### What the Player Experiences

**Initiating a crossing:**
1. Player requests transfer to Domain B (walks to a portal, uses a menu, whatever the UI is)
2. Domain A sends a pre-stage query to Domain B
3. Player reviews the compatibility report — sees what transfers, what downgrades, what's rejected
4. Player confirms (or cancels)
5. Domain A packages the player's accepted Objects into a transfer batch with Proofs (Conservation Law 4)
6. Domain B verifies, accepts, and the player spawns into Domain B's world. Rejected items stay on A.

**During transfer:** The player is in limbo — not on A, not yet on B. This takes 1-3 round trips over Leden (with promise pipelining, often just one). On a typical connection that's sub-second. The UI should show a transition screen, not pretend it's instant.

**What can go wrong:**
- Domain B's compatibility changes between pre-stage and transfer (rare — types don't change often). Transfer falls back to the new mapping; player is notified.
- Network failure mid-transfer. The escrow transform (see [transfer routing](../allgard/README.md#cross-domain-transfer-routing)) ensures objects return to A after timeout. Nothing is lost.
- Domain B rejects the transfer entirely (player banned, rate limited). Player stays on A.

### Asset Fidelity

A sword on Domain A is not automatically a sword on Domain B. Domain B decides how to interpret incoming objects:

- **Full fidelity**: Domain B recognizes the asset type and maps it 1:1. An iron sword is an iron sword.
- **Downgrade**: Domain B doesn't support the specific type but has a generic equivalent. A "Flamebrand of the Seventh Circle" becomes "magic sword (imported)."
- **Rejection**: Domain B doesn't accept the asset type at all. The object stays on A.

This is the honest version of the "metaverse interoperability" problem. Universal asset fidelity requires universal agreement on asset types — which requires either a central authority or a standard that everyone implements identically. Neither is realistic.

What works: **bilateral asset agreements**. Domain A and B agree on a mapping for specific types they both care about. Crafting materials, currency, basic equipment — these get explicit bilateral mappings. Exotic items get downgraded or rejected. It's messy, imperfect, and exactly how real-world trade works.

### Why This Isn't "The Metaverse"

Metaverse projects promise seamless cross-world experiences. They fail because:

1. **Game design requires control.** A PvP server can't accept a god-mode item from a creative server. Domain sovereignty solves this — each server controls what it accepts.
2. **Universal standards don't scale.** You can't standardize every possible game object. Bilateral agreements between domains that actually interact are tractable. A universal ontology isn't.
3. **Seamless transitions hide real problems.** Latency, trust verification, asset compatibility — these are real costs. Hiding them behind a seamless façade means they surface as bugs instead of explicit boundaries.

Midgard makes the boundary visible and the player informed. You know when you're crossing domains. You know what transferred and what didn't. The experience is a border crossing, not a teleporter — and that's the design.

## The Network in Action

Midgard exists to prove the Allgard model works. Here's what the federation looks like through game scenarios.

### Scenario 1: Two Servers, One Economy

**Ironhold** is a PvP-focused server with harsh decay rates. **Meadowvale** is a casual crafting server. Both run as independent Allgard domains.

**What happens:**
1. Ironhold and Meadowvale discover each other through Leden gossip.
2. They negotiate bilateral asset agreements: both recognize iron, steel, wood, and gold. Ironhold doesn't accept Meadowvale's "decorative furniture" type. Meadowvale doesn't accept Ironhold's "poisoned weapons."
3. A player on Meadowvale crafts a steel sword (Raido crafting script, verifiable). They want to sell it to an Ironhold player.
4. The buyer pre-stages the transfer — sees "steel sword, full fidelity, 5 gold fee." Confirms.
5. The sword transfers. Ironhold re-executes the crafting script to verify it was legitimately crafted (not duped). Gold transfers the other way.
6. Both domains log the transaction. Their bilateral reputation ticks up.

**What Allgard provides:**
- Law 1: Ironhold verifies the sword's minting script. It was crafted from real materials, not conjured.
- Law 2: The sword is on Meadowvale, then on Ironhold. Never both. Atomic.
- Law 3: Sword + fee in, gold out. Balanced.
- Law 4: Every step references prior state. The crafting, the listing, the transfer — causal chain.
- Law 5: Transfer rate limits prevent the same player from flooding the market.
- Law 6: Only the sword's owner authorized the sale. Non-transitive — the buyer can't resell to a third domain without the sword's new Grant.

### Scenario 2: A Domain Goes Rogue

**Darkmarket** starts minting gold with no backing — inflating its supply to buy cheap goods from other servers.

**What happens:**
1. Darkmarket publishes a minting script (required by verifiable minting). The script mints 1000 gold per invocation.
2. Trading partners re-execute the script and see the minting rate. At first, nobody cares — it's Darkmarket's sovereign choice.
3. Darkmarket starts exporting gold at scale. Ironhold notices: "I've received 50,000 gold from Darkmarket this week. Their published supply audit claims 60,000 total. But Meadowvale tells me via audit gossip that they've also received 40,000. That's 90,000 just between us — the audit is fraudulent."
4. Ironhold stops accepting Darkmarket's gold. Meadowvale does too. Word spreads through gossip.
5. Darkmarket is isolated. Its gold is worthless outside its own boundary.

**What Allgard provides:**
- Verifiable minting: the minting *script* is honest (it can't hide the logic). But the minting *volume* is caught by audit gossip.
- Bilateral verification: each domain independently accumulates evidence.
- No central authority needed: domains make their own trust decisions based on their own observations.
- Reputation is emergent: Darkmarket isn't "banned" — it's just untrusted by everyone who checked the math.

### Scenario 3: Guild Across Servers

A guild operates across Ironhold, Meadowvale, and a new server **Stormreach**. The guild leader holds assets on all three.

**What happens:**
1. Guild leader creates a Grant giving the guild bank (an automated Owner) transfer authority over guild assets on each domain.
2. A member on Stormreach requests materials from the guild bank on Meadowvale.
3. The guild bank submits a pre-stage query to Stormreach: "5 steel bars, 2 healing potions."
4. Stormreach responds: steel bars accepted (full fidelity), healing potions downgraded to "minor healing potion" (Stormreach has different potion tiers).
5. Member reviews, confirms. Transfer executes. Meadowvale produces Proofs, Stormreach verifies.
6. The guild leader revokes the guild bank's Grant over retired members' items. Revocation propagates — eventually consistent, but high-value items use pessimistic liveness checks.

**What Allgard provides:**
- Grants enable delegation without ownership transfer. The guild bank operates on behalf of the leader.
- Non-transitive by default: the guild bank can't re-delegate to random players without explicit permission.
- Pre-staging: the member knows exactly what they'll receive before the transfer happens.
- Revocation: the leader stays in control. The membrane pattern means one switch-off kills all downstream access.

### Scenario 4: Verifiable Crafting

A player discovers a rare crafting recipe on Ironhold. They want to sell crafted items cross-domain.

**What happens:**
1. The recipe is a Raido script: takes 5 star-metal bars + 1 dragon scale → 1 star-metal greatsword. The script enforces the inputs and outputs.
2. The player crafts a greatsword. The crafting Transform includes the script hash, inputs, and output.
3. A buyer on Meadowvale wants the greatsword. During pre-staging, Meadowvale fetches the crafting script, re-executes it, and verifies the greatsword was legitimately crafted from real materials.
4. Meadowvale doesn't need to trust Ironhold's claim about the sword. It verified the computation independently.

**What Allgard provides:**
- Verifiable transforms: the crafting logic is a content-addressed Raido script. Anyone can re-execute it.
- Conservation Law 3: the crafting script must balance — 5 bars + 1 scale in, 1 sword out. The bars and scale are destroyed (value sink).
- No "duped items" problem: every crafted item has a verifiable provenance chain back to its raw materials.

## Value Sinks

Midgard's concrete sinks for [Conservation Law 3](../allgard/CONSERVATION.md#law-3-conservation-of-exchange):

| Sink | Mechanism |
|------|-----------|
| **Crafting loss** | 3 iron bars → 1 sword, not 3 ↔ 1. |
| **Repair costs** | Equipment degrades without upkeep. |
| **Item decay** | Consumables, buffs, temporary enchantments expire. |
| **Transaction fees** | Cross-domain transfers cost something. |
| **Training costs** | Learning abilities consumes resources. |

Rates are per-domain — casual servers low, hardcore servers high.
