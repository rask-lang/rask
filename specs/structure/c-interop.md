# C Interop

## The Question
How does Rask call C code and expose Rask code to C?

## Decision
Two approaches: automatic header parsing (built-in C parser, like Zig) for well-behaved libraries, and explicit `extern "C"` bindings for edge cases. No libclang dependency.

## Rationale
Zig proves that a built-in C parser can handle most real-world C interop without heavy dependencies. Explicit bindings provide an escape hatch for C++, complex macros, and compiler extensions. The combination gives convenience for common cases and full control when needed.

## Specification

### Two Approaches

| Approach | Use Case |
|----------|----------|
| `import c "header.h"` | Automatic parsing for well-behaved C libraries |
| `extern "C" { }` | Explicit bindings for edge cases, C++, complex macros |

### Automatic Header Parsing

**Syntax:**

| Syntax | Effect |
|--------|--------|
| `import c "header.h"` | Parse header, expose as `c.symbol` |
| `import c "header.h" as name` | Parse header, expose as `name.symbol` |
| `import c { "a.h", "b.h" }` | Multiple headers, unified namespace |

**Implementation:**
- Compiler includes a built-in C parser (like Zig)—no external dependencies
- No libclang required; custom parser handles standard C
- Header parsed at compile time; C types/functions available immediately
- Calling C functions requires `unsafe` context

**Example:**
```rask
import c "stdio.h"
import c "mylib.h" as mylib

@entry
func main() {
    unsafe {
        c.printf("Hello %s\n".ptr, name.ptr)
        mylib.process(data.ptr, data.len)
    }
}
```

### Explicit Bindings

**For manual declarations when automatic parsing isn't suitable:**

```rask
// Single declaration
extern "C" func printf(format: *u8, ...) -> c_int

// Opaque type
extern "C" struct sqlite3

// Block syntax for multiple declarations
extern "C" {
    func open(path: *u8, flags: c_int) -> c_int
    func close(fd: c_int) -> c_int
    func read(fd: c_int, buf: *mut void, count: c_size) -> c_ssize
}
```

**Rules:**
- `extern "C"` declarations MUST match C ABI exactly
- Compiler does NOT verify correctness (programmer responsibility)
- Explicit bindings can coexist with `import c`
- Explicit bindings override auto-parsed declarations

**When to use explicit bindings:**
- C++ libraries (automatic parsing not supported)
- Headers with complex macros (token pasting, stringification)
- Compiler-specific extensions (`__attribute__`, `__declspec`)
- Binding only a subset of a large API

### Type Mapping

**Primitive types:**

| C Type | Rask Type |
|--------|-----------|
| `char` | `c_char` |
| `signed char` / `unsigned char` | `i8` / `u8` |
| `short` / `unsigned short` | `c_short` / `c_ushort` |
| `int` / `unsigned int` | `c_int` / `c_uint` |
| `long` / `unsigned long` | `c_long` / `c_ulong` |
| `long long` / `unsigned long long` | `c_longlong` / `c_ulonglong` |
| `float` / `double` | `f32` / `f64` |
| `_Bool` | `bool` |

**Pointer-sized types:**

| C Type | Rask Type |
|--------|-----------|
| `size_t` | `c_size` (alias for `usize`) |
| `ptrdiff_t` | `c_ptrdiff` (alias for `isize`) |
| `intptr_t` / `uintptr_t` | `isize` / `usize` |
| `wchar_t` | `c_wchar` |

**Pointer and function types:**

| C Type | Rask Type |
|--------|-----------|
| `T*` / `const T*` | `*T` |
| `void*` | `*void` |
| `int (*f)(int, int)` | `*func(c_int, c_int) -> c_int` |

**Composite types:**

| C Construct | Rask Equivalent |
|-------------|-----------------|
| `struct S { ... }` | `extern "C" struct S { ... }` |
| `union U { ... }` | `extern "C" union U { ... }` |
| `enum E { A, B }` | `extern "C" enum E { A, B }` |
| Bit fields | `@bitfield` annotation |
| Packed struct | `@packed` annotation |

### Preprocessor Handling

| Macro Type | Translation |
|------------|-------------|
| Integer constant (`#define FOO 42`) | `const FOO: c_int = 42` |
| String constant (`#define V "1.0"`) | `const V: *u8 = c"1.0"` |
| Simple alias (`#define HANDLE void*`) | `const HANDLE = *void` |
| Function-like (simple) | Inline generic function |
| Token pasting (`##`) | **Skip with warning** |
| Stringification (`#`) | **Skip with warning** |

**Warning for skipped macros:**
```rask
warning: skipping macro `CONTAINER_OF` (uses token pasting)
  --> /usr/include/linux/kernel.h:42
   = hint: use explicit binding if needed
```

### Exporting to C

| Feature | Mechanism |
|---------|-----------|
| Export function | `public extern "C" func name()` |
| Export type | `public extern "C" struct Name { ... }` |
| Header generation | `raskc --emit-header pkg` produces `pkg.h` |
| ABI | `extern "C"` uses C ABI; `public` alone uses Rask ABI |

**C-compatible types:**
- Primitives: `i8`-`i64`, `u8`-`u64`, `f32`, `f64`, `bool`
- C-specific: `c_int`, `c_long`, `c_size`, `c_char` (platform-dependent sizes)
- Pointers: `*T`, `*mut T`
- `extern "C" struct` with only C-compatible fields

**NOT C-compatible:**
- `string`, `Vec`, `Pool` (internal layout not stable)
- Handles (generational references have no C equivalent)
- Closures, trait objects

### Build Integration

```rask
// rask.build or CLI
c_include_paths: ["/usr/include", "vendor/"]
c_link_libs: ["ssl", "crypto"]
```

See [Build Scripts](build.md) for full build configuration.

### Edge Cases

| Case | Handling |
|------|----------|
| Header not found | Compile error with search paths shown |
| C++ header | Error: "C++ not supported; use explicit bindings" |
| Variadic C function | Callable from unsafe; Rask cannot export variadic |
| Opaque struct | Only pointer operations allowed |
| Inline function in header | Imported as declaration (body discarded) |
| Static function in header | Not imported (internal linkage) |
| Conflicting typedefs | First wins, warning emitted |
| Macro with side effects | Not imported; warning emitted |

## Examples

### SQLite Wrapper
```rask
import c "sqlite3.h" as sql

public struct Database {
    handle: *sql.sqlite3
}

public func open(path: string) -> Result<Database, Error> {
    let db: *sql.sqlite3 = null
    unsafe {
        let rc = sql.sqlite3_open(path.cstr(), &db)
        if rc != sql.SQLITE_OK {
            return Err(Error.new("sqlite open failed"))
        }
    }
    Ok(Database { handle: db })
}

public func close(db: Database) {
    unsafe { sql.sqlite3_close(db.handle) }
}
```

### Explicit Bindings for C++
```rask
// Can't parse C++ headers, so use explicit bindings
extern "C" {
    func cpp_library_init() -> c_int
    func cpp_library_process(data: *u8, len: c_size) -> c_int
    func cpp_library_shutdown()
}

func use_cpp_lib() {
    unsafe {
        cpp_library_init()
        cpp_library_process(data.ptr, data.len)
        cpp_library_shutdown()
    }
}
```

### Exporting to C
```rask
// Rask function callable from C
public extern "C" func rask_process(data: *u8, len: c_size) -> c_int {
    unsafe {
        let slice = slice_from_raw(data, len)
        match process(slice) {
            Ok(_) => 0,
            Err(_) => -1,
        }
    }
}

// C-compatible struct
public extern "C" struct RaskResult {
    success: bool,
    error_code: c_int,
}
```

## Linear Resources and FFI

Linear resources (files, sockets, etc.) crossing FFI boundary require special handling:

```rask
@resource
struct File { fd: c_int }

func call_c_with_file(file: File) -> Result<(), Error> {
    let fd = file.fd
    ensure file.close()?     // Guarantee cleanup after C returns
    unsafe {
        c.process_file(fd)   // Pass raw fd to C
    }
    Ok(())
}
```

**Rules:**
- Convert linear resource to raw pointer/handle before calling C
- The Rask side retains responsibility for cleanup
- Use `ensure` to guarantee cleanup after C call returns
- C functions cannot consume Rask linear resource types directly

## Integration Notes

- **Unsafe:** All C function calls require `unsafe` context. C cannot provide Rask's safety guarantees.
- **Memory Model:** Ownership at FFI boundary is programmer responsibility. Document who owns what.
- **Build System:** C include paths and link libraries configured in `rask.build` or CLI.
- **Tooling:** IDE should show C type mappings and warn about unsafe C calls.
