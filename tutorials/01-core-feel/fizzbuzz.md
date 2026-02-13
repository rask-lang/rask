# Challenge 1.1: FizzBuzz

Write `fizzbuzz.rk`. Print 1–100, but:
- multiples of 3 → "Fizz"
- multiples of 5 → "Buzz"
- multiples of both → "FizzBuzz"

## Starting Point

```rask
func main() {
    for i in 1..101 {
        // your code here
    }
}
```

## Design Questions

- Did you reach for `%` (modulo)? Does it exist?
- How does `for i in 1..101` feel vs `for i in 1..=100`?
- Did you try implicit return in `func main()`?
- How did `if`/`else if`/`else` feel compared to other languages?

<details>
<summary>Hints</summary>

- `%` works as modulo: `i % 3 == 0`
- `&&` for logical AND
- `println(i)` can print integers directly

</details>
