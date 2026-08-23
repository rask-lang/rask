<!-- id: ctrl.call-info -->
<!-- status: proposed -->
<!-- summary: @call_text and @call_location — compiler-filled parameters reporting the argument text and location of each call, unforgeable, forward-only propagation -->
<!-- depends: control/comptime.md, analysis/macro-story.md -->

# Call Information

A function can declare parameters the compiler fills with facts about each call — the
source text of an argument, the call's location. Callers never fill them. This covers
what `assert_eq!`, `dbg!`, and `#[track_caller]` need macros for in Rust, with plain
typed values. Design history: [analysis/macro-story.md](../analysis/macro-story.md).

## The Two Annotations

| Rule | Description |
|------|-------------|
| **CS1: Two compiler annotations** | `@call_text(param)` on a `string` parameter yields the source text of the caller's argument for `param` — an annotation taking an argument, the shape `@rename("x")` and `@default(0)` already have. `@call_location` on a `SourceLoc { file: string, line: u32, column: u32 }` parameter yields the call expression's position — a bare marker, the shape `@no_serialize` and `@test` already have. No kind vocabulary: these are two annotation names, and the set is closed |
| **CS2: Named functions only** | Both annotate parameters of named functions. Illegal in closure literals and in trait method signatures |
| **CS3: Outside arity** | A compiler-filled parameter is not part of the call's argument list. Callers cannot fill it positionally or by name — except by forwarding (CS5) |
| **CS4: Compiler fill** | At every direct call site the compiler splices the values as constants: `@call_text(p)` from the caller's argument expression for `p`, `@call_location` from the call expression itself |
| **CS5: Forward-only fill** | The only explicit fill: a named argument whose value is a parameter carrying the *same* annotation in the calling function. Any other expression — literal, computed, stored — is a compile error. Filled values are unforgeable: a text is always something some caller wrote, a location always names a real call site |

<!-- test: skip -->
```rask
func assert_eq<T: Equal + Debug>(a: T, b: T,
    @call_text(a) a_text: string,
    @call_text(b) b_text: string,
    @call_location loc: SourceLoc,
) {
    if a != b {
        panic("{loc.file}:{loc.line}: {a_text} != {b_text}\n  left:  {a:?}\n  right: {b:?}")
    }
}

assert_eq(parse("1+1"), 2)
// failure: "eval.rk:14: parse("1+1") != 2   left: 3  right: 2"

// Wrapper keeps blaming ITS caller by forwarding:
func env_or_die(key: string, @call_location loc: SourceLoc) -> string {
    return expect(os.env(key), "missing env {key}", loc: loc)
}
```

## Staging and Indirect Calls

| Rule | Description |
|------|-------------|
| **CS6: Runtime parameters** | Inside the callee, filled parameters are ordinary runtime parameters — `ctrl.comptime/CT8` already bars them from comptime positions. Spelling can never steer compilation. Message fusion happens through inlining and constant folding, as an optimization |
| **CS7: No indirect targets** | A function with filled parameters cannot be referenced as a function value or satisfy a function-typed position; wrap it in a closure. Calls through generic bounds are fine — they monomorphize into direct calls, and the call reported is the one inside the generic body |

## Text Rules

| Rule | Description |
|------|-------------|
| **CS8: Filled defaults** | When the caller omitted a defaulted argument, `@call_text` reports the default expression's text as written at the declaration |
| **CS9: Normalization and cap** | Runs of whitespace collapse to one space. Text over 256 bytes truncates with a trailing `…` |
| **CS10: Binary footprint** | Identical texts intern. Filled strings are the same class of data as the file:line strings panics already embed — a future build-level diagnostics-strip option covers both together; no flag specific to this feature |

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Recursive call inside a function with filled parameters | CS4 | Reports the recursive call's own line unless forwarded |
| Named argument at the call site | CS4 | Text is the argument expression only, label excluded |
| Wrapper forgets to forward | CS5 | Not an error — the report points one level deeper, visible in the signature |
| Forwarding text-of-`a` into a slot documented as text-of-`b` | CS5 | Allowed — same annotation. Forwarding guarantees provenance, not correspondence; mislabeling is a wrapper bug, fabrication stays impossible |
| Filled value stored in a struct, passed on later | CS5 | Degrades to an ordinary `string`/`SourceLoc` — printable, no longer forwardable |
| Filled value in `comptime if` / `value.()` | CS6 | Compile error via CT8 — it is a runtime parameter |
| `@call_text` / `@call_location` on a closure parameter | CS2 | Compile error |
| Trait declares a method with filled parameters | CS2 | Compile error at the trait declaration |

## Error Messages

**Fabricating a filled value [CS5]:**
```
ERROR [ctrl.call-info/CS5]: cannot fill `loc` with an ordinary value
   |
9  |  expect(v, "boom", loc: SourceLoc { file: "fake.rk", line: 1, column: 1 })
   |                         ^^^^^^^^^ only a `@call_location` parameter can be forwarded here

WHY: Compiler-filled values are unforgeable — diagnostics built on them must not lie.

FIX: Declare your own and forward it:

  func my_helper(v: T?, @call_location loc: SourceLoc) {
      expect(v, "boom", loc: loc)
  }
```

**Function value of a function with filled parameters [CS7]:**
```
ERROR [ctrl.call-info/CS7]: `assert_eq` has compiler-filled parameters and cannot be used as a value
   |
4  |  let f = assert_eq
   |          ^^^^^^^^^ an indirect call site cannot supply `a_text`, `b_text`, `loc`

FIX: Wrap it in a closure — the fill then happens at the closure's call to assert_eq:

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

**CS6 (runtime parameters):** An earlier draft made these values comptime-known with
fences. It was incoherent: comptime code evaluates once per instantiation (CT63), but the
values differ per call — observing them at comptime forces one instantiation per call
site. Since they are only ever visible as parameters, CT8 already provides the guarantee
the fences were for. If per-call-site comptime ever earns its keep, a filled parameter
marked `comptime` riding CT4 is the extension point — opt-in, visibly paying the
instantiation cost.

**CS1 (two annotations, not one with kinds):** The first draft was
`@call_site(text: a)` / `@call_site(location)` — one umbrella annotation with a
kind argument. Review killed it: `text:` there is an ordinary named argument, a
feature Rask already has, so it was never new syntax — but `location` was a bare
word sitting in a named-argument position, which is not a shape the language has
at all. The two are not two values of one field; one references a
parameter and one references nothing. So they are two annotation names, each
matching a shape already in the language: `@call_text(a)` like `@rename("x")`,
`@call_location` like `@no_serialize`. Nothing feature-specific to learn. A
third (`@call_function`, the enclosing function's name) could join the same way.

**Signature noise is a tooling problem, not a syntax one:** filled parameters
are the last ones in a signature and always compiler-filled, so an IDE can dim
them or fold them behind the visible arity — and it hints `@call_text`'s argument
like any other named argument. That is Principle 5 doing its job: the
information stays in the source, the presentation is the editor's business. This
is why the design does not chase a terser spelling.

### See Also

- [Comptime](comptime.md) — staging rules this leans on (`ctrl.comptime`)
- [Macro story](../analysis/macro-story.md) — the gap analysis this came from
