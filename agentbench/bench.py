#!/usr/bin/env python3
# SPDX-License-Identifier: (MIT OR Apache-2.0)
"""agentbench — measure how well models write Rask against this compiler.

    ./agentbench/bench.py tasks
    ./agentbench/bench.py selftest                 # do the reference solutions still build?
    ./agentbench/bench.py run --model mock:solves@2
    ./agentbench/bench.py run --model cli:claude-sonnet-5 --yes-spend
    ./agentbench/bench.py report agentbench/runs/<run>

`selftest` costs nothing and answers the question the gates can't: is a red task
the model's fault or the compiler's. `run` against a real model costs money and
refuses to start without --yes-spend.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from agentbench.lib import attempts, models, rask, report, tasks  # noqa: E402

BENCH_ROOT = Path(__file__).resolve().parent
RUNS = BENCH_ROOT / "runs"


def _commit() -> str:
    try:
        out = subprocess.run(["git", "rev-parse", "--short", "HEAD"],
                             cwd=BENCH_ROOT.parent, capture_output=True, text=True, timeout=10)
        return out.stdout.strip() or "unknown"
    except Exception:
        return "unknown"


# --- tasks ------------------------------------------------------------------

def cmd_tasks(args) -> int:
    selected = tasks.load_tasks(args.select)
    quarantine = tasks.quarantined()
    print(f"{len(selected)} task(s)\n")
    for task in selected:
        mark = "  [quarantined]" if task.id in quarantine else ""
        print(f"  {task.horizon:<6} {task.id:<32} {task.title}{mark}")
        if args.verbose:
            print(f"         concepts: {', '.join(task.concepts)}")
            if task.issues:
                print(f"         issues:   {', '.join(task.issues)}")
    return 0


# --- selftest ---------------------------------------------------------------

def cmd_selftest(args) -> int:
    """Compile every reference solution on both backends.

    This is the benchmark's own gate. A task whose reference has gone red is
    measuring the compiler, not the model, and has to be quarantined before the
    scores mean anything.
    """
    selected = tasks.load_tasks(args.select)
    quarantine = tasks.quarantined()
    rask_bin = rask.find_rask()
    print(f"selftest: {len(selected)} reference solution(s) against {rask_bin}\n")

    def check(task):
        return task, attempts.check_reference(task, rask_bin, timeout=args.timeout)

    green, expected_red, unexpected_red, unexpected_green = 0, 0, [], []
    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        for task, verdict in pool.map(check, selected):
            listed = task.id in quarantine
            if verdict.ok and not listed:
                green += 1
                continue
            if verdict.ok and listed:
                unexpected_green.append(task.id)
                print(f"UNEXPECTED PASS: {task.id} — prune it from quarantine.txt")
                continue
            if listed:
                expected_red += 1
                print(f"QUARANTINED: {task.id} [{verdict.outcome}] {quarantine[task.id]}")
                continue
            unexpected_red.append(task.id)
            print(f"BROKEN:     {task.id} [{verdict.outcome}] — the reference no longer works")
            print("  " + verdict.feedback(14).replace("\n", "\n  "))

    print("\n" + "─" * 50)
    print(f"selftest: {green} green, {expected_red} quarantined, "
          f"{len(unexpected_red)} broken, {len(unexpected_green)} unexpected-pass")
    if unexpected_red:
        print(f"BROKEN (fix the task, or quarantine it with an issue): {', '.join(unexpected_red)}")
    if unexpected_green:
        print(f"NOW PASSING — prune from quarantine.txt: {', '.join(unexpected_green)}")
    return 1 if unexpected_red or unexpected_green else 0


# --- run --------------------------------------------------------------------

def cmd_run(args) -> int:
    selected = [t for t in tasks.load_tasks(args.select)
                if args.include_quarantined or t.id not in tasks.quarantined()]
    if args.limit:
        selected = selected[: args.limit]
    if not selected:
        print("no tasks selected", file=sys.stderr)
        return 2

    probe = models.build(args.model, "")
    if probe.paid and not args.yes_spend:
        _print_estimate(selected, args)
        print("\nRefusing to spend without --yes-spend.", file=sys.stderr)
        return 2

    rask_bin = rask.find_rask()
    started = time.strftime("%Y-%m-%dT%H:%M:%S")
    stamp = time.strftime("%Y%m%d-%H%M%S")
    out_dir = Path(args.out) if args.out else RUNS / f"{stamp}-{args.model.replace(':', '-')}"

    meta = {
        "model": args.model,
        "rask": str(rask_bin),
        "commit": _commit(),
        "max_attempts": args.max_attempts,
        "started": started,
        "tasks": [t.id for t in selected],
    }
    print(f"agentbench: {len(selected)} task(s), model {args.model}, "
          f"budget {args.max_attempts} attempt(s)")
    print(f"  compiler: {rask_bin} @ {meta['commit']}")
    print(f"  output:   {out_dir}\n")

    def run_one(task):
        model = models.build(args.model, task.reference)
        return attempts.run_task(task, model, max_attempts=args.max_attempts,
                                 rask_bin=rask_bin, timeout=args.timeout,
                                 feedback_lines=args.feedback_lines)

    results = []
    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        for result in pool.map(run_one, selected):
            results.append(result)
            state = "solved" if result.solved else (
                result.attempts[-1].outcome if result.attempts else "no attempt")
            print(f"  {result.task_id:<32} {state:<14} {result.attempts_used} attempt(s)"
                  f"{'  [BACKEND DIVERGENCE]' if result.saw_divergence else ''}")

    results.sort(key=lambda r: (r.horizon, r.task_id))
    scored = report.score(results)
    report.write_run(out_dir, scored, results, {t.id: t for t in selected}, meta)

    print("\n" + "─" * 50)
    print(f"solve rate {scored.solved}/{scored.total} ({scored.solve_rate:.0%}) · "
          f"pass@1 {scored.pass_at_1:.0%} · convergence {scored.convergence:.2f}")
    if scored.divergent_tasks:
        print(f"BACKEND DIVERGENCES (compiler bugs found): {', '.join(scored.divergent_tasks)}")
    if scored.cost_usd:
        print(f"cost ${scored.cost_usd:.2f}")
    print(f"report: {out_dir / 'report.md'}")
    return 0


def _print_estimate(selected, args) -> None:
    """Rough cost ceiling, so nobody starts a run blind.

    Worst case means every task burning its whole attempt budget. Retries are
    the expensive part: the conversation carries forward, so attempt k resends
    the card, the task, and every earlier exchange.
    """
    card = len(attempts.system_prompt()) // 4
    task_tokens, reply_tokens, feedback_tokens = 400, 900, 500
    total_in = total_out = 0
    for _ in selected:
        carried = 0
        for _ in range(args.max_attempts):
            total_in += card + task_tokens + carried
            total_out += reply_tokens
            carried += reply_tokens + feedback_tokens
    print(f"{len(selected)} task(s) × up to {args.max_attempts} attempt(s)")
    print(f"  worst case ≈ {total_in:,} input + {total_out:,} output tokens")
    print(f"  (the language card is ~{card:,} of the input tokens, resent every attempt)")
    for name, (rate_in, rate_out) in sorted(models.PRICES.items()):
        print(f"  at {name} rates: ${(total_in * rate_in + total_out * rate_out) / 1e6:.2f}")
    print("  a solved task stops early, so a healthy run lands well under this")


# --- report -----------------------------------------------------------------

def cmd_report(args) -> int:
    """Re-render a finished run — new metrics over old transcripts, no new spend."""
    run_dir = Path(args.run)
    payload = json.loads((run_dir / "run.json").read_text(encoding="utf-8"))
    results = [_result_from_json(r) for r in payload["results"]]
    scored = report.score(results)
    by_id = {t.id: t for t in tasks.load_tasks(None)}
    missing = [r.task_id for r in results if r.task_id not in by_id]
    if missing:
        print(f"note: task definitions gone for {', '.join(missing)}; "
              "transcripts for those keep their old text", file=sys.stderr)
    report.write_run(run_dir, scored, [r for r in results if r.task_id in by_id],
                     by_id, payload["meta"])
    print(report.render_report(scored, results, payload["meta"]))
    return 0


def _result_from_json(row: dict) -> attempts.TaskResult:
    result = attempts.TaskResult(
        task_id=row["task_id"], title=row["title"], horizon=row["horizon"],
        concepts=row.get("concepts", []), solved=row["solved"],
        attempts_used=row["attempts_used"], seconds=row.get("seconds", 0.0))
    result.attempts = [attempts.Attempt(**a) for a in row["attempts"]]
    return result


# --- entry ------------------------------------------------------------------

def main(argv=None) -> int:
    parser = argparse.ArgumentParser(prog="bench.py", description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)

    p = sub.add_parser("tasks", help="list the task set")
    p.add_argument("--select", help="horizon, task id, concept, or substring (comma-separated)")
    p.add_argument("-v", "--verbose", action="store_true")
    p.set_defaults(func=cmd_tasks)

    p = sub.add_parser("selftest", help="check every reference solution on both backends")
    p.add_argument("--select")
    p.add_argument("--jobs", type=int, default=8)
    p.add_argument("--timeout", type=float, default=120.0)
    p.set_defaults(func=cmd_selftest)

    p = sub.add_parser("run", help="run the benchmark")
    p.add_argument("--model", default="mock:solves@2",
                   help="mock:reference | mock:solves@N | mock:garbage | cli:<model> | api:<model>")
    p.add_argument("--select")
    p.add_argument("--limit", type=int)
    p.add_argument("--max-attempts", type=int, default=3)
    p.add_argument("--jobs", type=int, default=4)
    p.add_argument("--timeout", type=float, default=120.0)
    p.add_argument("--feedback-lines", type=int, default=60,
                   help="how many lines of compiler output the model gets back")
    p.add_argument("--out", help="run directory (default agentbench/runs/<stamp>-<model>)")
    p.add_argument("--include-quarantined", action="store_true",
                   help="also run tasks whose reference is known broken")
    p.add_argument("--yes-spend", action="store_true",
                   help="required for cli:/api: models — this run costs money")
    p.set_defaults(func=cmd_run)

    p = sub.add_parser("report", help="re-score a finished run from its run.json")
    p.add_argument("run")
    p.set_defaults(func=cmd_report)

    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
