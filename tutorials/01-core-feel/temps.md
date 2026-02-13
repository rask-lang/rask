# Challenge 1.2: Temperature Converter

Write `temps.rk`. Define a `Scale` enum and convert between Celsius, Fahrenheit, and Kelvin.

## Starting Point

```rask
enum Scale {
    Celsius(f64)
    Fahrenheit(f64)
    Kelvin(f64)
}

func convert(from: Scale, to_scale: string) -> Scale {
    // your code here
}

func main() {
    const f = convert(Scale.Celsius(100.0), "F")
    println(f)    // Should be 212°F
}
```

## Design Questions

- How does `match` on nested enum data feel?
- Did you want to write `Celsius(temp)` or `Scale.Celsius(temp)` in patterns?
- How natural is the `f64` arithmetic?
- Did you use `match` as an expression (assigned to a variable) or as a statement?

<details>
<summary>Hints</summary>

- Convert to Kelvin first as an intermediate, then convert to the target scale
- `match from { Scale.Celsius(temp) => ... }` to destructure
- You can `return match to_scale { ... }` to use match as an expression in the return

**Formulas:**
- C → K: `temp + 273.15`
- F → K: `(temp + 459.67) / 1.8`
- K → C: `temp - 273.15`
- K → F: `(temp * 1.8) - 459.67`

</details>
