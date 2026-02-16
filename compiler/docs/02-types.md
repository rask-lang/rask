# Type System Deep Dive

How the Rask type checker works internally. This is `rask-types/src/checker/`—
16 files that figure out the type of every expression and catch type errors.

## The big picture

The type checker does three things:

1. **Register type definitions** — Walk all struct/enum/trait declarations,
   build a `TypeTable` so we know what types exist and what fields/variants
   they have.
2. **Check and infer** — Walk every expression and statement, assigning a type
   to each one. When types aren't annotated, create **type variables** and
   **constraints**.
3. **Solve constraints** — Run the unification algorithm to turn type variables
   into concrete types. Report errors if constraints conflict.


## The Type enum

`rask-types/src/types.rs` defines what a type looks like:

```rust
enum Type {
    // Primitives
    I8, I16, I32, I64, I128,
    U8, U16, U32, U64, U128,
    F32, F64, Bool, Char, String, Unit,

    // Compound
    Named(String),                  // user-defined: Point, Color
    Generic(String, Vec<Type>),     // parameterized: Vec<i32>, Map<string, i32>
    Tuple(Vec<Type>),               // (i32, string)
    Function(Vec<Type>, Box<Type>), // function types: (i32, i32) -> i32
    Option(Box<Type>),              // T?
    Result(Box<Type>, Box<Type>),   // T or E

    // Inference
    Var(TypeVarId),                 // unknown type, to be solved
    Never,                          // ! type (diverges)
    // ...
}
```

`Type::Var` is the key to inference. When the checker doesn't know a type yet,
it creates a fresh variable. Later, unification figures out what it should be.


## The TypeChecker struct

```rust
pub struct TypeChecker {
    resolved: ResolvedProgram,          // from name resolution
    types: TypeTable,                   // struct/enum/trait definitions
    ctx: InferenceContext,              // type variable + constraint state
    node_types: HashMap<NodeId, Type>,  // result: type of each node
    symbol_types: HashMap<SymbolId, Type>,
    errors: Vec<TypeError>,
    current_return_type: Option<Type>,  // for checking return statements
    current_self_type: Option<Type>,    // inside extend blocks
    local_types: Vec<HashMap<String, (Type, bool)>>,  // scope stack
    borrow_stack: Vec<ActiveBorrow>,           // ESAD Phase 1
    persistent_borrows: Vec<PersistentBorrow>, // ESAD Phase 2
    // ...
}
```

**`local_types`** is a scope stack. Each entry is a map from variable name to
`(type, is_read_only)`. When you enter a block, push a new map. When you
leave, pop it. Lookup searches from innermost to outermost.

The `bool` tracks mutability. Default parameters are read-only; `mutate`
params are writable.


## How inference works

Consider `const x = 42`. The type checker:

1. Creates a fresh type variable `?T0` for `x`.
2. Checks the right side: `42` has type `i32`.
3. Adds a constraint: `Equal(?T0, i32, span)`.
4. After processing all declarations, runs `solve_constraints()`.
5. The solver unifies `?T0` with `i32`, replacing `?T0` everywhere.

The state lives in `InferenceContext` (`inference.rs`):

```rust
pub struct InferenceContext {
    next_var: u32,                           // counter for fresh vars
    substitutions: HashMap<TypeVarId, Type>, // solved bindings
    constraints: Vec<TypeConstraint>,        // pending constraints
}
```

`fresh_var()` creates a new `Type::Var(id)` and bumps the counter.

`apply(ty)` walks a type, replacing any `Var(id)` that has a substitution.
This is called whenever you need the "current best known" type.


## Constraints

There are three kinds of constraints (`inference.rs`):

```rust
enum TypeConstraint {
    Equal(Type, Type, Span),
    // "these two types must be the same"

    HasField { ty, field, expected, span },
    // "this type must have a field with this name and type"

    HasMethod { ty, method, args, ret, span },
    // "this type must have a method with this signature"
}
```

`Equal` is the most common. It's generated when:
- A binding has an annotation: `const x: i32 = expr` → `Equal(i32, typeof(expr))`
- A function is called: each argument type must equal the parameter type
- A return statement: the expression type must match the function's return type

`HasField` and `HasMethod` are generated when the receiver type is still a
variable. Instead of immediately looking up the field/method (which we can't do
because we don't know the type yet), we defer it as a constraint.


## Unification

`unify.rs` solves `Equal` constraints. The algorithm:

```rust
fn unify(&mut self, t1: &Type, t2: &Type, span: Span) -> Result<bool, TypeError> {
    let t1 = self.ctx.apply(t1);  // resolve any known substitutions
    let t2 = self.ctx.apply(t2);

    match (&t1, &t2) {
        // Already equal → nothing to do
        (a, b) if a == b => Ok(false),

        // Unit and empty tuple are the same
        (Type::Tuple(elems), Type::Unit) if elems.is_empty() => Ok(false),

        // Variable on either side → bind it
        (Type::Var(id), other) | (other, Type::Var(id)) => {
            // Occurs check: prevent infinite types like T = Vec<T>
            if self.ctx.occurs_in(*id, other) {
                return Err(TypeError::InfiniteType { ... });
            }
            self.ctx.substitutions.insert(*id, other.clone());
            Ok(true)  // made progress
        }

        // Generics: names must match, then unify each type argument
        (Type::Generic(n1, args1), Type::Generic(n2, args2))
            if n1 == n2 && args1.len() == args2.len() => {
            // unify each pair of args
        }

        // Functions: unify params pairwise, then return types
        (Type::Function(p1, r1), Type::Function(p2, r2)) => { ... }

        // Everything else → type mismatch
        _ => Err(TypeError::Mismatch { expected: t1, found: t2, span })
    }
}
```

The solver runs in a loop (max 100 iterations). Each pass takes all pending
constraints, tries to solve them. If any constraint made progress (`Ok(true)`),
loop again. Stop when nothing changes or limit is hit.

### The occurs check

Before binding `?T0 = SomeType`, we check that `?T0` doesn't appear inside
`SomeType`. Without this, you'd get infinite types: `?T0 = Vec<?T0>` would
expand to `Vec<Vec<Vec<...>>>` forever.


## How specific things are checked

### Expressions (`check_expr.rs`)

Every expression returns its type. Examples:

- **Literal**: `42` → `i32`, `"hello"` → `string`, `true` → `bool`
- **Identifier**: Look up in `local_types` scope stack or `symbol_types`
- **Method call**: After desugaring, `a + b` is `a.add(b)`. Look up `add` on
  the type of `a`, check argument types, return the method's return type.
- **Field access**: `point.x` → look up struct definition, find field `x`,
  return its type.
- **If expression**: Check condition is `bool`, check both branches have the
  same type (or unify them).
- **Closure**: Create function type from parameter annotations and inferred
  body type.

### Statements (`check_stmt.rs`)

- **`const x = expr`**: Infer type from `expr`, store in scope
- **`const x: T = expr`**: Check `expr` type matches `T`
- **`let x = expr`**: Same as const but mutable
- **Assignment `x = expr`**: Check `x` is mutable and types match
- **Return**: Check return type matches function signature

### Functions (`check_fn.rs`)

1. Register parameters in a new scope (with their annotated types)
2. Set `current_return_type` to the function's declared return type
3. Check each statement in the body
4. Verify all paths return the right type

### Patterns (`check_pattern.rs`)

Handles `match` arms and `if x is Some(v)` patterns. Checks that:
- Pattern variants exist on the matched enum
- Bindings get the right types
- All variants are covered (exhaustiveness isn't fully checked yet)

### Generics (`generics.rs`)

When a function is called with type parameters (explicit or inferred), creates
fresh type variables for each parameter and substitutes them into the signature.
The constraints from checking arguments then determine the concrete types.


## Aliasing detection (ESAD)

The type checker also catches aliasing violations at the type level, as part of
the ESAD (Expression-Scoped Alias Detection) system:

- **Phase 1 (`borrow_stack`)**: Within a single expression, tracks active
  borrows to catch `items[0] + items.push(1)` (reading and mutating the same
  collection in one expression).
- **Phase 2 (`persistent_borrows`)**: Across statements within a scope, tracks
  `const ref = items` borrows that persist across statements.

This catches aliasing problems earlier than the ownership checker, with
better error messages because we still have expression-level context.


## The submodule map

```
rask-types/src/checker/
├── mod.rs            TypeChecker struct, top-level check() function
├── check_expr.rs     Expression type checking
├── check_stmt.rs     Statement type checking
├── check_fn.rs       Function checking
├── check_pattern.rs  Pattern matching validation
├── declarations.rs   Register structs, enums, traits, impls
├── type_defs.rs      TypeDef, MethodSig, TypedProgram definitions
├── type_table.rs     TypeTable storage and lookup
├── inference.rs      InferenceContext, TypeConstraint, fresh vars
├── unify.rs          Constraint solving and unification
├── generics.rs       Generic type parameter handling
├── parse_type.rs     Parse type strings ("Vec<i32>") into Type values
├── builtins.rs       Built-in methods on primitive types
├── borrow.rs         Aliasing detection (ESAD)
├── errors.rs         TypeError definitions
└── resolve.rs        Symbol → Type resolution helpers
```
