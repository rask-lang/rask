"""Tunable constraint knobs. Not protocol simulation — just parameters."""

from __future__ import annotations

from dataclasses import dataclass


@dataclass
class Constraints:
    # What market/state data agents can see.
    # "local": own location only
    # "regional": locations within `region_hops` hops
    # "global": everything
    visibility: str = "local"
    region_hops: int = 2

    # Who agents can trade with.
    # "local": same location only
    # "neighbors": adjacent locations
    # "global": anywhere
    trade_reach: str = "local"

    # How many ticks old the price/state data is for non-local observations.
    info_delay: int = 0

    # Max actions per agent per tick. 0 = unlimited.
    rate_limit: int = 0
