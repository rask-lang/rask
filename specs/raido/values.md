<!-- id: raido.values -->
<!-- status: proposed -->
<!-- summary: Raido value model — NaN-boxed dynamic types with handle and string sharing -->
<!-- depends: raido/README.md, memory/pools.md, memory/resource-types.md -->

# Values

All Raido values are 8 bytes, NaN-boxed. Seven types: `nil`, `bool`, `int`, `number`, `string`, `table`, `function`, plus `handle` for entity access and `userdata` for opaque Rask values.

## Types

| Rule | Description |
|------|-------------|
| **V1: NaN-boxed** | Every value is 8 bytes. Type discrimination via NaN-boxing: quiet NaN payloads encode non-float types. |
| **V2: Nil** | `nil` is a singleton. Falsy. Represents absence. |
| **V3: Bool** | `true` and `false`. Only `nil` and `false` are falsy; everything else is truthy (including `0` and `""`). |
| **V4: Number** | IEEE 754 f64. Used for floating-point math. |
| **V5: Int** | i64 integer. Used for entity IDs, counters, bitfields. Mixed int/number arithmetic promotes to number. |
| **V6: String** | Shares Rask's `string` representation — immutable, refcounted, UTF-8. Zero-copy across the boundary. |
| **V7: Table** | Arena-allocated hash+array hybrid. 0-indexed array part. Not interchangeable with Rask `Vec`/`Map`. |
| **V8: Function** | Arena-allocated closure (Raido bytecode + captured upvalues) or host function reference. |
| **V9: Handle** | 12-byte `Handle<T>` from Rask pools, packed into an arena-allocated box. First-class type — `h.field` does a pool lookup. |
| **V10: Userdata** | Arena-allocated box containing an arbitrary Rask value. Opaque to scripts unless the host registers accessors. |

```raido
-- Type examples
x = nil               -- nil
alive = true           -- bool
count = 42             -- int
speed = 3.14           -- number
name = "Alice"         -- string
pos = { x = 0, y = 0 } -- table
f = func(a) return a end -- function
-- handles and userdata come from the host
```

## NaN-Boxing Layout

| Bits | Meaning |
|------|---------|
| Normal f64 | `number` value (V4) |
| Quiet NaN + tag 0 | `nil` |
| Quiet NaN + tag 1 | `bool` (payload: 0/1) |
| Quiet NaN + tag 2 | `int` (payload: 48-bit, extend to i64 for large ints via heap boxing) |
| Quiet NaN + tag 3 | GC pointer (table, function, handle box, userdata, or heap-boxed int) |
| Quiet NaN + tag 4 | `string` (refcounted pointer, same as Rask) |

```
// 8-byte NaN-boxed value
// If bits are a valid f64 (not quiet NaN): it's a number
// If quiet NaN: low 48 bits are payload, bits 48-50 are type tag
```

## Int/Number Distinction

| Rule | Description |
|------|-------------|
| **N1: Literal inference** | `42` is int, `42.0` is number. No suffix needed. |
| **N2: Arithmetic promotion** | `int + number → number`, `int + int → int`. Division always returns number. |
| **N3: Integer division** | `//` operator for integer division: `7 // 2 → 3`. |
| **N4: Overflow** | Int arithmetic wraps on overflow (consistent with game engine expectations). |
| **N5: Comparison** | `42 == 42.0` is `true`. Int and number compare by mathematical value. |

```raido
count = 10         -- int
speed = 2.5        -- number
result = count + 1  -- int (int + int)
result = count * speed  -- number (int * number)
result = 7 // 2    -- int: 3
result = 7 / 2     -- number: 3.5
```

I added a separate int type because Lua's "everything is f64" causes real problems in game scripting. Entity IDs lose precision above 2^53. Bitfield operations produce wrong results on floats. Lua 5.3 added integers for these reasons — Raido gets them from the start.

## Strings

| Rule | Description |
|------|-------------|
| **S1: Arena-allocated** | Raido strings are arena-allocated byte arrays. Own representation, not Rask's refcounted `string`. |
| **S2: Copy at boundary** | Passing a string from Rask to Raido copies bytes into the arena. Returning a string creates a new Rask `string`. |
| **S3: Immutable** | Strings are immutable once created. Concatenation creates a new string. |
| **S4: UTF-8** | Strings are UTF-8 encoded, same as Rask. |
| **S5: Interning** | String literals in bytecode are interned in the arena. Equality checks on interned strings are pointer comparison. |

Strings are copied at the host/script boundary. For game scripting workloads (entity names, status messages, short format strings), the copy cost is negligible. The simplicity of arena-only strings — no refcount management, no shared lifetimes — is worth the copy.

## Tables

| Rule | Description |
|------|-------------|
| **T1: Hybrid structure** | Tables have an array part (contiguous integer keys 0..n) and a hash part (arbitrary keys). |
| **T2: 0-indexed** | Array part is 0-indexed. `t[0]` is the first element. |
| **T3: Arena-allocated** | Tables are allocated in the VM arena. Freed on arena reset, not individually. |
| **T4: No metatables** | Tables are plain data. No metamethods, no operator overloading. Host functions provide behavior. |
| **T5: Isolation** | Tables are not interchangeable with Rask `Vec` or `Map`. Explicit conversion required. |

```raido
-- Array usage
items = {10, 20, 30}
items[0]  -- 10
items[2]  -- 30

-- Hash usage
config = { width = 800, height = 600, title = "My Game" }
config.width  -- 800
config["title"]  -- "My Game"

-- Mixed
player = { "warrior", level = 5, hp = 100 }
player[0]  -- "warrior"
player.level  -- 5
```

**No metatables (T4).** Metatables are powerful but complex — they turn tables into a general-purpose OOP system with inheritance, operator overloading, and proxy patterns. That's too much machinery for a game scripting language. If scripts need polymorphic behavior, the host provides it through registered functions.

**0-indexed (T2).** This breaks from Lua tradition. I chose consistency with Rask over Lua compatibility. Every index crossing the host/script boundary would need ±1 adjustment otherwise — a bug factory in game code where indices map to entity slots, inventory positions, and tile coordinates.

## Handles

| Rule | Description |
|------|-------------|
| **H1: First-class type** | Handles are a distinct Raido type, not userdata. `type(h)` returns `"handle"`. |
| **H2: Pool-resolved field access** | `h.field` reads from the pool entry. `h.field = value` writes to it. The VM resolves the handle against the pool provided via `exec_with`. |
| **H3: Validity check** | Accessing a dead handle (generation mismatch) raises a runtime error. |
| **H4: Copy semantics** | Handles can be freely copied and stored in tables. They're lightweight references, not ownership. |
| **H5: Pool scope** | Handle field access only works during `exec_with` when the pool is provided. Outside that scope, accessing fields raises an error. |
| **H6: Type tag** | Each handle carries a pool name tag so the VM routes field access to the correct pool. |

```raido
func on_update(dt)
    for h in handles("enemies") do
        -- h.field reads/writes the pool entry directly
        h.x = h.x + h.vx * dt
        h.y = h.y + h.vy * dt

        if h.health <= 0 then
            remove(h)  -- remove from pool
        end
    end
end

-- Store handles in tables
targets = {}
for h in handles("enemies") do
    if h.aggro then
        table.insert(targets, h)
    end
end
```

Handle field access is the core Raido innovation. When the script does `h.x`, the VM:
1. Reads the pool name tag from the handle
2. Looks up the pool provided via `exec_with`
3. Validates the handle's generation
4. Reads/writes the field

This happens on every field access — it's not free. But it's the same work Rask does with `pool[h].x`, just happening inside the VM instead of in compiled code. For game scripting workloads (hundreds of entities, not millions), this is fine.

## Userdata

| Rule | Description |
|------|-------------|
| **U1: Opaque box** | Userdata wraps an arbitrary Rask value. Scripts can't inspect it unless the host registers accessors. |
| **U2: Arena-allocated** | The Rask value is stored in the arena. On arena reset, `Drop` runs for non-Copy types. |
| **U3: No resources** | `@resource` types (files, sockets, connections) cannot be stored as userdata. Compile error on the Rask side. |
| **U4: Type-safe extraction** | Host code extracts userdata with `ctx.arg_userdata::<T>(n)`, which does a runtime type check. |
| **U5: Copy-in** | Copy types are copied into the VM. Non-Copy types are moved (host loses access). |

```rask
// Rask side — register a function that takes userdata
vm.register("set_color", |ctx| {
    const color = try ctx.arg_userdata::<Color>(0)
    // use color...
})

// Pass userdata to script
vm.set_global("bg_color", raido.Value.userdata(Color { r: 255, g: 0, b: 0 }))
```

```raido
-- Script side — userdata is opaque
set_color(bg_color)  -- passes it back to host
-- bg_color.r  -- ERROR: cannot access fields on userdata
```

**U3 (no resources)** is a hard rule. If a `File` ends up as userdata, it would sit in the arena until reset — but `File` is a linear resource that must be consumed explicitly. Arena reset dropping it silently violates Rask's safety model. If you need file operations in scripts, expose them as host functions that manage the resource on the Rask side.

## Edge Cases

| Case | Rule | Behavior |
|------|------|----------|
| Very large int (>48 bits) | V5, N4 | Heap-boxed as GC object. Transparent to scripts. |
| `nil` as table key | T1 | Runtime error: nil cannot be a table key. |
| Handle after pool scope ends | H5 | Runtime error: "pool not available outside exec_with". |
| Handle to removed entity | H3 | Runtime error: "stale handle — entity was removed". |
| Arena reset with live userdata | U2 | `Drop` runs for all non-Copy userdata during reset. |
| Int overflow | N4 | Wraps (i64 wrapping semantics). |
| `int == number` comparison | N5 | `42 == 42.0` is true. Exact mathematical comparison. |

## Error Messages

```
ERROR [raido.values/H3]: stale handle — entity was removed
   |
5  |  h.health -= 10
   |  ^ handle points to generation 3, pool entry is generation 4

WHY: The entity this handle pointed to was removed. The pool slot was reused.

FIX: Check handle validity before access:
   if valid(h) then
       h.health -= 10
   end
```

```
ERROR [raido.values/U3]: resource type cannot be stored as userdata
   |
12 |  vm.set_global("file", raido.Value.userdata(file))
   |                        ^^^^^^^^^^^^^^^^^^^^^^^^^ File is @resource

WHY: Resource types must be consumed explicitly. GC collection would silently drop the resource.

FIX: Manage the resource on the Rask side and expose operations as host functions.
```

---

## Appendix (non-normative)

### Rationale

**V5 (separate ints):** I debated whether to follow Lua 5.0-5.2's "everything is f64" approach for simplicity. The game use case killed that idea — entity IDs above 2^53 lose precision, bitwise operations on floats are confusing, and loop counters as floats add conversion overhead. Lua 5.3 added integers for these exact reasons.

**T4 (no metatables):** Metatables are Lua's most powerful feature and its most complex. They enable OOP, operator overloading, lazy evaluation, proxy objects, and more. For a game scripting language, that's too much power in the hands of modders. It creates a maintenance burden — host developers need to understand arbitrary metamethod chains to debug script behavior. Plain tables + host functions keep the mental model simple.

**T2 (0-indexed):** This will be the most controversial decision for Lua users. The practical argument: game scripts constantly pass indices to/from the host. Tile coordinates, inventory slots, entity arrays — all 0-indexed in Rask. Every boundary crossing with ±1 adjustment is a bug waiting to happen. I chose integration correctness over Lua tradition.

**H2 (direct field access):** The alternative was copy-out/modify/copy-back (`local e = get(h); e.x += 1; set(h, e)`). That's safer (no VM-level field interception) but terrible ergonomics for the primary use case. Game scripts touch entity fields constantly — the syntax should be as short as possible.

### See Also

- `raido.interop` — How values cross the Rask/Raido boundary
- `raido.vm` — Arena allocation for tables, functions, handles, and userdata
- `mem.pools` — Pool/Handle system that handles reference
- `mem.resource-types` — Why resources can't be userdata (U3)
