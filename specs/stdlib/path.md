<!-- id: std.path -->
<!-- status: decided -->
<!-- summary: Cross-platform path manipulation with single Path type -->
<!-- depends: stdlib/strings.md, memory/value-semantics.md -->

# Path

Single `Path` type wrapping a string. No `PathBuf`/`OsStr`/`OsString` zoo. Non-UTF-8 paths get lossy conversion at the system boundary.

## Core Rules

| Rule | Description |
|------|-------------|
| **P1: String wrapper** | Path is a value type wrapping a string. Not Copy (owns string) |
| **P2: Normalized separators** | Internal separator is `/` on all platforms. Windows `\` normalized on input |
| **P3: UTF-8 only** | Non-UTF-8 filenames converted with lossy replacement at system boundary |
| **P4: No cleanup** | Path is not a linear resource — no cleanup needed |

## Type

| Type | Description | Size | Copy? |
|------|-------------|------|-------|
| `Path` | Cross-platform file path | Same as string | No (owns string) |

## Constructors

<!-- test: skip -->
```rask
Path.new(s: string) -> Path         // normalize separators
Path.from(s: string) -> Path        // alias for new
```

## Component Access

<!-- test: skip -->
```rask
path.parent() -> Path?              // "/home/user/file.txt" -> Some("/home/user")
path.file_name() -> string?         // "/home/user/file.txt" -> Some("file.txt")
path.extension() -> string?         // "/home/user/file.txt" -> Some("txt")
path.stem() -> string?              // "/home/user/file.txt" -> Some("file")
path.components() -> Vec<string>    // "/home/user" -> ["home", "user"]
```

## Predicates

<!-- test: skip -->
```rask
path.is_absolute() -> bool          // starts with "/" or drive letter
path.is_relative() -> bool          // not absolute
path.has_extension(ext: string) -> bool  // case-insensitive on Windows
```

## Manipulation

<!-- test: skip -->
```rask
path.join(other: string) -> Path    // append component with separator
path / other -> Path                // operator sugar for join
path.with_extension(ext: string) -> Path   // replace extension
path.with_file_name(name: string) -> Path  // replace final component
```

## Conversion

<!-- test: skip -->
```rask
path.to_string() -> string          // trivial — Path IS a string
```

## Platform Behavior

| Platform | Separator | Absolute prefix | Notes |
|----------|-----------|-----------------|-------|
| Linux/macOS | `/` | `/` | Paths are UTF-8 (lossy from OS) |
| Windows | `\` normalized to `/` | `C:/` or `//` (UNC) | Drive letters preserved |
| WASM | `/` | `/` | Virtual filesystem |

## Error Messages

```
ERROR [std.path/P3]: non-UTF-8 path encountered
   |
5  |  const p = fs.read_dir(dir)
   |            ^^^^^^^^^^^^^^^^ path contains invalid UTF-8

WHY: Rask paths are UTF-8. Non-UTF-8 filenames are lossy-converted at the system boundary.
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| `Path.new("")` | — | Empty path; `parent()` and `file_name()` return `None` |
| `Path.new("/")` | — | Root; `parent()` and `file_name()` return `None` |
| `Path.new("file.tar.gz")` | — | `extension()` -> `Some("gz")`, `stem()` -> `Some("file.tar")` |
| `Path.new(".gitignore")` | — | `extension()` -> `None`, `stem()` -> `Some(".gitignore")` (dotfiles are stems) |
| `Path.new("no_ext")` | — | `extension()` -> `None` |
| Trailing slash `"/home/user/"` | P2 | Normalized to `"/home/user"` |
| Double separators `"/home//user"` | P2 | Normalized to `"/home/user"` |
| Non-UTF-8 filenames on Unix | P3 | Lossy replacement (`\uFFFD`), affects <0.01% of real paths |

---

## Appendix (non-normative)

### Rationale

**P1 (single type):** I chose one Path type over Rust's `Path`/`PathBuf`/`OsStr`/`OsString` split. The conversion ergonomics aren't worth the type zoo.

**P3 (UTF-8 only):** Lossy conversion at the boundary trades <0.01% fidelity for 100% ergonomic string operations on paths. The tradeoff is worth it.

### Patterns & Guidance

**Building output paths:**

```rask
import path

func output_path(input: Path, ext: string) -> Path {
    const dir = input.parent() ?? Path.new(".")
    const name = input.stem() ?? "output"
    return dir / "{name}.{ext}"
}
```

**Walking files:**

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

**Chaining with `/` operator:**

```rask
import path

const base = Path.new("/usr/local")
const bin = base / "bin"             // /usr/local/bin
const exe = bin / "rask"             // /usr/local/bin/rask

// Chaining
const config = Path.new(home) / ".config" / "rask" / "settings.toml"
```

### Integration

- `fs.open(path)` accepts both `Path` and `string` (Path coerces to string)
- `fs.current_dir()` and `fs.home_dir()` return `Path`

### See Also

- `std.strings` — Path wraps string type
- `mem.value-semantics` — Value type semantics
