<!-- id: ctrl.comptime -->
<!-- status: decided -->
<!-- summary: Explicit comptime keyword for compile-time evaluation; restricted subset, no I/O -->
<!-- depends: types/generics.md -->
<!-- implemented-by: compiler/crates/rask-comptime/, compiler/crates/rask-miri/, compiler/crates/rask-interp/ -->

# Compile-Time Execution

Explicit `comptime` keyword marks compile-time evaluation. Restricted subset: pure computation, no I/O, no runtime-only features. Used for constants, generic specialization, conditional compilation.

## Comptime Forms

| Rule | Form | Syntax | Meaning |
|------|------|--------|---------|
| **CT1: Comptime variable** | Variable | `comptime mut x = expr` | Expression evaluated at compile time |
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

## Staging Model

Every expression has a stage: comptime (runs during compilation) or runtime (compiled into the binary). The stage is syntactic — set by the nearest enclosing comptime marker, never inferred.

| Rule | Description |
|------|-------------|
| **CT55: Two stages** | Comptime code computes values and decides what runtime code exists. Runtime code is the residue left after all comptime evaluation |
| **CT56: Comptime positions** | Comptime evaluation happens exactly at: `comptime` expressions, blocks, and variables; bodies of `comptime func`; `comptime` parameter arguments and array sizes; the iterable of `comptime for`; the condition of `comptime if`; the name in `value.(expr)` |
| **CT57: Residual bodies** | The body of a `comptime for` and the branches of a `comptime if` in runtime position stay runtime code. Comptime control decides *which* runtime code exists (unrolls, selects) — it never evaluates that code. Calls inside these bodies are ordinary runtime calls; CT6 doesn't apply to them |
| **CT58: Splicing** | A comptime value used in runtime position is embedded as constant data. It must be const-representable: primitives, `str`, and structs, enums, or fixed arrays of these. A comptime `string` embeds as `str`. Unfrozen `Vec`/`Map` cannot cross (CT19) |
| **CT59: Discarded branches** | A branch discarded by a runtime-position `comptime if` is syntax-checked only — same treatment as an uninstantiated generic body (`type.generics/G2`). This is what lets platform-specific code compile on every target |

<!-- test: skip -->
```rask
func encode<T: Encode>(value: T, mutate w: Writer) -> void or Error {
    comptime for field in reflect.fields<T>() {   // iterable: comptime (CT56)
        comptime if !field.is_skipped {           // condition: comptime (CT56)
            try w.write_key(field.serial_name)    // runtime residue (CT57);
            try encode(value.(field.name), mutate w)  // serial_name splices as str (CT58)
        }
    }
}
```

## Comptime For and Field Access

| Rule | Form | Syntax | Meaning |
|------|------|--------|---------|
| **CT48: Comptime for** | Loop | `comptime for x in comptime_iterable { body }` | Loop fully unrolled at compile time. Each iteration generates separate monomorphized code |
| **CT49: Field access** | Expression | `value.(comptime_expr)` | Access struct field by comptime-known string. Resolves at compile time to direct field access |

<!-- test: parse -->
```rask
import std.reflect

func print_fields<T>(value: T) {
    comptime for field in reflect.fields<T>() {
        // Each iteration: field.name is a different comptime string
        // value.(field.name) resolves to value.x, value.y, etc.
        print("{field.name} = {value.(field.name)}")
    }
}
```

| Rule | Description |
|------|-------------|
| **CT50: Unrolling** | `comptime for` fully unrolls at monomorphization time. Not a runtime loop — each iteration may have different types via comptime field access |
| **CT51: Comptime iterable** | The iterable must be comptime-known: `reflect.fields<T>()`, `reflect.variants<T>()`, or any comptime array |
| **CT52: No branch quota** | `comptime for` unrolling doesn't count against the backwards branch quota (CT35). The quota applies to comptime *interpreter* execution, not monomorphization-time unrolling |
| **CT53: Field name must be comptime** | The expression in `value.(expr)` must be comptime-known. Runtime strings are a compile error |
| **CT54: Field must exist** | Compile error if the comptime string doesn't match any field on the value's type |

Primary use case: serialization format libraries. See `std.encoding` for the full pattern.

## Calls at Comptime

Being callable at comptime is a property of what a function does, not of its marking. `comptime func` asserts the property at the definition; unmarked functions are checked where comptime code calls them.

| Rule | Description |
|------|-------------|
| **CT6: Comptime-evaluable calls** | A call in comptime position is legal iff the callee — after substituting generic parameters and resolving trait methods to concrete implementations — evaluates within CT7/CT8, transitively. No `comptime` marking required on the callee |
| **CT7: No I/O** | Cannot perform I/O (exception: `@embed_file`), spawn tasks, allocate from runtime pools |
| **CT8: No runtime values** | All inputs must be comptime-known; using runtime values in comptime context is a compile error |
| **CT60: Definition-time guarantee** | `comptime func` checks CT6/CT7 at the definition instead of at distant call sites. Obligations involving type parameters are deferred to instantiation (`type.generics/G2`). `comptime func` stays comptime-only (CT3) |
| **CT61: Trait bounds at comptime** | Calling a bound's method on `T` in comptime code is legal iff the concrete implementation, after instantiation, is comptime-evaluable — checked per instantiation. Auto-derived conformances (Equal, Hashable, Comparable, Cloneable, Debug, ErrorMessage) are comptime-evaluable by construction |
| **CT62: No dynamic dispatch** | `any Trait` cannot be created or called at comptime — heap allocation and vtables are runtime machinery (CT30–CT34) |

<!-- test: skip -->
```rask
func is_prime(n: u32) -> bool { ... }      // unmarked, pure

const PRIMES: [u32; _] = comptime {
    const v = Vec<u32>.new()
    for n in 2..100 {
        if is_prime(n) { v.push(n) }       // OK: is_prime is comptime-evaluable (CT6)
    }
    v.freeze()
}

comptime func bounds<T: Numeric>() -> (T, T) {
    return (T.zero(), T.one())             // legal per instantiation (CT61):
}                                          // i32's zero/one are comptime-evaluable

func example() {
    const n = read_config()
    const buf = repeat<n>(0xff)            // ERROR: n is runtime value (CT8)
}
```

## Generics and Phase Ordering

Comptime evaluation and monomorphization are not separate phases. Comptime code inside a generic function is part of instantiating it.

| Rule | Description |
|------|-------------|
| **CT63: Per-instantiation evaluation** | Comptime code in a generic function or type evaluates once per instantiation, after type and comptime parameters are substituted (`std.reflect/R5`, CT50). Non-generic comptime code evaluates once |
| **CT64: Demand-driven order** | Compilation starts from non-generic roots (consts, monomorphic functions) and interleaves instantiation with comptime evaluation on demand. Guarantee: an instantiation's comptime code is fully evaluated before its runtime residue is type-checked or compiled. Evaluations are memoized by (function, type arguments, comptime arguments) |
| **CT65: No compilation-state observation** | Comptime code observes source-declared facts (fields, variants, declared conformances, annotations) and build config (`cfg`) — never compilation progress (which instantiations exist, evaluation order, caches). Comptime results are deterministic and independent of evaluation order |
| **CT66: Types are not values** | Types reach comptime code only as generic arguments. No storing types in variables, returning them, or creating new types at comptime. The set of types and conformances is fixed by source before evaluation begins — this is what makes CT65 hold |
| **CT67: Value cycles are errors** | A comptime evaluation that demands its own result — directly, or through const initializers, type layouts, or instantiations — is a compile error reporting the full dependency chain. Residual calls demand only the callee's signature, never its comptime results: recursive and mutually recursive generic functions stay legal |
| **CT68: Instantiation depth limit** | An instantiation chain (each instantiation demanding the next) deeper than 64 is a compile error showing the chain. Override with `--instantiation-limit=N` |

<!-- test: skip -->
```rask
struct Bad {
    buf: [u8; comptime reflect.size_of<Bad>()]  // ERROR: layout of Bad
}                                               // depends on itself (CT67)

// Fine: encode<Node> calling encode<Owned<Node>?> calling encode<Node>
// is runtime recursion across instantiations — signatures only, no cycle (CT67)
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

<!-- test: parse -->
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

<!-- test: parse -->
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
| **CT48: Comptime for** | Loop unrolling | ✅ Full: unrolls over comptime arrays, each iteration separate code |
| **CT49: Field access** | `value.(name)` | ✅ Full: resolves to direct field access at compile time |

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
    mut i = 0
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
| **CT45: Error-type support** | Comptime functions can use `T or E` and `try` |
| **CT46: Panics as compile errors** | Comptime panics become compile errors with call stack |
| **CT47: Error propagation** | Errors propagate to compile error with context |

<!-- test: parse -->
```rask
enum DivError { ByZero }
extend DivError {
    func message(self) -> string { "Division by zero" }
}

comptime func safe_divide(a: i32, b: i32) -> i32 or DivError {
    if b == 0 {
        return DivError.ByZero
    }
    return a / b
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
| Comptime code calls unmarked pure function | CT6 | Works — evaluated at comptime, checked transitively |
| Comptime code calls function that does I/O | CT6/CT7 | Compile error with comptime call stack naming the I/O call |
| Comptime func with `<T: Trait>` calls bound method | CT61 | Legal iff T's implementation is comptime-evaluable; error shows instantiation chain |
| `any Trait` created or called at comptime | CT62 | Compile error: "dynamic dispatch not available at compile time" |
| `comptime for` body calls runtime function | CT57 | Works — the body is runtime residue, not comptime code |
| Discarded `comptime if` branch has type error | CT59 | Not reported — discarded branches are syntax-checked only |
| Comptime result feeds its own computation | CT67 | Compile error: "comptime dependency cycle" with chain |
| Instantiation chain past depth limit | CT68 | Compile error showing the chain |
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
| Runtime string in field access | CT53 | Compile error: "runtime string in comptime field access" |
| Non-existent field in field access | CT54 | Compile error: "no field X on type Y" |
| Comptime for over runtime iterable | CT51 | Compile error: "comptime for requires comptime-known iterable" |
| Nested comptime for | CT48 | Works — each level unrolls independently |

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

**Comptime-evaluability failure through a trait bound [CT61]:**
```
ERROR [ctrl.comptime/CT61]: cannot evaluate `Logger.hash` at compile time: it performs I/O

Comptime call stack:
  → cache_key<Logger>() at cache.rk:12:9
  → logger.hash() at cache.rk:14:16
      Logger's hash calls file.append() at logger.rk:33:5

Required by:
  const KEY = comptime cache_key<Logger>() at main.rk:4:11

WHY: Comptime code runs during compilation. It can call any function — marked or
     not — as long as evaluation stays inside the comptime subset (CT7/CT8).

FIX: Give Logger an I/O-free hash, or compute the key from a type that has one.
```

**Comptime dependency cycle [CT67]:**
```
ERROR [ctrl.comptime/CT67]: comptime dependency cycle

  → layout of struct Bad
  → comptime reflect.size_of<Bad>() at bad.rk:3:15
  → layout of struct Bad (cycle)

WHY: A comptime result cannot depend on itself. Layouts, const initializers, and
     instantiations form one dependency graph; cycles have no answer.

FIX: Break the cycle — size the buffer from the fields it holds, not from the
     struct that contains it.
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
<!-- test: parse -->
```rask
comptime func crc8_table() -> [u8; 256] {
    const table = [0u8; 256]
    for i in 0..256 {
        mut crc = i as u8
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
    mut crc = 0u8
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
        return Error.new("Packet too large")
    }
    return buffer
}

// Usage with different sizes
const small = try read_packet<64>(socket1)
const large = try read_packet<4096>(socket2)
```

### Conditional Compilation
<!-- test: parse -->
```rask
const DEBUG_MODE: bool = comptime cfg.debug
const LOGGING_ENABLED: bool = comptime cfg.features.contains("logging")

func process(data: []u8) -> void or Error {
    comptime if LOGGING_ENABLED {
        log.debug("Processing {} bytes", data.len)
    }

    for byte in data {
        comptime if DEBUG_MODE {
            // Validation only in debug builds
            if byte > 127 {
                return Error.new("Invalid byte")
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

**CT55-CT59 (staging):** The spec previously never said which parts of `comptime for`/`comptime if` run at compile time, which made CT6 read as forbidding the encoding pattern (runtime calls inside an unrolled body). The two-level rule fixes it: control parts are comptime, bodies are residue. Comptime *generates* runtime code; it never *runs* it.

**CT6/CT60 (property, not marking):** Comptime-callability is structural — determined by what the callee does, not what it's labeled. Requiring every transitively-called helper to be marked `comptime func` would be function coloring, which Rask rejects everywhere else (Principle 5); a pure helper like `is_prime` shouldn't need two versions. `comptime func` still earns its keyword: the guarantee moves to the definition, where the author is, instead of erupting at a call site three packages away. Same split as effects — inferred property, optional declared assertion.

**CT7-CT8 (pure subset):** I restrict to pure computation to keep the comptime interpreter simple and avoid full-language interpretation complexity (see Rust's limited `const fn`). Rask's runtime-heavy features (pools, linear resources, concurrency) don't make sense at compile time.

**CT61 (bounds at comptime):** Checked per instantiation, matching how generic bounds are checked everywhere else (`type.generics/G2`) and how effects are inferred per instance (`comp.effects/INF1`). A `comptime`-qualified bound in the type system would color generics — rejected.

**CT63-CT65 (ordering):** Demand-driven with memoization is the only order that works: comptime code needs T substituted (so it can't all run before monomorphization), and instantiation sites depend on comptime values (so it can't all run after). CT65 is what keeps this from becoming order-sensitivity: since comptime can't observe compilation progress, the schedule is unobservable, and the compiler is free to reorder, parallelize, and cache. CT66 closes the loop — no new types mid-flight means reflection answers never change while evaluation runs.

**CT67-CT68 (cycles):** Hard errors, no fixpoint iteration. A build that converges "eventually" is a build you can't reason about. The dependency chain in the error message is the debugging tool.

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

Comptime-callable logic doesn't need the `comptime` marking (CT6). Keep shared logic unmarked — test it at runtime, use it at comptime:

```rask
func factorial(n: u32) -> u32 {
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

// Same function at comptime (CT6)
const F5 = comptime factorial(5)
```

Workflow: write it unmarked → test at runtime with full debugging tools → use at comptime. Reserve `comptime func` for functions that need comptime-only machinery (`.freeze()`, reflection-heavy manipulation) or the definition-time guarantee — those can't run in tests (CT3).

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
- [Encoding](../stdlib/encoding.md) — Serialization via comptime for + field access (`std.encoding`)
- [Reflect](../stdlib/reflect.md) — Comptime type introspection (`std.reflect`)
- [Build System](../structure/build.md) — Build scripts and package configuration (`struct.build`)
- [Error Types](../types/error-types.md) — Result and error handling (`type.errors`)
