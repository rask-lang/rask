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
nobody sees. I'd put it on the type instead. `Reg<T>` is an address plus a value type,
and its only methods are volatile. `ReadReg<T>` and `WriteReg<T>` are the one-way kinds:

<!-- test: skip -->
```rask
@device
@unique
struct Uart0 {
    @offset(0x00) data: WriteReg<u32>
    @offset(0x18) status: ReadReg<UartStatus>
    @offset(0x30) ctrl: Reg<UartCtrl>
}

@register(u32)
struct UartStatus {
    @at(bit: 3) busy: u1
    @at(bit: 4) rx_full: u1
    @at(bit: 5) tx_full: u1
}

func put(uart: Uart0, byte: u8) {
    while uart.status.read().tx_full == 1 { }
    uart.data.write(byte as u32)
}
```

`Reg` is a value: an address that never dangles, because nothing frees a device.
Reading one isn't `unsafe` either. The one unsafe fact is that this address really is a UART laid out
like this, and that's stated where the device is made (section 2), not at every access.

A register's bits get a layout of their own (`@register`, see the second pass).
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
owner. Accepted as a direction and written up as [std.bytes](../stdlib/bytes.md).

Rask's own compiler does a third thing: an AST node keeps a `Span`, offsets into the
source, and looks text up when it needs it. That works today and is often the right answer.

## What's not here

Inline assembly has a design already (`mem.unsafe/ASM1–ASM5`). It needs building, not
designing. The same goes for the bugs and inert attributes the audit found (`transmute`,
`@packed`, `size_of`).

## Second pass

The first draft ended on open questions. These are my answers to the ones that have one.

### Registers get their own layout

`@binary` describes a byte stream: fields in order, most significant bit first, which is
how wire formats are written. A register is a word with named bit positions, and the
datasheet numbers them from the least significant end ("bits 3:1, MODE"). Reusing
`@binary` with a bit-order flag would make the reader count widths to find a field, and
one wrong width shifts every field after it without an error.

So registers get `@register`, where each field says where it starts and its type says
how wide it is. That's the datasheet's own shape:

<!-- test: skip -->
```rask
@register(u32)
struct UartCtrl {
    @at(bit: 0) enable: u1
    @at(bit: 1) mode: u3
    @at(bit: 8) baud_div: u12
}
```

The compiler checks that fields don't overlap and fit the word. Gaps are reserved bits.
`modify` reads the register first, so reserved bits keep their value. `write` sends zero
for them, which is what most datasheets ask for. A register that needs something else
writes it explicitly.

### `Reg` is stdlib code, and move-only

The only new primitive is volatile access on a raw pointer, in `unsafe`:
`p.read_volatile()` and `p.write_volatile(v)`. `Reg` is a Rask struct holding the
address, with its unsafe calls inside its methods. Building one is `unsafe`, since that's
where the "this address is this register" claim is made. Using one is safe.

`Reg` has to be `@unique`. It's 8 bytes, so without that it would copy implicitly, and
`let r = uart.data` would leave two handles on one register and undo section 2.
Read-only and write-only registers are separate types (`ReadReg<T>`, `WriteReg<T>`),
not a mode parameter. A method that doesn't exist reads better than one that fails a
bound.

### `Critical` means one thing on every target

The strategy's promise is that nothing else that can touch the value runs during the
block. On one core that's masking interrupts. On several it's masking plus a spinlock.
The program says what it needs and the target decides how, the same way `Shared` already
picks a lock implementation per platform.

The spinlock is less alarming than it sounds. Blocking is already a compile error inside
a critical section and inside a handler (section 4), so the only thing a core can wait on
is another core's critical section, and those are short and never wait themselves.

### Fixed-capacity collections

Without a heap, `Vec` needs a version with its storage inline. I'd make it the same type
with a capacity in it, `Vec<T, 64>`, not a new name: the same methods, so code written
against `Vec<T>` reads the same against the fixed one. `push` on a full vector panics,
the same as indexing out of range, and `try_push` returns `void or Full` for code where a
full buffer is normal, like an interrupt handler filling a ring.

Strings mostly survive without a heap: literals are static, and anything up to 15 bytes
is inline (`std.strings/S8`). Interpolation is what allocates, so `println` with a
no-heap build formats straight into the sink instead of building a string first.

### A slice parameter

Section 6's "borrow a range for a call" needs a type to write. `[T]` is a run of `T`
somewhere else, and it's only allowed as a parameter type:

<!-- test: skip -->
```rask
func crc32(data: [u8]) -> u32 { ... }

crc32(buf[10..20])          // part of a Vec
crc32(header)               // a whole [u8; 16] array
crc32(packet[3..])          // part of a bytes
```

It's the borrow every parameter already is (`mem.borrowing/B3`), with the range carried
along. `mutate data: [u8]` lets the callee change elements but not the length. It can't
be a field, a `let` or a return type, for the same reason nothing borrowed can be. One
function then serves a `Vec`, an array and `bytes`. Today that takes three functions or a
copy.

## Still open

- **Who builds `Devices`.** The board module is generated from an SVD file and holds the
  only `unsafe` lines. Is generation a `build.rk` step every project repeats, or a
  package per chip, like Rust's peripheral access crates?
- **Panics on bare metal.** A panic handler that resets the chip is the usual answer. But
  section 4's effect checks could go further: an interrupt handler that can panic is
  worth a warning, because a reset mid-interrupt is the hardest bug to find.
- **`[T]` versus `bytes` views.** A `BytesView` is a storable range of `bytes`; a `[u8]`
  parameter is a borrowed range of anything. That's two range types. I think both are
  needed, since one can be stored and one can point into a `Vec`, but it's worth a
  harder look.
- **Where the core design moves.** If these hold up, the "consider alternatives" line in
  `CORE_DESIGN.md` changes. That's for after a prototype, not now.
