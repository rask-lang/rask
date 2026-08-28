# SPDX-License-Identifier: (MIT OR Apache-2.0)
"""Run a candidate program through the compiler on both backends.

The verdict a task gets is deliberately not "did it pass". A candidate can fail
for two unrelated reasons and the benchmark is worthless if it blurs them:

  * the model wrote a wrong or unbuildable program  -> the model's failure
  * the backends disagree about a program           -> the compiler's failure

So every attempt is run twice, interpreter and native, and a disagreement is
reported as its own outcome. That mirrors tests/differential.sh, which treats
the interpreter as the reference for what the answer should be.
"""

from __future__ import annotations

import os
import re
import shutil
import subprocess
import tempfile
from dataclasses import dataclass, field, asdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

# Outcomes, worst-news-first. Ordering matters for reporting: DIVERGENCE is
# listed above the model's own failures because it invalidates the attempt.
OK = "ok"
DIVERGENCE = "divergence"
COMPILE_ERROR = "compile_error"
ASSERT_FAIL = "assert_fail"
CRASH = "crash"
TIMEOUT = "timeout"

# Timings differ run to run; strip them before comparing backends.
_TIMING = re.compile(r"\(\d+ms\)")
# error[E0123] / warning[E0123]
_CODE = re.compile(r"\berror\[([A-Z]\d+)\]")
# "3 tests, 1 passed, 2 failed"
_FAILED_COUNT = re.compile(r"\b([1-9]\d*) failed\b")
# "  ✓ verify: counts (0ms)" / "  ✗ verify: counts"
_TEST_LINE = re.compile(r"^\s*([\u2713\u2717])\s+(.+?)(?:\s+\(\d+ms\))?\s*$", re.M)


def find_rask() -> Path:
    """Locate the compiler the same way the shell gates do: release, then debug."""
    for candidate in ("release", "debug"):
        path = ROOT / "compiler" / "target" / candidate / "rask"
        if path.is_file() and os.access(path, os.X_OK):
            return path
    found = shutil.which("rask")
    if found:
        return Path(found)
    raise FileNotFoundError(
        "rask binary not found; build with "
        "`cd compiler && cargo build --release -p rask-cli`"
    )


def _normalize(text: str) -> str:
    return _TIMING.sub("", text).strip()


def _test_verdicts(text: str) -> dict[str, bool]:
    """Which named tests passed, per `rask test`'s own ✓/✗ lines."""
    return {name.strip(): mark == "\u2713" for mark, name in _TEST_LINE.findall(text)}


@dataclass
class BackendRun:
    backend: str
    exit_code: int
    output: str
    seconds: float

    @property
    def passed(self) -> bool:
        return self.exit_code == 0


@dataclass
class Verdict:
    outcome: str
    interp: BackendRun
    native: BackendRun
    error_codes: list[str] = field(default_factory=list)

    @property
    def ok(self) -> bool:
        return self.outcome == OK

    def feedback(self, limit: int = 60) -> str:
        """What the model sees after a failed attempt — the compiler's own words.

        Long output is trimmed from the middle, not either end. Which end matters
        depends on the failure: a check dump puts the first (usually causal) error
        at the top, while a test run puts the failure summary at the bottom.
        Cutting one end would systematically hide one of them.
        """
        if self.outcome == DIVERGENCE:
            return (
                "The two backends disagree on this program. This is a compiler bug,\n"
                "not your mistake, but the program still needs to work on both.\n"
                f"--- interpreter (exit {self.interp.exit_code}) ---\n"
                f"{_tail(self.interp.output, limit // 2)}\n"
                f"--- native (exit {self.native.exit_code}) ---\n"
                f"{_tail(self.native.output, limit // 2)}"
            )
        failing = self.interp if not self.interp.passed else self.native
        return _tail(failing.output, limit)

    def to_json(self) -> dict:
        return {
            "outcome": self.outcome,
            "error_codes": self.error_codes,
            "interp": asdict(self.interp),
            "native": asdict(self.native),
        }


def _tail(text: str, lines: int) -> str:
    """Keep the head and the tail, drop the middle. See Verdict.feedback."""
    rows = text.strip().splitlines()
    if len(rows) <= lines:
        return "\n".join(rows)
    head = (lines * 2) // 3
    tail = lines - head
    return "\n".join(
        rows[:head]
        + ["… (%d lines omitted)" % (len(rows) - lines)]
        + rows[-tail:]
    )


def _stage(run: "BackendRun") -> str:
    """Which stage the backend lost at — the coarsest thing they can disagree on."""
    if _CODE.search(run.output) or "Check FAILED" in run.output or "Build FAILED" in run.output:
        return "check"
    if run.exit_code == 124:
        return "timeout"
    return "run"


def _run(rask: Path, args: list[str], cwd: Path, timeout: float) -> tuple[int, str, float]:
    import time

    env = dict(os.environ)
    env.setdefault("RASK_RUNTIME_DIR", str(ROOT / "compiler" / "runtime"))
    start = time.monotonic()
    try:
        proc = subprocess.run(
            [str(rask), *args],
            cwd=cwd,
            env=env,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired as exc:
        elapsed = time.monotonic() - start
        partial = (exc.stdout or "") + (exc.stderr or "")
        if isinstance(partial, bytes):
            partial = partial.decode("utf-8", "replace")
        return 124, partial + f"\n[timed out after {timeout:.0f}s]", elapsed
    elapsed = time.monotonic() - start
    return proc.returncode, proc.stdout + proc.stderr, elapsed


def evaluate(source: str, rask: Path | None = None, timeout: float = 90.0,
             workdir: Path | None = None) -> Verdict:
    """Compile and run `source` on both backends and classify the result."""
    rask = rask or find_rask()
    tmp = None
    if workdir is None:
        tmp = tempfile.TemporaryDirectory(prefix="agentbench-")
        workdir = Path(tmp.name)
    try:
        workdir.mkdir(parents=True, exist_ok=True)
        prog = workdir / "candidate.rk"
        prog.write_text(source, encoding="utf-8")

        icode, iout, isecs = _run(rask, ["test", "--interp", prog.name], workdir, timeout)
        ncode, nout, nsecs = _run(rask, ["test", prog.name], workdir, timeout)

        interp = BackendRun("interp", icode, iout, isecs)
        native = BackendRun("native", ncode, nout, nsecs)
        return Verdict(_classify(interp, native), interp, native,
                       sorted(set(_CODE.findall(iout)) | set(_CODE.findall(nout))))
    finally:
        if tmp is not None:
            tmp.cleanup()


def _classify(interp: BackendRun, native: BackendRun) -> str:
    if interp.passed and native.passed:
        if _normalize(interp.output) == _normalize(native.output):
            return OK
        return DIVERGENCE
    if interp.passed != native.passed:
        return DIVERGENCE
    if interp.exit_code == 124 or native.exit_code == 124:
        return TIMEOUT
    # Both failed — but not necessarily the same way, and "both red" was hiding
    # real divergences. A model wrote `counts[word] = n` on a `Map`: the
    # interpreter failed one assertion, native panicked with a stack address as
    # an index in three tests, and the attempt scored `assert_fail` with no
    # divergence recorded. BD is the exit criterion the compiler never had, so
    # it can't only count the cases where one backend happens to be green.
    if _stage(interp) != _stage(native):
        return DIVERGENCE
    itests, ntests = _test_verdicts(interp.output), _test_verdicts(native.output)
    if itests and ntests and itests != ntests:
        return DIVERGENCE
    # Both failed. The interpreter is the reference, so read its output for the
    # shape of the failure. `rask test` is unambiguous about which stage lost:
    # a check error prints `error[E….]` and `Check FAILED`, a bad assertion
    # prints `✗ <name>` and a nonzero failed count.
    text = interp.output
    if _CODE.search(text) or "Check FAILED" in text or "Build FAILED" in text:
        return COMPILE_ERROR
    if "assertion failed" in text or "\u2717" in text or _FAILED_COUNT.search(text):
        return ASSERT_FAIL
    return CRASH
