# Spec Conventions

How spec files are identified, cited, and tracked.

## File Metadata Headers

Every spec file should have these HTML comment headers at the top:

```markdown
<!-- id: mem.ownership -->
<!-- status: decided -->
<!-- summary: Single owner, move semantics, scoped borrowing -->
<!-- depends: memory/value-semantics.md -->
<!-- implemented-by: compiler/crates/rask-ownership/, compiler/crates/rask-interp/ -->
```

| Field | Required | Description |
|-------|----------|-------------|
| `id` | Yes | Globally unique spec identifier, `area.topic` format |
| `status` | Yes | One of: `decided`, `open`, `proposed`, `deprecated` |
| `summary` | Yes | One line, no period. What this spec covers |
| `depends` | If applicable | Other spec files this spec builds on (paths relative to `specs/`) |
| `implemented-by` | If applicable | Compiler crates that implement this spec |

### Spec ID Format

`area.topic` — area prefix from the directory, topic from the filename.

| Prefix | Directory | Examples |
|--------|-----------|----------|
| `mem` | memory/ | `mem.ownership`, `mem.borrowing`, `mem.pools` |
| `type` | types/ | `type.structs`, `type.enums`, `type.traits` |
| `ctrl` | control/ | `ctrl.flow`, `ctrl.loops`, `ctrl.comptime` |
| `conc` | concurrency/ | `conc.async`, `conc.sync`, `conc.channels` |
| `std` | stdlib/ | `std.collections`, `std.strings`, `std.json` |
| `struct` | structure/ | `struct.modules`, `struct.build`, `struct.c-interop` |
| `tool` | tooling/ | `tool.lint`, `tool.warnings` |
| `comp` | compiler/ | `comp.codegen`, `comp.gen-coalesce` |

### Status Values

- **decided** — Design settled, may be partially or fully implemented
- **open** — Active area, design not finalized
- **proposed** — Draft, needs review
- **deprecated** — Superseded or removed

## Rule IDs

Normative rules in spec tables get short IDs: uppercase prefix + number.

```markdown
| **O1: Single owner** | Every value has exactly one owner at any time |
| **O2: Move on assignment** | For non-Copy types, assignment transfers ownership |
```

### Naming

The prefix abbreviates the concept, not the filename. Keep it 1-3 uppercase letters:

| Spec | Prefixes | Examples |
|------|----------|---------|
| ownership.md | O, T, D | O1 (ownership), T2 (cross-task), D3 (drop) |
| borrowing.md | B, V | B1 (borrow), V3 (view) |
| structs.md | S, M, P | S1 (struct), M3 (method), P2 (projection) |

Multiple prefixes per file are fine — they group related rules.

### Scope and Citations

Rule IDs are **local to their spec file**. The spec ID provides global uniqueness.

**Full citation format:** `spec-id/rule-id`

```
mem.ownership/O1    — "single owner" rule in the ownership spec
type.structs/M3     — "same module" method rule in the structs spec
comp.gen-coalesce/GC1  — distinguishes from type.gradual/GC1
```

Use full citations when referencing rules from another spec or from compiler error messages. Within the same spec, just use the rule ID (`O1`).

### When to Add Rule IDs

- **Yes:** Normative rules in tables, key constraints cited elsewhere
- **No:** Prose paragraphs, examples, rationale, design discussion

Don't force IDs onto every paragraph. If a rule isn't something another spec or the compiler would cite, it doesn't need an ID.

## Spec File Structure

Every spec follows this structure. The `---` separates normative content from non-normative appendix.

```
<!-- metadata headers -->

# Topic Name

One-line decision statement.

## [Section]              ← Rule table + minimal example
## [Section]              ← Rule table + minimal example
## Edge Cases             ← Table with Rule column
## Error Messages         ← Normative error formats with rule citations

---

## Appendix (non-normative)
### Rationale             ← Design reasoning, references rules by ID
### Patterns & Guidance   ← Tutorials, "when to use which"
### IDE Integration       ← Ghost annotations, hover info
### See Also              ← Cross-references with spec IDs
```

### Principles

1. **Rule tables first, examples second.** Lead with the rule table. One code block follows. No prose between.
2. **No restating the table.** If the rule table says it, don't repeat it in prose.
3. **One example per rule group.** Pick the clearest one.
4. **Error messages cite their rule.** Format: `ERROR [mem.borrowing/V2]: message`
5. **Edge cases table has a Rule column.** Links each case to its governing rule.
6. **Cross-references use citation format.** `mem.borrowing/S3` not "see borrowing.md".
7. **Rationale references rules by ID.** `**S3 (no escape):** I wanted to prevent...`

### Error Message Format

```
ERROR [spec-id/rule-id]: short description
   |
N  |  offending code
   |  ^^^^^^^ explanation

WHY: One sentence explaining the underlying rule.

FIX: Concrete code showing the fix.
```

### What Goes Where

| Content type | Location | Example |
|-------------|----------|---------|
| Rules, constraints | Main spec (rule tables) | "Views released at semicolon" |
| Compiler behavior | Main spec (error messages) | `ERROR [mem.borrowing/V2]` |
| Edge cases | Main spec (table) | "Chained temporaries: ALL extended" |
| "I chose X because..." | Appendix: Rationale | "B1: I wanted to avoid wrestling" |
| "When to use which" | Appendix: Guidance | Pattern selection tables |
| IDE features | Appendix: IDE Integration | Ghost annotations |
| Links to other specs | Appendix: See Also | `mem.ownership`, `std.collections` |

### Template: [borrowing.md](memory/borrowing.md)

The borrowing spec is the reference implementation of this format.

## Adoption

Apply incrementally — add headers, rule IDs, and structure when touching a spec. No need to update all 65 files at once.

Existing specs with rule IDs (ownership.md, structs.md, value-semantics.md, etc.) already follow many of these conventions. They need the `id`/`status`/`summary` headers and the normative/appendix split.
