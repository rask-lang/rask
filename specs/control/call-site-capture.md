<!-- id: ctrl.call-site -->
<!-- status: proposed -->
<!-- summary: @call_site parameter annotations — compiler-filled argument text and location, unforgeable, forward-only propagation -->
<!-- depends: control/comptime.md, analysis/macro-story.md -->

# Call-Site Capture

A function can declare parameters the compiler fills with facts about each call site —
the source text of an argument, the call's location. Callers never fill them. This covers
what `assert_eq!`, `dbg!`, and `#[track_caller]` need macros for in Rust, with plain
typed values. Design history: [analysis/macro-story.md](../analysis/macro-story.md).

## Capture Kinds and Placement

| Rule | Description |
|------|-------------|
| **CS1: Two kinds** | `@call_site(text: param)` yields `str` — the source text of the argument for `param`. `@call_site(location)` yields `SourceLoc { file: str, line: u32, column: u32 }` — the call expression's position. Kinds are compiler builtins; the set is closed |
| **CS2: Named functions only** | `@call_site` annotates parameters of named functions. Illegal in closure literals and in trait method signatures |
| **CS3: Outside arity** | A captured parameter is not part of the call's argument list. Callers cannot fill it positionally or by name — except by forwarding (CS5) |
| **CS4: Compiler fill** | At every direct call site the compiler splices the values as constants. `text` refers to the caller's argument expression for the named parameter; `location` to the call expression itself |
| **CS5: Forward-only fill** | The only explicit fill: a named argument whose value is a captured parameter of the same kind in the calling function. Any other expression — literal, computed, stored — is a compile error. Captured values are unforgeable: a text is always something some caller wrote, a location always names a real call site |

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

// Wrapper keeps blaming ITS caller by forwarding:
func env_or_die(key: str, @call_site(location) loc: SourceLoc) -> string {
    return expect(os.env(key), "missing env {key}", loc: loc)
}
```

## Staging and Indirect Calls

| Rule | Description |
|------|-------------|
| **CS6: Runtime parameters** | Inside the callee, captured parameters are ordinary runtime parameters — `ctrl.comptime/CT8` already bars them from comptime positions. Spelling can never steer compilation. Message fusion happens through inlining and constant folding, as an optimization |
| **CS7: No indirect targets** | A function with captured parameters cannot be referenced as a function value or satisfy a function-typed position; wrap it in a closure. Calls through generic bounds are fine — they monomorphize into direct calls, capturing the call inside the generic body |

## Text Rules

| Rule | Description |
|------|-------------|
| **CS8: Filled defaults** | When the caller omitted a defaulted argument, `text` captures the default expression's text as written at the declaration |
| **CS9: Normalization and cap** | Runs of whitespace collapse to one space. Text over 256 bytes truncates with a trailing `…` |
| **CS10: Binary footprint** | Identical texts intern. `--strip-call-site` replaces text with `""` and locations with zeros for size-critical release builds |

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Recursive call inside the capturing function | CS4 | Captures the recursive call's own line unless forwarded |
| Named argument at the call site | CS4 | Text is the argument expression only, label excluded |
| Wrapper forgets to forward | CS5 | Not an error — the report points one level deeper, visible in the signature |
| Forwarding text-of-`a` into a slot documented as text-of-`b` | CS5 | Allowed — kinds match. Forwarding guarantees provenance, not correspondence; mislabeling is a wrapper bug, fabrication stays impossible |
| Captured value stored in a struct, passed on later | CS5 | Degrades to an ordinary `str`/`SourceLoc` — printable, no longer forwardable |
| Captured value in `comptime if` / `value.()` | CS6 | Compile error via CT8 — it is a runtime parameter |
| `@call_site` on a closure parameter | CS2 | Compile error |
| Trait declares a capturing method | CS2 | Compile error at the trait declaration |

## Error Messages

**Fabricating a captured value [CS5]:**
```
ERROR [ctrl.call-site/CS5]: cannot fill captured parameter `loc` with an ordinary value
   |
9  |  expect(v, "boom", loc: SourceLoc { file: "fake.rk", line: 1, column: 1 })
   |                         ^^^^^^^^^ only a captured parameter of the same kind can be forwarded here

WHY: Captured values are unforgeable — diagnostics built on them must not lie.

FIX: Declare your own capture and forward it:

  func my_helper(v: T?, @call_site(location) loc: SourceLoc) {
      expect(v, "boom", loc: loc)
  }
```

**Function value of a capturing function [CS7]:**
```
ERROR [ctrl.call-site/CS7]: `assert_eq` captures its call site and cannot be used as a value
   |
4  |  let f = assert_eq
   |          ^^^^^^^^^ an indirect call site cannot supply `a_text`, `b_text`, `loc`

FIX: Wrap it in a closure — the capture then happens at the closure's call to assert_eq:

  let f = |a, b| assert_eq(a, b)
```

---

## Appendix (non-normative)

### Rationale

**CS3/CS5 (unpopulatable, forward-only):** Anything in default position is
caller-overridable API — a positional slip or a "helpful" caller makes the diagnostic
lie. The annotation placement was chosen because it is the only surface that can express
"callers never fill this". Wrapper propagation stays explicit (no `#[track_caller]`
attribute chain): each hop is a visible parameter, and a forgotten forward is
inspectable in the signature.

**CS6 (runtime parameters):** An earlier draft made captures comptime-known with fences.
It was incoherent: comptime code evaluates once per instantiation (CT63), captured
values differ per call site — observing them at comptime forces one instantiation per
call site. Since captures are only ever visible as parameters, CT8 already provides the
guarantee the fences were for. If per-call-site comptime ever earns its keep, a capture
parameter marked `comptime` riding CT4 is the extension point — opt-in, visibly paying
the instantiation cost.

**CS1 (closed kinds):** Each kind is a fact only the compiler has. A `function` kind
(enclosing function name) can join later without touching the machinery.

### See Also

- [Comptime](comptime.md) — staging rules this leans on (`ctrl.comptime`)
- [Macro story](../analysis/macro-story.md) — the gap analysis this came from
