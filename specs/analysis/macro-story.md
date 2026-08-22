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

> Gaps 1 and 2 have graduated into proposed specs:
> [control/call-site-capture.md](../control/call-site-capture.md) (`ctrl.call-site`, CS1–CS10)
> and [types/annotations.md](../types/annotations.md) (`type.annotations`, AN1–AN7).
> Gap 3 stays here — once accepted it amends the decided specs directly (a CT49 analog in
> `ctrl.comptime` plus `reflect.methods<T>()` in `std.reflect`), and forking those while
> proposed helps nobody. The sections below are the design history.

## Gap 1: Call-site capture

The everyday macro itch isn't codegen — it's `assert_eq!` printing `left: 5, right: 7` with
the source expression, and `dbg!(x)` echoing `x = ...`. Rust needs macros for this because a
function can't see its own call site. C# solved it without macros:
`[CallerArgumentExpression]` / `[CallerLineNumber]` — plain parameters whose defaults the
compiler fills in per call site.

Proposal: `@call_site` parameter annotations — the compiler fills these at every call
site; callers never do.

<!-- test: skip -->
```rask
func assert_eq<T: Equal + Debug>(a: T, b: T,
    @call_site(text: a) a_text: str,
    @call_site(text: b) b_text: str,
    @call_site(location) loc: SourceLoc,
) {
    if a != b {
        panic("{loc.file}:{loc.line}: {a_text} != {b_text}\n  left:  {a:?}\n  right: {b:?}")
    }
}

assert_eq(parse("1+1"), 2)
// failure: "eval.rk:14: parse("1+1") != 2   left: 3  right: 2"

func dbg<T: Debug>(v: T, @call_site(text: v) text: str) -> T {
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
- **Runtime parameters, and that falls out for free.** An earlier draft made captures
  comptime-known with fences against steering compilation. Poking at it broke it: comptime
  code in a callee evaluates once per *instantiation* (CT63), but captured values differ
  per *call site* — a callee observing them at comptime would force one instantiation per
  call site (500 asserts = 500 monomorphized bodies). The annotation placement dissolves
  the whole question: the only place a captured value is ever visible is inside the
  callee, as a parameter — and parameters are runtime values (CT8 already bars them from
  comptime). No fences needed; spelling can't steer compilation because captures can't
  reach comptime at all. The values are still compile-time-produced constants at each
  call site, and message fusion (`assert_eq` is small and inlines) happens through
  ordinary constant folding — an optimization, not a semantics. If per-call-site comptime
  ever earns its keep, the existing comptime-parameter machinery (CT4) is the extension
  point: a capture param marked `comptime` would opt into per-call-site instantiation,
  visibly. Deferred — it needs taint rules to keep capture-derived values out of staging,
  and the instantiation bloat is real.
- **A function sees its own call site only** — nothing upstream, nothing about other
  calls.
- Source text ends up in the binary as string constants, same as the file:line panics
  embed today. If size ever matters, a general diagnostics-strip build option covers
  both together — no capture-specific flag.

### Where the capture lives: the placement decision

Three placements were on the table; the parameter annotation won.

**A. Default expression** — `a_text: str = @call_site.text(a)`. Its strength: a default
argument is *already* an expression the compiler inserts per call site, so the semantics
exist. Its flaw is the same fact: anything in default position is a caller-overridable
part of the API. People *will* populate it — a positional slip lands in `a_text`, a
"helpful" caller passes their own text, and the diagnostic lies. A named-only rule
patches the slip, not the spoof. Rejected for that: captured facts shouldn't be
populatable at all.

**B. Parameter annotation** (chosen, shown above) — `@call_site(text: a) a_text: str`.

Pros:
- **Unpopulatable by construction.** The parameter is outside the call's arity: callers
  never pass it, positionally or by name. The only explicit fill is the forward rule
  (same-kind capture parameter), so text and locations are unforgeable.
- **Low noise at both ends.** `assert_eq(a, b)` at the call site; in the signature the
  captures read as metadata on trailing parameters, and IDEs can dim them.
- **In-family syntax.** Rask's `@` annotations already carry compiler behavior —
  `@native`, `@test`, `@resource`, `@rename` — this isn't stretching a data-only
  mechanism. (Gap 2's user annotations are the inert-data cousins; `@call_site` is a
  compiler-known one like `@native`.)

Costs, stated honestly:
- **One new semantic: the compiler-supplied parameter.** A parameter class that exists in
  the signature but not in the call arity. It's new machinery — the price of
  unpopulatable.
- **Function values need a rule.** An indirect call (through a closure or function value)
  has no meaningful callee-declared capture — the compiler at the indirect call site
  doesn't know the target wants one. v1 rule: referencing a function with `@call_site`
  parameters as a value is a compile error naming the parameter; wrap it in a closure
  (which then captures at the closure's own call site of the real function). Option A
  shares this problem — defaults don't exist in function types either — so it's a cost
  of the feature, not of the placement.
- **Capture kinds are closed.** `text` and `location` (maybe `function` later) are
  compiler builtins; users can't define new capture kinds. That's deliberate — each kind
  is a fact only the compiler has.

**C. Body builtin with implicit propagation** (Zig's `@src`, Rust's `#[track_caller]`):
no signature noise, but the caller's location threads through calls invisibly — hidden
parameters, hidden cost. Rejected on transparency.

**Why A's "reuses existing semantics" argument doesn't survive.** It was true only while
captures were overridable API — that's what a default *is*. Require unforgeability and
even the default placement needs the forward rule, at which point A and B have identical
semantics and the same new machinery; only the surface differs. A's surface then lies
(looks like an ordinary overridable default, isn't), B's surface says exactly what
happens. The annotation isn't the costlier option that won on ergonomics — it's the
honest spelling of the chosen semantics.

### Poking at it: where it cracks

Adversarial pass over the chosen design. Found four, one of which reshaped a rule (the
comptime story above); the rest get rules or honest caps here.

- **The indirect-call hole is a family, not a case.** Function values were already
  restricted, but closures declaring captures and trait methods called through
  `any Trait` vtables have the same problem: an indirect call site can't know the target
  captures. One unified v1 rule instead of three: `@call_site` parameters are legal only
  on named functions, and a capturing function can't be referenced as a value, used as a
  closure body's implicit target, or declared in a trait's method signature. Generic
  bounds are fine — calls through `T: Trait` monomorphize into direct calls, and the
  captured location is the call inside the generic body, which is the right answer.
- **Forwarding guarantees provenance, not correspondence.** With two text captures,
  a wrapper can forward its text-of-`a` into a callee slot documented as text-of-`b` —
  kinds match, meaning scrambled. Tightening this would need per-argument kind identity,
  which wrappers structurally can't satisfy. Honest cap: a captured `str` is genuinely
  something some caller wrote and a `SourceLoc` genuinely names a real call site;
  *which* argument it describes is trusted to the forwarding code, same as any other
  argument order. Diagnostics can be stale-labeled by a buggy wrapper, not fabricated.
- **Text needs three edge rules.** (1) An argument the caller didn't write — a filled
  default — captures the default expression's text as written at the declaration.
  (2) Text is the source slice with runs of whitespace collapsed, so multiline arguments
  don't break diagnostic layout. (3) Text longer than 256 bytes is truncated with `…` —
  captures are for diagnostics, not storage, and unbounded capture of a huge closure
  argument is binary bloat for nothing.
- **Binary-size pressure is real but boring.** Every capturing call site embeds a string.
  Identical texts intern; stripping belongs to a general build-level diagnostics-strip
  option shared with panic locations, not a per-feature flag.

Gap 2, poked: repeated annotations stay forbidden — the `@alias("a") @alias("b")` want is
served by an array field, `@alias(names: ["a", "b"])`. And removing a public annotation
is a semver-relevant API change (downstream walkers change behavior) — same class as
removing a field, worth a line in the eventual spec.

Gap 3, poked: a dispatch table built from `reflect.methods<T>()` assumes the filtered
methods share a signature; when one doesn't, the type error lands inside an unrolled
iteration. That's fine — but the diagnostic must carry the comptime-for context ("while
unrolling for m = cmd_restart") or it's unreadable. Same requirement encoding already
imposes, so the machinery exists.

**Wrapper propagation is forward-only.** The problem `#[track_caller]` solves: a helper
that panics should blame the *user's* line, not its own internals. Rust threads the
location through an invisible attribute chain. Here a wrapper declares its own capture
and forwards it — and forwarding is the *only* way to fill a captured parameter
explicitly:

<!-- test: skip -->
```rask
func expect<T>(v: T?, msg: str, @call_site(location) loc: SourceLoc) -> T {
    let x = v is Some else { panic("{loc.file}:{loc.line}: {msg}") }
    return x
}

// Wants failures blamed on ITS caller, not on the expect() line below:
func env_or_die(key: str, @call_site(location) loc: SourceLoc) -> string {
    return expect(os.env(key), "missing env {key}", loc: loc)   // forward
}
```

The forward rule: a captured parameter of kind K (text / location) may be filled only by
naming a captured parameter of the same kind K. Anything else — a literal, a computed
string, a stored value — is a compile error. So captured facts are unforgeable: a `str`
that arrived through `@call_site(text: ...)` is always genuinely what some caller wrote,
and a `SourceLoc` always names a real call site. Diagnostics built on them can't lie.
Each hop is still a visible parameter; forget to forward and the report points one level
too deep — inspectable in the signature, which an attribute chain never is.

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
annotation @validate { min: i64 = 0, max: i64 }

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
the CT58 splice set). Optionally declares its targets: `annotation @validate on field`;
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
