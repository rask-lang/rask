# Compile-Time Execution

## The Question
What subset of Rask can execute at compile time? How is compile-time execution requested? What restrictions apply? How does it integrate with the type system and build process?

## Decision
Explicit `comptime` keyword marks compile-time evaluation. Restricted subset of Rask (pure computation, no I/O, no runtime-only features). Used for constants, generic specialization, conditional compilation. Separate from build scripts (which run as separate programs before compilation).

## Rationale
Explicit marking (`comptime`) makes it clear when code runs at compile time vs runtime, following Zig's proven approach. Restricting to a pure computational subset keeps the comptime interpreter simple and avoids the complexity nightmare of full-language interpretation (as seen with Rust's limited `const fn`). Rask has runtime-heavy features (pools, linear resources, concurrency) that don't make sense at compile time—limiting comptime to pure computation is pragmatic. Separating comptime (in-compiler evaluation) from build scripts (separate programs) provides flexibility without complexity.

## Specification

### The `comptime` Keyword

**Purpose:** Explicitly marks code that must execute at compile time.

**Forms:**

| Usage | Syntax | Meaning |
|-------|--------|---------|
| Comptime variable | `comptime let x = expr` | Expression evaluated at compile time |
| Comptime constant | `const X = comptime expr` | Constant initialized at compile time |
| Comptime function | `comptime fn name() -> T { ... }` | Function can only be called at compile time |
| Comptime parameter | `fn f<comptime N: usize>() { ... }` | Generic parameter must be compile-time known |
| Comptime block | `comptime { ... }` | Block evaluated at compile time |

**Semantics:**
- `comptime` forces evaluation at compile time
- Comptime expressions MUST be evaluable with only compile-time-known inputs
- Comptime functions can ONLY call other comptime functions or pure operations
- Attempting to use runtime values in comptime context is a compile error

### Comptime Constants

**Declaration:**
```rask
const LOOKUP_TABLE: [u8; 256] = comptime build_table()
const MAX_SIZE: usize = comptime calculate_max()
const VERSION_STRING: String = comptime format_version(MAJOR, MINOR, PATCH)
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
```rask
comptime fn factorial(n: u32) -> u32 {
    if n <= 1 {
        return 1
    }
    n * factorial(n - 1)
}

comptime fn build_lookup_table() -> [u8; 256] {
    let mut table = [0u8; 256]
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

**Type parameters:**
```rask
fn make_buffer<comptime T: type>() -> T {
    T::default()
}

fn fixed_array<comptime N: usize>() -> [u8; N] {
    [0u8; N]
}
```

**Value parameters:**
```rask
fn repeat<comptime N: usize>(value: u8) -> [u8; N] {
    let mut arr = [0u8; N]
    for i in 0..N {
        arr[i] = value
    }
    arr
}

// Usage
let buf = repeat<16>(0xff)  // OK: 16 is comptime-known
let n = read_config()
let buf = repeat<n>(0xff)   // ❌ ERROR: n is runtime value
```

**Rules:**
- `comptime` generic parameters must be known at monomorphization time
- The type or value is substituted at compile time
- Enables array sizes, buffer capacities, algorithm selection based on comptime constants

### Comptime Blocks

**Conditional compilation:**
```rask
fn process(data: []u8) {
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
fn example() {
    comptime let iterations = if DEBUG_MODE { 100 } else { 10 }

    // Use comptime value in runtime loop
    for i in 0..iterations {  // Loop unrolled at compile time if small
        println(i)
    }
}
```

**Rules:**
- `comptime { ... }` executes at compile time, result affects compilation
- `comptime if` conditionally compiles code
- Comptime variables can be used in runtime code (their values are known)

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
| **I/O operations** | Network, files, sockets require runtime |
| **Pools and handles** | Require runtime generation tracking |
| **Linear resources** | Files, sockets, cleanup tracking is runtime |
| **Tasks and channels** | Concurrency doesn't exist at compile time |
| **Ensure blocks** | Scope-based cleanup is runtime concept |
| **Heap allocation** | No dynamic memory at compile time |
| **Vec, Map (unbounded)** | Dynamic growth requires heap allocation |
| **Unsafe blocks** | Raw pointers don't exist at compile time |

### String Handling at Comptime

**Allowed:**
```rask
comptime fn make_greeting(name: String) -> String {
    // String literals are comptime-known
    let prefix = "Hello, "

    // Concatenation works if result size is comptime-known
    // This is a compiler intrinsic, not heap allocation
    concat(prefix, name, "!")
}

const GREETING = comptime make_greeting("World")  // "Hello, World!"
```

**Not allowed:**
```rask
comptime fn read_file(path: String) -> String {
    // ❌ ERROR: I/O not allowed at comptime
    file.read(path)
}
```

**Implementation:**
- Comptime strings are stored in compiler memory, not runtime heap
- String operations are compiler intrinsics (concat, slice, etc.)
- Result must fit in comptime string buffer (e.g., 64KB limit)

### Error Handling at Comptime

**Comptime functions can use Result:**
```rask
comptime fn safe_divide(a: i32, b: i32) -> Result<i32, String> {
    if b == 0 {
        return Err("Division by zero")
    }
    Ok(a / b)
}

const X = comptime safe_divide(10, 2)?  // OK: unwraps to 5
const Y = comptime safe_divide(10, 0)?  // ❌ Compile error: "Division by zero"
```

**Panic at comptime:**
```rask
comptime fn get_value(i: usize) -> u8 {
    let table = [1u8, 2, 3]
    table[i]  // Panics if i >= 3
}

const A = comptime get_value(1)  // OK: 2
const B = comptime get_value(5)  // ❌ Compile error: "Index out of bounds: 5 >= 3"
```

**Rules:**
- Comptime panics become compile errors
- Error messages include comptime call stack
- `Result` and `?` work at comptime
- Errors propagate to compile error with context

### Debugging Comptime Code

**The challenge:** Comptime errors occur during compilation, not runtime. No traditional debugger (gdb/lldb), errors happen far from source, call stacks can be confusing.

**Debugging tools:**

#### 1. Comptime Print

Output during compilation for debugging:

```rask
comptime fn build_table() -> [u8; 256] {
    @comptime_print("Building lookup table...")

    let mut table = [0u8; 256]
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
$ raskc --comptime-verbose main.rask
Compiling main.rask...
  [comptime] Building lookup table...
  [comptime] Progress: 0/256
  [comptime] Progress: 64/256
  [comptime] Progress: 128/256
  [comptime] Progress: 192/256
  [comptime] Done!
Done.

$ raskc main.rask  # Without flag: silent
```

**Rules:**
- `@comptime_print(fmt, args...)` only works in comptime context
- Output shown only with `--comptime-verbose` flag
- Prints to stderr (doesn't pollute build output)

#### 2. Comptime Assertions

Explicit checks with clear error messages:

```rask
comptime fn safe_factorial(n: u32) -> u32 {
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
comptime fn factorial(n: u32) -> u32 {
    if n <= 1 { return 1 }
    n * factorial(n - 1)
}

const F = comptime factorial(1000)
```

**Error output:**
```
error: Comptime evaluation exceeded backwards branch quota (1,000)

Comptime call stack:
  → factorial(1000) at math.rask:5:9
  → factorial(999)  at math.rask:5:9
    ... [repeated 996 more times]
  → factorial(0)    at math.rask:5:9

Triggered by:
  const F = comptime factorial(1000) at main.rask:10:11
           ^^^^^^^^^^^^^^^^^^^^^^^^^

note: Add @comptime_quota(N) to increase limit
note: Or rewrite using iteration instead of recursion
```

**For panics:**
```
error: Comptime panic: Division by zero

Comptime call stack:
  → divide(10, 0) at math.rask:3:9
    panic("Division by zero")
    ^^^^^^^^^^^^^^^^^^^^^^^^^

Triggered by:
  const X = comptime divide(10, 0) at main.rask:15:11
```

#### 4. Testing Pattern

**Test comptime logic at runtime first:**

```rask
// The comptime function
comptime fn factorial(n: u32) -> u32 {
    if n <= 1 { return 1 }
    n * factorial(n - 1)
}

// Runtime tests (can use debugger!)
#[test]
fn test_factorial() {
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

**IDEs SHOULD provide:**

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

**To prevent infinite compilation:**

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
comptime fn slow() {
    let mut i = 0
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
comptime fn large_computation() -> [u8; 10000] {
    @comptime_quota(20000)  // Allow 20,000 backwards branches

    let mut table = [0u8; 10000]
    for i in 0..10000 {
        table[i] = compute(i)
    }
    table
}
```

### Integration with Type System

**Comptime in generic bounds:**
```rask
fn process<T, comptime N: usize>(items: [T; N])
where T: Copy {
    // N is known at compile time, T is substituted
    for i in 0..N {
        handle(items[i])
    }
}
```

**Comptime-dependent types:**
```rask
comptime fn select_type(use_large: bool) -> type {
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
comptime fn max(a: usize, b: usize) -> usize {
    if a > b { a } else { b }
}

fn buffer<comptime A: usize, comptime B: usize>() -> [u8; comptime max(A, B)] {
    [0u8; comptime max(A, B)]
}
```

### Comptime vs Build Scripts

**Two separate mechanisms:**

| Aspect | Comptime | Build Scripts |
|--------|----------|---------------|
| **When runs** | During compilation | Before compilation |
| **Language** | Restricted Rask subset | Full Rask |
| **Purpose** | Constants, generic specialization | Build orchestration, codegen |
| **Can do I/O?** | ❌ No | ✅ Yes |
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

**Use build scripts for:**
- Code generation from schemas (protobuf, etc.)
- Asset bundling
- Calling external build tools (C compiler, etc.)
- Complex dependency resolution logic
- Pre-build validation

### Examples

#### Lookup Table Generation

```rask
comptime fn crc8_table() -> [u8; 256] {
    let mut table = [0u8; 256]
    for i in 0..256 {
        let mut crc = i as u8
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

fn crc8(data: []u8) -> u8 {
    let mut crc = 0u8
    for byte in data {
        crc = CRC8_TABLE[(crc ^ byte) as usize]
    }
    crc
}
```

#### Generic Buffer Size

```rask
fn read_packet<comptime MAX_SIZE: usize>(socket: Socket) -> Result<[u8; MAX_SIZE], Error> {
    let mut buffer = [0u8; MAX_SIZE]
    let n = socket.read(&mut buffer[..])?
    if n > MAX_SIZE {
        return Err(Error::new("Packet too large"))
    }
    Ok(buffer)
}

// Usage with different sizes
let small = read_packet<64>(socket1)?
let large = read_packet<4096>(socket2)?
```

#### Conditional Compilation

```rask
const DEBUG_MODE: bool = comptime cfg.debug
const LOGGING_ENABLED: bool = comptime cfg.features.contains("logging")

fn process(data: []u8) -> Result<(), Error> {
    comptime if LOGGING_ENABLED {
        log.debug("Processing {} bytes", data.len)
    }

    for byte in data {
        comptime if DEBUG_MODE {
            // Validation only in debug builds
            if byte > 127 {
                return Err(Error::new("Invalid byte"))
            }
        }

        handle(byte)?
    }

    Ok(())
}
```

#### Fibonacci at Compile Time

```rask
comptime fn fib(n: u32) -> u32 {
    if n <= 1 {
        return n
    }
    fib(n - 1) + fib(n - 2)
}

const FIB_10: u32 = comptime fib(10)  // Computed at compile time: 55

fn example() {
    // Comptime value used at runtime
    println("Fibonacci(10) = {}", FIB_10)
}
```

#### Type Selection

```rask
comptime fn size_type(bits: usize) -> type {
    match bits {
        8 => u8,
        16 => u16,
        32 => u32,
        64 => u64,
        else => panic("Invalid bit size"),
    }
}

struct Register<comptime BITS: usize> {
    value: comptime size_type(BITS)
}

let reg8 = Register<8> { value: 0u8 }
let reg32 = Register<32> { value: 0u32 }
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
- **Memory Model:** Comptime has no runtime heap, no pools, no handles. All data lives in compiler memory. Move/copy semantics still apply (comptime functions consume/borrow their inputs).
- **Error Handling:** `Result` and `?` work at comptime. Errors become compile errors with full context. Panics also become compile errors.
- **Generics:** `comptime` parameters enable array sizes, algorithm selection, conditional feature inclusion. Monomorphization sees comptime-known values as constants.
- **Compilation Model:** Comptime evaluation happens during type checking, before codegen. Results are constants embedded in the final binary. Comptime limits ensure bounded compilation time.
- **Build System:** Comptime is orthogonal to build scripts. Comptime runs in-compiler (limited); build scripts run as separate programs (unlimited). Dependencies in `rask.toml` are resolved before comptime execution.
- **Module System:** Comptime constants can be `pub` and exported. Importing comptime constants from other packages is allowed. Comptime functions can be called across package boundaries.
- **Tooling Contract:** IDEs SHOULD show comptime values as ghost annotations (e.g., show `const X = comptime fib(10)` with ghost text `// 55`). Comptime errors should include clickable links to source locations in comptime call stack.

## Remaining Issues

### High Priority
None identified.

### Medium Priority
1. **Comptime heap allocation** — Should comptime have limited heap allocation (e.g., fixed-size arena)? Useful for Vec/Map at comptime, but adds complexity.
2. **Comptime memoization** — Should identical comptime calls be memoized to speed up compilation? (e.g., `fib(10)` called multiple times)
3. **Step-through debugger** — Should IDEs support stepping through comptime interpreter? Complex but valuable for debugging.

### Low Priority
4. **Comptime imports** — Can comptime code import modules? Or only use built-in types?
5. **Comptime standard library** — Which stdlib functions should be `comptime` compatible? (e.g., string formatting, math)
6. **Comptime error recovery** — Should comptime support try/catch for better error messages? Or just panic → compile error?
