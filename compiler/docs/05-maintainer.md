# Maintainer Guide

Practical reference for working in the codebase.


## Where to find things

| If you need to... | Look in... |
|---|---|
| Add a new keyword | `rask-ast/src/token.rs` + `rask-lexer/src/lexer.rs` |
| Add new syntax | `rask-parser/src/parser.rs` |
| Add a new AST node | `rask-ast/src/expr.rs`, `stmt.rs`, or `decl.rs` |
| Add a built-in type method | `rask-stdlib/` (type checker) + `rask-interp/src/builtins/` (runtime) |
| Change type checking rules | `rask-types/src/checker/check_expr.rs`, `check_stmt.rs` |
| Change ownership rules | `rask-ownership/src/lib.rs` |
| Change error messages | `rask-diagnostics/` or the stage's error module |
| Add a CLI command | `rask-cli/src/main.rs` + `rask-cli/src/commands/` |
| Add an interpreter stdlib module | `rask-interp/src/stdlib/` + `rask-interp/src/value.rs` (ModuleKind) |
| Change interpreter execution | `rask-interp/src/interp/eval_expr.rs`, `exec_stmt.rs`, `call.rs` |
| Change MIR lowering | `rask-mir/src/lower/` |
| Change native codegen | `rask-codegen/src/builder.rs` |
| Add a lint rule | `rask-lint/src/` (naming.rs, style.rs, idiom.rs) |


## Adding a new feature end-to-end

Say you want to add a new statement like `defer { cleanup() }`. You'd touch
every stage in order:

1. **`rask-ast`** — Add `StmtKind::Defer { body: Vec<Stmt> }` to `stmt.rs`

2. **`rask-lexer`** — Add `#[token("defer")]` to `RawToken` in `lexer.rs`.
   Add `Defer` to `TokenKind` in `rask-ast/src/token.rs`.

3. **`rask-parser`** — Handle `TokenKind::Defer` in `parse_stmt()`. Parse the
   body block.

4. **`rask-desugar`** — Add a case in `desugar_stmt()` to recurse into the
   body (so operators inside get desugared too).

5. **`rask-resolve`** — Handle `StmtKind::Defer` in the resolver's statement
   walker.

6. **`rask-types`** — Handle it in `check_stmt.rs`. Type-check the body.

7. **`rask-ownership`** — Handle it in the ownership checker. Verify borrows
   are valid.

8. **`rask-interp`** — Handle it in `exec_stmt.rs`. Execute the body at scope
   exit.

9. **`rask-mir`** — Handle it in `lower/stmt.rs`. Emit MIR blocks for it.

10. **`rask-codegen`** — Usually no changes needed if MIR handles it.

Every new language feature follows this pattern: define the data structure,
add lexer/parser support, then handle it in each subsequent stage.


## Adding a built-in method

If you want to add `string.reverse()`:

1. **`rask-stdlib/src/types.rs`** — Add `"reverse"` to the string method list
   with its signature. This is what the type checker sees.

2. **`rask-interp/src/builtins/strings.rs`** — Implement the actual runtime
   behavior. Take the string value, reverse it, return it.

3. **`rask-codegen/src/dispatch.rs`** — If the method needs native codegen
   support, add a mapping to the C runtime function.

4. **`runtime/runtime.c`** — If needed, add the C implementation.


## Adding a CLI command

1. **`rask-cli/src/main.rs`** — Add a match arm in the `match cmd_args[1]`
   block. Handle `--help`, validate args, dispatch to a handler function.

2. **`rask-cli/src/commands/`** — Create or add to a command file. The handler
   reads the file, runs whichever pipeline stages it needs, and displays
   results.

3. **`rask-cli/src/help.rs`** — Add a `print_<command>_help()` function.


## Error handling pattern

Every stage follows the same pattern:

```rust
// Stage produces its own error type
pub enum ParseError {
    Expected { what: String, got: TokenKind, span: Span },
    // ...
}

// Convert to unified Diagnostic
impl ToDiagnostic for ParseError {
    fn to_diagnostic(&self) -> Diagnostic {
        Diagnostic {
            severity: Severity::Error,
            code: Some("E0001".to_string()),
            message: format!("expected {}, found {}", self.what, self.got),
            labels: vec![Label::primary(self.span, "here")],
            help: Some("check your syntax".to_string()),
            suggestions: vec![],
        }
    }
}
```

The CLI layer collects these and displays them:

```rust
let diags: Vec<Diagnostic> = errors.iter().map(|e| e.to_diagnostic()).collect();
show_diagnostics(&diags, &source, path, "parse", format);
```


## Inspecting pipeline output

Every stage has a corresponding CLI command:

```bash
rask lex file.rk        # Token stream
rask parse file.rk      # AST (debug-printed declarations)
rask resolve file.rk    # Symbol table + resolution map
rask check file.rk      # Type assignments + type definitions
rask ownership file.rk  # Ownership/borrow verification
rask mono file.rk       # Monomorphized functions + memory layouts
rask mir file.rk        # MIR control-flow graphs
```

All accept `--json` for structured output.


## The Rust you'll encounter

Quick reference for non-Rust developers reading the code:

| Rust | Meaning |
|------|---------|
| `enum Foo { A, B(i32) }` | Type with variants (like tagged unions) |
| `match x { A => ..., B(n) => ... }` | Pattern match on a variant |
| `struct Foo { name: String }` | Data structure with named fields |
| `impl Foo { fn bar(&self) {} }` | Methods on a struct |
| `pub` | Public (visible outside the module) |
| `&self` | Read-only borrow of self |
| `&mut self` | Mutable borrow of self |
| `Box<T>` | Heap-allocated value (for recursive types) |
| `Vec<T>` | Growable array |
| `HashMap<K, V>` | Key-value map |
| `Option<T>` | `Some(value)` or `None` |
| `Result<T, E>` | `Ok(value)` or `Err(error)` |
| `?` | Return early if error (unwraps Ok, propagates Err) |
| `trait Foo { fn bar(&self); }` | Interface definition |
| `impl Foo for Bar` | Implementing an interface for a type |
| `#[derive(Debug, Clone)]` | Auto-generate common trait impls |
| `mod foo;` | Include `foo.rs` (or `foo/mod.rs`) as a submodule |
| `use crate::foo::Bar` | Import from within the same crate |
| `pub use foo::Bar` | Re-export |
| `Arc<T>` | Atomic reference counting (thread-safe shared ownership) |
| `Mutex<T>` | Mutual exclusion lock |
| `Arc<Mutex<T>>` | Thread-safe shared mutable value |
| `'a` | Lifetime annotation (how long a reference is valid) |
| `<'a>` on a struct | "This struct borrows something and can't outlive it" |

### Common patterns in this codebase

**The visitor pattern:** Most stages walk the AST recursively. Functions like
`check_decl()`, `check_stmt()`, `check_expr()` each handle one level, calling
each other for nested nodes.

**HashMap as side table:** Instead of mutating the AST, stages build separate
maps keyed by `NodeId`. `HashMap<NodeId, Type>` for types,
`HashMap<NodeId, SymbolId>` for name resolution, etc.

**Error collection:** Stages don't stop at the first error. They collect errors
into a `Vec`, then report all of them at the end. This gives better feedback.

**`pub(super)` and `pub(crate)`:** These are visibility modifiers. `pub(super)`
means "visible to the parent module." `pub(crate)` means "visible within the
crate but not to external users."
