# Collection Iteration Modes

See also: [README.md](README.md)

## Collection Iteration Modes

Collections support multiple iteration modes depending on access needs:

| Collection | Index/Handle Mode | Ref Mode | Consume Mode |
|------------|-------------------|----------|--------------|
| `Vec<T>` | `for i in vec` → `usize` | N/A | `for item in vec.consume()` → `T` |
| `Pool<T>` | `for h in pool` → `Handle<T>` | `for (h, item) in &pool` → `(Handle<T>, &T)` | `for item in pool.consume()` → `T` |
| `Map<K,V>` | `for k in map` → `K` (K: Copy) | `for (k, v) in &map` → `(&K, &V)` | `for (k,v) in map.consume()` → `(K, V)` |

**Vec iteration:**
- Index mode only (ref mode not needed—indices are cheap)
- Consume mode for ownership transfer

**Pool iteration:**
- Handle mode for mutations/removals
- Ref mode for ergonomic read access (both handle AND data)
- Consume mode for consumption

**Map iteration:**
- Key mode for Copy keys only
- Ref mode for all key types (no cloning)
- Consume mode for consumption

**Ref mode semantics:**

```
// Pool ref iteration:
for (h, entity) in &pool {
    print(h, entity.position);  // Both handle and data available
    pool.remove(h);              // ERROR: cannot mutate during ref iteration
}

// Equivalent to:
for h in pool.handles() {
    let entity = &pool[h];      // Expression-scoped borrow
    print(h, entity.position);
    // Mutation forbidden here - see enforcement below
}  // Borrow released, but still in ref iteration block
```

## Ref Mode Mutation Prevention

**Enforcement Mechanism:** Compile-time syntactic analysis within the ref loop block.

**Rule:** The compiler tracks that a collection is being ref-iterated and forbids all mutation operations within that `for` loop's body, even between iterations when no expression-scoped borrow is active.

**What's Forbidden:**

| Operation | In Ref Mode Loop | Error |
|-----------|------------------|-------|
| `pool.remove(h)` | ❌ Forbidden | "cannot mutate `pool` during ref iteration" |
| `pool.insert(item)` | ❌ Forbidden | "cannot mutate `pool` during ref iteration" |
| `pool.clear()` | ❌ Forbidden | "cannot mutate `pool` during ref iteration" |
| `pool[h].field = x` | ✅ Allowed | Mutates item in place, doesn't invalidate iteration |
| `other_pool.remove(h2)` | ✅ Allowed | Different collection |

**Why This Works:**

1. **No stored borrow:** The loop doesn't store a borrow variable that lives across iterations
2. **Expression-scoped access:** Each `entity` variable is expression-scoped (ends at statement boundary)
3. **Compile-time analysis:** The compiler syntactically identifies `for ... in &collection` and marks the collection as "ref-iterated" within that block scope
4. **Local analysis only:** Analysis is limited to the lexical scope of the loop body—no cross-function tracking needed

**Comparison with Handle Mode:**

```
// Handle mode: mutations allowed (programmer responsibility)
for h in pool {
    if pool[h].expired {
        pool.remove(h);  // ✅ Allowed - but may invalidate other handles
    }
}

// Ref mode: mutations forbidden (compiler prevents invalidation)
for (h, entity) in &pool {
    if entity.expired {
        pool.remove(h);  // ❌ Compile error
    }
}
```

**Design Rationale:**

- Handle mode: Maximum flexibility, programmer ensures safety
- Ref mode: Convenient read access with compiler-enforced safety
- No actual borrow tracking across iterations needed (local analysis)
- Mutation prevention is syntactic (based on loop form), not semantic (based on runtime borrow state)

**Ref mode borrows:** Despite the `&` syntax, ref mode does NOT create a collection-level borrow variable that lives across iterations. Each iteration creates fresh expression-scoped refs. However, the compiler forbids collection mutations syntactically within the ref loop block to prevent invalidation.

### Mutation Through Function Calls

**Question:** What about `helper_that_removes(pool)` — can you call a function that might mutate the collection?

**Answer:** Parameter modes provide the enforcement mechanism — entirely local analysis.

```
fn process_item(pool: read Pool<Entity>, h: Handle<Entity>) { ... }
fn remove_expired(pool: mutate Pool<Entity>) { ... }
fn take_ownership(pool: transfer Pool<Entity>) { ... }

for (h, entity) in &pool {
    process_item(pool, h);      // ✅ Allowed — read mode cannot mutate
    remove_expired(pool);       // ❌ Compile error — mutate mode forbidden
    take_ownership(pool);       // ❌ Compile error — transfer impossible
}
```

**Enforcement Rules:**

| Parameter Mode | In Ref Loop | Reason |
|----------------|-------------|--------|
| `read` | ✅ Allowed | Cannot mutate by definition |
| `mutate` | ❌ Forbidden | Would allow mutation, invalidates iteration |
| `transfer` | ❌ Forbidden | Ownership transfer impossible during iteration |

**Error Messages:**

```
error: cannot pass `pool` with mutate mode during ref iteration
  --> example.rsk:5:5
   |
 3 | for (h, entity) in &pool {
   |                    ----- ref iteration begins here
 4 |     process_item(pool, h);
 5 |     remove_expired(pool);
   |     ^^^^^^^^^^^^^^^^^^^^ `remove_expired` requires mutate mode
   |
   = help: use handle iteration `for h in pool` if mutation is needed
```

**Why This Is Local Analysis (Principle 5):**

1. The compiler sees `for ... in &pool` → marks `pool` as "ref-borrowed" in scope
2. For any call passing `pool`, the compiler checks the **parameter mode from the signature**
3. If mode is `mutate` or `transfer` → compile error
4. No cross-function analysis needed — the mode declaration is the contract

This maintains both local analysis (only signature info needed) and mechanical safety (compiler prevents the bug, no programmer discipline required).

**When to use each mode:**

| Mode | Use When |
|------|----------|
| Index/Handle | Need to mutate or remove during iteration |
| Ref | Read-only access, avoid cloning large items |
| Consume | Consuming all items, transferring ownership |

## Value Access

Access follows existing expression-scoped collection rules:

| Expression | Behavior | Constraint |
|------------|----------|------------|
| `vec[i]` where T: Copy (≤16 bytes) | Copies out T | T: Copy |
| `vec[i].field` where field: Copy | Copies out field | field: Copy |
| `vec[i].method()` | Borrows for call, releases at `;` | Expression-scoped |
| `&vec[i]` passed to function | Borrows for call duration | Cannot store in callee |
| `vec[i] = value` | Mutates in place | - |
| `vec[i]` where T: !Copy | **ERROR**: cannot move | Use `.clone()` or `.consume()` |

**Rule:** Each `collection[index]` access is independent. Borrow released at statement end (semicolon).

## Examples: Common Patterns

**Copy types (≤16 bytes):**
```
for i in ids {
    let id = ids[i];  // i32 copies implicitly
    process(id);
}
```

**Non-Copy types (require clone):**
```
for i in users {
    if users[i].is_admin {
        return users[i].clone();  // Explicit clone
    }
}
```

**Read-only processing (no clone needed):**
```
for i in documents {
    print(&documents[i].title);  // Borrows for call
}
```

**In-place mutation:**
```
for i in users {
    users[i].login_count += 1;  // Mutate directly
}
```

**Nested loops:**
```
for i in vec {
    for j in vec {
        compare(&vec[i], &vec[j]);  // Both borrows valid
    }
}
```

**Closures (capture index or cloned value):**
```
for i in events {
    let event_id = events[i].id;  // Copy field
    handlers.push(|| handle(event_id));  // Captures id
}

// Or clone for non-Copy:
for i in events {
    let event = events[i].clone();
    handlers.push(|| handle(event));  // Captures clone
}
```

## Linear Types

**Index iteration is forbidden for `Vec<Linear>`:**

```
// COMPILE ERROR:
for i in files {
    files[i].close()?;  // ERROR: cannot move out of index
}

// Required:
for file in files.consume() {
    file.close()?;
}
```

**Error message:** "cannot iterate `Vec<{Linear}>` by index; use `.consume()` to consume"

**Pool iteration works** (handles are Copy):
```
// Handle mode: allows mutations/removals
for h in pool {
    pool.remove(h)?.close()?;  // Explicit remove + consume
}

// Ref mode: ergonomic read access
for (h, entity) in &pool {
    print(h, entity.name);  // No cloning needed
}
```

## Map Iteration

**Key mode requires Copy keys:**

```
// OK: u64 is Copy
let counts: Map<u64, u32> = ...;
for id in counts {
    print(counts[id]);
}

// ERROR: string is not Copy
let config: Map<string, string> = ...;
for key in config {  // ERROR: string is not Copy
    print(config[key]);
}
```

**Ref mode works for all key types:**
```
// OK: no cloning needed
for (key, value) in &config {
    print(key, value);
}
```

**Consume for ownership transfer:**
```
for (key, value) in config.consume() {
    process_owned(key, value);
}
```

### Map Iteration Ergonomics Recommendations

**Common Patterns:**

| Task | Recommended Pattern | Complexity |
|------|---------------------|------------|
| Read all values | `for (k, v) in &map { use(k, v) }` | 1 line |
| Collect filtered keys | `let keys = Vec::new(); for k in map { if map[k].pred { keys.push(k) } }` | 3 lines |
| Update all values | `for k in map { map[k].field += 1 }` | 1 line |
| Find specific entry | `for k in map { if map[k] == target { return k } }` | 1 line |
| Build reverse map | `let rev = Map::new(); for k in map { rev.insert(map[k].id, k)? }` | 2 lines |
| Consume into vec | `let pairs = Vec::new(); for (k,v) in map.consume() { pairs.push((k,v)) }` | 2 lines |

**Anti-Patterns to Avoid:**

| Anti-Pattern | Problem | Solution |
|--------------|---------|----------|
| `for k in map { let v = &map[k]; process(v) }` | Redundant access | Use `for (k, v) in &map` |
| `for k in map.keys() { ... }` | Extra method call | Use `for k in map` |
| `map.iter().for_each(\|k\| ...)` | Unnecessary closure | Use `for k in map` |
| `for k in map { if !cond { continue }; ... }` | Verbose filtering | Use `.filter()` adapter |

**Language Comparison:**

Compare Rask map iteration with Go, Rust, and Python for common tasks:

**Task 1: Sum all values**

```
// Rask
let sum = 0;
for k in counts { sum += counts[k]; }

// Go
sum := 0
for _, v := range counts { sum += v }

// Rust
let sum: i32 = counts.values().sum();

// Python
sum = sum(counts.values())
```

**Ergonomic Score:** Rask = 2 lines, Go = 2 lines, Rust = 1 line (but requires type annotation), Python = 1 line
**Verdict:** ✅ Acceptable — matches Go, functional style available via adapters if needed

**Task 2: Filter entries to new map**

```
// Rask
let active = Map::new();
for k in users {
    if users[k].active {
        active.insert(k, users[k].clone())?;
    }
}

// Go
active := make(map[int]User)
for k, v := range users {
    if v.active {
        active[k] = v
    }
}

// Rust
let active: HashMap<_, _> = users.iter()
    .filter(|(_, u)| u.active)
    .map(|(k, u)| (*k, u.clone()))
    .collect();

// Python
active = {k: v for k, v in users.items() if v.active}
```

**Ergonomic Score:** Rask = 5 lines, Go = 5 lines, Rust = 4 lines, Python = 1 line
**Verdict:** ✅ Acceptable — matches Go imperative style, explicit clone visible

**Task 3: Read-only iteration**

```
// Rask
for (id, user) in &users {
    print(id, user.name);
}

// Go
for id, user := range users {
    fmt.Println(id, user.name)
}

// Rust
for (id, user) in &users {
    println!("{} {}", id, user.name);
}

// Python
for id, user in users.items():
    print(id, user.name)
```

**Ergonomic Score:** Rask = 1 line, Go = 1 line, Rust = 1 line, Python = 1 line
**Verdict:** ✅ Excellent — identical to all languages

**Task 4: Remove matching entries**

```
// Rask
let to_remove = Vec::new();
for k in users {
    if users[k].expired { to_remove.push(k); }
}
for k in to_remove { users.remove(k); }

// Go
for k := range users {
    if users[k].expired {
        delete(users, k)  // Safe during iteration in Go
    }
}

// Rust
users.retain(|_, u| !u.expired);

// Python
users = {k: v for k, v in users.items() if not v.expired}
```

**Ergonomic Score:** Rask = 4 lines, Go = 3 lines (but unsafe in most languages), Rust = 1 line, Python = 1 line
**Verdict:** ⚠️ More verbose — but prevents iterator invalidation bugs. Use `.retain()` method for 1-line solution (separate spec).

**ED Metric Validation:**

Using Ergonomic Density formula: ED = (Lines_Baseline / Lines_Rask)

| Task | Rask Lines | Go Lines (Baseline) | ED Score | Target: ≥0.83 |
|------|-----------|---------------------|----------|---------------|
| Sum values | 2 | 2 | 1.00 | ✅ Pass |
| Filter map | 5 | 5 | 1.00 | ✅ Pass |
| Read iteration | 1 | 1 | 1.00 | ✅ Pass |
| Remove entries | 4 | 3 | 0.75 | ⚠️ Borderline |
| **Average** | | | **0.94** | ✅ Pass |

**Conclusion:** Map iteration ergonomics meet target (ED ≥ 0.83). The safety trade-off in removal patterns is acceptable given Rask's goal of preventing iterator invalidation bugs.

**Recommended Standard Library Additions:**

To improve ergonomics for common patterns:

| Method | Signature | Purpose | Reduces From | To |
|--------|-----------|---------|--------------|-----|
| `.retain(pred)` | `(&mut self, \|&K, &V\| -> bool)` | Filter in place | 4 lines | 1 line |
| `.values()` | `(&self) -> Iterator<&V>` | Iterate values only | 2 lines | 1 line |
| `.keys()` | `(&self) -> Iterator<K>` | Explicit key iteration (Copy keys) | N/A | Clarity |

These are standard library methods, not core language features. They can be added without changing the iteration semantics specified here.

---

## See Also
- [Loop Syntax](loop-syntax.md) - Basic loop syntax and borrowing semantics
- [Consume and Linear](consume-and-linear.md) - Consume iteration details
- [Mutation and Errors](mutation-and-errors.md) - Mutation during iteration, error handling
