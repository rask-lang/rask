# Challenge 5.2: Chat Room

Write `chatroom.rk`. Simulate a multi-user chat:
- Multiple "user" threads send messages to a central channel
- A "server" thread receives messages and stores history
- Use `Shared` for the message history

Run for a fixed number of messages, then print the full history.

## Starting Point

```rask
import async.spawn

struct Message {
    user: string
    text: string
    timestamp: i32
}

func main() {
    using Multitasking {
        // Create a channel for incoming messages
        // Create shared history
        // Spawn user threads that send messages
        // Spawn server thread that receives and stores
        // Wait, then print history
    }
}
```

## Design Questions

- Did you use channels, `Shared`, or both?
- Was there a moment you wanted `async`/`await`?
- How does the concurrency code read — clear or tangled?
- Did ownership of Message through channels feel natural?

<details>
<summary>Hints</summary>

- Channel for users → server: `Channel<Message>.buffered(50)`
- `Shared<Vec<Message>>` for history that the server writes and main reads later
- Each user thread: loop a few times, send a Message, sleep briefly
- Server thread: `while rx.recv() is Ok(msg) { history.write(|h| h.push(msg)) }`
- Users clone `tx` so each thread has its own sender
- Close all senders when users are done — server's recv loop will exit
- Print history at the end by reading from Shared

</details>
