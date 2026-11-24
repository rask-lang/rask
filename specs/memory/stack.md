<!-- id: mem.stack -->
<!-- status: proposed -->
<!-- summary: Values are capped so a frame stays small; the runtime's C walks over values never recurse; every stack has a guaranteed size and every function checks it on entry, so overflow is a panic that names the recursion; a function can declare a depth, counted in calls, and get an error instead -->
<!-- depends: memory/value-semantics.md, memory/linear.md, types/error-types.md, control/panics.md, control/comptime.md, concurrency/runtime.md, concurrency/async.md -->

# The Stack

Natively, every value a function holds lives in its stack frame, and a stack is a fixed size, so a program can run out of it. Rask keeps two things off the stack by construction: a value bigger than 64 KiB is a compile error, and the runtime's own C code never recurses over a value. What's left is calls: a chain of them deeper than the stack. Every stack has a size the runtime picks and guarantees, and every function checks it on entry, so running out is a panic with a message, at the same call every run. A function that recurses over input can declare how deep it may go, and then the limit is a count of calls, the same on every target and both backends.

## Rules

### Values

| Rule | Description |
|------|-------------|
| **ST1: A value is at most 64 KiB** | [mem.value/VS12](value-semantics.md), E0980. Checked on every value, at the concrete type each generic instance gives it |
| **ST2: Sizes are counted the same on every target** | With 8-byte pointers, as the copy rule counts them ([mem.value/VS7a](value-semantics.md)), so a program compiles on every target or none |
| **ST3: The runtime's C never recurses over a value** | Clone, `==`, drop and hashing, which the runtime does in C, walk a value with a work list on the heap. A value nested a million deep costs heap there, not stack. Rask code that walks a value, generated or written (a derived `Encode`, a format library), recurses like any other Rask code and is caught by ST6 |

### Stacks

| Rule | Description |
|------|-------------|
| **ST4: Every stack has a size the runtime guarantees** | `main` 8 MiB, a `spawn { }` task 1 MiB, a `Thread.spawn { }` or `ThreadPool.spawn { }` thread 8 MiB, a `test` or `benchmark` block 8 MiB. The runtime maps each stack itself, `main`'s included, so `ulimit -s`, the environment and address randomisation don't move it. Below each is a 256 KiB reserve for C (ST8). A target without an OS takes the sizes from its build config |
| **ST5: A root can ask for more** | `spawn(stack: n) { }`, `Thread.spawn(stack: n) { }`, `using ThreadPool(stack: n) { }` for the pool's threads, `@stack(n)` on `main`, a `test` or a `benchmark`, with `n` in bytes and known at compile time |
| **ST6: Every function checks on entry** | It compares the stack pointer with the stack's limit and panics when there isn't room for its frame. The check is one compare and a branch that's never taken |
| **ST7: The overflow panic says what happened** | It names the function, the recursion it was in if it was in one (the functions repeating on the stack, and how many times), the stack's size, and how to give it more (ST5). Since the stack's size and the frames are fixed per binary, the same binary on the same input panics at the same call every run ([ctrl.panic/PD1](../control/panics.md)). A different target or backend can stop at a different depth, since frames differ in size |
| **ST8: C runs in the reserve** | Every call to C has the 256 KiB reserve below the Rask frame. Foreign C that runs through it hits the guard page and aborts with a message |

### Declared depth

| Rule | Description |
|------|-------------|
| **ST9: `@recursion_depth(n)`** | On a function, it limits how many calls to that function can be active at once in one task. The call that would make it `n + 1` doesn't run, and panics with the function's name and `n`. A generic function has one count, shared by all its instances |
| **ST10: The error instead of the panic** | A function with `@recursion_depth` whose return type includes `RecursionLimit` gets the error back from the call that would have gone too deep, and handles it like any other error. Such a function can't `take` a linear value (E0989): the refused call never runs, so nothing would consume it |
| **ST11: The count is the same everywhere** | It counts calls to the annotated function as written in the source, so native, the interpreter and comptime agree on which call is `n + 1`, whatever inlining or lowering does. Comptime's own depth limit ([ctrl.comptime/CT29](../control/comptime.md)) doesn't count these calls. A backend whose stack runs out first stops there with ST7's panic, and ST12 says where that can happen |

### Report

| Rule | Description |
|------|-------------|
| **ST12: `rask check --stack`** | For every root: the most stack its chains of direct calls can use, the recursion it can reach (with a declared depth or without), and its indirect calls. For each `@recursion_depth(n)` it says whether `n` frames fit the root's stack, natively and on the interpreter. Nothing it prints stops a build: a frame's size depends on the target |

## Examples

A parser on untrusted input declares its depth and turns the limit into its own error:

<!-- test: parse -->
```rask
@recursion_depth(512)
func parse_value(s: Scanner) -> Json or ParseError | RecursionLimit {
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

Input nested 600 deep makes 600 active calls to `parse_value`, so the 513th returns `RecursionLimit`. How many calls `parse_array` adds in between doesn't matter: only `parse_value` is counted. The stdlib's JSON decoder does the same with a depth of 128.

Recursion with no declaration runs until the stack runs out:

<!-- test: parse -->
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
   = the main stack is 8 MiB; give it more with `@stack(n)` on `main`,
     or bound it with `@recursion_depth(n)`
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| `[0i64; 8192]` | ST1 | OK: exactly 64 KiB |
| `struct Slots { lists: [Vec<i64>; 8193] }` | ST2 | E0980 on every target, wasm32 included |
| `a == b` on a tree a million deep | ST3 | Runs; the walk is a loop |
| A derived `==` called through `Vec<Tree>` from user code | ST3 | The same walk; no user frames involved |
| Derived `Encode` on a list a million deep | ST3, ST6 | Recurses in Rask; ST7's panic names `encode` |
| `ulimit -s unlimited`, then deep recursion in `main` | ST4 | Panics at the same call as with any other `ulimit`: `main`'s stack is the runtime's |
| A cycle of two functions, one annotated | ST9 | Only the annotated one is counted |
| `@recursion_depth` on a function no call can bring back to itself | ST9 | E0988: nothing to bound. A call through a closure, an `any` value or a generic parameter counts as one that might come back |
| `walk<T>` with `@recursion_depth(64)`, recursing through `walk<i64>` and `walk<string>` | ST9 | One count for both: 64 calls to `walk` in total |
| `@recursion_depth(100)` with `take f: File` and `RecursionLimit` in the return type | ST10 | E0989 |
| `@recursion_depth(1000)` called at comptime | ST11 | Runs to 1,000; CT29 doesn't count these calls. The branch quota (CT35) still bounds the total work |
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
   | ^^^^^^^^^^^^^^^^^^^^^ no call in `total` can lead back to `total`
```

```
error[E0989]: `walk_dir` can refuse a call, so it can't take `dir`
  --> fs.rk:2:14
   |
 1 | @recursion_depth(64)
 2 | func walk_dir(take dir: Dir) -> () or IoError | RecursionLimit {
   |               ^^^^^^^^^^^^ `Dir` is a resource; a call refused with
   |                            `RecursionLimit` never runs, so nothing closes it
   |
   = help: borrow it (`dir: Dir`) and close it in the caller
```

---

## Appendix (non-normative)

### Rationale

**Why not prove at compile time that the stack never overflows?** I tried, in four drafts, and each hit the same wall. Proving it needs every frame's real size, which only codegen knows, after inlining. So whether a program compiles would depend on the target and the inliner. It also needs every recursion bounded. Requiring an annotation lands on `factorial` in day-one code and can't reach the derived `==` on a recursive type, which has no source to annotate. Inferring a default depth instead puts a counter on every recursive call (about 2× on `fib` in a model), reserves 100,000 frames per task (300 MB for a JSON parser), can't be reserved on a microcontroller or in wasm, and counts frames that lowering and inlining decide, so the interpreter can't match it. There's no free option, so I took the cheap guarantees and made the rest visible. Each attempt and what broke it is in [analysis/stack-proofs.md](../analysis/stack-proofs.md).

**Why does the runtime map `main`'s stack?** The OS stack isn't a size anyone chose. Its limit comes from `ulimit -s`, the environment and the arguments sit at its top, and its start moves with address randomisation, so the overflow point moved between runs of one binary, and with `ulimit -s unlimited` there was no limit to check at all: the process grew until the kernel killed it. Mapping `main`'s stack like a task's gives it a size and a limit that are the binary's, which is what makes ST7's "same call every run" true.

**Why does the overflow panic differ between targets?** Because frames do. A 64-bit target's frames are bigger than a 32-bit one's, and native's are not the interpreter's. Where the depth has to be the same everywhere, `@recursion_depth` makes it so, and it costs a counter only in the functions that ask for it.

**Why count calls to one function, not frames of a cycle?** A cycle is something the compiler works out, and the reader can't see it: adding a call in one function changes which functions are in it. Calls to the annotated function are in the source, and the number means what it says.

**Why the runtime and not the user for deep values?** A tree built with a loop has no recursion in its source, so nothing tells the author that `==` on it recurses. When that recursion is Rask code, the entry check catches it with a clear panic. When it's the runtime's C, nothing does: it runs through the reserve and aborts. So the C walks are loops, and the Rask ones are ordinary recursion.

### What changes if this is accepted

- `conc.runtime`, "Stack limit": the panic message is ST7's; `main` runs on a mapped stack, and `main`'s and threads' sizes become ST4's.
- `conc.async`: `ThreadPool` config takes `stack: n`.
- `ctrl.comptime/CT27`, `CT29`: comptime honours `@recursion_depth` as ST11 does.
- `std.encoding`: the format-library pattern recurses in Rask, so a library reading untrusted input declares a depth; `std.json`'s decoder declares 128 and maps `RecursionLimit` to its parse error.
- The runtime's recursive walks over values become loops ([#2084](https://github.com/rask-lang/rask/issues/2084)).

### See Also

- [mem.value](value-semantics.md): VS12, the value cap
- [mem.linear](linear.md): consuming linear values
- [type.errors](../types/error-types.md): `catch` and `match` on results
- [ctrl.panic](../control/panics.md): S5, A1, PD1
- [conc.runtime](../concurrency/runtime.md): stacks and the limit check
