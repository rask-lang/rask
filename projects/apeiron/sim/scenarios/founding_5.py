"""Founding 5 scenario: 5 systems from galaxy gen, economic agents, all rules.

Bridges galaxy generation into a configured World ready to run.
"""

from __future__ import annotations

from engine.agent import AgentState, make_agent
from engine.world import World, Location
from engine.constraints import Constraints
from galaxy.generate import generate, distance
from galaxy.resources import assign_deposits, ELEMENTS_14, ELEMENTS_5
from galaxy.topology import build_graph
from rules.movement import movement_rule
from rules.extraction import Extract, extraction_rule
from rules.trade import Buy, Sell, trade_rule


def _pick_founding_systems(stars, deposits, n=5):
    """Pick n well-connected, resource-diverse systems from a dense cluster."""
    # Find the largest cluster
    from collections import Counter
    cluster_counts = Counter(s.cluster for s in stars if s.cluster >= 0)
    if not cluster_counts:
        return stars[:n], deposits[:n]
    biggest_cluster = cluster_counts.most_common(1)[0][0]
    candidates = [(s, deposits[s.id]) for s in stars if s.cluster == biggest_cluster]

    # Sort by resource diversity (most distinct elements first)
    candidates.sort(key=lambda x: -len(x[1]))

    # Pick top n that are close to each other
    picked = [candidates[0]]
    for s, d in candidates[1:]:
        if len(picked) >= n:
            break
        # Must be within reasonable distance of at least one picked star
        if any(distance(s, p[0]) < 100 for p in picked):
            picked.append((s, d))

    # Pad if we didn't get enough
    for s, d in candidates:
        if len(picked) >= n:
            break
        if (s, d) not in picked:
            picked.append((s, d))

    return [s for s, d in picked[:n]], [d for s, d in picked[:n]]


def _extractor_behavior(agent_id, element="iron"):
    """Extract available element, sell surplus when inventory > 500."""
    obs = yield
    while True:
        inv = obs.get("inventory", {})
        deposits = obs.get("location_state", {}).get("deposits", {})

        if deposits.get(element, 0) > 0:
            obs = yield Extract(agent_id, element, 200.0)
        elif inv.get(element, 0) > 0:
            obs = yield Sell(agent_id, element, inv[element], 1.0)
        else:
            obs = yield None

        # Sell surplus
        inv = obs.get("inventory", {})
        if inv.get(element, 0) > 500:
            obs = yield Sell(agent_id, element, inv[element] - 200, 1.5)
        else:
            obs = yield None


def _station_behavior(agent_id, buy_items=None):
    """Market maker: buy what's cheap, sell what's in stock."""
    if buy_items is None:
        buy_items = ["iron", "carbon", "hydrogen"]
    obs = yield
    while True:
        inv = obs.get("inventory", {})
        for item in buy_items:
            if inv.get(item, 0) < 1000:
                obs = yield Buy(agent_id, item, 100.0, 3.0)
            elif inv.get(item, 0) > 2000:
                obs = yield Sell(agent_id, item, 500.0, 2.0)
            else:
                obs = yield None


def build(
    seed: int = 42,
    num_stars: int = 500,
    num_systems: int = 5,
    elements: list | None = None,
    jump_range: float = 60.0,
) -> World:
    """Build a founding-5 scenario from galaxy generation."""
    if elements is None:
        elements = ELEMENTS_14

    stars = generate(seed=seed, num_stars=num_stars, num_clusters=5)
    all_deposits = assign_deposits(num_stars, elements, seed=seed + 1)
    founding_stars, founding_deposits = _pick_founding_systems(
        stars, all_deposits, n=num_systems
    )

    # Build locations
    locations = {}
    for star, deps in zip(founding_stars, founding_deposits):
        loc_id = f"sys_{star.id}"
        locations[loc_id] = Location(
            id=loc_id,
            position=(star.x, star.y, star.z),
            state={"deposits": dict(deps), "star_id": star.id},
        )

    # Set up neighbor lists based on jump range
    loc_ids = list(locations.keys())
    for i, lid_a in enumerate(loc_ids):
        for lid_b in loc_ids[i + 1:]:
            sa = founding_stars[i]
            sb = founding_stars[loc_ids.index(lid_b)]
            if distance(sa, sb) <= jump_range:
                locations[lid_a].neighbors.append(lid_b)
                locations[lid_b].neighbors.append(lid_a)

    # Build distances for movement rule
    distances = {}
    for i, (sa, lid_a) in enumerate(zip(founding_stars, loc_ids)):
        for sb, lid_b in zip(founding_stars[i + 1:], loc_ids[i + 1:]):
            d = distance(sa, sb)
            distances[(lid_a, lid_b)] = d

    # Create agents — 1 extractor + 1 station per system
    agents = {}
    for idx, (loc_id, deps) in enumerate(zip(loc_ids, founding_deposits)):
        # Extractor mines the most abundant element
        if deps:
            best_elem = max(deps, key=deps.get)
            ext_id = f"extractor_{idx}"
            agents[ext_id] = make_agent(
                ext_id, _extractor_behavior,
                AgentState(location=loc_id, inventory={"fuel": 500.0}),
                element=best_elem,
            )

        # Station buys common resources
        stn_id = f"station_{idx}"
        agents[stn_id] = make_agent(
            stn_id, _station_behavior,
            AgentState(location=loc_id, credits=5000.0,
                       inventory={"fuel": 200.0}),
        )

    # Metrics hook: record totals each tick
    def record_totals(world):
        total_credits = sum(a.state.credits for a in world.agents.values())
        total_fuel = sum(a.state.inventory.get("fuel", 0) for a in world.agents.values())
        world.recorder.add(world.tick, "total_credits", total_credits)
        world.recorder.add(world.tick, "total_fuel", total_fuel)
        for loc in world.locations.values():
            deps = loc.state.get("deposits", {})
            for elem, qty in deps.items():
                world.recorder.add(world.tick, f"deposit.{loc.id}.{elem}", qty)

    return World(
        locations=locations,
        agents=agents,
        rules=[extraction_rule(), trade_rule(), movement_rule(distances=distances)],
        metrics_hooks=[record_totals],
        constraints=Constraints(visibility="local"),
    )
