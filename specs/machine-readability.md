# Machine Readability

Rask code should be easy to analyze by tools — linters, refactoring engines, IDE plugins, AI assistants, and anything else that reads source. This isn't a separate goal from human readability. The same properties that make code clear to developers make it clear to machines: explicit intent, consistent patterns, local reasoning.

I'm documenting this as a design principle because it shaped many decisions and should shape future ones. Every property listed here was chosen for developer ergonomics first — the fact that it also helps automated tooling is a bonus, not the motivation.

---

## Why Rask Code Is Tool-Friendly

These properties emerged from other design goals but happen to make static analysis straightforward:

### Local Analysis (Principle 5)

Every function can be understood in isolation. Public signatures fully describe the interface. No cross-function inference, no whole-program analysis needed.

**What this enables:** A tool can reason about one function without loading the entire codebase. Refactoring a single file doesn't require global analysis. Incremental checking is trivial.

### Rich Signatures

Function signatures carry a lot of information in Rask:

```rask
func process(config: Config, take data: Vec<u8>) -> ProcessResult or IoError
    with Pool<Node>
```

From this single line, a tool can determine:
- `config` is read-only (won't be modified)
- `data` ownership transfers (caller loses access)
- Can fail with `IoError` (and only `IoError`)
- Needs a `Pool<Node>` in scope
- Returns `ProcessResult` on success

Most languages require reading the function body to learn half of this. In Rask, the signature is a specification.

### Keyword-Based Semantics

Rask uses words where other languages use symbols:

| Concept | Rask | Alternative |
|---------|------|------------|
| Error propagation | `try expr` | `expr?` |
| Ownership transfer | `own value` | implicit move |
| Pattern check | `x is Some(v)` | `let Some(v) = x` |
| Result type | `T or E` | `Result<T, E>` |

Keywords are unambiguous tokens. `try` means one thing in Rask. `?` means different things in different languages (ternary, null coalescing, regex quantifier, error propagation). Tools that process multiple languages benefit from unambiguous tokens; developers benefit from readable code. Same property, two beneficiaries.

### Explicit Returns

Functions require explicit `return`. There's no ambiguity about what a function produces — find the `return` statements and you have the complete picture. No implicit last-expression semantics to track (blocks use implicit last-expression, but functions don't).

### No Hidden Effects

No implicit async. No algebraic effects. No monkey patching. No operator overloading beyond the standard set. If a function does I/O, it shows up in the body. If it can fail, the return type says so. A tool reading source code gets the complete picture.

---

## The "One Obvious Way" Principle

For each common operation, there should be one idiomatic pattern. This isn't Python's "there should be one obvious way" — it's a consequence of having a small, opinionated standard library and avoiding feature duplication.

For the full canonical patterns reference covering all common operations, see [canonical-patterns.md](canonical-patterns.md). Below are the key examples.

### Canonical Patterns

**Error handling:** `try` propagation for bubbling, `match` for handling. Not `unwrap()` in production code, not `if err != nil` chains.

```rask
// Propagate
const data = try fs.read_file(path)

// Handle
match fs.read_file(path) {
    Ok(data) => process(data),
    Err(e) => log("failed: {e}"),
}
```

**Resource cleanup:** `ensure` for guaranteed cleanup. Not defer, not RAII destructors, not finally blocks.

```rask
const file = try fs.open(path)
ensure file.close()
const data = try file.read_text()
```

**Concurrency:** `spawn` for tasks, `with multitasking` for the runtime. Not async/await, not goroutine keywords.

```rask
with multitasking {
    const handle = spawn { fetch(url) }
    const result = try handle.join()
}
```

**Iteration:** `for x in collection` with adapters. Not C-style loops, not while-with-index.

```rask
for line in lines {
    if line.starts_with("#"): continue
    process(line)
}
```

**Pattern matching:** `if x is Pattern` for single checks, `match` for multiple branches. Not if-else chains on type tags.

```rask
if result is Ok(value) {
    use(value)
}

match event {
    Click(pos) => handle_click(pos),
    Key(k) => handle_key(k),
    Quit => break,
}
```

**Why one way matters:** When there's only one idiomatic pattern, code across different projects looks the same. Tools can pattern-match on idioms. Developers reading unfamiliar code recognize the shape immediately. Newcomers learn one pattern, not five.

---

## Naming Conventions

Method names should encode semantics. A developer — or a tool — should predict a method's behavior from its name alone.

### Required Patterns (Stdlib)

| Prefix/Suffix | Meaning | Returns | Examples |
|---------------|---------|---------|----------|
| `from_*` | Construction from source | `Self` or `Self or E` | `from_str(s)`, `from_bytes(b)` |
| `into_*` | Consuming conversion | new type (takes ownership) | `into_string()`, `into_vec()` |
| `as_*` | Cheap view or cast | reference or copy | `as_slice()`, `as_str()` |
| `to_*` | Non-consuming conversion | new type (may allocate) | `to_string()`, `to_lowercase()` |
| `is_*` | Boolean predicate | `bool` | `is_empty()`, `is_valid()` |
| `with_*` | Builder-style setter | `Self` | `with_capacity(n)` |
| `*_or(default)` | Unwrap with fallback | `T` | `unwrap_or(0)`, `env_or(k, d)` |
| `try_*` | Fallible version | `T or E` | `try_parse()`, `try_connect()` |

### Domain-Specific Patterns (Acceptable)

These don't follow the main table but are standard in their domain:

| Pattern | Domain | Examples |
|---------|--------|---------|
| `read_*` / `write_*` | Binary I/O | `read_u32be()`, `write_all()` |
| `decode` / `encode` | Serialization | `json.decode<T>()`, `json.encode()` |

### Audit Results

The existing stdlib specs already follow these conventions at 98%+ adherence. This section formalizes what's already in practice. Future stdlib additions must follow these patterns; `rask lint` will enforce them. See [tooling/lint.md](tooling/lint.md) for the linter spec.

---

## Error Messages as Fix Instructions

Error messages should be actionable. A developer reading an error should know exactly what to change. A tool reading an error should be able to generate the fix.

### Format

Every error message has three parts:

1. **What went wrong** — The symptom, with source span
2. **How to fix it** — Concrete code change, not vague advice
3. **Why the rule exists** — One sentence explaining the constraint

```
error[E0042]: cannot use `data` after ownership transfer

  14 | process(own data)
     |         ~~~~~~~~ ownership transferred here
  15 | println(data.len())
     |         ^^^^ used after transfer

fix: clone before transfer
  14 | process(own data.clone())

why: `own` transfers ownership — the caller can no longer access the value.
```

### Guidelines

- **Concrete fixes over vague suggestions.** "Clone before transfer" with the exact line, not "consider cloning the value or restructuring your code."
- **One primary fix.** If there are alternatives, mention them briefly after the main suggestion. Don't present three options with no guidance on which to pick.
- **The `fix:` section is machine-parseable.** Tools can extract the line number and replacement text to offer automated fixes.
- **The `why:` section teaches.** Developers learn the rule; they don't just memorize the fix.
- **Every new error must include `fix:` and `why:` text.** This is enforced in the compiler's `ToDiagnostic` implementations. The `fix` field provides a concrete action; the `why` field explains the constraint in one sentence.

---

## Tooling: `rask describe`

A command that emits structured summaries of a module's public interface. Useful for any tool that needs an API overview without reading source. For the formal JSON schema, see [tooling/describe-schema.md](tooling/describe-schema.md).

```
rask describe src/server.rk --format json
```

Output:

```json
{
  "module": "server",
  "imports": ["net", "json"],
  "types": [
    {
      "name": "Server",
      "kind": "struct",
      "fields": [
        { "name": "port", "type": "u16", "public": true }
      ],
      "methods": [
        {
          "name": "start",
          "self_mode": "take",
          "params": [{ "name": "config", "type": "Config", "mode": "borrow" }],
          "returns": { "ok": "()", "err": "ServerError" },
          "context": ["Pool<Connection>"]
        }
      ]
    }
  ],
  "functions": [],
  "enums": [
    {
      "name": "ServerError",
      "variants": ["BindFailed(string)", "ConfigInvalid(string)"]
    }
  ]
}
```

This replaces reading 200+ lines of source with a structured summary. IDE plugins, documentation generators, and AI assistants all benefit from the same data format.

**Default format:** Human-readable text summary. `--format json` for machine consumption.

## Tooling: `rask explain`

A command that generates a plain-text explanation of a function using compiler analysis (not documentation comments):

```
rask explain src/server.rk::handle_request
```

Output:

```
handle_request(conn: TcpConnection, db: Shared<Database>) -> () or ServerError

Takes ownership of conn (TcpConnection). Reads from db (shared).
Can fail with: ServerError.BindFailed, ServerError.ConfigInvalid.
Calls: parse_request, route, db.read, conn.write.
Resource: conn must be consumed (closed) before return.
```

Built from compiler knowledge: parameter modes, error types, call graph, resource tracking. The compiler already has this information — `rask explain` just surfaces it.

---

## Summary

These aren't new ideas bolted on — they're properties that fell out of existing design choices. Local analysis, explicit keywords, rich signatures, consistent naming. The spec formalizes them so future design decisions maintain these properties, and so tooling can rely on them being true.

The core insight: **code that's easy for humans to read is easy for tools to analyze.** Optimize for clarity, and machine readability follows.
