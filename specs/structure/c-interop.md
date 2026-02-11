<!-- id: struct.c-interop -->
<!-- status: decided -->
<!-- summary: Built-in C parser for headers plus explicit extern "C" bindings for edge cases -->
<!-- depends: memory/unsafe.md, structure/build.md -->

# C Interop

Two approaches: automatic header parsing (built-in C parser, like Zig) for well-behaved libraries, and explicit `extern "C"` bindings for edge cases. No libclang dependency.

## Import Mechanisms

| Rule | Description |
|------|-------------|
| **CI1: Auto-parse** | `import c "header.h"` parses header with built-in C parser, exposes as `c.symbol` |
| **CI2: Explicit binding** | `extern "C" { }` declares C functions/types manually |
| **CI3: Unsafe required** | All C function calls require `unsafe` context |
| **CI4: Override** | Explicit bindings override auto-parsed declarations per-symbol |
| **CI5: Hiding** | `import c "header.h" hiding { symbol }` suppresses specific auto-parsed symbols |

| Syntax | Effect |
|--------|--------|
| `import c "header.h"` | Parse header, expose as `c.symbol` |
| `import c "header.h" as name` | Parse header, expose as `name.symbol` |
| `import c { "a.h", "b.h" }` | Multiple headers, unified namespace |

<!-- test: skip -->
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

## Explicit Bindings

<!-- test: skip -->
```rask
extern "C" func printf(format: *u8, ...) -> c_int
extern "C" struct sqlite3

extern "C" {
    func open(path: *u8, flags: c_int) -> c_int
    func close(fd: c_int) -> c_int
    func read(fd: c_int, buf: *void, count: c_size) -> c_ssize
}
```

Use explicit bindings for: C++ libraries, complex macros (token pasting, stringification), compiler-specific extensions, binding only a subset.

## Type Mapping

| Rule | Description |
|------|-------------|
| **TM1: Platform types** | `c_int`, `c_long`, etc. resolve to target platform sizes, not host |
| **TM2: Pointer mapping** | `T*` → `*T`, `void*` → `*void`, function pointers → `*func(...)` |
| **TM3: Composite types** | `extern "C" struct/union/enum` for C-layout types |

### Primitive Types

| C Type | Rask Type |
|--------|-----------|
| `char` | `c_char` |
| `int` / `unsigned int` | `c_int` / `c_uint` |
| `long` / `unsigned long` | `c_long` / `c_ulong` |
| `size_t` | `c_size` (alias for `usize`) |
| `float` / `double` | `f32` / `f64` |

### Composite Types

| C Construct | Rask Equivalent |
|-------------|-----------------|
| `struct S { ... }` | `extern "C" struct S { ... }` |
| `union U { ... }` | `extern "C" union U { ... }` |
| `enum E { A, B }` | `extern "C" enum E { A, B }` |
| Bit fields | `@bitfield` annotation |
| Packed struct | `@packed` annotation |

## String Interop

| Rule | Description |
|------|-------------|
| **ST1: as_c_str** | `.as_c_str()` returns null-terminated `*u8` — zero-cost if already null-terminated, copies otherwise |
| **ST2: ptr + len** | `.ptr` + `.len` for pointer+length APIs — NOT null-terminated |
| **ST3: from_c** | `string.from_c(ptr)` copies from null-terminated C string (unsafe) |
| **ST4: Lifetime** | `.as_c_str()` pointer invalidated if string moved, dropped, or mutated |

<!-- test: skip -->
```rask
func call_c_string_api(name: string) {
    unsafe {
        c.printf("Hello %s\n".as_c_str(), name.as_c_str())
        c.write(fd, name.ptr, name.len)
        const rask_name = string.from_c(c.get_name())
    }
}
```

## Preprocessor Handling

| Macro Type | Translation |
|------------|-------------|
| Integer constant (`#define FOO 42`) | `const FOO: c_int = 42` |
| String constant (`#define V "1.0"`) | `const V: *u8 = c"1.0"` |
| Simple alias (`#define HANDLE void*`) | `const HANDLE = *void` |
| Function-like (simple) | Inline generic function |
| Token pasting (`##`) | Skip with warning |
| Stringification (`#`) | Skip with warning |

## Exporting to C

| Rule | Description |
|------|-------------|
| **EX1: Export function** | `public extern "C" func name()` |
| **EX2: Export type** | `public extern "C" struct Name { ... }` |
| **EX3: Header generation** | `raskc --emit-header pkg` produces `pkg.h` |
| **EX4: C-compatible only** | Exported types must use C-compatible fields (primitives, pointers, extern structs) |

Not C-compatible: `string`, `Vec`, `Pool`, handles, closures, trait objects.

## Linear Resources and FFI

| Rule | Description |
|------|-------------|
| **LR1: Convert first** | Convert linear resource to raw pointer/handle before calling C |
| **LR2: Rask owns cleanup** | Rask side retains responsibility for cleanup — use `ensure` |
| **LR3: No direct consumption** | C functions cannot consume Rask linear resource types directly |

## Cross-Compilation

| Rule | Description |
|------|-------------|
| **XC1: Target-aware types** | All `c_*` types resolve to target platform sizes |
| **XC2: Re-parse per target** | `import c "header.h"` re-parses per target (struct layouts, `#ifdef` guards differ) |
| **XC3: CC resolution** | `compile_c()` uses: `CC` env var → `zig cc` → system compiler |

## Error Messages

```
ERROR [struct.c-interop/CI3]: C call requires unsafe context
   |
8  |  c.printf("hello\n".ptr)
   |  ^^^^^^^^ C function call outside unsafe block
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Header not found | CI1 | Compile error with search paths shown |
| C++ header | CI1 | Error: "C++ not supported; use explicit bindings" |
| Variadic C function | CI3 | Callable from unsafe; Rask cannot export variadic |
| Opaque struct | CI2 | Only pointer operations allowed |
| Inline function in header | CI1 | Imported as declaration (body discarded) |
| Static function in header | CI1 | Not imported (internal linkage) |
| Macro with token pasting | CI1 | Skipped with warning |

---

## Appendix (non-normative)

### Rationale

**CI1 + CI2 (dual approach):** I chose two approaches because neither is sufficient alone. Auto-parsing handles 90% of C headers; explicit bindings cover the rest. The real power is combining them — auto-parse the bulk, override the parts that fail.

**CI3 (unsafe required):** C cannot provide Rask's safety guarantees. Making every C call unsafe forces explicit acknowledgment.

**XC3 (zig cc support):** Zig bundles cross-compilation toolchains for 90+ targets in a single binary. Rask delegates to whatever C toolchain is available — no bundled toolchains.

### Mixing Both Approaches

<!-- test: skip -->
```rask
import c "SDL2/SDL.h" as sdl hiding { SDL_FOURCC }

extern "C" {
    func SDL_FOURCC(a: u8, b: u8, c: u8, d: u8) -> u32
}
```

### Examples

**SQLite wrapper:**
<!-- test: skip -->
```rask
import c "sqlite3.h" as sql

public struct Database { handle: *sql.sqlite3 }

public func open(path: string) -> Database or Error {
    let db: *sql.sqlite3 = null
    unsafe {
        let rc = sql.sqlite3_open(path.as_c_str(), &db)
        if rc != sql.SQLITE_OK {
            return Err(Error.new("sqlite open failed"))
        }
    }
    Ok(Database { handle: db })
}
```

**Exporting to C:**
<!-- test: skip -->
```rask
public extern "C" func rask_process(data: *u8, len: c_size) -> c_int {
    unsafe {
        let slice = slice_from_raw(data, len)
        match process(slice) {
            Ok(_) => 0,
            Err(_) => -1,
        }
    }
}
```

### Non-Goals

- **No direct C++ import.** C++ name mangling, templates, exceptions don't cross FFI cleanly. Use `extern "C"` wrappers.
- **No libc independence.** Rask targets platforms where libc exists.
- **No bundled cross-compilation toolchains.** Use `zig cc`, system cross-compilers, or Docker.
- **No `rask cc` drop-in C compiler.** Maintenance cost doesn't justify it.

### See Also

- `mem.unsafe` — unsafe blocks, raw pointers
- `struct.build` — build scripts, `compile_c()`, `compile_rust()`
- `struct.modules` — import system
