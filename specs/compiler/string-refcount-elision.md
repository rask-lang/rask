// SPDX-License-Identifier: (MIT OR Apache-2.0)

<!-- id: comp.string-refcount-elision -->
<!-- status: proposed -->
<!-- summary: Compiler elides atomic refcount ops for string copies that are provably sole-owner -->
<!-- depends: stdlib/strings.md, compiler/clone-elision.md, memory/value-semantics.md -->

# String Refcount Elision

`string` is Copy — assignment copies 16 bytes. For heap strings (> 15 bytes), the copy also bumps an atomic refcount. SSO strings (≤ 15 bytes, `std.strings/S8`) have no refcount — they're pure value copies and bypass this pass entirely. For the remaining heap strings, the atomic increment/decrement pair is the main runtime cost. This pass eliminates those atomic ops when the compiler can prove they're unnecessary.

This is a compiler optimization. No user annotation, no semantic change. Strings behave identically whether ops are elided or not.

## Rules

| Rule | Description |
|------|-------------|
| **RE1: Inc/dec cancellation** | Copy followed by drop of the original (last use) → no atomic ops. Just memcpy 16 bytes. The increment and subsequent decrement cancel out |
| **RE2: Local-only strings** | String created in function and never escapes → skip all atomic ops. Initialize refcount to 1, free on drop without atomic decrement |
| **RE3: Literal propagation** | String literals have sentinel refcount (`std.strings/S6`). Compiler tracks "provably literal" through local analysis — no inc/dec needed |
| **RE4: Borrow chain elision** | `f(g(s))` where `s` is borrowed at each call → no refcount ops at call boundaries. Follows from `mem.ownership/O2` borrow inference |
| **RE5: IDE annotation** | IDE shows `[rc elided]` ghost text on string copies where refcount ops are skipped |
| **RE6: SSO bypass** | SSO strings (≤ 15 bytes, `std.strings/S8`) have no refcount. All refcount operations on SSO strings are no-ops by construction. The elision pass skips SSO-typed bindings entirely |

## Examples

### RE1: Inc/dec cancellation

<!-- test: skip -->
```rask
func process(user: User) {
    const name = user.name       // copy: normally inc refcount
    send(name)
    // user.name never used again → inc + dec cancel out
    // result: plain 16-byte memcpy, zero atomic ops
}
```

Without elision: `atomic_inc` on copy, `atomic_dec` when `user.name` drops at function exit. With RE1: neither fires — the inc and dec cancel.

### RE2: Local-only strings

<!-- test: skip -->
```rask
func format_greeting(first: string, last: string) -> string {
    const full = "{first} {last}"    // new string from interpolation
    const upper = full.to_uppercase() // new string, full is last-used here
    return upper
}
```

`full` is created locally (interpolation), used once, then dropped. Never escapes. Refcount stays at 1 the entire time — no atomic ops on creation or destruction.

`upper` is returned — it escapes, so its refcount ops are NOT elided. The caller's copy semantics take over.

### RE3: Literal propagation

<!-- test: skip -->
```rask
func log_level() -> string {
    const prefix = "INFO"     // literal → sentinel refcount
    const msg = prefix        // copy of literal → still sentinel
    return msg                // returns literal — no atomic ops anywhere
}
```

Literals use sentinel refcount (never incremented, never freed). When the compiler proves a binding traces back to a literal through only copies, the sentinel propagates — all inc/dec become no-ops.

### RE4: Borrow chain elision

<!-- test: skip -->
```rask
func render(name: string) -> string {
    return format(normalize(name))
    // name is borrowed through normalize → format
    // no refcount ops at either call boundary
}
```

Borrow inference (`mem.ownership/O2`) means `name` is borrowed, not copied, at each call site. No refcount change for borrows — this is already guaranteed by the ownership model, stated here for completeness.

## Escape Analysis

A string binding is "rc-required" (cannot be elided) when any of these hold:

| Escape condition | Why |
|-----------------|-----|
| Stored in collection that escapes function | Collection may outlive the binding |
| Passed to `take` parameter | Ownership transfers to callee |
| Captured by escaping closure | Closure may outlive the function |
| Sent cross-task (channel, spawn capture) | Another task may hold a reference |
| Stored in `Shared<T>` or `Mutex<T>` | Concurrent access possible |
| Returned from function | Caller takes ownership |

If none of these conditions hold at function exit, the binding's refcount ops are all no-ops.

## Edge Cases

| Case | Handling |
|------|----------|
| String in `Vec<string>` that escapes | NOT elided — collection escapes |
| String passed to `take` param | NOT elided — ownership transfers |
| String captured by inline closure | Elided — inline closure doesn't escape |
| String captured by escaping closure | NOT elided — closure may outlive function |
| String in `Shared<T>` or sent via channel | NOT elided — concurrent access |
| String returned from function | NOT elided — escapes to caller |
| String copy in loop | Conservative — NOT elided unless loop body is sole-use per iteration |
| Multiple copies of same string, some escape | Only non-escaping copies elided |
| SSO string (≤ 15 bytes) | RE6 — no refcount exists, skip entirely |
| String transitions SSO → heap (e.g., concat result) | New heap string gets normal refcount treatment |

## Implementation

MIR-level optimization pass, runs after clone elision (`comp.clone-elision`):

1. Identify all string-typed bindings in the function
2. For bindings provably SSO (constant ≤ 15 bytes, literal ≤ 15 bytes): skip — no refcount ops emitted (RE6)
3. For remaining bindings, set `rc_required = false`
4. Walk the MIR: set `rc_required = true` if any escape condition (above) applies
5. For bindings where `rc_required` is still false, replace all `atomic_inc`/`atomic_dec` with no-ops
6. For RE1 specifically: detect copy-then-drop pairs and cancel them regardless of escape status

Compile-time cost: O(n) per function — single forward pass over MIR. Negligible.

**Interaction with clone elision:** Clone elision runs first and may eliminate some copies entirely. Refcount elision handles the remaining copies that survive clone elision.

---

## Appendix (non-normative)

### Rationale

**RE1 (cancellation):** The most common pattern from the string audit — copy a field out of a struct, original drops at scope exit. ~60 string copies identified across validation programs; most are this pattern. The inc+dec pair is pure overhead when the original is about to die anyway.

**RE2 (local-only):** Strings built inside a function (interpolation, builder, `.to_string()`) that never leave the function don't need atomic ops at all. The refcount is always 1 — just track it as a regular allocation and free on drop.

**RE3 (literal propagation):** String literals are static — sentinel refcount means "never touch the refcount." When a chain of copies all trace back to a literal, the sentinel propagates. Common in logging, error messages, format strings.

**RE5 (IDE annotation):** Same rationale as `comp.clone-elision/CE5`. Visibility helps developers understand the cost model. Over time, developers learn which patterns are free and write naturally efficient code.

**RE6 (SSO bypass):** SSO strings (`std.strings/S8`) have no heap header and no refcount field. The elision pass doesn't need to analyze them — there's nothing to elide. This is the first line of defense: strings ≤ 15 bytes never generate atomic ops in the first place. RE1–RE5 handle the remaining heap strings.

**Expected impact:** SSO eliminates refcount ops for the most common short strings (field names, status codes, short identifiers, error tags). Based on the string audit (~60 copies across ~5,000 lines of validation programs), a significant fraction are short strings that would be SSO. For the remaining heap strings, RE1 alone covers the most common pattern. Combined estimate: 85-90% of string copy/drop pairs have zero atomic overhead.

### Why This Is String-Only

`string` is a language primitive — the compiler knows its exact layout, refcount location, and immutability guarantee. This optimization is not available to user-defined types, even if structurally similar. The compiler cannot verify deep immutability of arbitrary types without a new annotation system, and the cost of getting it wrong (data races from elided refcounts on mutable shared data) is too high.

For cheap sharing of arbitrary data, use `Shared<T>`. It's explicit, visible, and correct.

### See Also

- [Clone Elision](clone-elision.md) — Last-use clone-to-move optimization (`comp.clone-elision`)
- [String Handling](../stdlib/strings.md) — String spec, refcount semantics (`std.strings/S6`)
- [Value Semantics](../memory/value-semantics.md) — Copy threshold and string's Copy status (`mem.value`)
- [Ownership](../memory/ownership.md) — Borrow inference (`mem.ownership/O2`)
