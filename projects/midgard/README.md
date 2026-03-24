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

### What the Player Experiences

**Initiating a crossing:**
1. Player requests transfer to Domain B (walks to a portal, uses a menu, whatever the UI is)
2. Domain A packages the player's Objects (character, equipped items, carried inventory) into a transfer batch
3. Domain A sends the batch to Domain B with Proofs (Conservation Law 4)
4. Domain B verifies, accepts, and the player spawns into Domain B's world

**During transfer:** The player is in limbo — not on A, not yet on B. This takes 1-3 round trips over Leden (with promise pipelining, often just one). On a typical connection that's sub-second. The UI should show a transition screen, not pretend it's instant.

**What can go wrong:**
- Domain B rejects the transfer (objects not accepted, player banned, rate limited). Player stays on A.
- Network failure mid-transfer. The escrow transform (see [transfer routing](../allgard/README.md#cross-domain-transfer-routing)) ensures objects return to A after timeout. Nothing is lost.
- Domain B accepts the character but not some inventory items (incompatible types). Player arrives with partial inventory; rejected items stay on A.

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
