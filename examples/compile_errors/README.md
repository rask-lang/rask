# Compile Error Examples

This directory contains code that **should not compile**. Each file demonstrates a specific safety guarantee enforced by the Rask compiler.

These examples serve two purposes:
1. **Documentation:** Show what errors look like and explain why they occur
2. **Testing:** The compiler test suite verifies these files produce the expected errors

## Examples

### [borrow_stored.rk](borrow_stored.rk)
**Error:** Cannot store a reference in a struct

Demonstrates that references are block-scoped only. You cannot store a borrow in a struct, return it from a function, or let it escape its scope. This prevents use-after-free and dangling pointers by construction.

### [resource_leak.rk](resource_leak.rk)
**Error:** Resource type not consumed

Demonstrates that `@resource` types (files, connections, etc.) must be consumed exactly once. Forgetting to close a file or dropping a connection without cleanup is a compile error.

### [comptime_loop.rk](comptime_loop.rk)
**Error:** Comptime iteration limit exceeded

Demonstrates that compile-time execution has safety limits. Infinite loops or excessive computation at comptime produces a clear error rather than hanging the compiler.

### [error_mismatch.rk](error_mismatch.rk)
**Error:** Incompatible error types with `?`

Demonstrates that error propagation with `?` requires compatible error types. You cannot propagate an error that doesn't fit in the function's return type without explicit conversion.

## Running Tests

```bash
# Verify all files fail to compile with expected errors
rask test-errors examples/compile_errors/
```

Each file includes a `// ERROR:` comment indicating the expected error message pattern.
