<!-- id: mem.stack -->
<!-- status: proposed -->
<!-- summary: Values are capped so a frame stays small; every function checks the stack on entry and overflow is a panic that names the recursion; a function can declare a depth, counted in calls and the same everywhere, and get an error instead -->
<!-- depends: memory/value-semantics.md, types/error-types.md, control/panics.md, concurrency/runtime.md -->

# The Stack

Natively, every value a function holds lives in its stack frame, and a stack is a fixed size, so a program can run out of it. Rask keeps two things off the stack by construction: a value bigger than 64 KiB is a compile error, and the runtime never recurses over a value. What's left is calls: a chain of them deeper than the stack. Every function checks the stack on entry, so running out is a panic with a message, never a crash. A function that recurses over input can declare how deep it may go, and then the limit is a count of calls, the same on every target and both backends.

## Rules

### Values

| Rule | Description |
|------|-------------|
| **ST1: A value is at most 64 KiB** | [mem.value/VS12](value-semantics.md), E0980. Checked on every value, at the concrete type each generic instance gives it |
| **ST2: Sizes are counted the same on every target** | With 8-byte pointers, as the copy rule counts them ([mem.value/VS7a](value-semantics.md)), so a program compiles on every target or none |
| **ST3: The runtime never recurses over a value** | Clone, `==`, drop, hashing, encoding and decoding walk a value with a work list on the heap, so a value nested a million deep costs heap, not stack |

### Stacks

| Rule | Description |
|------|-------------|
| **ST4: Every stack has a size the runtime guarantees** | `main` 8 MiB, a `spawn { }` task 1 MiB, a `Thread.spawn { }` thread 8 MiB, a `test` or `benchmark` block 8 MiB, whatever `ulimit -s` or the OS default says. Below each is a 256 KiB reserve for C (ST8) |
| **ST5: A root can ask for more** | `spawn(stack: n) { }`, `Thread.spawn(stack: n) { }`, `@stack(n)` on `main`, a `test` or a `benchmark`, with `n` in bytes and known at compile time |
| **ST6: Every function checks on entry** | It compares the stack pointer with the stack's limit and panics when there isn't room for its frame. The check is one compare and a branch that's never taken |
| **ST7: The overflow panic says what happened** | It names the function, the recursion it was in if it was in one (the functions repeating on the stack, and how many times), the stack's size, and how to give it more (ST5). The same binary on the same input panics at the same call every run. A different target or backend can stop at a different depth, since frames differ in size |
| **ST8: C runs in the reserve** | Every call to C has the 256 KiB reserve below the Rask frame. Foreign C that runs through it hits the guard page and aborts with a message |

### Declared depth

| Rule | Description |
|------|-------------|
| **ST9: `@recursion_depth(n)`** | On a function, it limits how many calls to that function can be active at once in one task. The call that would make it `n + 1` doesn't run, and panics with the function's name and `n` |
| **ST10: The error instead of the panic** | A function with `@recursion_depth` whose return type includes `RecursionLimit` gets the error back from the call that would have gone too deep, and handles it like any other error |
| **ST11: The count is the same everywhere** | It counts calls to the annotated function as written in the source: native, the interpreter and comptime stop at the same call, whatever inlining or lowering does. Nothing else is counted, and a function without the annotation has no counter |
| **ST12: The stack must fit the depth** | Where the compiler can see the whole recursion (the annotated function and the direct calls it makes back to itself), `rask check --stack` says whether `n` frames fit the root's stack. It's information, not an error: a frame's size depends on the target |

### Report

| Rule | Description |
|------|-------------|
| **ST13: `rask check --stack`** | For every root: the most stack its chains of direct calls can use, the recursion it can reach (with a declared depth or without), and its indirect calls. Nothing it prints stops a build |

## Examples

A parser on untrusted input declares its depth and turns the limit into its own error:

```rask
@recursion_depth(512)
func parse_value(s: Scanner) -> Json or ParseError or RecursionLimit {
    // ... parse_array and parse_object call parse_value back
}

public func parse(text: string) -> Json or ParseError {
    match parse_value(Scanner.new(text)) {
        Json as v => return v,
        RecursionLimit => return ParseError.TooDeep,
        ParseError as e => return e,
    }
}
```

Input nested 600 deep makes 600 active calls to `parse_value`, so the 513th returns `RecursionLimit`. How many calls `parse_array` adds in between doesn't matter: only `parse_value` is counted.

Recursion with no declaration runs until the stack runs out:

```rask
func depth(t: Tree) -> i64 {
    mut best = 0
    for k in t.kids {
        best = max(best, depth(k))
    }
    return best + 1
}
```

```
panic at tree.rk:2: stack overflow in `depth`
   = recursion: depth → depth, 7,912 calls deep
   = the main stack is 8 MiB; give a task more with `spawn(stack: n)`,
     or bound it with `@recursion_depth(n)`
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| `[0i64; 8192]` | ST1 | OK: exactly 64 KiB |
| `struct Slots { lists: [Vec<i64>; 8193] }` | ST2 | E0980 on every target, wasm32 included |
| `a == b` on a tree a million deep | ST3 | Runs; the walk is a loop |
| A derived `==` called through `Vec<Tree>` from user code | ST3 | The same walk; no user frames involved |
| A cycle of two functions, one annotated | ST9 | Only the annotated one is counted |
| `@recursion_depth` on a function that never calls itself | ST9 | Compile error: nothing to bound |
| `@recursion_depth(1_000_000)` with a 200-byte frame in a 1 MiB task | ST7, ST12 | The stack runs out first; `rask check --stack` says so |
| Recursion through a closure stored in a field | ST6 | The entry check catches it; ST7 lists the repeating functions |
| An ensure overflowing while a task unwinds | ST6 | The limit drops by the reserve so ensures get room; a second overflow aborts (`ctrl.panic/A1`) |
| wasm32 | ST4 | The engine owns the call stack; its trap is turned into the ST7 panic where the engine allows it |

## Error Messages

```
error[E0988]: `@recursion_depth` on `total`, which never calls itself
  --> sum.rk:1:1
   |
 1 | @recursion_depth(100)
   | ^^^^^^^^^^^^^^^^^^^^^ `total` makes no call back to `total`
```

---

## Appendix (non-normative)

### Rationale

**Why not prove at compile time that the stack never overflows?** I tried, in four drafts, and each hit the same wall. Proving it needs every frame's real size, which only codegen knows, after inlining. So whether a program compiles would depend on the target and the inliner. It also needs every recursion bounded. Requiring an annotation lands on `factorial` in day-one code and can't reach the derived `==` on a recursive type, which has no source to annotate. Inferring a default depth instead puts a counter on every recursive call (about 2× on `fib` in a model), reserves 100,000 frames per task (300 MB for a JSON parser), can't be reserved on a microcontroller or in wasm, and counts frames that lowering and inlining decide, so the interpreter can't match it. There's no free option, so I took the cheap guarantees and made the rest visible.

**Why does the overflow panic differ between targets?** Because frames do. A 64-bit target's frames are bigger than a 32-bit one's, and native's are not the interpreter's. Where the depth has to be the same everywhere, `@recursion_depth` makes it so, and it costs a counter only in the functions that ask for it.

**Why count calls to one function, not frames of a cycle?** A cycle is something the compiler works out, and the reader can't see it: adding a call in one function changes which functions are in it. Calls to the annotated function are in the source, and the number means what it says.

**Why the runtime and not the user for deep values?** A tree built with a loop has no recursion in its source, so nothing tells the author that `==` on it recurses. That has to be the runtime's job.

### What changes if this is accepted

- `conc.runtime`, "Stack limit": the panic message is ST7's; `main`'s and threads' sizes become ST4's.
- `ctrl.comptime/CT27`, `CT29`: comptime honours `@recursion_depth` as ST11 does.
- The runtime's recursive walks over values become loops ([#2084](https://github.com/rask-lang/rask/issues/2084)).

### See Also

- [mem.value](value-semantics.md): VS12, the value cap
- [type.errors](../types/error-types.md): `catch` and `match` on results
- [ctrl.panic](../control/panics.md): S5, A1
- [conc.runtime](../concurrency/runtime.md): stacks and the limit check
