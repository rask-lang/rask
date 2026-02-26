<!-- id: std.os -->
<!-- status: decided -->
<!-- summary: Process control, environment, platform info, subprocess spawning, signal handling -->
<!-- depends: stdlib/io.md, stdlib/time.md, concurrency/channels.md -->

# OS

Single `os` module: env vars, args, exit, platform info, subprocess spawning, and signal handling.

## Environment Variables

| Rule | Description |
|------|-------------|
| **E1: Get** | `os.env(key)` returns `string?` |
| **E2: Get with default** | `os.env_or(key, default)` returns `string` |
| **E3: Set** | `os.set_env(key, value)` sets an env var |
| **E4: Remove** | `os.remove_env(key)` unsets an env var |
| **E5: List all** | `os.vars()` returns `Vec<(string, string)>` |

## Command-Line Arguments

| Rule | Description |
|------|-------------|
| **A1: Raw args** | `os.args()` returns `Vec<string>` including program name at index 0 |

## Process Control

| Rule | Description |
|------|-------------|
| **P1: Exit** | `os.exit(code: i32)` exits the process; return type is `!` (never) |
| **P2: PID** | `os.getpid()` returns `u32` |

## Platform Info

| Rule | Description |
|------|-------------|
| **I1: Platform** | `os.platform()` returns `"linux"`, `"macos"`, `"windows"`, or `"wasm"` |
| **I2: Architecture** | `os.arch()` returns `"x86_64"`, `"aarch64"`, or `"wasm32"` |

<!-- test: parse -->
```rask
import os

func main() {
    const port = os.env_or("PORT", "8080")
    const args = os.args()

    if args.len() < 2 {
        println("Usage: {args[0]} <file>")
        os.exit(1)
    }

    println("Running on {os.platform()}/{os.arch()}")
}
```

## Subprocess

| Rule | Description |
|------|-------------|
| **C1: Command builder** | `Command` configures program, args, env, working directory, and I/O before execution |
| **C2: Run** | `command.run()` executes to completion and captures output |
| **C3: Spawn** | `command.spawn()` starts the process and returns a linear `Process` handle |
| **C4: Process resource** | `Process` is `@resource` — must be consumed via `wait()` or `kill_and_wait()` |

<!-- test: skip -->
```rask
struct Command { }

extend Command {
    func new(program: string) -> Command
    func arg(take self, arg: string) -> Command
    func args(take self, args: Vec<string>) -> Command
    func env(take self, key: string, value: string) -> Command
    func cwd(take self, dir: string) -> Command
    func stdin(take self, cfg: Stdio) -> Command
    func stdout(take self, cfg: Stdio) -> Command
    func stderr(take self, cfg: Stdio) -> Command

    func run(self) -> Output or IoError
    func spawn(self) -> Process or IoError
}

enum Stdio { Inherit, Piped, Null }
```

### Output

<!-- test: skip -->
```rask
struct Output {
    public status: i32
    public stdout: string
    public stderr: string
}

extend Output {
    func success(self) -> bool    // status == 0
}
```

### Process

<!-- test: skip -->
```rask
@resource
struct Process { }

extend Process {
    func wait(take self) -> Output or IoError
    func kill_and_wait(take self) -> Output or IoError
    func try_wait(self) -> Output? or IoError
    func id(self) -> u32
    func write_stdin(self, data: string) -> () or IoError
    func read_stdout(self) -> string or IoError
}
```

<!-- test: skip -->
```rask
import os

// Run to completion
const output = try Command.new("ls").arg("-la").run()
if output.success() {
    println(output.stdout)
}

// Spawn and interact
const proc = try Command.new("grep")
    .arg("hello")
    .stdin(Stdio.Piped)
    .stdout(Stdio.Piped)
    .spawn()
ensure proc.kill_and_wait()

try proc.write_stdin("hello world\ngoodbye world\n")
const result = try proc.read_stdout()
const output = try proc.wait()
```

## Signals

| Rule | Description |
|------|-------------|
| **SG1: Signal enum** | Named signals for portable handling |
| **SG2: Channel-based** | `os.on_signal(signals)` returns a `Receiver<Signal>` from the concurrency module |
| **SG3: Default restored** | When the receiver is dropped, the default OS signal behavior is restored |

<!-- test: skip -->
```rask
enum Signal {
    Interrupt       // SIGINT (Ctrl+C)
    Terminate       // SIGTERM
    Hangup          // SIGHUP
    User1           // SIGUSR1
    User2           // SIGUSR2
}

os.on_signal(signals: Vec<Signal>) -> Receiver<Signal> or IoError
```

<!-- test: skip -->
```rask
import os

// Graceful shutdown
const signals = try os.on_signal([Signal.Interrupt, Signal.Terminate])

// Block until signal received
const sig = try signals.recv()
println("Received {sig}, shutting down...")
cleanup()
```

<!-- test: skip -->
```rask
import os

// Server with graceful shutdown
func main() -> () or Error {
    using Multitasking {
        const signals = try os.on_signal([Signal.Interrupt, Signal.Terminate])
        const server = try HttpServer.listen("0.0.0.0:8080")
        ensure server.close()

        const shutdown = spawn(|| {
            signals.recv()
        })

        const serve = spawn(|| {
            loop {
                const (req, responder) = try server.accept()
                spawn(|| {
                    ensure responder.respond(Response.internal_error("error"))
                    responder.respond(handle(req))
                }).detach()
            }
        })

        // Wait for either shutdown signal or server error
        select_first(shutdown, serve)
    }
}
```

## Error Messages

```
ERROR [std.os/P1]: unreachable code after os.exit()
   |
5  |  os.exit(1)
6  |  println("done")
   |  ^^^^^^^^^^^^^^^ unreachable — os.exit() never returns

WHY: os.exit() has return type ! (never), so subsequent code is dead.

FIX: Remove unreachable code or move os.exit() to end of block.
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| `os.env("MISSING")` | E1 | Returns `None` |
| `os.args()` with no args | A1 | Vec contains at least program name |
| `os.exit(0)` in defer | P1 | Exits immediately, remaining defers skipped |
| Process not consumed | C4 | Compile error |
| `wait()` on exited process | C4 | Returns cached Output |
| `write_stdin` without `Stdio.Piped` | C3 | `IoError.Other("stdin not piped")` |
| `read_stdout` without `Stdio.Piped` | C3 | `IoError.Other("stdout not piped")` |
| Program not found | C2 | `IoError.NotFound` |
| Signal receiver dropped | SG3 | Default OS signal behavior restored |
| Multiple receivers for same signal | SG2 | Last registration wins, previous receiver gets closed |
| `SIGKILL` / `SIGSTOP` | SG1 | Not in enum — cannot be caught (OS enforced) |

---

## Appendix (non-normative)

### Rationale

**Single module:** Previous design split env vars, args, and exit across `env`, `cli`, and `std` modules. One `os` import is simpler — these are all process-level operations.

**E1 (returns optional):** Env vars may or may not exist. Returning `string?` forces handling the missing case. `env_or` covers the common "default value" pattern.

**C1 (Command builder):** Builder pattern matches `OpenOptions` in `std.fs`. Each setter returns a new Command. `run()` covers the common case (execute, capture output). `spawn()` is for long-running or interactive processes.

**C4 (linear Process):** A spawned subprocess is an OS resource. Forgetting to wait on it leaves a zombie process. Making it `@resource` catches this at compile time. `kill_and_wait` is the consumption method for `ensure` — guarantees the child is reaped even on error paths.

**SG2 (channel-based signals):** Callbacks in signal context are tricky — limited to async-signal-safe functions, reentrancy issues, can't allocate. Channels avoid all of this. The signal handler writes to a pipe, the channel reads it in normal context. Integrates with `select` for multiplexing.

**SG3 (default restored on drop):** Channels are non-linear (`conc.async/CH1`), so the receiver can go out of scope. When it does, the signal handler is de-registered and the default OS behavior resumes (terminate for SIGINT/SIGTERM). This prevents stale handlers.

### Patterns & Guidance

**Environment-based configuration:**

<!-- test: skip -->
```rask
struct Config {
    host: string
    port: i64
    debug: bool
}

func load_config() -> Config {
    return Config {
        host: os.env_or("HOST", "localhost"),
        port: os.env("PORT")?.parse<i64>() ?? 8080,
        debug: os.env("DEBUG")?.to_lowercase() == "true",
    }
}
```

**Platform-specific paths:**

<!-- test: skip -->
```rask
func default_config_dir() -> string {
    return match os.platform() {
        "linux" => os.env_or("XDG_CONFIG_HOME", "{os.env_or("HOME", "/tmp")}/.config"),
        "macos" => "{os.env_or("HOME", "/tmp")}/Library/Application Support",
        "windows" => os.env_or("APPDATA", "C:/Users/Default/AppData/Roaming"),
        _ => ".config",
    }
}
```

### Migration

| Old | New | Notes |
|-----|-----|-------|
| `env.var(key)` | `os.env(key)` | Returns `string?` |
| `env.vars()` | `os.vars()` | Returns `Vec<(string, string)>` |
| `cli.args()` | `os.args()` | Identical |
| `std.exit(code)` | `os.exit(code)` | Identical |

Old import names (`env`, `std`, `cli`) remain as aliases during transition.

**Subprocess pipeline:**

<!-- test: skip -->
```rask
// Run a command and check its exit status
const output = try Command.new("cargo").arg("build").run()
if !output.success() {
    println("Build failed: {output.stderr}")
    os.exit(1)
}
```

**Graceful shutdown pattern:**

<!-- test: skip -->
```rask
const signals = try os.on_signal([Signal.Interrupt, Signal.Terminate])

// In a select or spawn, wait for signal
spawn(|| {
    const sig = try signals.recv()
    println("Shutting down on {sig}...")
    shutdown_server()
}).detach()
```

### See Also

- `std.cli` — Structured argument parsing (builds on `os.args()`)
- `std.io` — `IoError`, `Reader`/`Writer` traits
- `conc.async` — Channels for signal delivery, `select_first` for shutdown
- `mem.resource-types` — `@resource` and `ensure` semantics
