# Concurrency

> **Placeholder:** Brief overview. For detailed specifications, see the [concurrency](https://github.com/dritory/rask/blob/main/specs/concurrency/) specs.

## No Function Coloring

Functions are just functions - no `async`/`await` split:

```rask
func fetch_user(id: u64) -> User {
    const response = try http_get(url)  // Pauses task, not thread
    return parse_user(response)
}
```

## Spawning Tasks

```rask
func main() {
    with multitasking {
        const h = spawn { fetch_user(1) }
        const user = try h.join()
        println(user.name)
    }
}
```

Three spawn constructs:
- `spawn { }` - Green task (requires `with multitasking`)
- `spawn_thread { }` - Thread from pool (requires `with threading`)
- `spawn_raw { }` - Raw OS thread (no requirements)

## Channels

```rask
with multitasking {
    const chan = Channel.buffered(10)

    spawn {
        try chan.sender.send(42)
    }.detach()

    const val = try chan.receiver.recv()
    println("Received: {}", val)
}
```

Channels transfer ownership - no shared mutable state between tasks.

## Thread Pools

```rask
with threading(4) {
    const results = Vec.new()
    for i in 0..100 {
        const h = spawn_thread { compute(i) }
        try results.push(h)
    }

    for h in results {
        const val = try h.join()
        println(val)
    }
}
```

## Next Steps

- [Examples](../examples/README.md)
- [Formal async spec](https://github.com/dritory/rask/blob/main/specs/concurrency/async.md)
- [Formal sync spec](https://github.com/dritory/rask/blob/main/specs/concurrency/sync.md)
