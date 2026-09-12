# The Soul of Rask

*Written 2026-02-20.*

Every language has a personality. Go is pragmatic. Rust is principled. C is honest. You feel it in the syntax, in the error messages, in what the language makes easy and what it makes hard.

Rask exists because I got frustrated. Rust has genuinely great ideas—ownership, traits, pattern matching, functional programming, zero-cost abstractions. But using it for everyday work feels like using a cannon to shoot a bird. Yes, it's compile-time safe. Yes, it's zero-cost. But at what cost for the programmer who just needs stuff to work? Half the time I'm satisfying the borrow checker instead of solving my actual problem.

And the alternative is... Go? C#? Languages with garbage collectors where you trade control for convenience? There's this gap between "fight the compiler for safety" and "give up and let the GC handle it." I wanted something in that gap.

[Hylo](https://www.hylo-lang.org/) is probably the closest to what I'm building—value semantics, no garbage collector, mutable value semantics instead of borrow checking. If you're interested in this design space, look at what they're doing. Where we differ is mostly in feel: Hylo comes from a more academic angle (it grew out of Val, a research project), while I'm trying to optimize for the programmer who just wants to ship things without thinking too hard about memory.

When I started Rask, I didn't start with features. Features are consequences. I started with a question: *what should a systems language feel like in 2026?* Get the values right first, the design follows.

I care about three things: **transparency** (can I see what my code costs?), **structural safety** (are bugs impossible, not just caught?), and **pragmatism** (does this actually help me ship?). These three pull in different directions, and most of the interesting design work is figuring out which one wins for each decision.

Let me show you what I mean.

## No garbage collection

This is the easy one—all three values agree. No GC means deterministic cleanup (safe), no hidden pauses (transparent), and no GC tuning (pragmatic). When all three point the same way, the decision is obvious.

But most decisions aren't this clean.

## (Almost) No hidden costs

In C++, `auto result = greeting + " " + name` creates two temporary strings and two allocations. In Swift, passing a struct to a function silently copies it — could be 4 bytes, could be 4 kilobytes. These costs are real but invisible.

Rask doesn't do this. Large values move, not copy. If you want a copy, you write `.clone()`. Operators don't allocate behind your back. When something is expensive, you can see it in the code:

<!-- test: compile -->
```rask
struct Inventory {
    items: Vec<string>
}

struct Account {
    name: string
    inventory: Inventory
}

func process(take account: Account) {
    println(account.name)
}

func main() {
    let account = Account { name: "ada", inventory: Inventory { items: Vec.new() } }
    let items = account.inventory.clone()   // explicit: this copies
    println("{items.items.len()}")
    process(own account)                    // explicit: ownership transferred
}
```

Strings are the deliberate exception — `string` is immutable, refcounted, and Copy (16 bytes). It copies like an integer, no `.clone()` needed. I think that's fine because immutability eliminates aliased mutation risk, and the compiler elides most refcount operations anyway.

This is transparency winning over convenience. Some languages let you write `a + b` on strings and hide the allocation inside the operator. I'd rather make you call a function that says what it does.

## Implicit bounds checks

On the other hand, `results[i]` does a bounds check you can't see. That's pragmatism winning over transparency. I could require `results.checked_get(i)` everywhere, but writing checked access on every array index would be miserable for no real benefit—it's O(1), cheap, and if it panics you get a clear message.

This is where a strict "everything must be visible" rule would break down. Some costs just aren't worth the ceremony.

## Handle overhead

[I wrote about this in the previous note](why-a-new-language.md)—references can't be stored, so graph structures use handles into pools. Each handle access costs ~1-2ns for a generation check. That's real overhead.

*(Since writing this, handles-into-pools was replaced by `Rack<T>` + `Link<T>`.
Deleting a node nulls every edge pointing at it before the delete returns, so a
dangling link never exists and following a live one needs no check at all — it's
a pointer hop. The 1–2ns this section is arguing about is gone, and so is the
argument. The example below is the current spelling; the original took a
`Handle<Entity>` and a `using Pool<Entity>` clause.)*

<!-- test: compile -->
```rask
import memory.Rack
import memory.Link

enum EntityState {
    Alive
    Dead
}

struct Entity {
    health: i32
    state: EntityState
}

func damage(mutate e: Link<Entity>) {
    e.health -= 10
    if e.health <= 0 {
        e.state = EntityState.Dead
    }
}
```

What I wrote at the time: this is safety winning over performance — I could skip
the check with raw pointers, but use-after-free is worse than 2ns. That was the
right instinct and the wrong dilemma. The better move was to make the invalid
state impossible rather than pay to test for it, which is what the rack does.

## Readable over writable

Code is read far more than it's written. Early languages optimized for fewer characters because of memory constrains and terminal size. We don't need that inheritance.

I try to keep things readable in plain English, without going full pseudo-code python. Common patterns deserve syntax sugar if it helps to keep mental tax down.

Compare Rust and Rask:

```rust
// Rust
fn save_user(db: &mut Database, name: &str) -> Result<UserId, Error> {
    let id = db.next_id()?;
    let user = User::new(id, name.to_string());
    db.insert(user)?;
    Ok(id)
}
```

<!-- test: compile -->
```rask
// Rask
struct UserId {
    value: i64
}

struct Account {
    id: UserId
    name: string
}

struct Database {
    next: i64
}

enum DbError {
    Full
}

extend Account {
    func new(id: UserId, name: string) -> Account {
        return Account { id: id, name: name }
    }
}

extend Database {
    func next_id(mutate self) -> UserId or DbError {
        self.next += 1
        return UserId { value: self.next }
    }

    func insert(mutate self, account: Account) -> void or DbError {
        return
    }
}

func save_account(mutate db: Database, name: string) -> UserId or DbError {
    let id = try db.next_id()
    let account = Account.new(id, name)
    try db.insert(account)
    return id
}
```

`mutate` tells you the function changes `db`. `try` reads as a word, not a symbol (`?` is reserved for optionals). `return id` just works — functions returning `T or E` wrap it as `Ok` implicitly. No `&mut`, no `&str` vs `String`. You read the signature and know what it does — what it borrows, what it mutates, what it takes ownership of.

Of course, we lose some coherence by treating Result, Error and Option different, compared to Rust where it is just "plain Rust" code. I think that they are so ubiquitous that they deserve special treatment, resulting in cleaner, less noisy code.

## Stealing good ideas

Swift's optional syntax is great—so Rask has `T?` with `??` fallback. Zig's comptime is powerful—so Rask has compile-time execution. Go's goroutines are ergonomic—so Rask has `spawn(|| {})` without async/await.

That's pragmatism. I'd rather take a proven solution than invent a worse one for the sake of originality. I compare against *whichever language is simplest for each task*—not just Rust or Go. If Python solves a CLI tool in 20 lines, that's the bar.

## Linear resources

Forget to close a file? Compile error. I/O handles must be consumed exactly once:

<!-- test: compile -->
```rask
import fs
import io

struct Stats {
    lines: u64
}

enum StatsError {
    Io(io.IoError)
}

func parse_stats(data: string) -> Stats {
    return Stats { lines: data.lines().count() }
}

func process(path: string) -> Stats or StatsError {
    mut file = try fs.open(path)
    ensure file.close()

    let data = try file.read_text()
    return parse_stats(data)
    // file.close() runs here, guaranteed
}
```

No special cleanup syntax, no `defer`, no destructors-that-might-not-run. The compiler just refuses to compile if you forget. That's safety by structure—the bug isn't caught, it's impossible.

## So what's the soul?

There's no formula. Each decision is a judgment call, and I've probably gotten some of them wrong. But the pattern is: make the safe thing the default, make costs visible where it matters, and remove ceremony where it goes viral.

What I'm reaching for is a language where memory safety doesn't *feel like* memory safety. You write code thinking about your problem, and the safety falls out from the structure. I'm still early enough that it could all fall apart once real programs hit the design.
