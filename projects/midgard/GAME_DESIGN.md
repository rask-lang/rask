# Game Design

What's actually possible when you combine deterministic scripting (Raido), federated ownership (Allgard), and capability networking (Leden). Not a spec — an exploration.

## Player Creation Is the Game

The core loop isn't "kill monsters, get loot." It's "make things."

A domain operator writes Raido scripts that define their world's rules — combat, crafting, physics. But players write Raido scripts too. The difference between "operator" and "player" is just the scope of their capabilities.

**What players can create:**

- **Items with behavior.** A sword isn't just stats. It's a Raido script that defines what happens on hit. A player-smith who writes interesting weapon scripts becomes famous for it.
- **NPCs.** A shopkeeper that negotiates prices. A guard that patrols a route. A quest-giver that tracks progress. All Raido scripts, all fuel-limited, all scoped to what the gard grants them.
- **Quests.** A quest is a Raido script that watches for conditions and dispenses rewards. Any player can write one. Conservation Law 1 forces the quest creator to escrow rewards from their own supply — you can't promise what you don't have.
- **Mechanics.** A player proposes a new crafting recipe to the gard operator. If accepted, it becomes a published Raido script. Or the player runs their own gard with their own recipes.
- **Entire worlds.** A gard is cheap to run. A kid with a laptop can host a world. Their rules, their scripts, their sovereignty.

The fuel system is the safety net. Player scripts get a budget. They can't mine crypto or infinite-loop the server. They get enough fuel to do interesting things and no more. The gard sets the limits.

## AI Agents

Allgard doesn't care what an Owner is. It's a cryptographic identity. Human, bot, LLM — the system is agnostic. Conservation laws constrain everyone equally by structure, not policy.

### Why This Is the Right Fit

The unsolved problem with AI agents in open systems: how do you constrain them without "please be nice" policies?

Here the constraints are mechanical:

- **Can't create from nothing** — Law 1. Verifiable minting scripts required.
- **Can't access unauthorized objects** — Law 6. Capabilities are the only authority.
- **Can't spam** — Law 5. Bounded rates per entity, per time window.
- **Can't dupe or inflate** — Laws 1-3. Conservation is structural.
- **Can't escalate** — Grants attenuate only. Authority flows downhill.

The system won't execute invalid Transforms. That's fundamentally different from asking an AI to follow rules.

### AI Roles

**AI as NPC.** Not Raido-scripted NPCs (those are deterministic, pre-authored). LLM-powered characters that converse, form goals, make decisions. They hold capabilities like any player — they trade what they own, go where they have access, nothing else. A shopkeeper AI that negotiates prices, remembers regulars, adjusts stock.

**AI as gard operator.** An AI runs an entire world. Generates terrain procedurally, spawns encounters, tailors quests to who's playing. The gard is sovereign — the AI is the god of that space. Conservation laws still apply at the boundary when trading with other gards.

**AI as service provider.** An AI runs a courier gard — handles escrow transfers between gards that don't trust each other directly. Or a translation service mapping item types between incompatible gards. Or an arbitrage trader spotting price differences across the network. These are just Owners running businesses.

**AI as player assistant.** Capability-scoped delegation: "manage my shop while I'm offline, but you can't transfer items out." The Grant system is designed for exactly this — delegation with attenuation. The AI helper literally cannot exceed its authority.

### Non-Determinism Boundary

AI is non-deterministic. Raido is deterministic. They don't mix directly — an LLM can't run inside Raido. It sits outside as a client, submitting Transforms through Leden.

This means AI *decisions* aren't verifiable (you can't replay an LLM and get the same output), but AI *effects* are verifiable (every Transform is validated against conservation laws). The system trusts actions, not intentions. That's the right boundary — you don't need to verify *why* the AI sold you a sword for 50 gold, just that it owned the sword and you had the gold.

### Bots and Dominance

An AI can run many gards, many shops, many agents. Conservation laws prevent cheating but not dominance through volume. An AI that plays by the rules but outworks every human is allowed.

This is gard jurisdiction. A gard operator can set policies: "no automated Owners," "bot accounts flagged," whatever they want. Other gards might welcome bots. Players vote with their feet. Allgard intentionally doesn't have a concept of personhood — that's application policy, not infrastructure.

## Living Worlds

Raido's deterministic execution + serializable state enables something most game engines can't do well: worlds that exist when nobody's watching.

A gard runs a continuous Raido simulation. Weather changes. Crops grow. NPC populations migrate. Resources deplete and regenerate. When players log off, the simulation keeps ticking — or more practically, the gard fast-forwards through ticks (determinism means the result is identical whether you step through real-time or batch).

When a player logs in, the world is in a consistent state that follows from everything that happened since they left. Not "frozen until you arrive" — actually evolved.

Serializable state means the gard can snapshot at any point. Crash recovery is loading the last snapshot and replaying ticks. Migration to new hardware is serialize → transfer → deserialize. The world is portable.

## Portable Agents

A Raido VM state is a blob. An NPC, a pet, a familiar — it's a serialized VM snapshot that travels with its owner as an Object.

When you travel between gards, your pet comes with you. It carries its own behavior script. The destination gard doesn't need to know how your pet works — it just runs the VM. The pet behaves the same everywhere because it carries its own logic.

This extends to any autonomous entity: a courier bot you programmed, a guard that patrols your camp, a trading agent. They're Objects with embedded behavior that migrate like any other Object.

The destination gard can inspect the script (it's content-addressed) and decide whether to accept it: full fidelity, sandboxed execution with reduced fuel, or rejection. Same model as any other asset fidelity negotiation.

## Verifiable Provenance

Every crafted item carries the hash of the Raido script that created it, plus the hashes of its input materials. Anyone can re-execute the script with the same inputs and verify the output matches.

This gives you a provenance chain: this star-metal greatsword was crafted from these bars, which were smelted from this ore, which was mined at these coordinates. Every step is verifiable. No blockchain, no consensus, no gas fees — just deterministic execution and content addressing.

Players who care about authenticity (collectors, competitive players) can trace an item's full history. Players who don't can ignore it. The information is there either way.

## Composable Game Mechanics

A gard's rules are Raido scripts. Different gards run different scripts. This means:

- Gard A has medieval combat. Gard B has sci-fi weapons. Gard C mixes both.
- "Modding" isn't patching a game client. It's running a gard with different scripts. A mod *is* a gard.
- Players experience different rule sets as they move between gards. The rules are explicit, inspectable, and the gard advertises what scripts it runs.

This isn't theoretical — it falls out directly from Raido scripts being the game logic. A gard that wants different crafting recipes just publishes different scripts. A gard that wants different combat just publishes different combat scripts. There's nothing to patch, no client-side mods, no compatibility hell. The scripts run on the gard.

Cross-gard items work because the *item* is data and the *rules* are per-gard. A sword from Gard A follows Gard B's combat rules when used on Gard B. The item transfers; the mechanics are local.

## Gard Networks

Travel friction is proportional to how different two gards are. Two gards running the same asset types? Seamless — the player just arrives. A medieval gard and a sci-fi gard? Most items travel [sealed](README.md#asset-fidelity) — safe in inventory, but unusable until you return to a compatible gard.

The founding cluster is the first network — 5-20 gards with shared asset types and a shared currency. Travel within the cluster is seamless by design. Beyond the cluster, gards form networks through agreements:

- **De facto standards.** If 200 gards use the same "iron sword" type definition, that's a standard. Popular gard templates spread common types the way web frameworks spread conventions. Instant compatibility between all of them.
- **Unions.** A group of gards pre-negotiates: "we all recognize these 50 item types, these 3 currencies, zero exchange fees between members." Travel within the union is seamless. Travel out hits normal pre-staging.
- **Tiered trust.** A gard advertises: "Union members: instant. Allied gards: low fee. Unknown: full pre-staging, 10% exchange."

No new primitives needed. A union is just a bundle of bilateral Grants. Membership is having the full set. Leaving is revoking them. The protocol doesn't know unions exist — it just sees pre-staging queries that return "full fidelity" for everything.

## Emergent Economies

There's no designed economy. Conservation laws prevent cheating (duplication, inflation, conjuring value). Everything else emerges.

- Each gard sets its own minting rules, decay rates, crafting ratios.
- Exchange rates between gard currencies are market-determined — bilateral, not pegged.
- Scarcity is real because conservation is structural. If star-metal ore is rare on Ironhold, it's rare. Ironhold can't print more without publishing the minting script (which trading partners verify).
- Value sinks (crafting loss, decay, fees) prevent deflation spirals. Gards tune sink rates for their desired economy feel — casual or hardcore.

Players can run economic experiments. A UBI gard gives everyone a basic income. A pure-scarcity gard makes everything hard to get. An abundance gard lets creativity flow. Players vote with their feet. Bad economies empty out; good ones attract.

## Time and History

Serializable state opens up temporal mechanics:

- **Snapshots as save points.** A gard can roll back to any past state. Useful for events: "the invasion failed, but what if it hadn't?" Fork the timeline.
- **Replays as proof.** Deterministic execution means any sequence of events can be replayed and verified. Tournament results are provable. Disputed trades can be audited.
- **Historical tourism.** A gard publishes past snapshots as read-only instances. Visit the world as it was a month ago. See how the landscape changed.

This isn't time travel as a game mechanic (though a domain could build that). It's time travel as infrastructure — the ability to inspect, verify, and branch from any past state.

## Trustless Competition

Deterministic lockstep means both players in a duel run the same simulation. Inputs are exchanged, both sides compute, results must match. Divergence means someone tampered.

For small groups (2-16), this works without a trusted server. The participants are the servers. For larger events, a gard acts as the authority, but the replay is still verifiable by anyone after the fact.

Tournament brackets, competitive ladders, esports — all provable without trusting the organizer. The replay script is published, anyone can re-execute it, the result is deterministic.

## Player-Run Services

These aren't built-in features. They're just gards with specific scripts:

- **Bank.** A gard that holds assets in escrow, issues receipts, charges interest. Trust is bilateral — the bank builds reputation through honest transactions.
- **Courier.** Facilitates transfers between gards that don't trust each other directly. The courier gard acts as a trusted intermediary. Escrow ensures safety.
- **Auction house.** Aggregates listings from multiple gards, facilitates cross-gard trades, takes a cut. The conservation laws ensure every trade balances.
- **Tournament arena.** Hosts competitive matches, publishes deterministic replays as proof, awards prizes from its own supply.
- **Insurance.** A gard that covers losses from failed cross-gard transfers (timeout, rejection). Charges premiums, pays claims. Actuarial math via Raido scripts.
- **Mapping service.** Explores and catalogs gards — what they accept, their reputation, their asset types. Sells access to the catalog. Useful for players planning cross-gard travel.

The pattern: any service that exists in real economies can exist here, because the primitives (ownership, transfer, escrow, delegation) are general enough. The conservation laws keep everyone honest. Reputation is the currency of trust.
