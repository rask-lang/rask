<!-- SPDX-License-Identifier: (MIT OR Apache-2.0) -->
# Rask Design Challenges

Hands-on exercises to stress-test the language design. Write everything
from scratch — no peeking at `examples/` until you're done.

**Setup:** The compiler binary is at `compiler/target/release/rask`.

```
rask run <file>       # execute a .rk program
rask check <file>     # type-check only
rask lint <file>      # style/idiom check
rask fmt <file>       # auto-format
```

Create your challenge files in a `challenges/` folder.

---

## Level 1 — Core Feel

Goal: Does basic Rask feel natural? Are the keywords intuitive?

### Challenge 1.1: FizzBuzz

Write `fizzbuzz.rk`. Print 1–100, but:
- multiples of 3 → "Fizz"
- multiples of 5 → "Buzz"
- multiples of both → "FizzBuzz"

**Design questions to notice:**
- Did you reach for `%` (modulo)? Does it exist or is it `rem`?
- How does `for i in 1..101` feel vs `for i in 1..=100`?
- Did you try implicit return in `func main()`?

### Challenge 1.2: Temperature Converter

Write `temps.rk`. Define:
```rask
enum Scale {
    Celsius(f64)
    Fahrenheit(f64)
    Kelvin(f64)
}
```

Write `func convert(from: Scale, to_scale: string) -> Scale` using
`match`. Convert between all three scales.

**Design questions:**
- How does `match` on nested enum data feel?
- Did you want to write `Scale.Celsius(temp)` or just `Celsius(temp)` in patterns?
- How natural is the `f64` arithmetic?

### Challenge 1.3: Word Counter

Write `wordcount.rk`. Given a hardcoded multi-line string:
1. Count total words
2. Count unique words (use a `Map`)
3. Print the top 5 most frequent words

**Design questions:**
- How does `string.split_whitespace()` → Vec feel?
- Is iterating a `Map` natural?
- Did you want `.entry()` API or is `map.get()` + `map.insert()` fine?

---

## Level 2 — Ownership & Errors

Goal: Do move semantics and error handling feel invisible or annoying?

### Challenge 2.1: Stack Machine

Write `stack.rk`. Implement a simple stack-based calculator:

```rask
struct Stack {
    items: Vec<f64>
}
```

Support: `push`, `pop`, `add` (pop two, push sum), `mul`, `print_top`.

Feed it a sequence: `push 3, push 4, add, push 10, mul, print_top` → `70`.

**Design questions:**
- Did you need `mutate self` on methods? How did that feel?
- What happens when you `pop` an empty stack? Did you use `Option` or `Result`?
- Did you ever fight the ownership system?

### Challenge 2.2: File Analyzer

Write `analyzer.rk`. Read a file path from `cli.args()`, then print:
- Line count
- Word count
- Character count
- Longest line (with line number)

```
rask run analyzer.rk some_file.txt
```

**Design questions:**
- How many `match` blocks did you write for error handling?
- Did `try` (error propagation) work where you wanted it?
- Compare your code length to how you'd write this in Go. Shorter? Longer?

### Challenge 2.3: Linked Data

Write `contacts.rk`. Build a contact book:

```rask
struct Contact {
    name: string
    email: string
    tags: Vec<string>
}
```

Store contacts in a `Vec<Contact>`. Write functions to:
1. Add a contact
2. Search by name (partial match)
3. Find all contacts with a given tag
4. Remove a contact by name

**Design questions:**
- When you pass a `Vec<Contact>` to a function, did you get move errors?
- How did you handle "not found" — `Option`, `Result`, or bool?
- Did you want references/borrows? Did you need `clone()`?

---

## Level 3 — Concurrency

Goal: Are threads and channels simple enough for the common case?

### Challenge 3.1: Parallel Sum

Write `parallel_sum.rk`. Split a list of 1000 numbers across 4 threads,
sum each chunk, then combine results.

```rask
import std

func main() {
    const numbers = Vec.new()
    for i in 1..1001 {
        numbers.push(i)
    }
    // Split into 4 chunks, spawn 4 threads, collect results
}
```

**Design questions:**
- How did you split the Vec? Slice syntax? Manual indexing?
- Did `Channel.buffered()` feel right for collecting results?
- Compare this to Go's goroutines + channels. More or less ceremony?

### Challenge 3.2: Producer-Consumer Pipeline

Write `pipeline.rk`. Three-stage pipeline:
1. **Producer**: generates numbers 1–50
2. **Filter**: keeps only even numbers
3. **Printer**: prints results

Each stage runs in its own thread, connected by channels.

**Design questions:**
- How do you signal "done" through channels? Sentinel value? Close?
- Did you need `Shared` for anything or were channels enough?
- How much boilerplate compared to Go's `go func() { ... }`?

---

## Level 4 — Data Modeling

Goal: Do structs + enums + match cover real domain modeling?

### Challenge 4.1: JSON-lite Parser

Write `json_lite.rk`. Parse a tiny subset of JSON:

```rask
enum JsonValue {
    Null
    Bool(bool)
    Number(f64)
    Str(string)
    Array(Vec<JsonValue>)
}
```

Write a parser that handles: `null`, `true`, `false`, numbers, quoted
strings, and arrays. Skip objects for now.

Parse: `[1, "hello", true, [2, 3], null]`

**Design questions:**
- Did recursive enums (`Array(Vec<JsonValue>)`) work?
- How was pattern matching on nested JSON?
- Did you want helper methods on the enum? How did `extend JsonValue` feel?

### Challenge 4.2: Task Scheduler

Write `scheduler.rk`:

```rask
enum Priority { Low, Medium, High, Critical }

struct Task {
    id: i32
    name: string
    priority: Priority
    completed: bool
}
```

Build a scheduler that:
1. Adds tasks with priorities
2. Gets next task (highest priority first)
3. Marks tasks complete
4. Lists pending tasks

**Design questions:**
- Did you want `Ord` / `PartialOrd` on `Priority`? How did you compare?
- Was `Vec<Task>` enough or did you want a priority queue?
- How much code for the sorting/priority logic?

---

## Level 5 — Integration

Goal: Full program. Does everything hold together?

### Challenge 5.1: Log Analyzer

Write `loganalyzer.rk`. Read a log file where each line is:
```
[LEVEL] timestamp message
```

Report:
- Count per level (INFO, WARN, ERROR)
- All ERROR lines with line numbers
- The busiest 1-minute window (if timestamps are parseable)

Use `cli.args()` for the file path. Create a sample log file to test with.

**Design questions:**
- How many structs/enums did you define?
- Did the string parsing feel adequate or did you want regex?
- Total line count — is it competitive with Python? With Go?

### Challenge 5.2: Chat Room (Concurrency + Data)

Write `chatroom.rk`. Simulate a multi-user chat:

- Multiple "user" threads send messages to a central channel
- A "server" thread receives messages and broadcasts to all
- Use `Shared` for the message history

```rask
struct Message {
    user: string
    text: string
    timestamp: i32
}
```

Run for a fixed number of messages, then print the full history.

**Design questions:**
- Did you use channels, `Shared`, or both?
- Was there a moment you wanted `async`/`await`?
- How does the concurrency code read — clear or tangled?

---

## Scoring Yourself

After each challenge, rate 1–5:

| Criterion | Question |
|-----------|----------|
| **Fluency** | Did I write this without checking docs/examples? |
| **Brevity** | Is this shorter than the Go equivalent? |
| **Safety feel** | Did safety features help or annoy me? |
| **Error handling** | Was `try`/`match` on Results natural? |
| **Ownership** | Did I fight moves/borrows or barely notice them? |

### Red Flags to Watch For

- Needed `clone()` more than twice in one function → borrowing model may be too restrictive
- Wrote 3+ `match` blocks for error handling in a row → need better `try` ergonomics
- Wanted a feature that doesn't exist → write it down, it's a design signal
- Code is 2x longer than Go for the same thing → the design is failing its own litmus test
- Reached for `unsafe` → the safe surface area has a gap

### Green Flags

- "I forgot this has ownership" → safety is invisible, that's the goal
- Code reads like pseudocode → ergonomic simplicity is working
- Error handling felt like normal control flow → `T or E` design is paying off
- Concurrency "just worked" → no function coloring is paying off

---

## After You're Done

Collect your friction notes and scores. The patterns will tell you:
- If the same friction appears in 3+ challenges → it's a language problem, not a you problem
- If you scored <3 on brevity consistently → revisit the Go litmus test
- If ownership was invisible → the core thesis is validated
