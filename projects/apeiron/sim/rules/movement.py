"""Movement rule. Agents move between locations, consuming fuel."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Callable


@dataclass
class Move:
    agent_id: str
    to: str


def fuel_cost_linear(distance: float, cargo_mass: float, base_rate: float = 1.0) -> float:
    return base_rate * distance * (1.0 + cargo_mass / 100.0)


def movement_rule(
    fuel_cost_fn: Callable[[float, float], float] | None = None,
    distances: dict[tuple[str, str], float] | None = None,
):
    """Create a movement rule with configurable fuel cost and distances.

    distances: {(from, to): distance} — if missing, assume distance=1.
    fuel_cost_fn: (distance, cargo_mass) -> fuel_consumed.
    """
    if fuel_cost_fn is None:
        fuel_cost_fn = lambda d, m: fuel_cost_linear(d, m)
    if distances is None:
        distances = {}

    def resolve(world, actions):
        for action in actions:
            if not isinstance(action, Move):
                continue
            agent = world.agents.get(action.agent_id)
            if not agent:
                continue
            if action.to not in world.locations:
                continue
            if agent.state.location == action.to:
                continue

            cargo_mass = sum(agent.state.inventory.values())
            key = (agent.state.location, action.to)
            dist = distances.get(key, distances.get((action.to, agent.state.location), 1.0))
            cost = fuel_cost_fn(dist, cargo_mass)

            fuel = agent.state.inventory.get("fuel", 0.0)
            if fuel < cost:
                outcomes = world.state.setdefault("outcomes", {})
                outcomes[action.agent_id] = {"action": "move", "to": action.to,
                                             "ok": False, "reason": "no_fuel"}
                continue

            agent.state.inventory["fuel"] = fuel - cost
            agent.state.location = action.to
            world.recorder.add(world.tick, f"{agent.id}.fuel_burned", cost)
            outcomes = world.state.setdefault("outcomes", {})
            outcomes[action.agent_id] = {"action": "move", "to": action.to,
                                         "fuel_cost": cost, "ok": True}

        return world

    return resolve
