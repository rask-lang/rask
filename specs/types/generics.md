<!-- id: type.generics -->
<!-- status: decided -->
<!-- summary: Nominal trait conformance via extend...with, operator-to-method expansion, verified clone/equal/comparable, code specialization per type -->
<!-- depends: types/structs.md, types/enums.md, types/traits.md -->
<!-- implemented-by: compiler/crates/rask-types/ -->

# Generics and Traits

Trait conformance is declared — `extend Type with Trait` says the type satisfies the trait, and the compiler checks the signatures against the declaration. `duck trait` opts individual traits into shape-matching, but only inside a package: a duck trait can never be `public`, and the tooling keeps reminding you it's a sketch (DT1–DT4). Operators like `a + b` expand to method calls. The compiler generates specialized code for each concrete type you use (this is called *monomorphization*). For mixed-type collections, opt into runtime dispatch with `any Trait`.

## Core Principles

| Rule | Description |
|------|-------------|
| **G1: Declared conformance** | A type satisfies a trait through a declared `extend Type with Trait` block, checked against the trait's signatures. `duck trait` opts a trait into shape-matching (no declaration needed) within its own package — see DT1–DT4. The four core traits (Equal, Hashable, Comparable, Cloneable) are auto-derived for eligible types — compiler-provided conformance, overridable per EQ2/HA2/CO2 and subject to OC1. `Debug` (all types), `Encode`/`Decode` (markers), and `Error` (enums, `type.errors/ER6`) are also auto-derived. Of those, the four plus `Encode`/`Decode` may be overridden only by the package that declares the type (XC1) |
| **G2: Checked at use site** | The compiler verifies trait matching when you call a generic function, not when you define it |
| **G3: Body-local inference** | Non-public functions can have bounds inferred from body; see [Gradual Constraints](gradual-constraints.md) |
| **G4: Operator expansion** | `a + b` becomes `a.add(b)` before trait checking |
| **G5: Verified clone** | Compiler ensures clone produces deep copy; types with pointers require unsafe extend |
| **G6: Code specialization** | Each `<T>` usage generates specialized code (monomorphization) — fast calls, but increases binary size |
| **G6a: Methods specialize with their type** | A method declared in `extend One<A>` is specialized per receiver instantiation, same as a generic function: `One<i64>.get()` and `One<Big>.get()` are two bodies. That's what lets each instantiation have a layout that fits its type argument — a struct or tuple argument *is* its bytes, so one shared body couldn't take both an 8-byte and a 24-byte `self`. A method with its own parameters specializes on the receiver's arguments and then its own |
| **G7: Runtime polymorphism opt-in** | `any Trait` for heterogeneous collections; dispatch through function pointer table (vtable) |

## Trait Definition

| Rule | Description |
|------|-------------|
| **TD1: Module-scoped** | Traits must be module-scoped |
| **TD2: Default methods** | Traits may contain default implementations |
| **TD3: Composition** | Traits may compose using `:` syntax |
| **TD4: Members** | A trait body holds methods and associated types (`type Out`). Anything else — a `const`, a nested `struct` — is an error at the line that wrote it |

| Trait Form | Meaning |
|------------|---------|
| `trait Comparable` | Nominal (default) — types conform via `extend Type with Comparable` |
| `duck trait Frobber` | Shape-matched — any type with the right methods satisfies it, no declaration. Package-internal: never `public` (DT1), and flagged as a sketch by lint (DT3) |
| `trait Hashable: Equal` | Composition (requires all methods from Equal plus Hashable's own) |

```rask
trait Name {
    func method_name(self, params...) -> ReturnType
    func another_method(self) -> OtherType

    // Default implementation (optional)
    func helper(self) -> bool {
        self.method_name(...) != null
    }
}
```

Nominal is the default because conformance is a semantic claim, not just a shape: `compare()` existing doesn't make it a total order. The declaration states intent, gives the compiler a place to check signatures, and gives readers and tools a place to look.

## Trait Type Parameters

A trait can take type parameters, and the conformance says what they are:

```rask
trait Scale<Rhs> {
    func scale(self, k: Rhs) -> Self
}

extend Meters with Scale<f64> {
    func scale(self, k: f64) -> Meters {
        return Meters { v: self.v * k }
    }
}
```

The header binds `Rhs` to `f64`, so what the conformance owes is `scale(self, k: f64) -> Meters` — `Self` and `Rhs` are both substituted before the signature is checked.

| Rule | Description |
|------|-------------|
| **GT1: Parameters on the declaration** | `trait Scale<Rhs>` — one or two parameters, the same cap generic types carry. `Self` is always in scope and is not one of them |
| **GT2: The header applies them** | `extend T with Scale<f64>` substitutes through every required signature. A header that gives the wrong number of arguments is an error naming the trait's arity |
| **GT3: The applied trait is the conformance** | `Mul<f64>` and `Mul<Meters>` are different conformances of one trait, each with its own signatures and its own associated types (`type.associated-types/AT8`). A bare `T: Mul` bound means the defaulted instantiation, not "some instantiation". Both of them *on one type* is rejected at the second block while MN1 still decides the call — see AT8 |
| **GT4: Declared defaults** | `trait Mul<Rhs = Self>` lets `Mul` be written bare and mean `Mul<Self>`. Without a default the argument is required, in a bound and in a conformance header alike |
| **GT5: Bounds on the parameter** | `trait Scale<Rhs: Numeric>` is checked where the conformance names its argument, the same as a bound on a generic struct's parameter |

Associated types are the other half of this: a parameter is what the *conformance* chooses, an associated type is what the conformance *reports*. See [associated-types.md](associated-types.md).

## Duck Traits Are Package-Internal

`duck trait` is a sketching tool, and the compiler treats it as one. It is not a lighter-weight way to write a trait — it's a placeholder for a contract you haven't decided on yet, and it can't leave the package it was sketched in.

| Rule | Description |
|------|-------------|
| **DT1: Never public** | `public duck trait` is a compile error. A duck trait also may not appear in any public signature — not as a bound, not as `any Trait`, not in a public type alias, and not in a `public extend ... with` header (TV2 already caps that). A duck trait's methods may still be `public` on the types that match it; the *trait* is what stays package-internal |
| **DT2: Reported at publish** | `rask publish` reports duck traits the package declares, as a warning with a count (`struct.build/PB8`). Not a gate — DT1 already means they can't reach a consumer, so there's nothing for a release check to protect |
| **DT3: Flagged by lint** | `rask lint` warns on every `duck trait` declaration and names the harden step (`tool.lint/I3`). Suppressible with `@allow(idiom/duck-trait)` when the sketch is deliberate |
| **DT4: Hardening is mechanical** | Deleting the `duck` keyword turns it nominal. The compiler already knows every type matching by shape, so it lists them and a quick-fix inserts the `extend Type with Trait {}` declarations. Nothing else about the trait changes |

Declare a trait duck while sketching: no conformance declarations, methods move freely between types, nothing to keep in sync. Declaring conformance to a duck trait anyway is legal and harmless — documentation plus a signature check at the declaration instead of the use site. The stdlib ships zero duck traits.

DT1 is the rule that carries the weight, and the reason is versioning. Structural conformance across a package boundary is a trap: adding a public method to your type could silently make it satisfy a duck trait in a package you've never read, and removing one could break code you've never seen. Neither shows up in your diff, and neither is something semver can describe. Keeping the trait package-internal means every type that matches it is a type the same author owns, so a shape change and its consequences land in the same review.

Inside a package, that hazard doesn't exist. Consumers can't see a private duck trait, can't name it, and can't have their types satisfy it. If you drop a method and one of your own types stops matching, that's a compile error in your own build — the same as any other internal break. So DT2 and DT3 tell you the sketch is still there and leave the decision with you. A published package whose internals are genuinely still in flux is a legitimate thing to ship; blocking it would be ceremony with no victim, which is the same line `type.gradual/GC11` draws for inferred private signatures.

```
ERROR [type.generics/DT1]: `duck trait` cannot be public
   |
4  |  public duck trait Frobber {
   |  ^^^^^^ ^^^^
   |
WHY: shape-matching across a package boundary is a versioning trap — a
     stranger's type could start or stop satisfying `Frobber` without either
     author changing a line they'd notice.

FIX: drop `duck` to make it a real trait (the compiler will list the types
     that already match and generate the conformance declarations):

  public trait Frobber {

     ...or drop `public` to keep it a package-internal sketch.
```

The publish-time warning (DT2) is in [build.md](../structure/build.md#publishing) under `struct.build/PB8`.

## Generic Functions

| Rule | Description |
|------|-------------|
| **GF1: Public bounds explicit** | Public generic functions must declare trait constraints explicitly |
| **GF2: Private bounds inferred** | Non-public functions may omit constraints; compiler infers from body |
| **GF3: Caller constraints** | Calling a constrained function requires same or stronger constraints (explicit or inferred) |
| **GF4: Disjointness travels with the signature** | A signature writing `T or E` with a type parameter on either side carries an implicit "these must stay distinct" obligation, checked at the call site once `T` is known. Not spelled as a bound — the `or` already says it. See [error-types.md](error-types.md) ER3a |
| **GF5: Methods too** | A method declares type parameters the same way a function does, and they're independent of the receiver's. `Holder<T>` can have `func other<U>(self, u: U) -> U` — `T` is fixed by the receiver, `U` is chosen per call |

```rask
// Public: bounds MUST be explicit
public func process<T: Hashable>(items: Vec<T>) { ... }

// Private: bounds inferred from body
func helper(item) { item.hash() }
// Compiler infers: func helper<T: Hashable>(item: T)

// GF5: a method's own parameters, alongside the receiver's
extend Holder<T> {
    func mine(self) -> T { return self.item }
    func other<U>(self, u: U) -> U { return u }
}
```

Each distinct set of type arguments gets its own compiled body, same as a
generic function (see Code Specialization below) — so a method used at two
types is two bodies, not one that guesses.

See [Gradual Constraints](gradual-constraints.md) for inference rules, smart error messages, and edge cases.

## How Conformance Is Checked

A conformance declaration provides the trait's methods (or inherits them from methods already on the type):

```rask
extend Point with Comparable {
    func compare(self, other: Point) -> Ordering {
        // Custom implementation
    }
}
```

An empty `extend Point with Comparable {}` declares conformance using methods the type already has. Either way, the compiler checks each required method:
1. Method exists on the type (not a free function)
2. Parameter types match exactly
3. Return type matches exactly
4. Self parameter matches (value/mut/none)

| Type has | Trait requires | Satisfied |
|----------|----------------|-----------|
| `func compare(self, other: T) -> Ordering` | `compare(self, other: T) -> Ordering` | Yes |
| `func compare(self, other: T) -> i32` | `compare(self, other: T) -> Ordering` | No (return type mismatch) |
| `func compare(a: T, b: T) -> Ordering` | `compare(self, other: T) -> Ordering` | No (free function, not method) |

For `duck trait`, the same signature check runs at the use site against the type's own methods — no declaration involved. Errors point at the declaration for nominal traits and at the use site for duck ones.

## Conformance Declarations

| Rule | Description |
|------|-------------|
| **CD1: Conformance list** | `extend T with A, B, C { ... }` declares all listed conformances. Each trait's signature check runs independently against the block plus the type's existing methods. Modifiers (`public extend`, `scoped extend`) apply to every listed trait |
| **CD2: Block body unrestricted** | The block may mix methods for any of the listed traits and ordinary non-trait methods. The conformance list is a header on a normal extend block, not a per-trait container |
| **CD3: Composite chain** | Declaring a composite (`extend T with HashKey {}`) checks the full supertrait chain (TD3); auto-derived supertraits satisfy automatically, missing methods error at the declaration |

<!-- test: skip -->
```rask
// The common shape for a trait-rich type: one block, header carries the claims
extend LogSource with Reader, Displayable, Error {
    func read(mutate self, buf: Buffer) -> usize or IoError { ... }
    func display(self) -> string { ... }
    func message(self) -> string { ... }
    func rewind(mutate self) { ... }            // plain method, same block
}
```

## Method Namespace

One type, one method name, one meaning — with an opt-out scoped to the collision.

| Rule | Description |
|------|-------------|
| **MN1: Single namespace** | Methods defined in `extend T with Trait { }` are ordinary methods of T, same namespace as plain `extend T` blocks |
| **MN2: Shared implementation** | Two conformances requiring the same method name share the one implementation — legal iff both signatures match it. One implementation means one definition: two blocks each defining `label` on the same type is a duplicate method, whichever traits they name (XC3) |
| **MN3: Conflict needs scoping** | If the signatures disagree, the second conformance declaration is a compile error naming both traits — unless it is declared `scoped`. This covers two applied forms of one generic trait (`Mul<f64>` and `Mul<Meters>` on the same type) as much as two different traits. `scoped` is parsed but not yet honoured ([#1303](https://github.com/rask-lang/rask/issues/1303)), so today the error stands either way |
| **MN4: Scoped conformance** | `scoped extend T with Trait { ... }` — methods in a scoped conformance do not enter T's inherent namespace. Reachable through trait dispatch (generic bounds, `any Trait`) and trait-qualified calls |
| **MN5: Trait-qualified call** | `Trait.method(value, args)` — mirrors `Type.method()` static-call syntax. Legal for any conformance, needed only for scoped ones |

<!-- test: skip -->
```rask
extend Dog with Greeter {
    func greet(self) -> string { ... }               // ordinary method: dog.greet()
}

scoped extend Dog with Announcer {
    func greet(self, volume: i32) -> string { ... }  // trait-only
}

dog.greet()                 // Greeter's — the inherent one
Announcer.greet(dog, 5)     // Announcer's — qualified
```

## Override Coherence

The core-trait family carries cross-trait contracts (`a == b` implies `hash(a) == hash(b)`; `compare` agrees with `eq`) that data structures physically rely on. Auto-derive keeps them consistent by construction; overrides must not silently break that.

| Rule | Description |
|------|-------------|
| **OC1: Override cancels dependents** | Overriding `Equal` cancels auto-derived `Hashable` and `Comparable` for that type. Overriding `Hashable` alone is safe (hashing fewer fields than eq compares costs collisions, never correctness) and cancels nothing |
| **OC2: Loud, with the fix** | Using a cancelled conformance is a compile error at the use site naming the override and the fix: declare the dependent trait consistent with the new eq |
| **OC3: Canonical order only** | `Comparable` is the type's one canonical order. The OC diagnostics steer one-off orderings ("sort by salary") to `sort_by` |

## Cross-Package Conformance

There is no orphan rule. Any package may declare `extend T with Trait` for a type and a trait it doesn't own — except for six auto-derived traits that decide what happens to the type's data, which belong to its owner and nobody else.

| Rule | Description |
|------|-------------|
| **XC1: Contract traits belong to the owner** | `extend T with Equal`, `Hashable`, `Comparable`, `Cloneable`, `Encode` or `Decode` is legal only in the package that declares `T`. From any other package it's a compile error, the empty-body form included. All six are auto-derived for every eligible type (EQ1/HA1/CO1/CL1, `std.encoding/E12`), so a third party never needs one |
| **XC2: Everything else is open** | For every other trait, `extend T with Trait` is legal wherever both names are visible. No newtype wrapper, no forwarding methods, no ceremony for the case that has no conflict |
| **XC3: Two conformances never resolve silently** | Two declared conformances for the same (type, trait) pair are a compile error, never a pick. The pair is the *applied* trait, so two different applied forms of one generic trait are two conformances, not one declared twice — that they can still collide on a method name is MN3's, reported once and not twice. Both in one package: the error is at the second declaration. In two packages: at the place that needs the conformance, so a collision nobody uses costs nothing |
| **XC4: Visibility is the user's, not the build's** | A conformance is visible to a package iff the declaring package is in *that* package's dependency graph. A library keeps using its own conformance even when the program linking it also pulls in someone else's |
| **XC5: Conformance is part of the instantiation** | A generic instance is keyed by its type arguments *and* the conformances resolved for its bounds. `show<Doc>` under two different `Labeled` conformances is two instances, so neither can silently get the other's code |
| **XC6: Disambiguation is a nominal type** | Nothing names a conformance, so there is no syntax for choosing between two. `type MyDoc = Doc` is a distinct type (`type.aliases/T2`) that carries its own conformance (`T13`) |

### Why these six are carved out

The hazard worth a language rule isn't two packages disagreeing about behavior — it's a third party changing what happens to data the owner is responsible for. Two shapes of that, and both are silent.

**The four, by disagreeing.** Package D fills a `Map` keyed by `Json` using its own `Hashable`, package E probes that map with a different one, and entries that are plainly there can't be found. Nothing errors, nothing crashes, the lookup just says no. Every container in the stdlib rests on these four, every eligible type already has the one version the compiler derived (G1), and only the owner may replace it (EQ2/HA2/CO2, OC1).

**`Encode`/`Decode`, by overruling.** These two can't disagree: they're markers with no methods (`std.encoding/E11`), so a conformance block has no body, and the encoding is derived from the type's fields either way. What a third-party marker *can* do is widen what's serializable — a type whose owner wrote `@no_encode` because putting this value on a wire is meaningless or unsafe (`std.encoding/E16`) gets serialized anyway, decided by a package the owner has never read. `@no_encode` is the owner saying no about their own type's data. Someone else's `extend` is not the place that gets overturned.

`Debug` and `Error` are auto-derived too and are *not* carved out. Nothing is stored, keyed or transmitted on their say-so; a third-party `Debug` changes what a log line says, which is the ordinary ambiguity XC3 covers. The six are a closed list: a trait that turns out to decide what happens to someone else's data gets its own design round, not a quiet addition here.

### What XC3 does and doesn't buy

XC3 makes a collision loud in the program that has it. What it can't do is see a collision no single program has: two packages that never depend on each other each use their own conformance, nothing is ambiguous from either side, and nothing errors.

XC1 empties that of consequence for the stdlib's contracts — a hash can't be third-party, so two packages keying the same type are keying it the same way. What's left is a user trait carrying a contract of its own: a user-written sorted container instantiated under two different orderings. XC5 keeps the instantiations apart so neither runs the other's code, but a value built by one and probed by the other still misbehaves, and no diagnostic in this design catches it.

That residual is the price of having no orphan rule, and it's bounded: it takes two packages that independently conform the same foreign type to the same foreign trait, *and* a value crossing between them. Rust charges every user a newtype wrapper to rule it out. Rask charges the one user who hits it, and charges the same wrapper (XC6) — one line, where it's actually needed.

### Resolving a collision

In order of what to reach for:

1. **Drop one dependency.** Two packages conforming the same foreign type to the same foreign trait usually means they overlap in more than this.
2. **Move the use down.** Put the code that needs the conformance in a package that depends on one of the two. XC4 means it sees one conformance and compiles.
3. **Wrap it.** `type MyDoc = Doc` plus your own `extend MyDoc with Labeled { ... }`.

Step 3 gives you a type that compiles; it does not give you liba's behavior. Nothing names a conformance, and where both are in scope the method name collides too (MN1 puts conformance methods in the type's inherent namespace), so the wrapper can't delegate to either — it writes its own body. Keeping one of the two implementations is what step 2 is for: a package that sees one conformance also sees exactly one `label`.

### Error Messages

**Third-party contract-trait conformance [XC1]:**
```
error[E0409]: only `traitpkg` can declare `Hashable` for `traitpkg.Doc`
   |
4  |  public extend Doc with Hashable {
   |  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ this block is in `liba`
   |
7  |  public struct Doc {
   |  ----------------- `traitpkg.Doc` belongs to `traitpkg`

FIX: put the behaviour you want on a type of your own:
       type MyDoc = traitpkg.Doc
       extend MyDoc with Hashable { … }

WHY: `Hashable` is one answer per type — `Map`, `Set` and every sort built
     on them assume `traitpkg.Doc` answers the same way everywhere. A second
     answer from another package doesn't conflict loudly; it makes lookups
     miss entries the container holds. Only `traitpkg` can change the one
     `traitpkg.Doc` already has (type.generics/XC1).
```

`Encode`/`Decode` are the same rule and a different sentence — there is no
second encoding to conflict with, only someone else's `@no_encode` being
overruled, so the message says that instead. It doesn't wait for the
annotation: the rule is who decides, and a type with no annotation is one
whose owner hasn't decided yet.

```
error[E0409]: only `traitpkg` can make `traitpkg.Secret` encodable
   |
4  |  public extend Secret with Encode {
   |  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ this block is in `liba`
   |
5  |  public struct Secret {
   |  -------------------- `traitpkg.Secret` belongs to `traitpkg`, which
   |                       decides whether its data goes on a wire

FIX: if you need these fields on a wire, carry them in a type `liba` owns:
       struct SecretWire { … }

WHY: `Encode` has no methods — declaring it doesn't change how
     `traitpkg.Secret` serializes, it changes whether it does. That is the
     declaring package's call, and a type its owner marked `@no_encode`
     would be overruled from outside (type.generics/XC1).
```

**Two conformances in scope [XC3]:**
```
ERROR [type.generics/XC3]: two conformances of `Doc` to `Labeled` are in scope
   |
9  |  println(show(d))
   |          ^^^^ `show` needs `Doc: Labeled`, and two packages declare it
   |
   = liba/defs.rk:5   declared by `liba`
   = libb/defs.rk:5   declared by `libb`

WHY: Picking one would come down to link order. Which `label()` runs has
     to be something the source says.

FIX: Give the collision a type of its own, and say what it does:

  type MyDoc = traitpkg.Doc
  extend MyDoc with Labeled {
      func label(self) -> string { return "doc {self.value.n}" }
  }

     To keep one of the two implementations instead, move the code that
     needs it into a package that depends on `liba` or on `libb`, not both.
```

That's E0410, and it is what ships: two blocks in one package are reported at
the second declaration, two packages at the place that needs the conformance,
and a collision nobody asks for costs nothing.

XC4 decides who counts as seeing both. `liba` sees one and compiles; the
program that depends on `liba` and `libb` sees two and gets the error above.

That only means anything because XC5 keeps the two bodies apart underneath. A
conformance more than one package declares carries the declaring package in its
symbol — `Doc_label` becomes `Doc_label_liba` — the checker records which one
each call resolved to, and monomorphization emits both. Without it there is one
`Doc_label` and the block read last wins, so `liba` calling its own function
ran `libb`'s body and printed `b:7`.

## Conditional Conformance

| Rule | Description |
|------|-------------|
| **CC1: Conditional conformance** | Conformance on a generic type holds exactly for instantiations satisfying its condition, checked at monomorphization like every other bound (G2/G6) |
| **CC2: Explicit condition** | Conformances on generic types state the condition with `where` — public and package-private alike. Inferring the clause from the conformance body (gradual-constraints machinery) is deferred; relaxing to inference later is purely additive |
| **CC3: One condition per block** | A `where` clause applies to the whole block. Traits needing different conditions split into separate blocks |

<!-- test: skip -->
```rask
extend Ring<T> with Displayable where T: Displayable {
    func display(self) -> string {
        return self.items.map(|x| x.display()).join(", ")
    }
}
```

This is the same conditionality auto-derive has always applied implicitly ("Vec of Cloneable is Cloneable" — CL1), with syntax for user traits. The clause only exists on generic types conforming to traits; concrete types never write it.

## Operator Expansion

The compiler expands operators into method calls before type checking (G4), then verifies the method exists.

| Operator | Desugars To | Trait Requirement |
|----------|-------------|-------------------|
| `a + b` | `a.add(b)` | `add(self, other: T) -> T` |
| `a - b` | `a.sub(b)` | `sub(self, other: T) -> T` |
| `a * b` | `a.mul(b)` | `mul(self, other: T) -> T` |
| `a / b` | `a.div(b)` | `div(self, other: T) -> T` |
| `a == b` | `a.eq(b)` | `eq(self, other: T) -> bool` |
| `a < b` | `a.compare(b) == Less` | `compare(self, other: T) -> Ordering` |

| Rule | Description |
|------|-------------|
| **OP1: Concrete operators are authored sugar** | Operator expansion on concrete types is method-call sugar — no conformance involved. The method being called was deliberately written on that type; nothing is matched, so the accidental-conformance hazard nominal conformance addresses doesn't exist here. Generic operator use goes through nominal bounds (`Numeric`, `Comparable`) like any other generic call |

## Compiler-Verified Cloneable

The compiler auto-derives Cloneable where all fields implement Cloneable and no raw pointers exist (G5).

| Rule | Description |
|------|-------------|
| **CL1: Auto-derive** | Primitives, structs with all Cloneable fields, arrays/Vec of Cloneable, handles: auto-derived |
| **CL2: Pointer block** | Struct with raw pointer is NOT Cloneable unless `unsafe extend` |

```rask
trait Cloneable {
    func clone(self) -> Self
}
```

| Type | Cloneable Status |
|------|------------------|
| Primitives (i32, bool, f64) | Auto-derived (bitwise copy) |
| Struct with all Cloneable fields | Auto-derived (deep copy) |
| Struct with raw pointer | NOT Cloneable unless `unsafe extend` |
| Array/Vec of Cloneable | Auto-derived (element-wise clone) |
| Handle types | Auto-derived (handle copy, not referent) |

## Compiler-Verified Equal

The compiler auto-derives Equal where all fields implement Equal — same pattern as Cloneable.

| Rule | Description |
|------|-------------|
| **EQ1: Auto-derive** | Primitives, structs with all Equal fields, enums (tag + payload equality): auto-derived |
| **EQ2: Override** | `extend Type with Equal { ... }` overrides the auto-derived version |
| **EQ3: Enum equality** | Variants compared by tag, then field-wise payload equality |

```rask
struct Point {
    x: i32
    y: i32
}

// No extend block needed — Point is Equal because i32 is Equal
let a = Point { x: 1, y: 2 }
let b = Point { x: 1, y: 2 }
// a == b → true (field-wise comparison)
```

| Type | Equal Status |
|------|--------------|
| Primitives (i32, bool, f64, string) | Auto-derived |
| Struct with all Equal fields | Auto-derived (field-wise) |
| Enum with all Equal payloads | Auto-derived (tag + payload) |
| Struct with `any Trait` field | NOT Equal unless manually implemented |
| Struct with closure field | NOT Equal (closures have no equality) |

## Compiler-Verified Hashable

The compiler auto-derives Hashable where all fields implement Hashable. Since Hashable requires Equal (supertrait), auto-derive applies only when both are satisfied.

| Rule | Description |
|------|-------------|
| **HA1: Auto-derive** | Primitives, structs with all Hashable fields, enums (tag + payload hash): auto-derived |
| **HA2: Override** | `extend Type with Hashable { ... }` overrides the auto-derived version |
| **HA3: Hash combine** | Field-wise hash uses deterministic combine (order matches declaration order) |
| **HA3a: What a scalar's hash is** | `x.hash()` on an integer, a `bool`, a `char` or a `string` is FNV-1a over the value's little-endian bytes at its own width — the same function an int-keyed Map buckets with, so a value and the same value used as a key agree. Unseeded: a hash is as stable as `==`. The width counts, so `5u32` and `5u64` don't hash alike |
| **HA4: Float exclusion** | `f32` and `f64` are NOT Hashable (NaN != NaN violates Hashable contract). So `Map<f64, V>` is a compile error — including nested, as in `Vec<Map<f64, V>>`. A float *value* is fine; only the key position is excluded |
| **HA5: Bits as the hatch** | `x.to_bits() -> u64` reinterprets a float's bit pattern, so a caller who wants a float-keyed Map spells out what "the same key" means. u64 at both widths. Distinct values get distinct keys, and unlike a float key a NaN can be looked up again |

| Type | Hashable Status |
|------|-----------------|
| Integer primitives, bool, char, string | Auto-derived |
| `f32`, `f64` | NOT Hashable (NaN breaks equality) |
| Struct with all Hashable fields | Auto-derived (field-wise hash combine) |
| Enum with all Hashable payloads | Auto-derived (tag + payload) |
| Tuple / fixed array of Hashable elements | Auto-derived, element-wise (`type.tuples/TU11`) |
| Handle types | Auto-derived (hash of index + generation) |
| Nominal newtype (`type Id = u64 with (…)`) | Only when the clause lists `Hashable` — a newtype inherits nothing it doesn't name (`type.aliases/T11`) |

## Compiler-Verified Comparable

The compiler auto-derives Comparable where all fields implement Comparable — lexicographic by declaration order. Since Comparable requires Equal (supertrait), auto-derive applies only when both are satisfied.

| Rule | Description |
|------|-------------|
| **CO1: Auto-derive** | Primitives, structs with all Comparable fields, enums (variant order, then payload): auto-derived |
| **CO2: Override** | `extend Type with Comparable { ... }` overrides the auto-derived version |
| **CO3: Lexicographic** | Fields compared in declaration order — first field is most significant |
| **CO4: Floats included** | `f32`/`f64` are Comparable. `compare()` is a total order so `sort`, `min`, `max` and every `T: Comparable` helper work on them; the operators `<`, `>`, `<=`, `>=` stay IEEE, so a comparison against `NaN` is `false`. See `type.operators/ORD3` |

```rask
trait Comparable: Equal {
    func compare(self, other: Self) -> Ordering
}

enum Ordering { Less, Equal, Greater }
```

| Type | Comparable Status |
|------|-------------------|
| Integer primitives, bool, char, string | Auto-derived |
| `f32`, `f64` | NOT Comparable (NaN breaks totality) |
| Struct with all Comparable fields | Auto-derived (lexicographic by field order) |
| Enum with all Comparable payloads | Auto-derived (variant order, then payload) |
| Struct with float field | NOT Comparable unless manually implemented with `.total_cmp()` |

<!-- test: skip -->
```rask
struct Version {
    major: u32
    minor: u32
    patch: u32
}

// No extend block needed — Version is Comparable because u32 is Comparable
// Compares major first, then minor, then patch (lexicographic)
let a = Version { major: 1, minor: 2, patch: 0 }
let b = Version { major: 1, minor: 3, patch: 0 }
// a < b → true (minor field differs)
// a.compare(b) → Ordering.Less
```

## No Default Trait

There is no `Default` trait and no `.default()` method. Declared field defaults (`type.structs`) are the mechanism: a field with a declared default may be omitted at construction, and a struct whose fields all have defaults constructs with zero fields — `Config {}` *is* the default value. A struct with any defaultless field has no empty construction; the compiler names the missing field instead of inventing `""`/`0`. Universal zero-defaults were rejected as Go zero-values by another name.

## Comptime Generics

```rask
func dot<comptime N: usize>(a: [f32; N], b: [f32; N]) -> f32
```

Compiler infers `N` from array literals (`N = 2`) or known types (`arr: [f32; 5]`).

Errors if lengths differ, inference ambiguous, or non-literal const without explicit parameter.

## Must-Consume Types in Traits

Must-consume resource types (`@resource`) can be generic parameters. Narrowing over a `Resource?` must bind the value — wildcards are forbidden because that would silently drop the resource.

| Pattern | Resource content | Valid |
|---------|-----------------|-------|
| `if opt? as f` | Binds f | Yes, f must be consumed |
| `if opt?` (no bind, single-payload implicit) | Wildcard | No, compile error |
| `opt is none` branch | No value | Yes, nothing to consume |
| `if r? as f` on `Resource or E` | Binds f | Yes, f must be consumed |

## Trait Composition

Composition via `:` is additive (TD3). `T: HashKey` requires `hash`, `eq`, AND `clone`.

Compiler collects all methods from the full supertrait chain, deduplicates identical requirements, errors on conflicts.

```rask
trait Hashable: Equal {
    func hash(self) -> u64
}

trait HashKey: Hashable + Cloneable {}
// Requires: eq (from Equal), hash (from Hashable), clone (from Cloneable)
```

## Code Specialization (Monomorphization)

When you call `sort<i32>` and `sort<string>`, the compiler generates two separate `sort` functions — one optimized for `i32`, one for `string` (G6). Trait matching is verified at each call site. No whole-program analysis.

| Aspect | Behavior |
|--------|----------|
| Code size | Each type usage generates its own copy of the function |
| Type checking | Performed per usage with concrete types |
| Error location | Reported at the call site |
| Compilation | Incremental per compilation unit |

## Numeric Literals in Generics

Integer literals auto-coerce to T when `T: Numeric`. Compiler inserts `T.from_int()`. IDE shows ghost text.

```rask
trait Numeric {
    func add(self, other: Self) -> Self
    func zero() -> Self
    func one() -> Self
    func from_int(n: i64) -> Self
}

func increment<T: Numeric>(val: T) -> T {
    val + 1  // Compiler inserts: val + T.from_int(1)
}
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Zero usages | G2 | Function body syntax-checked; type errors may be deferred |
| Recursive generics | G6 | `Vec<Vec<T>>` allowed; compiler prevents infinite expansion |
| Trait visibility | TD1 | Package-visible by default, `public trait` exports — same rule as structs and functions (`struct.modules/V1`) |
| Same method required by two traits | MN2/MN3 | Same signature: shared implementation. Different: `scoped` or error |
| Third party declares `Hashable` or `Encode` for a foreign type | XC1 | Compile error (E0409) at the `extend`, whatever the body. Wrap in a nominal type instead |
| Third party declares any other trait for a foreign type | XC2 | Legal, no wrapper needed |
| Two packages declare the same (type, trait), nobody uses it | XC3 | Not an error — the check is where the conformance is required |
| One package declares the same (type, trait) twice | XC3 | Compile error at the second declaration |
| A library and the program linking it see different conformances | XC4/XC5 | Each uses the one its own dependencies give it; the two bodies are separate symbols |
| Trait evolution | TD2 | Adding a required method with a default body is non-breaking; without one it breaks every conformer (major version) |
| Generic struct fields | G1 | `struct Foo<T: Comparable>` requires T: Comparable at every usage |
| Negative constraints | — | Not in MVP; workaround via naming convention or separate functions. `T or E` disjointness is the one exception and needs no syntax (GF4) |
| `f<T>() -> T or E` called with `T = E` | GF4 | Compile error on the call, naming the parameter (`type.errors/ER3a`) |
| `f<T>() -> T?` called with `T = U?` | — | Legal — optionals nest, layers stay distinct (`type.optionals/OPT28`) |
| Associated types | `type.associated-types/AT1`–`AT10` | Promoted. Read off a unique conformance; equality constraints (`where T.Out == U`) stay out (AT7) |
| More than 2 type params | — | Not in MVP; traits limited to 1-2 parameters |
| Omitted bounds (private) | GF2 | Inferred from body; see [Gradual Constraints](gradual-constraints.md) |
| Container method access | GF1 | Methods on containers (like `Vec<T>.len()`) don't require constraints on T |

---

## Appendix (non-normative)

### Rationale

**G1 (declared conformance):** This flipped. The original design matched by shape by default, with `explicit trait` as the opt-out — chosen to avoid global impl tracking. Two things overturned it: accidental conformance is silent-wrong (a `compare()` that isn't a total order satisfies `Comparable` structurally and misbehaves instead of erroring), and the declaration's cost dropped — one line that states intent is cheap, especially when most code is machine-written and human-reviewed. Checking stays local: a declaration is checked where it's written, and bounds are still checked at the use site — no whole-program analysis either way. `duck trait` keeps shape-matching available for sketching, package-internal by DT1.

**MN1–MN5 (single namespace):** Under shape-matching, one method satisfied every matching trait by construction; nominal conformance created the "which trait owns this method" question. Single namespace matches how people think ("Dog has a greet method") and keeps `dog.greet()` working when `greet` was defined inside a conformance block. The collision case is rare, and `scoped` puts the ceremony exactly on the declaration that collides — no Rust-style qualified-call syntax tax on everyone.

**OC1 (override cancels dependents):** Auto-derive keeps the eq/hash/compare contracts consistent by construction; a declared Equal override paired with an untouched auto-derived hash is the one guaranteed-inconsistent state (Map entries silently vanish). Cancellation plus a loud error removes it. The compiler can't verify a hand-written hash is consistent — no compiler can — but it can refuse to pair your eq with a hash you never looked at.

**OP1 (operators stay authored):** The flip's hazard — being *matched* against a contract you never claimed — requires a bound satisfied by accident. Concrete operator use has no bound; it calls a method someone deliberately wrote. The generic path was already nominal. Watch-item: if public generic code over-constrains through `Numeric` when it only needs `add`, split Numeric from usage evidence — not preemptively.

**Default (removed):** Zero corpus usage, and DF-style universal zeros were Go zero-values by another name — a back door around Rask's all-fields-required construction. Declared field defaults replaced the trait; `Config {}` is the default value when every field declares one. No spec API used `T: Default` as a bound at removal time; a constructible-empty bound can return from usage evidence if ever needed.

**G4 (operator expansion):** Makes numeric code ergonomic — `a + b` reads naturally while the trait system handles dispatch.

**G5 (verified clone):** Compiler-verified Cloneable prevents aliasing bugs. Types with raw pointers can't silently claim to be cloneable.

**G6 (code specialization):** Keeps costs transparent and compilation fast. Each usage generates specialized code — no hidden function-pointer overhead.

**`duck trait`:** The opt-in that replaced `explicit trait` when the default flipped, renamed from `structural` (jargon). The register is deliberate — the keyword reading as unserious *is* the signal that the contract is loose by design. Sketch with it, delete the keyword to harden (the compiler generates the missing declarations). The stdlib ships zero duck traits; docs note the concept is known elsewhere as structural typing.

**XC1–XC6 (cross-package conformance):** Rust's orphan rule is the most-hated restriction in the language, and it exists for a real reason — two crates defining conflicting impls that then link together. What's wrong with it isn't the goal, it's the billing. Every user pays a newtype wrapper and a wall of forwarding methods, forever, as insurance against a conflict that almost never happens.

Rask inverts that because it can. The conflicts that actually corrupt data go through traits that are already auto-derived with one canonical version per type (G1) — so forbidding third-party versions of exactly those costs nobody anything and removes the corruption class outright. What's left is ambiguity, not corruption, and ambiguity can be reported. XC3 reports it, at the place that has it, naming both packages. The common case pays nothing; the rare real collision pays a newtype — the same thing Rust charges everyone.

The carve-out started at four — the traits the stdlib's containers key on — on the reasoning that a duplicate `Encode` is only ambiguity. That was right about `Encode` not disagreeing and wrong about it being harmless, and the rest of the reasoning had to change with it. `Encode` and `Decode` are markers (`std.encoding/E11`): there is no second implementation to conflict, because there is no implementation. What a third party gets by declaring one is the *decision*, and the decision `@no_encode` records is that this type's data does not go on a wire. A package that never wrote the type shouldn't be the one to reverse that, so XC1 covers all six.

Which makes the rule less "these traits get baked into data structures" and more "these traits decide what happens to data whose owner is someone else". `Debug` decides what a line of a log looks like and stays out.

XC5 is the part that makes XC3 more than a slogan. Two conformances in one build, resolved per instantiation (XC4), means the same generic at the same type argument can need two bodies. If the monomorphization key were just the type arguments, one of them would silently win and which one would depend on link order — the exact regression this design exists to prevent, reintroduced at the back.

That turned out to be true of plain methods as well, not just generic instances. Two `extend Doc with Labeled` blocks in two packages both put a `label` on one `Doc`, and both mangled to `Doc_label`: `liba` called its own function, which called `d.label()`, and ran `libb`'s body. So the declaring package is part of the symbol wherever more than one declares it — `Doc_label~liba` — and the checker records which block each call resolved to. It costs nothing in a program without a collision, where there is one block and the name is unchanged.

The separator is `~` because nothing else in a generated name uses it. `_` already means "qualified by a package" (`Doc` in `traitpkg` is `Doc_traitpkg`), so `Doc_label_liba` is also what a method *named* `label_liba` would produce; `$` is the type-argument separator, so `Doc_label$liba` reads as an instantiation. A Rask identifier can't contain `~`, so nothing a program declares can collide with it.

XC6 admits what it can't do: there is no syntax for "use liba's". Adding one would mean naming conformances, which means a second identity for something that already has a type and a trait. The cases that need it are served by structure — put the use in a package that sees one conformance — and the case that doesn't want either writes its own. I'd rather ship the gap than the naming scheme.

**DT1 (why a hard error):** The first cut said "prototype with it, harden later" and left later up to the author. That's fine advice and a bad guarantee — it stops being advice the moment a duck trait crosses a package boundary, where the shape-matching turns into a versioning hazard nobody can see from either side. DT1 turns it into a check, and it's a check the type system can do locally at the declaration: no whole-program analysis, no notion of "package intended for publication," just `public` plus `duck` on one line.

**Why DT2 isn't a gate too:** the first draft of this rule banned duck traits from published packages outright. That was harsher than the problem. Trace a private duck trait in a published package and the hazard doesn't survive: a consumer can't see the trait, can't name it, and can't have their own types satisfy it. Drop a method and one of your own types stops matching — a compile error in your own build, caught where every other internal break is caught. The versioning trap needs two parties, and DT1 already guarantees there's only one.

What's left for a publish gate is a discipline claim — "a published package isn't a scratchpad" — and that's not enough to block a release over. It would also forbid a legitimate shape: a small package whose internals are honestly still in flux. `type.gradual/GC11` declines to gate inferred private signatures for exactly this reason, and a private duck trait is the same shape; gating one and not the other would have been inconsistent. So DT2/DT3 report and DT4 makes acting on the report cheap. The line across both features: hard error where it can break someone else, a warning where it can only affect you.

### Patterns & Guidance

**Generic sorting:**

```rask
public func sort<T: Comparable>(items: Vec<T>) {
    for i in 1..items.len() {
        mut j = i
        while j > 0 && items[j] < items[j - 1] {
            swap(mut items[j], mut items[j - 1])
            j = j - 1
        }
    }
}
```

**HashMap with verified clone:**

```rask
trait HashKey: Hashable + Cloneable {}

public struct HashMap<K: HashKey, V> {
    buckets: Vec<Bucket<K, V>>
}

public func insert<K: HashKey, V>(map: HashMap<K, V>, key: K, val: V) {
    let idx = key.hash() % map.buckets.len()
    map.buckets[idx].add(key.clone(), val)  // Cloneable is compiler-verified deep copy
}
```

### Integration Notes

- **Memory model**: Generic ownership rules same as non-generic; move/copy determined per concrete type
- **Type system**: Conformance declared and checked locally at the `extend` block; bounds checked at use site. The one thing that isn't local is the (type, trait) table XC3 reads to spot a second conformance — a lookup, not an analysis pass. `duck trait` checks shape at use site, and DT1 is a declaration-local visibility check
- **Build system**: `rask publish` scans the package for `duck trait` declarations and warns (DT2, `struct.build/PB8`) — a syntactic check over the package's own sources, no dependency analysis, no effect on whether the release proceeds
- **Concurrency**: Generic tasks can send owned generic values; traits verified per concrete type
- **Compiler**: Specialization happens per compilation unit; no cross-unit analysis. The instance key carries the resolved conformances alongside the type arguments (XC5)
- **C interop**: Generic functions cannot be exported to C (no stable ABI); specialized wrappers required
- **Error handling**: Generic functions with `T or E` work normally; must-consume tracking per concrete type
- **Closures**: Generics in closures capture by value; traits verified at closure usage
- **Runtime polymorphism**: `any Trait` enables heterogeneous collections; see `type.traits`

### Standard Library Traits

| Trait | Methods | Auto-Derived? |
|-------|---------|---------------|
| `Equal` | `eq(self, other: Self) -> bool` | Yes — all Equal fields (EQ1) |
| `Comparable`: Equal | `compare(self, other: Self) -> Ordering` | Yes — all Comparable fields, lexicographic (CO1) |
| `Hashable`: Equal | `hash(self) -> u64` | Yes — all Hashable fields, no floats (HA1) |
| `Cloneable` | `clone(self) -> Self` | Yes — all Cloneable fields, no raw pointers (CL1) |
| `Error` | `message(self) -> string` | Yes — enums, from variant names + payloads (`type.errors/ER6`); structs declare |
| `Displayable` | `display(self) -> string` | No — opt-in (user-facing output is intentional) |
| `Debug` | `debug(self) -> string` | Yes — all types |
| `Numeric` | `add, sub, mul, div, neg, zero, one, from_int` | No |
| `Convert<From, To>` | `convert(self: From) -> To` | No |
| `Encode` | Marker — no methods | Yes — all-Encode public fields (`std.encoding/E12`) |
| `Decode` | Marker — no methods | Yes — all-Decode public fields (`std.encoding/E12`) |

Six of these decide what happens to a type's data: `Equal`, `Hashable`, `Comparable`, `Cloneable` get baked into the data structures holding it, and `Encode`/`Decode` say whether it may be serialized at all. All six are auto-derived, owner-overridable, and never third-party (XC1).

### See Also

- [Traits](traits.md) — Trait definitions and `any Trait` polymorphism (`type.traits`)
- [Structs](structs.md) — Struct definitions and methods (`type.structs`)
- [Enums](enums.md) — Enum types (`type.enums`)
- [Gradual Constraints](gradual-constraints.md) — Bound inference for private generics
- [Resource Types](../memory/resource-types.md) — Must-consume types (`mem.resource-types`)
