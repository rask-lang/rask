# SPDX-License-Identifier: (MIT OR Apache-2.0)
"""Model adapters.

Three real ones and two fakes. The fakes exist so the whole loop — prompting,
assembly, compilation, scoring, transcripts — can be exercised end to end
without spending anything, which is also how the harness gets tested in CI.

    mock:solves@2      fake: fails twice, then emits the reference solution
    mock:reference     fake: emits the reference immediately
    mock:garbage       fake: never produces anything buildable
    cli:<model>        shells out to `claude -p`
    api:<model>        POSTs to the Anthropic messages API (ANTHROPIC_API_KEY)

Every adapter reports token usage where the provider gives it, so a run ends
with a real cost figure rather than a guess.
"""

from __future__ import annotations

import json
import os
import subprocess
import urllib.request
from dataclasses import dataclass, field


@dataclass
class Reply:
    text: str
    input_tokens: int = 0
    output_tokens: int = 0
    cost_usd: float = 0.0
    error: str | None = None
    meta: dict = field(default_factory=dict)


class Model:
    """A stateless completion: system prompt plus a conversation, text back.

    Stateless on purpose — the attempt loop owns the conversation, so a
    transcript replays without the adapter having to remember anything.
    """

    name = "model"
    paid = False

    def complete(self, system: str, messages: list[dict]) -> Reply:
        raise NotImplementedError


# --- fakes ------------------------------------------------------------------

_GARBAGE = """
fn broken(a: i32) -> i32 {
    let mut x = a;
    Ok(x + 1)
}
"""


class MockModel(Model):
    """Deterministic stand-in. `solves@N` means: correct on attempt N.

    The failure it emits before then is Rust-shaped on purpose — `fn`, `let mut`,
    `Ok(...)` are the mistakes a model actually makes here, so the harness gets
    exercised against realistic diagnostics rather than a syntax error.
    """

    paid = False

    def __init__(self, spec: str, reference: str = ""):
        self.spec = spec
        self.reference = reference
        self.calls = 0
        self.name = f"mock:{spec}"
        self.solve_at = 1
        if spec.startswith("solves@"):
            self.solve_at = int(spec.split("@", 1)[1])
        elif spec == "garbage":
            self.solve_at = 10**9

    def complete(self, system: str, messages: list[dict]) -> Reply:
        self.calls += 1
        body = self.reference if self.calls >= self.solve_at else _GARBAGE
        return Reply(text=f"```rask\n{body.strip()}\n```", input_tokens=0, output_tokens=0)


# --- claude CLI -------------------------------------------------------------

class CliModel(Model):
    """`claude -p`, reusing whatever auth the machine already has.

    The conversation is flattened into one prompt instead of resumed with
    `--continue`: a resumed CLI session carries its own scratchpad and tool
    history, and the benchmark wants to measure the model reading compiler
    output, not the CLI's memory of the last run.
    """

    paid = True

    def __init__(self, model: str, binary: str = "claude", timeout: float = 600.0):
        self.model = model
        self.binary = binary
        self.timeout = timeout
        self.name = f"cli:{model}"

    def complete(self, system: str, messages: list[dict]) -> Reply:
        if len(messages) == 1:
            prompt = messages[0]["content"]
        else:
            prompt = "\n\n".join(f"[{m['role']}]\n{m['content']}" for m in messages)
        cmd = [
            self.binary, "-p",
            "--model", self.model,
            "--output-format", "json",
            "--system-prompt", system,
            # No repo access: the model must write Rask from the card in its
            # prompt, which is the thing being measured.
            "--disallowedTools", "Bash,Edit,Write,Read,Glob,Grep,WebFetch,WebSearch,Task",
        ]
        try:
            proc = subprocess.run(cmd, input=prompt, capture_output=True, text=True,
                                  timeout=self.timeout, cwd=os.environ.get("TMPDIR", "/tmp"))
        except subprocess.TimeoutExpired:
            return Reply(text="", error=f"claude CLI timed out after {self.timeout:.0f}s")
        if proc.returncode != 0:
            return Reply(text="", error=f"claude CLI exit {proc.returncode}: {proc.stderr[-800:]}")
        return _parse_cli_json(proc.stdout)


def _parse_cli_json(stdout: str) -> Reply:
    """Read `--output-format json`, tolerating a shape that shifts between versions."""
    try:
        payload = json.loads(stdout)
    except json.JSONDecodeError:
        # Not JSON — treat the raw stdout as the answer rather than losing the run.
        return Reply(text=stdout.strip())
    if isinstance(payload, list):
        payload = payload[-1] if payload else {}
    usage = payload.get("usage") or {}
    return Reply(
        text=payload.get("result") or payload.get("text") or "",
        input_tokens=int(usage.get("input_tokens", 0) or 0),
        output_tokens=int(usage.get("output_tokens", 0) or 0),
        cost_usd=float(payload.get("total_cost_usd", 0.0) or 0.0),
        meta={k: payload[k] for k in ("duration_ms", "num_turns", "session_id") if k in payload},
    )


# --- Anthropic API ----------------------------------------------------------

# Dollars per million tokens (input, output). Only used to price a run after the
# fact; the CLI reports its own cost and that figure wins when present.
PRICES = {
    "claude-opus-5": (5.0, 25.0),
    "claude-sonnet-5": (3.0, 15.0),
    "claude-haiku-4-5-20251001": (1.0, 5.0),
}


class ApiModel(Model):
    paid = True

    def __init__(self, model: str, max_tokens: int = 8000, timeout: float = 600.0):
        self.model = model
        self.max_tokens = max_tokens
        self.timeout = timeout
        self.name = f"api:{model}"
        self.key = os.environ.get("ANTHROPIC_API_KEY")

    def complete(self, system: str, messages: list[dict]) -> Reply:
        if not self.key:
            return Reply(text="", error="ANTHROPIC_API_KEY is not set")
        body = json.dumps({
            "model": self.model,
            "max_tokens": self.max_tokens,
            "system": system,
            "messages": messages,
        }).encode()
        req = urllib.request.Request(
            "https://api.anthropic.com/v1/messages",
            data=body,
            headers={
                "content-type": "application/json",
                "x-api-key": self.key,
                "anthropic-version": "2023-06-01",
            },
        )
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as resp:
                payload = json.loads(resp.read())
        except Exception as exc:  # network, auth, rate limit — all fatal for one attempt
            return Reply(text="", error=f"{type(exc).__name__}: {exc}")
        text = "".join(b.get("text", "") for b in payload.get("content", [])
                       if b.get("type") == "text")
        usage = payload.get("usage", {})
        inp = int(usage.get("input_tokens", 0))
        out = int(usage.get("output_tokens", 0))
        rate_in, rate_out = PRICES.get(self.model, (0.0, 0.0))
        return Reply(text=text, input_tokens=inp, output_tokens=out,
                     cost_usd=(inp * rate_in + out * rate_out) / 1e6,
                     meta={"stop_reason": payload.get("stop_reason")})


def build(spec: str, reference: str = "") -> Model:
    """Turn a `--model` string into an adapter."""
    kind, _, rest = spec.partition(":")
    if kind == "mock":
        return MockModel(rest or "reference", reference)
    if kind == "cli":
        return CliModel(rest or "claude-sonnet-5")
    if kind == "api":
        return ApiModel(rest or "claude-sonnet-5")
    raise ValueError(f"unknown model spec {spec!r} (expected mock:… , cli:… or api:…)")
