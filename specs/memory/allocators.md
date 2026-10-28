<!-- id: mem.alloc -->
<!-- status: proposed -->
<!-- summary: Dynamic current allocator — `using` installs one for a block and every allocation underneath takes it; collections remember theirs; escape and stash are compile errors -->
<!-- depends: memory/ownership.md, memory/borrowing.md, stdlib/collections.md, concurrency/async.md -->

# Allocators

Every task carries a current allocator. `using arena { … }` swaps it for the block, and every collection built while it's installed, in your code or in a library three calls down, takes memory from it. Nothing is declared, no type changes, and the compiler catches the two ways it could go wrong: a value leaving the block, and a callee hiding a fresh allocation somewhere that outlives the block.

I chose this over threading the allocator through signatures and types (`Vec<T, A>`, the Zig and Rust shape) because threading is viral in exactly the place that can't be fixed later. The moment a struct holds a `Map`, the struct grows an `A`, and a library whose author didn't thread it is `Global` forever. Here a library written without a thought for arenas runs in one unchanged. The price is a pointer per collection and an indirect call where the compiler can't see the caller, and the loss of reading a function and knowing where its memory goes — the ghost carries that instead.

## Allocator Interface

| Rule | Description |
|------|-------------|
| **AL1: Interface definition** | `Allocator` interface requires `alloc`, `dealloc`, `realloc` |
| **AL2: Global default** | `Global` implements `Allocator` and is the current allocator when nothing was installed |
| **AL3: Fallible allocation** | `alloc` returns `&raw u8 or AllocError`; `realloc` same |

<!-- test: skip -->
```rask
interface Allocator {
    func alloc(self, size: usize, align: usize) -> &raw u8 or AllocError
    func dealloc(self, ptr: &raw u8, size: usize, align: usize)
    func realloc(self, ptr: &raw u8, old_size: usize, new_size: usize, align: usize) -> &raw u8 or AllocError
}
```

AL4–AL11 are deleted. They made the allocator a defaulted type parameter (`Vec<T, Global>`) and threaded it through signatures as a hidden `using Allocator` parameter. The signature half went out with pools (#908); the type half went with this rewrite, since a type parameter is the viral part.

## The Current Allocator

| Rule | Description |
|------|-------------|
| **AL12: Using block** | `using expr { body }` where `expr: Allocator` installs `expr` as the current allocator for `body`, restored on exit — normal, `return`, `try`, and unwind alike |
| **AL21: Per task, dynamic** | The current allocator is a task-local value. A fresh collection (`Vec.new()`, `Map.new()`, a built `string`, `Heap(…)`, `Rack.new()`) takes whatever is current at the moment it allocates, however deep the call. AL14 (lexical only) is deleted: propagation into callees is the point |
| **AL22: A collection remembers its own** | A collection records the allocator it first allocated from and grows, shrinks and frees through that one, never through the current one. A `Global` vec pushed to inside an arena block keeps growing in `Global` |
| **AL23: Explicit choice** | `Vec.new_in(a)` and the same on every constructor builds the collection under `a` whatever is current. Same type as `Vec.new()`; only the remembered allocator differs |
| **AL24: Spawn starts at Global** | A spawned task starts with `Global` current. It never inherits the spawning block's allocator, because it may outlive it |

<!-- test: skip -->
```rask
public func parse_json(body: string) -> Json {     // written once, no allocator in sight
    mut fields = Map.new()
    …
}

func handle_request(req: Request) -> Response {
    using Arena.scoped(256.kilobytes()) {
        let body = try parse_json(req.body)       // the whole Json, nested maps included, lives in the arena
        return Response.json(body.keep())         // copied out to Global on the way back
    }
}
```

## Escape

| Rule | Description |
|------|-------------|
| **AL13: Block-born values stay in the block** | Any non-Copy value produced inside a `using` block — built there, or returned by a call made there — is block-born. A block-born value cannot be returned from the block, assigned to a binding or field declared outside it, pushed into a collection born outside it, sent on a channel, or captured by `spawn`. Compile error (E0420) |
| **AL25: Copy-out is explicit** | `.clone()` inside the block clones into the current allocator, so it doesn't help. `.keep()` deep-copies a value under `Global` and the result is not block-born. It's the one way out, and it's written where the copy happens |
| **AL26: Callers' values pass through** | A value born outside the block may be read and mutated inside it. AL22 keeps its growth in its own allocator, so passing it by `mutate` to a callee is fine |

AL13 is keyed on where the value was produced, not on its type, because its type doesn't say. That is the same shape as the `with` block rule for borrows (`mem.borrowing/W2`): a scope that lends you something checks that nothing leaves with it.

## Stash

A library can't leak a block-born value out of the block — AL13 is checked at the call site. What it can do is take a fresh allocation and put it somewhere that outlives the call: `self.index.insert(key, Vec.new())`, a global cache, an interner. Called under an arena, that's a use-after-free waiting for the block to end.

| Rule | Description |
|------|-------------|
| **AL27: Stash is inferred** | For every function the compiler computes, transitively over calls, which parameters and globals a fresh allocation can be moved into. Nothing is declared; it's a property of the body, shown as a ghost on the signature and by `rask api` |
| **AL28: Stashing into the outside is an error** | A call inside a `using` block to a function that stashes into a parameter the caller passes a non-block-born value for, or into a global, is a compile error (E0421) at the call, naming the line inside the callee that stashes. Fix: hoist the call out of the block, or wrap it in `using Global { … }` |
| **AL29: Block-born storage is fine** | Stashing into a parameter whose argument is block-born passes: the storage dies with the block too. A builder made in the block and filled by a helper is the common case |
| **AL30: Indirect calls** | Through a function value, a stored closure or an interface, the callee isn't one function. The compiler takes the union of the stash sets over every reachable target of that type, the way a widened method call is dispatched. A callback entered from C runs under `Global` whatever the task had installed, since its call site is where no check can run (`conc.async/CC3` draws the same line for `spawn`) |

AL17 and AL18 are deleted. An inline closure runs under whatever is current when it runs, which is what AL17 said; a stored closure does too, which is the opposite of AL18.

## Racks and Heap

| Rule | Description |
|------|-------------|
| **AL19: Rack backing allocator** | `Rack.new()` takes the current allocator for its chunks and remembers it (AL22). Under an arena the rack and every node go with the block |

`Heap(…)` takes the current allocator too (`mem.heap/HP6`). An arena is a bump allocator, so `drop` on an arena-backed `Heap<T>` runs the destructor and frees nothing; the block end frees the lot.

## Cost

| Rule | Description |
|------|-------------|
| **AL31: One pointer per collection** | The remembered allocator is one pointer in the collection header, null for `Global`. A `Vec` is 32 bytes, not 24. `string` keeps it in the heap header it already has |
| **AL32: Static where visible** | Direct calls lexically inside a `using` block, and every call in a program that never writes `using`, resolve at compile time. In a program with no `using` block at all the header pointer is removed, since every collection is provably `Global`. Semantics are AL21 either way |

Allocation already costs a `malloc`; the indirect call next to it is noise. The pointer is the real price and it's paid only by programs that install an allocator somewhere.

## Standard Allocators

| Allocator | Description |
|-----------|-------------|
| `Global` | System allocator (malloc/free). Default |
| `Arena` | Bump allocator. Bulk free on drop, `dealloc` is a no-op. Fixed size, exhaustion is `AllocError`, never a hidden `malloc` |
| `FixedBuffer` | Allocates from a `[u8; N]` the caller owns. No system calls. Embedded-friendly |

<!-- test: skip -->
```rask
// Request scratch — freed as one block
using Arena.scoped(1.megabytes()) {
    let tokens = tokenize(src)
    let ast = parse(tokens)
    return typecheck(ast).keep()
}

// No malloc at all
mut buf: [u8; 4096] = [0; 4096]
using FixedBuffer.new(buf) {
    let data = Vec.new()
    data.push(42)
}
```

## Error Messages

**Block-born value escaping [AL13]:**
```
ERROR [mem.alloc/AL13]: value born inside `using` block cannot leave it
   |
5  |  using Arena.scoped(1.megabytes()) {
6  |      let v = parse(src)
7  |      return v
   |      ^^^^^^^^ `v` was allocated under the arena, which is freed when the block ends

FIX: Copy it out, or move the allocation out of the block:

  return v.keep()
```

**Callee stashes into the outside [AL28]:**
```
ERROR [mem.alloc/AL28]: `index.add` would keep arena memory past the block
   |
12 |  using frame_arena {
13 |      index.add(name, parts)
   |      ^^^^^^^^^^^^^^^^^^^^^^ `index` was made outside the block
   |
   = `Index.add` stores a fresh `Vec` into `self.entries` (index.rk:41),
     and `self` here outlives the arena

FIX: Hoist the call out of the block, or run it under Global:

  using Global { index.add(name, parts.keep()) }
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Nested `using` blocks | AL12 | Inner installs for its scope; values born in the inner block can't leave it for the outer one |
| Outer collection pushed to inside the block | AL22, AL26 | Grows in its own allocator; the pushed value must not be block-born (AL13) |
| Block-born value pushed into block-born collection | AL13 | Fine, both die together |
| Comptime allocations | — | Compiler arena (`ctrl.comptime/CT17`), `using` is a compile error inside `comptime` |
| `try_push` on a `FixedBuffer`-backed vec | AL3 | `GrowError` when the buffer is full |
| Panic inside the block | AL12 | Unwind restores the previous allocator and frees the arena after `ensure` blocks ran |

---

## Appendix (non-normative)

### Rationale

**Why dynamic (AL21).** "Which allocator" has a different answer for the same function depending on who called it. Anything static has to clone the function per calling context, and per provenance of every parameter it grows through, and it still breaks at the first vtable. The runtime part is the minimum the semantics force: one pointer saying "current", one per collection saying "mine". AL32 recovers the static case wherever the compiler can see it.

**Why not type-erase the default (AL31).** The pointer is only there because the type doesn't say. A program that never installs an allocator has every collection on `Global` and the compiler knows it, so it drops the field. The cost lands on the programs that use the feature.

**Why `keep()` and not `clone()` (AL25).** Clone under an arena clones into the arena; that's what the current allocator means. Leaving the block is a different operation with a different cost, and the transparency rule wants it written where it happens.

**Why infer stash and not declare it (AL27).** Declaring it is function colouring under another name: it goes on every signature it's true of and every caller of those. Inference costs the compiler one transitive pass over MIR it already has the moves for, and the user nothing. If the analysis turns out to fire on every builder in a real program, the answer is a finer analysis, not an annotation.

### Open

An arena rewind marker (`frame_begin`/`frame_end` in Raido's terms) so a game loop acquires backing memory once and rewinds per frame instead of allocating and freeing an arena per frame. One rule, not yet written; see [c3-lessons.md](../analysis/c3-lessons.md).

### See Also

- [Borrowing](borrowing.md) — the `with` block rule AL13 is modelled on (`mem.borrowing/W2`)
- [Heap Values](heap.md) — `Heap(…)` takes the current allocator (`mem.heap/HP6`)
- [Racks and Links](racks.md) — rack chunks under an arena (`mem.racks`)
- [Async](../concurrency/async.md) — `using Multitasking { … }` installs a scheduler the same way
- [Collections](../stdlib/collections.md) — constructors and `new_in` (`std.collections`)
