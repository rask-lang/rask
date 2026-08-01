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
| **U3: Expression result** | Unsafe block/expression can return a value: `const x = unsafe ptr.read()` |
| **U4: Minimal scope** | Unsafe blocks SHOULD be as small as possible |

<!-- test: skip -->
```rask
const x = 42
mut ptr: *i32 = &x as *i32

const value = unsafe { *ptr }
```

## Operations Requiring Unsafe

| Operation | Reason |
|-----------|--------|
| Raw pointer dereference | `*ptr` may access invalid memory |
| Raw pointer arithmetic | `ptr.add(n)`, `ptr.offset(n)` may create dangling pointer |
| Raw pointer to reference | `&*ptr` creates reference from potentially invalid pointer |
| Calling C functions | C cannot provide Rask's safety guarantees |
| Calling unsafe Rask functions | Function declares it requires caller verification |
| Implementing unsafe traits | Trait contract cannot be verified by compiler |
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
| `ptr.is_null()` | Simple null check, no dereference |
| Pointer equality comparison (`==`, `!=`) | Compares addresses, no dereference |
| `slice.as_ptr()` | Yields immutable raw pointer, no dereference |

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
mut ptr: *i32 = &x as *i32
mut null_ptr: *i32 = null

// Using raw pointers (unsafe)
unsafe {
    const value = *ptr
    *ptr = 100
    const next = ptr.add(1)
}
```

## Pointer Operations

All pointer method operations require `unsafe` except `is_null()` and equality comparisons (`==`, `!=`).

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `*ptr` | Read/write | Dereference (undefined if invalid) |
| `ptr.read()` | `*T -> T` | Copy value from pointer |
| `ptr.write(v)` | `*T, T -> void` | Write value to pointer |
| `ptr.add(n)` | `*T, usize -> *T` | Offset forward by n elements (unsafe) |
| `ptr.offset(n)` | `*T, isize -> *T` | Offset by signed n elements (unsafe) |
| `ptr.sub(n)` | `*T, usize -> *T` | Offset backward by n elements (unsafe) |
| `ptr.cast()` | `*T -> *U` | Reinterpret pointer type (unsafe) |
| `ptr.is_null()` | `*T -> bool` | Test for null (safe) |
| `ptr == other` | `*T, *T -> bool` | Compare pointer addresses (safe) |
| `ptr != other` | `*T, *T -> bool` | Compare pointer addresses (safe) |

### `as_mut_ptr` on Slices

To obtain a mutable raw pointer from a slice, the slice itself must be mutable:

| Method | Receiver | Description |
|--------|----------|-------------|
| `slice.as_ptr()` | `[T]` or `mutate [T]` | Immutable raw pointer to first element (safe) |
| `slice.as_mut_ptr()` | `mutate [T]` | Mutable raw pointer to first element (safe) |

> **Note:** `as_mut_ptr()` requires the slice parameter to be declared `mutate`; calling it on a read-only (non-`mutate`) parameter is a compile error. This is intentional — the mode system prevents obtaining a writable raw pointer from data you do not own mutably.

<!-- test: skip -->
```rask
// Correct: parameter declared mutate
fn fill_zeros(mutate dest: [u8]) {
    const dest_ptr = dest.as_mut_ptr()
    unsafe {
        // ... use dest_ptr ...
    }
}

// Error: cannot call as_mut_ptr() on read-only parameter
fn bad(dest: [u8]) {
    const dest_ptr = dest.as_mut_ptr()  // error[E0321]: cannot mutate parameter `dest`
}
```

## Null Pointers

| Expression | Type | Description |
|------------|------|-------------|
| `null` | `*T` (inferred) | Null pointer literal |
| `ptr.is_null()` | `bool` | Safe null test |
| `ptr == null` | `bool` | Safe null test via pointer equality |

<!-- test: skip -->
```rask
mut ptr: *u8 = null

// Both forms are equivalent and safe (no unsafe block needed):
if ptr.is_null() {
    // ...
}

if ptr == null {
    // ...
}
```

## Pointer Arithmetic Examples

<!-- test: skip -->
```rask
fn sum_array(ptr: *i32, len: usize) -> i32 {
    mut total = 0
    mut i: usize = 0
    while i < len {
        unsafe {
            total = total + ptr.offset(i as isize).read()
        }
        i = i + 1
    }
    total
}
```

## Pointer Equality

Pointer equality (`==` and `!=`) compares memory addresses. It is **safe** and does not require an `unsafe` block. This enables standard null checks and sentinel comparisons without wrapping them in unsafe.

<!-- test: skip -->
```rask
mut ptr: *u8 = null
mut end_ptr: *u8 = null

// Safe: address comparison only
if ptr == null {
    // handle null
}

if ptr != end_ptr {
    // not at end
}
```

## Debug Mode Checks

In debug builds, the compiler inserts runtime checks for common pointer errors:

| Check | Trigger |
|-------|---------|
| Null dereference | Dereference of null pointer |
| Alignment | Dereference of misaligned pointer |
| Stack bounds | Pointer outside current stack frame |

Release builds omit these checks for maximum performance.

## Interaction with the Mode System

Raw pointer operations interact with Rask's ownership and mode system:

| Scenario | Rule |
|----------|------|
| `ptr.read()` | Does not require `mutate`; copies data out |
| `ptr.write(v)` | Does not require `mutate` on the pointer itself; pointer is a value |
| `slice.as_mut_ptr()` | Requires `mutate` on the slice receiver |
| `ptr.offset(n)` | Requires `unsafe`; signed arithmetic may go out of bounds |
| `ptr.add(n)` | Requires `unsafe`; unsigned arithmetic may go out of bounds |

The mode system ensures that obtaining a mutable raw pointer (`as_mut_ptr`) requires proving mutable access to the underlying data, preventing aliased mutation through raw pointers obtained from borrowed-read data.