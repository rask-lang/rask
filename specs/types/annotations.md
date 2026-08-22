<!-- id: type.annotations -->
<!-- status: proposed -->
<!-- summary: User-defined annotations — declared data records attached to declarations, read back through reflect at comptime -->
<!-- depends: control/comptime.md, stdlib/reflect.md, analysis/macro-story.md -->

# User Annotations

Libraries can declare their own annotations — pure data records attached to types,
fields, and functions, read back through reflect at comptime. This is the
`#[serde(...)]`-grade extensibility other languages get from derive macros, riding the
same `comptime for` residue mechanism that already powers encoding. Design history:
[analysis/macro-story.md](../analysis/macro-story.md).

## Declaration and Attachment

| Rule | Description |
|------|-------------|
| **AN1: Declaration** | `annotation name { field: T, field: T = default }`. Field types are limited to the const-representable set (`ctrl.comptime/CT58`): primitives, `str`, enums and fixed arrays of these. No methods, no `extend` blocks |
| **AN2: Targets** | Optional targets clause: `annotation validate on field`. Targets: `struct`, `enum`, `variant`, `field`, `func`, `param`. Attaching outside the declared targets is a compile error. Default: attachable anywhere |
| **AN3: Attachment checks as construction** | `@name(args)` type-checks exactly like the struct literal `name { args }` — non-defaulted fields required, names checked, values must be comptime constants |
| **AN4: No duplicates** | Attaching the same annotation twice to one item is a compile error. Repetition wants are served by an array field: `@alias(names: ["a", "b"])` |
| **AN5: Reserved names** | Compiler-known annotations (`@rename`, `@default`, `@no_serialize`, `@native`, `@test`, `@resource`, `@call_site`, …) are reserved. User annotations resolve by normal name resolution — module-scoped, importable |

<!-- test: skip -->
```rask
annotation validate on field { min: i64 = 0, max: i64 }

struct Order {
    @validate(max: 100)
    quantity: u32
}
```

## Reading

| Rule | Description |
|------|-------------|
| **AN6: Three operations** | On reflect items (fields, variants, methods, the type itself), all comptime-only: `item.has<A>()` → `bool`; `item.get<A>()` → `A?`; `item.annotations` → comptime array of all attached annotations, for generic tooling |
| **AN7: Pure data** | Annotations carry no behavior — no conformances, no dispatch, no processor hooks. Whatever an annotation "does" is ordinary code in the library that walks it |

<!-- test: skip -->
```rask
func check<T>(value: T) -> void or ValidationError {
    comptime for field in reflect.fields<T>() {
        comptime if field.has<validate>() {
            let rule = comptime field.get<validate>()
            if value.(field.name) as i64 > rule.max {
                return ValidationError.new("{field.name} over {rule.max}")
            }
        }
    }
    return void
}
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Annotation on a generic struct's field | AN3 | Values are constants — identical across instantiations, read per monomorphized type (`std.reflect/R5`) |
| `comptime if` around an attachment | AN3 | Not allowed — annotations are source-declared facts (`ctrl.comptime/CT65` spirit), not conditional |
| Two packages declare `validate` | AN5 | No clash — resolution is by type, `get<pkg_a.validate>()` vs `get<pkg_b.validate>()` |
| `get<A>()` on an item without `A` | AN6 | Returns none — pair with `has`, or match the optional |
| Annotation field of annotation type | AN1 | Compile error — not in the CT58 set |

## Error Messages

**Wrong target [AN2]:**
```
ERROR [type.annotations/AN2]: `@validate` cannot attach to a function
   |
8  |  @validate(max: 3)
9  |  func handle() { ... }
   |  ^^^^^^^^^^^^^^^^^ `validate` is declared `on field`

WHY: An annotation attached where no reader looks is dead metadata.

FIX: Attach it to a field, or widen the declaration: annotation validate on field, param { ... }
```

**Unknown or missing field [AN3]:**
```
ERROR [type.annotations/AN3]: missing field `max` in `@validate`
   |
4  |  @validate(min: 1)
   |  ^^^^^^^^^^^^^^^^^ `max` has no default and must be given

FIX: @validate(min: 1, max: 100)
```

---

## Appendix (non-normative)

### Rationale

**AN7 (not traits, no processors):** A trait is a behavior contract; an annotation is a
data record. Java's annotation processors put behavior in the metadata layer and got a
second compilation model for it. Here the reader owns the behavior — the whole checking
story is AN3's construction check, done by machinery that already exists.

**AN1 (restricted struct):** The CT58 limit is what lets attached values splice as
constants and read identically in every instantiation. No methods keeps annotations
inert — a method on an annotation is behavior sneaking back in.

**Semver note:** Removing a public annotation from a public type is an API change —
downstream walkers silently change behavior. Same class as removing a field; release
tooling should flag it.

### See Also

- [Comptime](../control/comptime.md) — `comptime for` residue, CT58 splice set
- [Reflect](../stdlib/reflect.md) — the items annotations hang off (`std.reflect`)
- [Encoding](../stdlib/encoding.md) — the compiler-known field annotations this generalizes
- [Macro story](../analysis/macro-story.md) — the gap analysis this came from
