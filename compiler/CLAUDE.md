Read this file before editing compiler code. It maps tasks to files so you don't have to grep around.

Pipeline: `.rk → Lexer → Parser → Desugar → Resolve → TypeCheck → Comptime → Ownership → MIR → Codegen/Interp`

## Hand information down, never re-derive it

Each pass hands the next one what it worked out, as data. A later pass never
reconstructs it from text.

- The parser is the only thing that reads source syntax. A type is a `TypeExpr`
  from there on, a checked type is a `Type`, and written type arguments live in
  their own field (`GenericName.type_args`, `StructLit.type_args`), not inside
  a name.
- If a pass needs something an earlier pass knew, add a field and pass it along.
  Don't render a type to a string and parse it again, don't `split('<')` a name,
  don't guess from capitalisation or from how a symbol is spelled.
- A string is a *name*: an identifier or a dotted path. It is never a type.
  `Display` on `TypeExpr`/`Type` is for messages and symbol mangling. Nothing
  reads that output back.

Why it matters: the compiler used to have seven type-string parsers that had
drifted apart. Each disagreement was a bug that only showed on one backend.
`no_type_text_reparsing` in `rask-cli/tests/` fails the build when one comes back.

## Crate guide

### rask-parser — Recursive descent parser
- `src/parser.rs` — all parsing logic: declarations, expressions, statements, patterns, types
- `src/hints.rs` — "did you mean?" recovery suggestions
- `src/lib.rs` — public API: `parse()` entry point
- New syntax: add parsing method in `parser.rs`, update AST types in `rask-ast`
- Precedence: check `parse_expr_bp()` (Pratt parsing with binding power)

### rask-ast — Shared AST node types
- `src/decl.rs` — declarations: functions, structs, enums, interfaces, impls
- `src/expr.rs` — expressions, patterns, match arms, operators (`BinOp`/`UnaryOp`)
- `src/stmt.rs` — statements
- `src/token.rs` — token types (shared with lexer)
- `src/lib.rs` — NodeId, Span, LineMap
- Changes here ripple through parser, desugar, types, interp, mir, codegen

### rask-desugar — Pre-typecheck transforms
- `src/lib.rs` — operator desugaring (`a + b` → `a.add(b)`)
- `src/defaults.rs` — default argument filling; named arguments keep their labels for the checker

### rask-resolve — Name resolution + package management
- `src/resolver.rs` — main resolution pass
- `src/scope.rs` — scope stack and symbol lookup
- `src/symbol.rs` — SymbolId and symbol table
- `src/error.rs` — resolution errors
- Package management (less commonly edited): `package.rs`, `registry.rs`, `lockfile.rs`, `semver.rs`

### rask-types — The type data every later stage reads
- `src/types.rs` — the `Type` enum itself, `TypeId`, `GenericArg`
- `src/defs.rs` — `TypeDef`, `MethodSig`: a declaration once its fields and signatures are `Type`s
- `src/abi.rs` — wrapper layouts and tag offsets the backends share
- Depends on `rask-ast` alone. The checker produces this data; nothing here checks anything

### rask-layout — Sizes, alignments and field offsets
- `src/lib.rs` — the one size model. Monomorphization lays out with it, and the checker's copy rule asks it through `TypeTable`
- Depends on `rask-ast`, `rask-types` and `rask-stdlib`, never on the checker: the checker reads it

### rask-check — Type checker and inference (largest crate)
- `src/checker/check_expr.rs` — type checking expressions (biggest file)
- `src/checker/check_stmt.rs` — type checking statements
- `src/checker/check_fn.rs` — function signature checking
- `src/checker/inference.rs` — type variable creation and constraint generation
- `src/checker/unify.rs` — type unification (constraint solving)
- `src/checker/generics.rs` — generic instantiation and bounds checking
- `src/checker/resolve.rs` — type name resolution (interfaces, methods)
- `src/checker/borrow.rs` — borrow scope tracking during type check
- `src/checker/arg_labels.rs` — named-argument labels checked against the resolved callee
- `src/checker/errors.rs` — TypeError definitions
- `src/checker/type_defs.rs` — TypedProgram, Callee
- `src/checker/builtins.rs` — built-in type registrations
- `src/checker/sizes.rs` — what the checker asks of layout: `TypeTable::size_align` answers the copy rule, `reflect.size_of` and `transmute`
- Inference bug: start in `unify.rs`, trace through `inference.rs`
- New built-in type: register in `builtins.rs`, define methods in `rask-stdlib`

### rask-mono — Monomorphization
- `src/reachability.rs` — discovers reachable functions from main(), name mangling
- `src/instantiate.rs` — creates concrete function copies with substituted types
- `src/lib.rs` — MonoProgram, MonoFunction, the order layouts are computed in

### rask-mir — Mid-level IR (SSA control-flow graph)
**Lowering (AST → MIR):**
- `src/lower/mod.rs` — main lowering context, function lowering entry
- `src/lower/expr.rs` — expression lowering (largest file)
- `src/lower/stmt.rs` — statement lowering
- `src/lower/match_lower.rs` — pattern match compilation
- `src/lower/collections.rs` — Vec/Map/string operations
- `src/lower/concurrency.rs` — spawn, channels, mutex
- `src/lower/closures.rs` — closure capture and lowering
- `src/lower/iterators.rs` — iterator chain fusion

**MIR types say what a value's bytes are and how it's released, nothing about
what it means.** Lowering takes meaning from the checker's type. `string` and
`bytes` are both `Buffer`; whether to print it quoted, which methods it has,
how it renders under `{:debug}` is asked of the checker during lowering, and
no pass after lowering needs to know. The release kinds: nothing (scalars,
`Ptr`, `FuncPtr`), a refcount (`Buffer`), its kind's free (`Container(kind)`),
an owned block (`Heap(T)`), and aggregates release their parts.

**MIR types:**
- `src/lib.rs` — MirFunction, MirStmt, MirTerminator, BlockId, LocalId
- `src/types.rs` — MirType, `src/operand.rs` — MirOperand, MirConst, MirRValue
- `src/builder.rs` — BlockBuilder, `src/program.rs` — MirProgram

**Analysis & transforms:** `src/analysis/` (dataflow), `src/transform/` (optimization passes)
- New MIR instruction: add to `MirStmtKind`/`MirTerminatorKind` in `lib.rs`, update codegen + miri

### rask-codegen — Native codegen via Cranelift
- `src/builder.rs` — main MIR→Cranelift translation (biggest file)
- `src/module.rs` — module setup, function declarations, linking
- `src/types.rs` — MirType→Cranelift type mapping
- `src/layouts.rs` — struct/enum memory layout for Cranelift
- `src/closures.rs` — closure codegen
- `src/dispatch.rs` — stdlib calls into the C runtime: built from `@native("sym")` declarations plus `rask_runtime.h`, and an internal table for names MIR mints. `src/vtable.rs` — vtable layout
- `src/debug_info.rs` — DWARF debug info
- Struct layout bug: check `layouts.rs` and `rask_layout`

### rask-interp — Tree-walking interpreter (the reference backend)

Native is what ships (`rask run` compiles); this is the reference for *what the
answer should be*. Its value is that it shares nothing below the AST — a bug in
MIR lowering or in a MIR pass is invisible to anything built on MIR, and two of
the four divergences fixed in #687/#688/#677/#698 lived exactly there.

- `src/interp/` — interpreter core (expression/statement evaluation)
- `src/stdlib/` — Rust implementations of the primitive layer. Shrinking: a
  module written in Rask has its implementation in `stdlib/*.rk` instead, and a
  Rust copy here would be the second one (see `RASK_IMPLEMENTED_TYPES`).
- `src/value.rs` — runtime Value type
- `src/env.rs` — variable environment / scope stack
- `src/builtins/` — built-in function implementations
- `src/resource.rs` — linear resource tracking at runtime
- New stdlib function: **write it in Rask** in `stdlib/*.rk` if it can be — both
  backends run that source, so there's nothing to disagree. Only reach for a
  Rust implementation here when the operation needs the machine or the OS, and
  then mark the declaration `@native("symbol")`.

### rask-stdlib — Stdlib type definitions and stubs
- `src/stubs.rs` — function signatures for all stdlib functions (type checker reads these)
- `src/types.rs` — stdlib type definitions (Vec, Map, string, etc.)
- `src/builtins.rs` — built-in operator/method registrations
- `src/registry.rs` — type registry for stdlib lookups
- `src/mir_metadata.rs` — MIR-level metadata for stdlib functions (used by codegen)
- New stdlib function: the declaration goes in `stdlib/*.rk`, and it must say
  where its body lives — a Rask body, `comptime func`, `@native("symbol")`,
  `@builtin` (the compiler lowers the call itself, e.g. `Shared.read`), or
  `@unimplemented`. A hollow declaration with no marker fails
  `every_stdlib_function_says_where_its_body_lives`.

### rask-diagnostics — Error formatting
- `src/lib.rs` — Diagnostic, Label, LabelStyle types
- `src/formatter.rs` — rich terminal output with colors and source snippets
- `src/json.rs` — JSON output for IDE integration
- Depends on nothing but `rask-ast`. Each crate turns its own errors into diagnostics in its `src/diagnostic.rs` (`impl ToDiagnostic`); a new error's message goes there, next to the error type. `crate_layers` in `rask-cli/tests/` fails the build if this crate grows a dependency
- `src/suggestions.rs` — automated fix suggestions
- `src/codes.rs` — error code enum (E001, W002, etc.)

### rask-cli — CLI binary
- `src/main.rs` — argument parsing, command dispatch, pipeline orchestration
- `src/commands/` — subcommand implementations (run, test, check, fmt, etc.)
- `src/help.rs` — help text, `src/output.rs` — output formatting
