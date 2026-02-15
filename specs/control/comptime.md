<!-- id: ctrl.comptime -->
<!-- status: decided -->
<!-- summary: Explicit comptime keyword for compile-time evaluation; restricted subset, no I/O -->
<!-- depends: types/generics.md -->
<!-- implemented-by: compiler/crates/rask-interp/ -->

# Compile-Time Execution

Explicit `comptime` keyword marks compile-time evaluation. Restricted subset: pure computation, no I/O, no runtime-only features. Used for constants, generic specialization, conditional compilation.

## Comptime Forms

| Rule | Form | Syntax | Meaning |
|------|------|--------|---------|
| **CT1: Comptime variable** | Variable | `comptime let x = expr` | Expression evaluated at compile time |
| **CT2: Comptime constant** | Constant | `const X = comptime expr` | Constant initialized at compile time |
| **CT3: Comptime function** | Function | `comptime func name() -> T { ... }` | Function can only be called at compile time |
| **CT4: Comptime parameter** | Parameter | `func f<comptime N: usize>() { ... }` | Generic parameter must be compile-time known |
| **CT5: Comptime block** | Block | `comptime { ... }` | Block evaluated at compile time |

<!-- test: parse -->
```rask
comptime func factorial(n: u32) -> u32 {
    if n <= 1 {
        return 1
    }
    return n * factorial(n - 1)
}

const LOOKUP_TABLE: [u8; 256] = comptime build_table()
const MAX_SIZE: usize = comptime calculate_max()

func fixed_array<comptime N: usize>() -> [u8; N] {
    return [0u8; N]
}

const buf = repeat<16>(0xff)  // OK: 16 is comptime-known
```

## Comptime Function Restrictions

| Rule | Description |
|------|-------------|
| **CT6: Comptime-only calls** | Comptime functions can only call other comptime functions |
| **CT7: No I/O** | Cannot perform I/O (exception: `@embed_file`), spawn tasks, allocate from runtime pools |
| **CT8: No runtime values** | All inputs must be comptime-known; using runtime values in comptime context is a compile error |

<!-- test: parse -->
```rask
comptime func build_lookup_table() -> [u8; 256] {
    const table = [0u8; 256]
    for i in 0..256 {
        table[i] = (i * 2) as u8
    }
    return table
}

func example() {
    const n = read_config()
    const buf = repeat<n>(0xff)   // ERROR: n is runtime value
}
```

## Return Semantics

| Rule | Description |
|------|-------------|
| **CT9: Function returns** | Comptime functions require explicit `return` (same as regular functions) |
| **CT10: Block values** | Comptime blocks use implicit last expression (expression context) |

<!-- test: parse -->
```rask
comptime func factorial(n: u32) -> u32 {
    if n <= 1 { return 1 }
    return n * factorial(n - 1)  // Explicit return required
}

const SQUARES = comptime {
    const arr = Vec.new()
    for i in 0..20 {
        arr.push(i * i)
    }
    arr  // Implicit - last expression is the value
}
```

## Conditional Compilation

The compiler provides a `cfg` constant for conditional compilation.

| Rule | Field | Type | Description |
|------|-------|------|-------------|
| **CT11: cfg.os** | `cfg.os` | `string` | Target OS: `"linux"`, `"macos"`, `"windows"` |
| **CT12: cfg.arch** | `cfg.arch` | `string` | Target architecture: `"x86_64"`, `"aarch64"`, `"riscv64"` |
| **CT13: cfg.env** | `cfg.env` | `string` | Target environment: `"gnu"`, `"musl"`, `"msvc"` |
| **CT14: cfg.profile** | `cfg.profile` | `string` | Build profile: `"debug"`, `"release"`, or custom |
| **CT15: cfg.debug** | `cfg.debug` | `bool` | Shorthand for `cfg.profile == "debug"` |
| **CT16: cfg.features** | `cfg.features` | `Set<string>` | Features enabled for this build |

<!-- test: skip -->
```rask
func get_backend() -> Backend {
    comptime if cfg.features.contains("ssl") {
        return SslBackend.new()
    } else {
        return PlainBackend.new()
    }
}

func default_path() -> string {
    comptime if cfg.os == "windows" {
        return "C:\\Users\\Default"
    } else {
        return "/home/default"
    }
}
```

## Collections with Freeze

Comptime supports collections (`Vec`, `Map`, `string`) with a compiler-managed allocator. Collections must be frozen to escape comptime as const data.

| Rule | Description |
|------|-------------|
| **CT17: Compiler allocator** | At comptime, collections use compiler-managed scratch heap (256MB limit) |
| **CT18: Freeze to escape** | Collections call `.freeze()` to become const: `Vec<T>` → `[T; N]`, `Map<K,V>` → static map, `string` → `str` |
| **CT19: Cannot escape unfrozen** | Compile error if comptime returns unfrozen collection |

<!-- test: skip -->
```rask
// Array generation - unknown size
const PRIMES: [u32; _] = comptime {
    const v = Vec<u32>.new()
    for n in 2..100 {
        if is_prime(n) { v.push(n) }
    }
    v.freeze()  // → [u32; 25]
}

// Map generation - lookup table
const KEYWORDS: Map<str, TokenKind> = comptime {
    const m = Map<str, TokenKind>.new()
    m.insert("if", TokenKind.If)
    m.insert("else", TokenKind.Else)
    m.freeze()  // → perfect hash or static map
}

const BAD = comptime {
    const v = Vec<u32>.new()
    v.push(1)
    v  // ERROR: cannot return unfrozen Vec from comptime
}
```

## Allowed Features

| Feature | Comptime Support | Rule |
|---------|------------------|------|
| **CT20: Arithmetic** | Arithmetic operations | ✅ Full: `+`, `-`, `*`, `/`, `%`, bitwise |
| **CT21: Logic** | Comparison, logic | ✅ Full: `==`, `<`, `&&`, `||` |
| **CT22: Control flow** | Control flow | ✅ Full: `if`, `match`, `while`, `for` |
| **CT23: Structs** | Structs | ✅ Full: construction, field access, methods |
| **CT24: Arrays** | Arrays | ✅ Full: fixed-size arrays, indexing, iteration |
| **CT25: Enums** | Enums | ✅ Full: variant construction, pattern matching |
| **CT26: Collections** | Vec, Map, string | ✅ With freeze: must call `.freeze()` to escape |

## Restricted Features

| Feature | Restriction | Rule |
|---------|-------------|------|
| **CT27: Recursion** | Depth limited | Prevent infinite loops (backwards branch quota) |
| **CT28: Loops** | Iteration limit | Default 1,000 backwards branches (configurable) |
| **CT29: Stack depth** | Limited | Prevent stack overflow in comptime interpreter |

## Forbidden Features

| Feature | Why Not Allowed | Rule |
|---------|-----------------|------|
| **CT30: General I/O** | Network, file writes | Require runtime (exception: `@embed_file`) |
| **CT31: Pools** | Pools and handles | Require runtime generation tracking |
| **CT32: Linear resources** | Files, sockets | Cleanup tracking is runtime concept |
| **CT33: Concurrency** | Tasks and channels | Concurrency doesn't exist at compile time |
| **CT34: Unsafe** | Unsafe blocks | Raw pointers don't exist at compile time |

## Comptime Limits

| Limit | Default | Override | Rule |
|-------|---------|----------|------|
| **CT35: Backwards branches** | 1,000 | `@comptime_quota(N)` | Prevent infinite loops/recursion |
| **CT36: Execution time** | 10 seconds | `--comptime-timeout=N` | Prevent build hangs |
| **CT37: Memory** | 256 MB | `--comptime-max-memory=N` | Prevent OOM |
| **CT38: String size** | 1 MB | - | Prevent memory issues |
| **CT39: Array size** | 16 MB | - | Prevent memory issues |

<!-- test: skip -->
```rask
comptime func slow() {
    let i = 0
    while i < 5000 {  // Exceeds default 1,000 backwards branches
        i += 1
    }
}

const X = comptime slow()
// ERROR: Comptime evaluation exceeded backwards branch quota (1,000)

comptime func large_computation() -> [u8; 10000] {
    @comptime_quota(20000)  // Allow 20,000 backwards branches

    const table = [0u8; 10000]
    for i in 0..10000 {
        table[i] = compute(i)
    }
    return table
}
```

## File Embedding

| Rule | Description |
|------|-------------|
| **CT40: Literal path** | Path MUST be a string literal; no runtime path injection |
| **CT41: Relative to package root** | Path is relative to package root for reproducibility |
| **CT42: Read-only** | Read-only operation; no side effects |
| **CT43: Compile-time read** | File read at compile time; contents embedded in binary |
| **CT44: Size limit** | File size limit (16 MB default) prevents memory issues |

<!-- test: skip -->
```rask
// Embed file contents as byte array
const SCHEMA: []u8 = comptime @embed_file("schema.json")

// Embed as string (file must be valid UTF-8)
const VERSION: string = comptime @embed_file("VERSION")

// Use in comptime computation
const CONFIG: Config = comptime parse_config(@embed_file("config.toml"))
```

## Error Handling

| Rule | Description |
|------|-------------|
| **CT45: Result support** | Comptime functions can use `Result` and `try` |
| **CT46: Panics as compile errors** | Comptime panics become compile errors with call stack |
| **CT47: Error propagation** | Errors propagate to compile error with context |

<!-- test: skip -->
```rask
comptime func safe_divide(a: i32, b: i32) -> i32 or string {
    if b == 0 {
        return Err("Division by zero")
    }
    return Ok(a / b)
}

const X = try comptime safe_divide(10, 2)  // OK: unwraps to 5
const Y = try comptime safe_divide(10, 0)  // Compile error: "Division by zero"

comptime func get_value(i: usize) -> u8 {
    const table = [1u8, 2, 3]
    return table[i]  // Panics if i >= 3
}

const A = comptime get_value(1)  // OK: 2
const B = comptime get_value(5)  // Compile error: "Index out of bounds: 5 >= 3"
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Comptime function calls runtime function | CT6 | Compile error: "Cannot call runtime function from comptime" |
| Runtime value in comptime context | CT8 | Compile error: "Value not known at compile time" |
| Infinite loop at comptime | CT35 | Compile error after iteration limit: "Exceeded max iterations (1,000)" |
| Comptime panic | CT46 | Compile error with message and call stack |
| Comptime I/O attempt | CT7 | Compile error: "I/O not allowed at compile time" |
| Comptime pool creation | CT31 | Compile error: "Pools not allowed at compile time" |
| Comptime task spawn | CT33 | Compile error: "Concurrency not allowed at compile time" |
| Exceeding comptime memory limit | CT37 | Compile error: "Comptime execution exceeded memory limit" |
| Comptime string concat (bounded) | CT20 | Works via compiler intrinsic (up to size limit) |
| Comptime Result propagation | CT45 | Works; error becomes compile error |
| Comptime array out of bounds | CT46 | Compile error: "Index out of bounds" |
| Recursive comptime (within limit) | CT35 | Works; memoized to avoid recomputation |
| Comptime type mismatch | - | Regular type error (type checking still applies) |
| Unfrozen collection escape | CT19 | Compile error: "cannot return unfrozen Vec from comptime" |

## Error Messages

**Exceeding branch quota [CT35]:**
```
ERROR [ctrl.comptime/CT35]: Comptime evaluation exceeded backwards branch quota (1,000)

Comptime call stack:
  → factorial(1000) at math.rk:5:9
  → factorial(999)  at math.rk:5:9
    ... [repeated 996 more times]
  → factorial(0)    at math.rk:5:9

Triggered by:
  const F = comptime factorial(1000) at main.rk:10:11
           ^^^^^^^^^^^^^^^^^^^^^^^^^

WHY: Backwards branch quota prevents infinite loops and unbounded recursion.

FIX: Add @comptime_quota(N) to increase limit, or rewrite using iteration:

  comptime func factorial(n: u32) -> u32 {
      @comptime_quota(2000)
      // ... or use iterative approach
  }
```

**Runtime value in comptime context [CT8]:**
```
ERROR [ctrl.comptime/CT8]: Value not known at compile time
   |
5  |  const n = read_config()
6  |  const buf = repeat<n>(0xff)
   |                     ^ runtime value cannot be used as comptime parameter

WHY: Comptime parameters must be compile-time known constants.

FIX: Use a compile-time constant instead:

  const buf = repeat<16>(0xff)  // OK: literal is comptime-known
```

**Comptime panic [CT46]:**
```
ERROR [ctrl.comptime/CT46]: Comptime panic: Division by zero

Comptime call stack:
  → divide(10, 0) at math.rk:3:9
    panic("Division by zero")
    ^^^^^^^^^^^^^^^^^^^^^^^^^

Triggered by:
  const X = comptime divide(10, 0) at main.rk:15:11

WHY: Comptime panics become compile errors to prevent invalid constants.

FIX: Fix the comptime logic or use a valid input value.
```

**Unfrozen collection escape [CT19]:**
```
ERROR [ctrl.comptime/CT19]: cannot return unfrozen Vec from comptime
   |
3  |  const BAD = comptime {
4  |      const v = Vec<u32>.new()
5  |      v.push(1)
6  |      v  // cannot escape unfrozen
   |      ^ collection must be frozen with .freeze()

WHY: Comptime collections use compiler-managed memory and must be
     materialized as const data to be embedded in the binary.

FIX: Call .freeze() to convert to const data:

  const GOOD = comptime {
      const v = Vec<u32>.new()
      v.push(1)
      v.freeze()  // → [1u32; 1]
  }
```

## Examples

### Lookup Table Generation
<!-- test: skip -->
```rask
comptime func crc8_table() -> [u8; 256] {
    const table = [0u8; 256]
    for i in 0..256 {
        let crc = i as u8
        for _ in 0..8 {
            if crc & 0x80 != 0 {
                crc = (crc << 1) ^ 0x07
            } else {
                crc = crc << 1
            }
        }
        table[i] = crc
    }
    return table
}

const CRC8_TABLE: [u8; 256] = comptime crc8_table()

func crc8(data: []u8) -> u8 {
    let crc = 0u8
    for byte in data {
        crc = CRC8_TABLE[(crc ^ byte) as usize]
    }
    return crc
}
```

### Generic Buffer Size
<!-- test: skip -->
```rask
func read_packet<comptime MAX_SIZE: usize>(socket: Socket) -> [u8; MAX_SIZE] or Error {
    const buffer = [0u8; MAX_SIZE]
    const n = try socket.read(buffer[..])
    if n > MAX_SIZE {
        return Err(Error.new("Packet too large"))
    }
    return Ok(buffer)
}

// Usage with different sizes
const small = try read_packet<64>(socket1)
const large = try read_packet<4096>(socket2)
```

### Conditional Compilation
<!-- test: skip -->
```rask
const DEBUG_MODE: bool = comptime cfg.debug
const LOGGING_ENABLED: bool = comptime cfg.features.contains("logging")

func process(data: []u8) -> () or Error {
    comptime if LOGGING_ENABLED {
        log.debug("Processing {} bytes", data.len)
    }

    for byte in data {
        comptime if DEBUG_MODE {
            // Validation only in debug builds
            if byte > 127 {
                return Err(Error.new("Invalid byte"))
            }
        }

        try handle(byte)
    }
}
```

---

## Appendix (non-normative)

### Rationale

**CT1-CT5 (Explicit comptime):** I chose explicit `comptime` marking to clarify when code runs at compile time vs runtime. Follows Zig's proven approach. Makes the boundary between compile-time and runtime visible in the code.

**CT6-CT8 (Restrictions):** I restrict to pure computation to keep the comptime interpreter simple and avoid full-language interpretation complexity (see Rust's limited `const fn`). Rask's runtime-heavy features (pools, linear resources, concurrency) don't make sense at compile time.

**CT17-CT19 (Freeze pattern):** Makes the boundary explicit. Compiler-managed scratch heap is bounded (256MB), deterministic (no allocator variance). Normal collection APIs work at comptime. `.freeze()` makes materialization into const data explicit.

**CT35-CT39 (Limits):** Prevent infinite compilation. Backwards branches (Zig-style) count loop iterations + recursive calls combined. Keeps build times predictable.

**CT40-CT44 (@embed_file):** Safe subset of I/O — no arbitrary I/O, only reads files at known paths. Deterministic, sandboxed, auditable. For complex codegen (parsing schemas, calling external tools), use build scripts instead.

**Separating comptime from build scripts:** Comptime (in-compiler, limited) vs build scripts (separate programs, unlimited) gives flexibility without complexity. Comptime for constants and specialization; build scripts for codegen and orchestration.

### Patterns & Guidance

**When to use comptime vs build scripts:**

```
Need to transform/process files (not just embed)?
  YES → Build script
  NO  → Need network or environment?
          YES → Build script
          NO  → Just embedding file contents?
                  YES → Comptime (@embed_file)
                  NO  → Result fits in 256MB comptime limit?
                          YES → Comptime (use collections with freeze)
                          NO  → Build script
```

| Task | Approach | Why |
|------|----------|-----|
| CRC lookup table (256 entries) | Comptime | Size known, fixed array |
| Primes up to N | Comptime (Vec + freeze) | Unknown size, use collection |
| Keyword lookup map | Comptime (Map + freeze) | Build map, freeze to static |
| Embed version string | Comptime (`@embed_file`) | Simple file read |
| Embed small config file | Comptime (`@embed_file`) | No transform needed |
| Parse embedded JSON | Comptime (collections) | `@embed_file` + parse + freeze |
| Types from JSON schema | Build script | Needs to generate source files |
| Protobuf codegen | Build script | Needs external tool |

**Dynamic-size results:**

Use collections with freeze:
```rask
const PRIMES: [u32; _] = comptime {
    const v = Vec<u32>.new()
    for n in 2..100 {
        if is_prime(n) { v.push(n) }
    }
    v.freeze()
}
```

Alternative two-pass pattern (when avoiding collections):
```rask
// Step 1: Count
comptime func count_primes(max: u32) -> usize { ... }

// Step 2: Fill
comptime func fill_primes<comptime N: usize>(max: u32) -> [u32; N] { ... }

const PRIME_COUNT: usize = comptime count_primes(100)
const PRIMES: [u32; PRIME_COUNT] = comptime fill_primes<PRIME_COUNT>(100)
```

### Debugging Tools

**1. Comptime Print**

Output during compilation:
```rask
comptime func build_table() -> [u8; 256] {
    @comptime_print("Building lookup table...")
    const table = [0u8; 256]
    for i in 0..256 {
        table[i] = compute(i)
        if i % 64 == 0 {
            @comptime_print("Progress: {}/256", i)
        }
    }
    @comptime_print("Done!")
    return table
}
```

Build output with `--comptime-verbose`:
```bash
$ raskc --comptime-verbose main.rk
Compiling main.rk...
  [comptime] Building lookup table...
  [comptime] Progress: 0/256
  [comptime] Progress: 64/256
  [comptime] Done!
```

**2. Comptime Assertions**

Explicit checks with clear error messages:
```rask
comptime func safe_factorial(n: u32) -> u32 {
    @comptime_assert(n <= 20, "Factorial input too large: {} (max 20)", n)
    if n <= 1 { return 1 }
    return n * safe_factorial(n - 1)
}
```

**3. Testing Pattern**

Test comptime logic at runtime first:
```rask
// The comptime function
comptime func factorial(n: u32) -> u32 {
    if n <= 1 { return 1 }
    return n * factorial(n - 1)
}

// Runtime tests (can use debugger!)
@test
func test_factorial() {
    assert_eq(factorial(0), 1)
    assert_eq(factorial(5), 120)
    assert_eq(factorial(10), 3628800)
}

// Once tests pass, use at comptime
const F5 = comptime factorial(5)
```

Workflow: write comptime function → test at runtime with full debugging tools → fix bugs using gdb/lldb/prints → apply to comptime once working.

### IDE Integration

IDEs should provide:

- **Hover for comptime values:**
  ```rask
  const F10 = comptime factorial(10)
          ^^^^^^^^^^^^^^^^^^^^^^ // IDE shows: 3628800
  ```

- **Navigate comptime errors:** Click error → jump to source, clickable call stack

- **Inline results:**
  ```rask
  const TABLE = comptime build_table()
                ^^^^^^^^^^^^^^^^^^^^^^ // Ghost: [0, 2, 4, 6, ...]
  ```

- **On-demand evaluation:** Right-click → "Evaluate at comptime"

### Comptime vs Build Scripts

| Aspect | Comptime | Build Scripts |
|--------|----------|---------------|
| **When runs** | During compilation | Before compilation |
| **Language** | Restricted Rask subset | Full Rask |
| **Purpose** | Constants, generic specialization, embedding | Build orchestration, codegen |
| **Can read files?** | ✅ `@embed_file` only | ✅ Yes (full I/O) |
| **Can write files?** | ❌ No | ✅ Yes |
| **Can call C code?** | ❌ No | ✅ Yes |
| **Can spawn tasks?** | ❌ No | ✅ Yes |
| **File** | In source files (comptime keyword) | `build.rk` |
| **Executed by** | Compiler's comptime interpreter | Separate compiled program |

**Use comptime for:**
- Compile-time constants
- Array sizes, buffer capacities
- Generic specialization
- Conditional compilation
- Type-level computation
- Embedding files (`@embed_file`)

**Use build scripts for:**
- Code generation from schemas (protobuf, etc.)
- Asset bundling
- Calling external build tools (C compiler, etc.)
- Complex dependency resolution logic
- Pre-build validation

### Comparison with Zig

| Capability | Zig | Rask |
|------------|-----|------|
| Comptime allocation | Arena-based | Compiler-managed with freeze |
| Dynamic arrays | Implicit materialization | Explicit `.freeze()` |
| Comptime I/O | Full | `@embed_file` only |
| Build scripts | Separate | Separate (for complex codegen) |

### See Also

- [Generics](../types/generics.md) — Generic parameters and specialization (`type.generics`)
- [Build System](../structure/build.md) — Build scripts and package configuration (`struct.build`)
- [Error Types](../types/error-types.md) — Result and error handling (`type.errors`)
