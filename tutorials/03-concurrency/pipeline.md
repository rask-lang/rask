# Challenge 3.2: Producer-Consumer Pipeline

Write `pipeline.rk`. Three-stage pipeline:
1. **Producer**: generates numbers 1–50
2. **Filter**: keeps only even numbers
3. **Printer**: prints results

Each stage runs in its own thread, connected by channels.

## Starting Point

```rask
import async.spawn

func main() {
    using Multitasking {
        // Create two channels: producer→filter, filter→printer
        // Spawn three tasks
        // Wait for completion
    }
}
```

## Design Questions

- How do you signal "done" through channels? Sentinel value? Closing?
- Did you need `Shared` for anything or were channels enough?
- How much boilerplate compared to Go's `go func() { ... }`?
- Was the ownership transfer through channels intuitive?

<details>
<summary>Hints</summary>

- Two channels: `Channel<i32>.buffered(10)` for each stage connection
- Producer sends 1–50, then closes its sender (`tx.close()`)
- Filter receives with `while rx.recv() is Ok(n)`, sends evens to next channel
- Printer receives and prints
- Close each sender when done to signal downstream that no more values are coming
- Use `.join()` or `.detach()` on task handles

</details>
