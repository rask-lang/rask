<!-- id: analysis.stack-proofs -->
<!-- status: proposed -->
<!-- summary: Six attempts to rule out stack overflow at compile time or by growing stacks, and what broke each. Read before reopening mem.stack -->
<!-- depends: memory/stack.md -->

# Stack Overflow: What Didn't Work

**Result: there's no free option.** I tried to make stack overflow a compile error, then to make it impossible by growing stacks, then to make every overflow stop at the same call everywhere. Each draft went through an adversarial review that built the failing programs and C models. [mem.stack](../memory/stack.md) is what survived: the value cap, runtime walks that never recurse, the entry check with a clear panic, and an opt-in `@recursion_depth`. This page is the record of the rest, so nobody walks the same road again without new facts.

The same few walls came up every time, so they're listed first.

## The walls

**Only codegen knows a frame's size.** A count from the source can't bound native code. Forty `i64` arguments counted 72 bytes from the source and used 608. The MIR inliner merges sibling callees into one frame where calls would take the max, so inlining can make a frame *bigger* than the call chain it replaced (a 24 KB chain became a 48 KB `main`). Codegen also invents slots the source doesn't have: copies of aggregate arguments, C structs passed by address, closure environments, padding for over-aligned slots. Any rule that turns bytes into a compile error makes compiling depend on the target, the inliner and the compiler version.

**Recursion is everywhere, including where nobody wrote it.** `a == b` on `struct Tree { kids: Vec<Tree> }` recurses through the derived `Tree.eq` and the stdlib's `Vec.eq<Tree>`. Both are direct calls, neither has source the user can annotate, and the cycle exists only after monomorphisation. Hashing, cloning and the stdlib's own `JsonValue` do the same.

**Day-one code recurses.** `factorial(n: i64)` and `fib(n: i32)` are in the first tier of tests. Any rule that makes recursion carry a mark lands there.

**Which frames exist is the compiler's choice.** `t.kids.fold(1, |acc, k| acc + count(k))` lowers to a loop with one frame per level. The same walk through a helper that takes a closure is three frames per level, or an indirect call when closure-target analysis can't resolve it. `sort_by` runs its comparator under C. The interpreter runs on the AST and implements `fold` and `==` in Rust. Comptime runs before monomorphisation, so the instance graph doesn't exist yet. Counting "frames of a cycle" therefore means something different on each backend.

**Indirect calls can't be bounded locally.** `any I`, a function value read from a field, a closure passed through a parameter from a field, a callback from C. Seeing a cycle through them needs every implementation in the program, which CORE_DESIGN principle 5 rules out.

## The attempts

### 1. A stack summary checked like a signature (v1–v3)

Every function carries the most stack a call to it can use, its frame plus the largest callee summary. A root over its stack is a compile error. Recursion must be proven bounded (a shrinking counter, a halving length) or declare `@recursion_depth(n)` with `RecursionLimit` in its return type.

What broke:

- The frame wall. Counting from the source undercounts; using codegen's frame ties compiling to the target and the inliner (the IN3 "called once" rule is package-wide, so a second call anywhere changes `main`'s frame).
- The holes are where stacks really overflow. The HTTP server is unbounded three ways (a cloned handler closure, `any Middleware`, sequence adapters returning closures). `Vec<any Node>` recursion isn't a visible cycle. The runtime's recursive clone aborted in C at 20,000 deep (#2084).
- `RecursionLimit` can't cross an interface method whose signature is fixed (`to_string` on a recursive JSON value), generic or higher-order cycles. Making a private helper recursive changes every caller's signature.
- The proof shapes are weak. An `i64` counter proves depth 2^63. `merge_sort`'s rounded-up half fails "at most half".
- Tail calls, which the draft exempted, don't exist, and can't when a drop or unwind record follows the call.

### 2. Growable stacks, segmented (v4 sketch)

The entry check allocates a new segment instead of panicking.

What broke: the hot split. A loop calling a function right at a segment boundary allocates and frees on every iteration. Rust dropped segmented stacks for this in 2013 and Go in 2014. Keeping a spare segment turns the allocation into a stack switch, still a hidden cost that moves with frame layout. Copying stacks, Go's fix, need every pointer into the stack, which Rust and Rask can't enumerate cheaply.

### 3. Big lazily-committed stacks (64 MiB per task)

Reserve address space, commit on touch.

What broke: it moves the limit rather than removing it. A runaway recursion fills 64 MiB of RAM before it dies instead of 1 MiB. It caps the number of tasks by address space, doesn't work without an MMU, and nothing about it is structural.

### 4. Heap chunks for recursion and indirect calls, everything else proven (v4)

Direct acyclic calls are proven; recursive and indirect calls move to a heap chunk when short.

What broke:

- Unbounded growth turns a runaway recursion from a 7 ms task panic into an OOM kill of the whole process: about 1.5 GB/s in a C model. A doubling `Vec` fails cleanly because a 16 GiB `malloc` returns NULL; a stream of 64 KiB chunk requests never fails, so the kernel kills the process instead. The interpreter already caps its own growable stack at 1 GiB for this reason.
- Hidden allocation inside `@no_alloc` code and interrupt handlers.
- A plain heap chunk has no guard page, so a C overrun silently corrupts the heap.

### 5. Exact stacks sized from summaries, no entry check, declared depth (v5)

Since recursion declares a depth, every root's stack is finite and known: reserve exactly that and drop the check on direct calls.

What broke:

- Derived `==` recursing through `Vec.eq<Tree>` is neither a visible cycle nor an indirect call, so with the check gone it overflows unchecked: a guard-page abort, or silent corruption on wasm and chips without an MMU. **Never remove the entry check: it's the only thing that catches what an analysis misses.**
- One chunk per nested indirect call means two kernel mappings per level. Linux's default `vm.max_map_count` runs out about 32,700 deep, shared with every other task's stack, so the failure depends on load.
- E0987 on day-one `factorial` and `fib`. The same body with `n: u16` compiles. Adding a call in `parse_primary` makes two other functions fail.
- `@recursion_depth(1_000_000_000)` compiles and fails at the 703rd spawn.
- "Depth" in a cycle of several functions was undefined: one counter for the cycle and one per function stop at different nesting.

### 6. Automatic default depth on every recursion (v6)

The compiler finds cycles on the instance graph, gives each a counter and a default of 100,000 frames; no annotation needed.

What broke:

- About 2× on call-bound recursion: `fib(35)` went from about 55 ms to 105 ms in a C model of the counter plus the existing check.
- 100,000 frames reserved per task that can reach a cycle: about 300 MB for the stdlib JSON parser, so every HTTP handler that parses JSON. Fine on 64-bit Linux; about 1,800 tasks on a 39-bit address space, 14 on 32-bit, impossible on a microcontroller.
- On wasm the engine owns the call stack and traps far earlier, at a depth that depends on the engine.
- The count depends on lowering and inlining (the "which frames exist" wall), so the interpreter would have to copy native's decisions, and comptime can't see the graph at all.
- Cycles merge when one function gains a call, so a body edit changes another function's limit.

## What survived, and why it's cheap

| Kept | Cost | Why it holds |
|------|------|--------------|
| 64 KiB value cap (VS12) | A compile error on a rare value | Bounds a frame's width, counted the same on every target |
| Runtime walks with a heap work list (#2084) | Runtime work, once | Removes the one overflow that can't be caught, and the one no annotation can reach |
| Entry check on every function | One compare per call | Catches everything, including what no analysis sees |
| Opt-in `@recursion_depth(n)` per function | A counter only where written | Counts calls to one named function as written, so it's the same everywhere |
| `rask check --stack` | Nothing in a build | Information without enforcement |

## Before reopening this

A new attempt has to answer the walls above, not just the attempt it replaces. In particular: any rule that makes a byte count decide whether a program compiles fails on targets and inlining. Any rule that marks recursion lands on day-one code and on derived code. Any default counter is paid on every recursive call. Any growth without a cap turns a task panic into a process kill.
