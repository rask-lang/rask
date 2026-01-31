# Solution: Unsafe Blocks and Raw Pointers

## The Question
How does Rask enable low-level code (OS interaction, FFI, performance-critical sections) while maintaining safety guarantees elsewhere?

## Decision
Explicit `unsafe` blocks quarantine operations that bypass safety checks. Raw pointers exist only in unsafe contexts. Safe wrappers encapsulate unsafe operations behind safe interfaces.

## Rationale
Safety is Rask's core property—but some code inherently cannot be verified by the compiler (FFI, hardware access, hand-optimized algorithms). The `unsafe` keyword marks these regions explicitly, making the safety boundary visible. All code outside unsafe blocks retains full safety guarantees. This follows Principle 4 (Transparent Costs): the risk is visible, not hidden.

## Specification

### Unsafe Blocks

**Syntax:**
```
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
```
let x = 42
let ptr: *i32 = &x as *i32           // From reference
let mut_ptr: *mut i32 = &mut x       // From mutable reference
let null: *i32 = null                // Null pointer literal
```

**Using raw pointers (unsafe):**
```
unsafe {
    let value = *ptr                  // Dereference
    *mut_ptr = 100                    // Write through pointer
    let next = ptr.add(1)             // Pointer arithmetic
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

```
unsafe fn dangerous_operation(ptr: *mut i32) {
    // Body is implicitly unsafe
    *ptr = 42
}

fn caller() {
    let mut x = 0
    unsafe {
        dangerous_operation(&mut x)    // Must be in unsafe block
    }
}
```

**Rules:**

| Rule | Description |
|------|-------------|
| **UF1: Implicit unsafe body** | Inside unsafe fn, no nested unsafe block needed |
| **UF2: Caller responsibility** | Caller must verify preconditions hold |
| **UF3: Document invariants** | Unsafe functions SHOULD document safety requirements |

**Documentation convention:**
```
/// Reads value from pointer.
///
/// # Safety
/// - `ptr` must be valid for reads
/// - `ptr` must be properly aligned
/// - The memory must be initialized
unsafe fn read_ptr<T>(ptr: *T) -> T {
    *ptr
}
```

### Unsafe Traits

Traits where implementing requires manual verification:

```
unsafe trait Send {}      // Safe to transfer across threads
unsafe trait Sync {}      // Safe to share reference across threads
```

**Implementing:**
```
struct MyType { ptr: *mut i32 }

// Implementer asserts: MyType can safely cross thread boundaries
unsafe impl Send for MyType {}
```

**Rules:**

| Rule | Description |
|------|-------------|
| **UT1: Explicit unsafe impl** | Implementing unsafe trait requires `unsafe impl` |
| **UT2: Contract obligation** | Implementer guarantees trait's safety contract |
| **UT3: Compiler trust** | Compiler trusts impl; soundness is implementer's responsibility |

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
```
pub struct SafeBuffer {
    ptr: *mut u8,
    len: usize,
}

impl SafeBuffer {
    pub fn new(size: usize) -> Result<SafeBuffer, AllocError> {
        unsafe {
            let ptr = alloc(size)?
            Ok(SafeBuffer { ptr, len: size })
        }
    }

    pub fn get(self, index: usize) -> Option<u8> {
        if index >= self.len {
            return None
        }
        unsafe {
            Some(*self.ptr.add(index))
        }
    }

    fn close(transfer self) {
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
```
unsafe {
    let mut buffer: [u8; 1024] = uninitialized()  // Explicit
    fill_buffer(&mut buffer)                       // Initialize before use
    // Reading before fill_buffer = undefined behavior
}
```

### Transmute

Reinterprets the bits of one type as another:

```
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

Platform-specific machine code (out of scope for core spec; will be separate spec when needed).

### Mutable Statics

Global mutable state requires unsafe access:

```
static mut COUNTER: u32 = 0

fn increment() {
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

```
union IntOrFloat {
    i: i32,
    f: f32,
}

let u = IntOrFloat { i: 42 }
unsafe {
    let f = u.f    // Reinterprets bits as f32
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
| Calling unsafe fn without unsafe block | Compile error |
| Implementing safe trait unsafely | Compile error (use unsafe impl) |
| Nested unsafe blocks | Redundant but allowed |

### FFI and C Interop

See [Module System](module-system.md) for C interop details. Summary:

| Operation | Requirement |
|-----------|-------------|
| Call C function | `unsafe { c.func(...) }` |
| Access C global | `unsafe { c.global }` |
| Pass callback to C | Function must be `extern "C"` |
| Receive pointer from C | Validation is caller's responsibility |

**Ownership across FFI:**
```
// C allocates, Rask frees
let ptr = unsafe { c.malloc(size) }
// ... use ptr ...
unsafe { c.free(ptr) }

// Rask allocates, C frees
let ptr = unsafe { alloc(size) }
// ... C uses ptr ...
// C must free with appropriate deallocator
```

## Examples

### Safe Wrapper for C String
```
pub struct CString {
    ptr: *mut u8,
    len: usize,
}

impl CString {
    pub fn new(s: string) -> CString {
        unsafe {
            let len = s.len() + 1
            let ptr = alloc(len)?
            copy(s.as_ptr(), ptr, s.len())
            *ptr.add(s.len()) = 0  // Null terminator
            CString { ptr, len }
        }
    }

    pub fn as_ptr(self) -> *u8 {
        self.ptr  // Safe: pointer creation, not use
    }

    fn close(transfer self) {
        unsafe { dealloc(self.ptr, self.len) }
    }
}
```

### Unchecked Array Access
```
struct FastBuffer<T> {
    data: *mut T,
    len: usize,
}

impl<T> FastBuffer<T> {
    /// # Safety
    /// `index` must be less than `self.len`
    pub unsafe fn get_unchecked(self, index: usize) -> T {
        *self.data.add(index)
    }

    pub fn get(self, index: usize) -> Option<T> {
        if index < self.len {
            unsafe { Some(self.get_unchecked(index)) }
        } else {
            None
        }
    }
}
```

### Atomic Counter
```
static COUNTER: AtomicU64 = AtomicU64::new(0)

fn increment() -> u64 {
    COUNTER.fetch_add(1, Ordering::SeqCst)  // Safe: atomics handle sync
}
```

## Integration Notes

- **Memory Model:** Unsafe breaks borrow checker assumptions; programmer ensures aliasing rules. Safe code relies on unsafe code maintaining invariants.
- **Type System:** Raw pointers are types like any other; their use is restricted. `unsafe impl` extends type's capabilities.
- **Generics:** `T: Send` requires T to be sendable; raw pointers are not Send/Sync by default. Bounds propagate through generic code.
- **Concurrency:** Data races are UB even in unsafe. Use atomics, mutexes, or ensure single-threaded access.
- **C Interop:** All C calls are unsafe. Safe wrappers validate inputs, handle errors, manage ownership.
- **Compile-Time Execution:** Unsafe blocks are NOT allowed in comptime (no pointers at compile time).
- **Tooling:** IDE SHOULD highlight unsafe blocks distinctly. Lints SHOULD warn about large unsafe blocks.

---

## Remaining Issues

### High Priority
(none)

### Medium Priority
1. **Inline assembly** — Syntax and semantics for `asm!` blocks

### Low Priority
2. **Provenance** — Should Rask have strict pointer provenance rules like Rust's Stacked Borrows?
