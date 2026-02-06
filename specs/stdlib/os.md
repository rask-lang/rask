# OS — Process and Platform Interface

## The Question

How does a Rask program interact with its environment? Where do environment variables, command-line args, and process control live?

## Decision

Single `os` module for all process and platform interaction. One import gives you env vars, args, exit, pid, and platform info.

## Rationale

**Why one module instead of Rust's scattered approach?**
- Rust splits this across `std::env` (vars, args), `std::process` (exit, Command), `std::os` (platform-specific). You need three imports for basic process interaction
- Go puts it all in `os` — env, args, exit, getpid. Simple and discoverable
- These are all "what is my process environment?" questions — they belong together

**Why no filesystem in `os`?**
- `current_dir()` and `home_dir()` are filesystem queries — they live in `fs`
- `os` is about process identity and platform. `fs` is about what's on disk
- Clean boundary: `os` never touches the filesystem

**Why `os.env()` returns `string?` instead of `string or EnvError`?**
- A missing env var isn't an error — it's expected. `Option` is the right type
- `os.env_or("PORT", "8080")` covers the common case of defaults
- Matches Go's `os.Getenv()` / `os.LookupEnv()` split but with Option sugar

## Specification

### Environment Variables

```rask
os.env(key: string) -> string?                       // get env var
os.env_or(key: string, default: string) -> string     // get with default
os.set_env(key: string, value: string)                // set env var
os.remove_env(key: string)                            // unset env var
os.vars() -> Vec<(string, string)>                    // all env vars
```

### Command-Line Arguments

```rask
os.args() -> Vec<string>    // raw args including program name at index 0
```

For structured argument parsing, see the `cli` module.

### Process Control

```rask
os.exit(code: i32) -> !     // exit process, never returns
os.getpid() -> u32          // process ID
```

### Platform Info

```rask
os.platform() -> string     // "linux", "macos", "windows", "wasm"
os.arch() -> string          // "x86_64", "aarch64", "wasm32"
```

### Access Pattern

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
    println("PID: {os.getpid()}")
}
```

## Examples

### Environment-Based Configuration

```rask
import os

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

### Platform-Specific Behavior

```rask
import os

func default_config_dir() -> string {
    return match os.platform() {
        "linux" => os.env_or("XDG_CONFIG_HOME", "{os.env_or("HOME", "/tmp")}/.config"),
        "macos" => "{os.env_or("HOME", "/tmp")}/Library/Application Support",
        "windows" => os.env_or("APPDATA", "C:/Users/Default/AppData/Roaming"),
        _ => ".config",
    }
}
```

### Graceful Shutdown

```rask
import os

func main() {
    const result = run_server()
    if result is Err(e) {
        println("Error: {e}")
        os.exit(1)
    }
}
```

## Migration from env/std/cli

Previous interpreter modules are now consolidated:

| Old | New | Notes |
|-----|-----|-------|
| `env.var(key)` | `os.env(key)` | Returns `string?` |
| `env.vars()` | `os.vars()` | Returns `Vec<(string, string)>` |
| `cli.args()` | `os.args()` | Identical |
| `std.exit(code)` | `os.exit(code)` | Identical |

Old import names (`env`, `std`, `cli`) remain as aliases during transition.

## References

- specs/stdlib/cli.md — Structured argument parsing (builds on `os.args()`)
- CORE_DESIGN.md — Transparent cost (env lookups are syscalls)

## Status

**Specified** — ready for implementation in interpreter.
