<!-- id: mem.relocatable -->
<!-- status: proposed -->
<!-- summary: Position-addressed state survives serialization, mmap, and snapshots; a graph round-trips but a reference into it is translated -->
<!-- depends: memory/racks.md, memory/value-semantics.md, stdlib/encoding.md, stdlib/reflect.md -->

# Relocatable Memory

Rask's "no storable references" design means user-visible types contain only owned values and references *into a container* — never a bare pointer into someone else's memory. That's what makes container state relocatable, and it's worth being precise about why, because this page used to give the wrong reason.

The old sentence said handles survive a round trip "because they're integers, not addresses." Integer-ness was how it was implemented, not the property. What the promise actually rested on is position: a handle is *a slot number plus a version stamp*, and `to_bytes` preserves the slot layout, so slot N still means slot N afterwards. Racks have slot numbers too — every node carries its `slot_index`, the rack keeps a live index→node directory, and address→position→address translation already ships as the thing `snapshot()` runs on. So the mechanism survives the move from handles to links.

What doesn't survive is handing the same reference back. A handle was a slot number you could keep; a link is an address, and an address from one allocation names nothing in another. The graph round-trips either way — a reference you held across the boundary does not.

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
| **FL3: References are not flat** | No reference into a container is flat. `Link<T>` is an address, so it never is, and nothing replaces `Handle<T>` in this tier when handles go (`mem.racks`, rask-lang/rask#908). The flat tier is primitives and flat structs, full stop |
| **FL3a: Handles, while they last** | `Handle<T>` and `WeakHandle<T>` still answer flat, because they are still index-plus-generation and `Pool` still ships. This is the deprecated half of FL3 and goes out with `mem.pools`; write nothing new that depends on it |
| **FL4: Comptime check** | `reflect.is_flat<T>()` returns `true` if T is flat. Resolved at compile time (`std.reflect/R1`). It cannot answer for a type holding a `Link<T>?` today — that is a generic instantiation and the walk reads the declaration's type parameters instead (rask-lang/rask#791). FL3 says what the answer will be |
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
| **RB5: Requires Encode + Decode** | Compile error if `T` does not satisfy `Encode + Decode`, same as PB4 |
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

## Pool Binary Serialization (deprecated)

> Superseded by the rack rules above, and going out with `mem.pools`. Kept because
> `Pool` still ships; see rask-lang/rask#908.

Pools with `T: Encode + Decode` can serialize to and from a compact binary format.

| Rule | Description |
|------|-------------|
| **PB1: Serialize** | `pool.to_bytes() -> Vec<u8>` — serializes all occupied slots via binary `Encode` |
| **PB2: Deserialize** | `Pool.from_bytes(bytes) -> Pool<T> or DecodeError` — reconstructs pool from bytes |
| **PB3: Handle preservation** | Handles obtained before `to_bytes()` are valid against the pool returned by `from_bytes()`. Same index, same generation. This is the reference-level promise that RB3 replaces with a graph-level one — a handle is a slot number the caller holds, so it can be handed back; a link cannot |
| **PB4: Requires Encode + Decode** | Compile error if `T` does not satisfy `Encode + Decode` |
| **PB5: Empty slots skipped** | Only occupied slots are serialized. Removed slots (generation bumped, no data) are recorded as gaps |

### Binary Format

The binary format embeds a schema descriptor for forward/backward compatibility.

| Section | Contents |
|---------|----------|
| Header | Magic bytes, format version, element count, schema descriptor |
| Schema descriptor | Field names + types, derived from `reflect.fields<T>()` at comptime |
| Generation array | Per-slot generation counters (occupied and empty) |
| Slot data | Occupied slots serialized via binary `Encode`, in index order |

### Schema Evolution

| Rule | Description |
|------|-------------|
| **SE1: Field matching** | On `from_bytes()`, fields are matched by name using the embedded schema descriptor |
| **SE2: Added fields** | Fields present in the current type but absent in the stored schema get their `@default` value, or their declared default (`std.encoding/E20`, `type.structs/FD6`). A field with neither is a compile error naming it |
| **SE3: Removed fields** | Fields present in the stored schema but absent in the current type are skipped |
| **SE4: Type mismatch** | If a field exists in both schemas but the type changed, `from_bytes()` returns `DecodeError` |

<!-- test: skip -->
```rask
struct Player {
    public id: u32
    public health: i32

    @default(0)
    public score: i64       // added after initial release — old data gets 0
}

func save_state(pool: Pool<Player>) -> Vec<u8> or EncodeError {
    return pool.to_bytes()
}

func load_state(bytes: Vec<u8>) -> Pool<Player> or DecodeError {
    return Pool.from_bytes(bytes)
}
```

### Handle Round-Trip

<!-- test: skip -->
```rask
func test_handle_roundtrip() -> void or Error {
    let pool = Pool.new()
    let h = pool.insert(Player { id: 1, health: 100, score: 0 })

    let bytes = try pool.to_bytes()
    let restored = try Pool.from_bytes(bytes)

    // h is still valid — same index, same generation
    assert(restored[h].id == 1)
    assert(restored[h].health == 100)
}
```

## Memory-Mapped Containers (Flat Types Only)

For flat types, a container's storage can be memory-mapped directly — no
serialization step.

| Rule | Description |
|------|-------------|
| **MM0: A rack is never mmappable** | Not even with a flat payload. A rack node carries a header — its rack, its incoming-edge list, its slot number — and the first two are addresses, so the storage is not an image that can be mapped back in. Graphs take the RB1/RB2 path, always. This is R1 losing graphs, said in terms of the operation that noticed |
| **MM1: Flat constraint** | `Pool.from_mmap(path)` and `pool.to_mmap(path)` require `T` to be flat (`FL1`). Compile error otherwise. Deprecated with `mem.pools`; the operation survives the retirement, the receiver does not |
| **MM2: Bitwise layout** | Mmap'd pools use the type's in-memory layout directly. No encode/decode step |
| **MM3: Platform constraint** | Mmap files are valid only on the same platform (same endianness, same alignment). Not cross-platform by default |
| **MM4: Compile error message** | When T is not flat, the error must identify which field is heap-backed and suggest `to_bytes()` as the alternative |

**MM4 error format:**

```
ERROR [mem.relocatable/MM1]: cannot mmap Pool<NamedEntity> — type is not flat
   |
5  |  pool.to_mmap("save.bin")
   |       ^^^^^^^ NamedEntity contains heap-backed fields
   |
3  |  struct NamedEntity {
4  |      public name: string    ← string owns heap memory
   |

WHY: Memory-mapped pools require flat types (no heap pointers). The mmap file
     is a direct image of memory — heap pointers would be meaningless.

FIX: Use pool.to_bytes() for types with heap-backed fields:

  let bytes = try pool.to_bytes()
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

func save_particles(pool: Pool<Particle>) -> void or IoError {
    try pool.to_mmap("particles.bin")
}

func load_particles() -> Pool<Particle> or IoError {
    return try Pool.from_mmap("particles.bin")
}
```

## Error Messages

**Non-encodable pool element [PB4]:**
```
ERROR [mem.relocatable/PB4]: cannot serialize Pool<Connection>
   |
5  |  pool.to_bytes()
   |       ^^^^^^^^^ Connection is not Encode
   |
3  |  struct Connection {
4  |      public socket: Socket    ← Socket is not Encode
   |

WHY: pool.to_bytes() requires T: Encode + Decode.

FIX: Mark non-serializable fields @no_serialize, or use @no_encode and implement
     custom serialization.
```

**Schema type mismatch [SE4]:**
```
ERROR [mem.relocatable/SE4]: schema mismatch in Pool.from_bytes()

  Field "health" changed type: stored as f32, current type is i32

WHY: Binary format embeds field types. Changing a field's type between
     serialization and deserialization is not automatically convertible.

FIX: Add a migration step, or keep the old field and add a new one.
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Empty pool `to_bytes()` | PB1 | Valid — produces header + empty slot data |
| `from_bytes()` with corrupted data | PB2 | Returns `DecodeError` |
| Flat struct with `@unique` annotation | FL1 | Still flat — `@unique` affects copy semantics, not memory layout |
| Pool with generation overflow slots | PB5 | Dead slots recorded in generation array, no data serialized |
| Mmap file from different platform | MM3 | Undefined — no cross-platform guarantee |
| `Handle<T>` where T has different layout | PB3 | Handle is valid if schema evolution succeeds (SE1–SE3) |
| Pool<T> where T: Encode but not Decode | PB4 | `to_bytes()` works; `from_bytes()` is compile error |
| Bounded pool `from_bytes()` exceeding capacity | PB2 | Returns `DecodeError` if element count exceeds capacity |

---

## Appendix (non-normative)

### Rationale

**R1–R3 (tiers):** I wanted to be upfront about what's actually relocatable. Every game dev will try `Entity { name: string }` with mmap and hit the wall. Being honest about the tiers prevents frustration. Flat types get the zero-cost path; deep types get the linear-scan path; opaque types don't pretend to work.

**FL3 / RB3 (what a link costs):** The first framing of this page credited handles with a property they got from being *positions*, not from being integers, and then the arrival of links looked like it destroyed the property. It doesn't. Racks already carry slot numbers, already keep an index→node directory, and already translate address→position→address — that is what `snapshot()` is. So `to_bytes`/`from_bytes` needs no new machinery, just a second pass.

What it genuinely costs is two things, and I'd rather write them down than round them off. A link you held before the round trip is dead afterwards, so undo/redo and time-travel debugging carry a translation step that handles didn't need. And a graph can never be in the flat tier, so mmap-a-graph-and-go is gone — not deferred, gone, because flat means "no addresses" and a link is one. I'm taking both. The per-read cost is what these types exist for, and paying it back at serialization time — once, linearly, in the one place a program is already writing every byte it owns — is the right end to pay it at.

**FL1–FL4 (flat constraint):** I considered a `Relocatable` trait but it would duplicate `Copy` for flat types and `Encode + Decode` for deep types. `reflect.is_flat<T>()` at comptime is simpler — it's a query, not a type-system concept. The compiler already knows the layout; just expose that knowledge.

**PB1–PB5 (pool serialization):** `pool.to_bytes()` with `T: Encode + Decode` handles the common case (deep types). The binary format with schema descriptors means you don't need manual migration code for additive changes — added fields get defaults, removed fields are skipped. This covers the 80% case of evolving game state, configuration, caches.

**SE1–SE4 (schema evolution):** Field-by-field matching by name gives forward/backward compatibility for free on additive changes. Type changes are intentionally an error — silent coercion between `f32` and `i32` would be a bug factory. If you need a migration, write one explicitly.

**MM1–MM4 (mmap):** Mmap is genuinely useful for particle systems, terrain data, and other flat-data workloads. But it's niche — most real structs have at least one `string` field. The error message quality matters more than the feature itself, because developers will hit the compile error and need to understand why.

**NP3 (pool internal storage):** A common confusion: "if there are no pointers, how does the pool store data?" The pool's internal slot array is heap-allocated — it owns the memory. The no-pointer property applies to handles that *reference into* the pool. The pool manages its own storage; handles are just integer keys into that storage.

### Patterns & Guidance

**State snapshot (undo/redo).** This is one of the two workflows that pays for links (RB3): `pop()` answers a graph, not the graph, so anything that wants to point back into it stores an id and looks it up.



<!-- test: skip -->
```rask
struct UndoStack<T: Encode + Decode> {
    public history: Vec<Vec<u8>>
    public max_entries: usize
}

extend UndoStack<T: Encode + Decode> {
    func push(mutate self, rack: Rack<T>) -> void or EncodeError {
        if self.history.len() >= self.max_entries {
            self.history.remove(0)
        }
        self.history.push(try rack.to_bytes())
    }

    func pop(mutate self) -> Rack<T> or DecodeError {
        let bytes = self.history.pop() ?? return DecodeError.Empty
        return Rack.from_bytes(bytes)
    }
}
```

Restoring "where the cursor was" is an id, not a link:

<!-- test: skip -->
```rask
struct Editor {
    nodes: Rack<Node>
    cursor: u32          // a node id, not a Link<Node> — survives the round trip
}
```

**Hot code reloading:**

Serialize state → recompile → deserialize. Uses field-by-field `Encode`/`Decode` (not bitwise), so layout changes between compilations are handled by schema evolution (SE1–SE3). Added fields get defaults, removed fields are skipped.

<!-- test: skip -->
```rask
func hot_reload(state: Rack<GameState>) -> Rack<GameState> or Error {
    let bytes = try state.to_bytes()
    // ... recompile happens here ...
    return try Rack.from_bytes(bytes)
}
```

**Process migration:**

Send `to_bytes()` over the network. The receiver rebuilds the graph with every internal edge re-pointed (RB2). Nobody carries a reference across a machine boundary, so this is one of the three workflows RB3 costs nothing.

<!-- test: skip -->
```rask
func migrate_to(world: Rack<Entity>, target: TcpConnection) -> void or Error {
    let bytes = try world.to_bytes()
    try target.write_bytes(bytes)
}

func receive_migration(stream: TcpConnection) -> Rack<Entity> or Error {
    let bytes = try stream.read_bytes()
    return try Rack.from_bytes(bytes)
}
```

### See Also

- [Racks and Links](racks.md) — `slot_index`, the node directory, `snapshot()` and `corresponding()` (`mem.racks`)
- [Pools and Handles](pools.md) — deprecated; Pool API, handle structure, generation counters (`mem.pools`)
- [Value Semantics](value-semantics.md) — Copy vs move, 16-byte threshold (`mem.value`)
- [Linearity](linear.md) — Why linear values are the Tier-3 opaque case (`mem.linear`)
- [Shared, Rack and Heap](shared-rack-heap.md) — Their relocatability tiers (`mem.shared-rack-heap`)
- [Resource Types](resource-types.md) — Why resources are Tier-3 opaque (`mem.resources`)
- [Encoding](../stdlib/encoding.md) — `Encode`/`Decode` traits, field annotations (`std.encoding`)
- [Reflect](../stdlib/reflect.md) — `reflect.is_flat<T>()`, comptime type introspection (`std.reflect`)
