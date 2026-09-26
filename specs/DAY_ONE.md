<!-- id: day-one -->
<!-- status: decided -->
<!-- summary: The reading set — the thirteen concepts you need to read Rask. This page is a budget, not a tutorial. -->

# Day-One Rask

This page is the **reading set**: what you need to know to read someone else's Rask. Not the whole language — the part that can appear in any codebase without warning. Everything else either announces itself in the types, is opt-in, or arrives as a compile error that explains itself.

This page is a budget. If it stops fitting on a page, the language got bigger — see the rule at the bottom.

## The thirteen

1. **Values are owned.** Assignment moves big values, copies small ones (≤16 bytes, all-Copy fields). `.clone()` keeps both — the visible cost. Use a moved value and the compiler names what moved and why.

2. **`let` and `mut`.** Immutable and mutable bindings. Newlines end statements.

3. **`func` and `extend`.** Functions; methods live in `extend Type` blocks. `public` is the only export marker.

4. **Parameter modes.** Read-only by default. `mutate` marks mutable access at both ends — `func f(mutate x: T)` and `f(mutate x)`. `take` consumes; callers may write `own x` for emphasis. Receivers are never marked.

5. **Errors are values.** `T or E` in the return type. `try x` propagates the error to the caller. `catch` handles it here, binder mandatory — a value (`x catch e => f(e)`, or `x catch _ => v` when the error is dropped), or an exit written where it happens (`x catch _ => return E`). `!` panics with the error's message. No exceptions.

6. **Optionals.** `T?` is "value or absent." `if x? as v` tests and binds; `x ?? v` supplies a value instead; `try x` propagates the absence. `try` is shared with errors; the fallback word is not — `?` marks something missing, `catch` something failed.

7. **Collections.** `Vec<T>` and `Map<K, V>`. Element access is inline (`v[i].field`, one expression) or `with v[i] as x { ... }` for several statements.

8. **Strings.** `string` is immutable and copies freely. Interpolation: `"hi {name}"`.

9. **`ensure`.** Cleanup that runs when the block exits — early return, error, or panic included. Written where the resource is made: `ensure file.close()`.

10. **Pattern matching.** `match` for branches, `if x is Pattern` for one check.

11. **Interfaces.** `Type implements Interface` declares conformance. `any Interface` holds mixed types — the cast allocates, and writing it is the marker.

12. **Concurrency.** `using Multitasking { }` once, near the top of `main`. `spawn(|| { ... })` returns a handle you must `.join()` or `.detach()`. Channels move values between tasks. No `async`/`await` — calls look like calls.

13. **A value can live in a container you reach through.** The type says which: `Shared<T, S>` when several names touch one value — reach it scoped, `with s.write() as v { ... }`. `Rack<T>` + `Link<T>` when many things point at each other — a link is storable in a field, and deleting a node sets every `Link<T>?` aimed at it to `none`, so there is no stale link to check for. `Heap<T>` for one owner behind an indirection. A function that deletes nodes you didn't hand it says `deleting`, and that call revokes your links.

## This is not the learning path

The book is: thirteen chapters under "Learn the language", of which four are
written. The outline is rask-lang/rask#1274 and the live order is
`docs/book/src/SUMMARY.md` — not repeated here, because a second copy of a
table of contents is a copy that drifts.

The two are different budgets and the filename hides it. A *learning path* is
ordered and starts from nothing. A *reading set* is unordered and answers "what
can appear in code I didn't write". The book teaches a chapter at a time; this
page lists what you must already recognise.

The two should cover the same ground, and that is a real check rather than a
slogan: the book's "When one owner isn't enough" is item 13 here, which is the
only reason to trust item 13 — the audit that produced it used the wrong corpus
(see the note under the budget rule). Mapping the other twelve onto the
thirteen chapters hasn't been done.

## What's deliberately not here

**The compiler teaches these when you meet them** — each arrives as an error that explains the rule: linear resources (`@resource`, consume-exactly-once), stale pool handles, disjoint field borrows, borrow escapes, `staged()` lock updates, runtime-scope errors.

**Opt-in, announced by the code that uses them:** `Rack<T>` + `Link<T>`, `Atomic<T>`, `comptime`, `unsafe`/FFI, duck interfaces and inferred signatures (sketch mode, lint-fenced).

## The budget rule

Anything added to *this page* gets the scrutiny new syntax gets (see the Ceremony Test in [CORE_DESIGN.md](CORE_DESIGN.md)). The other two piles can grow cheaply; this one is the language's size as users experience it. `spec.metrics` tracks it: the five validation programs must read using only this page.

**Item 13 was added by running that audit**, which had never been run. Three of the five needed a container the page called opt-in: `http_api_server` a `Shared`, `game_loop` a `Rack` and `Link`, `text_editor` a `Pool` (which `mem.racks` replaces). Announcing itself in the type is what makes the *writer's* choice visible; it does nothing for a reader who has not met `Link`. Neither program can be rewritten out of it — a game loop needs an entity graph and a server needs state across handlers — so the page grew, which is the outcome `spec.metrics/RS` names for exactly this case.
