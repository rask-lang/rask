# Market
<!-- id: apeiron.market --> <!-- status: proposed --> <!-- summary: Galaxy-wide order book over the network, local settlement for physical goods, instant for virtual assets -->

[ECONOMY.md](ECONOMY.md) defines bilateral trade between domains. That works — but it means every trade requires both parties to be at the same domain at the same time. Real markets don't work like this. Commodity markets let you place orders that execute when a counterparty shows up. Stock markets aggregate global supply and demand into prices that reflect information from everywhere.

Apeiron can have a galaxy-wide market without violating sovereignty or bilateral trust. The key insight: the order book is information. Settlement is physical. You can see prices and place orders from anywhere. But physical goods still need to be shipped.

## Two Classes of Goods

### Physical Goods

Materials, components, ships, fuel — Allgard objects with mass. These exist at a specific domain. They can't teleport. Trading physical goods means someone has to physically move them from seller to buyer. The transport cost IS the geographic friction that makes trade routes valuable.

Physical goods settle locally. The seller has the goods at their domain. The buyer (or a hired hauler) picks them up. The trade is bilateral — seller's domain facilitates the Transfer. Conservation laws apply to every step.

### Virtual Assets

Credits, knowledge objects, contracts, attestations, standing agreements — Allgard objects without meaningful mass. They represent value or information, not physical matter. Virtual assets can transfer over Leden without anyone moving a ship.

Virtual assets settle over the network. A credit transfer between two domains is a bilateral Allgard Transfer that happens at network speed (1-3 Leden round trips within established trust relationships). No transport needed. No fuel burned. The transfer is still bilateral and conservation-law-compliant — credits don't duplicate in transit. But the "shipping cost" is zero.

Recipes, blueprints, and doctrine are virtual. They have an object representation but no physical presence. Selling a recipe from system A to a buyer at system B requires only the Leden Transfer protocol, not a hauler.

## The Order Book

A market order is an Allgard object. It says: "I want to buy/sell X at price Y, with these terms."

### Order Format

```
market_order:
  id: <object_id>
  type: buy | sell
  issuer: <owner_id>
  domain: <domain_id>                 # Where the goods are (sell) or where to deliver (buy)
  
  # What
  item_type: <asset_type_hash>        # Standard asset type (e.g., "structural_steel", "recipe", "credits")
  quantity: 500
  
  # Price
  price_per_unit: 3.2                 # Credits per unit
  price_type: limit | market          # Limit: exact price. Market: best available.
  
  # Settlement
  settlement: local | network         # Physical goods = local. Virtual = network.
  escrow: <object_id>                 # Locked goods (sell) or locked credits (buy)
  
  # Terms
  expiry: <beacon_epoch>
  minimum_fill: 100                   # Don't partial-fill below this
  counterparty_requirements:
    min_reputation: 5                  # Minimum completed contracts with issuer
    faction: null                      # Optional: restrict to specific faction
    standing: neutral                  # Minimum standing level
```

### Placing Orders

A seller places a sell order by:
1. Creating the order object at their domain.
2. Locking the goods in escrow (conditional Transfer — goods are reserved for the order).
3. Publishing the order in their domain's market metadata.

A buyer places a buy order by:
1. Creating the order object at their domain (or at the target market domain).
2. Locking credits in escrow.
3. Publishing the order.

Orders are visible to anyone who can observe the market domain's metadata or who receives the order through market aggregation.

### Order Matching

**Local matching.** A domain matches buy and sell orders at its own market. The domain runs a matching script (Raido bytecode, content-addressed, verifiable). Standard matching: price-time priority. Sell at 3.0 meets buy at 3.2 — trade executes at 3.0 (seller's price, since it was posted first). The matching script is the market's rules.

**Cross-domain matching.** This is where it gets interesting. A sell order at domain A and a buy order at domain B. The order book is visible to both (through market metadata propagation). But the goods are at domain A and the buyer is at domain B.

For virtual assets: settlement happens over Leden. The buyer's credits transfer to the seller's domain. The virtual asset transfers to the buyer's domain. Two bilateral Transfers, coordinated by a shared order reference. Near-instant.

For physical goods: settlement requires transport. Three options:
1. **Buyer picks up.** Buyer (or their hired hauler) travels to seller's domain. Picks up goods. Bilateral Transfer at seller's domain.
2. **Seller delivers.** Seller (or their hired hauler) transports goods to buyer's domain. Bilateral Transfer at buyer's domain.
3. **Meet in the middle.** Both parties agree on a third domain for handoff. Useful when neither trusts the other's domain.

The trade itself (credit transfer for goods transfer) can execute immediately for the virtual part (credits move over the network). The physical part settles when the goods are actually delivered. The order's escrow holds until delivery confirmation.

## Market Aggregation

No single domain sees all orders. Markets are local by default. But aggregation makes them useful.

### Trade Hub Aggregation

A trade hub domain aggregates orders from nearby systems:

1. Hub establishes bilateral trade agreements with surrounding domains.
2. Each connected domain publishes its market orders as a Leden observation feed. The hub subscribes.
3. Hub maintains a combined order book — a read-only mirror of each connected domain's orders, plus orders placed directly at the hub.
4. Visitors to the hub see the aggregate book.

**Order ownership stays local.** The hub mirrors orders, it doesn't take custody. An order at domain A stays at domain A's escrow. The hub displays it and can initiate a match, but settlement goes through domain A. If domain A goes offline, the hub marks those orders as **stale** (unverified since last observation update). Stale orders show in the book with a warning — the hub can't guarantee the escrow still exists.

**Staleness protocol.** Each observation subscription has a heartbeat. If the hub doesn't receive an update from domain A within N epochs (configurable, probably 3-5), domain A's orders are marked stale. If the hub doesn't hear from domain A for 2N epochs, the orders are hidden. When domain A reconnects, the hub re-syncs the full order list.

**Hub profits by:**
- Charging a listing fee (credits per order posted through the hub)
- Taking a transaction fee (small percentage of matched trades brokered by the hub)
- Using the information advantage (the hub sees all orders, knows price trends before anyone else)

Multiple hubs can exist. They compete on coverage (how many domains connected), fees, staleness (how fresh the data is), and reliability. A hub that fabricates orders, manipulates matching, or fails to settle trades loses reputation and traders.

### Faction Markets

A faction can run an internal market — orders from all faction member domains, aggregated at the faction hub. Faction members trade with each other at preferential terms (lower fees, higher trust, faster settlement through pre-established bilateral trust).

This is a faction benefit: membership gives access to a larger, cheaper, more trusted market than any individual domain can offer.

### Galaxy-Wide Price Discovery

No single entity sees all prices. But information propagates:

1. Trade hubs aggregate regional prices.
2. Traders carry price information between hubs (visiting hub A, seeing prices at hub A, traveling to hub B where prices differ).
3. Market intel reports compile cross-hub price comparisons (see [KNOWLEDGE.md](KNOWLEDGE.md)).
4. AI traders arbitrage across hubs, narrowing price gaps to transport cost margins.

Over time, prices for standard commodities converge across hubs (minus transport costs). Exotic materials with fewer producers have wider price spreads — information is scarcer, arbitrage is riskier.

This IS the real-world commodity market model. London and New York gold prices differ by shipping cost plus insurance. Apeiron's iron prices between hub A and hub B differ by fuel cost plus risk premium.

## Market Data

Domains publish market data in navigation metadata (see [NAVIGATION.md](NAVIGATION.md)):

```
market:
  updated_epoch: <beacon_epoch>
  commodities:
    - type: "structural_steel"
      buy_price: 3.2                   # Best buy order
      sell_price: 3.5                  # Best sell order
      volume_24h: 5000                 # Units traded in last 24 epoch-hours
      supply: 12000                    # Units in local inventory
    - type: "hydrocarbon_fuel"
      buy_price: 2.4
      sell_price: 2.6
      volume_24h: 20000
      supply: 50000
  order_count: 47                      # Total open orders
```

This is summary data — not the full order book. Enough for navigation planning ("this system sells fuel at 2.5") but not enough for trading strategies. The full order book requires docking at the market domain or subscribing to its observation feed.

**Price history.** Domains that maintain price history (rolling averages, trends, peaks) provide a premium data product. A trade hub with 100 epochs of price history for 50 commodities is an information goldmine. This data is a knowledge product (see [KNOWLEDGE.md](KNOWLEDGE.md)).

## Escrow and Settlement

### For Virtual Assets

Near-instant. Both sides lock escrow (credits and virtual asset). The market matching script triggers two bilateral Transfers simultaneously. If either Transfer fails, both roll back. Standard Allgard conditional Transfer.

Settlement risk: minimal. Both escrows are locked before matching. The only failure mode is network partition during settlement — handled by Allgard timeout/recovery per [TRANSFER.md](../allgard/TRANSFER.md).

### For Physical Goods

Physical settlement is a three-party protocol: seller's domain (holds goods), buyer's domain (holds credits), and a delivery agent (moves goods). The delivery agent can be the buyer, the seller, or a third-party hauler.

**Step 1: Match.** The market matching script identifies a buy/sell pair. Both escrows are locked (goods at seller's domain, credits at buyer's domain). The match produces a **settlement ticket** — an Allgard object referencing both escrows, the delivery terms, and the deadline.

**Step 2: Claim.** A delivery agent claims the settlement ticket. The agent receives a conditional Grant from the seller's domain: pick up the escrowed goods. The agent also receives a conditional Grant from the buyer's domain: deliver goods and receive credits. Both Grants reference the settlement ticket and expire at the ticket's deadline.

**Step 3: Transport.** The agent picks up the goods (Transfer from seller's escrow to agent). Travels to buyer's domain. Delivers goods (Transfer from agent to buyer's domain).

**Step 4: Settle.** Buyer's domain confirms receipt (goods match the settlement ticket's item_type, quantity, and quality). Credits release from buyer's escrow to seller. The settlement ticket is marked complete.

**Failure modes — each with a specific resolution:**

| Failure | Who detects | Resolution |
|---------|-------------|------------|
| No agent claims ticket | Ticket deadline passes | Both escrows unlock. Seller relists goods. Buyer relists credits. Automatic — no human action. |
| Agent picks up but doesn't deliver | Buyer's domain sees no delivery by deadline | Buyer's credits return from escrow. Seller's goods are gone (agent has them). Seller's recourse: agent's escrow deposit (agents post a bond when claiming tickets) + negative attestation. |
| Goods destroyed in transit | Agent arrives without goods (or doesn't arrive) | Same as "doesn't deliver" — bond covers partial loss, attestation covers the rest. |
| Goods arrive but fail quality check | Buyer's domain rejects delivery | Goods return to agent. Agent can attempt re-delivery or return to seller. Credits stay in escrow until deadline. |
| Buyer's domain goes offline | Agent can't deliver | Agent holds goods until domain returns or deadline passes. On deadline: agent returns goods to seller's domain, both escrows unlock. |

**Agent bonds.** To claim a settlement ticket, the delivery agent locks a bond (credits or goods) at the seller's domain. The bond covers partial loss if the agent disappears with the goods. Bond size is set by the seller's domain — typically 10-30% of goods value. The bond returns when the settlement ticket completes successfully.

**Self-delivery.** If the buyer or seller IS the delivery agent, they skip the bond (they're already party to the trade). The buyer travels to pick up goods, or the seller travels to deliver. Same settlement steps, just one fewer party.

**Courier contract integration.** The delivery agent role maps directly to courier contracts per [CONTRACTS.md](CONTRACTS.md). A seller can post a sell order AND a linked courier contract. A hauler who fills the courier contract automatically claims the settlement ticket. The hauler's profit is the courier payment minus fuel costs. The market order and the courier contract share the same settlement ticket — they're one economic transaction, composed from two primitives.

## Commodity Standards

For a market to work, buyers and sellers must agree on what they're trading. "Structural steel" needs to mean the same thing across domains.

### Standard Asset Types

The founding cluster publishes standard asset type definitions:

```
asset_type:
  id: <hash>
  name: "Structural Steel"
  category: material
  properties:
    density: {min: 7.80, max: 7.90}
    hardness: {min: 0.70, max: 0.75}
    # ... property ranges that qualify as "structural steel"
  grade: standard
```

An object that falls within the property ranges qualifies as that asset type. Objects outside the ranges are "off-spec" — still tradeable, but not under the standard label. The market can list off-spec goods separately (usually at a discount).

**Grading.** Premium, standard, and substandard grades for the same asset type, based on tighter or looser property ranges. Premium structural steel (tighter range, more consistent) commands a higher price. Market orders can specify grade requirements.

**Custom types.** Domains can define their own asset types. A faction that discovers a novel alloy defines a new type with its own property ranges. The type is only recognized within domains that adopt the definition — initially just the faction, expanding as the alloy becomes traded.

## Stage 1 Testing

The monolith runs markets on all five founding systems:

- **Local market.** Each system has a market. AI traders place buy/sell orders. Verify matching, escrow, settlement.
- **Cross-domain orders.** Sell order at system A, buy order at system B. Verify credit transfer (virtual) executes while goods await physical delivery.
- **Trade hub.** One founding system acts as a hub, aggregating orders from neighbors. Verify the aggregate book shows orders from multiple systems.
- **Price discovery.** Start with seed prices. AI traders arbitrage. Verify prices converge across systems (minus transport cost margin).
- **Physical settlement.** AI hauler fills a cross-domain order. Verify: picks up goods at seller, transports, delivers to buyer, escrow releases.
- **Fuel cost affects prices.** Systems farther from the fuel hub have higher fuel prices. Verify this creates meaningful price differentials for heavy goods.
- **Market manipulation.** AI trader tries to corner a resource. Verify: expensive to sustain, frontier alternatives exist, eventually unprofitable.
- **Virtual asset trading.** Recipe sold from system A to buyer at system B. Verify instant settlement with no transport.

## Interaction With Other Systems

**Economy.** Markets are the economy's nervous system. [ECONOMY.md](ECONOMY.md) describes bilateral trade and job mechanics. This spec adds the aggregation layer that makes bilateral trade scale.

**Navigation.** Market data feeds navigation decisions. [NAVIGATION.md](NAVIGATION.md) defines how market summaries propagate through domain metadata. Traders plan routes based on price differentials visible in navigation data.

**Contracts.** Courier contracts ([CONTRACTS.md](CONTRACTS.md)) are the settlement mechanism for cross-domain physical good trades. The market creates the trades; contracts handle delivery.

**Knowledge.** Market intel — price histories, trend analyses, arbitrage maps — is a knowledge product per [KNOWLEDGE.md](KNOWLEDGE.md). The market creates the data; the knowledge economy trades in it.

**Reputation.** Market participants build reputation through completed trades. [REPUTATION.md](REPUTATION.md) attestations from market domains ("this trader completed 200 trades, zero disputes") are the trust basis for large orders.

## What This Spec Doesn't Cover

**Market microstructure.** Order types beyond limit/market (stop orders, iceberg orders, options). Financial engineering that emerges when markets get sophisticated. Defer — see if basic orders are sufficient.

**Currency exchange.** Trading credits for other currencies (fuel-backed tokens, faction scrip, commodity baskets). This is just trading one virtual asset for another — the market handles it. But exchange rate dynamics are complex and worth studying separately.

**Market regulation.** Do founding cluster markets ban wash trading? Enforce disclosure? Regulate monopolies? Domain policy. Each market domain sets its own rules. The founding cluster publishes recommended rules; domains adopt what works. Regulatory arbitrage (moving to lax markets) is a natural force — same as in real financial markets.

**Derivatives.** Futures, options, insurance products traded as market instruments. Technically just virtual asset types with expiry and conditional payouts. Could compose from contracts and market orders. Complex enough to deserve its own spec if anyone builds it.
