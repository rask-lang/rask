<!-- id: analysis.macro-story -->
<!-- status: proposed -->
<!-- summary: What macros actually buy Rust users, mapped against Rask. Most of it is already covered; three gaps remain, each closable without a macro layer: call-site capture, user annotations, comptime method dispatch -->
<!-- depends: control/comptime.md, stdlib/reflect.md, rejected-features.md -->

# The Macro Story, Without Macros

General macros stay rejected ([rejected-features.md](../rejected-features.md)): a second
language that formatters, linters, and IDEs can't see through. This doc asks the sharper
question — what do people *actually reach for macros for*, and which of those itches does
Rask still leave unscratched?

## Scorecard: Rust macro uses vs. Rask today

| Rust macro | What it buys | Rask answer | Status |
|------------|--------------|-------------|--------|
| `matches!(x, P)` | Pattern test as expression | `x is P` (language) | ✅ covered |
| `format!` / `println!` | Variadic formatting | String interpolation, compiler-known functions | ✅ covered |
| `vec![1, 2, 3]` | Literal sugar | `Vec.from([1, 2, 3])` | ✅ decided, accepted loss |
| `cfg!` / `#[cfg]` | Conditional compilation | `comptime if cfg.os == ...` (CT59) | ✅ covered |
| `#[derive(Serialize)]` | Struct-walking codegen | `comptime for` + `reflect.fields<T>()` + `value.(name)` | ✅ covered |
| `regex!` / sqlx queries | Validate a literal at compile time | `const RE = comptime Regex.parse("...")` — comptime panic is a compile error (CT46) | ✅ covered, pattern undocumented |
| `assert_eq!`, `dbg!` | Print the *expression*, not just its value | none | ❌ gap 1 |
| `#[track_caller]` | Report the caller's file:line | none | ❌ gap 1 |
| custom `#[derive]` with `#[serde(...)]`-style knobs | User-defined metadata on types/fields | Compiler-known annotations only (`@rename`, `@default`, ...) | ❌ gap 2 |
| dispatch-table generation | Call methods found by reflection | `value.(name)` is fields only (CT49) | ❌ gap 3 |
| `html!`, DSL blocks | New syntax | Write functions | 🚫 rejected, stays rejected |
| `derive_builder`, type generation | New *types* from old ones | Build scripts; CT66 forbids comptime type creation | 🚫 settled |
| lazy log arguments | Don't evaluate unless enabled | Explicit closure — cost stays visible | 🚫 by principle |

The scorecard says the rejection was right: most macro value is already structural in Rask.
Three gaps are real, and none of them needs token trees.

## Gap 1: Call-site capture

The everyday macro itch isn't codegen — it's `assert_eq!` printing `left: 5, right: 7` with
the source expression, and `dbg!(x)` echoing `x = ...`. Rust needs macros for this because a
function can't see its own call site. C# solved it without macros:
`[CallerArgumentExpression]` / `[CallerLineNumber]` — plain parameters whose defaults the
compiler fills in per call site.

Proposal: `@call_site` builtins, legal **only as parameter defaults**.

<!-- test: skip -->
```rask
func assert_eq<T: Equal + Debug>(
    a: T, b: T,
    a_text: str = @call_site.text(a),
    b_text: str = @call_site.text(b),
    loc: SourceLoc = @call_site.location(),
) {
    if a != b {
        panic("{loc.file}:{loc.line}: {a_text} != {b_text}\n  left:  {a:?}\n  right: {b:?}")
    }
}

assert_eq(parse("1+1"), 2)
// failure: "eval.rk:14: parse("1+1") != 2   left: 3  right: 2"

func dbg<T: Debug>(v: T, text: str = @call_site.text(v)) -> T {
    print("{text} = {v:?}")
    return v
}
```

Rules sketch — written to keep the blast radius small:

- `@call_site.text(param)` → `str`: source text of the argument expression for `param`.
  `@call_site.location()` → `SourceLoc { file, line, column }`. Spliced per call site as
  string constants.
- **Data, never code.** No AST, no token access, no evaluation. The text can only flow
  where any other `str` flows.
- **Comptime-known, but it cannot steer compilation.** Captured values are comptime-known
  `str`/`SourceLoc` — usable in comptime value computation, so `assert_eq`'s whole message
  prefix (`"{loc.file}:{loc.line}: {a_text} != {b_text}"`) fuses into one static string at
  compile time and the failure path costs nothing until it fires. Two restrictions keep it
  from becoming a macro: captured values are illegal in staging positions — the condition
  of a `comptime if`, the iterable of a `comptime for` — and illegal in `value.(...)`.
  Which code exists can never depend on how an argument was spelled; only *values* can.
  A residue stays: a comptime *value* computed from the text is spelling-dependent
  (`f(a + b)` vs `f((a)+(b))` can differ in a string constant). Accepted — it can only
  produce data or select code the program already contains, never synthesize code, and
  comptime evaluation is debuggable by construction (call stacks, `@comptime_print`,
  IDE hover), which is the actual complaint against macros.
- **A function sees its own call site only** — nothing upstream, nothing about other
  calls.
- Source text ends up in the binary as string constants, same as the file:line panics
  embed today. A release strip flag can blank them if that ever matters.

### Where the capture lives: three placements

The `@` builtin family is existing syntax (`@embed_file`, `@comptime_quota`); the open
question is where a capture may appear.

**A. Default expression** (shown above). A default argument is *already* an expression
the compiler inserts per call site — capture in default position reuses that semantics
exactly; the only new thing is the two builtins. Add one rule to cut the noise: a
parameter whose default captures the call site is **named-only** — it can't be filled
positionally, so `assert_eq(a, b, oops)` can't land in `a_text` by accident.

**B. Parameter annotation**, reusing gap 2's annotation syntax:

<!-- test: skip -->
```rask
func assert_eq<T: Equal + Debug>(a: T, b: T,
    @call_site(text: a) a_text: str,
    @call_site(location) loc: SourceLoc)
```

Reads cleaner, and gap 2 introduces `annotation` anyway. But it's surface reuse only:
it needs a brand-new semantic ("compiler-supplied parameter"), and wrappers still must
be able to pass the value explicitly — which reintroduces default-argument behavior
under an annotation costume.

**C. Body builtin with implicit propagation** (Zig's `@src`, Rust's `#[track_caller]`):
no signature noise, but the caller's location threads through calls invisibly — hidden
parameters, hidden cost. Rejected on transparency.

I recommend A: it's the placement where the semantics already exist, and named-only
handles the accident risk. B is a defensible surface if signature noise ends up mattering
more in practice.

**Wrapper propagation is explicit.** The problem `#[track_caller]` solves: a helper that
panics should blame the *user's* line, not its own internals. Rust threads the location
through an invisible attribute chain. Here the location is an ordinary parameter, so a
wrapper keeps the original caller by declaring its own capture and handing it down:

<!-- test: skip -->
```rask
func expect<T>(v: T?, msg: str, loc: SourceLoc = @call_site.location()) -> T {
    let x = v is Some else { panic("{loc.file}:{loc.line}: {msg}") }
    return x
}

// Wants failures blamed on ITS caller, not on line 3 below:
func env_or_die(key: str, loc: SourceLoc = @call_site.location()) -> string {
    return expect(os.env(key), "missing env {key}", loc: loc)   // hand it down
}
```

Each hop is a visible parameter. Forget to pass `loc` and the report points one level
deeper — annoying, but inspectable in the signature, which an attribute chain never is.

Cost: a couple of interned strings per call site — bounds-check tier, implicit is fine.
Tooling: call sites are ordinary calls; nothing to expand. Rides the existing
default-argument machinery.

This one feature covers `assert!`, `assert_eq!`, `dbg!`, `#[track_caller]`, and every
hand-rolled test/logging helper that wants to name what it was given.

## Gap 2: User-defined annotations

`@rename` / `@no_serialize` / `@default` exist but are compiler-known. The attribute half of
"Macros/attributes: not specified" is exactly this: let libraries declare their own
annotations and read them back through reflect.

<!-- test: skip -->
```rask
annotation validate { min: i64 = 0, max: i64 }

struct Order {
    @validate(max: 100)
    quantity: u32
}

func check<T>(value: T) -> void or ValidationError {
    comptime for field in reflect.fields<T>() {
        comptime if field.has<validate>() {
            let rule = comptime field.get<validate>()
            if value.(field.name) as i64 > rule.max { ... }
        }
    }
}
```

Annotations are pure data — declared shape, comptime-readable, no behavior of their own.
Whoever walks the fields decides what they mean. This is `#[serde(...)]`-grade
extensibility with zero codegen: the same residue mechanism (CT57) that already powers
encoding. It's also Principle 5 verbatim — metadata surfaced, nothing enforced.

### The API contract

**Not traits.** A trait is a behavior contract; an annotation is a data record. There is
no conformance, no dispatch, no methods, and no "annotation processor" hook (Java's
mistake — behavior belongs in the library that reads the data, not in the annotation).

**Declaration** is a restricted struct: fields with optional defaults, field types limited
to the const-representable set (primitives, `str`, enums and fixed arrays of these —
the CT58 splice set). Optionally declares its targets: `annotation validate on field`;
attaching it anywhere else is a compile error, so metadata can't sit somewhere no reader
will ever look. Default: attachable anywhere.

**Attachment is checked as construction.** `@validate(max: 100)` type-checks exactly like
the struct literal `validate { max: 100 }` — non-defaulted fields required, names checked,
values must be comptime constants. That's the whole checking story; the existing struct
diagnostics do the work. Duplicate attachment of the same annotation to one item is an
error, which keeps reading unambiguous.

**Reading** is three operations on reflect items (fields, variants, methods, and the
type itself), all comptime:

<!-- test: skip -->
```rask
field.has<validate>()   // -> bool
field.get<validate>()   // -> validate?   (the record, or none)
field.annotations       // comptime array of all attached, for generic tooling
```

That's the entire surface: declare, attach, `has`, `get`, enumerate. Anything an
annotation "does" is written as ordinary code in whatever walks it.

## Gap 3: Comptime method dispatch

`value.(field.name)` resolves fields by comptime string — this is settled canon
(`ctrl.comptime/CT49`, decided and implemented; the entire encoding story runs on it).
The parens are the signal that the member name is computed rather than literal, while
staying visually a field access — the alternatives were worse: `value[expr]` collides
with indexing, and a `reflect.get(value, name)` builtin hides that it compiles to a
direct field access. The same move on *methods* would finish the reflection story:

<!-- test: skip -->
```rask
comptime for m in reflect.methods<T>() {
    comptime if m.name.starts_with("cmd_") {
        registry.insert(m.name[4..], |args| self.(m.name)(args))
    }
}
```

Command routers, RPC dispatch, test discovery — the dispatch-table macros — with the exact
CT53/CT54 rules fields already have: name must be comptime-known, must exist, resolves to a
direct call. No dynamic reflection, no vtables, monomorphized like everything else.

## No new feature needed: validated literals

The `regex!` / checked-SQL story already falls out of the comptime spec and just needs
documenting as a pattern:

<!-- test: skip -->
```rask
const RE = comptime Regex.parse("[a-z]+@[a-z]+")   // bad pattern = compile error, with position
const Q  = comptime sql.check(@embed_file("schema.sql"), "SELECT id FROM users WHERE ...")
```

Parse/validate in an ordinary function, call it under `comptime`, freeze the result. A panic
becomes a compile error pointing at the literal (CT46). The only candidate addition is a
lint, not a feature: "argument is a literal and the callee is comptime-evaluable — consider
`comptime`" (Information Without Enforcement).

## What stays out, and why the limits hold

- **Expression/token access, `quote`-style templates** — the second language. Rejected once,
  stays rejected; C3 is the cautionary tale ([c3-lessons.md](c3-lessons.md)).
- **Comptime type creation** — CT66 is load-bearing: fixed type set is what makes comptime
  deterministic and order-free (CT65). Types from schemas remain build-script territory.
- **User syntax (`vec![]`, DSL blocks)** — tooling opacity. `Vec.from([...])` stands.
- **Lazy parameters** — hidden non-evaluation is hidden control flow. What the explicit
  version costs, side by side:

  <!-- test: skip -->
  ```rask
  // Rust macro: args silently not evaluated when the level is off
  //   debug!("state: {}", expensive_dump(world))

  log.debug("tick {n}")                        // cheap case: plain string, nothing changes
  log.debug(|| "state: {expensive_dump(world)}")  // expensive case: || marks "maybe skipped"
  ```

  `debug` overloads on `str` and `func() -> string`. Three extra characters buy visible
  control flow; the reader sees exactly which arguments might never run.

The pattern in all three gaps: macros are being used as a workaround for *information a
function can't see* — its call site, user metadata, a type's methods. Rask can hand over the
information directly, as typed values, and skip the code-generation detour entirely.
