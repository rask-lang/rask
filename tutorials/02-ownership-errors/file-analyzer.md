# Challenge 2.2: File Analyzer

Write `analyzer.rk`. Read a file path from `cli.args()`, then print:
- Line count
- Word count
- Character count
- Longest line (with line number)

```bash
rask run analyzer.rk some_file.txt
```

## Starting Point

```rask
import cli
import fs

func main() -> () or string {
    const args = cli.parse()
    const path = args.positional()[0]

    // Open file, read contents, analyze
}
```

## Design Questions

- How many `match` blocks did you write for error handling?
- Did `try` (error propagation) work where you wanted it?
- How does `ensure file.close()` feel vs Go's `defer`?
- Compare your code length to how you'd write this in Go. Shorter? Longer?

<details>
<summary>Hints</summary>

- `try fs.open(path)` — returns error if file doesn't exist
- `ensure file.close()` — put it right after opening
- `try file.read_text()` — read entire file as string
- `text.split("\n")` — split into lines
- `text.split_whitespace().len()` — total word count
- Track longest line with a variable and `for (i, line) in lines.enumerate()`
- The return type `() or string` means errors are returned as strings

</details>
