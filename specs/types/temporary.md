<!-- id: type.temporary -->
<!-- status: decided -->
<!-- summary: A struct declared @temporary makes values that live for the expression that makes them; nothing may keep one -->
<!-- depends: types/structs.md, types/generics.md -->
<!-- implemented-by: compiler/crates/rask-types/src/checker/temporary.rs -->

# Temporary Values

Some values are cheap because they point into something else, and only safe to
use while that something is still there: a piece of a string, a window onto a
buffer. `@temporary` on a struct says its values live for the expression that
makes them, and the compiler refuses every way of keeping one.

<!-- test: parse -->
```rask
@temporary(keep: [owned])
struct Window {
    rows: Vec<i64>
    from: usize
}

extend Window {
    func owned(self) -> Vec<i64> {
        return self.rows.clone()
    }
}
```

| Rule | Description |
|------|-------------|
| **TM1: What is temporary** | A struct declared `@temporary`, and a tuple, optional, result, array or union with one inside. A function may return one: the call is part of the expression that uses the value |
| **TM2: Nothing keeps one** | A `let`/`mut` binding, a field of a type that isn't itself temporary, and an element of a type that stores its arguments are each refused (E0303). A generic type stores an argument when the runtime owns storage for it (`Vec`, `Map`) or one of its fields mentions the parameter; `Sequence<T>` makes its items one call at a time and stores none, so `let words = line.split(" ")` is fine and `.to_vec()` on it is not |
| **TM3: The type says how to keep one** | `keep: [m, …]` names the methods that turn a value into something storable. The error lists each with the type it returns, read off the declaration, so a program's own temporary type gets the same advice the stdlib's does |

`StringSlice` is the stdlib's (`std.strings/S2`): `@temporary(keep: [view,
to_string])`.
