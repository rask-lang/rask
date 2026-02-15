# Parallel Work Streams — Phase 3

Previous phase landed MirType::String, closure MIR lowering, and stdlib wiring.
This phase fixes the three blockers preventing real programs from compiling natively.

Smoke test results after Phase 2 merges:

| Program | Native | Issue |
|---------|--------|-------|
| hello, strings, arithmetic | Works | — |
| structs with field access | Works | — |
| loops, control flow | Works | (needs explicit `i64` suffixes) |
| closures (same-type captures) | Works | — |
| closures (mixed types) | Cranelift verifier error | i64 + i32 in binary op |
| `Vec.new()`, `Map.new()` | UnresolvedVariable | MIR doesn't know type namespaces |
| `Shape.Circle(5)` | UnresolvedVariable | Enum constructor = MethodCall on type name |
| `let x = 0` in `-> i64` func | Type error | Unsuffixed int defaults to i32 |

| Stream | Primary Crates | Depends On |
|--------|---------------|------------|
| 1. Type constructors + enum variants in MIR | `rask-mir/lower/expr.rs` | Nothing |
| 2. Integer literal inference | `rask-types/checker/check_expr.rs`, `rask-types/checker/check_stmt.rs` | Nothing |
| 3. Cross-type binary ops in codegen | `rask-codegen/builder.rs` | Nothing |

All three are independent. Stream 1 is MIR lowering. Stream 2 is the type checker.
Stream 3 is Cranelift codegen. No file overlap.

---

## Stream 1: Type Constructors and Enum Variants in MIR

**Files:** `compiler/crates/rask-mir/src/lower/expr.rs`
**Goal:** `Vec.new()`, `Map.new()`, `string.concat()`, `Shape.Circle(5)` lower to correct MIR calls.

### Problem

`Vec.new()` parses as:
```
MethodCall { object: Ident("Vec"), method: "new", args: [] }
```

The MIR lowerer (expr.rs:173) handles MethodCall by calling `lower_expr(object)`.
For `Ident("Vec")`, that hits the Ident case (expr.rs:52) which looks up "Vec" in
`self.locals` — fails with UnresolvedVariable.

Same pattern for `Shape.Circle(5)` — the parser sees `Shape.Circle(args)` as a
MethodCall where the object is `Ident("Shape")`.

### Prompt

```
You're working on the Rask programming language compiler. Your task is to make the
MIR lowerer handle method calls on types (not values) — this covers stdlib type
constructors like `Vec.new()` and enum variant constructors like `Shape.Circle(5)`.

**The AST representation:**

Both `Vec.new()` and `Shape.Circle(5)` parse as:
```
ExprKind::MethodCall {
    object: Expr { kind: Ident("Vec") / Ident("Shape") },
    method: "new" / "Circle",
    args: [...],
}
```

The current MethodCall handler in `compiler/crates/rask-mir/src/lower/expr.rs` line
173 unconditionally calls `self.lower_expr(object)`, which fails when the object is
a type name.

**What to implement:**

In the MethodCall handler (expr.rs ~line 173), before lowering the object expression,
check if the object is a type name:

1. **Extract the type name.** If `object.kind` is `ExprKind::Ident(name)`, check
   if `name` is a known type (not a local variable).

2. **Check if it's a known type.** A name is a type if:
   - `self.ctx.find_struct(name)` returns Some — it's a struct type
   - `self.ctx.find_enum(name)` returns Some — it's an enum type
   - It's a known stdlib type: "Vec", "Map", "Pool", "string"
   - AND it's NOT in `self.locals` (locals shadow type names)

3. **Handle stdlib type constructors.** If the type is a stdlib type:
   - Construct the MIR call name by combining type name and method:
     `"Vec" + "new"` → `"Vec_new"`, `"Vec" + "push"` → nope, push is on instances
   - Actually, only constructors (static methods) come through this path. Instance
     methods like `v.push(1)` have a value as the object, not a type name.
   - Known static methods: `Vec.new()`, `Map.new()`, `Pool.new()`, `string.new()`,
     `string.concat(a, b)`, `string.from(s)`
   - Emit: `MirStmt::Call { func: FunctionRef { name: "{Type}_{method}" }, args }`
   - Return type: `MirType::Ptr` for all stdlib constructors (opaque heap pointer)

4. **Handle enum variant constructors.** If `self.ctx.find_enum(name)` returns Some:
   - Look up the variant by `method` name in the enum layout
   - If the variant has payload fields, this is a tuple variant: `Shape.Circle(5)`
     - Allocate a local of enum type
     - Store the variant tag
     - Store payload fields from args at the correct offsets
   - If no payload, this is a unit variant: `Color.Red`
     - Allocate a local of enum type
     - Store just the tag
   - Use the existing enum layout infrastructure — `EnumLayout` has `variants` with
     `tag` and `fields` for each variant
   - Look at how `ExprKind::StructLit` (line 336) handles struct construction for
     reference on Store statements with offsets

5. **Fall through for value method calls.** If the ident IS in locals (it's a value,
   not a type), proceed with the existing logic (lower_expr on object, prepend as
   first arg, emit Call).

**Where enum layout info is:**

Read `compiler/crates/rask-mono/src/layout.rs` to understand EnumLayout:
- `EnumLayout { name, tag_size, variants: Vec<VariantLayout> }`
- `VariantLayout { name, tag, payload_size, fields: Vec<FieldLayout> }`
- `FieldLayout { name, offset, size, ty }`

The tag goes at offset 0. Payload fields start after the tag (usually offset 8 for
alignment). `find_enum(name)` returns `(enum_layout_id, &EnumLayout)`.

**Example MIR output for `Shape.Circle(5)`:**

```
_0 = alloc_enum(Shape, size=16)  // or just alloc_temp
store _0 + 0 = 0                 // tag for Circle = 0
store _0 + 8 = 5                 // payload
```

You can model this with existing MIR statements:
```rust
MirStmt::Store { addr: result_local, offset: 0, value: MirOperand::Constant(MirConst::Int(tag)) }
MirStmt::Store { addr: result_local, offset: 8, value: arg_operand }
```

**Testing:**

Create test files in `/tmp/`:

1. `/tmp/test_vec_native.rk`:
```rask
func main() {
    const v = Vec.new()
    v.push(10_i64)
    v.push(20_i64)
    v.push(30_i64)
    print(v.len())
    print("\n")
}
```
Expected: `rask run --native` prints `3`

2. `/tmp/test_enum_native.rk`:
```rask
enum Color {
    Red,
    Green,
    Blue,
}

func main() {
    const c = Color.Green
    print("ok\n")
}
```
Expected: compiles and prints `ok`

3. `/tmp/test_enum_payload.rk`:
```rask
enum Shape {
    Circle(i64),
    Square(i64),
}

func main() {
    const c = Shape.Circle(5_i64)
    print("ok\n")
}
```
Expected: compiles and prints `ok`

4. Run existing tests: `cargo test -p rask-mir` and `cargo test -p rask-cli --test compile_run`

**Scope limits:**
- Don't change the parser or type checker
- Don't handle generic type constructors (`Vec<i32>.new()`) — monomorphizer handles those
- Don't handle enum match in this stream — match lowering already exists in expr.rs
- Keep the method call handler's existing logic as the fallback path

Read CLAUDE.md for project conventions before starting.
```

---

## Stream 2: Integer Literal Type Inference

**Files:** `compiler/crates/rask-types/src/checker/check_expr.rs`, `compiler/crates/rask-types/src/checker/check_stmt.rs`
**Goal:** `let x = 0` in a `-> i64` function infers `x` as `i64`, not `i32`.

### Problem

In `check_expr.rs` line 65, unsuffixed integer literals default to `i32`. The type
checker has `infer_expr_expecting()` (line 19) that propagates expected types to
literals, but this only works when there's an immediate expected type context.

Cases where inference fails:
- `let count = 0` in a `-> i64` function — no expected type at binding site
- `let count: i64 = 0` works (annotation provides expected type)
- `return count` then fails because count is i32 but function returns i64

The interpreter doesn't care (it uses dynamic types), but native compilation is strict.

### Prompt

```
You're working on the Rask programming language compiler's type checker. Your task is
to improve integer literal type inference so unsuffixed literals adopt the type required
by their context.

**Current behavior:**

In `compiler/crates/rask-types/src/checker/check_expr.rs`:
- Line 19: `infer_expr_expecting(&mut self, expr, expected)` — propagates expected type
  to unsuffixed literals. If the expected type is an integer type and the literal has
  no suffix, it adopts the expected type. This works.
- Line 65: `None => Type::I32` — fallback when no expected type. This is the problem.

In `compiler/crates/rask-types/src/checker/check_stmt.rs`, look at how `let`/`const`
bindings are checked. When there's a type annotation, the expected type is propagated.
When there's no annotation, the initializer is inferred without an expected type, so
unsuffixed literals fall to i32.

**What to fix:**

The fundamental issue: `let count = 0` has no type annotation, so the checker calls
`infer_expr(init)` not `infer_expr_expecting(init, expected)`. The initializer infers
as i32. Later, `return count` in a `-> i64` function creates a mismatch.

There are several approaches. Evaluate them:

**Option A: Use return type as context for all bindings.**
When checking a binding without annotation, if the current function has a return type,
use that as the expected type. Problem: this is too aggressive. `let len = v.len()`
should be i64 (whatever len returns), not the function's return type.

**Option B: Unification-based approach.**
Use a fresh type variable for unsuffixed integer literals (like Rust does internally).
At the end of type checking, default unconstrained integer type variables to i32.
This is the most correct solution but requires significant type checker changes.

**Option C: Bidirectional flow for assignments and returns.**
When checking `return expr`, pass the function's return type as the expected type to
`infer_expr_expecting`. When checking `let x = init` followed by `return x`, the
return check will see x:i32 vs expected i64, which still fails.

**Option D: Widen i32 → i64 at return sites (and function call sites).**
When a return statement has type i32 but the function returns i64, automatically
insert a widening coercion. Same for function arguments: if param is i64 and arg
is i32, coerce. This is pragmatic and handles 90% of cases.

**Recommended: Option D, with narrower coercion rules.**

Implement integer widening coercion:
- i8 → i16 → i32 → i64 (widening is always safe)
- u8 → u16 → u32 → u64 (widening is always safe)
- Do NOT coerce i32 → u64 or u32 → i64 (signedness mismatch)
- Do NOT coerce i64 → i32 (narrowing is lossy)

Apply coercion at these sites:
1. **Return statements** — if return expr type is a narrower integer than the
   function return type, coerce
2. **Function call arguments** — if arg type is a narrower integer than the parameter
   type, coerce
3. **Variable assignment** — if `let x: i64 = expr` where expr is i32, coerce

**Where to implement:**

Find the return checking in `check_stmt.rs` or `check_fn.rs`. Look for where the
checker unifies the return expression type with the function return type. Before
reporting a mismatch, check if widening coercion applies.

For function calls, look in `check_expr.rs` for the Call/MethodCall handling. Before
unifying each argument with its parameter type, check if widening applies.

You'll also need to make sure the MIR lowerer emits the actual widening instruction.
In `compiler/crates/rask-mir/src/lower/`, when the type checker has accepted a
widening coercion, the MIR should emit a `Cast` or `Extend` operation. Check if
MIR already has a way to represent integer extension — look at `MirRValue` variants.

Actually, the simpler approach: make the type checker accept the coercion by
treating i32-where-i64-expected as valid (don't report the error), and let MIR/codegen
handle the actual extension. Codegen already handles some type conversions — look at
`compiler/crates/rask-codegen/src/builder.rs` for `sextend`/`uextend` usage.

**Testing:**

1. `/tmp/test_int_infer.rk`:
```rask
func double(x: i64) -> i64 {
    return x * 2
}

func main() {
    let count = 0
    count = count + 1
    print(double(count))
    print("\n")
}
```
Should compile and run with `rask run --native`, printing `2`.

2. `/tmp/test_int_return.rk`:
```rask
func foo() -> i64 {
    const x = 42
    return x
}

func main() {
    print(foo())
    print("\n")
}
```
Should print `42`.

3. Verify existing tests still pass:
   `cargo test -p rask-types`
   `cargo test -p rask-cli --test compile_run`

4. Verify the interpreter still works for all example programs:
   `rask run examples/01_variables.rk`
   `rask run examples/02_functions.rk`

**Scope limits:**
- Only implement widening coercions for integer types (i8→i16→i32→i64, u8→u16→u32→u64)
- Don't change the default integer type from i32 — that's a bigger design decision
- Don't implement float coercion (f32→f64) — that can come later
- Don't change MIR or codegen if you can avoid it — let the type checker accept the
  coercion and rely on existing codegen type conversion

Read CLAUDE.md for project conventions before starting.
```

---

## Stream 3: Cross-Type Binary Ops in Codegen

**Files:** `compiler/crates/rask-codegen/src/builder.rs`
**Goal:** Binary ops with mismatched integer widths (i64 + i32) produce correct Cranelift IR.

### Problem

A closure captures `offset: i64` and has parameter `x: i32`. The MIR emits
`_3 = _1 + _2` where `_1: i32` and `_2: i64`. Cranelift's verifier rejects binary ops
with mismatched operand types.

This is a codegen-level fix. Even if the type checker gets smarter (Stream 2), there
will be cases where MIR has mismatched types in binary ops — closure captures in
particular infer types independently of parameters.

### Prompt

```
You're working on the Rask programming language compiler's Cranelift codegen. Your
task is to fix binary operations that have mismatched integer operand types.

**The problem:**

When codegen processes a `MirStmt::Assign { rvalue: BinaryOp { op, left, right } }`,
it translates the MIR operands to Cranelift values and emits the operation. But
Cranelift requires both operands of an `iadd`, `isub`, `imul`, etc. to have the same
type. If one operand is I32 and the other is I64, the verifier rejects it.

This happens with closure captures: the capture's type is inferred from the defining
scope (i64) while the closure parameter type comes from its annotation (i32).

**MIR example that triggers this:**
```
func main__closure_0(__env: ptr, x: i32) -> i32 {
  let offset: i64  // _2
  let _3: i32

  bb0:
    _2 = load_capture(_0+0)
    _3 = _1 + _2            // i32 + i64 — Cranelift rejects this
    return _3
}
```

**What to implement:**

In `compiler/crates/rask-codegen/src/builder.rs`, find where BinaryOp is lowered to
Cranelift IR. Before emitting the Cranelift instruction:

1. Get the Cranelift types of both operand values using `builder.func.dfg.value_type(val)`.

2. If they're different integer types (e.g., I32 vs I64):
   - Determine the wider type
   - Sign-extend (`sextend`) or zero-extend (`uextend`) the narrower operand to match
   - Use `sextend` by default (signed is the common case in Rask)

3. If the result type of the binary op should match a specific width (e.g., the
   destination is i32), truncate with `ireduce` after the operation.

4. Handle comparison ops too — `icmp` also requires matching types.

**Implementation sketch:**

```rust
fn ensure_same_type(
    &mut self,
    builder: &mut FunctionBuilder,
    left: Value,
    right: Value,
) -> (Value, Value) {
    let left_ty = builder.func.dfg.value_type(left);
    let right_ty = builder.func.dfg.value_type(right);
    if left_ty == right_ty {
        return (left, right);
    }
    // Widen the narrower one
    if left_ty.bits() < right_ty.bits() {
        let widened = builder.ins().sextend(right_ty, left);
        (widened, right)
    } else {
        let widened = builder.ins().sextend(left_ty, right);
        (left, widened)
    }
}
```

Call this before every integer binary op and comparison.

**Testing:**

1. `/tmp/test_closure_mixed.rk`:
```rask
func main() {
    const offset = 10
    const add = |x: i32| -> i32 { return x + offset }
    print(add(32))
    print("\n")
}
```
Should compile and print `42` (currently fails with Cranelift verifier error).

2. `/tmp/test_mixed_arithmetic.rk`:
```rask
func main() {
    const a: i64 = 100
    const b: i32 = 42
    print(a)
    print("\n")
    print(b)
    print("\n")
}
```
Verify this still works (it should — no mixed ops).

3. Run all codegen tests: `cargo test -p rask-codegen`
4. Run integration tests: `cargo test -p rask-cli --test compile_run`

**Scope limits:**
- Only handle integer type mismatches (not float/bool/ptr)
- Use sextend as the default widening strategy
- Don't change MIR types — this is a codegen-level fixup
- Don't try to infer "correct" result types — just make both operands match

Read CLAUDE.md for project conventions before starting.
```

---

## After These Three Merge

The next frontier becomes:
1. **Enum match in native code** — match with payload extraction (partially working, needs testing)
2. **Concurrency runtime for compiled programs** — spawn/join/channels require rask-rt or C equivalents
3. **Validation program native compile** — compile simple_grep.rk natively as the integration milestone
