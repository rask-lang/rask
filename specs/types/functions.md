<!-- id: type.functions -->
<!-- status: decided -->
<!-- summary: Function types carry each parameter's mode; a call through a function value applies it like a direct call -->
<!-- depends: memory/parameters.md, memory/closures.md -->
<!-- implemented-by: compiler/crates/rask-parser/, compiler/crates/rask-types/, compiler/crates/rask-ownership/, compiler/crates/rask-mir/, compiler/crates/rask-interp/ -->

# Function Types

`func(A, B) -> R` is the type of a function value: a named function used as a value, a closure, a `Sequence<T>`'s underlying function. `|A, B| -> R` is the same type spelled closure-style.

| Rule | Description |
|------|-------------|
| **FT1: The mode is part of the type** | Each parameter of a function type is borrowed, `mutate` or `take`, written before its type: `func(take Vec<i64>) -> Vec<i64>`, `func(mutate Counter)`. Two function types are equal only when every parameter has the same type *and* the same mode. A named function's value has its declared modes. A call through a function value applies them exactly as a direct call does: a `take` argument moves and the caller's name is gone, a `mutate` argument needs a mutable place and the `mutate` marker (`mem.parameters/PM4`) |
| **FT2: Closures fill the slots they can declare** | A closure's parameters are borrowed or `mutate` (`mem.closures/CP1`, `CP2`), so a closure fills `func(T)` or `func(mutate T)`. It can't declare `take` (`CP4`), so only a named function fills a `func(take T)` slot. A `deleting` parameter is `mutate` in a function type |

<!-- test: compile-fail: typecheck -->
```rask
func stash(take p: Vec<i64>) -> Vec<i64> {
    return p
}

func main() {
    let f: func(Vec<i64>) -> Vec<i64> = stash    // error: `stash` takes its parameter
    return
}
```

<!-- test: run | 3 -->
```rask
func grow(mutate p: Vec<i64>) {
    p.push(9)
}

func main() {
    let g: func(mutate Vec<i64>) = grow
    mut xs: Vec<i64> = [1, 2]
    g(mutate xs)
    println("{xs.len()}")
    return
}
```

I had function types without modes for a long time, and it was a hole, not a simplification. `stash` above fit a plain `func(Vec<i64>) -> Vec<i64>`, so a call through the value didn't move its argument: the caller went on reading a vector the callee had consumed, and nothing freed it. A `mutate` function behind a `func(T)` wrote through a `let` binding. The direct call and the call through the value have to mean the same thing, and the only place that can say what they mean is the type.

The other way out was to forbid `take` functions as values, which matches what closures can do. I didn't take it: a callback that consumes what it's handed (a sink, a builder that keeps every item) is an ordinary thing to want, and it would have made every function with a `take` parameter second-class for a reason the reader can't see at the use site.

An exact match is the whole rule. A borrowing function in a `mutate` slot would be safe in principle, but a `mutate` argument travels as the caller's address and a borrowed one doesn't, so the two aren't one calling convention. Writing the mode is cheaper than an adapter nobody sees.
