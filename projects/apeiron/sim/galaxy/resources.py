"""Assign resource deposits to stars based on element configs."""

from __future__ import annotations

import random
from dataclasses import dataclass


@dataclass
class Element:
    name: str
    tier: str  # "common", "strategic", "exotic", "synthetic"
    abundance: float  # probability a star has this element (0-1)
    quantity_range: tuple[float, float]  # (min, max) deposit size


ELEMENTS_5 = [
    Element("iron", "common", 0.95, (10_000, 100_000)),
    Element("carbon", "common", 0.90, (10_000, 80_000)),
    Element("hydrogen", "common", 0.95, (20_000, 200_000)),
    Element("silicon", "common", 0.90, (10_000, 80_000)),
    Element("copper", "common", 0.85, (5_000, 50_000)),
]

ELEMENTS_14 = [
    # Common (90-100% of systems, 100K-1M units)
    Element("iron", "common", 0.95, (100_000, 1_000_000)),
    Element("carbon", "common", 0.92, (100_000, 800_000)),
    Element("hydrogen", "common", 0.98, (200_000, 2_000_000)),
    Element("silicon", "common", 0.90, (100_000, 800_000)),
    Element("copper", "common", 0.88, (50_000, 500_000)),
    Element("aluminum", "common", 0.90, (100_000, 800_000)),
    Element("sulfur", "common", 0.85, (50_000, 400_000)),
    # Strategic bulk (60-80%)
    Element("titanium", "strategic", 0.70, (10_000, 100_000)),
    # Strategic trace (30-50%)
    Element("chromium", "strategic", 0.40, (1_000, 10_000)),
    Element("tungsten", "strategic", 0.35, (1_000, 10_000)),
    Element("gold", "strategic", 0.30, (1_000, 10_000)),
    # Exotic (5-15%)
    Element("uranium", "exotic", 0.10, (100, 1_000)),
    Element("platinum", "exotic", 0.08, (100, 1_000)),
    # Synthetic (never natural)
    Element("plutonium", "synthetic", 0.0, (0, 0)),
]


def assign_deposits(
    num_stars: int,
    elements: list[Element],
    seed: int,
) -> list[dict[str, float]]:
    """For each star, roll which elements are present and how much.

    Returns list of dicts: star_index -> {element_name: quantity}.
    """
    rng = random.Random(seed)
    deposits = []
    for _ in range(num_stars):
        star_deposits = {}
        for elem in elements:
            if elem.abundance > 0 and rng.random() < elem.abundance:
                lo, hi = elem.quantity_range
                qty = rng.uniform(lo, hi)
                star_deposits[elem.name] = qty
        deposits.append(star_deposits)
    return deposits
