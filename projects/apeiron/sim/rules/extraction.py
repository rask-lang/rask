"""Extraction rule. Agents mine deposits at their location."""

from __future__ import annotations

from dataclasses import dataclass


@dataclass
class Extract:
    agent_id: str
    element: str
    amount: float


def extraction_rule(rate_limit: float = 1000.0):
    """Create an extraction rule.

    rate_limit: max units extractable per agent per tick.
    Deposits live in location.state["deposits"] as {element: remaining}.
    """
    def resolve(world, actions):
        for action in actions:
            if not isinstance(action, Extract):
                continue
            agent = world.agents.get(action.agent_id)
            if not agent:
                continue
            loc = world.locations.get(agent.state.location)
            if not loc:
                continue

            deposits = loc.state.get("deposits", {})
            available = deposits.get(action.element, 0.0)
            if available <= 0:
                continue

            amount = min(action.amount, available, rate_limit)
            deposits[action.element] = available - amount
            old = agent.state.inventory.get(action.element, 0.0)
            agent.state.inventory[action.element] = old + amount

            world.recorder.add(
                world.tick,
                f"extracted.{action.element}",
                amount,
            )

        return world

    return resolve
