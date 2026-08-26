# SPDX-License-Identifier: (MIT OR Apache-2.0)
"""Task loading and candidate assembly.

A task is a directory holding three files:

    task.toml     the prompt, the contract the solution must satisfy, metadata
    verify.rk     `test` blocks that exercise the contract — the model never sees these
    reference.rk  a hand-written solution, so a red result can be blamed correctly

The reference is the part that makes the numbers mean anything. Without it a
failed task could be "the model can't write this" or "the compiler can't build
this" and there's no way to tell from the outside. `bench.py selftest` runs
every reference through both backends; a task whose reference is red is quarantined
and left out of the model-facing score.
"""

from __future__ import annotations

import re
import tomllib
from dataclasses import dataclass
from pathlib import Path

BENCH_ROOT = Path(__file__).resolve().parents[1]
TASK_ROOT = BENCH_ROOT / "tasks"
QUARANTINE = BENCH_ROOT / "quarantine.txt"

HORIZONS = ("day", "week", "month")

_FENCE = re.compile(r"```(?:rask|rk)?[ \t]*\n(.*?)```", re.DOTALL)


@dataclass
class Task:
    id: str
    title: str
    horizon: str
    concepts: list[str]
    issues: list[str]
    contract: str
    prompt: str
    verify: str
    reference: str
    path: Path

    def assemble(self, solution: str) -> str:
        """Glue a candidate solution to the hidden checks, solution first.

        `rask test` doesn't need a `func main`, so nothing is synthesized here —
        whatever the model wrote is what gets compiled, plus the checks.
        """
        return f"{solution.rstrip()}\n\n{self.verify.strip()}\n"


def extract_code(reply: str) -> str:
    """Pull Rask source out of a model reply.

    Models fence their code most of the time and prose around it the rest. Take
    the fenced blocks when there are any (all of them, joined — a reply that
    splits a struct and its `extend` across two blocks is common), else assume
    the whole reply is source.
    """
    blocks = _FENCE.findall(reply)
    if blocks:
        return "\n\n".join(b.strip() for b in blocks)
    return reply.strip()


def load_task(directory: Path) -> Task:
    meta = tomllib.loads((directory / "task.toml").read_text(encoding="utf-8"))
    horizon = meta.get("horizon", "day")
    if horizon not in HORIZONS:
        raise ValueError(f"{directory.name}: horizon must be one of {HORIZONS}")
    return Task(
        id=directory.name,
        title=meta["title"],
        horizon=horizon,
        concepts=list(meta.get("concepts", [])),
        issues=[str(i) for i in meta.get("issues", [])],
        contract=meta["contract"].strip(),
        prompt=meta["prompt"].strip(),
        verify=(directory / "verify.rk").read_text(encoding="utf-8"),
        reference=(directory / "reference.rk").read_text(encoding="utf-8"),
        path=directory,
    )


def load_tasks(select: str | None = None, root: Path = TASK_ROOT) -> list[Task]:
    """Load every task, optionally filtered by horizon, id, or substring."""
    tasks = []
    for directory in sorted(root.iterdir()):
        if not (directory / "task.toml").is_file():
            continue
        task = load_task(directory)
        if select and not _matches(task, select):
            continue
        tasks.append(task)
    return tasks


def _matches(task: Task, select: str) -> bool:
    for term in select.split(","):
        term = term.strip()
        if not term:
            continue
        if term == task.horizon or term == task.id or term in task.id:
            return True
        if term in task.concepts:
            return True
    return False


def quarantined() -> dict[str, str]:
    """Tasks whose reference solution doesn't build today, mapped to why.

    Same shape as tests/known_divergences.txt: `<task-id>  #issue  note`.
    """
    if not QUARANTINE.is_file():
        return {}
    out = {}
    for line in QUARANTINE.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split(None, 1)
        out[parts[0]] = parts[1].strip() if len(parts) > 1 else ""
    return out
