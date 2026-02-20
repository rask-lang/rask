---
layout: post
title: "The Soul of Rask"
date: 2026-02-20 12:00:00 +0100
categories: design
---

Every language has a personality. Go is pragmatic. Rust is principled. C is honest. You feel it in the syntax, in the error messages, in what the language makes easy and what it makes hard.

Rask exists because I got frustrated. Rust has genuinely great ideas—ownership, traits, pattern matching, functional programming, zero-cost abstractions. But using it for everyday work feels like using a cannon to shoot a bird. Yes, it's compile-time safe. Yes, it's zero-cost. But at what cost for the programmer who just needs stuff to work? Half the time I'm satisfying the borrow checker instead of solving my actual problem.

And the alternative is... Go? C#? Languages with garbage collectors where you trade control for convenience? There's this gap between "fight the compiler for safety" and "give up and let the GC handle it." I wanted something in that gap.

[Hylo](https://www.hylo-lang.org/) is probably the closest to what I'm building—value semantics, no garbage collector, mutable value semantics instead of borrow checking. If you're interested in this design space, look at what they're doing. Where we differ is mostly in feel: Hylo comes from a more academic angle (it grew out of Val, a research project), while I'm trying to optimize for the programmer who just wants to ship things without thinking too hard about memory.

When I started Rask, I didn't start with features. Features are consequences. I started with a question: *what should a systems language feel like in 2026?* Get the values right first, the design follows.

I care about three things: **transparency** (can I see what my code costs?), **structural safety** (are bugs impossible, not just caught?), and **pragmatism** (does this actually help me ship?). These three pull in different directions, and most of the interesting design work is figuring out which one wins for each decision.

Let me show you what I mean.

## No garbage collection

This is the easy one—all three values agree. No GC means deterministic cleanup (safe), no hidden pauses (transparent), and no GC tuning (pragmatic). When all three point the same way, the decision is obvious.

But most decisions aren't this clean.

## (Almost) No hidden costs

In C++, `auto result = greeting + " " + name` creates two temporary strings and two allocations. In Swift, passing a struct to a function silently copies it — could be 4 bytes, could be 4 kilobytes. These costs are real but invisible.

Rask doesn't do this. Large values move, not copy. If you want a copy, you write `.clone()`. Operators don't allocate behind your back. When something is expensive, you can see it in the code:

```rask
const name = user.name.clone()                  // explicit: this copies
process(own user)                               // explicit: ownership transferred
```

This is transparency winning over convenience. Some languages let you write `a + b` on strings and hide the allocation inside the operator. I'd rather make you call a function that says what it does.

## Implicit bounds checks

On the other hand, `results[i]` does a bounds check you can't see. That's pragmatism winning over transparency. I could require `results.checked_get(i)` everywhere, but writing checked access on every array index would be miserable for no real benefit—it's O(1), cheap, and if it panics you get a clear message.

This is where a strict "everything must be visible" rule would break down. Some costs just aren't worth the ceremony.

## Handle overhead

[I wrote about this in the first post](/2026/02/07/welcome-to-rask-blog/)—references can't be stored, so graph structures use handles into pools. Each handle access costs ~1-2ns for a generation check. That's real overhead.

```rask
func damage(h: Handle<Entity>) using Pool<Entity> {
    h.health -= 10                             // generation check here
    if h.health <= 0 {
        h.state = EntityState.Dead
    }
}
```

This is safety winning over performance. I could skip the check with raw pointers, but use-after-free is worse than 2ns. For the 90% of code that isn't a hot inner loop, I think that's the right call. For the rest, there's `unsafe`.

## Readable over writable

Code is read far more than it's written. Early languages optimized for fewer characters because of memory constrains and terminal size. We don't need that inheritance.

I try to keep things readable in plain English, without going full pseudo-code python. Common patterns deserve syntax sugar if it helps to keep mental tax down.

Compare Rust and Rask:

```rust
// Rust
fn save_user(db: &mut Database, name: &str) -> Result<UserId, Error> {
    let id = db.next_id()?;
    let user = User::new(id, name.to_string());
    db.insert(user)?;
    Ok(id)
}
```

```rask
// Rask
func save_user(mutate db: Database, name: string) -> UserId or Error {
    const id = try db.next_id()
    const user = User.new(id, name)
    try db.insert(user)
    return id
}
```

`mutate` tells you the function changes `db`. `try` reads as a word, not a symbol (`?` is reserved for optionals). `return id` just works — functions returning `T or E` wrap it as `Ok` implicitly. No `&mut`, no `&str` vs `String`. You read the signature and know what it does — what it borrows, what it mutates, what it takes ownership of.

Of course, we lose some coherence by treating Result, Error and Option different, compared to Rust where it is just "plain Rust" code. I think that they are so ubiquitous that they deserve special treatment, resulting in cleaner, less noisy code.

## Stealing good ideas

Swift's optional syntax is great—so Rask has `T?` with `??` fallback. Zig's comptime is powerful—so Rask has compile-time execution. Go's goroutines are ergonomic—so Rask has `spawn(|| {})` without async/await.

That's pragmatism. I'd rather take a proven solution than invent a worse one for the sake of originality. I compare against *whichever language is simplest for each task*—not just Rust or Go. If Python solves a CLI tool in 20 lines, that's the bar.

## Linear resources

Forget to close a file? Compile error. I/O handles must be consumed exactly once:

```rask
func process(path: string) -> Stats or Error {
    const file = try fs.open(path)
    ensure file.close()

    const data = try file.read_to_string()
    return parse_stats(data)
    // file.close() runs here, guaranteed
}
```

No special cleanup syntax, no `defer`, no destructors-that-might-not-run. The compiler just refuses to compile if you forget. That's safety by structure—the bug isn't caught, it's impossible.

## So what's the soul?

There's no formula. Each decision is a judgment call, and I've probably gotten some of them wrong. But the pattern is: make the safe thing the default, make costs visible where it matters, and remove ceremony where it goes viral.

What I'm reaching for is a language where memory safety doesn't *feel like* memory safety. You write code thinking about your problem, and the safety falls out from the structure. I'm still early enough that it could all fall apart once real programs hit the design.
