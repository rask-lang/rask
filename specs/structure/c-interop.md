<!-- id: struct.c-interop -->
<!-- status: decided -->
<!-- summary: Built-in C parser for headers plus explicit extern "C" bindings for edge cases -->
<!-- depends: memory/unsafe.md, structure/build.md -->

# C Interop

Two approaches: automatic header parsing (built-in C parser, like Zig) for well-behaved libraries, and explicit `extern "C"` bindings for edge cases. No libclang dependency.

## Import Mechanisms

| Rule | Description |
|------|-------------|
| **CI1: Auto-parse** | `import c "header.h"` translates the header to Rask declarations, the same text `rask bindc` writes (BI1), and compiles them as a package named `c`. Searched for beside the importing file, then as written, then along `CPATH`/`C_INCLUDE_PATH`, then along the C compiler's own system list (asked of `CC`, per XC3) |
| **CI2: Explicit binding** | `extern "C" { }` declares C functions/types manually |
| **CI3: Unsafe required** | All C function calls require `unsafe` context |
| **CI4: Override** | An explicit `extern "C" func` in the program replaces the auto-parsed one of the same name |
| **CI5: Hiding** | `import c "header.h" hiding { symbol }` suppresses specific auto-parsed symbols |

| Syntax | Effect |
|--------|--------|
| `import c "header.h"` | Parse header, expose as `c.symbol`. A quoted path is relative to the importing file, then the system include paths — same as `#include "…"` |
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

## Generated Bindings

`import c` is for a small header you use a few functions from. A library you
build on gets its binding written to a file, where it can be read, reviewed and
committed.

| Rule | Description |
|------|-------------|
| **BI1: One translation** | `rask bindc header.h -o sdl.rk` writes the declarations `import c "header.h"` would compile. Same translator, same output |
| **BI2: Never edited** | The generated file is a function of the header. Regenerate it; don't patch it |
| **BI3: Fix by override** | A declaration the translator got wrong is corrected by an explicit one in another file (CI4), which regeneration leaves alone |
| **BI4: Check** | `rask bindc --check header.h sdl.rk` fails when the file isn't what the header now translates to |
| **BI5: Skips are visible** | What can't be translated is written as a comment where the declaration would have been, carrying the C text. A macro that only renames (`#define SDL_mutex SDL_Mutex`) leaves no comment, since a library's old spellings run to hundreds; using one still says what it was |
| **BI6: Whose declarations** | The header's own directory is the library's: its declarations translate, and another file's only when one of them uses it (opaque when only pointed at). A header straight in a system include directory is the library's alone, since libc shares that directory |

A binding keeps C's names, so the library's own documentation still applies.
The Rask API on top of it, with `@resource` types, `T or E` and `Vec<u8>`
instead of pointer-and-length, is a separate module written by hand. That
layer is where taste lives, and it can't be generated: whether a function
reports failure by returning NULL, -1 or false is in the library's docs, not
its header.

<!-- test: skip -->
```rask
// sdl.rk: generated, never edited
extern "C" {
    func SDL_CreateWindow(title: *u8, w: c_int, h: c_int, flags: u64) -> *SDL_Window
    func SDL_DestroyWindow(window: *SDL_Window)
}

// window.rk: written by hand
@resource
public struct Window { raw: *SDL_Window }

extend Window {
    public func open(title: string, w: i32, h: i32) -> Window or SdlError {
        let t = try title.to_cstring()
        let raw = unsafe SDL_CreateWindow(t.as_ptr(), w, h, 0)
        if raw.is_null() {
            return sdl_error()
        }
        return Window(raw: raw)
    }

    public func close(take self) {
        unsafe SDL_DestroyWindow(self.raw)
    }
}
```

## Type Mapping

| Rule | Description |
|------|-------------|
| **TM1: Platform types** | `c_int`, `c_long`, etc. resolve to target platform sizes, not host |
| **TM2: Pointer mapping** | `T*` → `*T`, `void*` → `*void`, function pointers → `*func(...)` |
| **TM3: Composite types** | A C struct is a `@layout(C)` struct, a union a `union` (always C layout), an opaque struct `extern "C" struct Name` with no body |
| **TM4: Header structs are types** | A struct in an imported header is a type under the namespace — `c.Rect`. Built with a struct literal, fields read like any other struct's, C layout and offsets |
| **TM5: By value** | A struct parameter crosses the ABI as the platform passes one, not as a pointer. Returning a struct by value isn't implemented — the call is rejected |

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
| `struct S { ... }` | `@layout(C) struct S { ... }` |
| `struct S;` (no body) | `extern "C" struct S` |
| `union U { ... }` | `union U { ... }` ([mem.unsafe/UN1–UN3](../memory/unsafe.md#unions)) |
| `enum E { A, B }` | `type alias E = c_int` plus `const A: E = 0`, `const B: E = 1` |
| `int x : 3;` | `@bits(3) x: c_int` |
| Anonymous nested struct/union | A named type `Outer_anon0` and a field of it |
| Packed struct | `@packed` annotation |

| Rule | Description |
|------|-------------|
| **TM6: Enums are integers** | A C enum is its integer type, and its enumerators are constants. C stores any value of the type in it (flags, values from newer library versions), so a closed Rask enum would make an unlisted value undefined in a `match` |
| **TM7: Bitfields** | `@bits(n)` on an integer field of a `@layout(C)` struct packs it into the field's storage unit as the target's C ABI does: consecutive bitfields share a unit while they fit, low bits first. A read is shift and mask; a write panics if the value doesn't fit in `n` bits (a literal that doesn't fit is a compile error). A bitfield has no address |
| **TM8: Unions** | Size is the largest member's, alignment the strictest. Building one with a single member is safe and makes it the live one; reading any member is `unsafe`, since nothing records which is live. Members must be C-compatible (EX4), so nothing in a union owns memory |

A header's struct is named and built under its namespace:

<!-- test: skip -->
```rask
import c "mylib.h"          // typedef struct { int width; int height; } Rect;

func main() {
    let r = c.Rect(width: 6, height: 7)
    println("{r.width}")
    unsafe {
        println("{c.mylib_area(r)}")     // by value, in registers
    }
}
```

`c.Rect` is the spelling everywhere — in a signature, in a literal, in a type
argument. The bare `Rect` is not in scope: a header must not claim a name a Rask
declaration might want.

Handing a struct *to* C works; getting one *back* does not, and the call is
rejected rather than reading a value nobody wrote. Take it through an
out-parameter instead.

An out-parameter takes the address of a variable, field or element with `&`
([mem.unsafe](../memory/unsafe.md)). Taking it is safe; C writing through it
is what the `unsafe` call covers.

<!-- test: skip -->
```rask
mut w: c_int = 0
mut h: c_int = 0
unsafe c.SDL_GetWindowSize(window, &w, &h)
```

## String Interop

| Rule | Description |
|------|-------------|
| **ST1: to_cstring** | `s.to_cstring()` returns `cstring or NullByteError` — an owned, NUL-terminated copy. Refuses a string holding an interior `\0`, since C would read that as a shorter string |
| **ST2: ptr + len** | `.as_ptr()` + `.len()` for pointer+length APIs. Borrowed, not owned, and unchecked |
| **ST3: back again** | `cstring.decode()` returns `string or Utf8Error` — the bytes came from C, which promises nothing about encoding. A `cstring` isn't `Displayable`, so it has no `to_string()` |
| **ST4: Lifetime** | A `cstring` owns its buffer and frees it when it goes. A pointer from `.as_ptr()` owns nothing and is invalid once its source is dropped |

Full method list: [std.strings](../stdlib/strings.md#c-interop).

<!-- test: skip -->
```rask
func call_c_string_api(name: string) -> void or NullByteError {
    let c_name = try name.to_cstring()
    unsafe {
        c.puts(c_name.as_ptr())
        c.write(fd, name.as_ptr(), name.len())
    }
    return
}
```

## Preprocessor Handling

| Macro Type | Translation |
|------------|-------------|
| Integer constant (`#define FOO 42`) | `const FOO: c_int = 42` |
| Constant expression (`(1 << 3)`, `((Uint32)0x20)`, `A \| B`) | Evaluated: `const FOO: c_int = 8` |
| String constant (`#define V "1.0"`) | `const V: string = "1.0"` |
| Function-like (`#define MAX(a, b) …`) | Skipped, written as a comment (BI5) |
| Token pasting, stringification | Skipped, written as a comment |

| Rule | Description |
|------|-------------|
| **PP1: Constant expressions** | Integer literals, `+ - * / % << >> & \| ^ ~`, parentheses, casts to integer types, names of earlier constants and enumerators, and calls of function-like macros (expanded, with `##` pasting a name or a literal's suffix: `SDL_UINT64_C(0x20)` is `0x20 ## UL`) are evaluated at translation. The constant takes the cast's type, else `c_int`, widening to `c_long` or `c_ulong` when the value needs it |
| **PP2: No guessing** | A macro the evaluator can't reduce to one integer, float or string is skipped. Macros have no types; a guessed signature is worse than a comment |

## Inline Functions

A `static inline` function in a header has no symbol: the C compiler emits its
body only inside the C files that use it, so there's nothing to link to.

| Rule | Description |
|------|-------------|
| **IN1: Marked** | It translates to an `extern "C"` declaration carrying `@inline_from("header.h")` |
| **IN2: Compiler-built wrapper** | For each one the program calls, the compiler compiles a one-line C function that calls it, with the same C compiler and include paths as `import c` (XC3), and links that instead. Nothing is written into the project |
| **IN3: Cross builds** | A cross build with no C compiler for the target can't build the wrapper. The call is an error that says so |

<!-- test: skip -->
```rask
@inline_from("SDL3/SDL_rect.h")
extern "C" func SDL_RectEmpty(r: *SDL_Rect) -> bool
```

## Callbacks

A C function pointer is a bare code address. A Rask closure is code plus the
values it captured, so how one crosses depends on whether it captured anything.

| Rule | Description |
|------|-------------|
| **CB1: Named functions** | An `extern "C" func` with a body is a code address, and passes as a `*func(...)` directly |
| **CB2: Closures that capture nothing** | Compiled with the C calling convention, so they pass as `*func(...)` too, at no cost |
| **CB3: Closures that capture** | Need the C API's context pointer: the `void *` the registration function takes and hands back to the callback. The binding finds it (CB4) and records it as `@context(callback, userdata, at: i)`: parameter `userdata` is `callback`'s context, arriving as the callback's `i`th argument. At a call, `userdata` is left out and `callback` takes a closure without that argument. The compiler passes the closure as the context and a generated trampoline as the code |
| **CB4: Finding the context** | The callback type has a `void *` parameter, and the registration function has a `void *` parameter besides the function pointer. One of each is the pair. With several, the registration function's is picked by name (`userdata`, `user_data`, `ctx`, `context`, `arg`, `data`, `opaque`) and the callback's must still be the only `void *`. Anything else gets no `@context`, and only CB1/CB2 callbacks pass |
| **CB5: C holds it** | A capturing closure handed to C is C's to keep, and isn't freed. Register-once callbacks are what this is for. One registered every frame should capture nothing and reach its state through a `Shared` |
| **CB6: No context, no capture** | Passing a capturing closure where there's no context pointer is a compile error naming what it captured (E0936). A Rask function that isn't `extern "C"` is refused as a function pointer too (E0935): its panics would unwind through C's frames |

<!-- test: skip -->
```rask
// C: void SDL_SetAudioStreamGetCallback(SDL_AudioStream *stream,
//        void (*callback)(void *userdata, SDL_AudioStream *stream, int additional, int total),
//        void *userdata);
// binding:
@context(callback, userdata, at: 0)
extern "C" func SDL_SetAudioStreamGetCallback(stream: *SDL_AudioStream, callback: *func(*void, *SDL_AudioStream, c_int, c_int), userdata: *void)

unsafe c.SDL_SetAudioStreamGetCallback(stream, |s, additional, total| mixer.fill(s, additional))

// C: void qsort(void *base, size_t n, size_t size, int (*cmp)(const void *, const void *));
unsafe c.qsort(data.as_mut_ptr(), n, 4, |a, b| compare(a, b))    // captures nothing: CB2
```

## Variadic Functions

| Rule | Description |
|------|-------------|
| **VA1: Declared** | `extern "C" func printf(fmt: *u8, ...) -> c_int`. Only an extern can take `...`, and Rask can't define one |
| **VA2: What goes in** | After the fixed parameters, numbers, `bool` and raw pointers. Anything that owns memory or has a Rask layout is an error (E0937) |
| **VA3: C makes the call** | Each shape a program calls with goes through a C function with fixed parameters that makes the variadic call, compiled at link like the inline wrappers (IN2). C's default promotions and each target's variadic convention (`%al` on x86-64, the stack on Apple arm64) are the C compiler's |

<!-- test: skip -->
```rask
let fmt = "%d items, %.1f each\n".to_cstring()!
unsafe c.printf(fmt.as_ptr(), n, price)
```

## Exporting to C

| Rule | Description |
|------|-------------|
| **EX1: Export function** | `public extern "C" func name()` |
| **EX2: Export type** | `public extern "C" struct Name { ... }` |
| **EX3: Header generation** | `raskc --emit-header pkg` produces `pkg.h` |
| **EX4: C-compatible only** | Exported types must use C-compatible fields (primitives, pointers, extern structs) |

Not C-compatible: `string`, `Vec`, `Pool`, handles, closures, interface objects.

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
| **XC2: Re-parse per target** | `import c "header.h"` re-parses per target (struct layouts, `#ifdef` guards differ), reading the target compiler's system headers (XC3). A cross build with no compiler for the target reads none of the host's |
| **XC3: CC resolution** | `compile_c()`, the runtime build and `import c` all use one compiler: `CC` env var → `zig cc` → system compiler |

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
| Cross build, no target compiler | XC2 | System headers not searched; the error says why. Vendored headers beside the file still work |
| C++ header | CI1 | Error: "C++ not supported; use explicit bindings" |
| Variadic C function | VA1–VA3 | Callable from unsafe; Rask cannot define one |
| Header function the program never calls | CI1 | Not declared to the linker, so a header can describe more than the linked library has |
| Opaque struct | CI2 | Only pointer operations allowed |
| Inline function in header | IN1 | Imported; the compiler builds a wrapper to link against |
| Static (not inline) function in header | CI1 | Not imported (internal linkage) |
| Macro with token pasting | PP2 | Skipped, written as a comment |
| Mutable global | CI1 | Skipped, written as a comment |
| Name collides with a Rask keyword | BI1 | Suffixed with `_`: a parameter `type` becomes `type_` |

---

## Appendix (non-normative)

### Rationale

**CI1 + CI2 (dual approach):** I chose two approaches because neither is sufficient alone. Auto-parsing handles 90% of C headers; explicit bindings cover the rest. The real power is combining them — auto-parse the bulk, override the parts that fail.

**BI1 (one translator):** `import c` used to keep its own symbol table beside the
compiler's, and every gap in it was a gap only C imports had: constants that
never reached codegen, a struct inside a struct laid out as a pointer. Compiling
the generated text as an ordinary package means a header gets whatever Rask
declarations get, and `bindc` can't drift from `import c` because they are the
same output.

**TM6 (enums as integers):** I'd rather `SDL_INIT_VIDEO | SDL_INIT_AUDIO` just
work than have a closed enum that a newer libSDL can hand a value outside of.

**CB3 (closures through the context pointer):** Every C callback API worth
binding has the pointer, because C programmers need it for the same reason. The
binding reads it off the signatures, which is what a human writing the binding
would do.

**CI3 (unsafe required):** C cannot provide Rask's safety guarantees. Making every C call unsafe forces explicit acknowledgment.

**XC2 (no host fallback):** A header found in `/usr/include` for an `aarch64-macos` build is the wrong header, and a by-value C call reads its field offsets from that layout. A missing header is an error you can act on; a wrong one isn't visible at all.

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
    mut db: *sql.sqlite3 = nullptr
    unsafe {
        mut rc = sql.sqlite3_open(try path.to_cstring().as_ptr(), &db)
        if rc != sql.SQLITE_OK {
            return Error.new("sqlite open failed")
        }
    }
    return Database(handle: db)
}
```

**Exporting to C:**
<!-- test: skip -->
```rask
public extern "C" func rask_process(data: *u8, len: c_size) -> c_int {
    unsafe {
        mut slice = slice_from_raw(data, len)
        match process(slice) {
            Success as _ => 0,
            Error as _ => -1,
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
