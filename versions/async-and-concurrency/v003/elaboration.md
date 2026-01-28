# Elaboration: Async Function Syntax and Types

## Function Declaration Syntax

**Keyword placement:** `async` MUST precede `fn`.

```
async fn fetch_user(id: u64) -> Result<User, Error> {
    let response = async_http_get(url)?
    parse_user(response)
}
```

**NOT allowed:**
```
fn fetch_user(id: u64) async -> Result<User, Error>  // ERROR
```

**Rationale:** Prefix keyword makes async nature visible at declaration site, consistent with Rust/JS.

## Return Types

### Declared Return Type

Async functions declare the **eventual value type**, not the task type.

```
async fn compute() -> i32 {
    expensive_async_work()
}
```

**Actual return type (implicit):** Compiler translates to `AsyncTask<i32>` internally, but programmer writes `i32`.

**Rationale:** Reduces ceremony. The `async` keyword already signals task-ness.

### Result Types

```
async fn may_fail() -> Result<User, Error> {
    let data = async_read_file(path)?
    parse_user(data)
}
```

Same `?` propagation as sync code.

### Unit Return

```
async fn background_work() {
    loop {
        async_process_queue()
    }
}
```

Implicitly returns `AsyncTask<()>`.

## Calling Async Functions

### From Async Context (Await)

**Syntax:** Postfix `.await` operator.

```
async fn caller() {
    let user = fetch_user(123).await?
    print(user.name)
}
```

**Semantics:**
- `.await` yields control to runtime until task completes
- Returns the inner value (`User` in example)
- Can be chained: `fetch().await?.process().await?`

**Desugaring:**
```
fetch_user(123)         // Returns AsyncTask<Result<User, Error>>
    .await              // Yields, returns Result<User, Error>
    ?                   // Unwraps Result, returns User or propagates Error
```

### From Sync Context (Block On)

**NOT allowed directly.** Must use explicit adapter:

```
fn sync_main() {
    let user = fetch_user(123).await  // COMPILE ERROR: no async runtime
}
```

**Required:**
```
fn sync_main() {
    let user = block_on(fetch_user(123))?  // Explicit boundary
    print(user.name)
}
```

**block_on signature:**
```
fn block_on<T>(task: AsyncTask<T>) -> T
```

Creates temporary runtime, blocks thread until task completes.

**Cost:** Explicit function name signals blocking overhead.

## Async Blocks

**Syntax:** `async { ... }`

```
let task = async {
    let data = async_load().await?
    process(data)
}

let result = task.await?
```

**Type:** `async { expr }` produces `AsyncTask<T>` where `T` is type of `expr`.

**Use case:** Creating tasks without named functions (closures, inline tasks).

## Async Closures

**Syntax:** `async |args| { ... }`

```
let tasks = urls.map(async |url| {
    async_fetch(url).await
})

for task in tasks {
    let data = task.await?
    process(data)
}
```

**Capture rules:** Same as sync closures (move semantics, copy for small types).

## Function Color Boundaries

| From \ To | Sync Function | Async Function |
|-----------|---------------|----------------|
| **Sync** | Direct call | `block_on(async_fn())` |
| **Async** | Direct call (blocks runtime) | `async_fn().await` |

**Asymmetry:**
- Async can call sync (inefficient but allowed)
- Sync cannot call async directly (compile error, must use block_on)

**Rationale:** Prevents accidental blocking in async code; forces explicit boundary marking.

## Type System Integration

### AsyncTask<T> Type

**Internal type (not written by user):**
```
// Compiler-internal representation
struct AsyncTask<T> {
    // Implementation details (state machine, waker, etc.)
}
```

**User never writes:** `AsyncTask<T>` does not appear in user code.

**User writes:**
```
async fn foo() -> T     // Compiler infers AsyncTask<T>
```

**Rationale:** Reduces noise. `async` keyword is sufficient marker.

### Trait Bounds

```
fn process_async<F, T>(f: F) -> T
where
    F: AsyncFn() -> T  // AsyncFn trait for async closures
{
    block_on(f())
}
```

**AsyncFn trait:** Marker trait for async closures/functions.

### Generic Async Functions

```
async fn load<T: Deserialize>(path: string) -> Result<T, Error> {
    let bytes = async_read_file(path).await?
    deserialize(bytes)
}

let config: Config = load("config.json").await?
```

Works like sync generics; `async` applies to the instantiated function.

## Control Flow in Async

### Early Return

```
async fn validate_user(id: u64) -> Result<User, Error> {
    let user = fetch_user(id).await?
    if !user.active {
        return Err(Inactive)  // Works like sync
    }
    Ok(user)
}
```

### Loops with Await

```
async fn poll_until_ready(url: string) -> Response {
    loop {
        let response = async_fetch(url).await?
        if response.ready {
            return response
        }
        async_sleep(1.seconds).await
    }
}
```

## Edge Cases

| Case | Handling |
|------|----------|
| `.await` in sync function | COMPILE ERROR: no runtime |
| Async function not awaited | WARNING: "unused AsyncTask" |
| Nested `.await` | `fetch().await?.fetch2().await?` — allowed |
| `.await` in match arm | Allowed; match suspends until await completes |
| Recursive async fn | Allowed (boxed state machine) |
| Async fn with linear params | Allowed; consume before await or transfer ownership |
| `ensure` with `.await` | Allowed; await yields, ensure still fires on scope exit |
| Panic during `.await` | Propagates to caller (same as sync) |

## Syntax Examples

**Basic async function:**
```
async fn greet(name: string) -> string {
    format("Hello, {}", name)
}
```

**Async with error handling:**
```
async fn load_config() -> Result<Config, Error> {
    let raw = async_read_file("config.toml").await?
    parse_config(raw)
}
```

**Async closure:**
```
async |url| {
    async_fetch(url).await
}
```

**Async block:**
```
let task = async {
    expensive_work().await
}
```

**Calling chain:**
```
let result = fetch_user(123)
    .await?
    .get_posts()
    .await?
    .first()
```

## Integration Notes

**Nurseries:** Async nursery syntax uses `async nursery` (consistent with async fn):
```
async nursery { |n|
    n.async_spawn { work() }
}
```

**Channels:** Same channel types work in async; send/recv automatically yield when appropriate.

**Linear types:** Async functions can take linear parameters; must consume before suspend points (await).

**Error handling:** `?` works identically in async as in sync.

**Tooling:** IDE SHOULD show inferred AsyncTask types as ghost annotation on function signature.

## Cost Transparency

| Syntax | Visible Cost |
|--------|--------------|
| `async fn` | Function is async (yields at await points) |
| `.await` | Suspension point (yields to runtime) |
| `block_on` | Creates runtime, blocks thread (heavyweight) |
| Sync I/O in async | IDE warns "blocks runtime" |

**Passes TC ≥ 0.90:** Async boundaries, suspension points, and blocking all visible.
