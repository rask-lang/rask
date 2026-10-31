<!-- id: analysis.library-protocols -->
<!-- status: exploration -->
<!-- summary: How a library adds a language feature in Rask. Rust does it with references and Drop; Rask can do it with scopes and obligations, if `with` becomes a protocol a type conforms to. One language change, two spec changes, and a list of what still can't be built -->
<!-- depends: memory/shared-rack-heap.md, memory/borrowing.md, memory/linear.md, memory/closures.md, memory/unsafe.md, types/sequence-protocol.md, analysis/bare-metal.md -->

# Composing Without References

The thing to admire in Rust is that `Mutex`, `Rc`, `Bytes`, every lock-free queue and
every HAL are libraries. The language gives a few mechanisms and crates turn them into
features. Rask closed the door on that on purpose: no stored references, and no
user-defined type gets to run code on assignment, scope exit, or at a borrow boundary
(`mem.shared-rack-heap/BX1`–`BX4`). The reasons are good and I'm not reopening them.

This page asks what's left once those are off the table, and the answer is more than it
looks. Exploration: nothing here is decided, and one piece is a language change.

## Where Rust's composition comes from

Strip a Rust library abstraction down and it's built from four things: a trait with
associated types, monomorphized (Rask has this); `Drop`, so a wrapper can add cleanup
(Rask has no destructors); a reference or guard handed out of a method, with the borrow
checker bounding its life (Rask has `with`, compiler-owned); and `unsafe` plus
`UnsafeCell` to build the primitives (Rask has raw pointers but no hooks).

The three hooks are what `BX` closes. The appendix there gives the reason for each, and
two of them hold without qualification. A copy hook makes `let b = a` able to allocate,
lock or panic, so cost can't be read off the page. A drop hook is a destructor, and
cleanup that nobody wrote on any line is what `ensure` exists to replace.

The third reason is the one I want to look at again. Point 3 of the appendix says a
user-built scoped-access type would need "its own `with` protocol, i.e. user code opening
and closing a borrow region the checker has to trust", and then aliasing safety is
advisory. That conflates two things. Where the view's scope *ends* is decided by the
compiler, whoever wrote the type: the binding dies at the block's close brace, can't be
stored, returned or sent, and is lent, not given (`mem.borrowing/W8`). What a library
could supply is only what runs *around* the block, and that is already what the language
trusts a library about everywhere else: an `unsafe` block's pointer work and the runtime's
own mutex are trusted the same way.

So the three hooks are two hooks and a protocol. The hooks stay closed. The protocol is
the design.

## Rask already composes through scopes and obligations

Rask has three places where a library's code runs at a boundary the compiler enforces,
and all three are visible on the line that uses them:

| Boundary | Rust's mechanism | Rask's mechanism | Who may define one |
|---|---|---|---|
| Access for a bounded extent | `&'a T`, guards | `with x as v { }`, a yield closure lending its item (`type.sequence/SEQ34`) | The compiler, five types |
| Cleanup that must happen | `Drop` | `ensure`, and `@resource` with `take self` (`mem.linear/L1`–`L7`) | Any library |
| Shape decided per type | macros, const generics | `comptime for`, `reflect`, `comptime N` (`ctrl.comptime`) | Any library |

Two of the three are already open. A library defines a protocol with linear types today,
and the compiler enforces it on every path: a `Transfer` that must be waited on or
aborted, a transaction that must commit or roll back, a flash write that must relock.
Rust can only warn (`#[must_use]`), because `mem::forget` is safe there. That is a
composition primitive Rust doesn't have, and it's the one embedded code wants most.

The one that isn't open is scoped access. That's the whole gap.

## The design

### 1. `with` is a protocol

A type that hands out bounded access says so by conforming to an interface the compiler
knows:

<!-- test: skip -->
```rask
interface Access {
    type Item
    func read<R>(self, body: |Item| -> R) -> R
    func write<R>(mutate self, body: |mutate Item| -> R) -> R
}
```

`with cell.write() as v { … }` on such a type desugars to `cell.write(|mutate v| { … })`,
and `with cell.read() as v` to the read form. The body closure never escapes, so it is a
stack closure and the call inlines (`mem.closures/CM1`, the same rule `Sequence` leans
on for its zero-cost chains). `return`, `try`, `break` and `continue` propagate through
the closure exactly as they already do through a `for` body (`type.sequence/SEQ7`,
`SEQ8`): that machinery exists, it's just only wired to one keyword.

What the library writes is ordinary code. Here is the critical-section cell from
[bare-metal.md](bare-metal.md) section 4, as a library instead of a strategy:

<!-- test: skip -->
```rask
struct IrqCell<T> {
    value: T
}

extend IrqCell<T> implements Access {
    type Item = T

    func write<R>(mutate self, body: |mutate T| -> R) -> R {
        let saved = interrupts.disable()      // @resource: must be restored
        ensure saved.restore()
        return body(mutate self.value)
    }

    func read<R>(self, body: |T| -> R) -> R {
        let saved = interrupts.disable()
        ensure saved.restore()
        return body(self.value)
    }
}

const RX: IrqCell<RingBuf> = IrqCell(value: RingBuf.new())

@interrupt(Uart0Rx)
func on_rx() {
    with RX.write() as q { q.push(uart_byte()) }
}
```

Every cost is on a line: the `with` says a region is entered, `disable()` says what
entering costs, and the `ensure` says the restore happens on every exit, panic included
(`ctrl.panic/U1`). Nothing runs at assignment or at scope exit that the reader can't see.

Two things the compiler keeps for itself. Nested `with` on one value is refused the way
it is for `Shared` today (`conc.sync/WS3`): the body closure takes `mutate self.value`
while `self` is borrowed, so a second entry inside the first is the ordinary aliasing
error with no new rule. And `W8` holds: `q` is lent by the closure parameter, so the
body can't hand it to a `take` parameter or store it.

Today this compiles and silently does nothing: `with c.write() as v { v += 1 }` on a
method that returns a plain value binds a copy and drops the write (rask-lang/rask#1658).
The protocol gives that syntax a meaning; until then it should be an error.

### 2. Obligations compose

A library's protocol is a linear type, and today it stops at the first container. `Vec`
can't hold a `@resource` because dropping the vector would have to consume each element
(`mem.resources/RC1`). The fix is the one `Heap` already has: a container of
obligations is an obligation. `Vec<R>` for linear `R` is itself linear, which means it
has to be consumed, which means `take_all()` is the only way out and `pop`, `clear`,
`truncate` and `remove` aren't offered (they'd discard). A method restricted by the
element's linearity is an ordinary bound. Then a driver holds `Vec<Transfer>` and the
compiler still makes it wait on every one.

The second half is tokens. A linear value handed out by one function and consumed by
another is a capability: `interrupts.disable()` above returns one, and `restore` takes
it. A library makes "you may only do this inside a critical section" into a parameter
type, `func drain(q: RingBuf, cs: CriticalSection)`, and the compiler enforces it with
nothing new. Embedded Rust does this with `&CriticalSection<'cs>` and a lifetime; Rask
does it with a value and no lifetime, and the value can't be forgotten.

### 3. Placement: open, and not a conformance

`const RX: IrqCell<RingBuf>` above is refused today: package-level mutable state must
be `Shared` or `Atomic` (`struct.modules/PS2`), because anything else reached from two
tasks races. An earlier draft answered with a marker interface the library conforms to
with `unsafe`, Rust's `unsafe impl Sync`. That's the wrong shape for Rask.
`mem.unsafe/UT3` keeps `unsafe` conformances for contracts other unsafe code relies on.
Placement isn't one: in Rask, safe code has no path to two tasks touching one byte that
doesn't already go through a `Shared`, an `Atomic` or an `unsafe` block, so a marker on
the type would sign for a line that's already signed.

That leaves the question open. The candidate I'd try first keeps the trust where it
already is: the cell's state lives in static storage behind a raw pointer, its methods
take `self`, and the one line in `write` that touches the state is an `unsafe` block,
where the author vouches that every access masks. That needs a way to name static
storage from a `const`, which is [bare-metal.md](bare-metal.md)'s question, and a PS2
that asks about writes through `self` rather than about type names.

### 4. Shape is already open

Register maps, bit fields, fixed-capacity collections and typestate builders are
comptime, annotations and linear `take self -> Next`, all of which exist. They're listed
here only so the table is complete: a Rust HAL's composition is roughly a third traits,
a third `Drop`/references, a third macros and const generics, and Rask has the first and
last thirds today.

## What it buys, in embedded terms

Walking the list in bare-metal.md section 4 and the "where Rask pays more" list:

| Want | Rust | Rask with this |
|---|---|---|
| Interrupt-masking cell | `critical_section::Mutex<T>` + lifetime token | `IrqCell<T>` above: `Access` + linear token; placement open (section 3) |
| Hardware spinlock wrapper | crate, `unsafe impl Sync` | same shape, the `write` body takes and releases the lock in an `unsafe` block |
| ISR-to-main SPSC queue | crate with `UnsafeCell` and atomics | a struct of `Atomic` indices and an array, `Access` for the slots |
| DMA buffer handoff | `embedded-dma` + `'static` | `Transfer` is `@resource`, `wait` consumes it, `Vec<Transfer>` stays linear |
| Peripheral ownership | `Peripherals::take()` at runtime | `@unique` struct handed to `main`, checked at compile time (bare-metal.md §2) |
| Pin typestate | zero-sized generic markers | `take self -> Pin<Output>`; a state left mid-way is a compile error, not a leak |

`Critical` stops being a sixth strategy the compiler has to grow. It's a package.

## What this can't do

Honest list, because the point of the page is the boundary.

- **Return a guard.** Rust can acquire a lock in one function and return the
  `MutexGuard` to its caller. Here the region is a block; access crosses a function only
  downward, as a parameter borrowed for the call. Two locks held together are a two-
  binding `with`, not two guards. This is the design working, and it's still a thing
  you can't have.
- **Share without `Shared`.** Several names reaching one value needs a count or a
  tracer, and the count is the copy hook. A library `Rc<T>` stays impossible, and
  "feels like a value, shares underneath" stays the one kind of library Rask won't
  host. Compose with `Shared`, as `BX3` says.
- **Clean up without being asked.** A wrapper that frees something when it's dropped is
  a destructor. The library's choice is `@resource`, and the user writes the `take`.
- **Mutate through a newtype transparently.** `@reads_as` forwards reads only
  (`type.reads-as/RA4`). A mutating forward is `with inner.write() as v`, one line.
- **Point into yourself.** Intrusive lists and parent pointers go through `Rack`.

Against that, two things Rust can't do: a library protocol here is total, because there
is no `forget` and `ensure` runs on unwind; and every cost a library adds sits on a line
the reader can see, because the only boundaries it can hook are the visible ones.

## Checked against the rulings

Test 1 (cost visible; only the compiler hides it): the library's code runs at `with`,
`ensure` and `take`, each written at the use site. Nothing is added at assignment or
scope exit. Test 2 (the shape of the source decides): `with` on an `Access` type reads
the same as `with` on `Shared`, and means the same. Test 3 (statically decidable is
decided statically): the view's extent and the obligation to consume are both compile
time; the only runtime trust is the `unsafe` block inside the cell, the same trust as any
other pointer work. Test 5 (paths, not survival): the `ensure` in `write` runs on every path
out of the block, so a panic in a critical section restores interrupts.

## What changes if this is accepted

- `mem.shared-rack-heap/BX2` loses "`with`-scoped access" from its list; refcounted copy
  and shared interior stay. Appendix point 3 is rewritten as above. `BX1` keeps the
  five types; what it protects is sharing, not scoping.
- `mem.borrowing` gains the `Access` desugar beside `W1`–`W8`, with `W1` stated as a
  property the desugar preserves rather than a reason closures were rejected.
- `mem.resources/RC1`, `RC3`: a container of a linear type is linear, with the
  discarding methods withheld. Done (`RC5`, rask-lang/rask#1688).
- `struct.modules/PS2` asks about writes through `self` rather than listing types
  (section 3, still open).
- `conc.sync/WS3` and `DL1` generalize from "sync primitives" to `Access` types.

## Still open

- **Should `read` exist on `Access`, or only `write`?** `Shared` has both verbs so a
  reader lock can be cheaper. For a masking cell the two cost the same. One verb is
  simpler; two match `Shared`. I lean two, for `SH5`'s reason: strategy-agnostic code
  shouldn't care.
- **Inline access.** `cell.write().count += 1` is one expression today for `Shared`
  (`mem.borrowing/E5`). The desugar extends to it (`cell.write(|mutate v| v.count += 1)`),
  but the one-store panic guarantee in `E5` depends on the body being one store, which
  the compiler can check only after inlining. Probably fine; needs a prototype.
- **Placement.** Section 3. No marker interface; what PS2 should ask instead isn't
  settled.
- **The scheduler as a library.** Embassy's real trick is that the executor is a crate.
  That's [scheduler-protocol.md](../concurrency/scheduler-protocol.md) now: the runtime
  slot holds a `Scheduler` conformance, and the stack-per-task cost gets its own answer
  there (a growable stack, and a stackless mode for builds with no heap).
