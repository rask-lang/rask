# Economy Bootstrap
<!-- id: apeiron.economy --> <!-- status: proposed --> <!-- summary: How five founding operators break the bootstrap cycle — seed currency, single-domain minting, new player economics -->

The [constraint physics](PHYSICS.md) define what can exist. The [transformation physics](TRANSFORMATION.md) define what can be created. This spec defines how the economy starts — the ignition sequence that turns a galaxy of potential into a functioning market.

## The Problem

Apeiron's economy has a bootstrap cycle that can't be solved by players alone.

You need fuel to jump between systems. Fuel requires hydrogen and carbon, extracted and refined. Refining requires facilities. Facilities are built from materials. Materials require extraction. Extraction requires a domain — a star system you host and operate.

A new player arrives with nothing. They can't produce without things they don't have, and they can't trade without having produced something.

Five people who trust each other break this cycle by going first. Each claims a system, extracts, refines, builds, stockpiles — all before the first outside player arrives. They eat the cold-start cost. That's the deal.

## The Seed Currency

Credits. A fiat currency minted by one domain — the domain whose operator runs the mint. The other four accept credits through bilateral trade agreements. Standard Allgard: per-domain supply sovereignty.

I chose fiat over commodity money. Fuel was the other candidate — universal demand, natural sink through combustion. But fuel-as-currency conflates "money to move" with "money to trade." Players shouldn't feel like they're burning their wallet every time they jump. Fuel is a critical commodity. Credits are the unit of account. You buy fuel with credits.

### Why Credits Work

**Pre-bootstrapped backing.** The five systems run a functioning economy before any player arrives. Credits are redeemable for real goods — fuel, materials, components, ship upgrades — at 5 operating systems on day one. That's not a promise. That's inventory.

**Single-source minting.** The minting domain controls credit supply. Credits enter circulation when the mint operator pays them out — starter ship subsidies, courier jobs, scout bounties, mining contract payouts. The other four domains receive credits through bilateral trade: they sell goods and services to the minting domain (or to each other) for credits.

No activity, no payouts. Credit supply grows proportional to economic participation, not on a schedule.

**Transparent supply.** The minting domain's script is Raido bytecode — content-addressed, publicly auditable ([Conservation Law 1](../allgard/CONSERVATION.md)). Trading partners verify supply through bilateral audit. If the mint operator inflates recklessly, it's visible to everyone. Partners switch currencies. No coordination protocol needed — each domain independently decides whether to keep accepting credits. Hyperinflation requires hiding what you're doing. The conservation laws make that impossible.

**No centralization.** Credits are not protocol-privileged. They're one domain's asset type. Other domains accept them by choice, through bilateral agreements. Players choose which currency to use — credits, fuel, raw titanium, whatever a bilateral trade agrees on. Nobody mandates credits. They're the default because the founding economy has the most liquidity, not because anyone's forced to use them. If the minting domain loses trust, the other four can adopt a different currency tomorrow.

### Inflation

Credits will inflate mildly. I'm not fighting it.

The minting domain pays out credits for onboarding — starter ships, courier payments, scout bounties. That's inflationary. That's fine. Mild inflation punishes hoarding and keeps credits moving. Central banks target ~2% for the same reason.

Hyperinflation is prevented structurally:
- Minting is activity-tied, not scheduled. No activity = no new credits.
- The minting script is public. Every trading partner can audit supply.
- If partners don't like the minting rate, they stop accepting credits. The exit option IS the discipline.

The natural arc: credits dominate early (only currency with liquidity). Commodity money competes mid-game (players discover fuel and titanium hold value better as stores of value). Credits become one currency among many late-game as the founding systems' share of the economy shrinks. Players who hold real goods preserve value. Players who hold credits spend them. Both strategies work. Neither is forced.

### Initial Pricing

The five operators pre-negotiate starting price tables before launch. These aren't mandates — they're initial conditions. "Iron ore starts at roughly 0.3 credits per unit" so AI traders have something to anchor on.

AI traders start with seed prices and adjust based on local supply and demand. If inventory piles up, price drops. If buyers queue, price rises. Human players who find arbitrage opportunities between systems are doing exactly what they should — price differences reflect real transport costs and local supply conditions. Arbitrage narrows gaps until the profit margin equals the travel cost.

Nobody maintains these starting prices. They're scaffolding. Within weeks of real trading, market prices diverge from the tables. That's success.

## Founding System Selection

Five people who trust each other each claim a system from the galaxy seed.

### What Sensible Operators Do

**Pick nearby systems.** Short jump distances between all five. Fuel is scarce at bootstrap; long jumps kill early trade. Everyone naturally gravitates toward systems 1-2 jumps from each other. Star topology works. Long chains don't.

**Cover the elements between them.** Five systems together will typically cover all 7 common elements: iron, carbon, silicon, copper, aluminum, hydrogen, sulfur. No single system needs all of them — that's why you trade. If someone grabs a titanium system, that's a bonus. Trace strategic elements (chromium, tungsten, gold) are nice but the seed gives what it gives.

Exotic elements (uranium, platinum) sit at 5-15% availability per system. Five systems probably won't have any. That's fine. Exotics are the reason players push beyond the founding systems. If the founders had everything, there'd be no frontier.

### Emergent Specialization

Each operator picks a system that looks good to them. Because the seed distributes resources unevenly, they end up with different strengths. The same patterns tend to emerge:

- **Hydrogen-rich system** → fuel production hub. Every ship needs fuel. This system prints money early.
- **Iron/carbon system** → foundry and shipyard. Structural steel is the backbone material. Put the shipyard where the iron is.
- **Titanium system** → aerospace components. Hull plate needs titanium + aluminum. If one system has both, it dominates ship hulls.
- **Copper/silicon system** → electronics and conductors. Conductor wire, silicon carbide, compute components.
- **Mixed deposits with good connectivity** → trade hub. Doesn't need the best geology — it needs the best geography.

Every system extracts what it has. Specialization is about where *facilities* get built — you put the shipyard where the iron is, not where the titanium is.

| Specialization | Primary Exports | Primary Imports |
|---|---|---|
| Fuel hub | Hydrocarbon fuel, raw hydrogen | Iron, copper, manufactured goods |
| Foundry/shipyard | Structural steel, ships, hull plate | Titanium, copper, fuel |
| Aerospace | Hull plate, light alloy | Iron, carbon, fuel |
| Electronics | Conductor wire, silicon carbide | Iron, aluminum, fuel |
| Trade hub | Re-exported goods, brokerage | Everything (margin on geography, not geology) |

The trade hub profits because it's *between* the others. Two systems 2 jumps apart trade through the hub at 1 jump each. The hub buys low, sells high, and the transport savings justify the markup.

## The Bootstrap Sequence

Six phases. Each depends on the previous one completing. Every object at every phase has a valid proof chain back to the galaxy seed.

### Phase 0 — Selection

Founding operators pick five systems from the seed. Deploy domains — one per system, real hosting. Pre-negotiate bilateral trust at Allied level between all five.

Agree on shared definitions through bilateral negotiation before anything physical happens:

- Standard asset types (elements, materials, components, ships, credits)
- Physics scripts per [PHYSICS.md](PHYSICS.md)
- Element table per [ELEMENTS.md](ELEMENTS.md)
- 7 starter recipes per [TRANSFORMATION.md](TRANSFORMATION.md)
- Standard facility and ship blueprints

No central authority publishes these — they're conventions the five domains adopt by mutual agreement. All content-addressed Raido bytecode. Every domain references the same scripts by hash.

### Phase 1 — Extraction

AI outposts deploy to deposit sites and mint raw elements from the seed. Each minted unit carries a proof chain: galaxy seed → deposit location → extraction script hash → element type and quantity.

System inventories fill with local resources. A hydrogen-rich system accumulates hydrogen. An iron-rich system accumulates iron. No trade yet — just stockpiling.

This is where proof chains start. Every gram of iron that will ever become a ship hull traces back to this phase.

### Phase 2 — Industry

Pre-designed facilities deploy, constructed from extracted materials and validated by constraint physics. The smelter costs iron and carbon. The fuel refinery costs steel and copper. Facilities are real objects with real mass.

First crafting uses the 7 starter recipes:

| Recipe | Inputs | Ratio | Notes |
|---|---|---|---|
| Structural steel | Fe + C | 97:3 | Backbone construction material |
| Chromium steel | Fe + Cr | 82:18 | Requires strategic element — not all clusters have this early |
| Light alloy | Al + Si | 90:10 | Low-mass structural material |
| Hull plate | Ti + Al | 90:10 | Ship hulls, station armor |
| Hydrocarbon fuel | H + C | 80:20 | Energy and trade commodity |
| Conductor wire | Cu + Au | 95:5 | Needs trace gold — may require import |
| Silicon carbide | Si + C | 70:30 | Heat-resistant components, electronics |

Fuel production is the priority. Fuel is both the energy source for everything else and a key trade commodity. A cluster without fuel production is dead.

Chromium steel and conductor wire need strategic/trace elements. If the founding five don't have chromium or gold, those recipes wait. The economy works without them; it just works better with them.

### Phase 3 — Infrastructure

Shipyard facilities deploy at the foundry system. A shipyard is itself materials — steel beams, hull plate, conductor wire. Built from Phase 2 outputs.

Starter ships built from standard blueprints. A basic hauler needs:

- Hull plate → structural frame
- Structural steel → internal skeleton
- Conductor wire → electrical systems
- Silicon carbide → engine components
- Light alloy → fuel tank
- Structural steel + light alloy → cargo bay

Every component is real materials with real mass. The ship's total mass is the sum of its parts. Standard blueprints are published — anyone can verify the component tree.

### Phase 4 — Trade

AI haulers carry cargo between systems. Each exports surplus, imports shortfalls. Exchange rates emerge from bilateral AI negotiation. Credits become the unit of account because barter doesn't scale past two parties.

Credits flow outward from the minting domain. The minting domain pays credits for goods it imports — fuel, components, materials. Other domains earn credits by selling to the minting domain, or to each other once credits circulate. Domains without direct trade to the minting domain earn credits indirectly by selling to domains that have them.

Trade routes follow geography. Systems 1 jump apart trade directly. Systems 2 jumps apart route through the hub. The hub takes a cut. This isn't prescribed — it emerges because the transport cost math makes it cheaper.

### Phase 5 — Player Entry

The economy is running. Ships move. Markets clear. Inventories cycle.

New players spawn at a founding system, receive a starter ship (built from real materials, proof chain intact), and enter through labor.

### Nothing Special

The founding sequence is not special-cased. Same extraction scripts, same physics constraints, same conservation laws. Any group of players can repeat this process in a new region of the galaxy. The founding five just went first. They have no institutional authority — just a head start and existing inventory.

A player who claims system #6 runs the exact same scripts against the exact same seed. The only advantage the first five have is a head start.

## New Player Economics

You start with a ship. That's it.

### The Starter Ship

Common materials only: structural steel hull, basic engine, small fuel tank, small cargo bay. Fueled for roughly 10 short jumps within the founding systems. Empty cargo hold.

Deliberately weak. Fine for courier runs and short exploration. Not competitive for serious hauling, mining, or combat. No starting credits. No care package. The ship IS the bootstrap.

### Why Free Ships Work

**Common materials = negligible cost.** Structural steel and basic components are the cheapest things in the galaxy. The shipbuilding domain has massive stockpiles — it's been producing since Phase 3.

**New players add network value.** Every new player is a potential courier, miner, scout, trader. The network effect outweighs the material cost of a basic hull.

**Anti-farming.** Scrap value of a starter ship is negligible. Sybil resistance from Conservation Law 7 means spinning up accounts to harvest ships is more effort than just mining. Farming starter ships is strictly less efficient than mining directly.

### First Activities

All paid in credits:

**Courier jobs.** Individual domains post contracts: move cargo from System A to System B. Fuel cost of the trip must be less than payment. The margin is your profit.

**Mining contracts.** Individual domains offer extraction access. You operate an extractor provided by the domain, keep a share of output. No outpost needed — you're working on someone else's domain.

**Scout reports.** Run beacon-gated prospect Transforms on unclaimed systems from a nearby domain. Produces probability ranges for element deposits — not ground truth, but enough to inform claiming decisions. Payment in credits for verified prospect data. See [EXPLORATION.md](EXPLORATION.md).

**Facility rental.** Domains with public crafting facilities charge a fee in credits. No need to own a facility to start crafting.

### Progression

1. **Labor.** Courier, mine, scout. Earn credits and small amounts of materials. Learn the cluster's geography and economy.
2. **Crafting.** Use public facilities to upgrade ship components. Better engine = cheaper jumps. Bigger cargo bay = more profit per courier run. Each upgrade compounds.
3. **Trade.** Buy low, sell high between systems. Better ship = more cargo = more profit. You're competing with AI haulers now, and you can win because you're smarter about route selection.
4. **Expansion.** Accumulated enough to deploy an outpost in an unclaimed system. Now you're a domain operator. You mint your own resources, set your own prices.
5. **Independence.** Own facilities, own specialization, alliances with other operators. The founding cluster becomes one trading partner among many.

Nothing enforces these stages. A new player can fly into unclaimed space on day one. They'll run out of fuel and be stranded, but they can try. The progression is emergent from the economics, not from gates.

## Job Mechanics

All four starter jobs map directly to Allgard primitives. No new mechanisms needed — just composition.

### Courier Jobs

A domain posts a `courier_contract` Object containing cargo ID, destination domain, destination owner, deadline, and payment amount. The domain locks credits into escrow when creating the contract — a conditional Transfer that releases on delivery proof.

Player accepts by signing the contract (Transform: mutate contract state to accepted, referencing the player's Owner). The domain transfers cargo to the player via conditional Transfer Intent: `forward_to` = destination, `forward_deadline` = contract deadline, `on_failure` = `return_to_sender`.

Player jumps to the destination and delivers the cargo, completing the conditional forward. When the destination sends `TransferComplete` for the cargo, the escrowed credits release to the player. Two conditional transfers, linked: one for cargo, one for payment.

If the player fails to deliver by deadline: cargo returns automatically, escrowed credits return to the posting domain. Clean rollback, no intervention needed.

This composes entirely from conditional transfers. No courier-specific protocol.

### Mining Contracts

The domain issues a Grant to the player: `scope` = `operate_extractor`, `target` = a specific extractor Object, `expiry` = contract duration.

The player submits extraction Transforms through the Grant. The extractor runs a minting script that references the seed deposit. The script's output is split — say 70% to domain, 30% to player. Both receive Objects directly from the Transform. The split ratio is encoded in the minting script, content-addressed, publicly auditable.

Grant expires when the contract ends. Player loses extractor access.

Different domains offer different splits — that's competition for labor. The player inspects the minting script before accepting. If a domain's split is bad, don't take the contract.

### Facility Rental

Player transfers credits to the domain (standard bilateral transfer). Domain issues a time-limited Grant: `scope` = `submit_transforms`, `target` = a specific facility Object, `expiry` = rental period.

Player submits crafting Transforms through the Grant. The facility's Transform logic validates inputs (does the player have the materials?), runs the interaction function, produces output Objects owned by the player. Crafting loss from the mass budget applies as normal.

Grant expires when the rental period ends. Renew by paying again.

Domains compete on price, equipment quality (precision instruments affect craft outcomes), and location.

### Scout Reports

Scouts run beacon-gated prospect Transforms from a nearby domain, producing probability ranges for unclaimed systems. Each prospect costs committed fuel + one beacon tick. See [EXPLORATION.md](EXPLORATION.md) for the full model.

A scout report is an Object containing prospect data — star ID, body IDs, probability ranges, confidence levels (how many epochs of data). The scout transfers the report to a domain that buys prospect data.

The domain verifies each prospect result: re-run `prospect(seed, star_id, body_id, beacon_value)` for each claimed epoch. Match → credits to the player. No match → rejected.

The report is data, not a scarce resource. The player can sell it to multiple buyers. But unlike free seed data, prospect results required real fuel and real beacon commitments to produce — that's the value.

### No New Primitives

All four job types compose from Grants, conditional Transfers, Transforms, and Objects. Domains post jobs by creating contract Objects and locking escrow. Players accept by signing contracts and receiving Grants. Payment flows through conditional transfers tied to delivery proofs.

No job-specific protocol. No quest system. No special-cased mechanics. A domain that invents a new job type — escort missions, construction contracts, research commissions — uses the same primitives. I don't want a game engine that knows what a "job" is. I want primitives that let jobs emerge.

See: [../allgard/PRIMITIVES.md](../allgard/PRIMITIVES.md), [../allgard/TRANSFER.md](../allgard/TRANSFER.md)

## AI Economy

Each domain runs its own AI agents. There's no cluster-wide AI coordinator — each domain's AI operates autonomously within that domain's sovereignty. When the first player logs in, they enter a functioning economy with liquidity, supply chains, and price signals.

### What AI Does

**AI extractors** operate mining outposts at their home domain. Real extraction, real minting, real proof chains. Each domain decides how many extractors to run and where.

**AI haulers** move materials between systems. Burn real fuel, follow real routes. Each domain dispatches its own haulers for its own trade agreements.

**AI stations** set prices based on local supply and demand. Simple market-making with real inventory — they hold stock, run out of things, adjust prices when supply shifts. Each domain's pricing AI is independent.

**AI shipwrights** build ships from real materials. Production capacity is finite and bottlenecked by material supply like everything else.

**AI researchers** explore the interaction function. May publish discoveries or sell them. AI domains are sovereign too.

### Price Discovery

Start from pre-negotiated price tables. Adjust on supply and demand. Spatial arbitrage emerges naturally: fuel is cheap near gas giants, expensive far from them.

Human players who find better arbitrage than AI are just better traders. Welcome.

### The Arc

AI is constrained by the same 7 conservation laws. Can't cheat. Can be outcompeted. As humans take over high-value roles, AI retreats to commodity work and frontier extraction. The economy starts 100% AI and gradually becomes human-driven where it matters, AI-supported where it doesn't.

I chose this over a cold start because an economy with no liquidity is dead on arrival. AI provides the initial liquidity. Humans provide the intelligence that makes it interesting.

## Tuning Knobs

| Knob | Controls | Risk if wrong |
|------|----------|---------------|
| Starter ship fuel capacity | How far new players can go before earning | Too low = stuck. Too high = no urgency to earn. |
| Courier pay rates | Early-game income | Too low = grind. Too high = skip progression. |
| Mining contract share | Labor vs. capital split | Too low = exploitation feel. Too high = no incentive to operate. |
| Facility rental fees | Barrier to early crafting | Too high = new players can't upgrade. Too low = no incentive to build own. |
| AI trader aggressiveness | Price adjustment speed | Too aggressive = humans can't find margins. Too passive = easy exploitation. |
| Credit minting rate per activity | Money supply growth | Too high = inflation. Too low = deflationary spiral. |
| Base fuel cost per jump | Movement friction | Too high = nobody moves. Too low = geography doesn't matter. |

These are numbers, not rules. Each domain operator adjusts their own parameters. The minting domain controls minting rates. Other domains control their own fees, job rates, and AI behavior. Bilateral agreements handle cross-domain coordination. No conservation law or protocol constraint changes.

### Known Risks

**Credit inflation beyond mild.** If minting outpaces the economy's absorptive capacity, credits destabilize. Mitigated by activity-tied minting, transparent supply audits, and the fact that players can switch to commodity money if credits lose trust. The exit option IS the discipline.

**Early network dependency.** If the first five systems go down, new players can't enter. Mitigated by 5-system redundancy and eventually by player-run systems offering the same onboarding. The founding systems aren't special — they're just the first network. They should become optional, not permanent infrastructure.

**Price manipulation.** A wealthy player corners a resource. Expensive to sustain — you're buying real objects that cost real hosting to store. The frontier always provides alternatives. Cornering a market in a galaxy with infinite frontier is a losing strategy long-term.

**Exploration data asymmetry.** Early scouts know where the good unclaimed systems are. Real first-mover advantage. Intentional — rewarding exploration is the point. The galaxy is big enough that no one can scout it all.
