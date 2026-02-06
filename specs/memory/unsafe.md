# Solution: Unsafe Blocks and Raw Pointers

## The Question
How does Rask enable low-level code (OS interaction, FFI, performance-critical sections) while maintaining safety guarantees elsewhere?

## Decision
Explicit `unsafe` blocks quarantine operations that bypass safety checks. Raw pointers exist only in unsafe contexts. Safe wrappers encapsulate unsafe operations behind safe interfaces. **Debug mode catches common pointer errors at runtime** (Zig-inspired), while release mode allows full optimization.

## Rationale
Safety is Rask's core property—but some code inherently cannot be verified by the compiler (FFI, hardware access, hand-optimized algorithms). The `unsafe` keyword marks these regions explicitly, making the safety boundary visible. All code outside unsafe blocks retains full safety guarantees. This follows Principle 4 (Transparent Costs): the risk is visible, not hidden.

**Pragmatic UB handling:** Rather than making all pointer errors "just UB" like Rust, Rask provides debug-mode runtime checks. This dramatically reduces debugging time without sacrificing release performance. The philosophy: **crash loudly in development, run fast in production**.

## Specification

### Unsafe Blocks

**Syntax:**
```rask
unsafe {
    // Operations that bypass safety checks
}
```

**Semantics:**

| Rule | Description |
|------|-------------|
| **U1: Explicit scope** | Unsafe operations are ONLY valid inside `unsafe {}` blocks |
| **U2: Local scope** | Unsafe blocks do not propagate; calling a safe function from unsafe is safe |
| **U3: Expression result** | Unsafe block can return a value: `let x = unsafe { ptr.read() }` |
| **U4: Minimal scope** | Unsafe blocks SHOULD be as small as possible |

### Operations Requiring Unsafe

| Operation | Reason |
|-----------|--------|
| **Raw pointer dereference** | `*ptr` may access invalid memory |
| **Raw pointer arithmetic** | `ptr.add(n)` may create dangling pointer |
| **Raw pointer to reference** | `&*ptr` creates reference from potentially invalid pointer |
| **Calling C functions** | C cannot provide Rask's safety guarantees |
| **Calling unsafe Rask functions** | Function declares it requires caller verification |
| **Implementing unsafe traits** | Trait contract cannot be verified by compiler |
| **Accessing mutable statics** | Data races possible without synchronization |
| **Transmute** | Reinterprets bytes as different type |
| **Inline assembly** | Arbitrary machine code |
| **Union field access** | Reading wrong variant is undefined |

**NOT requiring unsafe:**

| Operation | Reason |
|-----------|--------|
| Creating raw pointers | Safe; using them is unsafe |
| Calling safe C wrappers | Wrapper provides safety |
| Reading immutable statics | No data race possible |
| Bounds-checked array access | Runtime check provides safety |

### Raw Pointer Types

**Types:**

| Type | Description |
|------|-------------|
| `*T` | Immutable raw pointer to T |
| `*mut T` | Mutable raw pointer to T |

**Properties:**

| Property | Behavior |
|----------|----------|
| Size | Same as `usize` (platform pointer size) |
| Copy | Always Copy (pointer value, not pointee) |
| Nullable | Can be null; no Option optimization |
| Alignment | May be unaligned (accessing may require care) |
| Validity | Not tracked; may dangle |

**Creating raw pointers (safe):**
```rask
const x = 42
let ptr: *i32 = &x as *i32           // From reference
let mut_ptr: *mut i32 = &mut x       // From mutable reference
let null: *i32 = null                // Null pointer literal
```

**Using raw pointers (unsafe):**
```rask
unsafe {
    const value = *ptr                  // Dereference
    *mut_ptr = 100                    // Write through pointer
    const next = ptr.add(1)             // Pointer arithmetic
    let ref_back: &i32 = &*ptr        // Pointer to reference
}
```

### Pointer Operations

**Available inside unsafe blocks:**

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `*ptr` | Read/write | Dereference (undefined if invalid) |
| `ptr.read()` | `*T -> T` | Copy value from pointer |
| `ptr.write(v)` | `*mut T, T -> ()` | Write value to pointer |
| `ptr.add(n)` | `*T, usize -> *T` | Offset by n elements |
| `ptr.sub(n)` | `*T, usize -> *T` | Offset back by n elements |
| `ptr.offset(n)` | `*T, isize -> *T` | Signed offset |
| `ptr.is_null()` | `*T -> bool` | Check for null (safe to call) |
| `ptr.cast<U>()` | `*T -> *U` | Reinterpret pointer type |

**Alignment operations:**

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `ptr.align_offset(align)` | `*T, usize -> usize` | Bytes to next aligned address |
| `ptr.is_aligned()` | `*T -> bool` | Check natural alignment |
| `ptr.is_aligned_to(n)` | `*T, usize -> bool` | Check alignment to n bytes |

### Unsafe Functions

Functions that require unsafe to call:

```rask
unsafe func dangerous_operation(ptr: *mut i32) {
    // Body is implicitly unsafe
    *ptr = 42
}

func caller() {
    const x = 0
    unsafe {
        dangerous_operation(&mut x)    // Must be in unsafe block
    }
}
```

**Rules:**

| Rule | Description |
|------|-------------|
| **UF1: Implicit unsafe body** | Inside unsafe func, no nested unsafe block needed |
| **UF2: Caller responsibility** | Caller must verify preconditions hold |
| **UF3: Document invariants** | Unsafe functions SHOULD document safety requirements |

**Documentation convention:**
```rask
/// Reads value from pointer.
///
/// # Safety
/// - `ptr` must be valid for reads
/// - `ptr` must be properly aligned
/// - The memory must be initialized
unsafe func read_ptr<T>(ptr: *T) -> T {
    *ptr
}
```

### Unsafe Traits

Traits where implementing requires manual verification:

```rask
unsafe trait Send {}      // Safe to transfer across threads
unsafe trait Sync {}      // Safe to share reference across threads
```

**Implementing:**
```rask
struct MyType { ptr: *mut i32 }

// Implementer asserts: MyType can safely cross thread boundaries
unsafe extend MyType with Send {}
```

**Rules:**

| Rule | Description |
|------|-------------|
| **UT1: Explicit unsafe extend** | Implementing unsafe trait requires `unsafe extend` |
| **UT2: Contract obligation** | Implementer guarantees trait's safety contract |
| **UT3: Compiler trust** | Compiler trusts extend; soundness is implementer's responsibility |

**Built-in unsafe traits:**

| Trait | Contract |
|-------|----------|
| `Send` | Type can be transferred to another thread |
| `Sync` | Type can be shared (via &T) between threads |

### Safe/Unsafe Boundary

**What enters unsafe:**
- Values can be passed into unsafe blocks freely
- References passed in must remain valid for duration of unsafe use

**What exits unsafe:**

| Exiting Value | Requirement |
|---------------|-------------|
| Copy types | Must be valid bit pattern |
| References | Must point to valid, properly-typed memory |
| Raw pointers | Can exit; remain unusable outside unsafe |
| Owned values | Must be fully initialized |

**Safe wrapper pattern:**
```rask
public struct SafeBuffer {
    ptr: *mut u8,
    len: usize,
}

extend SafeBuffer {
    public func new(size: usize) -> SafeBuffer or AllocError {
        unsafe {
            const ptr = try alloc(size)
            Ok(SafeBuffer { ptr, len: size })
        }
    }

    public func get(self, index: usize) -> Option<u8> {
        if index >= self.len {
            return None
        }
        unsafe {
            Some(*self.ptr.add(index))
        }
    }

    func close(take self) {
        unsafe { dealloc(self.ptr, self.len) }
    }
}
```

**Invariant maintenance:**
- Safe API must maintain invariants that unsafe code relies on
- Breaking invariants through safe code = soundness bug in wrapper
- All public methods must preserve internal consistency

### Memory Model Inside Unsafe

**Relaxed guarantees:**

| Rule | Safe Code | Unsafe Code |
|------|-----------|-------------|
| Aliasing | Enforced by borrow checker | Programmer responsibility |
| Initialization | All values initialized | May access uninitialized |
| Validity | Types always valid | May have invalid bit patterns |
| Alignment | Always aligned | May be unaligned |

**Still enforced (even in unsafe):**

| Invariant | Enforcement |
|-----------|-------------|
| Data race = UB | No relaxation; use atomics for concurrent access |
| Stack discipline | Cannot return pointer to local |
| Type size/layout | Fixed by type definition |

**Uninitialized memory:**
```rask
unsafe {
    let buffer: [u8; 1024] = uninitialized()  // Explicit
    fill_buffer(buffer)                        // Initialize before use
    // Reading before fill_buffer = undefined behavior
}
```

### Transmute

Reinterprets the bits of one type as another:

```rask
unsafe {
    let x: u32 = 0x41424344
    let bytes: [u8; 4] = transmute(x)  // [0x44, 0x43, 0x42, 0x41] on little-endian
}
```

**Requirements:**

| Requirement | Description |
|-------------|-------------|
| Same size | Source and target must have identical size |
| Valid bits | Result must be valid for target type |
| Alignment | Both types properly aligned |

**Common valid transmutes:**

| From | To | Notes |
|------|----|-------|
| `[u8; N]` | Integer types | If size matches |
| `*T` | `usize` | Pointer to integer |
| `u32` | `f32` | Bit reinterpretation |
| `&T` | `*T` | Reference to pointer |

**Invalid transmutes (undefined behavior):**

| From | To | Problem |
|------|----|---------|
| Any | `&T` | May create invalid reference |
| `u8` | `bool` | Values other than 0/1 invalid |
| Any | Enum | May create invalid discriminant |

### Inline Assembly

Inline assembly embeds platform-specific machine code within Rask functions.

**Syntax:**
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

**Components:**

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

**Clobbers:**

| Clobber | Meaning |
|---------|---------|
| `clobber("rax", "rbx")` | Named registers destroyed |
| `clobber(memory)` | Memory may be modified |
| `clobber(flags)` | CPU flags modified |

**Options:**

| Option | Effect |
|--------|--------|
| `volatile` | Prevent reordering/elimination (default if no outputs) |
| `pure` | No side effects beyond outputs; eliminable if unused |
| `nomem` | Does not access memory |
| `nostack` | Does not use stack |

**Examples:**

```rask
// Read timestamp counter
func rdtsc() -> u64 {
    unsafe {
        let result: u64
        asm {
            "rdtsc; shl rdx, 32; or rax, rdx"
            out("rax") result
            clobber("rdx")
        }
        result
    }
}

// Memory fence
func mfence() {
    unsafe {
        asm {
            "mfence"
            options(volatile, nomem, nostack)
        }
    }
}

// Add with carry
func add_carry(a: u64, b: u64) -> (u64, bool) {
    unsafe {
        let sum: u64
        let carry: u8
        asm {
            "add {sum}, {b}; setc {carry}"
            inout(reg) sum = a
            in(reg) b
            out(reg_byte) carry
            clobber(flags)
        }
        (sum, carry != 0)
    }
}
```

**Comptime Integration:**

Assembly strings can be built at compile time:

```rask
const ARCH_ADD = comptime {
    if target.arch == .x86_64 { "add {out}, {a}" }
    else if target.arch == .aarch64 { "add {out}, {a}, {b}" }
}

unsafe {
    asm {
        ARCH_ADD
        out(reg) result
        in(reg) a, b
    }
}

// Include from external file
const CRYPTO_KERNEL = comptime @embed_file("sha256_x64.s")
```

**Rules:**

| Rule | Description |
|------|-------------|
| **ASM1** | `asm` blocks MUST be inside `unsafe` |
| **ASM2** | All modified registers not in outputs MUST be in clobber list |
| **ASM3** | Memory side effects require `clobber(memory)` or `volatile` |
| **ASM4** | Template is passed to assembler; errors surface at link time |
| **ASM5** | Multiple operands of same direction can share a line: `in(reg) a, b, c` |

### Mutable Statics

Global mutable state requires unsafe access:

```rask
static mut COUNTER: u32 = 0

func increment() {
    unsafe {
        COUNTER += 1    // Data race if called from multiple threads
    }
}
```

**Rules:**

| Rule | Description |
|------|-------------|
| **MS1: Unsafe access** | All reads/writes to `static mut` require unsafe |
| **MS2: No sync** | No synchronization provided; use atomics or mutex |
| **MS3: Prefer alternatives** | Use thread-local, atomic, or synchronized types instead |

### Unions

Tagged unions (enums) are safe. Untagged unions require unsafe for field access:

```rask
union IntOrFloat {
    i: i32,
    f: f32,
}

const u = IntOrFloat { i: 42 }
unsafe {
    const f = u.f    // Reinterprets bits as f32
}
```

**Rules:**

| Rule | Description |
|------|-------------|
| **UN1: Creation safe** | Creating union with any field is safe |
| **UN2: Read unsafe** | Reading any field requires unsafe |
| **UN3: Write safe** | Writing to a field is safe (sets active field) |

### Edge Cases

| Case | Handling |
|------|----------|
| Null pointer dereference | Undefined behavior (not caught) |
| Dangling pointer | Undefined behavior |
| Unaligned access | Platform-dependent (may fault or be slow) |
| Integer overflow in pointer math | Wrapping (no panic in unsafe) |
| Double-free | Undefined behavior |
| Use-after-free | Undefined behavior |
| Data race | Undefined behavior |
| Calling unsafe func without unsafe block | Compile error |
| Implementing safe trait unsafely | Compile error (use unsafe extend) |
| Nested unsafe blocks | Redundant but allowed |

### Debug-Mode Safety (Zig-inspired)

Unlike Rust where UB is UB regardless of build mode, Rask provides **debug-mode safety nets** for common pointer errors.

**Build modes:**

| Mode | Pointer Checks | Performance | Use Case |
|------|----------------|-------------|----------|
| `debug` | Runtime checks enabled | Slower | Development, testing |
| `release` | Checks removed (UB if wrong) | Fast | Production |
| `release-safe` | Checks kept | Medium | Safety-critical production |

**Debug-mode checks:**

| Operation | Debug Behavior | Release Behavior |
|-----------|----------------|------------------|
| Null pointer deref | Panic with location | UB |
| Out-of-bounds pointer | Panic with location | UB |
| Use-after-free | Panic (if detectable) | UB |
| Double-free | Panic (if detectable) | UB |
| Unaligned access | Panic | Platform-dependent |

**Implementation:**
```rask
unsafe {
    // In debug: inserts `if ptr.is_null() { panic!(...) }`
    // In release: no check
    const x = *ptr
}
```

**Explicit unchecked (skips even debug checks):**
```rask
unsafe {
    // No checks even in debug mode - for when you've already validated
    const x = ptr.read_unchecked()
}
```

**Rationale:** Most pointer bugs are detectable at runtime. Catching them in debug mode dramatically reduces the time to find bugs, without sacrificing release performance. This follows Zig's pragmatic approach.

### Unsafe Contracts

Every unsafe operation has an implicit **contract**—preconditions the caller must ensure.

**Contract documentation syntax:**
```rask
/// Reads value from pointer.
///
/// # Safety
/// - `ptr` MUST be non-null
/// - `ptr` MUST be valid for reads of size `size_of<T>()`
/// - `ptr` MUST be properly aligned for `T`
/// - The memory MUST be initialized as a valid `T`
/// - No other thread may write to this memory during the read
unsafe func read<T>(ptr: *T) -> T
```

**Contract categories:**

| Category | Description | Example |
|----------|-------------|---------|
| **Validity** | Pointer must point to valid memory | "ptr must be valid for reads" |
| **Alignment** | Pointer must be properly aligned | "ptr must be aligned to 4 bytes" |
| **Initialization** | Memory must contain valid data | "memory must be initialized" |
| **Aliasing** | No conflicting concurrent access | "no other writes during read" |
| **Lifetime** | Pointer must not dangle | "ptr must remain valid for 'a" |

**Exit invariants:**

When exiting an unsafe block, these MUST hold:

| Invariant | Description |
|-----------|-------------|
| **Type validity** | All values of Rask types must be valid for their type |
| **Ownership** | Ownership invariants must be restored (no double-ownership) |
| **Borrow rules** | If returning to safe code, borrow checker assumptions must hold |
| **Linear resources** | Linear values must still be tracked (consumed or live) |

**Example of broken exit invariant:**
```rask
let v: Vec<i32> = unsafe {
    // BAD: Creates Vec with invalid internal state
    Vec.from_raw_parts(null(), 0, 100)  // null ptr, claims 100 capacity
}
// Safe code now has a "valid" Vec that will crash on use
```

### Checked Unsafe Mode

For safety-critical code that needs low-level access but can afford overhead:

```rask
@checked_unsafe
func memory_copy(dst: *mut u8, src: *u8, len: usize) {
    // All pointer operations have runtime checks, even in release
    for i in 0..len {
        *dst.add(i) = *src.add(i)  // Each add/deref is checked
    }
}
```

**Behavior:**

| Attribute | Debug | Release |
|-----------|-------|---------|
| (none) | Checked | Unchecked (UB) |
| `@checked_unsafe` | Checked | Checked |
| `@unchecked_unsafe` | Unchecked | Unchecked |

**Use cases:**
- Medical devices, aerospace (safety-critical)
- Parsing untrusted input in release builds
- When you want guaranteed crash over silent corruption

### FFI and C Interop

See [Modules](../structure/modules.md) for C interop details. Summary:

| Operation | Requirement |
|-----------|-------------|
| Call C function | `unsafe { c.func(...) }` |
| Access C global | `unsafe { c.global }` |
| Pass callback to C | Function must be `extern "C"` |
| Receive pointer from C | Validation is caller's responsibility |

**Ownership across FFI:**
```rask
// C allocates, Rask frees
const ptr = unsafe { c.malloc(size) }
// ... use ptr ...
unsafe { c.free(ptr) }

// Rask allocates, C frees
const ptr = unsafe { alloc(size) }
// ... C uses ptr ...
// C must free with appropriate deallocator
```

## Examples

### Safe Wrapper for C String
```rask
public struct CString {
    ptr: *mut u8,
    len: usize,
}

extend CString {
    public func new(s: string) -> CString {
        unsafe {
            const len = s.len() + 1
            const ptr = try alloc(len)
            copy(s.as_ptr(), ptr, s.len())
            *ptr.add(s.len()) = 0  // Null terminator
            CString { ptr, len }
        }
    }

    public func as_ptr(self) -> *u8 {
        self.ptr  // Safe: pointer creation, not use
    }

    func close(take self) {
        unsafe { dealloc(self.ptr, self.len) }
    }
}
```

### Unchecked Array Access
```rask
struct FastBuffer<T> {
    data: *mut T,
    len: usize,
}

extend<T> FastBuffer<T> {
    /// # Safety
    /// `index` must be less than `self.len`
    public unsafe func get_unchecked(self, index: usize) -> T {
        *self.data.add(index)
    }

    public func get(self, index: usize) -> Option<T> {
        if index < self.len {
            unsafe { Some(self.get_unchecked(index)) }
        } else {
            None
        }
    }
}
```

### Atomic Counter
```rask
static COUNTER: AtomicU64 = AtomicU64.new(0)

func increment() -> u64 {
    COUNTER.fetch_add(1, Ordering.SeqCst)  // Safe: atomics handle sync
}
```

## Integration Notes

- **Memory Model:** Unsafe breaks borrow checker assumptions; programmer ensures aliasing rules. Safe code relies on unsafe code maintaining invariants.
- **Type System:** Raw pointers are types like any other; their use is restricted. `unsafe extend` extends type's capabilities.
- **Generics:** `T: Send` requires T to be sendable; raw pointers are not Send/Sync by default. Bounds propagate through generic code.
- **Concurrency:** Data races are UB even in unsafe. Use atomics, mutexes, or ensure single-threaded access.
- **C Interop:** All C calls are unsafe. Safe wrappers validate inputs, handle errors, manage ownership.
- **Compile-Time Execution:** Unsafe blocks are NOT allowed in comptime (no pointers at compile time).
- **Tooling:** IDE SHOULD highlight unsafe blocks distinctly. Lints SHOULD warn about large unsafe blocks.

---

## Remaining Issues

### High Priority
1. ~~**Atomics and memory ordering**~~ — See [Atomics](atomics.md) for atomic types, memory orderings, and concurrent patterns.
2. ~~**Inline assembly**~~ — See Inline Assembly section above.

### Medium Priority
3. **Provenance rules** — Pointer provenance (like Rust's Stacked Borrows) is unspecified. May cause optimization unsoundness or UB edge cases with pointer-to-int casts.

### Low Priority
4. **Safe wrapper verification** — No mechanism to verify that safe wrappers correctly encapsulate unsafe. Consider unsafe field patterns or auditing tools.
5. **Storable pointer guidelines** — Tension with "no storable references" principle. Need patterns for safely managing stored raw pointers.

### Addressed in This Version
- ~~UB detection tooling~~ — Debug-mode safety catches common pointer errors at runtime
- ~~Formal unsafe contracts~~ — Added contract documentation syntax and exit invariants
