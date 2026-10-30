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
| **AL22: A collection remembers its own** | A collection records the current allocator when it's constructed, before it has allocated anything, and grows, shrinks and frees through that one, never through the current one. A `Global` vec pushed to inside an arena block keeps growing in `Global`, including an empty one whose first push happens there |
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
        return Response.json(body.clone_in(Global))   // copied out to Global on the way back
    }
}
```

## Escape

| Rule | Description |
|------|-------------|
| **AL13: A value can't outlive its allocator** | A value whose type owns heap memory (a collection, `string`, `Heap`, `Rack`, `Shared`, or anything holding one) is born under the allocator that was current when it was built, or under `a` for `new_in(a)` and `clone_in(a)`. A call's result is born where AL33 says. A literal or `const` is born under nothing and lives forever. A value born under a `using` block's allocator is block-born: it cannot be returned from that block, assigned to a binding or field declared outside it, pushed into a collection born outside it, sent on a channel, or captured by `spawn`. Compile error (E0420). Under `Global` nothing is block-born. A value born under an arena named directly (`new_in(arena)`, `clone_in(arena)`) lives as long as that arena's binding, the way a link lives as long as its rack (`mem.racks/RK6`): it can't be returned from the body that declared the arena, or stored anywhere longer-lived. A body that got the arena as a parameter is unrestricted, since the caller's arena outlives the call, and its result comes back born under the caller's arena (AL33) |
| **AL25: Copy-out is a clone with a target** | `.clone()` inside the block clones into the current allocator, so it doesn't help. `.clone_in(a)` deep-copies under `a`, and the result lives as long as `a` does: `clone_in(Global)` can leave every block, `clone_in(outer)` can leave the inner block but not `outer`'s. It's the one way out, and it's written where the copy happens. This is AL13 applied to the copy |
| **AL33: Call results are inferred like stashes** | For every function the compiler computes, in the same pass as AL27, where its result's heap memory can come from: a fresh allocation under the current allocator, a fresh one under a named allocator, or a parameter passed through. At the call the result is born under the shortest-lived of those: the current allocator for a fresh one, the named allocator for a named one, the argument's own allocator for a pass-through. Indirect calls take the union over targets (AL30). Nothing is declared |
| **AL26: Callers' values pass through** | A value born outside the block may be read and mutated inside it. AL22 keeps its growth in its own allocator, so passing it by `mutate` to a callee is fine |

AL13 is keyed on where the value was produced, not on its type, because its type doesn't say. That is the same shape as the `with` block rule for borrows (`mem.borrowing/W2`): a scope that lends you something checks that nothing leaves with it.

## Stash

A library can't leak a block-born value out of the block — AL13 is checked at the call site. What it can do is take a fresh allocation and put it somewhere that outlives the call: `self.index.insert(key, Vec.new())`, a global cache, an interner. Called under an arena, that's a use-after-free waiting for the block to end.

| Rule | Description |
|------|-------------|
| **AL27: Stash is inferred** | For every function the compiler computes, transitively over calls, which parameters and globals a fresh allocation made under the current allocator can be moved into. An allocation under a named one (`new_in(Global)`, `clone_in(Global)`, the body's own `using`) isn't counted, so a cache or interner that means to keep things writes that and is never a stash. Nothing is declared; it's a property of the body, shown as a ghost on the signature and by `rask api` |
| **AL28: Stashing into the outside is an error** | A call made while an allocator other than `Global` is current, to a function that stashes into storage that outlives that allocator's block — a parameter whose argument was born before the block, or a global — is a compile error (E0421) at the call, naming the line inside the callee that stashes. Fix at the call: hoist it out of the block, or wrap it in `using Global { … }`. Fix in the library, if keeping is the point: allocate what it keeps under `Global` (AL27) |
| **AL29: Block-born storage is fine** | Stashing into a parameter whose argument is block-born passes: the storage dies with the block too. A builder made in the block and filled by a helper is the common case |
| **AL30: Indirect calls** | Through a function value, a stored closure or an interface, the callee isn't one function. The compiler takes the union of the stash sets over every reachable target of that type, the way a widened method call is dispatched. An `extern "C"` function runs its body under `Global`, set on entry and restored on return, whoever calls it. C can only reach Rask through one (closures aren't C-compatible, `struct.c-interop/EX4`), and C may call it from a thread with no Rask task, so the function and not the task decides. Called from Rask it runs under `Global` too: one behaviour, not two |

AL17 and AL18 are deleted. An inline closure runs under whatever is current when it runs, which is what AL17 said; a stored closure does too, which is the opposite of AL18.

## Frames

A game loop wants one block of memory for the whole run and a clean slate every frame. `Arena.scoped` gets the clean slate by acquiring and freeing the backing memory each time. A frame keeps the memory: it marks where the arena's bump pointer is when the frame opens and moves it back there when the frame closes.

| Rule | Description |
|------|-------------|
| **AL34: A frame is a `using` that rewinds** | `using arena.frame() { body }` marks the arena's position, installs the arena as the current allocator, and moves the position back to the mark on exit, normal, `return`, `try` and unwind alike. Everything allocated during the body is gone; everything allocated before it stays. Values born in the body are frame-born, and AL13 keeps them inside like any block |
| **AL35** | Deleted. It made frames a separate `FrameArena` type that held nothing but the current frame. AL36 and AL37 get the same safety on an ordinary `Arena` |
| **AL36: Older values don't grow during a frame** | `arena.frame()` borrows `arena` for the block, like `with` (`mem.borrowing`), so inside it the arena can't be named: no `new_in(arena)`, no passing it on, no nested `arena.frame()`. A value born under `arena` before the frame opened may be read and changed in place inside the frame, but not grown: no push, insert, extend, reserve or longer string, directly or through a call that grows it (AL37). Growing takes room above the mark, and the rewind would free it under the value. Compile error (E0422) at the push, or at the call naming the line in the callee that grows. Values from any other allocator grow as usual |
| **AL37: Growth is inferred** | For every function the compiler computes, in the same pass as AL27 and AL33, which parameters it may grow: any operation that allocates through the parameter's remembered allocator (AL22), directly or by passing it to a callee that does. Indirect calls take the union over targets (AL30). Nothing is declared; it shows as a ghost on the signature next to the stash set |

<!-- test: skip -->
```rask
func run(mut world: World) {
    let arena = Arena.new(4.megabytes())          // backing acquired once
    using arena {
        let level = load_level()                  // stays for the whole run
        loop {
            using arena.frame() {
                level.player.x += 1               // fine: changed in place
                let hits = collide(level)         // fine: frame scratch
                draw(hits)
                world.tick()                      // fine: world grows in Global
            }                                     // rewound to the mark, level untouched
            level.spawn_wave()                    // fine: no frame open, level may grow
        }
    }
}
```

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
| `Global` | The process allocator. `malloc`/`free` unless the program replaces it before its first allocation (`rask_allocator_set` in the runtime), which is how a build with no OS gets a heap over a static buffer ([bare-metal.md](../analysis/bare-metal.md)). Default |
| `Arena` | Bump allocator. Bulk free on drop, `dealloc` is a no-op. Fixed size, exhaustion is `AllocError`, never a hidden `malloc`. `arena.frame()` rewinds to a mark (AL34) |
| `FixedBuffer` | Allocates from a `[u8; N]` the caller owns. No system calls. Embedded-friendly |

<!-- test: skip -->
```rask
// Request scratch — freed as one block
using Arena.scoped(1.megabytes()) {
    let tokens = tokenize(src)
    let ast = parse(tokens)
    return typecheck(ast).clone_in(Global)
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

  return v.clone_in(Global)
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

  using Global { index.add(name, parts.clone_in(Global)) }
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
| Frame-born value assigned to a binding outside the frame | AL13 | Compile error, same as any nested block |
| `return Vec.new_in(arena)` where this body declared `arena` | AL13 | Compile error: the arena is freed on return |
| `Vec.new_in(arena)` in a function that took `arena` as a parameter, returned | AL13, AL33 | Fine: the caller's arena outlives the call, and the caller sees the vec as born under it |
| `Vec.new_in(arena)` inside `arena.frame()` | AL36 | Compile error: the frame borrows `arena` |
| `arena.frame()` inside another `arena.frame()` | AL36 | Compile error: `arena` is already borrowed by the outer frame |
| `level.enemies.push(e)` inside the frame, `level` born before it | AL36 | Compile error at the push. Grow it before or after the frame, or keep `level` in `Global` |
| `level.enemies[i].hp -= 1` inside the frame | AL36 | Fine: in place, nothing grows |
| Frame-born vec pushed to inside the frame | AL34 | Fine: it's above the mark and goes with it |
| Frame runs out of room | AL3 | `AllocError` from the allocation that didn't fit. The arena never grows, so a frame never hides a `malloc` |
| Outer `World` mutated in place inside a frame | AL22 | Fine. Its collections remember `Global` and grow there |
| Pointer from `.as_ptr()` handed to C inside a block | AL13 | No new rule. A raw pointer is already invalid once its source is dropped (`struct.c-interop/ST4`), the source is block-born so it's dropped no later than the block ends, and the arena is freed no earlier. C keeping it longer was already the `unsafe` block's promise to break |
| `return "done"` from inside a block | AL13 | Fine. A literal is born under nothing |
| A 32-byte struct of numbers built in the block | AL13 | Fine. Its type owns no heap memory, so the allocator never touched it |
| `let v = Vec.new_in(Global)` in the block, then returned | AL13 | Fine. Born under `Global` |
| `let cfg = pick(global_cfg)` in the block, then returned | AL33 | Fine when `pick` only ever returns its argument: `cfg` is born where `global_cfg` was. Rejected if `pick` can also build a fresh one, since that one would be in the arena |
| C callback wants the caller's arena | AL30 | Pass the arena through C's userdata pointer and install it with `using` in the callback body. That block is ordinary Rask and checked like any other; the cast back from `*void` is `unsafe`, and it carries the promise that the arena is still alive when C calls |

---

## Appendix (non-normative)

### Rationale

**Why dynamic (AL21).** "Which allocator" has a different answer for the same function depending on who called it. Anything static has to clone the function per calling context, and per provenance of every parameter it grows through, and it still breaks at the first vtable. The runtime part is the minimum the semantics force: one pointer saying "current", one per collection saying "mine". AL32 recovers the static case wherever the compiler can see it.

**Why not type-erase the default (AL31).** The pointer is only there because the type doesn't say. A program that never installs an allocator has every collection on `Global` and the compiler knows it, so it drops the field. The cost lands on the programs that use the feature.

**Why `clone_in(a)` and not `clone()` (AL25).** Clone under an arena clones into the arena; that's what the current allocator means. Leaving the block needs the copy to land somewhere else, and the call site should say where. `clone_in` reuses two words a reader already has: `clone` for the cost, `_in` from `Vec.new_in(a)` for the target.

**Why infer stash and not declare it (AL27).** Declaring it is function colouring under another name: it goes on every signature it's true of and every caller of those. Inference costs the compiler one transitive pass over MIR it already has the moves for, and the user nothing. If the analysis turns out to fire on every builder in a real program, the answer is a finer analysis, not an annotation.

**Why growth is checked (AL36).** Raido's frame marker keeps everything below the mark and rewinds what the frame added. Older data pointing above the mark is a host bug there, caught only by an opt-in runtime check that's off by default (`debug_frame_writes`). In Rask the frame-born half is already AL13. Growth is the other way in: growing an older value takes room at the top of the arena, above the mark, so the rewind would free memory that value still points at. A separate arena type holding only the current frame avoids the question, at the cost of a second type and of level data living somewhere else. Inferring "may grow this parameter" costs one more fact in a pass that already runs, and it lets the level share the arena with its frames. The error points at the push that would have dangled.

### Open

**Whether `extern "C"` functions always run under `Global` (AL30) is provisional.** It's the simplest rule that's safe, and safety doesn't depend on the choice: everything a callback builds under `Global` can go anywhere. What it costs is speed and one surprise. Callbacks never see the caller's arena, and Rask code calling an exported function directly gets `Global` allocations inside an arena block. The alternative is inheriting the current allocator when C calls back on a Rask task's thread, which makes passing a Rask function to C count as calling it for the escape and stash checks. I don't know yet which matters more over time. What would settle it is real C libraries with callbacks in the validation programs (SQLite row callbacks in the HTTP server, a SAX-style parser), measured: how much callback allocation misses the arena, and how often the userdata workaround gets written.

### See Also

- [Borrowing](borrowing.md) — the `with` block rule AL13 is modelled on (`mem.borrowing/W2`), and the borrow `frame()` takes (AL36)
- [Heap Values](heap.md) — `Heap(…)` takes the current allocator (`mem.heap/HP6`)
- [Racks and Links](racks.md) — rack chunks under an arena (`mem.racks`)
- [Async](../concurrency/async.md) — `using Multitasking { … }` installs a scheduler the same way
- [Collections](../stdlib/collections.md) — constructors and `new_in` (`std.collections`)
