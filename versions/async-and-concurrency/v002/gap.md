# Gap 2: Async Runtime Initialization

**Priority:** HIGH
**Type:** Underspecified

**Question:** How is the async runtime initialized? What is its lifecycle?

**Current spec:** Mentions "async runtime" and "100k+ green tasks" but doesn't specify:
- Is it global or per-thread?
- How is it started/stopped?
- What's the initialization API?
- Cost model (heap size, thread count)
- Can sync and async code mix in same binary?

**Impact:** Cannot write async programs without knowing how to initialize runtime.
