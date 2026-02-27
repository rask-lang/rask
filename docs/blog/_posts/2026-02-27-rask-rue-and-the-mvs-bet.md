---
layout: post
title: "Rask, Rue, and the Mutable Value Semantics Bet"
date: 2026-02-27 12:00:00 +0100
categories: design
---

Steve Klabnik — the person who literally wrote [The Rust Programming Language](https://doc.rust-lang.org/book/) — launched [Rue](https://github.com/steveklabnik/rue) in December 2025. His question: "What if Rust wasn't trying to compete with C/C++ for the highest performance possible?"

That's basically the same question I started Rask with: what happens when you prioritize ergonomics over pointer-level control, while keeping memory safety and no GC?

Rue's answer looks familiar. No storable references. No lifetime annotations. Explicit mutation through `inout` parameters. Affine types. If you squint, it's the same design space as Rask's `mutate` keyword, block-scoped borrowing, and move-by-default semantics.

This isn't a coincidence. It's convergent evolution.

## Three projects, one bet

[Hylo](https://www.hylo-lang.org/) (formerly Val) started this formally — a Google Research project exploring *mutable value semantics* (MVS). The core insight: instead of tracking which references alias (Rust's approach), ban aliasing entirely. Everything is a value. You can't hold a pointer to someone else's data. If you want to mutate a parameter, you say so explicitly (`inout` in Hylo and Rue, `mutate` in Rask).

Now we have three projects making the same bet:

| | Hylo | Rue | Rask |
|--|------|-----|------|
| **Origin** | Academic (Google Research) | Klabnik (Rust book author) | Pragmatic (me, frustrated with Rust) |
| **Core model** | Pure MVS | Affine types + MVS | Values + block-scoped borrows |
| **Mutation** | `inout` | `inout` | `mutate` |
| **Storable refs** | No (projections only) | No (`inout` can't escape) | No (block-scoped views) |
| **Lifetime annotations** | None | None | None |

The fact that all three arrived at the same tradeoff independently is the strongest signal that this design space is real. It's not just "some guy's weird idea." The author of the Rust book thinks it's worth exploring.

## Where the hard problems are

Rue is early — no strings, no stdlib, no heap allocation yet. So it hasn't hit the problems that make MVS interesting. The easy part is "values move, mutation is explicit." The hard part is everything else:

**Multi-statement collection access.** You have a `Vec` of entities. You want to read a field, branch on it, update two other fields. In Rust you'd borrow `&mut entities[i]`. In a pure MVS world, you can't — no storable references, even temporary ones across statements. Rue and Hylo don't have a clean answer yet.

Rask's answer is `with`:

```rask
with pool[h] as entity {
    entity.health -= damage
    entity.last_hit = now()
    if entity.health <= 0 {
        entity.status = Status.Dead
    }
}
```

Not a closure — `return`, `try`, `break` all work naturally. Same syntax for Pool, Vec, Map, Cell, Shared, Mutex. This is the kind of problem you only discover when you try to write real programs, not toy examples.

**Graphs and entity systems.** MVS says "everything is a value." But an entity in a game has relationships — targets, parents, neighbors. You need indirection. Rask has `Pool<T>` + `Handle<T>` with generation-validated access. Each handle costs ~1-2ns to validate. That's the explicit price for never having use-after-free.

**Partial borrows.** You have a `GameState` with five fields. Two systems need to mutate different fields concurrently. Rask has field projections:

```rask
func movement(mutate state: GameState.{entities}, dt: f32) { ... }
func scoring(mutate state: GameState.{score}, points: i32) { ... }
```

Disjoint fields, no conflict, no lifetime parameters. I haven't seen Rue or Hylo tackle this.

**Implicit state threading.** Every function that touches a Pool needs the Pool passed in. That's noisy. Rask's context clauses handle it:

```rask
func damage(h: Handle<Player>, amount: i32) using Pool<Player> {
    h.health -= amount    // Compiler threads the pool automatically
}
```

## The call-site marker question

One thing Rue does differently: `inout` is marked at the call site. When you call `add_one(&x)`, you can see the mutation right there. Swift does this too (`addOne(&x)`). C# has `ref`.

Rask doesn't require this — `apply_damage(player, 10)` has no marker at the call site. The IDE shows `mutate` as ghost text, but in a diff or terminal, you have to check the signature. I wrote up the full reasoning in [the parameters spec](../../specs/memory/parameters.md), but the short version: I mark the irreversible action (`own` for ownership transfer) and leave the reversible one (mutation) to the signature + IDE. Three languages disagree with me on this. I might be wrong. If real-world usage shows it causes confusion, call-site markers can be added without breaking anything.

## What this means for Rask

Validation, mostly. When I started Rask, dropping storable references felt like a gamble. Now three independent projects are betting on the same fundamental tradeoff. The design space is real.

Rask's advantage is completeness — not in "we have more features" but in "we've hit the hard problems and found answers." `with` blocks, Pool+Handle, field projections, context clauses, resource types, `ensure` cleanup — these are all solutions to problems that pure MVS runs into once you try to write real software.

Rue will likely find its own answers to these problems. The more projects exploring this space, the better. Maybe they'll find something I missed.
