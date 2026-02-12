<!-- id: compiler.stdlib-audit -->
<!-- status: in-progress -->
<!-- summary: Audit of interpreter stdlib implementation vs spec requirements -->

# Standard Library Implementation Audit

Comparison of interpreter builtins implementation against spec requirements. Tracks what's implemented, what's missing, and what needs changes.

## Vec Implementation Status

### Core Methods (Implemented)

| Method | Status | Notes |
|--------|--------|-------|
| `push(item)` | ✓ Implemented | Returns `Result<(), PushError>` per spec |
| `pop()` | ✓ Implemented | Returns `Option<T>` |
| `len()` | ✓ Implemented | Returns count |
| `get(i)` | ✓ Implemented | Returns `Option<T>` (copies) |
| `is_empty()` | ✓ Implemented | Returns bool |
| `clear()` | ✓ Implemented | Empties vec |
| `reverse()` | ✓ Implemented | In-place reversal |
| `contains(x)` | ✓ Implemented | Linear search |
| `join(sep)` | ✓ Implemented | String concatenation |
| `eq(other)` | ✓ Implemented | Value equality |
| `ne(other)` | ✓ Implemented | Value inequality |
| `clone()` | ✓ Implemented | Deep copy |
| `insert(idx, item)` | ✓ Implemented | Insert at position |
| `remove(idx)` | ✓ Implemented | Remove at position |
| `first()` | ✓ Implemented | Returns `Option<T>` |
| `last()` | ✓ Implemented | Returns `Option<T>` |

### Iterator Methods (Implemented)

| Method | Status | Notes |
|--------|--------|-------|
| `iter()` | ✓ Implemented | Returns vec (borrows in real impl) |
| `filter(\|x\| bool)` | ✓ Implemented | Filters elements |
| `map(\|x\| y)` | ✓ Implemented | Transforms elements |
| `flat_map(\|x\| vec)` | ✓ Implemented | Maps and flattens |
| `fold(acc, \|acc, x\| r)` | ✓ Implemented | Reduces with accumulator |
| `reduce(\|acc, x\| r)` | ✓ Implemented | Reduces without initial |
| `enumerate()` | ✓ Implemented | Returns (index, value) pairs |
| `zip(other)` | ✓ Implemented | Pairs with another vec |
| `any(\|x\| bool)` | ✓ Implemented | Short-circuits on true |
| `all(\|x\| bool)` | ✓ Implemented | Short-circuits on false |
| `find(\|x\| bool)` | ✓ Implemented | Returns first match |
| `position(\|x\| bool)` | ✓ Implemented | Returns index of first match |
| `skip(n)` | ✓ Implemented | Skips n elements |
| `take(n)` / `limit(n)` | ✓ Implemented | Takes first n elements |
| `chunks(size)` | ✓ Implemented | Splits into chunks |
| `flatten()` | ✓ Implemented | Flattens nested vecs |
| `collect()` | ✓ Implemented | No-op (already vec) |
| `dedup()` | ✓ Implemented | Removes consecutive duplicates |
| `sum()` | ✓ Implemented | Sums integers |
| `min()` | ✓ Implemented | Finds minimum |
| `max()` | ✓ Implemented | Finds maximum |
| `sort()` | ✓ Implemented | In-place sort |
| `sort_by(\|a, b\| cmp)` | ✓ Implemented | Custom sort |

### Missing from Spec

| Method | Spec Reference | Priority | Notes |
|--------|----------------|----------|-------|
| `take_all()` | `std.iteration/T1` | **HIGH** | Consumes vec, yields owned values. Critical for linear types |
| `modify(i, \|v\| R)` | `std.collections/V1` | **HIGH** | Mutate element via closure |
| `read(i, \|v\| R)` | `std.collections/V1` | **HIGH** | Read element via closure |
| `modify_many([i, j], \|[a,b]\| R)` | `std.collections/D1` | MEDIUM | Multi-element mutation |
| `swap(i, j)` | `std.collections/D1` | LOW | Swap two indices |
| `remove_where(\|x\| bool)` | `std.collections` | MEDIUM | Conditional removal |
| `drain_where(\|x\| bool)` | `std.collections` | MEDIUM | Remove and collect |
| `retain(\|x\| bool)` | `std.collections` | LOW | Keep matching |
| `push_with(\|slot\| T)` | `std.collections` | LOW | In-place construction |
| `shrink_to_fit()` | `std.collections` | LOW | Shrink allocation |
| `shrink_to(n)` | `std.collections` | LOW | Shrink to capacity |
| `get_clone(i)` | `std.collections/V3` | MEDIUM | Clone out (non-Copy types) |

### Capacity Management (Not Yet Spec'd for Interpreter)

These are compile-target features, not needed for interpreter MVP:
- `Vec.with_capacity(n)`
- `Vec.fixed(n)`
- `capacity()`, `remaining()`, `allocated()`
- `reserve(n)`

## Map Implementation Status

### Core Methods (Implemented)

| Method | Status | Notes |
|--------|--------|-------|
| `insert(k, v)` | ✓ Implemented | Returns `Option<V>` (previous value) |
| `get(k)` | ✓ Implemented | Returns `Option<V>` |
| `remove(k)` | ✓ Implemented | Returns `Option<V>` |
| `len()` | ✓ Implemented | Returns count |
| `is_empty()` | ✓ Implemented | Returns bool |
| `contains(k)` | ✓ Implemented | Check key exists |
| `clear()` | ✓ Implemented | Empties map |
| `keys()` | ✓ Implemented | Returns vec of keys |
| `values()` | ✓ Implemented | Returns vec of values |
| `entries()` | ✓ Implemented | Returns vec of (k,v) pairs |
| `clone()` | ✓ Implemented | Deep copy |
| `eq(other)` | ✓ Implemented | Value equality |

### Missing from Spec

| Method | Spec Reference | Priority | Notes |
|--------|----------------|----------|-------|
| `ensure(k, \|\| v)` | `std.collections` | **HIGH** | Insert if missing |
| `ensure_modify(k, \|\| v, \|v\| R)` | `std.collections` | **HIGH** | Insert then mutate |
| `read(k, \|v\| R)` | `std.collections` | **HIGH** | Read via closure |
| `modify(k, \|v\| R)` | `std.collections` | **HIGH** | Mutate via closure |
| `get_clone(k)` | `std.collections` | MEDIUM | Clone out (non-Copy) |
| `take_all()` | `std.iteration` | **HIGH** | Consume and yield (k,v) |

## Pool Implementation Status

### Core Methods (Implemented)

| Method | Status | Notes |
|--------|--------|-------|
| `insert(item)` | ✓ Implemented | Returns `Handle<T>` |
| `get(h)` | ✓ Implemented | Borrows element |
| `get_mut(h)` | ✓ Implemented | Mutable borrow |
| `remove(h)` | ✓ Implemented | Takes ownership |
| `len()` | ✓ Implemented | Returns count |
| `is_empty()` | ✓ Implemented | Returns bool |
| `contains(h)` | ✓ Implemented | Check handle valid |
| `clear()` | ✓ Implemented | Empties pool |
| `clone()` | ✓ Implemented | Deep copy (handles stay valid) |
| `eq(other)` | ✓ Implemented | Value equality |

### Missing from Spec

| Method | Spec Reference | Priority | Notes |
|--------|----------------|----------|-------|
| `take_all()` | `std.iteration/T1` | **HIGH** | Consume pool, yield owned values |
| `iter()` (handle+ref mode) | `std.iteration/I2` | **HIGH** | Yield `(Handle<T>, borrowed T)` |

### Handle Methods (Implemented)

| Method | Status | Notes |
|--------|--------|-------|
| Index access `pool[h]` | ✓ Implemented | Expression-scoped borrow |

## String Implementation

Strings are represented as `Value::String(Arc<Mutex<String>>)` in the interpreter. No methods currently implemented on strings themselves — they're treated as primitives.

### Missing String Methods (Not Yet Spec'd)

Strings need their own spec document. Expected methods:
- `len()`, `is_empty()`
- `chars()`, `bytes()`
- `split(sep)`, `lines()`
- `trim()`, `trim_start()`, `trim_end()`
- `starts_with()`, `ends_with()`
- `contains()`
- `replace(old, new)`
- `to_uppercase()`, `to_lowercase()`
- Slicing: `s[0..3]`
- Concatenation: `+` operator

## Iteration Protocol

The spec describes three iteration modes:
1. **Index/Handle mode**: `for i in vec` yields indices
2. **Ref mode**: `for item in vec.iter()` borrows elements
3. **Take-all mode**: `for item in vec.take_all()` consumes collection

### Current Interpreter Behavior

The interpreter doesn't distinguish between these modes yet:
- `for i in vec` iterates over **elements** (not indices)
- `for item in vec.iter()` also iterates over elements (no borrow tracking)
- `vec.take_all()` **does not exist**

### Required Changes

To align with spec:
1. **Default iteration should yield indices**, not elements
2. Implement `take_all()` for all collections
3. Add `modify(i, |v| ...)` and `read(i, |v| ...)` for element access
4. Track borrows for ref mode (interpreter can be permissive)

## Error Handling

Current interpreter returns Rust `Result<Value, RuntimeError>` but doesn't wrap in Rask `Result<Ok, Err>` enum values consistently.

### Fallible Operations

Per spec, these should return `Result<T, E>`:
- `vec.push(x)` → `Result<(), PushError<T>>`
- `map.insert(k, v)` → `Result<Option<V>, InsertError<V>>`
- `vec.reserve(n)` → `Result<(), AllocError>`

Current implementation panics on allocation failure (Rust default) rather than returning error values.

## Priority Fixes

### Critical (Block Compiler)

These are needed before the compiler can emit correct code:

1. **`take_all()` for Vec, Map, Pool** — Required for linear type handling
2. **`modify()` and `read()` closures** — Required for safe non-Copy element access
3. **`ensure()` and `ensure_modify()`** — Common patterns in spec examples

### High (Interpreter Correctness)

These affect interpreter behavior vs spec:

1. **Default iteration yields indices** — Currently yields elements (wrong)
2. **Fallible allocation** — Should return `Result`, not panic
3. **String methods** — Need basic operations (split, trim, etc.)

### Medium (Convenience)

Nice to have but not blocking:

1. `remove_where()`, `drain_where()`, `retain()`
2. `modify_many()`, `swap()`
3. `get_clone()` for non-Copy types
4. `shrink_to_fit()`, capacity management

## Testing Recommendations

1. **Spec tests** — Add `<!-- test: run -->` blocks to specs/stdlib/*.md
2. **Error cases** — Test fallible operations return errors
3. **Iteration modes** — Verify index vs ref vs take_all behaviors
4. **Linear types** — Test that take_all is required for Vec<File>
5. **Closure methods** — Test `modify()`, `read()`, `ensure_modify()`

## Next Steps

1. Implement `take_all()` for Vec, Map, Pool (high priority)
2. Fix default iteration to yield indices (breaking change!)
3. Add `modify()` and `read()` closure methods
4. Implement `ensure()` and `ensure_modify()` for Map
5. Create string methods spec
6. Add comprehensive stdlib tests

---

Last updated: 2026-02-12
Audit scope: Interpreter builtins only (compile-target runtime TBD)
