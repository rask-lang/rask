# Challenge 2.1: Stack Machine

Write `stack.rk`. Implement a simple stack-based calculator.

## Starting Point

```rask
struct Stack {
    items: Vec<f64>
}

// Implement: push, pop, add (pop two, push sum), mul, print_top

func main() {
    let stack = Stack { items: Vec.new() }
    // push 3, push 4, add, push 10, mul, print_top → 70
}
```

## Design Questions

- Did you need `mutate self` on methods? How did that feel?
- What happens when you `pop` an empty stack? Did you use `Option` or `Result`?
- Did you ever fight the ownership system?
- How did `let` (mutable) vs `const` (immutable) feel for the stack variable?

<details>
<summary>Hints</summary>

- Use `extend Stack { ... }` to add methods
- `mutate self` is needed for any method that modifies `self.items`
- `Vec.pop()` returns `Option<T>` — handle the `None` case
- `??` provides a default: `self.items.pop() ?? 0.0`
- The stack variable needs `let` because methods with `mutate self` modify it

</details>
