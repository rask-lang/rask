Read this file before editing compiler code. It maps tasks to files so you don't have to grep around.

Pipeline: `.rk → Lexer → Parser → Desugar → Resolve → TypeCheck → Comptime → Ownership → MIR → Codegen/Interp`

## Crate guide

### rask-parser — Recursive descent parser
- `src/parser.rs` — all parsing logic: declarations, expressions, statements, patterns, types
- `src/hints.rs` — "did you mean?" recovery suggestions
- `src/lib.rs` — public API: `parse()` entry point
- New syntax: add parsing method in `parser.rs`, update AST types in `rask-ast`
- Precedence: check `parse_expr_bp()` (Pratt parsing with binding power)

### rask-ast — Shared AST node types
- `src/decl.rs` — declarations: functions, structs, enums, traits, impls
- `src/expr.rs` — expressions, patterns, match arms, operators (`BinOp`/`UnaryOp`)
- `src/stmt.rs` — statements
- `src/token.rs` — token types (shared with lexer)
- `src/lib.rs` — NodeId, Span, LineMap
- Changes here ripple through parser, desugar, types, interp, mir, codegen

### rask-desugar — Pre-typecheck transforms
- `src/lib.rs` — operator desugaring (`a + b` → `a.add(b)`)
- `src/defaults.rs` — default argument filling, named→positional argument resolution

### rask-resolve — Name resolution + package management
- `src/resolver.rs` — main resolution pass
- `src/scope.rs` — scope stack and symbol lookup
- `src/symbol.rs` — SymbolId and symbol table
- `src/error.rs` — resolution errors
- Package management (less commonly edited): `package.rs`, `registry.rs`, `lockfile.rs`, `semver.rs`

### rask-types — Type checker and inference (largest crate)
- `src/checker/check_expr.rs` — type checking expressions (biggest file)
- `src/checker/check_stmt.rs` — type checking statements
- `src/checker/check_fn.rs` — function signature checking
- `src/checker/inference.rs` — type variable creation and constraint generation
- `src/checker/unify.rs` — type unification (constraint solving)
- `src/checker/generics.rs` — generic instantiation and bounds checking
- `src/checker/resolve.rs` — type name resolution (traits, methods)
- `src/checker/borrow.rs` — borrow scope tracking during type check
- `src/checker/errors.rs` — TypeError definitions
- `src/checker/type_defs.rs` — TypeDef, MethodSig, TypedProgram
- `src/checker/builtins.rs` — built-in type registrations
- `src/types.rs` — the `Type` enum itself
- Inference bug: start in `unify.rs`, trace through `inference.rs`
- New built-in type: register in `builtins.rs`, define methods in `rask-stdlib`

### rask-mono — Monomorphization
- `src/reachability.rs` — discovers reachable functions from main(), name mangling
- `src/instantiate.rs` — creates concrete function copies with substituted types
- `src/layout.rs` — computes struct/enum memory layouts (size, alignment, field offsets)
- `src/lib.rs` — MonoProgram, MonoFunction

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
- `src/dispatch.rs` — trait dispatch, `src/vtable.rs` — vtable layout
- `src/debug_info.rs` — DWARF debug info
- Struct layout bug: check `layouts.rs` and `rask-mono` layout computation

### rask-interp — Tree-walking interpreter (primary execution backend)
- `src/interp/` — interpreter core (expression/statement evaluation)
- `src/stdlib/` — runtime implementations of stdlib functions
- `src/value.rs` — runtime Value type
- `src/env.rs` — variable environment / scope stack
- `src/builtins/` — built-in function implementations
- `src/resource.rs` — linear resource tracking at runtime
- New stdlib function: add runtime impl in `stdlib/`, register in `rask-stdlib/src/stubs.rs`

### rask-stdlib — Stdlib type definitions and stubs
- `src/stubs.rs` — function signatures for all stdlib functions (type checker reads these)
- `src/types.rs` — stdlib type definitions (Vec, Map, string, etc.)
- `src/builtins.rs` — built-in operator/method registrations
- `src/registry.rs` — type registry for stdlib lookups
- `src/mir_metadata.rs` — MIR-level metadata for stdlib functions (used by codegen)
- New stdlib function: add signature in `stubs.rs`, runtime in `rask-interp/src/stdlib/`

### rask-diagnostics — Error formatting
- `src/lib.rs` — Diagnostic, Label, LabelStyle types
- `src/formatter.rs` — rich terminal output with colors and source snippets
- `src/json.rs` — JSON output for IDE integration
- `src/convert.rs` — ToDiagnostic trait: converts errors from other crates
- `src/suggestions.rs` — automated fix suggestions
- `src/codes.rs` — error code enum (E001, W002, etc.)

### rask-cli — CLI binary
- `src/main.rs` — argument parsing, command dispatch, pipeline orchestration
- `src/commands/` — subcommand implementations (run, test, check, fmt, etc.)
- `src/help.rs` — help text, `src/output.rs` — output formatting
