"""Run a scenario, record metrics, optionally save plots."""

from __future__ import annotations

import argparse
import importlib
import sys
import time

from engine.world import run


def main():
    parser = argparse.ArgumentParser(description="Run a world simulation")
    parser.add_argument("--scenario", default="founding_5",
                        help="Scenario module name in scenarios/")
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--ticks", type=int, default=1000)
    parser.add_argument("--out", default=None,
                        help="Output directory for CSV + plots")
    args = parser.parse_args()

    mod = importlib.import_module(f"scenarios.{args.scenario}")
    world = mod.build(seed=args.seed)

    print(f"scenario={args.scenario} seed={args.seed} ticks={args.ticks}")
    print(f"  locations: {len(world.locations)}")
    print(f"  agents: {len(world.agents)}")
    print(f"  rules: {len(world.rules)}")

    t0 = time.time()
    world = run(world, args.ticks)
    elapsed = time.time() - t0

    print(f"  completed in {elapsed:.2f}s ({args.ticks / elapsed:.0f} ticks/s)")
    print(f"  recorded {len(world.recorder.entries)} metric entries")

    # Summary
    keys = world.recorder.keys()
    total_credits = world.recorder.last("total_credits")
    total_fuel = world.recorder.last("total_fuel")
    if total_credits is not None:
        print(f"  total_credits: {total_credits:.0f}")
    if total_fuel is not None:
        print(f"  total_fuel: {total_fuel:.0f}")

    trade_keys = [k for k in keys if k.startswith("trade.") and k.endswith(".qty")]
    if trade_keys:
        total_traded = sum(
            v for _, v in world.recorder.series(trade_keys[0])
        ) if trade_keys else 0
        print(f"  total_trade_volume: {total_traded:.0f}")

    if args.out:
        import os
        os.makedirs(args.out, exist_ok=True)
        csv_path = os.path.join(args.out, "metrics.csv")
        world.recorder.to_csv(csv_path)
        print(f"  saved {csv_path}")

        from engine.plot import time_series
        for key in ["total_credits", "total_fuel"]:
            s = world.recorder.series(key)
            if s:
                path = os.path.join(args.out, f"{key}.png")
                time_series({key: s}, path, title=key)
                print(f"  saved {path}")


if __name__ == "__main__":
    main()
