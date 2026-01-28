# Gap 3: Async Function Syntax and Types

**Priority:** HIGH
**Type:** Underspecified

**Question:** What is the syntax for async functions and their return types?

**Current spec:** Mentions `async fn` and "function color" but doesn't specify:
- Function signature syntax: `async fn foo()` or `fn foo() async`?
- Return type: explicit `Task<T>` or inferred?
- How to call async functions from sync code
- How to await async operations
- Whether there's a separate `await` keyword/operator

**Impact:** Cannot write or call async functions.
