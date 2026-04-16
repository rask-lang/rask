"""Parameter sweep runner. Cartesian product of params, multiprocessing Pool."""

from __future__ import annotations

import itertools
from multiprocessing import Pool
from typing import Any, Callable


def _run_one(args: tuple[Callable, dict]) -> dict:
    func, params = args
    result = func(**params)
    return {"params": params, "result": result}


def sweep(
    func: Callable[..., Any],
    params: dict[str, list],
    workers: int = 4,
) -> list[dict]:
    """Run func for every combination of params. Returns list of {params, result}.

    Example:
        results = sweep(
            func=run_scenario,
            params={"seed": [1, 2, 3], "visibility": ["local", "global"]},
            workers=4,
        )
        # runs 6 combinations across 4 workers
    """
    keys = list(params.keys())
    combos = list(itertools.product(*params.values()))
    jobs = [(func, dict(zip(keys, combo))) for combo in combos]

    if workers <= 1:
        return [_run_one(job) for job in jobs]

    with Pool(workers) as pool:
        return pool.map(_run_one, jobs)
