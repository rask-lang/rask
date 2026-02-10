// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Analysis commands: typecheck, ownership, comptime.

use colored::Colorize;
use rask_diagnostics::{Diagnostic, ToDiagnostic};
use std::fs;
use std::process;

use crate::{output, Format, show_diagnostics};

pub fn cmd_typecheck(path: &str, format: Format) {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: reading {}: {}", output::error_label(), output::file_path(path), e);
            process::exit(1);
        }
    };

    let mut lexer = rask_lexer::Lexer::new(&source);
    let lex_result = lexer.tokenize();

    if !lex_result.is_ok() {
        let diags: Vec<Diagnostic> = lex_result.errors.iter().map(|e| e.to_diagnostic()).collect();
        show_diagnostics(&diags, &source, path, "lex", format);
        if format == Format::Human {
            eprintln!("\n{}", output::banner_fail("Lex", lex_result.errors.len()));
        }
        process::exit(1);
    }

    let mut parser = rask_parser::Parser::new(lex_result.tokens);
    let mut parse_result = parser.parse();

    if !parse_result.is_ok() {
        let diags: Vec<Diagnostic> = parse_result.errors.iter().map(|e| e.to_diagnostic()).collect();
        show_diagnostics(&diags, &source, path, "parse", format);
        if format == Format::Human {
            eprintln!("\n{}", output::banner_fail("Parse", parse_result.errors.len()));
        }
        process::exit(1);
    }

    rask_desugar::desugar(&mut parse_result.decls);

    let resolved = match rask_resolve::resolve(&parse_result.decls) {
        Ok(r) => r,
        Err(errors) => {
            let diags: Vec<Diagnostic> = errors.iter().map(|e| e.to_diagnostic()).collect();
            show_diagnostics(&diags, &source, path, "resolve", format);
            if format == Format::Human {
                eprintln!("\n{}", output::banner_fail("Resolve", errors.len()));
            }
            process::exit(1);
        }
    };

    match rask_types::typecheck(resolved, &parse_result.decls) {
        Ok(typed) => {
            if format == Format::Human {
                println!("{} Types ({} registered) {}\n", "===".dimmed(), typed.types.iter().count(), "===".dimmed());
                for type_def in typed.types.iter() {
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
                    }
                }

                println!("\n{} Expression Types ({}) {}\n", "===".dimmed(), typed.node_types.len(), "===".dimmed());
                let mut count = 0;
                for (node_id, ty) in &typed.node_types {
                    if count < 20 {
                        println!("  NodeId({}) -> {:?}", node_id.0, ty);
                        count += 1;
                    }
                }
                if typed.node_types.len() > 20 {
                    println!("  ... and {} more", typed.node_types.len() - 20);
                }

                println!("\n{}", output::banner_ok("Typecheck"));
            }
        }
        Err(errors) => {
            let diags: Vec<Diagnostic> = errors.iter().map(|e| e.to_diagnostic()).collect();
            show_diagnostics(&diags, &source, path, "typecheck", format);
            if format == Format::Human {
                eprintln!("\n{}", output::banner_fail("Typecheck", errors.len()));
            }
            process::exit(1);
        }
    }
}

pub fn cmd_ownership(path: &str, format: Format) {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: reading {}: {}", output::error_label(), output::file_path(path), e);
            process::exit(1);
        }
    };

    let mut lexer = rask_lexer::Lexer::new(&source);
    let lex_result = lexer.tokenize();

    if !lex_result.is_ok() {
        let diags: Vec<Diagnostic> = lex_result.errors.iter().map(|e| e.to_diagnostic()).collect();
        show_diagnostics(&diags, &source, path, "lex", format);
        if format == Format::Human {
            eprintln!("\n{}", output::banner_fail("Lex", lex_result.errors.len()));
        }
        process::exit(1);
    }

    let mut parser = rask_parser::Parser::new(lex_result.tokens);
    let mut parse_result = parser.parse();

    if !parse_result.is_ok() {
        let diags: Vec<Diagnostic> = parse_result.errors.iter().map(|e| e.to_diagnostic()).collect();
        show_diagnostics(&diags, &source, path, "parse", format);
        if format == Format::Human {
            eprintln!("\n{}", output::banner_fail("Parse", parse_result.errors.len()));
        }
        process::exit(1);
    }

    rask_desugar::desugar(&mut parse_result.decls);

    let resolved = match rask_resolve::resolve(&parse_result.decls) {
        Ok(r) => r,
        Err(errors) => {
            let diags: Vec<Diagnostic> = errors.iter().map(|e| e.to_diagnostic()).collect();
            show_diagnostics(&diags, &source, path, "resolve", format);
            if format == Format::Human {
                eprintln!("\n{}", output::banner_fail("Resolve", errors.len()));
            }
            process::exit(1);
        }
    };

    let typed = match rask_types::typecheck(resolved, &parse_result.decls) {
        Ok(t) => t,
        Err(errors) => {
            let diags: Vec<Diagnostic> = errors.iter().map(|e| e.to_diagnostic()).collect();
            show_diagnostics(&diags, &source, path, "typecheck", format);
            if format == Format::Human {
                eprintln!("\n{}", output::banner_fail("Typecheck", errors.len()));
            }
            process::exit(1);
        }
    };

    let ownership_result = rask_ownership::check_ownership(&typed, &parse_result.decls);

    if ownership_result.is_ok() {
        if format == Format::Human {
            println!("{}", output::banner_ok("Ownership"));
            println!();
            println!("All ownership and borrowing rules verified:");
            println!("  {} No use-after-move errors", output::status_pass());
            println!("  {} Borrow scopes valid", output::status_pass());
            println!("  {} Aliasing rules satisfied", output::status_pass());
        }
    } else {
        let diags: Vec<Diagnostic> = ownership_result.errors.iter().map(|e| e.to_diagnostic()).collect();
        show_diagnostics(&diags, &source, path, "ownership", format);
        if format == Format::Human {
            eprintln!("\n{}", output::banner_fail("Ownership", ownership_result.errors.len()));
        }
        process::exit(1);
    }
}

pub fn cmd_comptime(path: &str, format: Format) {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: reading {}: {}", output::error_label(), output::file_path(path), e);
            process::exit(1);
        }
    };

    let mut lexer = rask_lexer::Lexer::new(&source);
    let lex_result = lexer.tokenize();

    if !lex_result.is_ok() {
        let diags: Vec<Diagnostic> = lex_result.errors.iter().map(|e| e.to_diagnostic()).collect();
        show_diagnostics(&diags, &source, path, "lex", format);
        if format == Format::Human {
            eprintln!("\n{}", output::banner_fail("Lex", lex_result.errors.len()));
        }
        process::exit(1);
    }

    let mut parser = rask_parser::Parser::new(lex_result.tokens);
    let mut parse_result = parser.parse();

    if !parse_result.is_ok() {
        let diags: Vec<Diagnostic> = parse_result.errors.iter().map(|e| e.to_diagnostic()).collect();
        show_diagnostics(&diags, &source, path, "parse", format);
        if format == Format::Human {
            eprintln!("\n{}", output::banner_fail("Parse", parse_result.errors.len()));
        }
        process::exit(1);
    }

    rask_desugar::desugar(&mut parse_result.decls);

    // Evaluate comptime blocks using the restricted comptime interpreter
    let mut comptime_interp = rask_comptime::ComptimeInterpreter::new();
    comptime_interp.register_functions(&parse_result.decls);

    let mut evaluated = 0usize;
    let mut errors = Vec::new();

    for decl in &parse_result.decls {
        if let rask_ast::decl::DeclKind::Const(c) = &decl.kind {
            if matches!(c.init.kind, rask_ast::expr::ExprKind::Comptime { .. }) {
                match comptime_interp.eval_expr(&c.init) {
                    Ok(val) => {
                        evaluated += 1;
                        if format == Format::Human {
                            println!("  {} const {} = {:?}", output::status_pass(), c.name, val);
                        }
                    }
                    Err(e) => {
                        errors.push((c.name.clone(), e));
                    }
                }
            }
        }
    }

    if format == Format::Human {
        println!();
        if errors.is_empty() {
            println!("{}", output::banner_ok("Comptime"));
            println!();
            println!("Evaluated {} comptime block(s) successfully.", evaluated);
        } else {
            for (name, err) in &errors {
                eprintln!("  {} const {}: {}", output::status_fail(), name, err);
            }
            eprintln!();
            eprintln!("{}", output::banner_fail("Comptime", errors.len()));
            eprintln!();
            eprintln!("{} evaluated, {} failed", evaluated, errors.len());
            process::exit(1);
        }
    }
}
