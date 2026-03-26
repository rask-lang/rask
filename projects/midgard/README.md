# Midgard

Virtual world architecture built on [Raido](../raido/), [Allgard](../allgard/), and [Leden](../leden/).

## Why

MMOs are broken in three ways:

1. **Only admins create.** Players consume content. They don't build worlds, write mechanics, or define how things work.
2. **Hosting requires a corporation.** You can't run your own world. Someone else owns the servers, the rules, and your stuff.
3. **Servers are islands.** Your character is trapped. Your items, your progress, your identity — locked to one operator.

Midgard is the answer to all three. Anyone can host a world. Anyone can create within it. Worlds connect to each other without a central authority.

## How It Works

You spin up a **gard** — that's your world. You're sovereign over it. You write [Raido](../raido/) scripts that define how things work: crafting recipes, combat formulas, NPC behavior, terrain generation, whatever you want. That's your game.

Players in your world use Raido too. They script NPCs, design items, write quest logic. Creation isn't admin-only — it's gameplay. Fuel limits and capability scoping keep it safe.

A player travels from your gard to someone else's. They arrive with their character, inventory, and currency intact. The transfer happens via [Allgard](../allgard/)'s federation model over [Leden](../leden/)'s protocol — [conservation laws](../allgard/CONSERVATION.md) ensure nothing gets duped, inflated, or lost in transit. The player doesn't see any of this. The federation infrastructure — gossip, audit, reputation, proof verification — runs automatically as part of the runtime.

That's it. The rest is details.

## What Midgard Adds

Game-specific concerns on top of the federation model:

- **Standard asset types** — the founding cluster shares a common set of game object types (swords, characters, regions, currency). Travel between gards with standard types is seamless.
- **Lockstep simulation** — deterministic lockstep for small groups (2-16 participants). Raido's fixed-point arithmetic and seedable PRNG guarantee bitwise-identical results across machines.
- **UGC sandboxing** — entity scripts, modding, NPC AI via Raido. Scripts get only the references the host hands them. Fuel-limited.
- **Verifiable crafting** — crafting recipes, damage formulas, and economic transforms are Raido scripts. Cross-gard crafting is [verifiable](../allgard/README.md#verifiable-transforms) — the receiving gard re-executes the script to confirm the result.
- **Cross-gard rate limiting** — Conservation Law 5 is per-gard. Coordinated abuse from multiple gards needs application-level policy.

For deeper exploration of what's possible — AI agents, living worlds, player-run economies, portable NPCs — see [GAME_DESIGN.md](GAME_DESIGN.md).

## Domain Crossing

What happens when a player moves between gards?

### The Experience

You travel to another gard. You arrive. Your character, inventory, and currency come with you. That's it.

No compatibility reports. No conversion dialogs. No loading screens with progress bars. The federation machinery — pre-staging, proof verification, conservation law checks, currency conversion — runs in the background during the transition. The player sees a travel animation. By the time it finishes, they're there.

This is the default experience between any gards running [standard asset types](../allgard/README.md#founding-cluster). Within the founding cluster, it's the *only* experience. Travel between gards should feel like walking between rooms, not like clearing customs.

### The Model

A player is an Owner. Their character, inventory, and equipment are Objects. Each Object lives on exactly one Domain (Law 2). "Traveling to another gard" means transferring Objects from Domain A to Domain B.

The boundary between gards is real — different hosts, potentially different rules. But the boundary is an infrastructure concern, not a player experience. [Federation is a property, not an experience.](../allgard/README.md#invisible-federation)

### How It Works Underneath

Every crossing runs a pre-stage query. Domain A tells Domain B: "Here's what's coming." Domain B responds with its mapping. Between gards with established bilateral agreements and standard asset types, this resolves silently — everything maps, nothing to report. The transfer executes, the player arrives.

**Timing:** 1-3 round trips over Leden (with promise pipelining, often just one). Sub-second on typical connections. The travel animation is longer than the actual transfer.

No new protocol primitive needed. Domain B already has catalog observation capabilities from Leden. Pre-staging is a query followed by a mapping response.

### Edge Cases

The seamless default covers gards in the founding cluster and any gards with established bilateral agreements. Outside that, friction surfaces — but only when something would genuinely surprise the player:

- **Exotic items.** A non-standard item the destination doesn't recognize travels [sealed](#asset-fidelity) — safe in inventory, intact, usable when you return to a compatible gard. The player gets a brief notification: "Flamebrand will travel sealed on Ironhold." Not a blocker, not a dialog. A notification.
- **First visit to an unaffiliated gard.** No established agreement. The player sees what transfers at full fidelity and what travels sealed. This is rare within any established network.
- **Low-reputation gard.** A trust warning. The player decides whether to proceed.
- **Changed compatibility.** A gard updated its asset types since the last visit. The player sees what changed.

Nothing is ever lost or downgraded. The worst case is sealed transfer — the item travels as an opaque blob, unusable on the destination, fully intact when you return home.

### Leaving and Disconnecting

Objects transfer to visited gards on a [lease](../allgard/PRIMITIVES.md#leased-transfer), not permanently. Every exit is safe:

| Exit | What happens | Speed |
|------|-------------|-------|
| Player travels home | Objects transfer home immediately | Instant |
| Player logs off on visited gard | Home gard revokes lease, objects return | Seconds |
| Player crashes / loses connection | Home gard detects session loss, revokes lease, objects return | Seconds |
| Visited gard goes dark | Lease timeout expires, objects recovered from Proof chain | Minutes to hours |

The player never needs to think about this. Their stuff is always home when they get there.

### What Can Go Wrong

- Destination gard's compatibility changes between pre-stage and transfer (rare — types don't change often). Transfer falls back to the new mapping; player is notified.
- Network failure mid-transfer. The escrow transform (see [transfer routing](../allgard/README.md#cross-domain-transfer-routing)) ensures objects return to the home gard after timeout. Nothing is lost.
- Visited gard goes dark while player is there. Home gard revokes the lease; if unreachable, the lease timeout kicks in. Objects return home either way. Recent mutations after the last Proof sync may be lost — you might lose the last few minutes of gameplay, not your items.
- Destination gard rejects the transfer entirely (player banned, rate limited). Player stays where they are.

### Asset Fidelity

Within the founding cluster, standard items just work — an iron sword is an iron sword everywhere. That's the default.

For items outside the standard set, the destination gard decides:

- **Full fidelity**: The gard recognizes the type and maps it 1:1.
- **Sealed transfer**: The gard doesn't recognize the type, but carries it faithfully as an opaque sealed object. The player can't use it there, but it persists exactly as-is. When they return home or visit a compatible gard, the item is intact. Like carrying a locked box through customs.
- **Stays home**: The item doesn't travel. It remains on the home gard, accessible when the player returns.

No downgrades. A "Flamebrand of the Seventh Circle" never becomes "magic sword (imported)." That destroys player trust and makes people never leave their home gard — which kills federation. You either support the item fully, carry it sealed, or leave it behind.

**Why sealed transfer matters.** A player can travel anywhere without worrying about their inventory. Standard items work. Exotic items come along sealed — unusable on the visited gard, but safe. The player's stuff is never damaged by traveling.

Sealed objects are content-addressed blobs with type tags. The visited gard doesn't need to understand the type to host the bytes. It just needs to enforce conservation laws on the sealed object (ownership, transfer, no duplication).

### Why This Isn't "The Metaverse"

Metaverse projects promise universal interoperability across all virtual worlds. That's impossible — you can't standardize every possible game object across every possible world. Allgard doesn't try.

Instead: gards that share the same asset types (the founding cluster, unions, de facto standards) get seamless travel for free. Gards that don't share types fall back to [sealed transfer](#asset-fidelity) — items travel intact but unusable until you return to a compatible gard. No downgrades, no data loss, no universal ontology needed.

The founding cluster IS the product. Seamless travel between 5-20 gards with shared types, shared currency, shared rules. That's the experience from day one. Federation to exotic gards outside the cluster is the advanced feature you grow into.

## The Network in Action

Midgard exists to prove the Allgard model works. Here's what the federation looks like through game scenarios.

### Scenario 1: Two Gards, One Economy

**Ironhold** is a PvP-focused gard with harsh decay rates. **Meadowvale** is a casual crafting gard. Both are in the founding cluster with shared standard asset types.

**What happens:**
1. A player on Meadowvale crafts a steel sword (Raido crafting script, verifiable). They want to sell it to an Ironhold player.
2. The buyer requests the transfer. No compatibility dialog — steel swords are a standard type. The transfer runs silently.
3. The sword transfers. Ironhold re-executes the crafting script to verify it was legitimately crafted (not duped). Gold transfers the other way.
4. Both gards log the transaction. Their bilateral reputation ticks up.
5. Ironhold's "poisoned weapons" travel sealed on Meadowvale. Meadowvale's "decorative furniture" travels sealed on Ironhold. Standard types work everywhere.

**What Allgard provides:**
- Law 1: Ironhold verifies the sword's minting script. Crafted from real materials, not conjured.
- Law 2: The sword is on Meadowvale, then on Ironhold. Never both. Atomic.
- Law 3: Sword + fee in, gold out. Balanced.
- Law 4: Every step references prior state. Causal chain.
- Law 5: Transfer rate limits prevent market flooding.
- Law 6: Only the sword's owner authorized the sale.

### Scenario 2: A Gard Goes Rogue

**Darkmarket** starts minting gold with no backing — inflating its supply to buy cheap goods from other gards.

**What happens:**
1. Darkmarket publishes a minting script (required by verifiable minting). The script mints 1000 gold per invocation.
2. Trading partners re-execute the script and see the minting rate. At first, nobody cares — it's Darkmarket's sovereign choice.
3. Darkmarket starts exporting gold at scale. Ironhold notices: "I've received 50,000 gold from Darkmarket this week. Their published supply audit claims 60,000 total. But Meadowvale tells me via audit gossip that they've also received 40,000. That's 90,000 just between us — the audit is fraudulent."
4. Ironhold stops accepting Darkmarket's gold. Meadowvale does too. Word spreads through gossip.
5. Darkmarket is isolated. Its gold is worthless outside its own boundary.

**What Allgard provides:**
- Verifiable minting: the minting *script* is honest (it can't hide the logic). But the minting *volume* is caught by audit gossip.
- Bilateral verification: each gard independently accumulates evidence.
- No central authority needed: gards make their own trust decisions based on their own observations.
- Reputation is emergent: Darkmarket isn't "banned" — it's just untrusted by everyone who checked the math.

### Scenario 3: Guild Across Gards

A guild operates across Ironhold, Meadowvale, and a new gard **Stormreach**. The guild leader holds assets on all three.

**What happens:**
1. Guild leader creates a Grant giving the guild bank (an automated Owner) transfer authority over guild assets on each gard.
2. A member on Stormreach requests materials from the guild bank on Meadowvale.
3. Steel bars transfer seamlessly — standard type, full fidelity. Healing potions travel sealed (Stormreach has different potion tiers). The member gets a notification about the sealed potions, not a blocking dialog.
4. Transfer executes. Meadowvale produces Proofs, Stormreach verifies.
5. The guild leader revokes the guild bank's Grant over retired members' items. Revocation propagates — eventually consistent, but high-value items use pessimistic liveness checks.

**What Allgard provides:**
- Grants enable delegation without ownership transfer. The guild bank operates on behalf of the leader.
- Non-transitive by default: the guild bank can't re-delegate to random players without explicit permission.
- Sealed transfer: non-standard items travel safely without being downgraded.
- Revocation: the leader stays in control. The membrane pattern means one switch-off kills all downstream access.

### Scenario 4: Verifiable Crafting

A player discovers a rare crafting recipe on Ironhold. They want to sell crafted items to players on other gards.

**What happens:**
1. The recipe is a Raido script: takes 5 star-metal bars + 1 dragon scale → 1 star-metal greatsword. The script enforces the inputs and outputs.
2. The player crafts a greatsword. The crafting Transform includes the script hash, inputs, and output.
3. A buyer on Meadowvale wants the greatsword. During transfer, Meadowvale fetches the crafting script, re-executes it, and verifies the greatsword was legitimately crafted from real materials.
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
| **Transaction fees** | Cross-gard transfers cost something. |
| **Training costs** | Learning abilities consumes resources. |

Rates are per-gard — casual servers low, hardcore servers high.
