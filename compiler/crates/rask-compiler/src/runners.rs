// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! The declaration rewrites behind `emit::Entry::Tests` and `Entry::Benches`.
//!
//! Each `test`/`benchmark` block becomes a plain function, and a synthetic
//! `main` calls every one so monomorphization keeps them. Codegen's runner
//! replaces that `main` as the entry point. Both run between the check and
//! monomorphization, through `compile_file_with`'s hook.

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::expr::{Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::{NodeId, Span};

/// Turn `test` blocks into `__test_N` functions. Returns `(display name,
/// function name)` for each test that matches `filter`.
pub fn extract_tests(decls: &mut Vec<Decl>, filter: Option<&str>) -> Vec<(String, String)> {
    let mut tests = Vec::new();
    let mut test_fns = Vec::new();

    for decl in decls.iter() {
        match &decl.kind {
            DeclKind::Test(t) => {
                // A `comptime test` already ran, during the pipeline's comptime
                // pass (std.testing/T11). Emitting it again would run it twice
                // and report the second run as the result.
                if t.is_comptime || filter.is_some_and(|pat| !t.name.contains(pat)) {
                    continue;
                }
                let fn_name = format!("__test_{}", tests.len());
                tests.push((t.name.clone(), fn_name.clone()));
                // Marked so MIR can tell a test body from any other function
                // that happens to return nothing. `try` inside one has no
                // caller and ends the test (#932); inside an ordinary void
                // function it's a compile error, and the two must not be
                // told apart by the return type they share.
                let marker = rask_ast::attr::Attr::synthetic("test_body", None);
                test_fns.push(body_fn(decl, fn_name, t.body.clone(), vec![marker]));
            }
            // @test functions — include as tests, keep the original function too
            DeclKind::Fn(f) if f.attrs.iter().any(|a| a.is("test")) && f.params.is_empty() => {
                if filter.is_some_and(|pat| !f.name.contains(pat)) {
                    continue;
                }
                tests.push((f.name.clone(), f.name.clone()));
            }
            _ => {}
        }
    }

    decls.retain(|d| !matches!(&d.kind, DeclKind::Test(_)));
    install_runner(decls, test_fns, &tests);
    tests
}

/// Turn `benchmark` blocks into `__bench_N` functions. Returns `(display
/// name, function name)` for each benchmark that matches `filter`.
pub fn extract_benchmarks(decls: &mut Vec<Decl>, filter: Option<&str>) -> Vec<(String, String)> {
    let mut benchmarks = Vec::new();
    let mut bench_fns = Vec::new();

    for decl in decls.iter() {
        if let DeclKind::Benchmark(b) = &decl.kind {
            if filter.is_some_and(|pat| !b.name.contains(pat)) {
                continue;
            }
            let fn_name = format!("__bench_{}", benchmarks.len());
            benchmarks.push((b.name.clone(), fn_name.clone()));
            bench_fns.push(body_fn(decl, fn_name, b.body.clone(), vec![]));
        }
    }

    decls.retain(|d| !matches!(&d.kind, DeclKind::Benchmark(_)));
    install_runner(decls, bench_fns, &benchmarks);
    benchmarks
}

/// Swap the program's `main` for one that calls every runner entry.
///
/// Only when there are entries. With none (or none matching the filter)
/// nothing replaces `main`, and dropping it would leave the program with no
/// entry point (#330). A file with neither, like one whose only tests are
/// comptime ones, gets an empty `main` so monomorphization has a root; the
/// runner sees nothing to run and never builds a binary from it.
fn install_runner(decls: &mut Vec<Decl>, body_fns: Vec<Decl>, entries: &[(String, String)]) {
    let has_main = decls.iter().any(is_main);
    if !entries.is_empty() {
        decls.retain(|d| !is_main(d));
    }
    decls.extend(body_fns);
    if !entries.is_empty() || !has_main {
        decls.push(main_calling(entries));
    }
}

fn is_main(d: &Decl) -> bool {
    matches!(&d.kind, DeclKind::Fn(f) if f.name == "main")
}

fn body_fn(block: &Decl, name: String, body: Vec<Stmt>, attrs: Vec<rask_ast::attr::Attr>) -> Decl {
    Decl {
        id: block.id,
        kind: DeclKind::Fn(FnDecl {
            name,
            type_params: vec![],
            params: vec![],
            ret_ty: None,
            body,
            is_pub: false,
            is_private: false,
            is_comptime: false,
            is_unsafe: false,
            abi: None,
            attrs,
            doc: None,
            span: block.span,
            decl_start: block.span.start,
        }),
        span: block.span,
    }
}

/// `func main() { f1() f2() … }`. Codegen generates the real entry point;
/// this one is here so monomorphization reaches every entry.
fn main_calling(entries: &[(String, String)]) -> Decl {
    let span = Span::new(0, 0);
    let mut body: Vec<Stmt> = entries.iter().map(|(_, fn_name)| Stmt {
        id: NodeId(0),
        kind: StmtKind::Expr(Expr {
            id: NodeId(0),
            kind: ExprKind::Call {
                func: Box::new(Expr {
                    id: NodeId(0),
                    kind: ExprKind::Ident(fn_name.clone()),
                    span,
                }),
                args: vec![],
            },
            span,
        }),
        span,
    }).collect();
    if !entries.is_empty() {
        body.push(Stmt { id: NodeId(0), kind: StmtKind::Return(None), span });
    }
    Decl {
        id: NodeId(0),
        kind: DeclKind::Fn(FnDecl {
            name: "main".to_string(),
            type_params: vec![],
            params: vec![],
            ret_ty: None,
            body,
            is_pub: false,
            is_private: false,
            is_comptime: false,
            is_unsafe: false,
            abi: None,
            attrs: vec![],
            doc: None,
            span,
            decl_start: span.start,
        }),
        span,
    }
}
