// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Analysis commands: typecheck, ownership, comptime, unsafe report.

use colored::Colorize;
use rask_diagnostics::{Diagnostic, ToDiagnostic};
use rask_types::UnsafeCategory;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::process;

use std::path::Path;

use crate::{output, Format, show_diagnostics, collect_rk_files};

/// Resolve a path to the .rk file(s) to process.
/// - Single file: returns it as-is.
/// - Directory with build.rk: returns the first .rk file (package detection handles the rest).
/// - Directory without build.rk: returns all .rk files for per-file processing.
fn resolve_rk_targets(path: &str) -> Vec<String> {
    let p = Path::new(path);
    if !p.is_dir() {
        return vec![path.to_string()];
    }

    let files = collect_rk_files(p);
    if files.is_empty() {
        eprintln!("{}: no .rk files found in {}", output::error_label(), output::file_path(path));
        process::exit(1);
    }

    // Package directory: run_frontend on one file discovers the whole package
    if p.join("build.rk").is_file() {
        return vec![files[0].clone()];
    }

    files
}

pub fn cmd_typecheck(path: &str, format: Format) {
    let files = resolve_rk_targets(path);
    for file in &files {
        typecheck_single(file, format, files.len() > 1);
    }
}

fn typecheck_single(path: &str, format: Format, multi: bool) {
    let result = crate::run_check_or_exit(path, format);

    if format == Format::Human {
        if multi {
            println!("{} {} {}", "===".dimmed(), output::file_path(path), "===".dimmed());
        }
        println!("{} Types ({} registered) {}\n", "===".dimmed(), result.typed.types.iter().count(), "===".dimmed());
        for type_def in result.typed.types.iter() {
            match type_def {
                rask_types::TypeDef::Struct { name, fields, .. } => {
                    println!("  struct {} {{", name);
                    for (field_name, field_ty) in fields {
                        println!("    {}: {:?}", field_name, field_ty);
                    }
                    println!("  }}");
                }
                rask_types::TypeDef::Enum { name, variants, .. } => {
                    println!("  enum {} {{", name);
                    for (var_name, var_types) in variants {
                        if var_types.is_empty() {
                            println!("    {}", var_name);
                        } else {
                            println!("    {}({:?})", var_name, var_types);
                        }
                    }
                    println!("  }}");
                }
                rask_types::TypeDef::Trait { name, .. } => {
                    println!("  trait {}", name);
                }
                rask_types::TypeDef::Union { name, fields, .. } => {
                    println!("  union {} {{", name);
                    for (field_name, field_ty) in fields {
                        println!("    {}: {:?}", field_name, field_ty);
                    }
                    println!("  }}");
                }
                rask_types::TypeDef::NominalAlias { name, underlying, with_traits } => {
                    if with_traits.is_empty() {
                        println!("  type {} = {:?}", name, underlying);
                    } else {
                        println!("  type {} = {:?} with ({})", name, underlying, with_traits.join(", "));
                    }
                }
            }
        }

        println!("\n{} Expression Types ({}) {}\n", "===".dimmed(), result.typed.node_types.len(), "===".dimmed());
        let mut count = 0;
        for (node_id, ty) in &result.typed.node_types {
            if count < 20 {
                println!("  NodeId({}) -> {:?}", node_id.0, ty);
                count += 1;
            }
        }
        if result.typed.node_types.len() > 20 {
            println!("  ... and {} more", result.typed.node_types.len() - 20);
        }

        println!("\n{}", output::banner_ok("Typecheck"));
    }
}

pub fn cmd_ownership(path: &str, format: Format) {
    let files = resolve_rk_targets(path);
    for file in &files {
        let _result = crate::run_check_or_exit(file, format);
    }

    // run_frontend already checked ownership — if we get here, it passed
    if format == Format::Human {
        println!("{}", output::banner_ok("Ownership"));
        println!();
        println!("All ownership and borrowing rules verified:");
        println!("  {} No use-after-move errors", output::status_pass());
        println!("  {} Borrow scopes valid", output::status_pass());
        println!("  {} Aliasing rules satisfied", output::status_pass());
    }
}

pub fn cmd_comptime(path: &str, format: Format) {
    let p = Path::new(path);
    let files: Vec<String> = if p.is_dir() {
        let f = collect_rk_files(p);
        if f.is_empty() {
            eprintln!("{}: no .rk files found in {}", output::error_label(), output::file_path(path));
            process::exit(1);
        }
        f
    } else {
        vec![path.to_string()]
    };

    let multi = files.len() > 1;
    let mut total_evaluated = 0usize;
    let mut total_errors = 0usize;

    for file in &files {
        let source = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{}: reading {}: {}", output::error_label(), output::file_path(file), e);
                total_errors += 1;
                continue;
            }
        };

        let mut lexer = rask_lexer::Lexer::new(&source);
        let lex_result = lexer.tokenize();

        if !lex_result.is_ok() {
            let diags: Vec<Diagnostic> = lex_result.errors.iter().map(|e| e.to_diagnostic()).collect();
            show_diagnostics(&diags, &source, file, "lex", format);
            if format == Format::Human {
                eprintln!("\n{}", output::banner_fail("Lex", lex_result.errors.len()));
            }
            total_errors += lex_result.errors.len();
            continue;
        }

        let mut parser = rask_parser::Parser::new(lex_result.tokens);
        let mut parse_result = parser.parse();

        if !parse_result.is_ok() {
            let diags: Vec<Diagnostic> = parse_result.errors.iter().map(|e| e.to_diagnostic()).collect();
            show_diagnostics(&diags, &source, file, "parse", format);
            if format == Format::Human {
                eprintln!("\n{}", output::banner_fail("Parse", parse_result.errors.len()));
            }
            total_errors += parse_result.errors.len();
            continue;
        }

        rask_desugar::desugar(&mut parse_result.decls);

        let mut comptime_interp = rask_comptime::ComptimeInterpreter::new();
        comptime_interp.register_functions(&parse_result.decls);

        if multi && format == Format::Human {
            println!("{} {} {}", "===".dimmed(), output::file_path(file), "===".dimmed());
        }

        for decl in &parse_result.decls {
            if let rask_ast::decl::DeclKind::Const(c) = &decl.kind {
                if matches!(c.init.kind, rask_ast::expr::ExprKind::Comptime { .. }) {
                    match comptime_interp.eval_expr(&c.init) {
                        Ok(val) => {
                            total_evaluated += 1;
                            if format == Format::Human {
                                println!("  {} const {} = {:?}", output::status_pass(), c.name, val);
                            }
                        }
                        Err(e) => {
                            total_errors += 1;
                            if format == Format::Human {
                                eprintln!("  {} const {}: {}", output::status_fail(), c.name, e);
                            }
                        }
                    }
                }
            }
        }
    }

    if format == Format::Human {
        println!();
        if total_errors == 0 {
            println!("{}", output::banner_ok("Comptime"));
            println!();
            println!("Evaluated {} comptime block(s) successfully.", total_evaluated);
        } else {
            eprintln!("{}", output::banner_fail("Comptime", total_errors));
            eprintln!();
            eprintln!("{} evaluated, {} failed", total_evaluated, total_errors);
            process::exit(1);
        }
    }
}

fn category_label(cat: UnsafeCategory) -> &'static str {
    match cat {
        UnsafeCategory::PointerDeref => "Pointer Dereference",
        UnsafeCategory::PointerDerefWrite => "Pointer Dereference (write)",
        UnsafeCategory::PointerArithmetic => "Pointer Arithmetic",
        UnsafeCategory::PointerMethod => "Pointer Method",
        UnsafeCategory::ExternCall => "Extern Call",
        UnsafeCategory::UnsafeFuncCall => "Unsafe Function Call",
        UnsafeCategory::Transmute => "Transmute",
        UnsafeCategory::UnionFieldAccess => "Union Field Access",
    }
}

pub fn cmd_unsafe_report(path: &str, format: Format) {
    let files = resolve_rk_targets(path);
    let multi = files.len() > 1;
    for file in &files {
        unsafe_report_single(file, format, multi);
    }
}

fn unsafe_report_single(path: &str, format: Format, multi: bool) {
    let result = crate::run_check_or_exit(path, format);

    let ops = &result.typed.unsafe_ops;

    if format == Format::Json {
        let mut json = String::from("{\n  \"unsafe_ops\": [");
        for (i, (span, cat)) in ops.iter().enumerate() {
            if i > 0 { json.push(','); }
            let _ = write!(json,
                "\n    {{\"category\": \"{:?}\", \"start\": {}, \"end\": {}}}",
                cat, span.start, span.end
            );
        }
        let _ = write!(json, "\n  ],\n  \"total\": {}\n}}", ops.len());
        println!("{}", json);
        return;
    }

    // Human format
    if multi {
        println!("{} {} {}", "===".dimmed(), output::file_path(path), "===".dimmed());
    }

    if ops.is_empty() {
        println!("{}", output::banner_ok("Unsafe Report"));
        println!();
        println!("No unsafe operations found.");
        return;
    }

    // Build line map from the first source file (for line/col display).
    let line_map = result.source_files.first().map(|(_, s)| rask_ast::LineMap::new(s));

    // Group by category, preserving order via BTreeMap on discriminant
    let mut grouped: BTreeMap<u8, (UnsafeCategory, Vec<&rask_ast::Span>)> = BTreeMap::new();
    for (span, cat) in ops {
        let key = *cat as u8;
        grouped.entry(key).or_insert_with(|| (*cat, Vec::new())).1.push(span);
    }

    println!("{} Unsafe Report {}\n", "===".dimmed(), "===".dimmed());

    let mut total_categories = 0;
    for (_key, (cat, spans)) in &grouped {
        total_categories += 1;
        println!("{} ({})", category_label(*cat).yellow(), spans.len());
        for span in spans {
            if let Some(ref lm) = line_map {
                let (line, col) = lm.offset_to_line_col(span.start);
                println!("  {}:{}:{}", output::file_path(path), line, col);
            } else {
                println!("  offset {}..{}", span.start, span.end);
            }
        }
        println!();
    }

    println!("{} {} unsafe operation(s) across {} categor{} {}",
        "===".dimmed(),
        ops.len(),
        total_categories,
        if total_categories == 1 { "y" } else { "ies" },
        "===".dimmed(),
    );
}
