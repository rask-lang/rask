# SPDX-License-Identifier: (MIT OR Apache-2.0)
"""Scoring and transcripts.

Four numbers carry the run:

  solve rate    fraction of tasks correct on both backends within the attempt budget
  pass@1        fraction correct on the first try — the "does the language read
                right" number, uncontaminated by the compiler's error messages
  convergence   mean attempts among solved tasks; 1.0 is perfect
  divergences   attempts where the two backends disagreed. Not the model's fault
                and not scored against it — these are compiler bugs the benchmark
                found, and each one belongs in tests/suite as a probe

Plus one that is really a diagnostics metric: **teach rate**, per error code. A
code has an opportunity every time it appears in an attempt that has a successor;
it's cleared if it's gone from the next attempt. NORTH_STAR commitment 4 says
every rule teaches — a code with a low teach rate is a message that doesn't, and
that is a bug to file, not a model to blame.
"""

from __future__ import annotations

import json
from collections import Counter, defaultdict
from dataclasses import dataclass, field
from pathlib import Path

from . import rask
from .attempts import TaskResult


# Targets, in the style of specs/METRICS.md — each a number a run either hits or
# doesn't, so "is the compiler stable enough" stops being a matter of opinion.
#
#   ASR  agent solve rate      solved / total                    ≥ 0.95 (day+week)
#   AC   agent convergence     mean attempts among solved        ≤ 1.5
#   BD   backend divergence    tasks that hit one / total        = 0
#   TR   teach rate            per code seen 3+ times            ≥ 0.7
#
# BD is the exit criterion the compiler never had. A run where models write
# nineteen ordinary programs and not one of them exposes a backend disagreement
# is evidence "native is stable" in a way a bug count isn't. The first run of
# this set scored BD = 2/19 before a model was ever called — both references
# tripped one on the way in (#1000, #1002).
TARGETS = {
    "ASR": 0.95,
    "AC": 1.5,
    "BD": 0.0,
    "TR": 0.7,
}
# A code needs this many retry opportunities before its teach rate means anything.
TEACH_MIN_SAMPLES = 3


@dataclass
class Score:
    total: int
    solved: int
    pass_at_1: float
    solve_rate: float
    convergence: float
    outcomes: Counter
    divergent_tasks: list[str]
    thrash_tasks: list[str]
    teach: dict[str, dict]
    cost_usd: float
    input_tokens: int
    output_tokens: int
    seconds: float
    by_horizon: dict[str, dict]
    # (task id, why) for tasks the provider never answered for. Scored nowhere,
    # reported plainly, because a quota wall that reads as "the model failed"
    # is the worst possible way to lose a measurement.
    aborted_tasks: list = field(default_factory=list)
    waited_seconds: float = 0.0
    billing: str = "free"


def score(results: list[TaskResult], billing: str = "free") -> Score:
    # An aborted task is one the provider never answered for — quota gone, the
    # login bad, the transport broken. Scoring it would report a plan limit as
    # a language failure, so it leaves the denominators entirely. Its finished
    # attempts still count toward teach rate and divergences: those are real
    # measurements of the compiler regardless of how the task ended.
    aborted = [r for r in results if r.aborted]
    scored = [r for r in results if not r.aborted]

    total = len(scored)
    solved = [r for r in scored if r.solved]
    first_try = [r for r in scored if r.solved and r.attempts_used == 1]
    outcomes = Counter(a.outcome for r in results for a in r.attempts)

    divergent = [r.task_id for r in results if r.saw_divergence]
    thrash = [r.task_id for r in scored if _thrashed(r)]

    return Score(
        total=total,
        solved=len(solved),
        pass_at_1=len(first_try) / total if total else 0.0,
        solve_rate=len(solved) / total if total else 0.0,
        convergence=(sum(r.attempts_used for r in solved) / len(solved)) if solved else 0.0,
        outcomes=outcomes,
        divergent_tasks=divergent,
        thrash_tasks=thrash,
        teach=teach_rates(results),
        cost_usd=sum(r.cost_usd for r in results),
        input_tokens=sum(a.input_tokens for r in results for a in r.attempts),
        output_tokens=sum(a.output_tokens for r in results for a in r.attempts),
        seconds=sum(r.seconds for r in results),
        by_horizon=_by_horizon(scored),
        aborted_tasks=[(r.task_id, r.aborted) for r in aborted],
        waited_seconds=sum(r.waited_seconds for r in results),
        billing=billing,
    )


def check_targets(score: "Score") -> list[tuple[str, str, str, bool]]:
    """(name, target, actual, met) per target. Day and week only for ASR — the
    month tasks are the frontier, and holding them to the same bar would make the
    number say "the hard tasks are hard" instead of "ordinary programs work"."""
    core = {h: score.by_horizon[h] for h in ("day", "week") if h in score.by_horizon}
    core_total = sum(row["total"] for row in core.values())
    core_solved = sum(row["solved"] for row in core.values())
    asr = (core_solved / core_total) if core_total else 0.0
    divergence = (len(score.divergent_tasks) / score.total) if score.total else 0.0

    weak = [(code, row) for code, row in score.teach.items()
            if (row["opportunities"] or 0) >= TEACH_MIN_SAMPLES
            and (row["teach_rate"] or 0.0) < TARGETS["TR"]]

    rows = [
        ("ASR — solve rate, day+week", f"≥ {TARGETS['ASR']:.0%}",
         f"{asr:.0%} ({core_solved}/{core_total})", asr >= TARGETS["ASR"]),
        ("AC — mean attempts when solved", f"≤ {TARGETS['AC']}",
         f"{score.convergence:.2f}",
         score.convergence <= TARGETS["AC"] and score.solved > 0),
        ("BD — tasks hitting a backend divergence", "0",
         f"{len(score.divergent_tasks)}/{score.total}", divergence <= TARGETS["BD"]),
        ("TR — codes below the teach floor", "none",
         ", ".join(code for code, _ in weak) or "none", not weak),
    ]
    return rows


def _thrashed(result: TaskResult) -> bool:
    """Two consecutive attempts that failed the same way with the same codes.

    The model read a diagnostic, changed the program, and got the same complaint
    back. That's the redesign signal NORTH_STAR talks about.
    """
    for prev, cur in zip(result.attempts, result.attempts[1:]):
        if prev.outcome == cur.outcome != rask.OK and prev.error_codes == cur.error_codes:
            return True
    return False


def teach_rates(results: list[TaskResult]) -> dict[str, dict]:
    seen: Counter = Counter()
    cleared: Counter = Counter()
    tasks_by_code: dict[str, set] = defaultdict(set)
    for result in results:
        for prev, cur in zip(result.attempts, result.attempts[1:]):
            for code in set(prev.error_codes):
                seen[code] += 1
                tasks_by_code[code].add(result.task_id)
                if code not in set(cur.error_codes):
                    cleared[code] += 1
        for attempt in result.attempts:
            for code in set(attempt.error_codes):
                tasks_by_code[code].add(result.task_id)
    out = {}
    for code in sorted(set(tasks_by_code)):
        opportunities = seen[code]
        out[code] = {
            "opportunities": opportunities,
            "cleared": cleared[code],
            "teach_rate": (cleared[code] / opportunities) if opportunities else None,
            "tasks": sorted(tasks_by_code[code]),
        }
    return out


def _by_horizon(results: list[TaskResult]) -> dict[str, dict]:
    groups: dict[str, list[TaskResult]] = defaultdict(list)
    for result in results:
        groups[result.horizon].append(result)
    out = {}
    for horizon, group in groups.items():
        solved = [r for r in group if r.solved]
        out[horizon] = {
            "total": len(group),
            "solved": len(solved),
            "pass_at_1": sum(1 for r in solved if r.attempts_used == 1) / len(group),
            "convergence": (sum(r.attempts_used for r in solved) / len(solved)) if solved else 0.0,
        }
    return out


# --- rendering --------------------------------------------------------------

def _cost_label(billing: str) -> str:
    """A subscription run spends plan quota, not dollars.

    The CLI still prices every call at list rates, which is useful — it says
    what the run would have cost on the API — but calling that "cost" on a Max
    plan is wrong, so the column says what it is.
    """
    return "list-price equivalent (plan quota, no API spend)" if billing == "subscription" else "cost"


def render_report(score: Score, results: list[TaskResult], meta: dict) -> str:
    lines = [
        "# Agent benchmark run",
        "",
        f"- model: `{meta.get('model')}`",
        f"- compiler: `{meta.get('rask')}` @ `{meta.get('commit', 'unknown')}`",
        f"- attempt budget: {meta.get('max_attempts')}",
        f"- started: {meta.get('started')}",
        "",
        "## Headline",
        "",
        "| metric | value |",
        "|---|---|",
        f"| solve rate | {score.solved}/{score.total} ({score.solve_rate:.0%}) |",
        f"| pass@1 | {score.pass_at_1:.0%} |",
        f"| convergence (mean attempts when solved) | {score.convergence:.2f} |",
        f"| tasks that hit a backend divergence | {len(score.divergent_tasks)} |",
        f"| tasks that thrashed (same error twice running) | {len(score.thrash_tasks)} |",
        f"| {_cost_label(score.billing)} | ${score.cost_usd:.2f} "
        f"({score.input_tokens:,} in / {score.output_tokens:,} out) |",
        f"| compiler time | {score.seconds:.0f}s |",
        "",
    ]

    if score.aborted_tasks:
        lines += [
            "## Not scored — the provider never answered",
            "",
            "These left the denominators. A usage limit or a broken login is not a",
            "model that failed to write Rask, and the numbers above cover only tasks",
            "that actually got a reply.",
            "",
        ]
        for task_id, why in score.aborted_tasks:
            lines.append(f"- `{task_id}` — {why}")
        lines.append("")
    if score.waited_seconds:
        lines += [f"Waited {score.waited_seconds:.0f}s in total on provider "
                  "rate limits and retries.", ""]

    lines += ["## Targets", "", "| target | want | got | |", "|---|---|---|---|"]
    for name, want, got, met in check_targets(score):
        lines.append(f"| {name} | {want} | {got} | {'ok' if met else '**miss**'} |")
    lines.append("")

    if score.by_horizon:
        lines += ["## By horizon", "", "| horizon | solved | pass@1 | convergence |", "|---|---|---|---|"]
        for horizon in ("day", "week", "month"):
            row = score.by_horizon.get(horizon)
            if not row:
                continue
            lines.append(f"| {horizon} | {row['solved']}/{row['total']} | "
                         f"{row['pass_at_1']:.0%} | {row['convergence']:.2f} |")
        lines.append("")

    lines += ["## Attempt outcomes", "", "| outcome | count |", "|---|---|"]
    for outcome, count in score.outcomes.most_common():
        lines.append(f"| {outcome} | {count} |")
    lines.append("")

    if score.divergent_tasks:
        lines += [
            "## Backend divergences — compiler bugs, not model failures",
            "",
            "The model wrote a program the two backends disagree about. Each of these",
            "should become a probe in `tests/suite/` with an issue.",
            "",
        ]
        for task_id in score.divergent_tasks:
            lines.append(f"- `{task_id}` — see `transcripts/{task_id}.md`")
        lines.append("")

    if score.teach:
        lines += [
            "## Error codes — do they teach?",
            "",
            "`cleared` counts the times the code was gone from the next attempt.",
            "A low rate means the message didn't get the model to a fix.",
            "",
            "| code | seen with a retry | cleared | teach rate | tasks |",
            "|---|---|---|---|---|",
        ]
        for code, row in sorted(score.teach.items(),
                                key=lambda kv: (-(kv[1]["opportunities"] or 0), kv[0])):
            rate = "–" if row["teach_rate"] is None else f"{row['teach_rate']:.0%}"
            lines.append(f"| {code} | {row['opportunities']} | {row['cleared']} | {rate} | "
                         f"{', '.join(row['tasks'][:4])} |")
        lines.append("")

    lines += ["## Per task", "", "| task | horizon | result | attempts | transcript |", "|---|---|---|---|---|"]
    for result in results:
        if result.solved:
            verdict = "solved"
        elif result.attempts:
            verdict = result.attempts[-1].outcome
        else:
            verdict = "no attempt"
        lines.append(f"| `{result.task_id}` | {result.horizon} | {verdict} | "
                     f"{result.attempts_used} | [log](transcripts/{result.task_id}.md) |")
    lines.append("")
    return "\n".join(lines)


def render_transcript(result: TaskResult, task) -> str:
    """The readable record. Failures are meant to be read here, not counted."""
    lines = [
        f"# {result.task_id} — {result.title}",
        "",
        f"horizon: **{result.horizon}** · concepts: {', '.join(result.concepts) or '—'}",
        f"· result: **{'solved' if result.solved else 'unsolved'}** "
        f"in {result.attempts_used} attempt(s)",
        "",
    ]
    if result.aborted:
        lines += [f"**Not scored** — {result.aborted}", ""]
    if result.stalls:
        stalls = "; ".join(f"attempt {s['attempt']}: {s['kind']}, waited {s['waited']:.0f}s"
                           for s in result.stalls)
        lines += [f"Provider stalls: {stalls}", ""]
    lines += [
        "## Prompt given to the model",
        "",
        task.prompt,
        "",
        "Contract:",
        "",
        "```rask",
        task.contract,
        "```",
        "",
    ]
    for attempt in result.attempts:
        lines += [f"## Attempt {attempt.index} — `{attempt.outcome}`", ""]
        if attempt.model_error:
            lines += [f"The model call failed: {attempt.model_error}", ""]
            continue
        lines += ["### What the model wrote", "", "```rask", attempt.code.strip(), "```", ""]
        if attempt.outcome == rask.OK:
            lines += ["Both backends green.", ""]
            continue
        if attempt.error_codes:
            lines += [f"Error codes: {', '.join(attempt.error_codes)}", ""]
        lines += ["### Interpreter", "", "```", attempt.interp_output.strip()[-4000:], "```", ""]
        if attempt.native_output.strip() != attempt.interp_output.strip():
            lines += ["### Native", "", "```", attempt.native_output.strip()[-4000:], "```", ""]
    return "\n".join(lines)


def write_run(directory: Path, score: Score, results: list[TaskResult],
              tasks_by_id: dict, meta: dict) -> None:
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "transcripts").mkdir(exist_ok=True)
    payload = {
        "meta": meta,
        "score": {
            "total": score.total,
            "solved": score.solved,
            "pass_at_1": score.pass_at_1,
            "solve_rate": score.solve_rate,
            "convergence": score.convergence,
            "outcomes": dict(score.outcomes),
            "divergent_tasks": score.divergent_tasks,
            "thrash_tasks": score.thrash_tasks,
            "teach": score.teach,
            "cost_usd": score.cost_usd,
            "input_tokens": score.input_tokens,
            "output_tokens": score.output_tokens,
            "by_horizon": score.by_horizon,
            "aborted_tasks": score.aborted_tasks,
            "waited_seconds": score.waited_seconds,
            "billing": score.billing,
        },
        "results": [r.to_json() for r in results],
    }
    (directory / "run.json").write_text(json.dumps(payload, indent=2), encoding="utf-8")
    (directory / "report.md").write_text(render_report(score, results, meta), encoding="utf-8")
    for result in results:
        task = tasks_by_id[result.task_id]
        (directory / "transcripts" / f"{result.task_id}.md").write_text(
            render_transcript(result, task), encoding="utf-8")
