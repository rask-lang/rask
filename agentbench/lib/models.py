# SPDX-License-Identifier: (MIT OR Apache-2.0)
"""Model adapters.

Two real ones and three fakes. The fakes exist so the whole loop — prompting,
assembly, compilation, scoring, transcripts — can be exercised end to end
without spending anything, which is also how the harness gets tested in CI.

    mock:solves@2      fake: fails twice, then emits the reference solution
    mock:reference     fake: emits the reference immediately
    mock:garbage       fake: never produces anything buildable
    cli:<model>        shells out to `claude -p` — spends Claude plan quota
    api:<model>        POSTs to the Anthropic messages API (ANTHROPIC_API_KEY)

`cli:` is the adapter to reach for on a Pro/Max subscription: the CLI signs
requests with the OAuth login, so a run draws on plan quota instead of metered
API credit. `api:` is for a metered key.

Every adapter reports token usage where the provider gives it, and says which
kind of billing it used, so a run ends with a real figure rather than a guess.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import urllib.request
from dataclasses import dataclass, field


# Model errors the loop should wait out rather than score. A rate-limited
# attempt is not a model that failed to write Rask, and counting it as one
# quietly poisons solve rate and convergence both.
RETRYABLE = {"rate_limit", "overloaded", "transport", "timeout"}


@dataclass
class Reply:
    text: str
    input_tokens: int = 0
    output_tokens: int = 0
    cost_usd: float = 0.0
    error: str | None = None
    error_kind: str | None = None
    retry_after: float = 0.0
    meta: dict = field(default_factory=dict)

    @property
    def retryable(self) -> bool:
        return self.error_kind in RETRYABLE


class Model:
    """A stateless completion: system prompt plus a conversation, text back.

    Stateless on purpose — the attempt loop owns the conversation, so a
    transcript replays without the adapter having to remember anything.
    """

    name = "model"
    paid = False
    billing = "free"

    def complete(self, system: str, messages: list[dict]) -> Reply:
        raise NotImplementedError


# --- error classification ---------------------------------------------------

_LIMIT_PATTERNS = (
    "usage limit",
    "limit reached",
    "rate limit",
    "rate_limit",
    "too many requests",
    "429",
    "quota",
)
_OVERLOAD_PATTERNS = ("overloaded", "529", "503", "502", "service unavailable")
_AUTH_PATTERNS = (
    "please run /login",
    "invalid api key",
    "authentication_error",
    "unauthorized",
    "401",
    "403",
    "credit balance",
)


def classify_error(text: str) -> str:
    """Name the failure so the loop knows whether waiting will help.

    `auth` and `other` are terminal — no amount of backoff fixes a missing
    login or a malformed request. The rest are worth another try.
    """
    low = (text or "").lower()
    if any(p in low for p in _AUTH_PATTERNS):
        return "auth"
    if any(p in low for p in _LIMIT_PATTERNS):
        return "rate_limit"
    if any(p in low for p in _OVERLOAD_PATTERNS):
        return "overloaded"
    if "timed out" in low or "timeout" in low:
        return "timeout"
    if any(p in low for p in ("connection", "network", "temporarily", "eof", "reset by peer")):
        return "transport"
    return "other"


# `Claude usage limit reached|1748631600` — the CLI hands back the epoch second
# the plan window reopens, which is the only honest answer to "how long".
_RESET_EPOCH = re.compile(r"limit reached\|(\d{9,})")


def parse_reset(text: str) -> float:
    """Epoch second the quota window reopens, or 0 if the message didn't say."""
    m = _RESET_EPOCH.search(text or "")
    return float(m.group(1)) if m else 0.0


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
    billing = "free"

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

# Everything this machine would otherwise inject into the child: its CLAUDE.md,
# skills, hooks, plugins, MCP servers, settings, and the CLI's own system prompt
# and tool definitions. All of it is an unmeasured variable in a benchmark whose
# whole point is "the model gets the card and nothing else" — and on a
# subscription it's ~24k tokens of quota per call spent on context the model
# must not use.
ISOLATION = [
    "--safe-mode",             # no CLAUDE.md, skills, hooks, plugins, MCP, settings
    "--tools", "",             # no tool use: write Rask from the card or don't
    "--strict-mcp-config",
    "--setting-sources", "",
    "--no-session-persistence",
]

# Auth vars that would flip `claude` from the subscription login to metered API
# billing. Dropped from the child unless the run explicitly opts in.
API_AUTH_VARS = ("ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN")


class CliModel(Model):
    """`claude -p`, reusing the login the machine already has.

    On a Pro/Max plan that login is the subscription, so a run spends plan
    quota and no API credit. An exported ANTHROPIC_API_KEY would silently
    switch the whole run to metered billing, so it's removed from the child's
    environment; set RASK_BENCH_CLI_API_KEY=1 to keep it and bill the key.

    The conversation is flattened into one prompt instead of resumed with
    `--continue`: a resumed CLI session carries its own scratchpad and tool
    history, and the benchmark wants to measure the model reading compiler
    output, not the CLI's memory of the last run.
    """

    paid = True

    def __init__(self, model: str, binary: str = "claude", timeout: float = 600.0,
                 effort: str | None = None):
        self.model = model
        self.binary = binary
        self.timeout = timeout
        self.effort = effort
        self.name = f"cli:{model}"
        self.use_api_key = os.environ.get("RASK_BENCH_CLI_API_KEY") == "1"
        keyed = any(os.environ.get(v) for v in API_AUTH_VARS)
        self.billing = "api" if (keyed and self.use_api_key) else "subscription"

    def _env(self) -> dict:
        env = dict(os.environ)
        if not self.use_api_key:
            for var in API_AUTH_VARS:
                env.pop(var, None)
        return env

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
            *ISOLATION,
        ]
        if self.effort:
            cmd += ["--effort", self.effort]
        try:
            proc = subprocess.run(cmd, input=prompt, capture_output=True, text=True,
                                  timeout=self.timeout, env=self._env(),
                                  cwd=os.environ.get("TMPDIR", "/tmp"))
        except subprocess.TimeoutExpired:
            return Reply(text="", error=f"claude CLI timed out after {self.timeout:.0f}s",
                         error_kind="timeout")
        if proc.returncode != 0:
            blurb = (proc.stderr or proc.stdout)[-800:]
            return Reply(text="", error=f"claude CLI exit {proc.returncode}: {blurb}",
                         error_kind=classify_error(blurb), retry_after=parse_reset(blurb))
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
    text = payload.get("result") or payload.get("text") or ""
    # `input_tokens` counts only what wasn't served from cache. On a
    # subscription every one of these still comes off the plan window, and the
    # card is 6k tokens resent every attempt, so cached reads are most of the
    # real spend — count all three or the run under-reports by ~100x.
    total_in = (int(usage.get("input_tokens", 0) or 0)
                + int(usage.get("cache_creation_input_tokens", 0) or 0)
                + int(usage.get("cache_read_input_tokens", 0) or 0))
    reply = Reply(
        text=text,
        input_tokens=total_in,
        output_tokens=int(usage.get("output_tokens", 0) or 0),
        cost_usd=float(payload.get("total_cost_usd", 0.0) or 0.0),
        meta={k: payload[k] for k in ("duration_ms", "num_turns", "session_id") if k in payload},
    )
    # The CLI can exit 0 and still report a failure in the payload — a hit rate
    # limit comes back this way, with the refusal text sitting in `result`.
    status = payload.get("api_error_status")
    if payload.get("is_error") or payload.get("subtype") in ("error_during_execution",) or status:
        blurb = f"{payload.get('subtype') or 'error'} {status or ''} {text}".strip()
        reply.error = f"claude CLI reported an error: {blurb[:800]}"
        reply.error_kind = classify_error(blurb)
        reply.retry_after = parse_reset(blurb)
    return reply


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
    billing = "api"

    def __init__(self, model: str, max_tokens: int = 8000, timeout: float = 600.0):
        self.model = model
        self.max_tokens = max_tokens
        self.timeout = timeout
        self.name = f"api:{model}"
        self.key = os.environ.get("ANTHROPIC_API_KEY")

    def complete(self, system: str, messages: list[dict]) -> Reply:
        if not self.key:
            return Reply(text="", error="ANTHROPIC_API_KEY is not set", error_kind="auth")
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
        except Exception as exc:  # network, auth, rate limit — one attempt's problem
            blurb = f"{type(exc).__name__}: {exc}"
            return Reply(text="", error=blurb, error_kind=classify_error(blurb))
        text = "".join(b.get("text", "") for b in payload.get("content", [])
                       if b.get("type") == "text")
        usage = payload.get("usage", {})
        inp = int(usage.get("input_tokens", 0))
        out = int(usage.get("output_tokens", 0))
        rate_in, rate_out = PRICES.get(self.model, (0.0, 0.0))
        return Reply(text=text, input_tokens=inp, output_tokens=out,
                     cost_usd=(inp * rate_in + out * rate_out) / 1e6,
                     meta={"stop_reason": payload.get("stop_reason")})


def build(spec: str, reference: str = "", *, effort: str | None = None) -> Model:
    """Turn a `--model` string into an adapter."""
    kind, _, rest = spec.partition(":")
    if kind == "mock":
        return MockModel(rest or "reference", reference)
    if kind == "cli":
        return CliModel(rest or "claude-sonnet-5", effort=effort)
    if kind == "api":
        return ApiModel(rest or "claude-sonnet-5")
    raise ValueError(f"unknown model spec {spec!r} (expected mock:… , cli:… or api:…)")
