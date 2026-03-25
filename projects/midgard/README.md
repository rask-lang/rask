# Midgard

Virtual world architecture built on [Raido](../raido/), [Allgard](../allgard/), and [Leden](../leden/).

## Why

MMOs are broken in three ways:

1. **Only admins create.** Players consume content. They don't build worlds, write mechanics, or define how things work.
2. **Hosting requires a corporation.** You can't run your own world. Someone else owns the servers, the rules, and your stuff.
3. **Servers are islands.** Your character is trapped. Your items, your progress, your identity — locked to one operator.

Midgard is the answer to all three. Anyone can host a world. Anyone can create within it. Worlds connect to each other without a central authority.

## How It Works

You spin up a **domain** — that's your world. You're sovereign over it. You write [Raido](../raido/) scripts that define how things work: crafting recipes, combat formulas, NPC behavior, terrain generation, whatever you want. That's your game.

Players in your world use Raido too. They script NPCs, design items, write quest logic. Creation isn't admin-only — it's gameplay. Fuel limits and capability scoping keep it safe.

A player walks to the edge of your world and steps into someone else's. Their character and inventory transfer via [Allgard](../allgard/)'s federation model over [Leden](../leden/)'s protocol. [Conservation laws](../allgard/CONSERVATION.md) ensure nothing gets duped, inflated, or lost in transit. The federation infrastructure — gossip, audit, reputation, proof verification — is invisible to both players and domain operators. It runs automatically as part of the runtime.

That's it. The rest is details.

## What Midgard Adds

Game-specific concerns on top of the federation model:

- **Game object types** — swords, characters, regions, currency. Concrete types with game semantics mapped to Allgard Objects.
- **Lockstep simulation** — deterministic lockstep for small groups (2-16 participants). Raido's fixed-point arithmetic and seedable PRNG guarantee bitwise-identical results across machines.
- **UGC sandboxing** — entity scripts, modding, NPC AI via Raido. Scripts get only the references the host hands them. Fuel-limited.
- **Verifiable crafting** — crafting recipes, damage formulas, and economic transforms are Raido scripts. Cross-domain crafting is [verifiable](../allgard/README.md#verifiable-transforms) — the receiving domain re-executes the script to confirm the result.
- **Cross-domain rate limiting** — Conservation Law 5 is per-domain. Coordinated abuse from multiple domains needs application-level policy.

For deeper exploration of what's possible — AI agents, living worlds, player-run economies, portable NPCs — see [GAME_DESIGN.md](GAME_DESIGN.md).

## Domain Crossing

What happens when a player moves between servers (domains)?

### The Model

A player is an Owner. Their character, inventory, and equipment are Objects. Each Object lives on exactly one Domain (Law 2). "Moving to another server" means transferring your Objects from Domain A to Domain B.

The boundary is real — different domains, different rules, different trust. But the experience should hide that boundary when nothing interesting is happening. [Federation is a property, not an experience.](../allgard/README.md#invisible-federation)

### Default Experience

Between domains with established bilateral agreements and standard asset types, crossing is invisible:

1. Player requests transfer to Domain B (walks to a portal, uses a menu, whatever the UI is)
2. "Travel to Ironhold?" → yes
3. Player arrives. Character, inventory, currency transferred. Currency auto-converted at the agreed bilateral rate. Standard items at full fidelity. Exotic items travel sealed — safe in inventory, usable when you return home.

The pre-staging, proof verification, and conservation law checks happen underneath. The player doesn't see them because nothing is lost. This is the common case within the [founding cluster](../allgard/README.md#founding-cluster) and between any domains with established agreements.

**During transfer:** The player is briefly in limbo — not on A, not yet on B. This takes 1-3 round trips over Leden (with promise pipelining, often just one). On a typical connection that's sub-second. The UI shows a transition screen.

### When Friction Surfaces

The compatibility report appears when something would change:

| Trigger | What the player sees |
|---------|---------------------|
| First visit to an unknown domain | Full compatibility report — what transfers, what downgrades, what's rejected |
| Exotic items in inventory | Notification for sealed items: "Flamebrand will travel sealed — can't use on Ironhold" |
| Domain trust below threshold | Warning: "This domain has low reputation. Proceed?" |
| Compatibility changed since last visit | Delta report: "Ironhold no longer accepts cursed items" |

Example full report (shown for first visits or when items are affected):

| Object | Result |
|--------|--------|
| Character (Lv 30 Ranger) | Full fidelity |
| Iron Sword | Full fidelity |
| Flamebrand of the Seventh Circle | Sealed — can't use on Ironhold, safe in inventory |
| Cursed Amulet | Sealed — can't use on Ironhold, safe in inventory |
| 200 gold (Domain A) | Converted → 180 gold (Domain B), 10% exchange fee |

Nothing is lost. The player sees this *before committing*. "The Flamebrand and Cursed Amulet won't work on Ironhold — they'll travel sealed. Proceed?"

### Pre-Staging (Protocol)

Under the hood, every crossing runs pre-staging. Domain A sends a pre-stage query to Domain B: "Here's what's coming — character, inventory, currency. What's your mapping?"

No new protocol primitive needed. Domain B already has catalog observation capabilities from Leden. Pre-staging is a query (what do you accept?) followed by a mapping response. The transfer only happens after confirmation.

The difference is what the player sees. Between established domains with standard items, pre-staging resolves silently — everything maps, nothing to report. The compatibility UI only surfaces when the result would surprise the player.

### Leaving and Disconnecting

Objects transfer to visited domains on a [lease](../allgard/PRIMITIVES.md#leased-transfer), not permanently. This means every exit is safe:

| Exit | What happens | Speed |
|------|-------------|-------|
| Player walks back to home domain | Objects transfer home immediately | Instant |
| Player logs off on Domain B | Home domain revokes lease, objects return | Seconds |
| Player crashes / loses connection | Home domain detects session loss, revokes lease, objects return | Seconds |
| Domain B goes dark | Home domain can't reach B. Lease timeout expires, objects recovered from Proof chain | Minutes to hours |

The player never needs to think about this. Their stuff is always home when they get there.

### What Can Go Wrong

- Domain B's compatibility changes between pre-stage and transfer (rare — types don't change often). Transfer falls back to the new mapping; player is notified.
- Network failure mid-transfer. The escrow transform (see [transfer routing](../allgard/README.md#cross-domain-transfer-routing)) ensures objects return to home domain after timeout. Nothing is lost.
- Domain B goes dark while player is visiting. Home domain revokes the lease; if B is unreachable, the lease timeout kicks in. Objects return home either way. Recent mutations on B after the last Proof sync may be lost — you might lose the last few minutes of gameplay, not your items.
- Domain B rejects the transfer entirely (player banned, rate limited). Player stays on A.

### Asset Fidelity

A sword on Domain A is not automatically a sword on Domain B. Domain B decides how to interpret incoming objects:

- **Full fidelity**: Domain B recognizes the asset type and maps it 1:1. An iron sword is an iron sword.
- **Sealed transfer**: Domain B doesn't recognize the type, but carries it faithfully as an opaque sealed object. The player can't use it on Domain B, but it persists exactly as-is. When the player returns home or visits a domain that recognizes the type, the item is intact. Like carrying a locked box through customs.
- **Stays home**: The item doesn't travel. It remains in the player's inventory on their home domain, accessible when they return.

No downgrades. A "Flamebrand of the Seventh Circle" never becomes "magic sword (imported)." That destroys player trust and makes people never leave their home domain — which kills federation. You either support the item fully, carry it sealed, or leave it behind.

**Why sealed transfer matters.** It means a player can travel anywhere without worrying about their inventory. Standard items work. Exotic items come along as sealed objects — useless on the visited domain, but safe. The player's stuff is never damaged by traveling. This is the UX that makes people willing to cross domain boundaries.

Sealed objects are content-addressed blobs with type tags — they already have all the properties needed. The visited domain doesn't need to understand the type to host the bytes. It just needs to enforce the conservation laws on the sealed object (ownership, transfer, no duplication).

This is the honest version of the "metaverse interoperability" problem. Universal asset fidelity requires universal agreement on asset types — which requires either a central authority or a standard that everyone implements identically. Neither is realistic.

What works: **bilateral asset agreements** for types both domains care about, plus **sealed transfer** for everything else. Crafting materials, currency, basic equipment — these get explicit bilateral mappings through [standard asset types](../allgard/README.md#founding-cluster). Exotic items travel sealed. Nothing is lost, nothing is downgraded.

### Why This Isn't "The Metaverse"

Metaverse projects promise seamless cross-world experiences. They fail because:

1. **Game design requires control.** A PvP server can't accept a god-mode item from a creative server. Domain sovereignty solves this — each server controls what it accepts.
2. **Universal standards don't scale.** You can't standardize every possible game object. Bilateral agreements between domains that actually interact are tractable. A universal ontology isn't.
3. **Seamless transitions that hide real problems** surface those problems as bugs. The fix isn't to show everything — it's progressive disclosure. Default to seamless, surface friction only when something would surprise the player.

Between established domains with standard assets, the experience is seamless — one click, you're there. Between unknown domains or with exotic items, the boundary is visible and the player is informed. Both are the design.

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
4. Stormreach responds: steel bars accepted (full fidelity), healing potions travel sealed (Stormreach has different potion tiers and doesn't recognize Meadowvale's). The member can use the steel bars immediately; the potions stay in inventory as sealed objects until the member returns to a domain that recognizes them.
5. Member confirms. Transfer executes. Meadowvale produces Proofs, Stormreach verifies.
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
