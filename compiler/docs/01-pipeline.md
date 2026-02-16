# Pipeline Stages

Each stage takes the output of the previous one and produces input for the next.
If any stage fails, the compiler stops and shows diagnostics.

## Stage 1: Lexing (`rask-lexer`)

Turns raw text into a flat list of tokens.

**Files:** `rask-lexer/src/lexer.rs`, `rask-ast/src/token.rs`

The lexer uses a library called **logos** which generates a fast tokenizer from
annotations. There are two passes:

1. **logos pass**: Pattern-matches source text into `RawToken` variants.
   `#[token("func")]` matches exact text, `#[regex(...)]` matches patterns.
2. **Value pass**: `convert_token()` parses the matched text into actual values.
   For example, `"42u32"` becomes `TokenKind::Int(42, Some(IntSuffix::U32))`.

```rust
// logos handles the pattern matching (fast)
#[derive(Logos)]
#[logos(skip r"[ \t]+")]  // skip spaces, NOT newlines
enum RawToken {
    #[token("func")]  Func,
    #[token("==")]    EqEq,
    #[regex(r"[0-9][0-9_]*(i8|i16|i32|...)?")]  DecInt,
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_]*")]         Ident,
    // ...
}
```

Each token carries a `Span` (byte offset range in source). This is how error
messages point to the right character later.

**Why newlines are tokens:** Rask uses newlines as statement terminators (like
Go), so they can't be skipped here. The parser decides when they matter.

**Error recovery:** Collects up to 20 errors before stopping. An unexpected
character gets reported but doesn't abort—the lexer skips it and continues.

**Nested block comments:** `/* */` uses a custom callback that tracks nesting
depth, so `/* outer /* inner */ still comment */` works correctly.


## Stage 2: Parsing (`rask-parser`)

Turns a flat token list into a tree (the AST).

**Files:** `rask-parser/src/parser.rs`, `rask-parser/src/hints.rs`

The parser is a **recursive descent parser** with **Pratt parsing** for
expressions:

- **Recursive descent** means each grammar construct is a function.
  `parse_fn()` calls `parse_stmt()` which calls `parse_expr()`, etc.
- **Pratt parsing** handles operator precedence. Each operator has a "binding
  power"—`*` binds tighter than `+`, so `1 + 2 * 3` becomes `1 + (2 * 3)`.

The parser struct tracks position and collects errors:

```rust
pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,              // current position
    errors: Vec<ParseError>,
    next_node_id: u32,       // counter for unique NodeIds
    pending_gt: bool,        // for splitting >> in generics
    allow_brace_expr: bool,  // false in control flow conditions
}
```

**`pending_gt`** is worth calling out. When parsing `Vec<Map<K, V>>`, the `>>`
at the end is lexed as a single `GtGt` token. The parser needs to split it
into two `>` tokens. `pending_gt` tracks that state.

**`allow_brace_expr`** prevents ambiguity. In `if x { ... }`, the `{` starts
a block, not a struct literal. The parser sets this flag to `false` when parsing
conditions.

**Navigation methods:**

```rust
fn current(&self) -> &Token     // peek at current token
fn advance(&mut self) -> &Token // consume and move forward
fn expect(&mut self, kind) -> Result<&Token, ParseError>
fn skip_newlines(&mut self)     // skip insignificant newlines
fn expect_terminator(&mut self) // expect newline, `;`, `}`, or EOF
```

**Error recovery:** When the parser hits something unexpected, it calls
`synchronize()` which skips tokens until it finds the start of the next
declaration (`func`, `struct`, etc.) and continues. This means one typo
doesn't prevent you from seeing errors elsewhere in the file.

**Rust syntax hints:** The parser recognizes Rust mistakes and gives specific
messages. `fn` → "use `func`", `pub` → "use `public`", `::` → "use `.`",
`let mut` → "`let` is already mutable".

### The AST

Defined in `rask-ast/src/`. Every node has an `id: NodeId`, a `kind` enum, and
a `span: Span`. The three levels:

- **Declarations** (`decl.rs`): Top-level items—functions, structs, enums,
  traits, imports, extend blocks, tests, benchmarks, package declarations.
- **Statements** (`stmt.rs`): Things inside function bodies—let/const bindings,
  assignments, loops, returns, ensure blocks.
- **Expressions** (`expr.rs`): Things that produce values—literals, identifiers,
  binary/unary ops, calls, method calls, if/match, closures, struct literals,
  array literals, ranges, try, spawn, select, etc. (~30 variants.)


## Stage 3: Desugaring (`rask-desugar`)

Transforms operators into method calls before type checking.

**File:** `rask-desugar/src/lib.rs`

`a + b` becomes `a.add(b)`. `a == b` becomes `a.eq(b)`. `a != b` becomes
`!a.eq(b)`. `-x` becomes `x.neg()`.

This simplifies every later stage. The type checker, interpreter, and codegen
don't need special cases for operators—they all go through method dispatch.

**What's NOT desugared:**
- `&&` and `||` stay as binary ops because they short-circuit. `a && b` must
  not evaluate `b` if `a` is false. Method calls evaluate all arguments.
- `!` (logical not), `&` (reference), `*` (deref) stay as unary ops.

The desugarer walks the entire AST recursively, modifying it in place. It
generates fresh `NodeId`s starting at 1,000,000 to avoid collisions with
parser-assigned IDs.


## Stage 4: Name Resolution (`rask-resolve`)

Figures out what every name refers to.

**Files:** `rask-resolve/src/resolver.rs`, `rask-resolve/src/scope.rs`,
`rask-resolve/src/symbol.rs`, `rask-resolve/src/package.rs`

When you write `println("hello")`, the resolver connects that identifier to the
built-in print function. When you write `x + 1`, it connects `x` to the
parameter or local variable named `x`.

**Output:** A `ResolvedProgram` containing:
- **Symbol table**: Every named thing gets a `SymbolId` with its kind, name,
  and source location.
- **Resolution map**: `HashMap<NodeId, SymbolId>` connecting each use of a name
  to its definition.

The resolver uses a **scope tree**:

```rust
pub struct Resolver {
    symbols: SymbolTable,
    scopes: ScopeTree,
    resolutions: HashMap<NodeId, SymbolId>,
    type_param_map: HashMap<String, Vec<TypeParam>>,
    // ...
}
```

It pushes a scope when entering a function/block, pops when leaving. Name
lookup searches from the innermost scope outward.

Built-in functions (`println`, `print`, `panic`, `format`) and built-in types
(`Vec`, `Map`, `Set`, `string`, `Channel`, `Pool`, `Atomic`, `Shared`, etc.)
are registered before any user code.

**Package resolution** (`package.rs`) handles multi-package projects. A
`PackageRegistry` discovers packages from directory structure, resolves
dependencies, detects cycles, and tracks per-package metadata.


## Stage 5: Type Checking (`rask-types`)

Determines the type of every expression and verifies type correctness.

**Files:** `rask-types/src/checker/` (16 files). See [Type System](02-types.md)
for the deep dive.

**Output:** `TypedProgram` containing:
- `TypeTable`: definitions of all structs, enums, traits
- `node_types: HashMap<NodeId, Type>`: the type of every expression
- `call_type_args`: which concrete types were used at generic call sites


## Stage 6: Ownership Checking (`rask-ownership`)

Verifies memory safety: no use-after-free, no double-free, no data races.

**Files:** `rask-ownership/src/lib.rs`, `rask-ownership/src/state.rs`,
`rask-ownership/src/error.rs`

Every binding tracks its state:

```rust
enum BindingState {
    Owned,     // has the value
    Borrowed,  // someone's reading it
    Moved,     // given away, can't use
}
```

The checker walks every function body. When a value is passed with `own`,
it's marked `Moved`. Using it after that is an error. When a `@resource`
binding reaches end of scope without being consumed, that's an error too.

**Two borrow scopes—the key design choice:**
- **Persistent** (block-scoped): `const ref = something` — borrow lasts until
  end of block.
- **Instant** (statement-scoped): `items[0]` — borrow expires at the semicolon.

This means you don't need lifetime annotations for the common case of "index
into a collection, then mutate it on the next line." Rust requires explicit
lifetimes for this; Rask doesn't.


## Stage 7: Hidden Parameter Desugaring (`rask-hidden-params`)

Transforms `using` context clauses into explicit parameters.

**File:** `rask-hidden-params/src/lib.rs`

```rask
func damage(h: Handle<Player>) using Pool<Player> {
    h.health -= 10
}
// becomes:
func damage(h: Handle<Player>, __ctx_pool: &Pool<Player>) {
    h.health -= 10
}
```

Call sites are rewritten too. Runs after type checking, before monomorphization.


## Stage 8: Monomorphization (`rask-mono`)

Eliminates generics by creating concrete copies.

**Files:** `rask-mono/src/lib.rs`, `rask-mono/src/instantiate.rs`,
`rask-mono/src/layout.rs`, `rask-mono/src/reachability.rs`

See [Code Generation](04-codegen.md) for the deep dive.

**Output:** `MonoProgram` with:
- Concrete function instances (generics resolved to real types)
- `StructLayout` / `EnumLayout` (field offsets, sizes, alignment)


## Stage 9: MIR Lowering (`rask-mir`)

Flattens tree-shaped AST into a control-flow graph of basic blocks.

**Files:** `rask-mir/src/lower/mod.rs`, `rask-mir/src/lower/expr.rs`,
`rask-mir/src/lower/stmt.rs`

See [Code Generation](04-codegen.md) for the deep dive.


## Stage 10: Code Generation (`rask-codegen`)

Translates MIR to native machine code via Cranelift, then links with the C
runtime.

**Files:** `rask-codegen/src/builder.rs`, `rask-codegen/src/module.rs`,
`rask-codegen/src/closures.rs`, `rask-codegen/src/dispatch.rs`

See [Code Generation](04-codegen.md) for the deep dive.
