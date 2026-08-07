<!-- id: tool.annotate -->
<!-- status: proposed -->
<!-- summary: rask annotate — ghost text materialized as a read-only report for diffs, review, and terminals -->
<!-- depends: compiler/effects.md, memory/parameters.md, memory/closures.md, concurrency/io-context.md, types/gradual-constraints.md, memory/context-clauses.md, tooling/describe-schema.md -->

# Annotate

`rask annotate` renders source with compiler-inferred information set beside each line — the same information IDEs show as ghost text, for the surfaces that have no ghosts: diffs, PR review, pastebins, grep output, terminals.

This closes the gap principles 7 and 9 leave open: the compiler knows, tooling shows — but code is mostly *reviewed* in places where "tooling" meant only the IDE. One command, one renderer, three outputs: annotated source (default), annotated diff (`--diff`), machine-readable sidecar (`--json`).

Annotations are computed when displayed and stored nowhere — the same lifecycle as ghost text. Nothing is run "before" a PR and nothing annotate produces lives in the repository.

## The Command

| Rule | Description |
|------|-------------|
| **AN1: View, not state** | Output goes to stdout, computed at display time. `rask annotate` never writes files, and its output is never an input to anything but eyes and the `--json` consumers |
| **AN2: Report, not source** | Output is a line-numbered report (diagnostics-formatter style) with annotations in a right-hand gutter. It is deliberately **not** valid Rask — it cannot be committed, hand-edited, or left to go stale, and no comment form is reserved for it |
| **AN3: One renderer** | Label text is shared with the IDE ghost layer, verbatim: `[io]` from `comp.effects`, `⟨pauses⟩` from `conc.io-context`, `[moves: x (T)]` from `mem.closures`, `mutate x` from `mem.parameters`. Annotate introduces no vocabulary of its own |
| **AN4: Deterministic** | Same source + same compiler → byte-identical output. Multiple annotations on one line render in fixed order — call-site modes, captures, match modes, inferred signature/context, effects, pause — joined with ` · ` |
| **AN5: Clean check required** | Annotation data comes from the type-check, ownership, and effect passes. A file that doesn't check prints diagnostics instead |

```
$ rask annotate src/round.rk

src/round.rk
   1 | func damage(h, amount) {                     « func damage(h: Handle<Player>, amount: i32) · using Pool<Player>
   2 |     with pools[h] as mut player {
   3 |         player.hp -= amount
   4 |     }
   5 | }
   6 |
   7 | func run_round(seed: i32) {                  « func run_round(seed: i32) -> void or Error · [io]
   8 |     mut player = Player.new(seed)
   9 |     apply_damage(mutate player, 10)
  10 |     let report = try http.post(STATS_URL, player.encode())
     |                                              « ⟨pauses⟩
  11 |     spawn(own || { archive(report) }).detach()
     |                                              « [moves: report (Response)]
  12 | }
```

## What Gets Annotated

Not every ghost earns a place in a diff. The cut:

| Rule | Description |
|------|-------------|
| **TR1: Review tier (default)** | Information where a wrong assumption changes what the code does to data or when it runs: mutation, consumption, suspension, I/O, hidden dependencies |
| **TR2: Full tier (`--all`)** | Everything the IDE ghosts, including pure comprehension aids — types, scopes, optimizer decisions |
| **TR3: Proved only** | Annotate shows what the compiler proved, never what it guessed. `type.gradual/IS3` applies verbatim: propagated nominal bounds render as `T: Comparable`, raw shape requirements as `T: {frobnicate}` — never a trait name inferred from shape |

| Information | Rendering (source spec) | Tier |
|-------------|------------------------|------|
| `own` at unmarked take call sites | `own user` (`mem.parameters`; `mutate` is in source per PM4 — never annotated) | review |
| Pause points | `⟨pauses⟩` (`conc.io-context`) | review |
| Effects on function definitions | `[pure]`, `[io]`, `[io, async]`, … (`comp.effects` Ghost Text Format) | review |
| `own` closure captures | `[moves: name (T)]`, `[copies: name (T)]` (`mem.closures`) | review |
| Consuming match arms | `[takes]`, `[consumes]` (`type.enums`) | review |
| Inferred private signatures | full signature on the `func` line (`type.gradual`) | review |
| Inferred `using` clauses | `using Pool<Player>` (`mem.context`) | review |
| Inferred binding types | `: Vec<string>` | full |
| Non-`own` closure captures | `[borrows: …]`, `[inline]` (`mem.closures`) | full |
| Borrowing match arms | `[borrows]`, `[mutates]`, `[reads]` (`type.enums`) | full |
| Borrow scopes / views | `[view: until line N]`, `[bound: lines N-M]` (`mem.borrowing`) | full |
| Optimizer decisions | `[clone elided → move]`, `[rc elided]`, `[fused loop]` | full |
| Parameter names at positional calls | `name:` ghosts (`SYNTAX.md`) | full |

The tier line is the same rule that keeps CORE_DESIGN's visibility bands honest: anything that mutates, consumes, suspends, or does I/O must be recoverable without an IDE — so it's review tier here.

## Diff Mode

The PR-review case. `rask annotate --diff <rev>` takes a git revision and annotates the change, not the whole tree.

| Rule | Description |
|------|-------------|
| **DF1: Changed lines** | Output renders the diff hunks against `<rev>` with review-tier annotations in the gutter on added and context lines |
| **DF2: Drift** | After the diff, a `drift:` section lists review-tier annotations that *changed* on lines the diff does **not** touch — a caller whose effect set, pause behavior, or inferred signature moved because a callee changed |
| **DF3: Diffable elaboration** | Because output is deterministic (AN4), CI can also diff two full `rask annotate --all` runs directly — the "elaborated view" is this command's output, not a separate artifact |

```
$ rask annotate --diff main src/

src/config.rk @@ -12,4 +12,5 @@
 -12 | func load_defaults() -> Config {
 +12 | func load_defaults() -> Config or Error {   « [pure] → [io]
 -13 |     return Config.builtin()
 +13 |     let data = try fs.read_text(DEFAULTS)  « ⟨pauses⟩
 +14 |     return try Config.parse(data)
  15 | }

drift: unchanged lines whose meaning changed
  src/server.rk:41   handle_request — [pure] → [io]      (via load_defaults)
  src/server.rk:41   handle_request — now ⟨pauses⟩       (I/O reachable under Multitasking)
```

DF2 is the piece even an IDE reviewer doesn't get: the IDE ghosts the buffer you have open, not the callers three files away whose behavior your change just altered.

## JSON Output

| Rule | Description |
|------|-------------|
| **JS1: Versioned schema** | `--json` emits `{version, file, annotations: [...]}` following `tool.describe` conventions (version field, empty arrays over null) |
| **JS2: Annotation record** | `{line, span: [start, end], kind, text, tier}`. Kinds: `param_mode`, `pause`, `effects`, `captures`, `match_mode`, `inferred_signature`, `inferred_context`, `binding_type`, `borrow_scope`, `optimization`, `param_name` |
| **JS3: Drift record** | With `--diff`, adds `drift: [{file, line, function, kind, before, after, via}]` |

```json
{
  "version": 1,
  "file": "src/game.rk",
  "annotations": [
    {"line": 8, "span": [214, 226], "kind": "pause", "text": "⟨pauses⟩", "tier": "review"},
    {"line": 3, "span": [40, 46], "kind": "param_mode", "text": "own user", "tier": "review"}
  ]
}
```

The sidecar is the CI integration point: a PR bot consumes it to decorate the diff the way the IDE decorates the buffer. The bot is a consumer of this schema, not part of this spec.

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Multiple annotated calls on one line | AN4 | Annotations grouped per call, in call order |
| Annotation wider than the gutter | AN2 | Continuation line below (` | « …`), never truncated |
| Generic function effects | TR1 | Definition shows the union across instantiations (`comp.effects/INF1` is per-instance; the union is the honest definition-site summary) |
| Borrow-scope line numbers | TR2 | Source line numbers, which the report already shows — full tier only |
| File doesn't typecheck | AN5 | Print diagnostics, exit nonzero, no partial annotation |
| Hover-only payloads (comptime values, struct layout, loop modes, reflection dumps) | — | Out of scope: point queries, not line facts. They stay IDE/hover territory |

## Error Messages

```
ERROR [tool.annotate/AN5]: cannot annotate — file does not typecheck
   |
   = annotations are computed from type, ownership, and effect analysis
   = fix the errors below, then re-run

<normal rask check diagnostics follow>
```

---

## Appendix (non-normative)

### Rationale

**This is not "the language needs generated comments to be readable."** The source is readable for what it says — intent, the irreversible actions (`own`, `take` in signatures), error flow (`try`), cost (`.clone()`, `spawn`). What annotate recovers is the layer the language *deliberately* keeps out of source (`mem.parameters`, `comp.effects`): call-site mutation, effects, pause points. A reviewer who never runs annotate is exactly where the standing rationale put them — "the signature is one jump away." Annotate removes the jump; it isn't a prerequisite for reading. If practice shows reviewers genuinely can't live without this layer inline, the fix is the syntax door in `mem.parameters` (optional call-site markers), not more tooling.

**Why a report and not comments in the source (`rask fmt --explicit`)?** First draft of this spec emitted valid Rask with `//~` ghost comments appended. Review killed it, correctly, three ways: valid source invites saving, and saved output is a set of claims the compiler no longer checks — stale the moment code changes; hand-edited annotations lie with the compiler's voice; and the marker becomes a reserved comment form policed by a lint. A report format (AN2) forecloses all three structurally instead of by lint — there is nothing to commit, nothing to edit, nothing to reserve. The whole point of the ghost layer is that it's *recomputed*; annotate is a lens you hold up to the code, never a state the code is in.

**Why not call-site markers in the language instead?** That's the standing decision in `mem.parameters` (appendix, "Why no call-site markers"): mark the irreversible action (`own`), not the reversible one (`mutate`), and don't pay per-call ceremony for per-definition contracts. Annotate is the missing half of that argument — the recovery path for the reviewer that rationale used to wave at the IDE for.

**Why fold the "elaborated view" in?** An elaborated module view that reviewers diff instead of raw source was a candidate direction. But a canonical elaboration is exactly what `annotate --all` on a full file already is, given determinism (AN4). A second artifact with its own format would drift from the first.

**The tier cut (TR1).** The review tier is derived from what a reviewer must not be wrong about: does this call change my data (`mutate`), destroy it (`own`, `[consumes]`), suspend mid-lock (`⟨pauses⟩`), touch the outside world (`[io]`), or depend on something not in the parameter list (`using`)? Types and scopes help you read faster; getting them wrong doesn't ship a bug past review the same way "didn't know that call was I/O" does.

**Drift (DF2) is the feature.** Annotated changed lines are convenience — the reviewer could have chased signatures by hand. Effect and pause drift on *untouched* lines is information no reading of the diff can produce, with any tooling, because the affected lines aren't in the diff. It's also cheap: effect inference is already transitive with fixed-point propagation (`comp.effects/FX2`), so "which functions' summaries changed between two revisions" falls out of comparing two effect maps.

**Explicit capture lists on cross-task closures** (the "surface it in source instead" direction): deferred, door open. `own` already marks the escape/move decision at the closure site, `mutate x` captures are already explicit, and cross-task closures require `own` today — what's invisible is only capture *membership* and per-variable copy-vs-move, which the review tier covers. Adding explicit capture-list syntax stays possible without breaking anything (it would be optional annotation on existing syntax, same shape as the call-site-marker door in `mem.parameters`).

### Implementation Notes

Cheapest first slice: effect labels. `rask-effects` already computes the map and `Effects::label()` already renders the exact ghost strings — it's unit-tested and has no callers. Precedent for the command shape: `rask unsafe` (`cmd_unsafe_report`), which dumps compiler-classified unsafe operations as grouped text or `--json` — the same "information without enforcement" surface this spec generalizes.

The LSP inlay-hint provider and annotate should share one span→label layer (AN3 is a rule, not an aspiration): today `inlay_hints.rs` is hard-coded to inferred binding types, and every other ghost in this table is unimplemented on both surfaces. Building the shared layer once serves both.

`rask check --explicit-context` (specced in `comp.hidden-params`, unimplemented) is subsumed by `kind: inferred_context`.

### See Also

- `tool.describe` — declared API surface (`rask api`); annotate covers what it deliberately excludes ("Type inference results — only explicitly written types")
- `tool.lint` — `@pure` checking against the same effect metadata
- `comp.effects` — effect inference and the ghost label format
- `mem.parameters` — the call-site-marker decision this spec completes
- `mem.closures` — capture list ghost format
- `conc.io-context` — pause point rules
- `type.gradual` — inferred signatures, IS3 honesty rule
