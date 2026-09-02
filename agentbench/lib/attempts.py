# SPDX-License-Identifier: (MIT OR Apache-2.0)
"""The attempt loop: one task, up to N tries, compiler output as the feedback.

This is the measurement NORTH_STAR asks for — "have models write Rask against
the compiler, measure convergence, and read the failure transcripts". The loop
is deliberately thin: prompt, compile on both backends, hand the diagnostics
back verbatim, repeat. Nothing summarizes or rewrites the compiler's words,
because how well those words teach is one of the things being measured.
"""

from __future__ import annotations

import threading
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
    # Set when the run never got an answer out of the model — quota gone, the
    # transport broke, the login is bad. Not a task the model failed, so it
    # drops out of the scored set instead of counting as a miss.
    aborted: str | None = None
    # Every provider error the loop waited out, so a transcript shows the
    # stalls that a clean solve rate would otherwise hide.
    stalls: list[dict] = field(default_factory=list)

    @property
    def cost_usd(self) -> float:
        return sum(a.cost_usd for a in self.attempts)

    @property
    def waited_seconds(self) -> float:
        return sum(s.get("waited", 0.0) for s in self.stalls)

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


class QuotaLatch:
    """Shared "the plan window is gone" flag.

    Without it, a subscription run that hits its usage limit on task 3 makes
    every remaining task discover the same wall on its own, one full backoff
    ladder each. The first task to give up trips the latch and the rest abort
    immediately, so the partial run is scored and reported in seconds instead
    of an hour.
    """

    def __init__(self):
        self._event = threading.Event()
        self.reason: str | None = None
        self._lock = threading.Lock()

    def trip(self, reason: str) -> None:
        with self._lock:
            if not self._event.is_set():
                self.reason = reason
                self._event.set()

    @property
    def tripped(self) -> bool:
        return self._event.is_set()

    def wait(self, seconds: float) -> None:
        """Sleep, but wake early if another task decides the window is closed."""
        self._event.wait(seconds)


def _backoff(kind: str, retry: int, reply: Reply, base: float, cap: float) -> float:
    """How long to wait before retrying, in seconds.

    A rate limit that names its reset time gets waited out to that second; a
    subscription window can be hours away, so `cap` decides when to stop
    waiting and abort the task instead of holding the process hostage.
    """
    if reply.retry_after:
        return max(5.0, min(cap, reply.retry_after - time.time() + 5.0))
    if kind == "rate_limit":
        return min(cap, base * (2 ** retry))
    return min(cap, base * (1.5 ** retry))


def run_task(task: tasks.Task, model: Model, *, max_attempts: int = 3,
             rask_bin: Path | None = None, timeout: float = 90.0,
             feedback_lines: int = 60, on_attempt=None,
             model_retries: int = 4, retry_base: float = 15.0,
             max_wait: float = 600.0, quota: QuotaLatch | None = None) -> TaskResult:
    system = system_prompt()
    conversation: list[dict] = [
        {"role": "user", "content": FIRST_TURN.format(prompt=task.prompt, contract=task.contract)}
    ]
    result = TaskResult(task.id, task.title, task.horizon, list(task.concepts),
                        solved=False, attempts_used=0)
    started = time.monotonic()

    if quota and quota.tripped:
        result.aborted = f"not started: {quota.reason}"
        return result

    for index in range(1, max_attempts + 1):
        reply = _complete_with_retries(
            model, system, conversation, result, index,
            model_retries=model_retries, retry_base=retry_base,
            max_wait=max_wait, quota=quota)

        if reply.error:
            # A provider that never answered is not a model that got Rask
            # wrong. Record it, mark the task unscored, and stop.
            result.attempts.append(Attempt(
                index=index, reply="", code="", source="", outcome="model_error",
                error_codes=[], interp_output="", native_output="",
                model_error=reply.error))
            result.aborted = f"{reply.error_kind or 'error'}: {reply.error}"
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


def _complete_with_retries(model: Model, system: str, conversation: list[dict],
                           result: TaskResult, index: int, *, model_retries: int,
                           retry_base: float, max_wait: float,
                           quota: QuotaLatch | None) -> Reply:
    """One attempt's completion, waiting out rate limits and blips.

    A retry here does not consume an attempt: the model never saw the prompt,
    so counting it would inflate convergence and invent thrash that didn't
    happen. The waits are recorded on the result instead.
    """
    for retry in range(model_retries + 1):
        reply = model.complete(system, conversation)
        if not reply.error or not reply.retryable or retry == model_retries:
            return reply
        wait = _backoff(reply.error_kind, retry, reply, retry_base, max_wait)
        result.stalls.append({
            "attempt": index, "kind": reply.error_kind,
            "error": reply.error[:400], "waited": round(wait, 1),
        })
        if reply.error_kind == "rate_limit" and reply.retry_after and quota:
            gap = reply.retry_after - time.time()
            if gap > max_wait:
                mins = gap / 60.0
                quota.trip(f"plan usage limit — window reopens in {mins:.0f} min")
                return reply
        if quota:
            quota.wait(wait)
            if quota.tripped:
                return reply
        else:
            time.sleep(wait)
    return reply


def check_reference(task: tasks.Task, rask_bin: Path | None = None,
                    timeout: float = 90.0) -> rask.Verdict:
    """Does the hand-written solution still build and pass on both backends?

    A red answer here means the task is unfair to a model — no wording of the
    prompt could have produced a green run — so `selftest` quarantines it.
    """
    return rask.evaluate(task.assemble(task.reference), rask=rask_bin, timeout=timeout)
