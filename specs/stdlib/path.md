# Path — Cross-Platform Path Manipulation

Single `Path` type. It's a string with path-aware methods. No `PathBuf`/`OsStr`/`OsString` zoo. Non-UTF-8 paths get lossy conversion at the system boundary.

## Specification

### Type

| Type | Description | Size | Copy? |
|------|-------------|------|-------|
| `Path` | Cross-platform file path | Same as string | No (owns string) |

Path is a value type wrapping a string. Separator is `/` internally on all platforms. Windows `\` is accepted on input and normalized to `/`.

### Constructors

```rask
Path.new(s: string) -> Path         // normalize separators
Path.from(s: string) -> Path        // alias for new
```

### Component Access

```rask
path.parent() -> Path?              // "/home/user/file.txt" -> Some("/home/user")
path.file_name() -> string?         // "/home/user/file.txt" -> Some("file.txt")
path.extension() -> string?         // "/home/user/file.txt" -> Some("txt")
path.stem() -> string?              // "/home/user/file.txt" -> Some("file")
path.components() -> Vec<string>    // "/home/user" -> ["home", "user"]
```

### Predicates

```rask
path.is_absolute() -> bool          // starts with "/" or drive letter
path.is_relative() -> bool          // not absolute
path.has_extension(ext: string) -> bool  // case-insensitive on Windows
```

### Manipulation

```rask
path.join(other: string) -> Path    // append component with separator
path / other -> Path                // operator sugar for join
path.with_extension(ext: string) -> Path   // replace extension
path.with_file_name(name: string) -> Path  // replace final component
```

### Conversion

```rask
path.to_string() -> string          // trivial — Path IS a string
```

### Access Pattern

```rask
import path

const p = Path.new("/home/user/docs/report.txt")

const dir = p.parent()              // Some("/home/user/docs")
const name = p.file_name()          // Some("report.txt")
const ext = p.extension()           // Some("txt")
const stem = p.stem()               // Some("report")

const sub = p.parent() ?? Path.new("/") / "backup" / "report.bak"
```

Note: `Path` is accessed through the `path` module import. `Path.new()` is a type constructor available after `import path`.

## Examples

### Building Paths

```rask
import path

func output_path(input: Path, ext: string) -> Path {
    const dir = input.parent() ?? Path.new(".")
    const name = input.stem() ?? "output"
    return dir / "{name}.{ext}"
}

func main() {
    const src = Path.new("src/main.rk")
    const out = output_path(src, "o")
    println(out.to_string())  // "src/main.o"
}
```

### Grep Clone — Walking Files

```rask
import path
import fs

func find_rask_files(dir: Path) -> Vec<Path> or string {
    const entries = try fs.read_dir(dir)
    const results = Vec.new()
    for entry in entries {
        const p = dir / entry.name()
        if entry.is_dir() {
            const sub = try find_rask_files(p)
            for f in sub {
                try results.push(f)
            }
        } else if p.has_extension("rask") {
            try results.push(p)
        }
    }
    return results
}
```

### Path Joining with `/` Operator

```rask
import path

const base = Path.new("/usr/local")
const bin = base / "bin"             // /usr/local/bin
const exe = bin / "rask"             // /usr/local/bin/rask

// Chaining
const config = Path.new(home) / ".config" / "rask" / "settings.toml"
```

## Edge Cases

- `Path.new("")` — empty path, `parent()` returns `None`, `file_name()` returns `None`
- `Path.new("/")` — root, `parent()` returns `None`, `file_name()` returns `None`
- `Path.new("file.tar.gz")` — `extension()` returns `Some("gz")`, `stem()` returns `Some("file.tar")`
- `Path.new(".gitignore")` — `extension()` returns `None`, `stem()` returns `Some(".gitignore")` (dotfiles are stems, not extensions)
- `Path.new("no_ext")` — `extension()` returns `None`
- Trailing slash: `Path.new("/home/user/")` normalized to `Path.new("/home/user")`
- Double separators: `Path.new("/home//user")` normalized to `Path.new("/home/user")`

## Platform Notes

| Platform | Separator | Absolute prefix | Notes |
|----------|-----------|-----------------|-------|
| Linux/macOS | `/` | `/` | Paths are UTF-8 (lossy from OS) |
| Windows | `\` normalized to `/` | `C:/` or `//` (UNC) | Drive letters preserved |
| WASM | `/` | `/` | Virtual filesystem |

Non-UTF-8 filenames on Unix are converted with lossy replacement (`�`) when entering Rask. This affects <0.01% of real-world paths.

## Integration

- `fs.open(path)` accepts both `Path` and `string` (Path coerces to string)
- `fs.current_dir()` and `fs.home_dir()` return `Path` — see fs module
- Path is not a linear resource — no cleanup needed

## References

- specs/stdlib/strings.md — Path wraps string type
- specs/memory/value-semantics.md — Value type semantics

## Status

**Specified** — ready for implementation in interpreter.
