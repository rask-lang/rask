<!-- id: mem.relocatable -->
<!-- status: proposed -->
<!-- summary: Position-addressed state survives serialization, mmap, and snapshots; a graph round-trips but a reference into it is translated -->
<!-- depends: memory/racks.md, memory/value-semantics.md, stdlib/encoding.md, stdlib/reflect.md -->

# Relocatable Memory

Rask's "no storable references" design means user-visible types contain only owned values and references *into a container* — never a bare pointer into someone else's memory. What makes container state relocatable is **position**: a container preserves its slot layout across a round trip, so slot N before is slot N after, and every reference can be written as the slot number it names.

Racks are position-addressed. Every node carries its `slot_index`, the rack keeps a live index→node directory, and address→position→address translation already ships — it is what `snapshot()` runs on.

What position doesn't give you is handing the same reference back. A `Link<T>` is an address, and an address from one allocation names nothing in another. The graph round-trips; a reference you held across the boundary does not.

**Terminology:** "Relocatable" here means *data that can be moved to a different memory address, process, or machine and rebuilt from its own bytes*. This is not the same as position-independent code (PIC/PIE). "Without pointer fixup" was true while every reference was a handle; with links it is true of the flat tier only, and the deep tier pays one linear pass that rewrites indices back into addresses.

**Honest framing:** This isn't impossible elsewhere. GC'd languages can serialize state too. The unique thing is systems-level performance with deterministic cleanup, and a fixup step that is either absent (flat) or one linear pass the container runs for you (deep). Rust can achieve similar results but requires manual `#[repr(C)]` layout management and unsafe pointer-to-offset conversions.

## Workflows unlocked by one API

`to_bytes()` / `from_bytes()` is a single pair of methods, and it backs five workflows without extra primitives. Three of them never hold a reference across the boundary, so they cost nothing. Two do, and they carry an explicit translation step:

| Workflow | How `to_bytes` / `from_bytes` backs it | Carries a translation? |
|----------|----------------------------------------|------------------------|
| Save / load (game state, app state) | Serialize on shutdown, deserialize on startup | No — you load and start from the graph |
| Hot reload | Serialize → recompile → deserialize; schema evolution handles additive changes | No — same shape |
| Process migration | Send bytes over the network; the graph arrives whole | No — nobody carries a reference across machines |
| Undo / redo | Push `to_bytes()` to a history stack; `from_bytes()` on undo | **Yes** — undo hands back a graph, so a link you held is dead and the node has to be found by id (RB3, RB4) |
| Time-travel debugging | Checkpoint state per-tick; rewind by loading a prior checkpoint | **Yes** — same |

The three that don't care are the common ones. The two that do are real ergonomic losses and are named here rather than buried: an undo stack that wants to restore "the node the cursor was on" stores an id field on the node, not a link.

## Relocatability Tiers

Not everything is trivially relocatable. Types fall into three tiers based on their internal structure.

| Rule | Tier | Types | Mechanism | Cost |
|------|------|-------|-----------|------|
| **R1: Flat** | Flat | Primitives, flat structs | Bitwise copy / mmap | Zero |
| **R2: Deep** | Deep | Flat + `string`, `Vec`, `Map` (no resources) | Binary serialization (heap contents traversed) | Linear scan |
| **R3: Opaque** | Opaque | Resource types, closures, `any Trait` | Cannot serialize | N/A |

Closures and `any Trait` contain function pointers — process-local, not serializable. Resource types (`@resource`) have external side effects that can't survive a round-trip. The spec doesn't try to make these relocatable.

**Graphs are R2, and there is no rule that could move them to R1.** Flat means "contains no addresses" and a `Link<T>` is an address, so a node struct of primitives-plus-links is a linear scan, not a bitwise copy. Mmap-a-graph-and-go is not a gap waiting to be closed — it is what the model costs. This is the one capability that was real under handles and isn't under links, and #626's top persistence tier should be read with that in mind.

## Flat Type Constraint

A type is *flat* when it contains no heap-backed fields, recursively.

| Rule | Description |
|------|-------------|
| **FL1: Definition** | A type is flat if all fields are flat, recursively. No `string`, `Vec`, `Map`, `Shared`, `any Trait`, closures, or resource types |
| **FL2: Primitives** | `bool`, `i8`–`i64`, `u8`–`u64`, `f32`, `f64`, `usize` are flat |
| **FL3: References are not flat** | A `Link<T>` is an address, so no struct holding one is flat. The flat tier is primitives and flat structs, full stop. `Handle<T>` used to answer flat — index-plus-generation, no address — which credited the zero-cost tier with graphs it cannot carry; it went out with the pool (rask-lang/rask#908) |
| **FL4: Comptime check** | `reflect.is_flat<T>()` returns `true` if T is flat. Resolved at compile time (`std.reflect/R1`) |
| **FL5: Enums** | An enum is flat if all variant payloads are flat |

<!-- test: skip -->
```rask
import std.reflect

struct GameEntity {
    public id: u32
    public health: i32
    public position: Point3D
}

struct Point3D { public x: f32, public y: f32, public z: f32 }

// Flat — all fields are primitives
let flat = comptime reflect.is_flat<GameEntity>()   // true

struct NamedEntity {
    public id: u32
    public name: string   // heap-backed
}

// Not flat — contains string
let not_flat = comptime reflect.is_flat<NamedEntity>()  // false
```

## No Pointer Fixup Property

| Rule | Description |
|------|-------------|
| **NP1: Position is the identity** | What survives a round trip is a *slot number*, not a reference value. A handle carried its slot number in the open; a link carries one in the node header (`slot_index`). Either way the container preserves slot layout, so slot N before is slot N after |
| **NP2: Addresses never cross a boundary** | No serialized form contains a memory address. A link field is written as its target's slot number and read back as whatever address that slot now has |
| **NP3: Container internal storage** | The container owns its own heap storage. NP2 is about references *into* it, not about the storage itself — `to_bytes()` writes slot contents, never the container's own pointers |

## Rack Binary Serialization

A rack with `T: Encode + Decode` serializes to the same binary format, with one
addition: a link field is written as the target's slot number.

| Rule | Description |
|------|-------------|
| **RB1: Serialize** | `rack.to_bytes() -> Vec<u8>` — walks the directory in slot order, writing each node's payload via binary `Encode` and each `Link<T>?` field as the target's slot number (`none` as the empty slot) |
| **RB2: Deserialize** | `Rack.from_bytes(bytes) -> Rack<T> or DecodeError` — allocates slots in stored index order, then makes a second pass rewriting each stored slot number back to the address that slot now holds, registering the incoming edge as it goes |
| **RB3: The graph survives; a link does not** | `from_bytes()` answers a complete, independent graph with every internal edge re-pointed. A link the caller held before `to_bytes()` names an address in the old allocation and is not valid against the new rack. There is no mechanism that would make it valid, and claiming one would be the dishonest version of this rule |
| **RB4: Naming a node across the boundary** | Give the node an id field and look it up. `corresponding()` translates a link into *another rack in this process* (`mem.racks`), which covers `snapshot()`; it cannot translate into a graph rebuilt from bytes, because the link it would take as input is already stale |
| **RB5: Requires Encode + Decode** | Compile error if `T` does not satisfy `Encode + Decode` |
| **RB6: Deleted slots are gaps** | Only live nodes are written. A deleted slot is recorded as a gap so the surviving slot numbers keep their meaning — this is what NP1 rests on |

The second pass is what a link costs. It is linear in nodes plus edges, runs once
at deserialization, and never touches the per-read cost that made links worth
having in the first place (`mem.racks`, "The cost, stated"). Nothing here needs an
id-assignment pass invented for it: the slot number already exists, is already
maintained on every insert and delete, and is already what `snapshot()` translates
through.

**Not costed.** `to_bytes`/`from_bytes` is unimplemented for racks as of this
writing, the edge-rebuild pass has not been measured, and the interaction with
`@resource` nodes and `Encode`'s existing schema descriptor is unexamined. This
section is the shape of the answer, not a report on one.

## Schema Evolution

Stored bytes carry a schema descriptor — field names and types, from
`reflect.fields<T>()` at comptime — so a type that gained or lost a field since
the bytes were written still reads back.

| Rule | Description |
|------|-------------|
| **SE1: Field matching** | On `from_bytes()`, fields are matched by name using the embedded schema descriptor |
| **SE2: Added fields** | Fields present in the current type but absent in the stored schema get their `@default` value, or their declared default (`std.encoding/E20`, `type.structs/FD6`). A field with neither is a compile error naming it |
| **SE3: Removed fields** | Fields present in the stored schema but absent in the current type are skipped |
| **SE4: Type mismatch** | If a field exists in both schemas but the type changed, `from_bytes()` returns `DecodeError` |


These were written for the pool's `to_bytes`/`from_bytes`, which went out with
the pool (rask-lang/rask#908). The rules are the format's, not the
container's, so they carry over to RB1/RB2 unchanged.

## Memory-Mapped Containers (Flat Types Only)

For flat types, a container's storage can be memory-mapped directly — no
serialization step.

| Rule | Description |
|------|-------------|
| **MM0: A rack is never mmappable** | Not even with a flat payload. A rack node carries a header — its rack, its incoming-edge list, its slot number — and the first two are addresses, so the storage is not an image that can be mapped back in. Graphs take the RB1/RB2 path, always. This is R1 losing graphs, said in terms of the operation that noticed |
| **MM1: Flat constraint** | `from_mmap(path)` / `to_mmap(path)` require the element type to be flat (`FL1`). Compile error otherwise. The receiver went with the pool; the operation is waiting on a container to sit on |
| **MM2: Bitwise layout** | An mmap'd container uses the type's in-memory layout directly. No encode/decode step |
| **MM3: Platform constraint** | Mmap files are valid only on the same platform (same endianness, same alignment). Not cross-platform by default |
| **MM4: Compile error message** | When T is not flat, the error must identify which field is heap-backed and suggest `to_bytes()` as the alternative |

**MM4 error format:**

```
ERROR [mem.relocatable/MM1]: cannot mmap a container of NamedEntity — type is not flat
   |
5  |  particles.to_mmap("save.bin")
   |            ^^^^^^^ NamedEntity contains heap-backed fields
   |
3  |  struct NamedEntity {
4  |      public name: string    ← string owns heap memory
   |

WHY: Memory mapping requires flat types (no heap pointers). The mmap file is a
     direct image of memory — heap pointers would be meaningless.

FIX: Use to_bytes() for types with heap-backed fields:

  let bytes = try particles.to_bytes()
  try fs.write("save.bin", bytes)
```

<!-- test: skip -->
```rask
struct Particle {
    public x: f32
    public y: f32
    public vx: f32
    public vy: f32
    public life: f32
}

func save_particles(particles: Vec<Particle>) -> void or IoError {
    try particles.to_mmap("particles.bin")
}

func load_particles() -> Vec<Particle> or IoError {
    return try Vec.from_mmap("particles.bin")
}
```

## Error Messages

**Non-encodable node [RB1]:**
```
ERROR [mem.relocatable/RB1]: cannot serialize a rack of Connection
   |
5  |  world.to_bytes()
   |        ^^^^^^^^^ Connection is not Encode
   |
3  |  struct Connection {
4  |      public socket: Socket    ← Socket is not Encode
   |

WHY: to_bytes() requires the node type to be Encode + Decode.

FIX: Mark non-serializable fields @no_serialize, or use @no_encode and implement
     custom serialization.
```

**Schema type mismatch [SE4]:**
```
ERROR [mem.relocatable/SE4]: schema mismatch in from_bytes()

  Field "health" changed type: stored as f32, current type is i32

WHY: Binary format embeds field types. Changing a field's type between
     serialization and deserialization is not automatically convertible.

FIX: Add a migration step, or keep the old field and add a new one.
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Empty rack `to_bytes()` | RB1 | Valid — produces header and no nodes |
| `from_bytes()` with corrupted data | RB2 | Returns `DecodeError` |
| Flat struct with `@unique` annotation | FL1 | Still flat — `@unique` affects copy semantics, not memory layout |
| A link held across a round trip | RB3 | Dangles — the graph survives, the reference doesn't. Find the node by an id it declares |
| Mmap file from different platform | MM3 | Undefined — no cross-platform guarantee |
| A rack with a flat node type | MM0 | Still not mmappable — the node header holds addresses |

---

## Appendix (non-normative)

### Rationale

**R1–R3 (tiers):** I wanted to be upfront about what's actually relocatable. Every game dev will try `Entity { name: string }` with mmap and hit the wall. Being honest about the tiers prevents frustration. Flat types get the zero-cost path; deep types get the linear-scan path; opaque types don't pretend to work.

**FL3 / RB3 (what a link costs):** handles looked like they made persistence work because they're integers. They didn't — they made it work because they're *positions*, and racks are position-addressed too. So links need no new machinery here, just a second pass at deserialization. What a link does cost is the reference-level promise: a handle was a slot number the caller held, so it could be handed straight back after a round trip; a link is an address and cannot be.

They do cost two things, and I'd rather write them down than round them off. A link you held before the round trip is dead afterwards, so undo/redo and time-travel debugging carry a step that handles didn't need. And a graph can never be flat, so mmap-a-graph-and-go is gone — not deferred, gone, because flat means "no addresses" and a link is one. I'm taking both. Per-read speed is what these types exist for, and paying for it once at serialization time, in the one place a program is already writing every byte it owns, is the right end to pay at.

**FL1–FL4 (flat constraint):** I considered a `Relocatable` trait but it would duplicate `Copy` for flat types and `Encode + Decode` for deep types. `reflect.is_flat<T>()` at comptime is simpler — it's a query, not a type-system concept. The compiler already knows the layout; just expose that knowledge.

**SE1–SE4 (schema evolution):** Field-by-field matching by name gives forward/backward compatibility for free on additive changes. Type changes are intentionally an error — silent coercion between `f32` and `i32` would be a bug factory. If you need a migration, write one explicitly.

**MM1–MM4 (mmap):** Mmap is genuinely useful for particle systems, terrain data, and other flat-data workloads. But it's niche — most real structs have at least one `string` field. The error message quality matters more than the feature itself, because developers will hit the compile error and need to understand why.

**NP3 (container internal storage):** A common confusion: "if nothing serialized holds an address, how does the container store data?" Its storage is heap-allocated and it owns it. NP2 is about what crosses the boundary — a reference into the container is written as a slot number — not about how the container holds its own memory.

### Patterns & Guidance

**Point back into a restored graph with an id.** This is what RB3 costs, and the
only thing about these workflows that isn't in the rules. `from_bytes` answers
*a* graph, not *the* graph, so a link stored across the round trip is dead:

<!-- test: skip -->
```rask
struct Editor {
    nodes: Rack<Node>
    cursor: u32          // a node id, not a Link<Node> — survives the round trip
}
```

**Hot reload** goes through field-by-field `Encode`/`Decode`, not a bitwise copy,
so a layout change between compilations is handled by schema evolution (SE1–SE3)
rather than corrupting the read.

### See Also

- [Racks and Links](racks.md) — `slot_index`, the node directory, `snapshot()` and `corresponding()` (`mem.racks`)
- [Value Semantics](value-semantics.md) — Copy vs move, 16-byte threshold (`mem.value`)
- [Linearity](linear.md) — Why linear values are the Tier-3 opaque case (`mem.linear`)
- [Shared, Rack and Heap](shared-rack-heap.md) — Their relocatability tiers (`mem.shared-rack-heap`)
- [Resource Types](resource-types.md) — Why resources are Tier-3 opaque (`mem.resources`)
- [Encoding](../stdlib/encoding.md) — `Encode`/`Decode` traits, field annotations (`std.encoding`)
- [Reflect](../stdlib/reflect.md) — `reflect.is_flat<T>()`, comptime type introspection (`std.reflect`)
