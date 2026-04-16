"""Trade rule. Agents post buy/sell orders, matched within a location."""

from __future__ import annotations

from dataclasses import dataclass


@dataclass
class Buy:
    agent_id: str
    item: str
    qty: float
    max_price: float


@dataclass
class Sell:
    agent_id: str
    item: str
    qty: float
    min_price: float


def trade_rule():
    """Create a trade rule. Price-time priority matching per location.

    Buys and sells at the same location are matched. The trade price
    is the average of the buy max and sell min (split the spread).
    Partial fills are allowed.
    """
    def resolve(world, actions):
        buys_by_loc: dict[str, list[Buy]] = {}
        sells_by_loc: dict[str, list[Sell]] = {}

        for action in actions:
            if isinstance(action, Buy):
                agent = world.agents.get(action.agent_id)
                if agent:
                    loc = agent.state.location
                    buys_by_loc.setdefault(loc, []).append(action)
            elif isinstance(action, Sell):
                agent = world.agents.get(action.agent_id)
                if agent:
                    loc = agent.state.location
                    sells_by_loc.setdefault(loc, []).append(action)

        all_locs = set(buys_by_loc) | set(sells_by_loc)
        for loc in all_locs:
            buys = sorted(buys_by_loc.get(loc, []),
                          key=lambda b: -b.max_price)  # highest bid first
            sells = sorted(sells_by_loc.get(loc, []),
                           key=lambda s: s.min_price)  # lowest ask first

            buy_remaining = {id(b): b.qty for b in buys}
            sell_remaining = {id(s): s.qty for s in sells}

            for buy in buys:
                if buy_remaining[id(buy)] <= 0:
                    continue
                for sell in sells:
                    if sell_remaining[id(sell)] <= 0:
                        continue
                    if buy.max_price < sell.min_price:
                        break  # no more matches possible
                    if buy.agent_id == sell.agent_id:
                        continue
                    if buy.item != sell.item:
                        continue

                    buyer = world.agents[buy.agent_id]
                    seller = world.agents[sell.agent_id]

                    trade_qty = min(
                        buy_remaining[id(buy)],
                        sell_remaining[id(sell)],
                        seller.state.inventory.get(sell.item, 0.0),
                    )
                    if trade_qty <= 0:
                        continue

                    price = (buy.max_price + sell.min_price) / 2.0
                    total_cost = price * trade_qty

                    if buyer.state.credits < total_cost:
                        affordable = buyer.state.credits / price if price > 0 else 0
                        trade_qty = min(trade_qty, affordable)
                        total_cost = price * trade_qty
                    if trade_qty <= 0:
                        continue

                    # Execute trade
                    seller.state.inventory[sell.item] = (
                        seller.state.inventory.get(sell.item, 0.0) - trade_qty
                    )
                    buyer.state.inventory[buy.item] = (
                        buyer.state.inventory.get(buy.item, 0.0) + trade_qty
                    )
                    buyer.state.credits -= total_cost
                    seller.state.credits += total_cost

                    buy_remaining[id(buy)] -= trade_qty
                    sell_remaining[id(sell)] -= trade_qty

                    world.recorder.add(world.tick, f"trade.{buy.item}.qty", trade_qty)
                    world.recorder.add(world.tick, f"trade.{buy.item}.price", price)

        return world

    return resolve
