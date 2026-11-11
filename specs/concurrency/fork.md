<!-- id: conc.fork -->
<!-- status: draft -->
<!-- summary: fork { } and { } runs blocks in parallel that borrow from the frame, and returns when all have finished -->
<!-- depends: concurrency/async.md, memory/closures.md, memory/borrowing.md -->
<!-- implemented-by: compiler/crates/rask-ownership/, compiler/crates/rask-interp/, compiler/crates/rask-mir/, compiler/runtime/threadpool.c -->

# Fork

`spawn` moves what it names into the task, because the task may outlive the
frame. A fork can't outlive it: every block finishes before the expression
does. So the blocks borrow, and the borrow rules between them are what keep it
race-free.

<!-- test: skip -->
```rask
func total(tree: Tree) -> i64 {
    match tree {
        Leaf(v) => return v,
        Node(l, r) => {
            let (a, b) = fork { total(l) } and { total(r) }
            return a + b
        }
    }
}
```

## Rules

| Rule | Description |
|------|-------------|
| **FK1: All blocks finish** | `fork { a } and { b }` runs every block and finishes when the last one does. More blocks chain with more `and`. Its value is a tuple of the blocks' values, in the order written |
| **FK2: Blocks borrow** | A block points at what it names, like a closure that stays in its frame (`mem.closures/CM1`). Nothing is copied or moved in, and a block can't consume what it names (`CM4`) |
| **FK3: Blocks don't conflict** | If one block writes a place, no other block may read or write it. Places are compared by path, so `p.x` and `p.y` are separate, and `p` overlaps both. Every block may read the same thing (E0960) |
| **FK4: A block is its own frame** | As with a task block (`conc.async/S7`): `return v` ends the block with `v`, `try` ends it with the error and makes that block's value `T or E`, and `break`/`continue` can't reach a loop outside it |
| **FK5: Parallel on a pool, in order without one** | Inside `using ThreadPool`, blocks after the first go to the pool and the forking thread runs the first. Outside one, the blocks run one after another in written order. FK3 makes the answer the same either way, so a library may fork without opening a runtime (`conc.async/C6`) |
| **FK6: A panic waits for its siblings** | A block that panics doesn't cut the others short. When all have finished, the fork re-raises the first panic in written order. The other blocks still point into the frame, so it can't unwind before they stop |
| **FK7: Task-bound values stay in one block** | A link, a rack, a `Local` box, or a value holding one may be named by one block only (E0961). Inside that block it is used by one thread at a time, as always. The same goes for a function value whose body the compiler can't see, such as a parameter: `both(h, h)` would hand one closure that writes its captures to two blocks |

FK7 is stricter than it has to be for racks. Two blocks reading one rack is
race-free in principle, except that storing a link anywhere writes the rack's
edge index (`mem.racks/RK10`, RK15), and so does the list a `for` over
`nodes()` walks. Staged structural mutation is what lifts it: see `mem.racks`.

## Not covered

- **Two halves of one `Vec`.** There's no way yet to lend out two
  non-overlapping index ranges of one buffer, so sorting the two halves in
  place needs them in separate values.
- **Green tasks.** Inside `using Multitasking` without a pool, blocks run in
  order.

## Error messages

```
ERROR [conc.fork/FK3]: two fork blocks touch `count`, and one of them writes it
   |
 3 |  let (a, b) = fork { count += 1 } and { print(count) }
   |                        ^^^^^              ----- read here, maybe at the same time
   |                        written here
   |
FIX: give each block its own variable and combine the results after the fork
WHY: the blocks may run at the same time, so a write in one and a read in the
     other is a data race.
```

```
ERROR [conc.fork/FK7]: two fork blocks name a link
   |
 3 |  fork { a.hp -= 1 } and { b.hp -= 1 }
   |         ^                 ^
   |
FIX: do the graph work in one block
WHY: two links can name the same node, and two blocks writing one node race.
```

## Appendix (non-normative)

### Rationale

I took the block form over a `both(f, g)` function because it reads like
`spawn`, the block can't be stored or called twice, and it isn't stuck at two:
a function can't declare an N-tuple result, a construct can.

FK5's in-order fallback is the reason the fork is safe to put in library code.
Running blocks one after another is always a correct schedule, so the fork
says "these may run in parallel", and the application decides whether they do.

A block that's still queued when the forking thread gets to it is taken back
and run there. That's what keeps nested forks from deadlocking a small pool:
a thread only ever waits for a block that some other thread is running.
