# C Interop

## The Question
How does Rask call C code and expose Rask code to C?

## Decision
Two approaches: automatic header parsing (built-in C parser, like Zig) for well-behaved libraries, and explicit `extern "C"` bindings for edge cases. No libclang dependency.

## Rationale
Zig proves a built-in C parser can handle most real-world C interop without heavy dependencies. Explicit bindings give you an escape hatch for C++, complex macros, and compiler extensions. The combination gives convenience for common cases and full control when you need it.

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
- All C function calls require `unsafe` context

**Example:**
```rask
import c "stdio.h"
import c "mylib.h" as mylib

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
- `extern "C"` declarations must match C ABI exactly
- Compiler doesn't verify correctness (programmer responsibility)
- Explicit bindings can coexist with `import c`
- Explicit bindings override auto-parsed declarations

**When to use explicit bindings:**
- C++ libraries (automatic parsing not supported)
- Headers with complex macros (token pasting, stringification)
- Compiler-specific extensions (`__attribute__`, `__declspec`)
- Binding only a subset of a large API

### Mixing Both Approaches

I chose two approaches because neither is sufficient alone. Auto-parsing handles 90% of C headers; explicit bindings cover the rest. The real power is combining them—auto-parse the bulk, override the parts that fail.

**Rules:**
- Explicit bindings override auto-parsed declarations per-symbol (same name replaces)
- Explicit bindings for symbols not in the header are additive (new declarations)
- `import c ... hiding { symbol1, symbol2 }` suppresses specific auto-parsed symbols without replacing them

**Example — SDL2 with macro overrides:**
```rask
import c "SDL2/SDL.h" as sdl hiding { SDL_FOURCC }

// SDL_FOURCC is a macro using token pasting — auto-parse skipped it.
// Provide the binding manually:
extern "C" {
    func SDL_FOURCC(a: u8, b: u8, c: u8, d: u8) -> u32
}

func main() {
    unsafe {
        sdl.SDL_Init(sdl.SDL_INIT_VIDEO)
        const format = SDL_FOURCC('Y' as u8, 'U' as u8, 'Y' as u8, '2' as u8)
        sdl.SDL_Quit()
    }
}
```

**Example — large library, only binding a subset:**
```rask
// Auto-parse for types and constants, explicit for the 3 functions we use
import c "openssl/ssl.h" as ssl

extern "C" {
    // Override with more precise signatures
    func SSL_CTX_new(method: *ssl.SSL_METHOD) -> *ssl.SSL_CTX
    func SSL_new(ctx: *ssl.SSL_CTX) -> *ssl.SSL
    func SSL_free(ssl_ptr: *ssl.SSL)
}
```

This dual-mode approach is Rask's edge over Zig, which only has automatic translation—when `@cImport` fails on a symbol, you're stuck rewriting the entire binding set manually.

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

### String Interop

Rask strings are length-based (not null-terminated) internally. C expects null-terminated `char*`. Conversion methods bridge the gap.

| Method | Signature | Behavior |
|--------|-----------|----------|
| `.as_c_str()` | `string -> *u8` | Returns null-terminated pointer. Zero-cost if string is already null-terminated internally; allocates a copy otherwise. Valid until string is dropped or mutated. |
| `.ptr` | `string -> *u8` | Raw pointer to string data. NOT null-terminated. Use with `.len` for pointer+length APIs. |
| `string.from_c(ptr)` | `*u8 -> string` | Copies from null-terminated C string. Scans for null. **Unsafe.** |
| `string.from_c(ptr, len)` | `(*u8, c_size) -> string` | Copies from pointer + explicit length. No null scan. **Unsafe.** |

**Example:**
```rask
func call_c_string_api(name: string) {
    unsafe {
        // Null-terminated — for printf, fopen, etc.
        c.printf("Hello %s\n".as_c_str(), name.as_c_str())

        // Pointer + length — for write(), send(), etc.
        c.write(fd, name.ptr, name.len)

        // Reading back from C
        const c_result = c.get_name()
        const rask_name = string.from_c(c_result)
    }
}
```

**Lifetime:** `.as_c_str()` returns a pointer into the string's memory. The pointer is invalidated if the string is moved, dropped, or mutated. Storing the pointer past the string's lifetime is undefined behavior.

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

**Not C-compatible:**
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
| Rust crate via C ABI | Use `compile_rust()` in build script + `import c` for header; see [Build Scripts](build.md) |

## Examples

### SQLite Wrapper
```rask
import c "sqlite3.h" as sql

public struct Database {
    handle: *sql.sqlite3
}

public func open(path: string) -> Database or Error {
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

Linear resources (files, sockets, etc.) crossing FFI boundary need special handling:

```rask
@resource
struct File { fd: c_int }

func call_c_with_file(file: File) -> () or Error {
    let fd = file.fd
    ensure try file.close()  // Guarantee cleanup after C returns
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

## Cross-Compilation

C interop must work correctly when cross-compiling. The target platform determines type sizes, struct layouts, and header behavior.

### Target-Aware C Types

All `c_*` types resolve to the **target** platform's sizes, not the host's:

| Type | Linux x86_64 | Windows x86_64 | ARM 32-bit |
|------|-------------|----------------|------------|
| `c_int` | 4 bytes | 4 bytes | 4 bytes |
| `c_long` | 8 bytes | 4 bytes | 4 bytes |
| `c_size` | 8 bytes | 8 bytes | 4 bytes |
| `c_char` | signed | signed | unsigned |

The compiler resolves these from the target triple (`ctx.target.arch`, `ctx.target.os`, `ctx.target.env`). No hardcoded sizes.

### Header Re-Parsing

`import c "header.h"` re-parses headers per target. This is necessary because:
- Struct layouts change (padding, alignment differ across platforms)
- `#ifdef` guards produce different declarations per OS
- Type sizes affect field offsets in `extern "C" struct`

The C parser receives the target triple and sets predefined macros (`__linux__`, `_WIN32`, `__aarch64__`, etc.) accordingly.

### C Compiler Selection for `compile_c()`

`compile_c()` in build scripts needs a C compiler for the target. Resolution order:
1. `CC` environment variable (explicit override)
2. `zig cc -target <triple>` if Zig is available (zero-setup cross-compilation)
3. System C compiler for native builds

I chose to support `zig cc` as an optional backend because Zig bundles cross-compilation toolchains for 90+ targets in a single binary. This gives Rask cross-compilation for C dependencies without maintaining our own toolchains.

```rask
func build(ctx: BuildContext) -> () or Error {
    // compile_c() auto-detects cross-compiler
    // If targeting aarch64-linux and zig is available, uses zig cc
    try ctx.compile_c(CompileOptions {
        sources: ["vendor/sqlite3.c"],
        flags: ["-O2"],
    })
}
```

**No bundled toolchains.** Rask doesn't ship libc sources or cross-compilers. That's Zig's job and they do it well. Rask delegates to whatever C toolchain is available.

## Non-Goals

Things I considered and deliberately chose not to do:

- **No direct C++ import.** C++ name mangling, templates, exceptions, and RTTI don't cross FFI boundaries cleanly. Nobody has solved this well—not Zig, not Rust. The industry standard is C wrappers (`extern "C"` functions), and that works fine. Use explicit bindings for C++ libraries.
- **No libc independence.** Rask targets platforms where libc exists (Linux, macOS, Windows, BSDs). Bare-metal and freestanding targets are out of scope for now. This keeps the runtime simpler and the stdlib more capable.
- **No bundled cross-compilation toolchains.** Zig bundles ~50MB of libc sources. That's impressive engineering but not something I want to maintain. Use `zig cc`, system cross-compilers, or Docker.
- **No `rask cc` drop-in C compiler.** It would require bundling Clang. The maintenance cost doesn't justify the adoption benefit.

## Integration Notes

- **Unsafe:** All C function calls require `unsafe` context. C cannot provide Rask's safety guarantees.
- **Memory Model:** Ownership at FFI boundary is programmer responsibility. Document who owns what.
- **Build System:** C include paths and link libraries configured in `rask.build` or CLI.
- **Tooling:** IDE should show C type mappings and warn about unsafe C calls.
