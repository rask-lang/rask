<!-- id: std.os -->
<!-- status: decided -->
<!-- summary: Process control, environment variables, platform info -->

# OS

Single `os` module for process and platform interaction: env vars, args, exit, pid, platform info.

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

---

## Appendix (non-normative)

### Rationale

**Single module:** Previous design split env vars, args, and exit across `env`, `cli`, and `std` modules. One `os` import is simpler — these are all process-level operations.

**E1 (returns optional):** Env vars may or may not exist. Returning `string?` forces handling the missing case. `env_or` covers the common "default value" pattern.

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

### See Also

- `std.cli` — Structured argument parsing (builds on `os.args()`)
