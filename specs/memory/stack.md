<!-- id: mem.stack -->
<!-- status: proposed -->
<!-- summary: Every function carries a stack summary, checked like a signature; a root whose calls can need more stack than it has is a compile error, and recursion needs a proven or declared depth -->
<!-- depends: memory/value-semantics.md, types/error-types.md, concurrency/runtime.md -->

# The Stack

Natively, every value a function holds lives in its stack frame, and a stack is a fixed size, so a program can run out of it. C, C++, Rust and Zig find out when the program crashes; Go grows the stack until memory runs out. Rask answers the question at compile time. Each function carries a stack summary, the most stack a call to it can use, and every place a stack begins is checked against the summaries of what it calls.

The summary is checked the way a signature is. Checking a function reads its own body and its callees' summaries, never their bodies, so a function is still checked on its own.

## Rules

### Values

| Rule | Description |
|------|-------------|
| **ST1: A value is at most 64 KiB** | [mem.value/VS12](value-semantics.md), E0980. Checked on every value, at the concrete type each generic instance gives it |
| **ST2: Sizes are counted the same on every target** | With 8-byte pointers, as the copy rule counts them ([mem.value/VS7a](value-semantics.md)), so a program compiles on every target or none |

### Summaries

| Rule | Description |
|------|-------------|
| **ST3: A function's summary** | Its own frame, as native codegen lays it out (locals, temporaries, saved registers, the outgoing-argument area, anything inlined into it), plus the largest summary among the calls it makes. Codegen records the frame when it compiles the function; the summary ships with the compiled package beside the signatures |
| **ST4: Summaries are checked locally** | A function's summary is computed from its own body and its callees' summaries. Changing a function's frame rechecks its callers, as changing its return type would. A generic function has a summary per instance |
| **ST5: A closure parameter is part of the summary** | A function that calls a closure or function value it was given as a parameter has a summary in terms of that parameter: `each`'s is its frame plus whatever `f` needs. The caller, which knows what it passed, fills it in. `rows.each(\|r\| process(r))` and `for r in rows { process(r) }` get the same answer |
| **ST6: What has no summary** | A call through a function value read from a field or a collection, through `any Interface`, or into C that isn't Rask's own runtime has no summary. It counts as unbounded (ST10) |

### Roots

| Rule | Description |
|------|-------------|
| **ST7: Every stack has a size the runtime guarantees** | `main` 8 MiB, a `spawn { }` task 1 MiB, a `Thread.spawn { }` thread 8 MiB, a pool worker 8 MiB, a `test` or `benchmark` block 8 MiB. The runtime makes that true whatever `ulimit -s` or the OS default says. The space the runtime keeps for unwinding is on top of these numbers, not taken from them |
| **ST8: A root that can overflow is a compile error** | A root whose summary is over its stack is E0986, at the root, with the chain that needs the most and each frame's size |
| **ST9: A root can ask for more** | `spawn(stack: n) { }`, `Thread.spawn(stack: n) { }`, `@stack(n)` on `main`, a `test` or a `benchmark`, with `n` in bytes and known at compile time. A library that spawns its caller's function (`http.serve`) takes a `stack` option and passes it on |
| **ST10: Unbounded is checked at run time** | A root whose chain passes through a call with no summary (ST6) isn't an error. Every function still checks the stack on entry, and running out panics with a stack overflow naming the function. `rask check --stack` lists every root and what made it unbounded |

### Recursion

| Rule | Description |
|------|-------------|
| **ST11: A recursive cycle needs a depth** | A cycle in the call graph (a function calling itself, or functions calling each other) is a compile error (E0987) unless its depth is proven (ST12) or declared (ST13). A cycle can't span packages, since packages don't import each other in a cycle, so finding one needs only the package |
| **ST12: Proven depth** | The compiler proves a depth without help in two shapes: every recursive call passes a counter of an integer type strictly smaller, behind a guard that stops at zero (`f(n - 1)` with `n: u8` goes at most 255 deep); or every recursive call passes a length at most half of its own (`sort(v[..mid])` with `mid = v.len() / 2` goes at most 64 deep). The check reads the function's own body |
| **ST13: Declared depth** | `@recursion_depth(n)` on each function in the cycle. The function's return type must include `RecursionLimit` (`i64 or RecursionLimit`); a call that would go deeper than `n` returns `RecursionLimit` without entering. The caller handles it like any error: `try`, `catch`, `match`, or `!` to panic with its message |
| **ST14: A cycle's summary** | Its depth times the sum of its members' frames, plus the largest summary among the calls it makes out of the cycle |
| **ST15: Tail calls aren't recursion** | A call in tail position (`return f(x)`) reuses the frame, which codegen guarantees, and doesn't count as a cycle |
| **ST16: The depth is the same on both backends** | Depth is counted in calls, per task, so the interpreter returns `RecursionLimit` at exactly the call native does |

## Examples

Recursion on parsed input declares its depth and hands the limit to its caller:

```rask
@recursion_depth(10_000)
func eval(e: Expr) -> i64 or RecursionLimit {
    match e {
        Expr.Num(n) => return n,
        Expr.Add(a, b) => return try eval(a) + try eval(b),
    }
}

func main() {
    let e = parse(input())
    println(eval(e)!)                // or catch RecursionLimit and report "too deeply nested"
}
```

A sort proves its own depth and needs nothing:

```rask
func merge_sort(v: Vec<i64>) -> Vec<i64> {
    if v.len() <= 1 { return v }
    let mid = v.len() / 2
    return merge(merge_sort(v[..mid].to_vec()), merge_sort(v[mid..].to_vec()))
}
```

A task whose chain is too big for its stack:

```rask
func scratch() -> i64 {
    mut a1 = [0u8; 65536]
    // ... through a20
    return 0
}

func main() {
    using Multitasking {
        let h = spawn { return scratch() }    // E0986
        println(h.join()!)
    }
}
```

```
error[E0986]: this task can need more stack than it has
  --> main.rk:9:17
   |
 9 |         let h = spawn { return scratch() }
   |                 ^^^^^ a task has 1 MiB
   = chain: spawn body (48 B) → scratch (1,310,784 B) = 1,310,832 B
   = help: give it more with `spawn(stack: 2_097_152) { ... }`, or keep
           the arrays in a `Vec`
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| `[0i64; 8192]` | ST1 | OK: exactly 64 KiB |
| `struct Slots { lists: [Vec<i64>; 8193] }` | ST2 | E0980 on every target, wasm32 included |
| A big frame in a function only `main` calls | ST8 | Checked against `main`'s 8 MiB, not a task's 1 MiB |
| One function reached from `main` and from a `spawn` | ST8 | Checked against each root; the error names the one that fails |
| A dependency's patch release grows a private frame | ST4 | A caller's root can fail E0986 on update. The chain names the dependency's function |
| `vec.sort_by(cmp)` | ST5 | `sort_by`'s summary includes `cmp`'s, filled in at the call |
| `handlers[i](req)`, a function value from a `Vec` | ST6, ST10 | Unbounded; the run-time check covers it |
| A callback C calls on a thread C started | ST6 | No Rask root; the run-time check has no bound there and says so in `rask check --stack` |
| Recursion through a closure stored in a field | ST6, ST11 | Not a visible cycle; unbounded, checked at run time |
| `@recursion_depth` on a function not in a cycle | ST13 | Compile error: nothing to bound |
| A cycle where one member lacks `@recursion_depth` | ST13 | E0987 on that member |
| `return f(x - 1)` | ST15 | A loop, no depth needed |

## Error Messages

```
error[E0987]: `walk` calls itself with no bound on how deep it goes
  --> tree.rk:3:1
   |
 3 | func walk(n: Node) -> i64 {
   |      ^^^^ cycle: walk → walk
   = help: declare a depth with `@recursion_depth(n)` and return
           `i64 or RecursionLimit`, or make the call the last thing `walk`
           does
```

---

## Appendix (non-normative)

### Rationale

**Why native's frame and not one counted from the source?** A count from the source can only be a lower bound. Native also saves registers around calls, passes arguments on the stack, and merges inlined callees into the frame, and the register allocator decides those. A function passing forty `i64` arguments counted 72 bytes from the source and used 608. Only codegen knows the real number, so codegen records it. The cost is that the interpreter needs native's numbers for E0986, so every command runs codegen up to frame layout, without emitting code.

**Why summaries and not a whole-program pass?** I want each function checked on its own. A summary is to the stack what a signature is to types: the caller reads it, never the body. That also means a function's stack use is part of what it promises, so a dependency that grows a frame can break a caller. That's true of the stack in every language. Here it's a compile error with the chain in it rather than a crash.

**Why make recursion declare its depth?** Recursion over input (a tree parsed from a file, nested JSON) goes as deep as the input does, and no analysis can bound that. Rewriting it as a loop with a `Vec` moves the same memory to the heap and fails there instead. What's left is to make the bound visible and the failure an ordinary error. A JSON parser should refuse input nested past some depth anyway; `@recursion_depth` makes it say so, and `RecursionLimit` makes the caller decide what happens.

**Why an error and not a panic?** A panic is easy to forget about. An error in the return type has to be handled, and `!` is the one-character way to say it can't happen here.

**Why stop at function values in fields?** A function value stored in a struct or a `Vec` can be anything stored there from anywhere in the program. Bounding it would mean a whole-program pass or a stack bound in every function type. ST5 covers the common shape, a closure passed straight to the function that calls it, and the rest is left to the run-time check and listed by `rask check --stack`.

### See Also

- [mem.value](value-semantics.md): VS12, the value cap
- [type.errors](../types/error-types.md): `try`, `catch`, `!`
- [conc.runtime](../concurrency/runtime.md): stacks and the run-time check
