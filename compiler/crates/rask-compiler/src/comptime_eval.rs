// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! The compile-time fold — the single source of truth for comptime values.
//!
//! Every `comptime` block, every call to a `comptime func`, and every
//! `const … = comptime …` is computed here, while compiling, and recorded
//! under its node (ctrl.comptime/CT2, CT56). Lowering reads the value and
//! never sees the code. Something the evaluator can't compute, or a value
//! nothing static can hold, is a compile error at the `comptime` that asked
//! for it: there is no run-time fallback (#1552).
//!
//! Runs once in the compile pipeline (`finalize_compile`), and on the check
//! path for its errors, so both backends report the same ones.

use std::collections::HashMap;

use rask_ast::decl::{Decl, DeclKind};
use rask_ast::expr::{Expr, ExprKind};
use rask_ast::Span;
use rask_comptime::{ComptimeError, ComptimeInterpreter, ComptimeValue};
use rask_diagnostics::Diagnostic;
use rask_mir::{ComptimeGlobalMeta, Folded, FoldedForm, FoldedNode, FoldedStmt};
use rask_mono::MonoProgram;
use rask_types::TypedProgram;

use crate::comptime_aggregate::{encode, NoBytes};
use crate::{is_comptime_init, CfgConfig};

/// CT35: backwards branches allowed per fold unless `@comptime_quota` says more.
const DEFAULT_BRANCH_QUOTA: usize = 1_000;

/// Fold every compile-time value in the program. Returns the values and the
/// errors; a program with any error doesn't compile.
pub fn fold_comptime(
    decls: &[Decl],
    typed: &TypedProgram,
    mono: &MonoProgram,
    cfg: Option<&CfgConfig>,
) -> (Folded, Vec<Diagnostic>) {
    let mut interp = ComptimeInterpreter::new();
    if let Some(c) = cfg {
        interp.inject_cfg(c);
    }
    interp.register_functions(decls);

    let records = mono.node_records(typed);
    let type_names: HashMap<rask_types::TypeId, String> = typed.types.type_name_map();
    let ctx = rask_mir::lower::MirContext::new(typed, &mono.struct_layouts, &mono.enum_layouts, &records, &type_names);

    let mut fold = Fold { interp, typed, mono, ctx: &ctx, folded: Folded::default(), diags: Vec::new() };

    // Module consts first, in declaration order, so compile-time code can read
    // them: `func f(n: i32 = MAX_RETRIES)` folds `MAX_RETRIES` at every call.
    // A const that has to be compile-time is folded and recorded; any other
    // goes into the environment if it can be computed, and is a run-time
    // value otherwise (`const S = Shared.new(…)`).
    for decl in decls {
        let DeclKind::Const(c) = &decl.kind else { continue };
        if is_comptime_init(&c.init, decls) {
            let quota = fold.quota_of(c, decl.span);
            let declared = c.ty.as_ref().map(|t| fold.ctx.resolve_type_expr(t));
            if let Some(v) = fold.fold(&c.init, quota, declared) {
                fold.folded.consts.insert(c.name.clone(), c.init.id);
                fold.interp.define_global(c.name.clone(), v);
            }
        } else {
            fold.interp.reset_branch_count();
            fold.interp.set_quota(DEFAULT_BRANCH_QUOTA);
            if let Ok(v) = fold.interp.eval_expr(&c.init) {
                fold.interp.define_global(c.name.clone(), v);
            }
        }
    }

    for target in targets(decls) {
        match target {
            Target::Value(e) => {
                fold.fold(e, DEFAULT_BRANCH_QUOTA, None);
            }
            Target::Stmt(stmt, body) => fold.settle(stmt, body),
        }
    }

    // A generic function's copies have nodes of their own, each replacing one
    // of the generic body's. A value folded there is the copy's value too.
    for (&copy, &origin) in &mono.node_origins {
        if let Some(node) = fold.folded.nodes.get(&origin).cloned() {
            fold.folded.nodes.insert(copy, node);
        }
        if let Some(stmt) = fold.folded.statements.get(&origin).copied() {
            fold.folded.statements.insert(copy, stmt);
        }
    }

    // A default is a block at every use; its errors are one error, at the
    // default.
    let mut seen = std::collections::HashSet::new();
    fold.diags.retain(|d| seen.insert((d.message.clone(), d.labels.first().map(|l| (l.span.start, l.span.end)))));
    (fold.folded, fold.diags)
}

/// Does the program have anything to fold?
pub(crate) fn has_comptime(decls: &[Decl]) -> bool {
    decls.iter().any(|d| matches!(&d.kind, DeclKind::Const(c) if is_comptime_init(&c.init, decls)))
        || !targets(decls).is_empty()
}

/// A compile-time position: a value, or a `comptime` statement.
enum Target<'a> {
    Value(&'a Expr),
    Stmt(&'a rask_ast::stmt::Stmt, &'a [rask_ast::stmt::Stmt]),
}

/// Every compile-time position outside a module const's initializer: a
/// `comptime` block, a call to a `comptime func`, a `comptime` statement. Not
/// what's inside one — that's part of computing it — and not the body of a
/// `comptime func` or a `comptime test`, which only ever run at compile time
/// as a whole.
///
/// Nor inside a `comptime for`: its body is unrolled per field during
/// lowering, and what it decides can depend on which field it's on. Lowering
/// settles those, and one it can't settle is an error there.
fn targets(decls: &[Decl]) -> Vec<Target<'_>> {
    struct Collect<'a, 'd> {
        decls: &'d [Decl],
        found: Vec<Target<'a>>,
    }
    impl<'a> rask_ast::visit::Visit<'a> for Collect<'a, '_> {
        fn expr(&mut self, e: &'a Expr) -> bool {
            if matches!(e.kind, ExprKind::Comptime { .. }) || is_comptime_init(e, self.decls) {
                self.found.push(Target::Value(e));
                return false;
            }
            true
        }
        fn stmt(&mut self, s: &'a rask_ast::stmt::Stmt) -> bool {
            match &s.kind {
                rask_ast::stmt::StmtKind::Comptime(body) => {
                    self.found.push(Target::Stmt(s, body));
                    false
                }
                rask_ast::stmt::StmtKind::ComptimeFor { .. } => false,
                _ => true,
            }
        }
    }
    let mut c = Collect { decls, found: Vec::new() };
    for decl in decls {
        let bodies: Vec<&[rask_ast::stmt::Stmt]> = match &decl.kind {
            DeclKind::Fn(f) if !f.is_comptime => vec![&f.body],
            DeclKind::Struct(s) => s.methods.iter().filter(|m| !m.is_comptime).map(|m| m.body.as_slice()).collect(),
            DeclKind::Enum(e) => e.methods.iter().filter(|m| !m.is_comptime).map(|m| m.body.as_slice()).collect(),
            DeclKind::Impl(i) => i.methods.iter().filter(|m| !m.is_comptime).map(|m| m.body.as_slice()).collect(),
            DeclKind::Test(t) if !t.is_comptime => vec![&t.body],
            DeclKind::Benchmark(b) => vec![&b.body],
            _ => Vec::new(),
        };
        for body in bodies {
            rask_ast::visit::visit_body(body, &mut c);
        }
    }
    c.found
}

struct Fold<'a, 'c> {
    interp: ComptimeInterpreter,
    typed: &'a TypedProgram,
    mono: &'a MonoProgram,
    ctx: &'a rask_mir::lower::MirContext<'c>,
    folded: Folded,
    diags: Vec<Diagnostic>,
}

impl Fold<'_, '_> {
    /// `@comptime_quota(N)` on a const, else the default.
    fn quota_of(&mut self, c: &rask_ast::decl::ConstDecl, span: Span) -> usize {
        let Some(arg) = c.comptime_quota_arg() else { return DEFAULT_BRANCH_QUOTA };
        match arg.literal() {
            Some(rask_ast::expr::Literal::Int(n)) if n > 0 => usize::try_from(n).unwrap_or(usize::MAX),
            _ => {
                self.diags.push(
                    Diagnostic::error(format!(
                        "`@comptime_quota` wants a positive number of branches, found `{arg}`"
                    ))
                    .with_code("E0383")
                    .with_primary(span, "on this const")
                    .with_why("the quota is a branch count (ctrl.comptime/CT35); the default is 1,000"),
                );
                DEFAULT_BRANCH_QUOTA
            }
        }
    }

    /// Compute `node`, write it out as the type it has, and record it. The
    /// value on success, for a const the rest of the fold can read.
    ///
    /// `declared` is a const's written type, which is what its value is.
    fn fold(&mut self, node: &Expr, quota: usize, declared: Option<rask_mir::MirType>) -> Option<ComptimeValue> {
        self.interp.reset_branch_count();
        self.interp.set_quota(quota);
        let value = match self.interp.eval_expr(node) {
            Ok(v) => v,
            Err(e) => {
                self.diags.push(eval_error(&e, node.span));
                return None;
            }
        };
        let checked = self.typed.node_types.get(&node.id);
        let ty = declared.unwrap_or_else(|| match checked {
            Some(t) => self.ctx.type_to_mir(t),
            None => rask_mir::MirType::I64,
        });
        match self.data_for(&value, &ty, checked) {
            Ok(meta) => {
                let symbol = rask_mir::folded_symbol(node.id);
                self.folded.data.insert(symbol.clone(), meta);
                self.folded.nodes.insert(node.id, FoldedNode { symbol, ty });
                Some(value)
            }
            Err(NoBytes::Overflow(why)) => {
                self.diags.push(
                    Diagnostic::error(format!("integer overflow at compile time: {why}"))
                        .with_code("R0018")
                        .with_primary(node.span, "computed here")
                        .with_why("comptime overflow is a compile error (type.overflow/CT1)"),
                );
                None
            }
            Err(NoBytes::Gap(what)) => {
                self.diags.push(
                    Diagnostic::error(format!("this can't be a compile-time value: it holds {what}"))
                        .with_code("E0383")
                        .with_primary(node.span, "computed here")
                        .with_why(
                            "a `comptime` value goes into the program as constant data of its \
                             type, so nothing in it may point at memory the running program \
                             would own (ctrl.comptime/CT58, CT69). Build this one at run time, \
                             without `comptime`",
                        ),
                );
                None
            }
        }
    }

    /// Settle a `comptime` statement. A `comptime if` keeps the branch its
    /// condition picks; a block of statements runs, and an `assert` in it
    /// that fails is a compile error.
    fn settle(&mut self, stmt: &rask_ast::stmt::Stmt, body: &[rask_ast::stmt::Stmt]) {
        self.interp.reset_branch_count();
        self.interp.set_quota(DEFAULT_BRANCH_QUOTA);
        if let Some((cond, _, _)) = rask_ast::comptime_if::parts(body) {
            match self.interp.eval_expr(cond) {
                Ok(ComptimeValue::Bool(b)) => {
                    self.folded.statements.insert(stmt.id, FoldedStmt::Branch(b));
                }
                Ok(other) => self.diags.push(
                    Diagnostic::error(format!(
                        "a `comptime if` condition has to be a `bool`, and this one is a `{}`",
                        other.type_name()
                    ))
                    .with_code("E0383")
                    .with_primary(cond.span, "computed here"),
                ),
                Err(e) => self.diags.push(eval_error(&e, cond.span)),
            }
            return;
        }
        match self.interp.eval_test_block(body) {
            Ok(()) => {
                self.folded.statements.insert(stmt.id, FoldedStmt::Ran);
            }
            Err(e) => self.diags.push(eval_error(&e, stmt.span)),
        }
    }

    /// The data a value is used from: a string's characters, a list's or a
    /// map's elements, or anything else's bytes as `ty`.
    fn data_for(
        &self,
        value: &ComptimeValue,
        ty: &rask_mir::MirType,
        checked: Option<&rask_types::Type>,
    ) -> Result<ComptimeGlobalMeta, NoBytes> {
        let empty = |form| ComptimeGlobalMeta {
            form,
            bytes: Vec::new(),
            elem_count: 0,
            elem: None,
            map_types: None,
            string_relocs: Vec::new(),
        };
        // A collection's element types, from its checked type: `Vec<string>`
        // says its elements are strings even when there are none.
        let type_args: Vec<&rask_types::Type> = match checked {
            Some(rask_types::Type::Generic { args, .. } | rask_types::Type::UnresolvedGeneric { args, .. }) => args
                .iter()
                .filter_map(|a| match a {
                    rask_types::GenericArg::Type(t) => Some(t.as_ref()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        match value {
            ComptimeValue::String(s) if *ty == rask_mir::MirType::String => {
                Ok(ComptimeGlobalMeta { bytes: s.as_bytes().to_vec(), ..empty(FoldedForm::Text) })
            }
            // A list is a `Vec` unless the slot is a fixed array.
            ComptimeValue::Array(_) | ComptimeValue::Map(_) if !matches!(ty, rask_mir::MirType::Array { .. }) => {
                let form = if matches!(value, ComptimeValue::Map(_)) { FoldedForm::Map } else { FoldedForm::List };
                let ser = value.serialize().ok_or_else(|| {
                    NoBytes::Gap("a list or map whose elements aren't numbers, booleans, characters or strings".to_string())
                })?;
                Ok(ComptimeGlobalMeta {
                    bytes: ser.bytes,
                    string_relocs: ser.string_relocs,
                    elem_count: value.elem_count(),
                    elem: type_args.first().map(|t| self.ctx.payload_to_mir(t)),
                    map_types: match type_args.as_slice() {
                        [k, v] => Some((k.to_string(), v.to_string())),
                        _ => value.map_types().map(|(k, v)| (k.to_string(), v.to_string())),
                    },
                    ..empty(form)
                })
            }
            _ => {
                let ser = encode(value, ty, self.ctx, self.mono)?;
                Ok(ComptimeGlobalMeta { bytes: ser.bytes, string_relocs: ser.string_relocs, ..empty(FoldedForm::Value) })
            }
        }
    }
}

/// A block that didn't finish, or that asked the evaluator for something it
/// can't do. Either way the value doesn't exist, and nothing stands in for it.
fn eval_error(e: &ComptimeError, span: Span) -> Diagnostic {
    let (code, why) = match e {
        ComptimeError::DivisionByZero => ("R0001", "division by zero is undefined"),
        ComptimeError::IntegerOverflow(_) => ("R0018", "comptime overflow is a compile error (type.overflow/CT1)"),
        ComptimeError::Unimplemented(_) => (
            "E0383",
            "a `comptime` value is computed while compiling, and `todo()` has no value to give",
        ),
        _ if e.is_hard() => (
            "E0383",
            "a `comptime` value is computed while compiling, so there is no run time to retry on (ctrl.comptime/CT2)",
        ),
        _ => (
            "E0383",
            "a `comptime` value is computed while compiling or not at all; there is no run-time fallback (ctrl.comptime/CT2)",
        ),
    };
    Diagnostic::error(format!("can't compute this at compile time: {e}"))
        .with_code(code)
        .with_primary(span, "computed here")
        .with_why(why)
}

/// Run every `comptime test` in the program (std.testing/T11).
///
/// A `comptime test` is supposed to run during compilation, with a failure
/// reported as a compile error. Nothing ran them: `is_comptime` was parsed and
/// then ignored, so they were collected by the test runner and executed at run
/// time like any other test — `rask check` on a failing one exited 0.
///
/// Every way a test body can fail to finish is an error here, not just a false
/// `assert`. That's what makes the two edge cases in the spec fall out: a test
/// that opens a file or reaches `spawn` can't run at compile time, so saying so
/// is the whole answer.
pub fn evaluate_comptime_tests(decls: &[Decl], cfg: Option<&CfgConfig>) -> Vec<Diagnostic> {
    let tests: Vec<(&rask_ast::decl::TestDecl, Span)> = decls
        .iter()
        .filter_map(|d| match &d.kind {
            DeclKind::Test(t) if t.is_comptime => Some((t, d.span)),
            _ => None,
        })
        .collect();
    if tests.is_empty() {
        return Vec::new();
    }

    let mut interp = rask_comptime::ComptimeInterpreter::new();
    if let Some(c) = cfg {
        interp.inject_cfg(c);
    }
    interp.register_functions(decls);

    let mut diags = Vec::new();
    for (test, span) in tests {
        interp.reset_branch_count();
        let Err(err) = interp.eval_test_block(&test.body) else {
            continue;
        };
        diags.push(comptime_test_diagnostic(&test.name, err, span));
    }
    diags
}

fn comptime_test_diagnostic(
    name: &str,
    err: rask_comptime::ComptimeError,
    test_span: Span,
) -> Diagnostic {
    use rask_comptime::ComptimeError;

    if let ComptimeError::AssertionFailed { kind, label, span } = &err {
        let label = if label.is_empty() { "this was false".to_string() } else { label.clone() };
        return Diagnostic::error(format!("comptime test \"{name}\" failed"))
            .with_code("E0848")
            .with_primary(*span, label)
            .with_why(format!(
                "a `comptime test` runs during compilation, so a failing `{kind}` \
                 is a compile error rather than a red line in the test report \
                 [std.testing/T11]"
            ));
    }

    Diagnostic::error(format!("comptime test \"{name}\" can't run at compile time"))
        .with_code("E0848")
        .with_primary(test_span, err.to_string())
        .with_why(
            "a `comptime test` is evaluated by the compiler, which has no files, \
             no sockets and no scheduler — only the comptime subset. Drop the \
             `comptime` and let it run as an ordinary test [std.testing/T11]"
                .to_string(),
        )
}
