# How the Rask Compiler Works

A walkthrough of the compiler code. Assumes no Rust or compiler background.

## What a compiler actually does

A compiler reads source code (text) and transforms it into something a computer
can execute. It does this in stages, each one understanding the code at a deeper
level:

```
Source text  →  Tokens  →  Tree structure  →  Meaning  →  Execution
  "1 + 2"    [1] [+] [2]   Add(1, 2)        "integer"    3
```

The Rask compiler has **10 stages**. Each stage lives in its own crate (Rust's
term for a library/package) under `compiler/crates/`. They're independent—each
one takes input from the previous and produces output for the next.

## The crate map

```
compiler/crates/
├── rask-ast/            Data structures everything shares (tokens, AST nodes)
├── rask-lexer/          Stage 1: Text → tokens
├── rask-parser/         Stage 2: Tokens → tree (AST)
├── rask-desugar/        Stage 3: Simplify tree (operators → method calls)
├── rask-resolve/        Stage 4: Connect names to definitions
├── rask-types/          Stage 5: Figure out every expression's type
├── rask-ownership/      Stage 6: Verify memory safety
├── rask-hidden-params/  Stage 7: Desugar `using` into regular params
├── rask-mono/           Stage 8: Eliminate generics
├── rask-mir/            Stage 9: Flatten into basic blocks
├── rask-codegen/        Stage 10: Generate machine code
├── rask-interp/         Alternative to 8-10: interpret the AST directly
├── rask-cli/            The `rask` command: dispatches to stages
├── rask-diagnostics/    Error formatting (shared by all stages)
├── rask-stdlib/         Built-in type methods (print, Vec, etc.)
├── rask-comptime/       Compile-time evaluation
├── rask-fmt/            Code formatter
├── rask-lint/           Linter
├── rask-describe/       Public API display
├── rask-lsp/            Editor integration (Language Server Protocol)
├── rask-rt/             Runtime library
├── rask-spec-test/      Spec validation
└── rask-wasm/           WebAssembly (experimental)
```

## How everything connects

When you run `rask run hello.rk`, the CLI reads the file and runs each stage
in sequence. You can see this directly in `rask-cli/src/commands/run.rs`:

```rust
// 1. Lex
let mut lexer = rask_lexer::Lexer::new(&source);
let lex_result = lexer.tokenize();

// 2. Parse
let mut parser = rask_parser::Parser::new(lex_result.tokens);
let mut parse_result = parser.parse();

// 3. Desugar
rask_desugar::desugar(&mut parse_result.decls);

// 4. Resolve
let resolved = rask_resolve::resolve(&parse_result.decls)?;

// 5. Typecheck
let typed = rask_types::typecheck(resolved, &parse_result.decls)?;

// 6. Ownership
let ownership_result = rask_ownership::check_ownership(&typed, &parse_result.decls);

// 7. Run
let mut interp = rask_interp::Interpreter::with_args(program_args);
interp.run(&parse_result.decls)?;
```

Each stage can fail with errors. If any stage fails, the compiler stops and
shows diagnostics. It never passes broken data to the next stage.

There are **two execution paths** after stage 6:

- **Interpretation** (`rask run`): Stages 1-6, then the interpreter walks the
  AST and executes it directly. This is what you use day-to-day.
- **Compilation** (`rask build`): Stages 1-6, then hidden param desugaring,
  monomorphization, MIR lowering, and code generation. Produces a native binary.

```
                    Source Code
                        │
                   ┌────▼────┐
                   │  Lexer  │
                   └────┬────┘
                   ┌────▼────┐
                   │ Parser  │
                   └────┬────┘
                   ┌────▼─────┐
                   │ Desugar  │
                   └────┬─────┘
                   ┌────▼─────┐
                   │ Resolver │
                   └────┬─────┘
                   ┌────▼──────┐
                   │ Typecheck │
                   └────┬──────┘
                   ┌────▼───────┐
                   │ Ownership  │
                   └────┬───────┘
                        │
           ┌────────────┴────────────┐
      ┌────▼──────┐          ┌───────▼─────────┐
      │ Interpret │          │ Hidden Params    │
      │ (rask run)│          └───────┬──────────┘
      └───────────┘          ┌───────▼──────────┐
                             │ Monomorphize     │
                             └───────┬──────────┘
                             ┌───────▼──────────┐
                             │ MIR Lowering     │
                             └───────┬──────────┘
                             ┌───────▼──────────┐
                             │ Code Generation  │
                             │ (rask build)     │
                             └─────────────────-┘
```

Each stage also has a corresponding CLI command so you can inspect what it
produces (`rask lex`, `rask parse`, `rask resolve`, `rask check`,
`rask ownership`, `rask mono`, `rask mir`).

---

## Stage 1: Lexing (rask-lexer)

**What it does:** Turns raw text into a flat list of tokens.

**Input:** A string like `"func main() { println("hello") }"`

**Output:** A list of tokens:
`[Func, Ident("main"), LParen, RParen, LBrace, Ident("println"), LParen, String("hello"), RParen, RBrace, Eof]`

### How it works

The lexer uses a library called `logos` which generates a fast tokenizer from
pattern annotations. Look at `rask-lexer/src/lexer.rs`:

```rust
#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(skip r"[ \t]+")]  // Skip spaces and tabs (NOT newlines—they matter)
enum RawToken {
    #[token("func")]
    Func,
    #[token("let")]
    Let,
    #[token("==")]
    EqEq,
    #[regex(r"[0-9][0-9_]*")]
    DecInt,
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_]*")]
    Ident,
    // ... etc
}
```

The `#[token("func")]` annotation says "when you see the exact text `func`,
produce a `Func` token." The `#[regex(...)]` annotation uses a pattern—`DecInt`
matches `42`, `1_000`, etc.

Logos does a first pass producing `RawToken`s. Then `convert_token()` does a
second pass to parse literal values:

```rust
RawToken::DecInt => {
    let (stripped, suffix) = parse_int_suffix(slice);
    let cleaned: String = stripped.chars().filter(|c| *c != '_').collect();
    let value = cleaned.parse::<i64>()?;
    TokenKind::Int(value, suffix)
}
```

This two-pass design means logos handles the pattern matching (fast), and we
handle the value parsing (flexible).

### Key details

- **Newlines are tokens.** Rask uses newlines as statement terminators (like Go),
  so they can't be skipped. The parser decides when newlines matter.
- **Comments are skipped.** Line comments (`//`) use `logos::skip`. Block
  comments (`/* */`) use a custom callback that handles nesting.
- **Spans track position.** Every token records its byte offset range in the
  source. This is how error messages can point to the exact character.
- **Error recovery.** The lexer collects up to 20 errors before stopping,
  so you get multiple error messages at once.

### The data structures

Defined in `rask-ast/src/token.rs`:

```rust
struct Token {
    kind: TokenKind,   // What kind of token (keyword, number, operator, etc.)
    span: Span,        // Where in the source file (byte start..end)
}

enum TokenKind {
    Int(i64, Option<IntSuffix>),  // 42, 0xFF, 100u32
    Float(f64, Option<FloatSuffix>),
    String(String),
    Ident(String),                // variable/function names
    Func,                         // keywords
    Let,
    Const,
    Plus,                         // operators
    EqEq,
    LBrace,                       // delimiters
    Newline,
    Eof,
    // ... ~80 variants total
}
```

---

## Stage 2: Parsing (rask-parser)

**What it does:** Turns a flat token list into a tree that represents the
program's structure.

**Input:** `[Func, Ident("add"), LParen, Ident("a"), Colon, Ident("i32"), ...]`

**Output:** An AST (Abstract Syntax Tree):
```
FnDecl {
    name: "add",
    params: [Param { name: "a", ty: "i32" }, ...],
    body: [Return(Binary { op: Add, left: Ident("a"), right: Ident("b") })]
}
```

### How it works

The parser is a **recursive descent parser** with **Pratt parsing** for
expressions. These are standard techniques:

- **Recursive descent** means each grammar rule is a function. `parse_fn()`
  parses functions, `parse_struct()` parses structs, etc. They call each other
  recursively—`parse_fn()` calls `parse_stmt()` for the body, which calls
  `parse_expr()` for expressions.

- **Pratt parsing** handles operator precedence (should `1 + 2 * 3` be
  `(1 + 2) * 3` or `1 + (2 * 3)`?). Each operator has a "binding power"—higher
  means tighter binding. `*` binds tighter than `+`, so `2 * 3` groups first.

The parser struct in `rask-parser/src/parser.rs`:

```rust
pub struct Parser {
    tokens: Vec<Token>,   // The token list from the lexer
    pos: usize,           // Current position in that list
    errors: Vec<ParseError>,
    next_node_id: u32,    // Counter for unique AST node IDs
    // ...
}
```

The core navigation methods:

```rust
fn current(&self) -> &Token      // Look at current token (don't consume)
fn advance(&mut self) -> &Token  // Consume current token, move forward
fn check(&self, kind) -> bool    // Is current token this kind?
fn expect(&mut self, kind) -> Result<&Token, ParseError>  // Consume, or error
fn skip_newlines(&mut self)      // Skip past insignificant newlines
```

A simplified parse function looks like:

```rust
fn parse_fn(&mut self) -> Result<FnDecl, ParseError> {
    self.expect(&TokenKind::Func)?;                      // consume "func"
    let name = self.expect_ident()?;                      // consume name
    self.expect(&TokenKind::LParen)?;                     // consume "("
    let params = self.parse_param_list()?;                // parse parameters
    self.expect(&TokenKind::RParen)?;                     // consume ")"
    let ret_ty = if self.match_token(&TokenKind::Arrow) { // optional "-> Type"
        Some(self.parse_type_string()?)
    } else { None };
    let body = self.parse_block_stmts()?;                 // parse { ... }
    Ok(FnDecl { name, params, ret_ty, body, ... })
}
```

### Error recovery

When the parser hits an unexpected token, it doesn't just die. It:

1. Records the error (up to 20)
2. Calls `synchronize()` which skips tokens until it finds the start of the
   next declaration (`func`, `struct`, etc.)
3. Continues parsing

This means one typo doesn't prevent you from seeing other errors.

### The AST data structures

Defined in `rask-ast/src/`. Every node has three things:

```rust
struct Expr {
    id: NodeId,      // Unique identifier (used by later stages to attach info)
    kind: ExprKind,  // What kind of expression
    span: Span,      // Source location
}
```

**NodeId** is critical. Later stages (type checker, ownership checker) need to
attach information to specific AST nodes. Instead of modifying the tree, they
build lookup tables: `HashMap<NodeId, Type>`, `HashMap<NodeId, SymbolId>`, etc.

The expression kinds (`rask-ast/src/expr.rs`) cover everything:

```rust
enum ExprKind {
    Int(i64, Option<IntSuffix>),   // Literals
    String(String),
    Bool(bool),
    Ident(String),                 // Variable references
    Binary { op, left, right },    // a + b, a == b
    Call { func, args },           // f(x)
    MethodCall { object, method, args },  // x.foo(y)
    Field { object, field },       // point.x
    If { cond, then_branch, else_branch },
    Match { scrutinee, arms },
    Block(Vec<Stmt>),
    Closure { params, body },
    StructLit { name, fields },    // Point { x: 1, y: 2 }
    // ... ~30 variants total
}
```

Statements (`rask-ast/src/stmt.rs`):

```rust
enum StmtKind {
    Expr(Expr),
    Let { name, ty, init },      // let x = 5 (mutable)
    Const { name, ty, init },    // const x = 5 (immutable)
    Assign { target, value },    // x = 10
    Return(Option<Expr>),
    For { var, iter, body },
    While { cond, body },
    // ...
}
```

Declarations (`rask-ast/src/decl.rs`):

```rust
enum DeclKind {
    Fn(FnDecl),
    Struct(StructDecl),
    Enum(EnumDecl),
    Trait(TraitDecl),
    Impl(ImplDecl),        // extend blocks
    Import(ImportDecl),
    Test(TestDecl),
    // ...
}
```

### Rust-to-Rask hint system

The parser recognizes common Rust syntax mistakes and gives helpful messages.
This lives in `rask-parser/src/hints.rs`. If you write `fn` instead of `func`:

```
error: unknown keyword 'fn'
  hint: use 'func' instead of 'fn'
```

---

## Stage 3: Desugaring (rask-desugar)

**What it does:** Transforms operators into method calls.

**Input:** `Binary { op: Add, left: a, right: b }`

**Output:** `MethodCall { object: a, method: "add", args: [b] }`

### Why?

This is a design choice that simplifies the type checker and interpreter.
Instead of having special cases for every operator, everything goes through
method dispatch. The type checker only needs to know how to check method calls,
and the interpreter only needs to know how to dispatch methods.

### How it works

`rask-desugar/src/lib.rs` walks the entire AST and replaces operator nodes:

```rust
fn binary_op_method(op: BinOp) -> Option<&'static str> {
    match op {
        BinOp::Add => Some("add"),
        BinOp::Sub => Some("sub"),
        BinOp::Eq  => Some("eq"),
        BinOp::Lt  => Some("lt"),
        // ...
        BinOp::And | BinOp::Or => None,  // Short-circuit: keep as binary
    }
}
```

`!=` gets special treatment: `a != b` becomes `!a.eq(b)` (negate the result
of `eq`).

Logical operators (`&&`, `||`) are **not** desugared because they short-circuit:
`a && b` doesn't evaluate `b` if `a` is false. Method calls always evaluate
all arguments.

The desugarer also handles unary operators: `-x` becomes `x.neg()`,
`~x` becomes `x.bit_not()`. But `!x` (logical not) and `&x` (reference)
stay as unary operators.

---

## Stage 4: Name Resolution (rask-resolve)

**What it does:** Figures out what every name refers to.

When you write `println("hello")`, the resolver connects that `println` to the
built-in print function. When you write `x + 1` inside a function, it connects
`x` to the parameter or local variable named `x`.

**Input:** The AST (list of declarations).

**Output:** A `ResolvedProgram` containing:
- A **symbol table**: every named thing (function, variable, type) gets a
  `SymbolId` and an entry recording its kind, name, and location.
- A **resolution map**: `HashMap<NodeId, SymbolId>` connecting each use of a
  name to its definition.

### How it works

The resolver (`rask-resolve/src/resolver.rs`) does two things:

1. **Registration pass**: Walk all declarations, create symbols for them.
   Functions, structs, enums, traits, imports—everything gets a `SymbolId`.

2. **Resolution pass**: Walk all expressions, look up every identifier in the
   current scope.

It uses a **scope tree** to handle nested scopes:

```rust
pub struct Resolver {
    symbols: SymbolTable,
    scopes: ScopeTree,
    resolutions: HashMap<NodeId, SymbolId>,
    // ...
}
```

When entering a function body, it pushes a new scope. When exiting, it pops it.
Looking up a name searches from the innermost scope outward.

Built-in functions are registered first:

```rust
fn register_builtins(&mut self) {
    // Built-in functions: println, print, panic, format
    // Built-in types: Vec, Map, Set, string, Channel, Pool, ...
}
```

---

## Stage 5: Type Checking (rask-types)

**What it does:** Determines the type of every expression and verifies type
correctness.

**Input:** `ResolvedProgram` + AST declarations.

**Output:** `TypedProgram` containing:
- `TypeTable`: definitions of all structs, enums, and traits
- `node_types: HashMap<NodeId, Type>`: the type of every expression
- `call_type_args`: which concrete types were used at generic call sites

### The Type enum

```rust
enum Type {
    I8, I16, I32, I64, I128,
    U8, U16, U32, U64, U128,
    F32, F64,
    Bool,
    Char,
    String,
    Unit,                          // void / no value
    Named(String),                 // user-defined types
    Generic(String, Vec<Type>),    // Vec<i32>, Map<string, i32>
    Function(Vec<Type>, Box<Type>), // (param types) -> return type
    Option(Box<Type>),             // T?
    Result(Box<Type>, Box<Type>),  // T or E
    Var(TypeVarId),                // inference variable (unknown yet)
    Never,                         // ! type (function never returns)
    // ...
}
```

### How type checking works

The type checker (`rask-types/src/checker/`) visits every node in the AST
and assigns it a type.

**TypeChecker struct:**

```rust
pub struct TypeChecker {
    resolved: ResolvedProgram,     // From stage 4
    types: TypeTable,              // Struct/enum/trait definitions
    ctx: InferenceContext,         // Type variable tracking
    node_types: HashMap<NodeId, Type>,  // Result: type of each node
    errors: Vec<TypeError>,
    local_types: Vec<HashMap<String, (Type, bool)>>,  // Scope stack
    // ...
}
```

For a simple expression like `1 + 2`:
- After desugaring, this is `1.add(2)`
- Type checker sees `1` → type is `i32`
- Looks up method `add` on `i32` → expects `(i32) -> i32`
- Checks argument `2` is `i32` → yes
- Result type: `i32`

### Type inference

When you write `const x = 42`, the type checker needs to figure out that `x`
is `i32` without you saying so. It uses **type variables** and **unification**:

1. Create a fresh type variable `?T0` for `x`
2. Check the right side: `42` has type `i32`
3. Add constraint: `?T0 = i32`
4. **Unify**: solve constraints, replace `?T0` with `i32` everywhere

The unification algorithm is in `rask-types/src/checker/unify.rs`. It's the
standard algorithm: walk two types in parallel, and whenever you find a type
variable, bind it to whatever's on the other side.

### What it checks

Submodules handle different parts:
- `check_expr.rs` — expressions (calls, field access, operators, etc.)
- `check_stmt.rs` — statements (let/const bindings, assignments, loops)
- `check_fn.rs` — function signatures and bodies
- `check_pattern.rs` — match/if-is patterns
- `generics.rs` — generic type parameter validation
- `borrow.rs` — aliasing detection (part of type-level safety)

---

## Stage 6: Ownership Checking (rask-ownership)

**What it does:** Verifies memory safety without garbage collection.

This is the "borrow checker"—the part that prevents use-after-free, double-free,
and data races.

**Input:** `TypedProgram` + AST declarations.

**Output:** `OwnershipResult` — either empty (safe) or a list of errors.

### The core model

Every binding (variable) is in one of these states:

```rust
enum BindingState {
    Owned,      // Has the value, can use it
    Borrowed,   // Someone else is reading it
    Moved,      // Value was given away, can't use anymore
}
```

### What it catches

**Use after move:**
```rask
const data = Vec.new()
process(own data)         // ownership transferred
println(data.len())       // ERROR: data was moved
```

**Aliasing violations:**
```rask
let items = Vec.new()
const ref1 = items        // borrow
items.push(42)            // ERROR: can't mutate while borrowed
```

### How it works

The `OwnershipChecker` in `rask-ownership/src/lib.rs`:

```rust
pub struct OwnershipChecker<'a> {
    program: &'a TypedProgram,
    bindings: HashMap<String, BindingState>,   // Track each variable's state
    borrows: Vec<ActiveBorrow>,                // Currently active borrows
    resource_bindings: HashSet<String>,        // @resource types
    ensure_registered: HashSet<String>,        // Resources with cleanup
    errors: Vec<OwnershipError>,
}
```

It walks every function body:
1. When a `const`/`let` binding is created → mark as `Owned`
2. When a value is passed with `own` → mark as `Moved`
3. When a value is used after being moved → emit error
4. When a `@resource` binding reaches end of scope without being consumed
   → emit error

### Borrow scopes

Rask has two kinds of borrows:

- **Persistent (block-scoped):** `const ref = something` — the borrow lasts
  until the end of the block.
- **Instant (statement-scoped):** `items[0]` — the borrow only lasts for that
  statement. This is why you can index a collection and then modify it on the
  next line.

This distinction is a key design choice. It means you don't need lifetime
annotations for the common case of "use a value, then modify it."

---

## Stage 7: Hidden Parameter Desugaring (rask-hidden-params)

**What it does:** Transforms `using` context clauses into explicit function
parameters.

This is a Rask-specific feature. When you write:

```rask
func damage(h: Handle<Player>) using Pool<Player> {
    h.health -= 10
}
```

This stage rewrites it to:

```rask
func damage(h: Handle<Player>, __ctx_pool: &Pool<Player>) {
    h.health -= 10
}
```

Call sites are also rewritten to pass the context automatically.

It runs **after** type checking (types are already verified) but **before**
monomorphization (so concrete types can be substituted).

---

## Stage 8: Monomorphization (rask-mono)

**What it does:** Eliminates generics by creating concrete copies of functions
for each type they're used with.

If you have `func identity<T>(x: T) -> T` and call it with `i32` and `string`,
the monomorphizer creates two functions: `identity_i32` and `identity_string`.

**Input:** `TypedProgram` + AST.

**Output:** `MonoProgram` — a list of concrete functions plus memory layouts.

```rust
pub struct MonoProgram {
    pub functions: Vec<MonoFunction>,       // Concrete function instances
    pub struct_layouts: Vec<StructLayout>,  // Memory layout of each struct
    pub enum_layouts: Vec<EnumLayout>,      // Memory layout of each enum
}
```

### How it works

1. Start from `main()` (the entry point)
2. Walk its body, find all function calls
3. For each call to a generic function, record which concrete types are used
4. Create a specialized copy of that function with types substituted
5. Walk the new copy for more calls (BFS)
6. Repeat until no new instances are discovered

This is **tree shaking** — only reachable functions end up in the output.
Unused code is dropped automatically.

The layout computation figures out struct sizes:

```
struct Point { x: f64, y: f64 }
→ StructLayout { size: 16, align: 8, fields: [
    { name: "x", offset: 0, size: 8 },
    { name: "y", offset: 8, size: 8 },
  ]}
```

---

## Stage 9: MIR Lowering (rask-mir)

**What it does:** Flattens the tree-shaped AST into a **control-flow graph**
made of basic blocks.

The AST represents code as nested trees. MIR represents it as a flat list of
blocks connected by jumps. This is closer to how the CPU actually works.

**Input:** Monomorphized AST + layouts.

**Output:** `MirFunction` for each function.

```rust
pub struct MirFunction {
    pub name: String,
    pub params: Vec<MirLocal>,
    pub ret_ty: MirType,
    pub locals: Vec<MirLocal>,     // All variables + temporaries
    pub blocks: Vec<MirBlock>,     // The control-flow graph
    pub entry_block: BlockId,
}

pub struct MirBlock {
    pub id: BlockId,
    pub statements: Vec<MirStmt>,       // Assignments, calls
    pub terminator: MirTerminator,      // Jump, branch, return
}
```

### Example transformation

AST (tree):
```
if x > 0 {
    println("positive")
} else {
    println("non-positive")
}
```

MIR (flat blocks):
```
bb0:
    _1 = x.gt(0)
    branch _1 -> bb1, bb2

bb1:
    call println("positive")
    jump -> bb3

bb2:
    call println("non-positive")
    jump -> bb3

bb3:
    (continue)
```

The lowering logic is in `rask-mir/src/lower/`:
- `mod.rs` — function-level lowering
- `expr.rs` — expression lowering (each expression → an operand + temp locals)
- `stmt.rs` — statement lowering (assignments, control flow → blocks + terminators)

---

## Stage 10: Code Generation (rask-codegen)

**What it does:** Translates MIR into native machine code using the Cranelift
compiler backend.

Cranelift is a code generator library (like a mini LLVM). It takes an
intermediate representation and produces machine code for the target platform.

The codegen process:
1. Declare runtime functions (print, exit, etc.)
2. Create Cranelift function declarations for each MIR function
3. Register string constants as global data
4. For each function: translate MIR blocks → Cranelift IR → machine code
5. Emit an object file (.o)
6. Link with the C runtime to produce an executable

---

## The Interpreter Path (rask-interp)

For `rask run`, the compiler skips stages 7-10 and uses a **tree-walk
interpreter** instead. This directly executes the AST.

```rust
pub struct Interpreter {
    env: Environment,                                    // Variable scopes
    functions: HashMap<String, FnDecl>,                  // Function registry
    enums: HashMap<String, EnumDecl>,
    struct_decls: HashMap<String, StructDecl>,
    methods: HashMap<String, HashMap<String, FnDecl>>,   // type → method → decl
    // ...
}
```

### How it executes

The interpreter has three main operations:

1. **Register** (`register.rs`): Walk all declarations, store functions/structs/
   enums/methods in lookup tables.
2. **Evaluate expressions** (`eval_expr.rs`): Recursively evaluate an `Expr`
   node to produce a `Value`.
3. **Execute statements** (`exec_stmt.rs`): Run `Stmt` nodes for their side
   effects (assignments, loops, returns).

When it hits a function call, it:
1. Looks up the function by name
2. Creates a new scope
3. Binds arguments to parameter names
4. Executes the function body
5. Returns the result

For method calls (remember, operators are method calls after desugaring):
1. Look up built-in methods first (add, sub, eq, etc. on primitives)
2. Then check user-defined methods from `extend` blocks
3. The dispatch logic is in `dispatch.rs`

The interpreter supports the full language: closures, threads, channels, async
tasks, file I/O, networking—everything needed to run real programs.

---

## Error Handling (rask-diagnostics)

All stages produce errors in their own format, but they all convert to a
unified `Diagnostic` type:

```rust
pub struct Diagnostic {
    pub severity: Severity,        // Error, Warning, Info
    pub code: Option<String>,      // E0001, W0042, etc.
    pub message: String,           // "type mismatch"
    pub labels: Vec<Label>,        // Spans with messages
    pub help: Option<String>,      // "try adding a type annotation"
    pub suggestions: Vec<Suggestion>, // Auto-fix suggestions
}
```

Every error type implements the `ToDiagnostic` trait:

```rust
trait ToDiagnostic {
    fn to_diagnostic(&self) -> Diagnostic;
}
```

The formatter (`rask-diagnostics/src/formatter.rs`) takes a diagnostic and the
source code, and produces output like:

```
error[E0012]: type mismatch
  --> file.rk:3:12
   |
 3 |     const x: i32 = "hello"
   |              ^^^   ^^^^^^^ expected i32, found string
   |
help: check the type annotation
```

There's also a JSON output mode for IDE integration.

---

## Shared Concepts

### NodeId

Every AST node gets a unique `NodeId` (a simple integer). This is how later
stages reference specific nodes without modifying the tree:

```rust
struct NodeId(u32);
```

The parser assigns IDs sequentially. The desugarer starts at 1,000,000 to avoid
collisions (since it creates new nodes).

Later stages build maps like:
- `HashMap<NodeId, SymbolId>` (resolver: "this identifier refers to this symbol")
- `HashMap<NodeId, Type>` (type checker: "this expression has this type")

### Span

Every token and AST node carries a `Span`:

```rust
struct Span {
    start: usize,  // Byte offset in source
    end: usize,
}
```

This is how error messages point to the right line and column.

### The `Result` pattern

Most stages return `Result<SuccessType, Vec<ErrorType>>`. The CLI layer
(`rask-cli/src/commands/`) converts errors to diagnostics and displays them:

```rust
let typed = match rask_types::typecheck(resolved, &parse_result.decls) {
    Ok(t) => t,
    Err(errors) => {
        let diags: Vec<Diagnostic> = errors.iter().map(|e| e.to_diagnostic()).collect();
        show_diagnostics(&diags, &source, path, "typecheck", format);
        process::exit(1);
    }
};
```

---

## Rust Concepts You'll Encounter

If you're reading the code and hit unfamiliar Rust syntax, here's a cheat sheet:

| Rust | What it means |
|------|---------------|
| `enum Foo { A, B(i32) }` | A type that can be one of several variants |
| `match x { A => ..., B(n) => ... }` | Pattern matching on an enum |
| `struct Foo { name: String }` | A data structure with named fields |
| `impl Foo { fn bar(&self) {} }` | Methods on a struct |
| `pub` | Public (visible outside the module) |
| `&self` | Borrow of self (read-only reference) |
| `&mut self` | Mutable borrow of self |
| `Box<T>` | Heap-allocated value (needed for recursive types) |
| `Vec<T>` | Growable array |
| `HashMap<K, V>` | Key-value map |
| `Option<T>` | Either `Some(value)` or `None` |
| `Result<T, E>` | Either `Ok(value)` or `Err(error)` |
| `?` operator | Return early if error |
| `trait Foo { fn bar(&self); }` | Interface definition |
| `impl Foo for Bar { }` | Implementing an interface |
| `#[derive(Debug, Clone)]` | Auto-generate common trait implementations |
| `mod foo;` | Include a submodule (from `foo.rs` or `foo/mod.rs`) |
| `use crate::foo::Bar;` | Import from within the same crate |
| `pub use foo::Bar;` | Re-export (make visible to users of this crate) |

---

## Quick Reference: Where to Find Things

| If you need to... | Look in... |
|---|---|
| Add a new keyword | `rask-ast/src/token.rs` (TokenKind) + `rask-lexer/src/lexer.rs` (RawToken) |
| Add new syntax | `rask-parser/src/parser.rs` |
| Add a new AST node | `rask-ast/src/expr.rs`, `stmt.rs`, or `decl.rs` |
| Add a new expression type | `rask-ast/src/expr.rs` (ExprKind) then handle in parser, desugar, type checker, interpreter |
| Change type checking rules | `rask-types/src/checker/` (check_expr.rs, check_stmt.rs, etc.) |
| Add a built-in method | `rask-stdlib/` for type checker + `rask-interp/src/builtins/` for runtime |
| Change ownership rules | `rask-ownership/src/lib.rs` |
| Change error messages | `rask-diagnostics/` or the specific stage's error module |
| Add a CLI command | `rask-cli/src/main.rs` + `rask-cli/src/commands/` |
| Change how the interpreter runs | `rask-interp/src/interp/` (eval_expr.rs, exec_stmt.rs, call.rs) |

## Adding a new feature: the full path

Say you want to add a new statement like `defer { cleanup() }`. You'd touch:

1. **rask-ast** — Add `StmtKind::Defer { body: Vec<Stmt> }` to `stmt.rs`
2. **rask-lexer** — Add `#[token("defer")]` to `RawToken`, add `Defer` to `TokenKind`
3. **rask-parser** — Handle `TokenKind::Defer` in `parse_stmt()`, parse the body
4. **rask-desugar** — Add a case in `desugar_stmt()` to recurse into the body
5. **rask-resolve** — Handle `StmtKind::Defer` in the resolver's statement walker
6. **rask-types** — Handle it in `check_stmt.rs` (type-check the body)
7. **rask-ownership** — Handle it in the ownership checker (verify borrows)
8. **rask-interp** — Handle it in `exec_stmt.rs` (execute at scope exit)
9. **rask-mir** — Handle it in `lower/stmt.rs` (emit MIR for it)

Every new feature follows this pattern: define the data, lex it, parse it,
then handle it in every subsequent stage.
