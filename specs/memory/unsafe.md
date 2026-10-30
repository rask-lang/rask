<!-- id: mem.unsafe -->
<!-- status: decided -->
<!-- summary: Explicit unsafe blocks for raw pointers, FFI, inline assembly. The compiler still tracks where each pointer points; debug-mode runtime checks for what it can't see -->
<!-- depends: memory/ownership.md -->
<!-- implemented-by: compiler/crates/rask-parser/, compiler/crates/rask-types/ -->

# Unsafe Blocks

Explicit `unsafe` blocks quarantine operations that bypass safety checks. `unsafe` gives up one guarantee: that a dereference lands on live memory of the right type. It gives up nothing else. Ownership, overflow, the borrow rules and the linear rules hold inside the block as they do outside it, and the compiler still knows where each pointer came from, so most pointer mistakes are compile errors (PT1–PT5). Debug mode catches what's left at runtime (Zig-inspired); release mode runs fast.

## Unsafe Block Rules

| Rule | Description |
|------|-------------|
| **U1: Explicit scope** | Unsafe operations are ONLY valid inside `unsafe {}` blocks |
| **U2: Local scope** | Unsafe does not propagate; calling a safe function from unsafe is safe |
| **U3: Expression result** | Unsafe block/expression can return a value: `let x = unsafe ptr.read()` |
| **U4: Minimal scope** | Unsafe blocks SHOULD be as small as possible |

<!-- test: skip -->
```rask
let xs: Vec<i32> = [42]
let ptr = unsafe xs.as_ptr()

let value = unsafe { *ptr }
```

## Operations Requiring Unsafe

| Operation | Reason |
|-----------|--------|
| Raw pointer dereference | `*ptr` may access invalid memory |
| Raw pointer arithmetic | `ptr.add(n)` may create dangling pointer |
| Taking a pointer (`as_ptr`) | The pointer is the only name for that memory the borrow rules don't see |
| Calling C functions | C cannot provide Rask's safety guarantees |
| Calling unsafe Rask functions | Function declares it requires caller verification |
| Conforming to an unsafe interface | Other unsafe code relies on a contract the compiler can't check (UT1) |
| Transmute | Reinterprets bytes as different type |
| Inline assembly | Arbitrary machine code |
| Union field access | Reading wrong variant is undefined |

**NOT requiring unsafe:**

| Operation | Reason |
|-----------|--------|
| Copying, storing or comparing a pointer | To safe code a pointer is a number |
| Calling safe C wrappers | Wrapper provides safety |
| Reading immutable statics | No data race possible |
| Bounds-checked array access | Runtime check provides safety |

## Raw Pointer Type

| Type | Description |
|------|-------------|
| `*T` | Raw pointer to T (read or write access) |

| Property | Behavior |
|----------|----------|
| Size | Same as `usize` (platform pointer size) |
| Copy | Always Copy (pointer value, not pointee) |
| Nullable | Can be null; no Option optimization |
| Alignment | May be unaligned |
| Validity | Not tracked; may dangle |

<!-- test: skip -->
```rask
mut xs: Vec<i32> = [42, 7]
let ptr = unsafe xs.as_mut_ptr()

unsafe {
    let value = *ptr
    *ptr = 100
    let next = ptr.add(1)
}
```

## Pointer Operations

All require unsafe except `is_null()`.

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `*ptr` | Read/write | Dereference (undefined if invalid) |
| `ptr.read()` | `*T -> T` | Copy value from pointer |
| `ptr.write(v)` | `*T, T -> void` | Write value to pointer |
| `ptr.add(n)` | `*T, usize -> *T` | Offset by n elements |
| `ptr.sub(n)` | `*T, usize -> *T` | Offset back by n elements |
| `ptr.offset(n)` | `*T, isize -> *T` | Signed offset |
| `ptr.is_null()` | `*T -> bool` | Check for null (safe to call) |
| `ptr.cast<U>()` | `*T -> *U` | Reinterpret pointer type |
| `ptr.align_offset(n)` | `*T, usize -> usize` | Bytes to next aligned address |
| `ptr.is_aligned()` | `*T -> bool` | Check natural alignment |
| `ptr.is_aligned_to(n)` | `*T, usize -> bool` | Check alignment to n bytes |

## Unsafe Functions

| Rule | Description |
|------|-------------|
| **UF1: Implicit unsafe body** | Inside unsafe func, no nested unsafe block needed |
| **UF2: Caller responsibility** | Caller must verify preconditions hold |
| **UF3: Document invariants** | Unsafe functions SHOULD document safety requirements via `/// # Safety` |

<!-- test: skip -->
```rask
/// Reads value from pointer.
///
/// # Safety
/// - `ptr` must be valid for reads
/// - `ptr` must be properly aligned
/// - The memory must be initialized
unsafe func read_ptr<T>(ptr: *T) -> T {
    return *ptr
}

func caller(xs: Vec<i32>) -> i32 {
    unsafe {
        return read_ptr(xs.as_ptr())
    }
}
```

## Where Pointers Point

A pointer is made by a call, and the call says where it points. The compiler keeps that with the binding the pointer lands in, the way a binding keeps a closure's frame limit (`mem.closures/SL1`), and checks every later use against it. These rules hold inside `unsafe` as well as outside.

| Rule | Description |
|------|-------------|
| **PT1: A call points where its body says** | A call that answers a pointer answers one into the arguments its body can return pointers into, the receiver included. That's inferred per function, like resizes and allocator origins (`mem.alloc/AL33`), never written: `func data(v: Vec<i32>) -> *i32 { return unsafe v.as_ptr() }` makes `data(xs)` point into `xs`, and a function that borrows `v` but returns `malloc` memory points into nothing. With no body to read (a native, an interface method, a closure value), the call points into every argument it borrows: `xs.as_ptr()` points into `xs`, and `p.add(1)` points wherever `p` does. A `take` argument isn't borrowed, and a number has no storage. A pointer from a C function (`malloc`, or one that hands back an opaque handle), an integer cast or `null` points into nothing the compiler can see: C doesn't say where its pointers point. A loop item or a `with` element is its collection's storage. A struct, tuple, container or closure holding a pointer points where the pointer does |
| **PT2: It doesn't leave what it points into** | A pointer into a value this function owns (a local, a `take` parameter) can't be returned, stored into a parameter's field or a `mutate` parameter, or sent into a task (E0930). One into a borrowed parameter may be returned or stored into a caller's field: the caller owns what it points into. No pointer into anything of this frame crosses into a task, since the task may outlive the call |
| **PT3: It dies with its source** | After what a pointer points into changes size (its buffer may move), moves, is replaced, or reaches the end of its block, using the pointer, or a value holding it as a whole, is a compile error (E0929). Reading a part of that value with no pointer in it is fine (`h.n` beside a dead `h.p`), and a value that must be consumed still must be. Taking the pointer again afterwards is fine |
| **PT4: No writes under a view** | While a `with` binding or a `for mutate` loop holds a place in a collection, writing through a pointer into that collection is a compile error (E0931). The binding is the one name that may change that place until the block ends |
| **PT5: The source stays alive while the pointer is used** | What a pointer points into isn't released before the pointer's last use, however long after its own last use by name that is. PT3 rules out the paths where it's freed on purpose; this one keeps the compiler from freeing it early |

<!-- test: skip -->
```rask
mut xs: Vec<i32> = [1, 2]
let p = unsafe xs.as_ptr()
xs.push(3)                      // may move xs's buffer
println(unsafe *p)              // error E0929: p points into xs, which changed size
let q = unsafe xs.as_ptr()      // fine: taken after the push
```

What's left for the `unsafe` block to vouch for is what the compiler can't see: memory a C function or `malloc` handed over, an offset from `add` staying inside the block it points into, and inline assembly. The debug-mode checks are for those.

## Unsafe Interfaces

An unsafe interface is one whose contract some unsafe code elsewhere relies on, and that the compiler can't check from a signature. A collection generic over an allocator trusts `dealloc` to free what `alloc` handed out. The person who writes that `dealloc` isn't the person who wrote the collection, so the conformance is where their half of the promise is written down.

| Rule | Description |
|------|-------------|
| **UT1: Explicit unsafe conformance** | Conforming to an `unsafe interface` requires `unsafe T implements I` (E0386) |
| **UT2: Contract obligation** | The conformance promises what the interface's doc comment says. The unsafe code that calls through it may rely on that promise and nothing more |
| **UT3: Only where it means something** | `unsafe` in front of a conformance to an ordinary interface, or in front of `extend`, is a compile error (E0932). It would read as a promise nobody relies on |

<!-- test: parse -->
```rask
/// Unsafe code calling `dealloc` relies on it freeing exactly what `alloc`
/// returned, and on `alloc`'s blocks not overlapping.
unsafe interface Allocator {
    func alloc(self, size: usize) -> *u8
    func dealloc(self, ptr: *u8, size: usize)
}

struct Bump { base: *u8, used: usize }

unsafe Bump implements Allocator {
    func alloc(self, size: usize) -> *u8 { return unsafe self.base.add(self.used) }
    func dealloc(self, ptr: *u8, size: usize) { }
}
```

There's no `Send` or `Sync`. What crosses a task is decided by a value's shape (`conc.async/S5`): copies are copied, owned values move, and borrows, links and `Local` boxes are refused. A raw pointer from `malloc` or C crosses as a number, and touching what it points at on the other side needs `unsafe` again. One into this frame's values is refused (PT2).

## Safe/Unsafe Boundary

| Exiting Value | Requirement |
|---------------|-------------|
| Copy types | Must be valid bit pattern |
| Raw pointers | Can exit; remain unusable outside unsafe, and still bound by PT2–PT4 |
| Owned values | Must be fully initialized |

**Exit invariants** — when leaving an unsafe block, these MUST hold:

| Invariant | Description |
|-----------|-------------|
| Type validity | All values of Rask types must be valid for their type |
| Ownership | No value has two owners: a value read through a pointer and kept is a copy, or it was moved out and its old place is never read again |

## Transmute

Reinterprets the bits of one type as another. Requires unsafe.

| Requirement | Description |
|-------------|-------------|
| Same size | Source and target must have identical size, as laid out in memory, padding included. A mismatch is a compile error |
| Written target | The target type comes from where the result goes (an annotated binding, a parameter, a return type). With none, it's a compile error |
| Valid bits | Result must be valid for target type |
| Alignment | Both types properly aligned |

<!-- test: parse -->
```rask
unsafe {
    mut x: u32 = 0x41424344
    mut data: [u8; 4] = transmute(x)
}
```

**Common valid transmutes:**

| From | To | Notes |
|------|----|-------|
| `[u8; N]` | Integer types | If size matches |
| `*T` | `usize` | Pointer to integer. The result points into nothing the compiler tracks (PT1) |
| `u32` | `f32` | Bit reinterpretation |

**Invalid transmutes (undefined behavior):**

| From | To | Problem |
|------|----|---------|
| `u8` | `bool` | Values other than 0/1 invalid |
| Any | Enum | May create invalid discriminant |
| Any | A type holding a `Vec`, `string`, `Heap` or link | Two owners for one block, or a pointer the runtime never handed out |

## Inline Assembly

| Rule | Description |
|------|-------------|
| **ASM1: Requires unsafe** | `asm` blocks MUST be inside `unsafe` |
| **ASM2: Declare clobbers** | All modified registers not in outputs MUST be in clobber list |
| **ASM3: Memory side effects** | Memory side effects require `clobber(memory)` or `volatile` |
| **ASM4: Assembler errors** | Template is passed to assembler; errors surface at link time |
| **ASM5: Multiple operands** | Multiple operands of same direction can share a line: `in(reg) a, b, c` |

**Syntax:**

<!-- test: skip -->
```rask
asm {
    "template with {placeholders}"
    out(constraint) name
    in(constraint) name
    inout(constraint) name
    clobber(list)
    options(list)
}
```

| Component | Purpose |
|-----------|---------|
| Template string | Assembly instructions; `{name}` substituted with operand locations |
| `out(constraint) name` | Output: assembly writes to this variable |
| `in(constraint) name` | Input: variable value available to assembly |
| `inout(constraint) name` | Both input and output |
| `clobber(list)` | Registers/memory modified as side effect |
| `options(list)` | Behavior modifiers |

**Constraints:**

| Constraint | Meaning |
|------------|---------|
| `reg` | Any general-purpose register |
| `reg_byte` | Byte-sized register (al, bl, etc.) |
| `xmm`, `ymm` | SSE/AVX registers |
| `mem` | Memory operand |
| `imm` | Immediate constant |
| `"rax"` | Specific register by name |

**Options:**

| Option | Effect |
|--------|--------|
| `volatile` | Prevent reordering/elimination (default if no outputs) |
| `pure` | No side effects beyond outputs; eliminable if unused |
| `nomem` | Does not access memory |
| `nostack` | Does not use stack |

## Unions

| Rule | Description |
|------|-------------|
| **UN1: Creation safe** | Creating union with any field is safe |
| **UN2: Read unsafe** | Reading any field requires unsafe |
| **UN3: Write safe** | Writing to a field is safe (sets active field) |

<!-- test: parse -->
```rask
union IntOrFloat {
    i: i32,
    f: f32,
}

let u = IntOrFloat(i: 42)
unsafe {
    let f = u.f    // Reinterprets bits as f32
}
```

## Debug-Mode Safety

Instead of "all pointer errors are UB" regardless of build mode, Rask provides debug-mode runtime checks. Crash loudly in development, run fast in production.

**Build modes:**

| Mode | Pointer Checks | Use Case |
|------|----------------|----------|
| `debug` | Runtime checks enabled | Development, testing |
| `release` | Checks removed (UB if wrong) | Production |
| `release-safe` | Checks kept | Safety-critical production |

**Debug-mode checks:**

| Operation | Debug Behavior | Release Behavior |
|-----------|----------------|------------------|
| Null pointer deref | Panic with location (built) | UB |
| Out-of-bounds pointer | Panic with location | UB |
| Use-after-free | Panic (if detectable) | UB |
| Double-free | Panic (if detectable) | UB |
| Unaligned access | Panic | Platform-dependent |

Of the debug checks, the null check on `*p`, `*p = v` and `p.read()` is built; the rest aren't yet.

**Checked/unchecked attributes** (on a function, for every raw-pointer access in it):

| Attribute | Debug | Release |
|-----------|-------|---------|
| (none) | Checked | Unchecked (UB) |
| `@checked_unsafe` | Checked | Checked |
| `@unchecked_unsafe` | Unchecked | Unchecked |

<!-- test: parse -->
```rask
// Skips even debug checks — for when you've already validated
unsafe {
    let x = ptr.read_unchecked()
}

// Keeps checks even in release — for safety-critical code
@checked_unsafe
func memory_copy(dst: *u8, src: *u8, len: usize) {
    for i in 0..len {
        unsafe { *dst.add(i) = *src.add(i) }
    }
}
```

## Memory Model Inside Unsafe

Inside an `unsafe` block the language is the same language. Overflow panics the same way, ownership and linearity are checked the same way, and the pointer rules above apply. What changes is that a dereference is trusted.

| | Memory the compiler tracks | Memory it doesn't (C, `malloc`, integer casts) |
|---|---|---|
| Lifetime | Checked: PT2, PT3, PT5 | The block's to vouch for |
| Aliasing with a `with` or `for mutate` | Checked: PT4 | The block's to vouch for |
| Bounds of `ptr.add(n)` | Debug-mode check | Debug-mode check where the allocator knows the block |
| Initialization, valid bit patterns | The block's to vouch for | The block's to vouch for |
| Data races | Undefined; use atomics | Undefined; use atomics |

Pointer arithmetic wraps rather than panicking, everywhere: a pointer is an address, and only dereferencing one can go wrong.

## FFI and C Interop

| Operation | Requirement |
|-----------|-------------|
| Call C function | `unsafe { c.func(...) }` |
| Access C global | `unsafe { c.global }` |
| Pass callback to C | Function must be `extern "C"` |
| Receive pointer from C | Validation is caller's responsibility |

See `struct.c-interop` for full C interop details.

## Error Messages

**Unsafe operation outside block [U1]:**
```
ERROR [mem.unsafe/U1]: unsafe operation outside unsafe block
   |
5  |  let x = *ptr
   |            ^^^^ raw pointer dereference requires unsafe

WHY: Unsafe operations must be explicitly scoped so safety boundaries are visible.

FIX: Wrap in unsafe:

  let x = unsafe *ptr
  let x = unsafe { *ptr }  // block form also works
```

**Calling unsafe function without block [U1]:**
```
ERROR [mem.unsafe/U1]: call to unsafe function outside unsafe block
   |
8  |  dangerous_operation(ptr)
   |  ^^^^^^^^^^^^^^^^^^^^^^^ requires unsafe

FIX:
  unsafe dangerous_operation(ptr)
  unsafe { dangerous_operation(ptr) }  // block form also works
```

**Missing clobber declaration [ASM2]:**
```
ERROR [mem.unsafe/ASM2]: register modified but not declared as clobber
   |
4  |  asm { "mov rax, 1" }
   |         ^^^ rax modified but not in out() or clobber()

FIX: Add to clobber list or declare as output:

  asm { "mov rax, 1"; clobber("rax") }
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Null pointer dereference | U1 | UB in release; panic in debug |
| Dangling pointer | U1 | UB in release; panic if detectable in debug |
| Unaligned access | U1 | Platform-dependent (may fault or be slow) |
| Integer overflow in pointer math | — | Wrapping, inside `unsafe` and out |
| Pointer used after its source grew | PT3 | Compile error (E0929) |
| Pointer into a local returned | PT2 | Compile error (E0930) |
| Double-free | U1 | UB in release; panic if detectable in debug |
| Use-after-free | U1 | UB in release; panic if detectable in debug |
| Data race | — | UB even in unsafe; use atomics |
| Calling unsafe func without unsafe block | U1 | Compile error |
| Implementing an unsafe interface without `unsafe` | UT1 | Compile error (write `unsafe T implements I`) |
| `unsafe` on an ordinary conformance or an `extend` | UT3 | Compile error (E0932) |
| Nested unsafe blocks | U2 | Redundant but allowed |
| Unsafe in comptime | U1 | Not allowed (no pointers at compile time) |

---

## Appendix (non-normative)

### Rationale

**U1–U4 (unsafe blocks):** Safety is core — but FFI, hardware access, hand-optimized algorithms can't be verified by the compiler. `unsafe` marks these regions explicitly. The boundary is visible. Code outside unsafe keeps full safety guarantees.

**Debug-mode safety:** Instead of "all pointer errors are UB" like Rust, I added debug-mode runtime checks. Cuts debugging time massively without hurting release performance. Philosophy: crash loudly in development, run fast in production. This follows Zig's pragmatic approach.

**Single pointer type (`*T`):** Rust's `*const T` / `*mut T` distinction adds ceremony without real safety — you can cast between them freely. A single `*T` type is simpler and equally honest about what raw pointers are.

**PT1–PT5 (where pointers point):** Rust leaves a raw pointer's lifetime to the programmer, because it can't connect a `*const T` back to the reference it came from. Rask has no references, so every pointer starts at a call, and the call's arguments say what it points into. That makes the common mistakes (a pointer kept across a `push`, returned out of the frame that owns the buffer, written while a `with` holds the element) the same errors a `with` binding or a closure already gets, with no annotation. The rule for a call reads the callee's body, the way resize and allocator inference do, so a function that borrows a vector and returns fresh memory isn't tied to the vector. Rust can't do this from a signature and asks for lifetime annotations instead. Only where there's no body to read does the call fall back to the strict reading, every borrowed argument.

**UT1–UT3 (unsafe interfaces):** The marker exists for one job: a contract that unsafe code on the other side of a generic relies on. `Send` and `Sync` don't qualify. In Rust, safe code can hand an `Rc` or a `&T` to another thread, so the type needs a marker saying it may. In Rask safe code has no such path: what crosses is decided by shape, and every place two tasks could touch one byte goes through an `unsafe` block someone wrote. A marker on the type would be a second signature on that same line.

**UF3 (document invariants):** Every unsafe operation has an implicit contract — preconditions the caller must ensure. The `/// # Safety` convention makes these visible.

### Patterns & Guidance

**Safe wrapper pattern:**

The core pattern for unsafe code: encapsulate raw operations behind a safe API that maintains invariants.

<!-- test: parse -->
```rask
public struct SafeBuffer {
    ptr: *u8,
    len: usize,
}

extend SafeBuffer {
    public func new(size: usize) -> SafeBuffer or AllocError {
        unsafe {
            let ptr = try alloc(size)
            return SafeBuffer(ptr, len: size)
        }
    }

    public func get(self, index: usize) -> u8? {
        if index >= self.len {
            return none
        }
        return unsafe { *self.ptr.add(index) }
    }

    func close(take self) {
        unsafe { dealloc(self.ptr, self.len) }
    }
}
```

- Safe API must maintain invariants that unsafe code relies on
- Breaking invariants through safe code = soundness bug in wrapper
- All public methods must preserve internal consistency

**Unchecked access pattern:**

<!-- test: skip -->
```rask
struct FastBuffer<T> {
    data: *T,
    len: usize,
}

extend<T> FastBuffer<T> {
    /// # Safety
    /// `index` must be less than `self.len`
    public unsafe func get_unchecked(self, index: usize) -> T {
        return *self.data.add(index)
    }

    public func get(self, index: usize) -> T? {
        if index < self.len {
            return unsafe { self.get_unchecked(index) }
        }
        return none
    }
}
```

**FFI ownership:**

<!-- test: parse -->
```rask
// C allocates, Rask frees
let ptr = unsafe { c.malloc(size) }
unsafe { c.free(ptr) }

// Safe CString wrapper
public struct CString {
    ptr: *u8,
    len: usize,
}

extend CString {
    public func new(s: string) -> CString {
        unsafe {
            let len = s.len() + 1
            let ptr = alloc(len)
            copy(s.as_ptr(), ptr, s.len())
            *ptr.add(s.len()) = 0
            return CString(ptr, len)
        }
    }

    public func as_ptr(self) -> *u8 {
        return self.ptr
    }

    func close(take self) {
        unsafe { dealloc(self.ptr, self.len) }
    }
}
```

**Inline assembly examples:**

<!-- test: skip -->
```rask
func rdtsc() -> u64 {
    unsafe {
        mut result: u64
        asm {
            "rdtsc; shl rdx, 32; or rax, rdx"
            out("rax") result
            clobber("rdx")
        }
        return result
    }
}

// Comptime: assembly strings can be built at compile time
const ARCH_ADD = comptime {
    if target.arch == .x86_64 { "add {out}, {a}" }
    else if target.arch == .aarch64 { "add {out}, {a}, {b}" }
}
```

**Unsafe contract documentation:**

<!-- test: parse -->
```rask
/// Reads value from pointer.
///
/// # Safety
/// - `ptr` MUST be non-null
/// - `ptr` MUST be valid for reads of size `size_of<T>()`
/// - `ptr` MUST be properly aligned for `T`
/// - The memory MUST be initialized as a valid `T`
/// - No other thread may write to this memory during the read
unsafe func read_at<T>(ptr: *T) -> T
```

Contract categories: validity (pointer points to valid memory), alignment (properly aligned), initialization (memory contains valid data), aliasing (no conflicting concurrent access).

### IDE Integration

IDE SHOULD highlight unsafe blocks distinctly. Lints SHOULD warn about large unsafe blocks.

| Feature | Behavior |
|---------|----------|
| Unsafe highlighting | `unsafe {}` blocks shown with distinct background |
| Hover on `unsafe` | Shows which operations inside require unsafe |
| Large block warning | Lint when unsafe block exceeds ~20 lines |

### Expression Form

`unsafe` supports both block and expression forms. The expression form is shorthand for a single-operation unsafe block:

<!-- test: parse -->
```rask
extern "C" func get_ptr() -> *i32

func example() {
    let ptr = unsafe get_ptr()

    // Block form
    let a = unsafe { *ptr }

    // Expression form (equivalent)
    let b = unsafe *ptr
}
```

The expression form encourages minimal unsafe scope — one operation, no ceremony. For multiple operations, use the block form.

### Design Decision: Granular Unsafe

I considered replacing blanket `unsafe {}` blocks with per-operation markers — syntax like `@unsafe deref` or specialized functions like `raw_deref(ptr)` that would tag each operation individually.

I decided against it. Per-operation syntax doesn't catch real bugs. The bugs in unsafe code are logic errors *within* correctly-identified operations (dangling pointer, wrong offset), not accidentally using the wrong *category* of operation. Adding per-operation markers increases ceremony without safety benefit — exactly the kind of abstraction tax the language tries to eliminate.

The compiler already classifies every unsafe operation internally (`UnsafeCategory` enum: pointer deref, deref write, arithmetic, extern call, unsafe func call, transmute, union field access). This data enables tooling — a future CLI command can report all unsafe operations by category across a codebase, providing auditability without syntax cost.

What I added instead:
- **Expression-form `unsafe <expr>`** — minimal syntax for single operations
- **`idiom/large-unsafe-block` lint** — warns when an unsafe block exceeds 10 statements, enforcing U4 (minimal scope)
- **Operation classification infrastructure** — compiler tracks categories for future tooling

This is "information without enforcement" — the data exists for tools that want it, but the language doesn't force developers to annotate what the compiler already knows.

### See Also

- [Ownership](ownership.md) -- Single-owner model (`mem.ownership`)
- [Atomics](atomics.md) -- Atomic types and memory orderings (`mem.atomics`)
- [C Interop](../structure/c-interop.md) -- Full FFI details (`struct.c-interop`)
- [Linearity](linear.md) -- Consume-exactly-once rules (`mem.linear`)
- [Resource Types](resource-types.md) -- `@resource` annotation (`mem.resources`)
- [Concurrency](../concurrency/README.md) -- What crosses a task (`conc.async/S5`)
