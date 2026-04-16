"""World: locations, agents, rules, tick loop.

A rule is a callable: rule(world, actions) -> world.
The engine doesn't know what actions or rules exist. It just runs them.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Callable

from .agent import Agent
from .constraints import Constraints
from .record import Recorder


@dataclass
class Location:
    """A place in the world. Freeform state — rules define what's in it."""
    id: str
    position: tuple[float, ...] = (0.0, 0.0)
    state: dict[str, Any] = field(default_factory=dict)
    neighbors: list[str] = field(default_factory=list)


Rule = Callable[["World", list], "World"]
ObservationBuilder = Callable[["World", Agent], dict]


def default_observation(world: World, agent: Agent) -> dict:
    """Minimal observation: agent's own state + current location state."""
    loc = world.locations.get(agent.state.location)
    return {
        "tick": world.tick,
        "agent_id": agent.id,
        "location": loc.id if loc else "",
        "location_state": dict(loc.state) if loc else {},
        "inventory": dict(agent.state.inventory),
        "credits": agent.state.credits,
    }


@dataclass
class World:
    locations: dict[str, Location] = field(default_factory=dict)
    agents: dict[str, Agent] = field(default_factory=dict)
    rules: list[Rule] = field(default_factory=list)
    constraints: Constraints = field(default_factory=Constraints)
    recorder: Recorder = field(default_factory=Recorder)
    observe: ObservationBuilder = field(default=default_observation)
    tick: int = 0
    state: dict[str, Any] = field(default_factory=dict)


def step(world: World) -> World:
    """Advance one tick. Collect actions from agents, resolve via rules."""
    actions = []
    for agent in world.agents.values():
        obs = world.observe(world, agent)
        action = agent.step(obs)
        if action is not None:
            actions.append(action)

    for rule in world.rules:
        world = rule(world, actions)

    world.tick += 1
    return world


def run(world: World, ticks: int) -> World:
    """Run the simulation for N ticks."""
    for _ in range(ticks):
        world = step(world)
    return world
