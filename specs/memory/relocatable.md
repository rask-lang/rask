<!-- id: mem.relocatable -->
<!-- status: proposed -->
<!-- summary: Pointer-free types enable binary serialization, mmap, and state snapshots without fixup -->
<!-- depends: memory/pools.md, memory/value-semantics.md, stdlib/encoding.md, stdlib/reflect.md -->

# Relocatable Memory

Rask's "no storable references" design means user-visible types contain only owned values and integer handles — never pointers. This makes pool state relocatable: handles survive serialization round-trips because they're integers, not addresses.

**Terminology:** "Relocatable" here means *data that can be moved to a different memory address, process, or machine without pointer fixup*. This is not the same as position-independent code (PIC/PIE). The property comes from the absence of pointers in user-visible types.

**Honest framing:** This isn't impossible elsewhere. GC'd languages can serialize state too. The unique thing is: systems-level performance with deterministic cleanup, and no pointer fixup step. Rust can achieve similar results but requires manual `#[repr(C)]` layout management and unsafe pointer-to-offset conversions.

## Workflows unlocked by one API

`pool.to_bytes()` / `Pool.from_bytes()` is a single pair of methods. Because handles are integers that survive the round-trip, it backs five distinct workflows without any extra primitives:

| Workflow | How `to_bytes` / `from_bytes` backs it |
|----------|----------------------------------------|
| Save / load (game state, app state) | Serialize on shutdown, deserialize on startup |
| Undo / redo | Push `to_bytes()` to a history stack; `from_bytes()` on undo |
| Hot reload | Serialize → recompile → deserialize; schema evolution handles additive changes |
| Process migration | Send bytes over the network; handles remain valid on receiver |
| Time-travel debugging | Checkpoint state per-tick; rewind by loading a prior checkpoint |

This falls out of the no-storable-references choice (handles are integers, not addresses). It's the most concrete payoff of the "everything is a value" principle: state has no hidden pointers, so state is portable.

## Relocatability Tiers

Not everything is trivially relocatable. Types fall into three tiers based on their internal structure.

| Rule | Tier | Types | Mechanism | Cost |
|------|------|-------|-----------|------|
| **R1: Flat** | Flat | Primitives, handles, flat structs | Bitwise copy / mmap | Zero |
| **R2: Deep** | Deep | Flat + `string`, `Vec`, `Map` (no resources) | Binary serialization (heap contents traversed) | Linear scan |
| **R3: Opaque** | Opaque | Resource types, closures, `any Trait` | Cannot serialize | N/A |

Closures and `any Trait` contain function pointers — process-local, not serializable. Resource types (`@resource`) have external side effects that can't survive a round-trip. The spec doesn't try to make these relocatable.

## Flat Type Constraint

A type is *flat* when it contains no heap-backed fields, recursively.

| Rule | Description |
|------|-------------|
| **FL1: Definition** | A type is flat if all fields are flat, recursively. No `string`, `Vec`, `Map`, `Cell`, `Shared`, `Mutex`, `any Trait`, closures, or resource types |
| **FL2: Primitives** | `bool`, `i8`–`i64`, `u8`–`u64`, `f32`, `f64`, `usize` are flat |
| **FL3: Handles** | `Handle<T>` is flat (integer components only) |
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
const flat = comptime reflect.is_flat<GameEntity>()   // true

struct NamedEntity {
    public id: u32
    public name: string   // heap-backed
}

// Not flat — contains string
const not_flat = comptime reflect.is_flat<NamedEntity>()  // false
```

## No Pointer Fixup Property

| Rule | Description |
|------|-------------|
| **NP1: Handle identity** | Handles are integers (pool_id, index, generation). Handle identity survives any byte-level round-trip — serialization, mmap, network transfer |
| **NP2: No address dependency** | User-visible types never contain memory addresses. Moving data to a different address, process, or machine requires no pointer translation |
| **NP3: Pool internal storage** | The pool itself owns a heap-allocated slot array. NP2 applies to handles referencing INTO the pool, not the pool's own internal storage. `pool.to_bytes()` serializes the slot array contents, not addresses |

## Pool Binary Serialization

Pools with `T: Encode + Decode` can serialize to and from a compact binary format.

| Rule | Description |
|------|-------------|
| **PB1: Serialize** | `pool.to_bytes() -> Vec<u8>` — serializes all occupied slots via binary `Encode` |
| **PB2: Deserialize** | `Pool.from_bytes(bytes) -> Pool<T> or DecodeError` — reconstructs pool from bytes |
| **PB3: Handle preservation** | Handles obtained before `to_bytes()` are valid against the pool returned by `from_bytes()`. Same index, same generation |
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
| **SE2: Added fields** | Fields present in the current type but absent in the stored schema get their `@default` value, or zero value if applicable (`std.encoding/E20`, `E28`) |
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
func test_handle_roundtrip() -> () or Error {
    const pool = Pool.new()
    const h = pool.insert(Player { id: 1, health: 100, score: 0 })

    const bytes = try pool.to_bytes()
    const restored = try Pool.from_bytes(bytes)

    // h is still valid — same index, same generation
    assert(restored[h].id == 1)
    assert(restored[h].health == 100)
}
```

## Memory-Mapped Pools (Flat Types Only)

For flat types, pools can be memory-mapped directly — no serialization step.

| Rule | Description |
|------|-------------|
| **MM1: Flat constraint** | `Pool.from_mmap(path)` and `pool.to_mmap(path)` require `T` to be flat (`FL1`). Compile error otherwise |
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

  const bytes = try pool.to_bytes()
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

func save_particles(pool: Pool<Particle>) -> () or IoError {
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

FIX: Mark non-serializable fields @skip, or use @no_encode and implement
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

**FL1–FL4 (flat constraint):** I considered a `Relocatable` trait but it would duplicate `Copy` for flat types and `Encode + Decode` for deep types. `reflect.is_flat<T>()` at comptime is simpler — it's a query, not a type-system concept. The compiler already knows the layout; just expose that knowledge.

**PB1–PB5 (pool serialization):** `pool.to_bytes()` with `T: Encode + Decode` handles the common case (deep types). The binary format with schema descriptors means you don't need manual migration code for additive changes — added fields get defaults, removed fields are skipped. This covers the 80% case of evolving game state, configuration, caches.

**SE1–SE4 (schema evolution):** Field-by-field matching by name gives forward/backward compatibility for free on additive changes. Type changes are intentionally an error — silent coercion between `f32` and `i32` would be a bug factory. If you need a migration, write one explicitly.

**MM1–MM4 (mmap):** Mmap is genuinely useful for particle systems, terrain data, and other flat-data workloads. But it's niche — most real structs have at least one `string` field. The error message quality matters more than the feature itself, because developers will hit the compile error and need to understand why.

**NP3 (pool internal storage):** A common confusion: "if there are no pointers, how does the pool store data?" The pool's internal slot array is heap-allocated — it owns the memory. The no-pointer property applies to handles that *reference into* the pool. The pool manages its own storage; handles are just integer keys into that storage.

### Patterns & Guidance

**State snapshot (undo/redo):**

<!-- test: skip -->
```rask
struct UndoStack<T: Encode + Decode> {
    public history: Vec<Vec<u8>>
    public max_entries: usize
}

extend UndoStack<T: Encode + Decode> {
    func push(self, pool: Pool<T>) -> () or EncodeError {
        if self.history.len() >= self.max_entries {
            self.history.remove(0)
        }
        self.history.push(try pool.to_bytes())
    }

    func pop(self) -> Pool<T> or DecodeError {
        const bytes = self.history.pop() ?? return DecodeError.Empty
        return Pool.from_bytes(bytes)
    }
}
```

**Hot code reloading:**

Serialize state → recompile → deserialize. Uses field-by-field `Encode`/`Decode` (not bitwise), so layout changes between compilations are handled by schema evolution (SE1–SE3). Added fields get defaults, removed fields are skipped.

<!-- test: skip -->
```rask
func hot_reload(pool: Pool<GameState>) -> Pool<GameState> or Error {
    const bytes = try pool.to_bytes()
    // ... recompile happens here ...
    return try Pool.from_bytes(bytes)
}
```

**Process migration:**

Send `to_bytes()` over the network. Handles are valid on the receiving end because they're integers — no address translation needed.

<!-- test: skip -->
```rask
func migrate_to(pool: Pool<Entity>, target: TcpStream) -> () or Error {
    const bytes = try pool.to_bytes()
    try target.write_all(bytes)
}

func receive_migration(stream: TcpStream) -> Pool<Entity> or Error {
    const bytes = try stream.read_all()
    return try Pool.from_bytes(bytes)
}
```

### See Also

- [Pools and Handles](pools.md) — Pool API, handle structure, generation counters (`mem.pools`)
- [Value Semantics](value-semantics.md) — Copy vs move, 16-byte threshold (`mem.value`)
- [Linearity](linear.md) — Why linear values are the Tier-3 opaque case (`mem.linear`)
- [Boxes](boxes.md) — Box types and their relocatability tiers (`mem.boxes`)
- [Resource Types](resource-types.md) — Why resources are Tier-3 opaque (`mem.resources`)
- [Encoding](../stdlib/encoding.md) — `Encode`/`Decode` traits, field annotations (`std.encoding`)
- [Reflect](../stdlib/reflect.md) — `reflect.is_flat<T>()`, comptime type introspection (`std.reflect`)
