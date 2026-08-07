<!-- id: day-one -->
<!-- status: decided -->
<!-- summary: The reading set — the twelve concepts you need to read Rask. This page is a budget, not a tutorial. -->

# Day-One Rask

This page is the **reading set**: what you need to know to read someone else's Rask. Not the whole language — the part that can appear in any codebase without warning. Everything else either announces itself in the types, is opt-in, or arrives as a compile error that explains itself.

This page is a budget. If it stops fitting on a page, the language got bigger — see the rule at the bottom.

## The twelve

1. **Values are owned.** Assignment moves big values, copies small ones (≤16 bytes, all-Copy fields). `.clone()` keeps both — the visible cost. Use a moved value and the compiler names what moved and why.

2. **`const` and `mut`.** Immutable and mutable bindings. Newlines end statements.

3. **`func` and `extend`.** Functions; methods live in `extend Type` blocks. `public` is the only export marker.

4. **Parameter modes.** Read-only by default. `mutate` marks mutable access at both ends — `func f(mutate x: T)` and `f(mutate x)`. `take` consumes; callers may write `own x` for emphasis. Receivers are never marked.

5. **Errors are values.** `T or E` in the return type. `try x` propagates the error to the caller. `catch` handles it here, binder mandatory — a value (`x catch e => f(e)`, or `x catch _ => v` when the error is dropped), or an exit written where it happens (`x catch _ => return E`). `!` panics with the error's message. No exceptions.

6. **Optionals.** `T?` is "value or absent." `if x? as v` tests and binds; `x ?? v` supplies a value instead; `try x` propagates the absence. `try` is shared with errors; the fallback word is not — `?` marks something missing, `catch` something failed.

7. **Collections.** `Vec<T>` and `Map<K, V>`. Element access is inline (`v[i].field`, one expression) or `with v[i] as x { ... }` for several statements.

8. **Strings.** `string` is immutable and copies freely. Interpolation: `"hi {name}"`.

9. **`ensure`.** Cleanup that runs when the block exits — early return, error, or panic included. Written where the resource is made: `ensure file.close()`.

10. **Pattern matching.** `match` for branches, `if x is Pattern` for one check.

11. **Traits.** `extend Type with Trait` declares conformance. `any Trait` holds mixed types — the cast allocates, and writing it is the marker.

12. **Concurrency.** `using Multitasking { }` once, near the top of `main`. `spawn(|| { ... })` returns a handle you must `.join()` or `.detach()`. Channels move values between tasks. No `async`/`await` — calls look like calls.

## What's deliberately not here

**The compiler teaches these when you meet them** — each arrives as an error that explains the rule: linear resources (`@resource`, consume-exactly-once), stale pool handles, disjoint field borrows, borrow escapes, `staged()` lock updates, runtime-scope errors.

**Opt-in, announced by the code that uses them:** `Pool<T>` + `Handle<T>`, the box family (`Cell`, `Shared`, `Mutex`, `Owned`), `Atomic<T>`, `comptime`, `unsafe`/FFI, context clauses (`using Pool<T>`), duck traits and inferred signatures (sketch mode, lint-fenced).

## The budget rule

Anything added to *this page* gets the scrutiny new syntax gets (see the Ceremony Test in [CORE_DESIGN.md](CORE_DESIGN.md)). The other two piles can grow cheaply; this one is the language's size as users experience it. `spec.metrics` tracks it: the five validation programs must read using only this page.
