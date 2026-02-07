# Compile-Time Execution

## The Question
What subset of Rask can execute at compile time? How is compile-time execution requested? What restrictions apply? How does it integrate with the type system and build process?

## Decision
Explicit `comptime` keyword marks compile-time evaluation. Restricted subset: pure computation, no I/O, no runtime-only features. Used for constants, generic specialization, conditional compilation. Separate from build scripts (separate programs before compilation).

## Rationale
Explicit marking: `comptime` clarifies when code runs at compile time vs runtime. Follows Zig's proven approach. I restrict to pure computation to keep the comptime interpreter simple and avoid full-language interpretation complexity (see Rust's limited `const fn`). Rask's runtime-heavy features (pools, linear resources, concurrency) don't make sense at compile time. Separating comptime (in-compiler) from build scripts (separate programs) gives flexibility without complexity.

## Specification

### The `comptime` Keyword

Explicitly marks code that must execute at compile time.

**Forms:**

| Usage | Syntax | Meaning |
|-------|--------|---------|
| Comptime variable | `comptime let x = expr` | Expression evaluated at compile time |
| Comptime constant | `const X = comptime expr` | Constant initialized at compile time |
| Comptime function | `comptime func name() -> T { ... }` | Function can only be called at compile time |
| Comptime parameter | `func f<comptime N: usize>() { ... }` | Generic parameter must be compile-time known |
| Comptime block | `comptime { ... }` | Block evaluated at compile time |

**Semantics:**
- Forces evaluation at compile time
- Expressions must be evaluable with only compile-time-known inputs
- Functions can only call other comptime functions or pure operations
- Using runtime values in comptime context is a compile error

### Comptime Constants

**Declaration:**
```rask
const LOOKUP_TABLE: [u8; 256] = comptime build_table()
const MAX_SIZE: usize = comptime calculate_max()
const VERSION_STRING: string = comptime format_version(MAJOR, MINOR, PATCH)
```

**Rules:**
- `const` declarations are implicitly comptime-evaluated
- Explicit `comptime` is optional but clarifies intent
- The initializer expression must be comptime-evaluable
- All dependencies must be comptime-known

**Error cases:**

| Case | Handling |
|------|----------|
| Runtime function in const | Compile error: "Cannot call runtime function in const initializer" |
| Non-comptime dependency | Compile error: "Value not known at compile time" |
| Comptime evaluation fails | Compile error with backtrace of comptime call stack |

### Comptime Functions

**Declaration:**
<!-- test: parse -->
```rask
comptime func factorial(n: u32) -> u32 {
    if n <= 1 {
        return 1
    }
    n * factorial(n - 1)
}

comptime func build_lookup_table() -> [u8; 256] {
    const table = [0u8; 256]
    for i in 0..256 {
        table[i] = (i * 2) as u8
    }
    table
}
```

**Restrictions:**
- Can only call other `comptime` functions
- Can only use comptime-allowed features (see Allowed Features section)
- Cannot perform I/O, allocate from heap pools, spawn tasks
- All inputs must be comptime-known
- Return value becomes comptime-known

**Use at runtime:**
- Comptime functions CANNOT be called at runtime
- If runtime use needed, write a separate runtime function
- Or make the function generic over comptime/runtime (see below)

### Generic Comptime Parameters

**Type parameters** use regular generics (types are inherently compile-time):
<!-- test: parse -->
```rask
func make_buffer<T>() -> T {
    T.default()
}
```

**Value parameters** use `comptime` modifier:
<!-- test: parse -->
```rask
func fixed_array<comptime N: usize>() -> [u8; N] {
    [0u8; N]
}
```

**Combined:**
<!-- test: parse -->
```rask
func repeat<comptime N: usize>(value: u8) -> [u8; N] {
    const arr = [0u8; N]
    for i in 0..N {
        arr[i] = value
    }
    arr
}

// Usage
const buf = repeat<16>(0xff)  // OK: 16 is comptime-known
const n = read_config()
const buf = repeat<n>(0xff)   // ❌ ERROR: n is runtime value
```

**Rules:**
- `comptime` generic parameters must be known at monomorphization time
- The type or value is substituted at compile time
- Enables array sizes, buffer capacities, algorithm selection based on comptime constants

### Comptime Blocks

**Conditional compilation:**
```rask
func process(data: []u8) {
    comptime {
        if FEATURE_LOGGING {
            // This code is conditionally included at compile time
        }
    }

    // Runtime code
    for byte in data {
        comptime if FEATURE_VALIDATION {
            validate(byte)  // Included only if FEATURE_VALIDATION is true
        }
        process_byte(byte)
    }
}
```

**Comptime variables in runtime context:**
```rask
func example() {
    comptime let iterations = if DEBUG_MODE { 100 } else { 10 }

    // Use comptime value in runtime loop
    for i in 0..iterations {  // Loop unrolled at compile time if small
        println(i)
    }
}
```rask

**Rules:**
- `comptime { ... }` executes at compile time, result affects compilation
- `comptime if` conditionally compiles code
- Comptime variables can be used in runtime code (their values are known)

### Comptime Collections with Freeze

Comptime supports standard collections (`Vec`, `Map`, `string`) with a compiler-managed allocator. Collections must be frozen to escape comptime as const data.

**How it works:**

1. **Compiler-managed allocator** — At comptime, collections use an internal scratch heap
   - Subject to existing 256MB limit
   - Deterministic: same source produces same result across all machines (fixed allocation strategy, not system allocator)

2. **Freeze to escape** — Collections call `.freeze()` to become const
   - `Vec<T>.freeze()` → `[T; N]` (size inferred from length)
   - `Map<K,V>.freeze()` → static map (perfect hash or similar)
   - `string.freeze()` → `str` (string literal)

3. **Cannot escape unfrozen** — Compile error if comptime returns unfrozen collection

**Examples:**

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
    m.insert("for", TokenKind.For)
    m.freeze()  // → perfect hash or static map
}

// string building
const GREETING: str = comptime {
    const s = string.new()
    s.push_str("Hello, ")
    s.push_str(USERNAME)
    s.push_str("!")
    s.freeze()  // → string literal
}
```

**Error case:**
```rask
const BAD = comptime {
    const v = Vec<u32>.new()
    v.push(1)
    v  // ❌ ERROR: cannot return unfrozen Vec from comptime
}
```rask

**Why freeze?**

Makes the boundary explicit. Compiler-managed scratch heap is bounded (256MB), deterministic (no allocator variance), frozen means immutable. Normal collection APIs work. `.freeze()` makes materialization explicit.

**Comparison with Zig:**

| Capability | Zig | Rask |
|------------|-----|------|
| Comptime allocation | Arena-based | Compiler-managed with freeze |
| Dynamic arrays | Implicit materialization | Explicit `.freeze()` |
| Comptime I/O | Full | `@embed_file` only |
| Build scripts | Separate | Separate (for complex codegen) |

### Allowed Features in Comptime

**Full support (works identically to runtime):**

| Feature | Comptime Support | Notes |
|---------|------------------|-------|
| Arithmetic operations | ✅ Full | `+`, `-`, `*`, `/`, `%`, bitwise, etc. |
| Comparison, logic | ✅ Full | `==`, `<`, `&&`, `||`, etc. |
| Control flow | ✅ Full | `if`, `match`, `while`, `for` |
| Function calls | ✅ Comptime only | Can only call other `comptime` functions |
| Structs | ✅ Full | Construction, field access, methods |
| Arrays | ✅ Full | Fixed-size arrays, indexing, iteration |
| Tuples | ✅ Full | Construction, destructuring |
| Enums | ✅ Full | Variant construction, pattern matching |
| Strings | ✅ Limited | Literals, concatenation (see below) |
| Type operations | ✅ Full | `sizeof`, `alignof`, type checks |
| File embedding | ✅ `@embed_file` | Read-only, compile-time path (see below) |

**Partial support (restricted):**

| Feature | Restriction | Rationale |
|---------|-------------|-----------|
| String operations | No heap allocation | Use fixed-size buffers or comptime-known sizes |
| Recursion | Depth limited | Prevent infinite loops at compile time |
| Loops | Iteration limit | Prevent infinite loops (configurable, default 10,000) |
| Stack depth | Limited | Prevent stack overflow in comptime interpreter |

**Not allowed (compile error if used):**

| Feature | Why Not Allowed |
|---------|-----------------|
| **General I/O** | Network, sockets, file writes require runtime (exception: `@embed_file`) |
| **Pools and handles** | Require runtime generation tracking |
| **Linear resources** | Files, sockets, cleanup tracking is runtime |
| **Tasks and channels** | Concurrency doesn't exist at compile time |
| **Ensure blocks** | Scope-based cleanup is runtime concept |
| **Unsafe blocks** | Raw pointers don't exist at compile time |

**Allowed with restrictions:**

| Feature | Restriction |
|---------|-------------|
| **Vec, Map, string** | Must call `.freeze()` to escape comptime (see Comptime Collections with Freeze) |

### string Handling at Comptime

**Allowed:**
```rask
comptime func make_greeting(name: string) -> string {
    // String literals are comptime-known
    const prefix = "Hello, "

    // Concatenation works if result size is comptime-known
    // This is a compiler intrinsic, not heap allocation
    concat(prefix, name, "!")
}

const GREETING = comptime make_greeting("World")  // "Hello, World!"
```

**Not allowed:**
```rask
comptime func read_file(path: string) -> string {
    // ❌ ERROR: I/O not allowed at comptime
    file.read(path)
}
```

**Implementation:**
- Comptime strings are stored in compiler memory, not runtime heap
- String operations are compiler intrinsics (concat, slice, etc.)
- Result must fit in comptime string buffer (e.g., 64KB limit)

### File Embedding at Comptime

**The `@embed_file` intrinsic:**

```rask
// Embed file contents as byte array
const SCHEMA: []u8 = comptime @embed_file("schema.json")

// Embed as string (file must be valid UTF-8)
const VERSION: string = comptime @embed_file("VERSION")

// Use in comptime computation
const CONFIG: Config = comptime parse_config(@embed_file("config.toml"))
```

**Constraints:**

| Constraint | Rationale |
|------------|-----------|
| Path MUST be a string literal | No runtime path injection |
| Path is relative to package root | Reproducible across machines |
| Read-only operation | No side effects |
| File read at compile time | Contents embedded in binary |
| File size limit (16 MB default) | Prevent memory issues |

**Error cases:**

| Case | Handling |
|------|----------|
| File not found | Compile error with path |
| File not readable | Compile error with OS error |
| Path is runtime value | Compile error: "Path must be string literal" |
| File too large | Compile error: "File exceeds embed limit" |
| Invalid UTF-8 (for string) | Compile error: "File is not valid UTF-8" |

**Why this is safe:**
- No arbitrary I/O—only reads files at known paths
- Deterministic—same source always embeds same content
- Sandboxed—cannot read outside package directory
- Auditable—embedded files listed in build output

**Use cases:**
- Embedding version strings, build info
- Bundling static assets (small icons, schemas)
- Including configuration templates
- Embedding test fixtures

For complex codegen (parsing schemas, calling external tools), use build scripts instead.

### Error Handling at Comptime

**Comptime functions can use Result:**
```rask
comptime func safe_divide(a: i32, b: i32) -> i32 or string {
    if b == 0 {
        return Err("Division by zero")
    }
    Ok(a / b)
}

const X = try comptime safe_divide(10, 2)  // OK: unwraps to 5
const Y = try comptime safe_divide(10, 0)  // ❌ Compile error: "Division by zero"
```

**Panic at comptime:**
```rask
comptime func get_value(i: usize) -> u8 {
    const table = [1u8, 2, 3]
    table[i]  // Panics if i >= 3
}

const A = comptime get_value(1)  // OK: 2
const B = comptime get_value(5)  // ❌ Compile error: "Index out of bounds: 5 >= 3"
```

**Rules:**
- Comptime panics become compile errors
- Error messages include comptime call stack
- `Result` and `try` work at comptime
- Errors propagate to compile error with context

### Debugging Comptime Code

Comptime errors occur during compilation. No traditional debugger (gdb/lldb). Errors can be far from source, call stacks confusing.

**Debugging tools:**

#### 1. Comptime Print

Output during compilation for debugging:

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
    table
}
```

**Build output:**
```bash
$ raskc --comptime-verbose main.rk
Compiling main.rk...
  [comptime] Building lookup table...
  [comptime] Progress: 0/256
  [comptime] Progress: 64/256
  [comptime] Progress: 128/256
  [comptime] Progress: 192/256
  [comptime] Done!
Done.

$ raskc main.rk  # Without flag: silent
```

**Rules:**
- `@comptime_print(fmt, args...)` only works in comptime context
- Output shown only with `--comptime-verbose` flag
- Prints to stderr (doesn't pollute build output)

#### 2. Comptime Assertions

Explicit checks with clear error messages:

```rask
comptime func safe_factorial(n: u32) -> u32 {
    @comptime_assert(n <= 20, "Factorial input too large: {} (max 20)", n)

    if n <= 1 { return 1 }
    n * safe_factorial(n - 1)
}

const F = comptime safe_factorial(25)
// ❌ Compile error: "Comptime assertion failed: Factorial input too large: 25 (max 20)"
```

**Usage:**
```rask
@comptime_assert(condition, message, args...)
```

Fails with formatted message if condition is false.

#### 3. Enhanced Error Messages

**Call stack collapsing for recursion:**

```rask
comptime func factorial(n: u32) -> u32 {
    if n <= 1 { return 1 }
    n * factorial(n - 1)
}

const F = comptime factorial(1000)
```

**Error output:**
```rask
error: Comptime evaluation exceeded backwards branch quota (1,000)

Comptime call stack:
  → factorial(1000) at math.rk:5:9
  → factorial(999)  at math.rk:5:9
    ... [repeated 996 more times]
  → factorial(0)    at math.rk:5:9

Triggered by:
  const F = comptime factorial(1000) at main.rk:10:11
           ^^^^^^^^^^^^^^^^^^^^^^^^^

note: Add @comptime_quota(N) to increase limit
note: Or rewrite using iteration instead of recursion
```

**For panics:**
```rask
error: Comptime panic: Division by zero

Comptime call stack:
  → divide(10, 0) at math.rk:3:9
    panic("Division by zero")
    ^^^^^^^^^^^^^^^^^^^^^^^^^

Triggered by:
  const X = comptime divide(10, 0) at main.rk:15:11
```

#### 4. Testing Pattern

Test comptime logic at runtime first:

```rask
// The comptime function
comptime func factorial(n: u32) -> u32 {
    if n <= 1 { return 1 }
    n * factorial(n - 1)
}

// Runtime tests (can use debugger!)
@test
func test_factorial() {
    // These run at runtime - full debugging available
    assert_eq(factorial(0), 1)
    assert_eq(factorial(1), 1)
    assert_eq(factorial(5), 120)
    assert_eq(factorial(10), 3628800)
}

// Once tests pass, use at comptime with confidence
const F5 = comptime factorial(5)
```

**Workflow:**
1. Write comptime function
2. Test at runtime with full debugging tools
3. Fix bugs using gdb/lldb/prints
4. Apply to comptime once working

#### 5. IDE Integration

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

### Comptime Limits

To prevent infinite compilation:

| Limit | Default | Override | Purpose |
|-------|---------|----------|---------|
| **Backwards branches** | 1,000 | `@comptime_quota(N)` | Prevent infinite loops/recursion |
| Execution time | 10 seconds | `--comptime-timeout=N` | Prevent build hangs |
| Memory per evaluation | 256 MB | `--comptime-max-memory=N` | Prevent OOM |
| String size | 1 MB | - | Prevent memory issues |
| Array size | 16 MB | - | Prevent memory issues |

**Backwards branches** (Zig-style): Counts loop iterations + recursive calls combined.

**Exceeding branch quota:**
```rask
comptime func slow() {
    let i = 0
    while i < 5000 {  // Exceeds default 1,000 backwards branches
        i += 1
    }
}

const X = comptime slow()
// ❌ Compile error: "Comptime evaluation exceeded backwards branch quota (1,000)"
//     Add @comptime_quota(N) to increase limit
```

**Override per-scope:**
```rask
comptime func large_computation() -> [u8; 10000] {
    @comptime_quota(20000)  // Allow 20,000 backwards branches

    const table = [0u8; 10000]
    for i in 0..10000 {
        table[i] = compute(i)
    }
    table
}
```

### Integration with Type System

**Comptime in generic constraints:**
```rask
func process<T, comptime N: usize>(items: [T; N])
where T: Copy {
    // N known at compile time, T substituted
    for i in 0..N {
        handle(items[i])
    }
}
```

**Comptime-dependent types:**
```rask
comptime func select_type(use_large: bool) -> type {
    if use_large {
        u64
    } else {
        u32
    }
}

const SIZE_TYPE = comptime select_type(LARGE_MODE)

struct Config {
    size: SIZE_TYPE  // Type selected at compile time
}
```

**Type-level computation:**
```rask
comptime func max(a: usize, b: usize) -> usize {
    if a > b { a } else { b }
}

func buffer<comptime A: usize, comptime B: usize>() -> [u8; comptime max(A, B)] {
    [0u8; comptime max(A, B)]
}
```

### Comptime vs Build Scripts

Two separate mechanisms:

| Aspect | Comptime | Build Scripts |
|--------|----------|---------------|
| **When runs** | During compilation | Before compilation |
| **Language** | Restricted Rask subset | Full Rask |
| **Purpose** | Constants, generic specialization, embedding | Build orchestration, codegen |
| **Can read files?** | ✅ `@embed_file` only | ✅ Yes (full I/O) |
| **Can write files?** | ❌ No | ✅ Yes |
| **Can call C code?** | ❌ No | ✅ Yes |
| **Can spawn tasks?** | ❌ No | ✅ Yes |
| **File** | In source files (comptime keyword) | `rask.build` |
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

### Choosing Between Comptime and Build Scripts

**Decision tree:**

```rask
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

**Examples:**

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

### Patterns for Dynamic-Size Results

**Preferred: Use Collections with Freeze**

For unknown-size results, use `Vec`, `Map`, or `string` with `.freeze()`. Simpler than the legacy two-pass pattern:

```rask
const PRIMES: [u32; _] = comptime {
    const v = Vec<u32>.new()
    for n in 2..100 {
        if is_prime(n) { v.push(n) }
    }
    v.freeze()
}
```

**Alternative: Two-Pass Computation**

When you want to avoid collections:

```rask
// Step 1: Count
comptime func count_primes(max: u32) -> usize {
    let count = 0
    for i in 2..max {
        if is_prime(i) { count += 1 }
    }
    count
}

// Step 2: Fill
comptime func fill_primes<comptime N: usize>(max: u32) -> [u32; N] {
    const result = [0u32; N]
    let idx = 0
    for i in 2..max {
        if is_prime(i) {
            result[idx] = i
            idx += 1
        }
    }
    result
}

const PRIME_COUNT: usize = comptime count_primes(100)
const PRIMES: [u32; PRIME_COUNT] = comptime fill_primes<PRIME_COUNT>(100)
```

**Build Script for Complex Codegen**

For codegen requiring external tools or extensive I/O:

```rask
// rask.build
@entry
func main() -> () or Error {
    const schema = try fs.read_file("schema.json")
    const code = generate_types_from_schema(schema)
    try fs.write_file("generated/types.rk", code)
    Ok(())
}
```

### Examples

#### Lookup Table Generation

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
    table
}

const CRC8_TABLE: [u8; 256] = comptime crc8_table()

func crc8(data: []u8) -> u8 {
    let crc = 0u8
    for byte in data {
        crc = CRC8_TABLE[(crc ^ byte) as usize]
    }
    crc
}
```

#### Generic Buffer Size

```rask
func read_packet<comptime MAX_SIZE: usize>(socket: Socket) -> [u8; MAX_SIZE] or Error {
    const buffer = [0u8; MAX_SIZE]
    const n = try socket.read(buffer[..])
    if n > MAX_SIZE {
        return Err(Error.new("Packet too large"))
    }
    Ok(buffer)
}

// Usage with different sizes
const small = try read_packet<64>(socket1)
const large = try read_packet<4096>(socket2)
```

#### Conditional Compilation

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

    Ok(())
}
```

#### Fibonacci at Compile Time

```rask
comptime func fib(n: u32) -> u32 {
    if n <= 1 {
        return n
    }
    fib(n - 1) + fib(n - 2)
}

const FIB_10: u32 = comptime fib(10)  // Computed at compile time: 55

func example() {
    // Comptime value used at runtime
    println("Fibonacci(10) = {}", FIB_10)
}
```

#### Type Selection

```rask
comptime func size_type(bits: usize) -> type {
    match bits {
        8 => u8,
        16 => u16,
        32 => u32,
        64 => u64,
        _ => panic("Invalid bit size"),
    }
}

struct Register<comptime BITS: usize> {
    value: comptime size_type(BITS)
}

const reg8 = Register<8> { value: 0u8 }
const reg32 = Register<32> { value: 0u32 }
```

### Edge Cases

| Case | Handling |
|------|----------|
| Comptime function calls runtime function | Compile error: "Cannot call runtime function from comptime" |
| Runtime value in comptime context | Compile error: "Value not known at compile time" |
| Infinite loop at comptime | Compile error after iteration limit: "Exceeded max iterations (10,000)" |
| Comptime panic | Compile error with message and call stack |
| Comptime I/O attempt | Compile error: "I/O not allowed at compile time" |
| Comptime pool creation | Compile error: "Pools not allowed at compile time" |
| Comptime task spawn | Compile error: "Concurrency not allowed at compile time" |
| Exceeding comptime memory limit | Compile error: "Comptime execution exceeded memory limit" |
| Comptime string concat (bounded) | Works via compiler intrinsic (up to size limit) |
| Comptime Result propagation | Works; error becomes compile error |
| Comptime array out of bounds | Compile error: "Index out of bounds" |
| Recursive comptime (within limit) | Works; memoized to avoid recomputation |
| Comptime type mismatch | Regular type error (type checking still applies) |

## Integration Notes

- **Type System:** Comptime enables type-level computation (selecting types, computing sizes). Generic parameters can be `comptime` to require compile-time-known values.
- **Memory Model:** Comptime has no runtime heap, pools, or handles. All data lives in compiler memory. Move/copy semantics still apply.
- **Error Handling:** `Result` and `try` work at comptime. Errors become compile errors with full context.
- **Generics:** `comptime` parameters enable array sizes, algorithm selection, conditional feature inclusion. Monomorphization sees comptime-known values as constants.
- **Compilation Model:** Comptime evaluation happens during type checking, before codegen. Results are constants embedded in binary. Comptime limits ensure bounded compilation time.
- **Build System:** Comptime orthogonal to build scripts. Comptime runs in-compiler (limited); build scripts run as separate programs (unlimited).
- **Module System:** Comptime constants can be `public` and exported. Can import comptime constants from other packages. Comptime functions callable across package boundaries.
- **Tooling Contract:** IDEs should show comptime values as ghost annotations. Comptime errors include clickable links to source locations in call stack.

## Remaining Issues

### High Priority
None identified.

### Medium Priority
1. **Step-through debugger** — Should IDEs support stepping through comptime interpreter? Complex but valuable for debugging.
2. **Comptime standard library** — Which stdlib functions should be `comptime` compatible? (e.g., string formatting, math)

### Low Priority
4. **Comptime imports** — Can comptime code import modules? Or only use built-in types?
5. **Comptime error recovery** — Should comptime support try/catch for better error messages? Or just panic → compile error?

### Resolved
5. ~~**Comptime memoization**~~ — Resolved. Pure comptime functions cached by `(function, arguments, body_hash)`. See [Semantic Hash Caching](../compiler/semantic-hash-caching.md).
6. ~~**Comptime heap allocation**~~ — Resolved via "Collections with Freeze" pattern. `Vec`, `Map`, `string` work at comptime with compiler-managed allocator; `.freeze()` materializes to const data.
