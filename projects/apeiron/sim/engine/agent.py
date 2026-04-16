"""Agent: an ID, mutable state, and a generator that yields actions.

Generator protocol:
    def my_behavior(agent_id):
        obs = yield           # prime: wait for first observation
        while True:
            obs = yield action  # yield action, receive next observation

Each gen.send(obs) returns one action (or None to idle).
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Generator


@dataclass
class AgentState:
    """Freeform agent state. Lives in the world, not in the generator."""
    location: str = ""
    inventory: dict[str, float] = field(default_factory=dict)
    credits: float = 0.0
    extra: dict[str, Any] = field(default_factory=dict)


@dataclass
class Agent:
    id: str
    state: AgentState
    gen: Generator

    def step(self, observation: dict) -> Any | None:
        """Send observation to generator, get back an action or None."""
        return self.gen.send(observation)


def make_agent(
    id: str,
    behavior: Any,
    state: AgentState | None = None,
    **kwargs,
) -> Agent:
    """Create an agent from a generator function.

    behavior is a generator function:
        def my_behavior(agent_id, **kwargs):
            obs = yield                       # prime
            while True:
                obs = yield SomeAction(...)   # act + receive next obs
    """
    if state is None:
        state = AgentState()
    gen = behavior(id, **kwargs)
    next(gen)  # prime: advance to first yield
    return Agent(id=id, state=state, gen=gen)
