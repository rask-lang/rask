---
layout: post
title: "Welcome to the Rask Blog"
date: 2026-02-07 23:30:00 +0100
categories: announcement
---

Hi! I'm having great fun creating a new programming language! It is called Rask, and started out as a small experiment in language design, but now I feel it actually might bring something new!

## What is Rask?

What if I took the best parts of Rust, Zig and Go and created an in-between language?

Can we have memory safety without Rust's lifetime tracking? Don't get me wrong, Rust is a perfectly good language for what it's meant for. But sometimes fighting the borrow checker and waiting for compilation isn't what you want. Most programs don't need a mathematical proof to function.

On the other side you have Go—simple to learn, easy to get stuff done with. But it lacks the powerful features Rust users take for granted. Plus it has a garbage collector and hard-wired runtime threading, which you don't want for high-performance apps.

In 2026 we have multiple contenders for this gap. Off the top of my head: Hylo, Zig, V, and Vale, all awesome in their own right. But Hylo is too academic for my taste, and Vale's generational references for everything are too performance-taxing. V promises safe memory management without ceremony but has failed to live up to the claims. Zig is probably the closest to what I want, but it leaves safety as an opt-in rather than a default.

This brings us to Rask. The simple premise is this:
**What if references cannot be stored?**

Stored references cause use-after-free bugs, dangling pointers, and all the lifetime complexity Rust needs to track. If we can live without them, memory safety goes from complex lifetime tracking and global borrow checking to simple local analysis.

Here's the key insight: most code doesn't store references. You pass them to functions, use them for a bit, then they expire. That's expression-scoped borrowing—works great, no annotations needed. The problems come when references escape their scope or get stored in structs.

So I made a tradeoff. __References can't be stored__. If you need indirection for graphs or cycles, use handles—validated indices into a pool. Yes, that's an extra indirection. But the cost is visible and you only pay it where you actually need it. This isn't zero-cost abstraction. It's mathematically impossible to have zero-cost memory safety __and__ no lifetime tracking and global analysis. Rask's design compensates for this using smart compiler optimization and forcing more data-oriented design.

I've also made many improvements to Rust's __noisy__ grammar and restrictive, almost parental style of coding. This is fine for a hardcore system language with safety before anything else, but Rask takes a different approach: the common case should be the default, not the opposite. We sacrifice some of the safety the strictness of Rust gives, and get a more ergonomical developer experience. At least on paper, you will be the judge of that ;)

Here's a sneak peek at some core features:

- **Linear resources** - Files and sockets must be explicitly closed. The compiler checks this at compile time, so you can't leak handles.
- **No async/await** - I/O is just I/O. No more refactoring half your codebase because one function needs to wait for network.
- **Syntax sugar where it matters** - Option and Result show up in 90% of code, so I made them ergonomic with `?` sugar and `try` for propagation.

Here's what it looks like:

```rask
func process_config(path: string) -> Config or Error {
    const file = try fs.open(path)
    ensure file.close()

    const content = try file.read_to_string()
    const lines = content.split('\n')

    let settings = Map.new()
    for line in lines {
        if line.starts_with('#'): continue

        const parts = line.split('=')
        if parts.len() != 2: return Error.InvalidFormat

        settings.insert(parts[0].trim(), parts[1].trim())
    }

    return Config { settings }
}
```

No lifetimes. No async. No borrow checker fights. Just clean, simple code that can't leak resources.

## What I'll write about

Design decisions. Why certain features work the way they do, what tradeoffs I'm making. Sometimes I'll be wrong—that's fine, I'll write about that too.

Implementation progress. Right now there's a working interpreter but no compiler yet. I'll document what I'm building and what's blocking me.

Real code. HTTP servers, text editors, game loops. If Rask makes these harder than Go, the design needs work.

## Where we are

Design phase. Most of the specs are written, the interpreter handles the core features. Threading works, linear resources work, the type checker catches most errors.

Next up: compiler. Probably LLVM backend, maybe cranelift. Haven't decided yet.

## Try it yourself

Want to see more? Try Rask in your browser with the [playground](../../playground/), read through the [language guide](../../../book/guide/), or dive into the [design specs](https://github.com/rask-lang/rask/tree/main/specs).

Have thoughts or questions? [Open an issue](https://github.com/rask-lang/rask/issues) or start a [discussion](https://github.com/rask-lang/rask/discussions) on GitHub.
