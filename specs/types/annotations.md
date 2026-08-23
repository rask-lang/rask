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
| **AN1: Declaration** | `annotation @name { field: T, field: T = default }` — a keyword, a sigiled name, a field body, nothing else. The name keeps its `@` so the declaration spells exactly what attachment sites write. Field types are limited to the const-representable set (`ctrl.comptime/CT58`): primitives, `string`, enums and fixed arrays of these. No methods, no `extend` blocks |
| **AN3: Attachment checks as construction** | `@name(args)` type-checks exactly like the struct literal `name { args }` — non-defaulted fields required, names checked, values must be comptime constants |
| **AN4: No duplicates** | Attaching the same annotation twice to one item is a compile error. Repetition wants are served by an array field: `@alias(names: ["a", "b"])` |
| **AN5: Reserved names** | Compiler-known annotations (`@rename`, `@default`, `@no_serialize`, `@native`, `@test`, `@resource`, `@call_text`, `@call_location`, …) are reserved. User annotations resolve by normal name resolution — module-scoped, importable |

<!-- test: skip -->
```rask
annotation @validate { min: i64 = 0, max: i64 }

struct Order {
    @validate(max: 100)
    quantity: u32
}
```

**AN2 is deleted** — it was an `on struct, field, …` targets clause making misplacement a compile error. Placement is not the compiler's call: an annotation is data, and which items a reader cares about is the reader's business (Principle 5, *Information Without Enforcement*). The clause also cost a bespoke sub-grammar — `on` plus six target words that mean nothing anywhere else in the language. If misplaced metadata proves to be a real problem it returns as a lint, not syntax. The ID is not reused.

## Reading

| Rule | Description |
|------|-------------|
| **AN6: Three operations** | On reflect items (fields, variants, methods, the type itself), all comptime-only: `item.has<A>()` → `bool`; `item.get<A>().field` reads one field of the attached value; `item.annotations` → comptime array of `AnnotationInfo { name: string }`, for tooling that walks by name instead of asking for a type it already knows |
| **AN7: Pure data** | Annotations carry no behavior — no conformances, no dispatch, no processor hooks. Whatever an annotation "does" is ordinary code in the library that walks it |
| **AN8: A projection, not a value** | An annotation name is not a type: `indexed` in a parameter, field, return, `let` or type-argument position is a compile error. `get<A>()` therefore has no result you can bind — only `.field` on it, which splices as a constant. Reading an annotation an item doesn't carry is an error at the read, naming `has<A>()` as the guard |

<!-- test: skip -->
```rask
func check<T>(value: T) -> void or ValidationError {
    comptime for field in reflect.fields<T>() {
        comptime if field.has<validate>() {
            let max = field.get<validate>().max
            if value.(field.name) as i64 > max {
                return ValidationError.new("{field.name} over {max}")
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
| `comptime if item.has<A>()` guarding a `get<A>()` | AN6 | The guard has to be *comptime* — a runtime `if` on the same test leaves the untaken branch's reads to be resolved, and a field without `A` has nothing to read |
| Two packages declare `validate` | AN5 | No clash — the name in `get<...>` resolves in the ordinary way, so `pkg_a.validate` and `pkg_b.validate` stay distinct |
| `get<A>()` on an item without `A` | AN8 | Error at the read — guard with `comptime if item.has<A>()`. Not catchable when the item is written: which item a `comptime for` binding is on is only known once the loop unrolls |
| `let r = field.get<A>()` | AN8 | Compile error — there is no type to bind it to. Project a field instead: `field.get<A>().max` |
| Annotation field of annotation type | AN1 | Compile error — not in the CT58 set |

## Error Messages

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

**AN1 (one shape, no clauses):** The declaration reads like a struct because it
is one. Every extra clause is a word someone has to learn for this construct
alone — the first draft was `annotation name on field`, four bare words where
only context told you which two were keywords. The sigiled name fixes that
ambiguity; deleting AN2's targets clause removes the rest. The CT58 field-type
limit is what lets attached values splice as constants and read identically in
every instantiation, and forbidding methods keeps annotations inert — a method
on an annotation is behavior sneaking back in.

**AN8 (why there is no annotation value):** AN3 says you can't construct one, so
a variable of annotation type is a slot nothing can ever fill — the type checker was
happily accepting `func peek(a: indexed)`, a function that could never be called. Two
ways out: let annotations materialize as ordinary structs, or let them not exist at
runtime at all. Materializing them means a type users can't construct but can copy,
store in a Vec and return — the non-construction rule would be a formality. So they
don't exist: `get<A>()` is a projection, `field.get<A>().max` splices the constant `3`,
and the annotation itself never reaches MIR. This also matches how `field.name` already
works — no FieldInfo struct is ever built natively either; the members splice.

**AN7 (not traits, no processors):** A trait is a behavior contract; an annotation is a
data record. Java's annotation processors put behavior in the metadata layer and got a
second compilation model for it. Here the reader owns the behavior — the whole checking
story is AN3's construction check, done by machinery that already exists.

**Semver note:** Removing a public annotation from a public type is an API change —
downstream walkers silently change behavior. Same class as removing a field; release
tooling should flag it.

### See Also

- [Comptime](../control/comptime.md) — `comptime for` residue, CT58 splice set
- [Reflect](../stdlib/reflect.md) — the items annotations hang off (`std.reflect`)
- [Encoding](../stdlib/encoding.md) — the compiler-known field annotations this generalizes
- [Macro story](../analysis/macro-story.md) — the gap analysis this came from
