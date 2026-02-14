# Challenge 4.1: JSON-lite Parser

Write `json_lite.rk`. Parse a tiny subset of JSON: `null`, `true`, `false`, numbers,
quoted strings, and arrays. Skip objects for now.

## Starting Point

```rask
enum JsonValue {
    Null
    Bool(bool)
    Number(f64)
    Str(string)
    Array(Vec<JsonValue>)
}

// Write a parser that handles this input:
// [1, "hello", true, [2, 3], null]

func main() {
    const input = "[1, \"hello\", true, [2, 3], null]"
    const result = parse(input)
    println(result)
}
```

## Design Questions

- Did recursive enums (`Array(Vec<JsonValue>)`) work?
- How was pattern matching on nested JSON?
- Did you want helper methods on the enum? How did `extend JsonValue` feel?
- How did you handle parse errors — panic, Option, or Result?

<details>
<summary>Hints</summary>

- A parser struct can track position: `struct Parser { input: string, pos: i32 }`
- Use `extend Parser { ... }` for methods like `parse_value`, `parse_array`, `parse_string`
- `input[pos]` or iterate characters
- Skip whitespace between tokens
- For arrays: parse `[`, then loop parsing values separated by `,`, then `]`
- Recursive: `parse_value` can call `parse_array` which calls `parse_value`
- `extend JsonValue` can add a `to_string` method for printing

</details>
