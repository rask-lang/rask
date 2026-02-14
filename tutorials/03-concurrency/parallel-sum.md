# Challenge 3.1: Parallel Sum

Write `parallel_sum.rk`. Split a list of 1000 numbers across 4 threads,
sum each chunk, then combine results.

## Starting Point

```rask
import async.spawn

func main() {
    const numbers = Vec.new()
    for i in 1..1001 {
        numbers.push(i)
    }
    // Split into 4 chunks, spawn 4 threads, collect results
    // Print the total sum
}
```

## Design Questions

- How did you split the Vec? Slice syntax? Manual indexing?
- Did `Channel.buffered()` feel right for collecting results?
- Compare this to Go's goroutines + channels. More or less ceremony?
- Did ownership of the chunks feel natural?

<details>
<summary>Hints</summary>

- Calculate chunk size: `numbers.len() / 4`
- Clone each chunk before sending to a thread (or slice + clone)
- Use a channel to collect partial sums:
  ```rask
  let (tx, rx) = Channel<i64>.buffered(4)
  ```
- Each spawned task sums its chunk and sends the result
- Main thread receives 4 results and adds them up
- Expected total: 500500 (sum of 1 to 1000)

</details>
