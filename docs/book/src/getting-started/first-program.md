# Your First Program

Create a file called `hello.rk`:

<!-- test: run | Hello, Rask! -->
```rask
func main() {
    println("Hello, Rask!")
}
```

Run it:

```bash
rask run hello.rk
```

Output:
```
Hello, Rask!
```

## What's Happening?

- `func main()` is the program entry point
- `println()` is a builtin for printing with newline

## Variables

Let's try variables:

<!-- test: run | Hello from Rask in 2027! -->
```rask
func main() {
    let name = "Rask"
    mut year = 2026
    year += 1
    println(format("Hello from {} in {}!", name, year))
}
```

- `let` binds a name once: no reassignment, and no mutating the value either (you can still
  *move* it — handing ownership away isn't mutation)
- `mut` is what you reach for when the value needs to change — reassignment or a mutating
  method like `v.push(x)`
- Types are inferred, but you can write them explicitly: `let year: i64 = 2026`

`const` exists too, but it's only for module-level constants — not for locals inside a function.

## Functions

<!-- test: run | Hello, World! -->
```rask
func greet(name: string) {
    println(format("Hello, {}!", name))
}

func main() {
    greet("World")
}
```

Functions that return values need explicit `return`:

<!-- test: run | 2 + 3 = 5 -->
```rask
func add(a: i32, b: i32) -> i32 {
    return a + b
}

func main() {
    let result = add(2, 3)
    println(format("2 + 3 = {}", result))
}
```

## Next: Explore the Guide

[Continue to Language Guide →](../guide/README.md)
