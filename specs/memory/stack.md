<!-- id: mem.stack -->
<!-- status: proposed -->
<!-- summary: A task's stack is sized from what it calls, so direct calls can't overflow; visible recursion declares or proves its depth and panics past it at the same call everywhere; only indirect calls are checked at run time -->
<!-- depends: memory/value-semantics.md, types/error-types.md, control/panics.md, concurrency/runtime.md -->

# The Stack

Natively, every value a function holds lives in its stack frame, and a stack is a fixed size, so a program can run out of it. C, C++, Rust and Zig find out when the program crashes. Go grows the stack until it hits a 1 GB cap. Rask moves the question to the compiler. A stack only holds what the compiler can bound, the same rule the value cap applies to a single value: width is capped at 64 KiB, and depth comes from the calls, which the compiler can count. What it can't count, recursion over data, has to say how deep it may go.

## Rules

### Values

| Rule | Description |
|------|-------------|
| **ST1: A value is at most 64 KiB** | [mem.value/VS12](value-semantics.md), E0980. Checked on every value, at the concrete type each generic instance gives it |
| **ST2: Sizes are counted the same on every target** | With 8-byte pointers, as the copy rule counts them ([mem.value/VS7a](value-semantics.md)), so a program compiles on every target or none |

### Summaries

| Rule | Description |
|------|-------------|
| **ST3: A function's summary** | The most stack a call to it can use: its frame as codegen lays it out (after inlining), plus the largest summary among its direct calls, plus what its recursive cycles need (ST13). Codegen records it beside the function's signature in the compiled package |
| **ST4: Summaries are computed locally** | From the function's own frame and its callees' summaries, never their bodies, the way an inferred signature is. A generic function has one per instance. A summary is in bytes and differs between targets; it sizes stacks and never decides whether a program compiles |
| **ST5: A closure parameter is part of the summary** | A function that calls a closure it was given as a parameter has a summary in terms of that parameter, filled in by the caller that passes it. `rows.each(\|r\| process(r))` and `for r in rows { process(r) }` get the same summary |
| **ST6: An indirect call has no summary at compile time** | A call through a function value read from a field or a collection, through `any Interface`, or from C into Rask. The callee's summary travels with it at run time: in the function value and in the vtable entry |

### Stacks

| Rule | Description |
|------|-------------|
| **ST7: A stack is as big as its root needs** | `main`, a `spawn { }` task, a `Thread.spawn { }` thread and a `test` or `benchmark` block each get a stack reserved at their root's summary. Pages are committed when touched, so a reservation costs address space until it's used. A root whose summary is unbounded (ST6) gets a reservation of its bounded part plus 1 MiB |
| **ST8: Direct calls are never checked** | A function whose callers reserved its summary can't run out, so it has no stack check on entry. The only checks are at indirect calls (ST9) and at entry from C (ST15) |
| **ST9: An indirect call makes room** | It compares the room left with the callee's summary. If it's short, the callee runs on a chunk: a block mapped for it with a guard page and the C reserve (ST15) below it. A task keeps one spare chunk when the call returns and frees the rest |
| **ST10: Indirect calls nest at most 100,000 deep** | Per task. The call past that panics with "indirect calls nested 100,000 deep", naming the callee. Only recursion through function values or interfaces gets near it, and it's counted the same on every backend and target |
| **ST11: `@no_alloc` code can't take a chunk** | An indirect call in it that finds too little room panics instead (ST10's message, with the room missing). `rask check --stack` lists every such call |

### Recursion

| Rule | Description |
|------|-------------|
| **ST12: A visible cycle needs a depth** | A cycle in the call graph is a compile error (E0987) unless its depth is proven (ST13) or declared (ST14). The graph has the direct calls, the closures passed as parameters (ST5), and calls through `any I` to every implementation of that method in the same package. Packages don't import each other in a cycle, so the package is all the check reads |
| **ST13: Proven depth** | The compiler proves a depth in two shapes, read from the function's own body: every recursive call passes a counter of an integer type strictly smaller than the one it got, behind a guard that stops at zero; or every recursive call passes a length at most `(len + 1) / 2` of its own behind a guard that `len >= 2`. A proof counts only if it bounds the depth at 1,048,576 calls or fewer: a `u16` counter qualifies, an `i64` counter doesn't, halving gives 64 |
| **ST14: Declared depth** | `@recursion_depth(n)` on each function in the cycle, or on an interface method, which counts every call through it. The call that would go deeper than `n` panics with the function's name and `n`. A function whose return type includes `RecursionLimit` gets the error back instead, from the call that would have gone too deep, and handles it like any other |
| **ST15: Depth is counted in calls** | Per task, on entry to the cycle, the same on both backends, every target and every optimisation level, and at comptime. Inlining never changes the count |
| **ST16: A cycle's summary** | Its depth times the sum of its members' frames, plus the largest summary among the calls it makes out of the cycle |

### C

| Rule | Description |
|------|-------------|
| **ST17: C runs in a fixed reserve** | Every call to C has 256 KiB below the Rask frame, counted in the caller's summary. The runtime's own C stays inside it: none of its walks over a value recurse. Foreign C that runs through the reserve hits the guard page and aborts with a message |
| **ST18: C calling Rask checks on entry** | The entry thunk compares the room left with the callee's summary and moves to a chunk when short (ST9). A callback on a thread C started runs on a stack the runtime maps for it, sized from its summary |

## Examples

A tree walk declares how deep it goes. Past that, it panics at exactly that call, on every backend:

```rask
@recursion_depth(10_000)
func sum(n: Node) -> i64 {
    mut total = n.value
    for c in n.children {
        total += sum(c)
    }
    return total
}
```

A parser on untrusted input wants an error instead, so it puts `RecursionLimit` in its return type and maps it into its own:

```rask
@recursion_depth(512)
func parse_value(s: Scanner) -> Json or ParseError or RecursionLimit {
    // ... parse_array and parse_object call parse_value, and carry the same annotation
}

public func parse(text: string) -> Json or ParseError {
    match parse_value(Scanner.new(text)) {
        Json as v => return v,
        RecursionLimit => return ParseError.TooDeep,
        ParseError as e => return e,
    }
}
```

A sort proves its depth and needs nothing:

```rask
func merge_sort(v: Vec<i64>) -> Vec<i64> {
    if v.len() < 2 { return v }
    let mid = v.len() / 2
    return merge(merge_sort(v[..mid].to_vec()), merge_sort(v[mid..].to_vec()))
}
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| `[0i64; 8192]` | ST1 | OK: exactly 64 KiB |
| `struct Slots { lists: [Vec<i64>; 8193] }` | ST2 | E0980 on every target, wasm32 included |
| A function with a 60 KiB frame called 20 deep | ST7 | The stack is reserved at 1.2 MiB; nothing is checked |
| A dependency's release grows a private frame | ST4 | Callers' stacks grow with it. Nothing fails to compile |
| `vec.sort_by(cmp)` | ST5 | `sort_by`'s summary includes `cmp`'s, filled in at the call |
| `handlers[i](req)` | ST6, ST9 | Compares room with the handler's summary; a chunk if short |
| `Vec<any Node>`, where `walk` calls `child.walk()` | ST12 | A cycle when the package implements `Node.walk`; needs a depth |
| Recursion through a closure stored in a field | ST10 | Not a visible cycle; panics past 100,000 nested indirect calls |
| `return f(x - 1)` | ST12 | Still recursion. A loop says the same thing without a depth |
| `@recursion_depth` on a function not in a cycle | ST14 | Compile error: nothing to bound |
| A cycle where one member lacks `@recursion_depth` | ST14 | E0987 on that member |
| An ensure running while a task unwinds | ST3 | Its calls are in its function's summary, so it has room |
| A comptime call 300 deep in a cycle declared 10,000 | ST15 | Runs; comptime counts the same as run time |

## Error Messages

```
error[E0987]: `walk` calls itself with no bound on how deep it goes
  --> tree.rk:3:1
   |
 3 | func walk(n: Node) -> i64 {
   |      ^^^^ cycle: walk → walk
   = help: declare a depth with `@recursion_depth(n)`, or write it as a
           loop over a `Vec` of pending nodes
```

```
panic at tree.rk:3: `walk` went deeper than its @recursion_depth(10000)
```

---

## Appendix (non-normative)

### Rationale

**Why size the stack instead of checking it?** Because the compiler knows. A chain of direct calls is finite, and its frames are known once codegen has laid them out, so a root's summary is an exact upper bound. Every function checking the stack on entry, which is what Rask does today, guards against something that can't happen. What's left after that is recursion and indirect calls, and the rules handle each where it is.

**Why bytes from codegen and not a count from the source?** A count from the source can't bound native: register saves, outgoing arguments and inlining all add to the frame, and the inliner can merge siblings into one frame bigger than any call chain would need. An earlier draft made a summary decide whether a program compiles, which tied compiling to the target and the inliner. Here bytes only size stacks, so they're free to come from codegen. What decides whether a program compiles is counted in calls, the same everywhere.

**Why make recursion declare its depth?** Recursion over input (a parsed tree, nested JSON) goes as deep as the input does. Without a bound, the choice is between a fixed stack that overflows at a depth nobody chose and a growing one that eats memory until the kernel kills the process. A declared depth is a bound you chose, and the stack is sized for it. Going past it is an ordinary panic at the same call every time.

**Why a panic and not an error?** Almost nobody would handle it. A tree walk or a sort past its depth is a bug, and an error would make every caller write `!`. The one caller that wants an error, a parser reading untrusted input, asks for it by putting `RecursionLimit` in its return type. It's the same split as heap exhaustion, which panics unless you use the `try_` variants.

**Why chunks for indirect calls, and why keep one?** The callee isn't known until the call, so its room can't be reserved ahead. A chunk never moves, so pointers into the stack stay valid. Segmented stacks were dropped by Rust and Go because a call sitting on a segment boundary allocated and freed on every iteration. Keeping one spare chunk makes that a stack-pointer switch, a few nanoseconds.

**What's still a run-time failure?** Recursion past its declared depth, an indirect-call nest past 100,000, foreign C running through its reserve, and the machine running out of memory to back the pages. The first is a bound you wrote; the second is recursion through a function value or an interface in another package, which seeing would take the whole program (CORE_DESIGN principle 5); the last two are outside what Rask can see.

### What changes if this is accepted

- `conc.runtime`, "Stack limit": the check on every function entry goes, replaced by ST8–ST9. The 1 MiB fiber reservation becomes ST7's.
- `ctrl.panic/S5`: stack overflow becomes "past a declared depth (ST14) or past ST10's nesting". The edge case of an ensure overflowing while it unwinds goes (ST3 covers it).
- `ctrl.comptime/CT27`, `CT29`: comptime counts recursion as ST15 does, with no limit of its own.
- `comp.codegen`, open issue 4: answered here.
- `conc.scheduler`: its copying stack is replaced by ST7 and ST9.
- The interpreter runs recursion on a stack that grows with it, since depth is now bounded by ST14 and ST10 rather than by its host stack.
- The runtime's recursive walks over values become loops ([#2084](https://github.com/rask-lang/rask/issues/2084)).

### See Also

- [mem.value](value-semantics.md): VS12, the value cap
- [type.errors](../types/error-types.md): `catch` and `!`
- [ctrl.panic](../control/panics.md): S5
- [conc.runtime](../concurrency/runtime.md): task stacks
