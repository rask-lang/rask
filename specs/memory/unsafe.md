<!-- id: mem.unsafe -->
<!-- status: decided -->
<!-- summary: Explicit unsafe blocks for raw pointers, FFI, inline assembly; debug-mode runtime checks -->
<!-- depends: memory/ownership.md -->
<!-- implemented-by: compiler/crates/rask-parser/, compiler/crates/rask-types/ -->

# Unsafe Blocks

Explicit `unsafe` blocks quarantine operations that bypass safety checks. Debug mode catches common pointer errors at runtime (Zig-inspired); release mode runs fast.

## Unsafe Block Rules

| Rule | Description |
|------|-------------|
| **U1: Explicit scope** | Unsafe operations are ONLY valid inside `unsafe {}` blocks |
| **U2: Local scope** | Unsafe does not propagate; calling a safe function from unsafe is safe |
| **U3: Expression result** | Unsafe block can return a value: `const x = unsafe { ptr.read() }` |
| **U4: Minimal scope** | Unsafe blocks SHOULD be as small as possible |

<!-- test: skip -->
```rask
const x = 42
let ptr: *i32 = &x as *i32

const value = unsafe { *ptr }
```

## Operations Requiring Unsafe

| Operation | Reason |
|-----------|--------|
| Raw pointer dereference | `*ptr` may access invalid memory |
| Raw pointer arithmetic | `ptr.add(n)` may create dangling pointer |
| Raw pointer to reference | `&*ptr` creates reference from potentially invalid pointer |
| Calling C functions | C cannot provide Rask's safety guarantees |
| Calling unsafe Rask functions | Function declares it requires caller verification |
| Implementing unsafe traits | Trait contract cannot be verified by compiler |
| Accessing mutable statics | Data races possible without synchronization |
| Transmute | Reinterprets bytes as different type |
| Inline assembly | Arbitrary machine code |
| Union field access | Reading wrong variant is undefined |

**NOT requiring unsafe:**

| Operation | Reason |
|-----------|--------|
| Creating raw pointers | Safe; using them is unsafe |
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
// Creating raw pointers (safe)
const x = 42
let ptr: *i32 = &x as *i32
let null_ptr: *i32 = null

// Using raw pointers (unsafe)
unsafe {
    const value = *ptr
    *ptr = 100
    const next = ptr.add(1)
}
```

## Pointer Operations

All require unsafe except `is_null()`.

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `*ptr` | Read/write | Dereference (undefined if invalid) |
| `ptr.read()` | `*T -> T` | Copy value from pointer |
| `ptr.write(v)` | `*T, T -> ()` | Write value to pointer |
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

func caller() {
    let x = 0
    unsafe {
        dangerous_operation(&x)
    }
}
```

## Unsafe Traits

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

<!-- test: parse -->
```rask
unsafe trait Send {}
unsafe trait Sync {}

struct MyType { ptr: *i32 }

// Implementer asserts: MyType can safely cross thread boundaries
unsafe extend MyType with Send {}
```

## Safe/Unsafe Boundary

| Exiting Value | Requirement |
|---------------|-------------|
| Copy types | Must be valid bit pattern |
| References | Must point to valid, properly-typed memory |
| Raw pointers | Can exit; remain unusable outside unsafe |
| Owned values | Must be fully initialized |

**Exit invariants** — when leaving an unsafe block, these MUST hold:

| Invariant | Description |
|-----------|-------------|
| Type validity | All values of Rask types must be valid for their type |
| Ownership | Ownership invariants must be restored (no double-ownership) |
| Borrow rules | Borrow checker assumptions must hold for returned values |
| Linear resources | Linear values must still be tracked (consumed or live) |

## Transmute

Reinterprets the bits of one type as another. Requires unsafe.

| Requirement | Description |
|-------------|-------------|
| Same size | Source and target must have identical size |
| Valid bits | Result must be valid for target type |
| Alignment | Both types properly aligned |

<!-- test: skip -->
```rask
unsafe {
    let x: u32 = 0x41424344
    let bytes: [u8; 4] = transmute(x)
}
```

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

## Mutable Statics

| Rule | Description |
|------|-------------|
| **MS1: Unsafe access** | All reads/writes to `static mut` require unsafe |
| **MS2: No sync** | No synchronization provided; use atomics or mutex |
| **MS3: Prefer alternatives** | Use thread-local, atomic, or synchronized types instead |

<!-- test: skip -->
```rask
static mut COUNTER: u32 = 0

func increment() {
    unsafe {
        COUNTER += 1
    }
}
```

## Unions

| Rule | Description |
|------|-------------|
| **UN1: Creation safe** | Creating union with any field is safe |
| **UN2: Read unsafe** | Reading any field requires unsafe |
| **UN3: Write safe** | Writing to a field is safe (sets active field) |

<!-- test: skip -->
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
| Null pointer deref | Panic with location | UB |
| Out-of-bounds pointer | Panic with location | UB |
| Use-after-free | Panic (if detectable) | UB |
| Double-free | Panic (if detectable) | UB |
| Unaligned access | Panic | Platform-dependent |

**Checked/unchecked attributes:**

| Attribute | Debug | Release |
|-----------|-------|---------|
| (none) | Checked | Unchecked (UB) |
| `@checked_unsafe` | Checked | Checked |
| `@unchecked_unsafe` | Unchecked | Unchecked |

<!-- test: skip -->
```rask
// Skips even debug checks — for when you've already validated
unsafe {
    const x = ptr.read_unchecked()
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
5  |  const x = *ptr
   |            ^^^^ raw pointer dereference requires unsafe

WHY: Unsafe operations must be explicitly scoped so safety boundaries are visible.

FIX: Wrap in an unsafe block:

  const x = unsafe { *ptr }
```

**Calling unsafe function without block [U1]:**
```
ERROR [mem.unsafe/U1]: call to unsafe function outside unsafe block
   |
8  |  dangerous_operation(ptr)
   |  ^^^^^^^^^^^^^^^^^^^^^^^ requires unsafe

FIX:
  unsafe { dangerous_operation(ptr) }
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
| Integer overflow in pointer math | — | Wrapping (no panic in unsafe) |
| Double-free | U1 | UB in release; panic if detectable in debug |
| Use-after-free | U1 | UB in release; panic if detectable in debug |
| Data race | — | UB even in unsafe; use atomics |
| Calling unsafe func without unsafe block | U1 | Compile error |
| Implementing safe trait unsafely | UT1 | Compile error (use `unsafe extend`) |
| Nested unsafe blocks | U2 | Redundant but allowed |
| Unsafe in comptime | U1 | Not allowed (no pointers at compile time) |

---

## Appendix (non-normative)

### Rationale

**U1–U4 (unsafe blocks):** Safety is core — but FFI, hardware access, hand-optimized algorithms can't be verified by the compiler. `unsafe` marks these regions explicitly. The boundary is visible. Code outside unsafe keeps full safety guarantees.

**Debug-mode safety:** Instead of "all pointer errors are UB" like Rust, I added debug-mode runtime checks. Cuts debugging time massively without hurting release performance. Philosophy: crash loudly in development, run fast in production. This follows Zig's pragmatic approach.

**Single pointer type (`*T`):** Rust's `*const T` / `*mut T` distinction adds ceremony without real safety — you can cast between them freely. A single `*T` type is simpler and equally honest about what raw pointers are.

**UF3 (document invariants):** Every unsafe operation has an implicit contract — preconditions the caller must ensure. The `/// # Safety` convention makes these visible.

### Patterns & Guidance

**Safe wrapper pattern:**

The core pattern for unsafe code: encapsulate raw operations behind a safe API that maintains invariants.

<!-- test: skip -->
```rask
public struct SafeBuffer {
    ptr: *u8,
    len: usize,
}

extend SafeBuffer {
    public func new(size: usize) -> SafeBuffer or AllocError {
        unsafe {
            const ptr = try alloc(size)
            return Ok(SafeBuffer { ptr, len: size })
        }
    }

    public func get(self, index: usize) -> Option<u8> {
        if index >= self.len {
            return None
        }
        return unsafe { Some(*self.ptr.add(index)) }
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

    public func get(self, index: usize) -> Option<T> {
        if index < self.len {
            return unsafe { Some(self.get_unchecked(index)) }
        }
        return None
    }
}
```

**FFI ownership:**

<!-- test: skip -->
```rask
// C allocates, Rask frees
const ptr = unsafe { c.malloc(size) }
unsafe { c.free(ptr) }

// Safe CString wrapper
public struct CString {
    ptr: *u8,
    len: usize,
}

extend CString {
    public func new(s: string) -> CString {
        unsafe {
            const len = s.len() + 1
            const ptr = alloc(len)
            copy(s.as_ptr(), ptr, s.len())
            *ptr.add(s.len()) = 0
            return CString { ptr, len }
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
        let result: u64
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

<!-- test: skip -->
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

Contract categories: validity (pointer points to valid memory), alignment (properly aligned), initialization (memory contains valid data), aliasing (no conflicting concurrent access).

### IDE Integration

IDE SHOULD highlight unsafe blocks distinctly. Lints SHOULD warn about large unsafe blocks.

| Feature | Behavior |
|---------|----------|
| Unsafe highlighting | `unsafe {}` blocks shown with distinct background |
| Hover on `unsafe` | Shows which operations inside require unsafe |
| Large block warning | Lint when unsafe block exceeds ~20 lines |

### See Also

- [Ownership](ownership.md) -- Single-owner model (`mem.ownership`)
- [Atomics](atomics.md) -- Atomic types and memory orderings (`mem.atomics`)
- [C Interop](../structure/c-interop.md) -- Full FFI details (`struct.c-interop`)
- [Resource Types](resource-types.md) -- Must-consume types (`mem.resources`)
- [Concurrency](../concurrency/README.md) -- Send/Sync in concurrent contexts (`conc`)
