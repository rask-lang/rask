<!-- id: analysis.bare-metal -->
<!-- status: proposed -->
<!-- summary: Exploration, nothing decided. Can Rask reach registers, DMA, interrupts and bare-metal targets without storable references, and without making everything unsafe? Six sketches, mostly built from rules Rask already has -->
<!-- depends: memory/value-semantics.md, memory/linear.md, memory/resource-types.md, memory/shared-rack-heap.md, memory/borrowing.md, memory/unsafe.md, compiler/effects.md, stdlib/strings.md -->

# Low-Level Rask Without Stored References

`CORE_DESIGN.md` sends kernels and drivers to C and Rust. This doc asks whether that
has to be true. It's exploration: nothing here is decided, and none of it is built.

The worry is that low-level code is made of pointers, and Rask won't let you keep one.
But the no-stored-references rule exists because a reference can outlive what it points
at. Most of the memory a driver touches can't do that. A device register belongs to the
hardware, not the program, and is never freed. A DMA buffer changes hands at moments the
program can name. So the question is narrower than it looks: which of those can be a
plain value, and which need a new kind of ownership?

My answer so far: almost all of it is a value or a move, and `unsafe` shrinks to one
line per device.

## 1. A register is a value, and volatile is its type

A register is an address the hardware watches. Writing it sends a byte; reading it asks
"is the transmitter free". The compiler assumes memory only changes when the program
writes it, so it would read a status register once and spin forever, or merge two writes
into one. A *volatile* access forbids that: every read and write happens, in order.

C and Zig put volatile on the pointer (`volatile uint32_t *`, `*volatile u32`), so
every access through it is right and every access through a plain pointer is a bug
nobody sees. I'd put it on the type instead. `Reg<T, A>` is an address plus a value
type and an access mode, and its only methods are volatile:

<!-- test: skip -->
```rask
@device
@unique
struct Uart0 {
    @offset(0x00) data: Reg<u32, WriteOnly>
    @offset(0x18) status: Reg<UartStatus, ReadOnly>
    @offset(0x30) ctrl: Reg<UartCtrl>
}

@binary
struct UartStatus {
    busy: u1
    rx_full: u1
    tx_full: u1
    _: u29
}

func put(uart: Uart0, byte: u8) {
    while uart.status.read().tx_full == 1 { }
    uart.data.write(byte as u32)
}
```

`Reg` is a value. It's an address, it copies, it never dangles. Reading one isn't
`unsafe` either. The one unsafe fact is that this address really is a UART laid out
like this, and that's stated where the device is made (section 2), not at every access.

`@binary` already describes bit layouts, so a register's fields can be one.
`ctrl.modify(|c| c.with(enable: 1))` is a read, a change and a write, as in Rust's
svd2rust. Vendors ship register maps as SVD files, and a `build.rk` step can turn one into
a module of `@device` structs. That's a generator writing source, not a macro.

## 2. A device has one owner

Two drivers writing the same UART is the real bug, and it isn't a memory-safety bug, so
`unsafe` doesn't catch it. Embedded Rust makes it a runtime check: `Peripherals::take()`
returns the devices the first time and `None` after.

Rask can make it an ownership fact. `@unique` already makes a small struct move-only
(`mem.value-semantics/U1`). A device struct holds `Reg` fields, which aren't
`Cloneable`, so `.clone()` isn't available either (U2). That leaves one copy, and moving
it is the only way to hand it on. Where the one copy comes from is the entry function:

<!-- test: skip -->
```rask
@entry
func main(take dev: Devices) {
    let Devices(uart0, gpio, ..) = dev
    let console = Console.new(take uart0)    // the console owns the UART now
    blink(take gpio)
}
```

The startup code builds `Devices` once, in the generated board module, with
`unsafe { Uart0.at(0x4000_C000) }` for each device. That's the only unsafe line in the
picture. Since nothing else can build one, the first-call check happens at compile time
and costs nothing at runtime.

## 3. DMA is a move

With DMA the hardware writes into a buffer while the program keeps running. The danger is
touching the buffer before it's done, or freeing it while the hardware is still writing.
Both are ownership questions:

<!-- test: skip -->
```rask
let transfer = rx_dma.start(take buf)    // the hardware owns buf now
ensure transfer.abort()                  // runs only if we never got buf back
do_other_work()
let buf = transfer.wait()                // consumes the transfer, cancels the ensure
```

`Transfer` is `@resource`, so it has to be finished exactly once. The rule that the
statement after an acquisition commits it (`mem.linear/L7`) forces the `ensure` line, so
a panic in `do_other_work` stops the hardware before the buffer is freed. `wait` consumes
the transfer, and that cancels the `ensure` (L6).

Rust can't force this. `mem::forget` is safe there, so a forgotten transfer leaves the
hardware writing into freed memory. That's why the embedded-dma crate requires `'static`
buffers. Rask has no forget.

Two things a type has to rule out:
- **A buffer that moves.** Moving a `Vec` moves its header, and the bytes stay put. Moving
  a `[u8; 64]` copies the bytes to a new address, behind the hardware's back. So `start`
  takes heap-backed storage only: `Vec<u8>`, `Heap<[u8; N]>`, or a static region.
- **Caches.** On chips with a data cache, the CPU and the DMA engine see different
  memory until someone flushes or invalidates. `start` and `wait` are the two points where
  that has to happen, so the driver writes it once and callers can't skip it.

## 4. Interrupt handlers

An interrupt handler runs between any two instructions of the main program. Two problems
follow: what the handler may do, and how it shares data with the code it interrupted.

What it may do is an effects question, and effect inference already exists:

<!-- test: skip -->
```rask
@interrupt(Uart0Rx)
func on_rx() {
    // compile error if anything reachable from here blocks, spawns or allocates
}
```

A handler must not wait on anything (`blocks`), start tasks (`async`) or allocate. The
first two come straight from `comp.effects`, and allocation is what `@no_alloc` already
checks. A handler that calls something which, three calls down, takes a lock is a compile
error that names the call chain.

Shared data is a `Shared` strategy. Package-level mutable state already has to be
`Shared` or `Atomic` (`structure.modules/PS2`). A `Critical` strategy masks interrupts for
the length of a `with` block:

<!-- test: skip -->
```rask
const RX: Shared<RingBuf, Critical> = Shared.new(RingBuf.new())

@interrupt(Uart0Rx)
func on_rx() {
    with RX.write() as q { q.push(uart_byte()) }
}

func next_byte() -> u8? {
    with RX.write() as q { return q.pop() }     // interrupts off for these lines only
}
```

A `with` block can't hand out a reference past its end, so no code can reach the buffer
with interrupts on, and a critical section can't be left open by mistake. Blocking is
already forbidden inside one, so it can't deadlock. This adds a strategy to the closed set
in `mem.shared-rack-heap/BX1`. It's compiler-provided like the others, not a hatch for
users.

Later, the compiler could do what RTIC does: it sees which handlers touch which `Shared`,
so it could mask only the interrupts that can actually collide instead of all of them.

## 5. A build with no runtime

Today the runtime owns `main`, schedules tasks and talks to the OS. A bare-metal build has
none of that. I'd make it a build setting (`runtime = "none"` in `rask.toml`) and let the
existing checks say what breaks:

- **Tasks:** anything with the `async` effect is an error. There's no scheduler to run it.
- **OS modules:** `fs`, `net` and `os` aren't there to import.
- **Heap:** optional. Without one, `@no_alloc` applies to the whole build, and `Vec`,
  `Map` and `string` go with it. That leaves arrays, structs and enums, and it needs a
  fixed-capacity collection that doesn't exist yet (`Vec<T, max: 64>` or similar). That
  one is real new design.
- **Panics:** with no task to kill, a panic goes to a function the program names, which
  usually halts or resets the chip.
- **Printing:** `println` needs somewhere to write. The program names the sink, typically
  the UART from section 2.

The effects framework does the hard part: whether a function can run here is already a
fact the compiler knows.

## 6. Bytes without copies

Rask has no slice type. Taking part of a buffer means `.to_vec()`, which copies, or
passing indices. Two separate needs hide in that.

**Borrowing a range for a call.** `crc(buf[10..20])` should pass those ten bytes without
copying. That's sound as it stands: a parameter is borrowed for the call
(`mem.borrowing/B3`), and the caller can't grow `buf` while it's suspended in the call. So
a range of a collection can be a parameter type, borrowed exactly as long as any other
parameter. It can't be stored, for the same reason nothing borrowed can.

**Keeping a piece.** A network parser wants to return "the header is bytes 10–20" and
keep it. Strings already solve this: `StringView` shares the source buffer and holds a
refcount on it, so it can be stored and never dangles (`std.strings/V1`). A `bytes`
primitive built the same way, immutable and refcounted with storable views, would give byte
data what strings have. That's the model of Go's slices and of Rust's `bytes` crate, which
networking code in Rust uses for exactly this. The cost is an atomic refcount operation per
view, the same one strings pay, and the same elision applies when the compiler can see one
owner.

Rask's own compiler does a third thing: an AST node keeps a `Span`, offsets into the
source, and looks text up when it needs it. That works today and is often the right answer.

## What's not here

Inline assembly has a design already (`mem.unsafe/ASM1–ASM5`). It needs building, not
designing. The same goes for the bugs and inert attributes the audit found (`transmute`,
`@packed`, `size_of`).

## Open questions

- **Bit order.** `@binary` is MSB-first, for wire formats. Registers number bits from the
  least significant end. Either `@binary` takes an order or registers need their own layout.
- **Multi-core.** Masking interrupts protects one core. Two cores need a spinlock, so
  `Critical` would mean different things per target. Is that one strategy or two?
- **Is `Reg` a stdlib type?** It can be written in Rask over unsafe volatile reads and
  writes on raw pointers, which would be the only new primitive. That keeps it out of the
  closed set.
- **Fixed-capacity collections.** Without a heap, what replaces `Vec` and `string`, and do
  they share their interfaces with the heap versions?
- **Where the core design moves.** If these hold up, the "consider alternatives" line in
  `CORE_DESIGN.md` changes. That's for after a prototype, not now.
