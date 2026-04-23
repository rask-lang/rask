<!-- id: type.aliases -->
<!-- status: decided -->
<!-- summary: type is nominal by default; type alias for transparent shorthand -->
<!-- depends: types/primitives.md, types/generics.md, types/traits.md -->

# Type Declarations

`type Name = Underlying` creates a nominal type — same layout, no implicit conversion. When you name a type, you usually want it distinct. `type alias` is the opt-in for transparent shorthand.

## Nominal Types (Default)

| Rule | Description |
|------|-------------|
| **T1: Syntax** | `type Name = UnderlyingType` at module scope |
| **T2: Nominal** | The declared type and underlying type are different types — no implicit conversion |
| **T3: Same layout** | Runtime representation identical to underlying type (zero overhead) |
| **T4: Generic** | `type Name<T> = UnderlyingType<T>` — type parameters allowed |
| **T5: Visibility** | `public type Name = ...` exports the type |
| **T6: No cycles** | `type A = A` or mutual cycles are compile errors |
| **T7: Constructor** | `TypeName(value)` wraps the underlying value |
| **T8: Extraction** | `.value` unwraps to the underlying type |
| **T9: No implicit coercion** | Neither direction is implicit; compiler errors guide the fix |

<!-- test: skip -->
```rask
type UserId = u64
type Email = string
type Celsius = f64

const id = UserId(42)          // explicit construction
const raw: u64 = id.value      // explicit extraction
```

## Trait Inheritance

Nominal types don't automatically inherit traits from the underlying type. Declare which traits carry over with `with`.

| Rule | Description |
|------|-------------|
| **T10: No auto-inherit** | Traits from underlying type are NOT inherited by default |
| **T11: Explicit with** | `type Name = Type with (Trait1, Trait2)` inherits listed traits |
| **T12: Delegated impl** | Inherited traits delegate to underlying value — no manual impl needed |
| **T13: Manual extend** | `extend` blocks work normally for adding custom behavior |

<!-- test: skip -->
```rask
type UserId = u64 with (Equal, Hashable, Comparable, Debug)

const ids = Map<UserId, User>.new()             // ✓ Hashable inherited
const bad = UserId(1) + UserId(2)               // ❌ Numeric not inherited
```

<!-- test: skip -->
```rask
type Token = string

extend Token {
    func validate(self) -> bool {
        return self.value.len() > 0 && self.value.len() <= 256
    }
}
```

**Why no auto-inherit?** If `UserId` inherited `Numeric` from `u64`, you could write `user_id + 1` — almost certainly a bug. Explicit `with` forces you to answer: "does addition make sense for user IDs?" Usually no. Comparison and hashing usually yes.

## Transparent Aliases

For shortening generic types and function signatures. The alias and underlying type are identical — no conversion needed.

| Rule | Description |
|------|-------------|
| **A1: Syntax** | `type alias Name = ExistingType` |
| **A2: Transparent** | Alias and target are the same type everywhere |
| **A3: Generic aliases** | `type alias Pair<T> = (T, T)` — type parameters allowed |
| **A4: No with clause** | Transparent aliases don't use `with` — they ARE the underlying type |

<!-- test: skip -->
```rask
type alias Matrix = Vec<Vec<f64>>
type alias Callback<T> = func(T) -> bool
type alias AppResult<T> = T or AppError
type alias Handler = func(i32) -> string
```

## Pattern Matching

| Rule | Description |
|------|-------------|
| **T14: Match on inner** | `if id is UserId(v)` destructures to underlying value |
| **T15: Guard pattern** | `const v = id is UserId else { ... }` — same as enums |

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Type wrapping type | T2 | `type A = u64`, `type B = A` — B and A are different types; B wraps A |
| Copy semantics | T3 | Follows underlying: if `u64` copies, `UserId` copies (unless `@unique`) |
| `@unique` on type | T3 | `@unique type Token = u64` — move-only even though u64 copies |
| `@resource` on type | T3 | `@resource type FileHandle = i32` — must consume |
| `with` empty list | T11 | `type X = T with ()` — same as no `with` clause |
| Comptime | T7 | Constructor and extraction work in comptime context |
| Generic bounds | T4 | `type Wrapper<T: Clone> = T with (Clone)` — bounds propagate |
| Alias to alias | A2 | Chains resolve: `type alias A = B`, `type alias B = i32` → A is i32 |
| Cyclic alias | T6 | Compile error with cycle path |
| Shadowing builtin | — | `type string = i32` — error: cannot shadow builtin type |

## Error Messages

**Type mismatch [T9]:**
```
ERROR [type.aliases/T9]: type mismatch — expected Email, got string
   |
8  |  send_email("alice@example.com")
   |             ^^^^^^^^^^^^^^^^^^^^ string literal, not Email
   |
FIX: send_email(Email("alice@example.com"))
```

**Missing trait [T10]:**
```
ERROR [type.aliases/T10]: UserId does not implement Numeric
   |
3  |  const next = id + 1
   |                  ^ no method 'add' on UserId
   |
WHY: Nominal types don't inherit traits automatically.
FIX: Add 'Numeric' to the with clause: type UserId = u64 with (..., Numeric)
     Or use id.value + 1 to operate on the underlying u64.
```

**Cyclic type [T6]:**
```
ERROR [type.aliases/T6]: cyclic type declaration
   |
1  |  type A = B
   |       ^ type A
2  |  type B = A
   |           ^ resolves back to A

FIX: Break the cycle by using the underlying type directly.
```

---

## Appendix (non-normative)

### Rationale

**T2 (nominal by default):** When you write `type UserId = u64`, you almost always want the compiler to distinguish `UserId` from raw `u64`. That's the whole point of naming it. Go got this right — `type` creates a distinct type. The transparent case (shortening `Vec<Vec<f64>>` to `Matrix`) is the exception, not the rule. Exceptions get the longer spelling: `type alias`.

**T10 (no auto-inherit):** The key design decision. If nominal types inherited everything, `UserId + UserId` would compile. That defeats the purpose. Explicit `with` forces you to think about which operations make sense for the domain type.

**A1 (type alias syntax):** `type alias` reads naturally as English and makes the transparency explicit. You can't accidentally create a transparent alias — you have to write `alias`. This prevents the bug where someone writes `type Matrix = Vec<Vec<f64>>` intending shorthand but accidentally creates a nominal type that breaks code elsewhere.

**T3 (same layout):** Zero runtime overhead is non-negotiable. Nominal types are a compile-time concept. The generated code is identical to using the underlying type directly.

### Patterns & Guidance

#### Domain IDs

The most common use case — preventing ID mixups:

<!-- test: skip -->
```rask
type UserId = u64 with (Equal, Hashable, Debug)
type OrderId = u64 with (Equal, Hashable, Debug)
type ProductId = u64 with (Equal, Hashable, Debug)

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
type Meters = f64 with (Debug)
type Feet = f64 with (Debug)

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

// Can't mix: altitude_m + altitude_ft → compile error
```

#### Validated strings

<!-- test: skip -->
```rask
type Email = string with (Equal, Hashable, Debug)

extend Email {
    func parse(raw: string) -> Email or ValidationError {
        if !raw.contains("@"): return ValidationError.invalid("email", raw)
        return Email(raw)
    }
}

const email = try Email.parse(input)
```

#### When to use what

| Need | Use |
|------|-----|
| Distinct domain type | `type UserId = u64` (nominal, default) |
| Shorthand for long types | `type alias Matrix = Vec<Vec<f64>>` (transparent) |
| Custom data + behavior | `struct UserId { value: u64 }` (full struct) |
| Prevent copying | `@unique type Token = u64` |
| Must-consume resource | `@resource struct File { fd: i32 }` |

### IDE Integration

- **Constructor ghost text:** At call sites, IDE shows `UserId(...)` wrapper when a nominal type is expected
- **`.value` hint:** On hover, show underlying type
- **Quick fix:** "Wrap in UserId" / "Unwrap with .value" as code actions on type mismatch errors
- **Alias expansion:** For `type alias`, hover shows expanded type

### See Also

- `type.structs` — Full struct wrappers (when you need more than a single underlying value)
- `type.generics` — Generic type parameters
- `mem.value/U1–U2` — `@unique` marker (prevents copying)
- `mem.resource` — `@resource` marker (must-consume)
