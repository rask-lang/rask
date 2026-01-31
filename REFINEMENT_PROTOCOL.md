# Specification Refinement Protocol (Streamlined)

## Philosophy
Use Claude Code's native capabilities without drowning in file overhead:
- Direct file access (Read/Write/Edit)
- Continuous context across all phases
- Adaptive behavior (can change strategy based on discoveries)
- **Git for versioning** (not manual snapshots)
- **Single history log** (not hundreds of metadata files)

## Directory Structure

```
rask/
├── specs/                           # Source of truth
│   ├── memory-model.md
│   ├── async-and-concurrency.md
│   ├── generics.md
│   └── ...
│
├── .refinement/                     # Working directory (gitignored)
│   ├── current/
│   │   ├── memory-model_analysis.md       # Temporary gap analysis
│   │   ├── async-and-concurrency_analysis.md
│   │   └── ...
│   │
│   └── history.jsonl                # Append-only refinement log
│
├── CORE_DESIGN.md                   # Immutable design
└── REFINEMENT_SUMMARY_{category}.md # Human-readable summary (optional)
```

### Key Principles

1. **specs/ is clean**: Only final specifications live here
2. **Git tracks history**: Each gap refinement = one git commit
3. **Working files are temporary**: `.refinement/current/` holds analysis, can be deleted
4. **Single history file**: `history.jsonl` replaces hundreds of metadata files
5. **No redundant copies**: Use `git diff` instead of storing spec_before/spec_after

## Workflow

When asked to refine a category (e.g., "Refine memory specification"):

### Phase 1: Analysis & Gap Identification

**Read:**
- `CORE_DESIGN.md` (the immutable design)
- `specs/{category}.md` (current specification)
- Other `specs/*.md` files (for cross-category awareness)

**Analyze:**
- The CORE design is FINAL. Find gaps in HOW it works, not WHETHER it's right.
- Identify:
  - **Ambiguities**: Multiple interpretations possible
  - **Underspecified**: Not detailed enough to implement
  - **Missing**: Edge cases, error conditions not covered
  - **TODOs**: Explicitly marked gaps

**Output:**
- Write analysis to `.refinement/current/{category}_analysis.md`
- List gaps with PRIORITY (HIGH/MEDIUM/LOW)
- Format for easy reference:
  ```markdown
  ## Gap 1: [Title]
  **Priority:** HIGH
  **Type:** Underspecified
  **Question:** [What implementer needs to know]
  ```

**DO NOT:**
- Challenge core design decisions
- Propose alternatives to locked decisions
- Focus on minor formatting issues

### Phase 2: Specification (Per Gap)

For each HIGH/MEDIUM priority gap (up to max_gaps, default 3):

**Read:**
- Current `specs/{category}.md` (may have been updated by previous gap)
- The specific gap from `.refinement/current/{category}_analysis.md`
- `CORE_DESIGN.md` for constraints

**Specify:**
- Write concrete, implementable specification
- Use **MUST/MUST NOT**, not "should/might"
- Use tables for edge cases (not prose paragraphs)
- Target: 50-150 lines per gap elaboration
- ONE example maximum (prefer pseudocode)

**Self-Validate:**
- Does it conflict with CORE design? (If yes: revise or skip)
- Is it internally consistent? (Check your own spec for contradictions)
- Does it conflict with other category specs? (Re-read others if needed)
- Is it complete enough to implement?
- Is it concise? (Cut redundant explanations)

**Integrate:**
- Read current `specs/{category}.md`
- Use **Edit tool** to integrate the elaboration into the right section
- OR use **Write tool** if restructuring is needed
- Preserve existing content, add new specifications
- Condense if spec is getting too long (target: <500 lines)
- **Update "Remaining Issues" section at bottom of spec:**
  - Remove the gap you just addressed
  - Add any new issues discovered during specification
  - Keep list prioritized (HIGH/MEDIUM/LOW)

**Record:**
- Append one line to `.refinement/history.jsonl`:
  ```json
  {"timestamp": "2026-01-31T10:23:45Z", "category": "memory-model", "gap": "Region lifetime semantics", "action": "integrated", "lines_added": 42, "lines_removed": 5, "commit": "abc123"}
  ```
- Git commit with message: `refine({category}): {gap-title}`
  - Example: `refine(memory-model): clarify region lifetime semantics`

**Decision Points:**
- If gap conflicts with CORE: Skip and log to history.jsonl with `"action": "skipped", "reason": "conflicts with CORE"`
- If gap requires design decision: Flag for user review (don't commit yet)
- If spec becomes too verbose: Condense as you integrate
- If change is minor: Still commit (git makes this cheap)

### Phase 3: Summary Report

After processing all gaps:

**Read back:**
- Updated `specs/{category}.md`
- `.refinement/history.jsonl` (filter by category)
- Git log for recent commits

**Report:**
- Gaps found: N
- Gaps addressed: N
- Gaps skipped: N (with reasons)
- Git commits: abc123..xyz789
- Net change: +/- lines in spec
- Remaining TODOs or open questions

**Format:**
```markdown
## Refinement Summary: {Category}

**Analysis:** {N} gaps identified (H: X, M: Y, L: Z)

**Addressed:**
1. Gap: [Title] → integrated (commit: abc123)
2. Gap: [Title] → integrated (commit: def456)
3. Gap: [Title] → skipped (conflicts with CORE)

**Specification:** specs/{category}.md
- Before: X lines
- After: Y lines
- Net: +Z lines
- Updated "Remaining Issues" section with X unresolved gaps

**Git History:**
- Commits: abc123..xyz789
- View changes: `git log --oneline specs/{category}.md`
- View diff: `git diff abc123..xyz789 specs/{category}.md`

**Next Steps:**
- See "Remaining Issues" section at bottom of specs/{category}.md
- X HIGH priority gaps still need addressing
```

**Note:** The "Remaining Issues" section at the bottom of `specs/{category}.md` serves as the living TODO list. No separate summary file needed.

## Model Selection Guidelines

When spawning **Task agents** (optional, for parallel processing):
- **Analysis tasks:** Use `model: "haiku"` (pattern matching, gap finding)
- **Specification writing:** Use `model: "opus"` (precision, creativity)
- **Validation/integration:** Use `model: "sonnet"` (balanced capability)

For direct refinement (single Claude Code session): Use default model (sonnet).

## State Management

**Files:**
- `specs/{category}.md` - Always the current/latest spec (source of truth)
  - Should end with "## Remaining Issues" section listing unresolved gaps
- `.refinement/current/{category}_analysis.md` - Latest gap analysis (temporary)
- `.refinement/history.jsonl` - Append-only refinement log
- `CORE_DESIGN.md` - Immutable truth (never modified)

**Versioning:**
- Git is the version control system
- Each gap addressed = one git commit
- Use semantic commit messages: `refine({category}): {gap-title}`
- Tag important milestones: `git tag v1.0-memory-model`

**History Log Format:**
Each line in `.refinement/history.jsonl` is a JSON object:
```json
{
  "timestamp": "2026-01-31T10:23:45Z",
  "category": "memory-model",
  "gap": "Region lifetime semantics",
  "action": "integrated|skipped|flagged",
  "reason": "conflicts with CORE" (if skipped),
  "lines_added": 42,
  "lines_removed": 5,
  "commit": "abc123def" (git commit hash)
}

Avoid using Bash with the find, grep, cat, head, tail, sed, awk, or echo commands... Use specialized tools instead.

```

## Example Commands

**Refine single category:**
```
"Refine the memory model specification following REFINEMENT_PROTOCOL.md.
Process top 3 HIGH priority gaps."
```

**Refine with specific focus:**
```
"Refine concurrency spec, focusing on thread safety and race prevention gaps.
Max 2 gaps."
```

**Refine all categories:**
```
"Refine all specs in specs/ directory.
Process 2 HIGH priority gaps each. Create git commits for each gap."
```

**Continue refinement:**
```
"Continue refining memory-model. Process 3 more HIGH priority gaps from analysis."
```

## Advanced: Parallel Refinement

For processing multiple categories simultaneously:

```
"Spawn Task agents in parallel for each spec in specs/.
Each should refine following REFINEMENT_PROTOCOL.md, processing 2 HIGH priority gaps.
Model: sonnet for all. Create git commits for integrated changes."
```

Each agent will:
- Work independently on its category
- Have full tool access (Read/Write/Edit)
- Save to separate `specs/{category}.md` files
- Append to shared `.refinement/history.jsonl` (file-safe)
- Create git commits with `refine({category}): ...` messages
- Return summary report when done

You then consolidate the results by reading summaries and git log.

## Cleanup & Maintenance

**After refinement session:**
```bash
# Optional: Clean up temporary analysis files
rm .refinement/current/*_analysis.md

# Optional: Squash refinement commits into one
git rebase -i HEAD~5  # if you made 5 commits

# Optional: Tag major milestones
git tag v1.0-memory-model
```

**View refinement history:**
```bash
# See all refinement commits
git log --oneline --grep="refine("

# See changes to specific spec
git log -p specs/memory-model.md

# View history log
cat .refinement/history.jsonl | jq .

# Filter history by category
cat .refinement/history.jsonl | jq 'select(.category == "memory-model")'
```

## Spec File Structure

Every `specs/{category}.md` should follow this structure:

```markdown
# {Category Name}

[Main specification content...]

---

## Remaining Issues

### High Priority
1. **Gap Title** — Brief description of what's missing
2. **Another Gap** — What needs to be specified

### Medium Priority
3. **Gap Title** — Description

### Low Priority
4. **Gap Title** — Description

**Note:** Issues are removed as they're addressed during refinement.
```

The "Remaining Issues" section:
- Lives at the **bottom** of each spec file
- Updated after each gap is addressed (remove completed, add newly discovered)
- Provides immediate visibility into what's still needed
- Eliminates need for separate TODO/summary files

## Notes

- **Brevity is critical**: Specs are read in LLM context windows. Keep concise.
- **Tables over prose**: Edge cases, conditions, behaviors → use tables.
- **One example max**: Only if behavior is non-obvious.
- **MUST/MUST NOT**: Imperative language, not "should" or "might".
- **Trust the CORE**: Never suggest changing locked design decisions.
- **Self-validate**: Check your own work before moving to next gap.
- **Git is cheap**: Don't fear small commits. They're better than no history.
- **JSONL is append-only**: Safe for parallel writes (each agent appends one line)
- **Analysis files are temporary**: Delete after session if you want (git has the real history)
- **"Remaining Issues" is living**: Update it every refinement, it's the TODO list
