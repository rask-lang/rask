# Your First Program

Create a file called `hello.rk`:

```rask
func main() {
    println("Hello, Rask!")
}
```

Run it:

```bash
rask hello.rk
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

```rask
func main() {
    const name = "Rask"
    const year = 2025
    println(format("Hello from {} in {}!", name, year))
}
```

- `const` creates an immutable binding
- `let` creates a mutable binding (for values you'll reassign)
- Types are inferred, but you can write them explicitly: `const year: i64 = 2025`

## Functions

```rask
func greet(name: string) {
    println(format("Hello, {}!", name))
}

func main() {
    greet("World")
}
```

Functions that return values need explicit `return`:

```rask
func add(a: i32, b: i32) -> i32 {
    return a + b
}

func main() {
    const result = add(2, 3)
    println(format("2 + 3 = {}", result))
}
```

## Next: Explore the Guide

[Continue to Language Guide →](../guide/README.md)
