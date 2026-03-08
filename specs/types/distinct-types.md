<!-- id: type.distinct -->
<!-- status: proposed -->
<!-- summary: Nominal type aliases with same layout, no implicit conversion -->
<!-- depends: types/type-aliases.md, types/generics.md, types/traits.md -->

# Distinct Types

A distinct type wraps an existing type with a new identity. Same runtime layout, no implicit conversion in either direction. Fills the gap between transparent aliases (no safety) and struct wrappers (too heavy).

## Declaration

| Rule | Description |
|------|-------------|
| **DT1: Syntax** | `newtype Name = UnderlyingType` at module scope |
| **DT2: Nominal** | Distinct type and underlying type are different types — no implicit conversion |
| **DT3: Same layout** | Runtime representation identical to underlying type (zero overhead) |
| **DT4: Generic** | `newtype Name<T> = UnderlyingType<T>` — type parameters allowed |
| **DT5: Visibility** | `public newtype Name = ...` exports the type |
| **DT6: No cycles** | `newtype A = A` is a compile error (same as type aliases) |

<!-- test: skip -->
```rask
newtype UserId = u64
newtype Email = string
newtype Celsius = f64
newtype Handle<T> = u32

const id = UserId(42)          // explicit construction
const raw: u64 = id.value      // explicit extraction
```

## Construction & Extraction

| Rule | Description |
|------|-------------|
| **DT7: Constructor** | `TypeName(value)` — wraps underlying value |
| **DT8: Extraction** | `.value` field — unwraps to underlying type |
| **DT9: No implicit coercion** | Neither direction is implicit; compiler errors guide the fix |

<!-- test: skip -->
```rask
newtype UserId = u64

const id = UserId(42)
const raw = id.value           // u64

func find_user(id: UserId) -> User? {
    return db.get(id.value)
}

find_user(UserId(42))          // ✓
```

```
ERROR [type.distinct/DT9]: expected UserId, got integer literal
   |
5  |  find_user(42)
   |            ^^ expected UserId
   |
FIX: find_user(UserId(42))
```

## Trait Inheritance

Distinct types don't automatically inherit traits from the underlying type. Declare which traits carry over.

| Rule | Description |
|------|-------------|
| **DT10: No auto-inherit** | Traits from underlying type are NOT inherited by default |
| **DT11: Explicit derive** | `newtype Name = Type with (Trait1, Trait2)` inherits listed traits |
| **DT12: Delegated impl** | Inherited traits delegate to underlying value — no manual impl needed |
| **DT13: Manual extend** | `extend` blocks work normally for adding custom behavior |

<!-- test: skip -->
```rask
// Inherit specific traits
newtype UserId = u64 with (Equal, Hashable, Comparable, Debug)

// Now this works:
const ids = Map<UserId, User>.new()

// But this doesn't (Numeric not inherited):
const bad = UserId(1) + UserId(2)   // ❌ no add method
```

<!-- test: skip -->
```rask
// Inherit nothing (maximum safety)
newtype Token = string

// Add custom behavior
extend Token {
    func validate(self) -> bool {
        return self.value.len() > 0 && self.value.len() <= 256
    }
}
```

**Why no auto-inherit?** The whole point is preventing misuse. If `UserId` inherits `Numeric` from `u64`, you can write `user_id + 1` — which is probably a bug. Explicit inheritance forces you to think about which operations make sense for the domain type.

## Pattern Matching

| Rule | Description |
|------|-------------|
| **DT14: Match on inner** | `if id is UserId(v)` destructures to underlying value |
| **DT15: Guard pattern** | `const v = id is UserId else { ... }` — same as enums |

<!-- test: skip -->
```rask
newtype StatusCode = u16 with (Equal, Comparable)

func is_success(code: StatusCode) -> bool {
    return code.value >= 200 && code.value < 300
}
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Distinct wrapping distinct | DT2 | `newtype A = u64`, `newtype B = A` — B and A are different types; B wraps A, not u64 |
| Copy semantics | DT3 | Follows underlying: if `u64` copies, `UserId` copies (unless `@unique`) |
| `@unique` on distinct | DT3 | `@unique newtype Token = u64` — move-only even though u64 copies |
| `@resource` on distinct | DT3 | `@resource newtype FileHandle = i32` — must consume |
| `with` empty list | DT11 | `newtype X = T with ()` — same as no `with` clause |
| Comptime | DT7 | Constructor and extraction work in comptime context |
| Generic bounds | DT4 | `newtype Wrapper<T: Clone> = T with (Clone)` — bounds propagate |

## Error Messages

**Type mismatch [DT9]:**
```
ERROR [type.distinct/DT9]: type mismatch — expected Email, got string
   |
8  |  send_email("alice@example.com")
   |             ^^^^^^^^^^^^^^^^^^^^ string literal, not Email
   |
FIX: send_email(Email("alice@example.com"))
```

**Missing trait [DT10]:**
```
ERROR [type.distinct/DT10]: UserId does not implement Numeric
   |
3  |  const next = id + 1
   |                  ^ no method 'add' on UserId
   |
WHY: Distinct types don't inherit traits automatically.
FIX: Add 'Numeric' to the with clause: newtype UserId = u64 with (..., Numeric)
     Or use id.value + 1 to operate on the underlying u64.
```

---

## Appendix (non-normative)

### Rationale

**DT1 (keyword choice):** I considered several options:

| Option | Example | Pros | Cons |
|--------|---------|------|------|
| `newtype` | `newtype UserId = u64` | One keyword, Haskell tradition, widely known | Compound word |
| `distinct type` | `distinct type UserId = u64` | Reads naturally, descriptive | Two keywords, heavier |
| `opaque type` | `opaque type UserId = u64` | OCaml/Swift precedent | "opaque" implies hidden internals; this isn't opaque to the defining module |
| `@nominal` attribute | `@nominal type UserId = u64` | Consistent with `@unique`/`@resource` | Attribute + keyword = awkward; attributes modify behavior, this changes identity |

I chose `newtype` because it's one word, well-known across PL communities, and reads naturally: "this is a new type based on u64." The Haskell precedent means experienced developers already know what it does. `distinct type` is fine English but adds a keyword for no semantic gain. `@nominal` is tempting for consistency with `@unique`/`@resource`, but annotations modify existing declarations — `newtype` is a fundamentally different kind of declaration, not a modified alias.

**DT2 (nominal):** Transparent aliases (`type UserId = u64`) document intent but don't enforce it. I've seen real bugs from passing raw integers where domain types were expected. The type system should catch these — that's what it's for.

**DT10 (no auto-inherit):** This is the key design decision. If distinct types inherited everything, `UserId + UserId` would compile. That defeats the purpose. Explicit `with` forces you to answer: "does addition make sense for user IDs?" Usually no. Comparison and hashing usually yes. The `with` clause makes this visible.

**DT3 (same layout):** Zero runtime overhead is non-negotiable. Distinct types are a compile-time concept. The generated code should be identical to using the underlying type directly.

### Patterns & Guidance

#### Domain IDs

The most common use case — preventing ID mixups:

<!-- test: skip -->
```rask
newtype UserId = u64 with (Equal, Hashable, Debug)
newtype OrderId = u64 with (Equal, Hashable, Debug)
newtype ProductId = u64 with (Equal, Hashable, Debug)

func lookup_order(user: UserId, order: OrderId) -> Order? {
    return db.orders.get(user.value, order.value)
}

// Can't accidentally swap arguments:
// lookup_order(order_id, user_id) → compile error
```

#### Units of measurement

Prevent unit confusion (the Mars Climate Orbiter problem):

<!-- test: skip -->
```rask
newtype Meters = f64 with (Debug)
newtype Feet = f64 with (Debug)

extend Meters {
    func to_feet(self) -> Feet {
        return Feet(self.value * 3.28084)
    }
}

extend Feet {
    func to_meters(self) -> Meters {
        return Meters(self.value / 3.28084)
    }
}

// Can't mix units:
// const total = altitude_m + altitude_ft  → compile error
```

#### Validated strings

Strings that must satisfy invariants:

<!-- test: skip -->
```rask
newtype Email = string with (Equal, Hashable, Debug)

extend Email {
    func parse(raw: string) -> Email or ValidationError {
        if !raw.contains("@"): return Err(ValidationError.invalid("email", raw))
        return Email(raw)
    }
}

// Force validation at construction:
const email = try Email.parse(input)
```

#### When to use what

| Need | Use |
|------|-----|
| Readability only | `type UserId = u64` (transparent alias) |
| Prevent misuse at compile time | `newtype UserId = u64` (distinct type) |
| Custom data + behavior | `struct UserId { value: u64 }` (full struct) |
| Prevent copying | `@unique struct Token { value: u64 }` |
| Must-consume resource | `@resource struct File { fd: i32 }` |

Use distinct types when the underlying representation is correct but the domain semantics are different. Use structs when you need additional fields or custom layout.

### IDE Integration

- **Constructor ghost text:** At call sites, IDE shows `UserId(...)` wrapper when a distinct type is expected
- **`.value` hint:** On hover, show underlying type
- **Quick fix:** "Wrap in UserId" / "Unwrap with .value" as code actions on type mismatch errors

### See Also

- `type.aliases` — Transparent type aliases (no safety barrier)
- `mem.value/U1–U2` — `@unique` marker (prevents copying)
- `mem.resource` — `@resource` marker (must-consume)
- `type.structs` — Full struct wrappers (when you need more than a newtype)
