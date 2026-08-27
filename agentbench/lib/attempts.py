# SPDX-License-Identifier: (MIT OR Apache-2.0)
"""The attempt loop: one task, up to N tries, compiler output as the feedback.

This is the measurement NORTH_STAR asks for — "have models write Rask against
the compiler, measure convergence, and read the failure transcripts". The loop
is deliberately thin: prompt, compile on both backends, hand the diagnostics
back verbatim, repeat. Nothing summarizes or rewrites the compiler's words,
because how well those words teach is one of the things being measured.
"""

from __future__ import annotations

import time
from dataclasses import dataclass, field, asdict
from pathlib import Path

from . import rask, tasks
from .models import Model, Reply

ROOT = Path(__file__).resolve().parents[2]
LANGUAGE_CARD = ROOT / "LANGUAGE_GUIDE.md"

SYSTEM_PREAMBLE = """\
You are writing Rask, a compiled systems language. Rask is not Rust, Go, or \
TypeScript, and habits from those languages produce code that does not compile \
here. The complete language reference follows; treat it as normative.

Answer with Rask source and nothing else — one ```rask fenced block, no prose \
before or after it, no explanation. Do not restate the task. Do not write \
`func main`. Do not write `test` blocks; the grader supplies its own.
"""

FIRST_TURN = """\
{prompt}

Your solution must define exactly these, with these names and signatures:

```rask
{contract}
```

Hidden tests will call them. Reply with one ```rask block containing the whole \
solution.
"""

RETRY_TURN = """\
That did not work. The compiler said:

```
{feedback}
```

Fix it and reply with the complete corrected solution as one ```rask block. \
Send the whole program again, not a patch.
"""


@dataclass
class Attempt:
    index: int
    reply: str
    code: str
    source: str
    outcome: str
    error_codes: list[str]
    interp_output: str
    native_output: str
    input_tokens: int = 0
    output_tokens: int = 0
    cost_usd: float = 0.0
    model_error: str | None = None


@dataclass
class TaskResult:
    task_id: str
    title: str
    horizon: str
    concepts: list[str]
    solved: bool
    attempts_used: int
    attempts: list[Attempt] = field(default_factory=list)
    seconds: float = 0.0

    @property
    def cost_usd(self) -> float:
        return sum(a.cost_usd for a in self.attempts)

    @property
    def saw_divergence(self) -> bool:
        return any(a.outcome == rask.DIVERGENCE for a in self.attempts)

    def to_json(self) -> dict:
        out = asdict(self)
        out["cost_usd"] = self.cost_usd
        out["saw_divergence"] = self.saw_divergence
        return out


def system_prompt() -> str:
    card = LANGUAGE_CARD.read_text(encoding="utf-8")
    return f"{SYSTEM_PREAMBLE}\n\n----- BEGIN RASK LANGUAGE REFERENCE -----\n{card}\n----- END RASK LANGUAGE REFERENCE -----\n"


def run_task(task: tasks.Task, model: Model, *, max_attempts: int = 3,
             rask_bin: Path | None = None, timeout: float = 90.0,
             feedback_lines: int = 60, on_attempt=None) -> TaskResult:
    system = system_prompt()
    conversation: list[dict] = [
        {"role": "user", "content": FIRST_TURN.format(prompt=task.prompt, contract=task.contract)}
    ]
    result = TaskResult(task.id, task.title, task.horizon, list(task.concepts),
                        solved=False, attempts_used=0)
    started = time.monotonic()

    for index in range(1, max_attempts + 1):
        reply: Reply = model.complete(system, conversation)
        if reply.error:
            result.attempts.append(Attempt(
                index=index, reply="", code="", source="", outcome="model_error",
                error_codes=[], interp_output="", native_output="",
                model_error=reply.error))
            break

        code = tasks.extract_code(reply.text)
        source = task.assemble(code)
        verdict = rask.evaluate(source, rask=rask_bin, timeout=timeout)

        attempt = Attempt(
            index=index, reply=reply.text, code=code, source=source,
            outcome=verdict.outcome, error_codes=verdict.error_codes,
            interp_output=verdict.interp.output, native_output=verdict.native.output,
            input_tokens=reply.input_tokens, output_tokens=reply.output_tokens,
            cost_usd=reply.cost_usd)
        result.attempts.append(attempt)
        result.attempts_used = index
        if on_attempt:
            on_attempt(task, attempt)

        if verdict.ok:
            result.solved = True
            break

        conversation = conversation + [
            {"role": "assistant", "content": reply.text},
            {"role": "user", "content": RETRY_TURN.format(
                feedback=verdict.feedback(feedback_lines))},
        ]

    result.seconds = time.monotonic() - started
    return result


def check_reference(task: tasks.Task, rask_bin: Path | None = None,
                    timeout: float = 90.0) -> rask.Verdict:
    """Does the hand-written solution still build and pass on both backends?

    A red answer here means the task is unfair to a model — no wording of the
    prompt could have produced a green run — so `selftest` quarantines it.
    """
    return rask.evaluate(task.assemble(task.reference), rask=rask_bin, timeout=timeout)
