// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Code generation commands: mir.

use colored::Colorize;
use rask_diagnostics::{Diagnostic, ToDiagnostic};
use std::fs;
use std::process;

use crate::{output, show_diagnostics, Format};

/// Dump MIR for a single file.
///
/// Runs the full pipeline (lex → parse → desugar → resolve → typecheck → ownership →
/// monomorphize → lower to MIR) and prints the resulting MIR for each function.
pub fn cmd_mir(path: &str, format: Format) {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "{}: reading {}: {}",
                output::error_label(),
                output::file_path(path),
                e
            );
            process::exit(1);
        }
    };

    // Lex
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

    // Parse
    let mut parser = rask_parser::Parser::new(lex_result.tokens);
    let mut parse_result = parser.parse();
    if !parse_result.is_ok() {
        let diags: Vec<Diagnostic> =
            parse_result.errors.iter().map(|e| e.to_diagnostic()).collect();
        show_diagnostics(&diags, &source, path, "parse", format);
        if format == Format::Human {
            eprintln!(
                "\n{}",
                output::banner_fail("Parse", parse_result.errors.len())
            );
        }
        process::exit(1);
    }

    // Desugar
    rask_desugar::desugar(&mut parse_result.decls);

    // Resolve
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

    // Typecheck
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

    // Ownership
    let ownership_result = rask_ownership::check_ownership(&typed, &parse_result.decls);
    if !ownership_result.is_ok() {
        let diags: Vec<Diagnostic> = ownership_result
            .errors
            .iter()
            .map(|e| e.to_diagnostic())
            .collect();
        show_diagnostics(&diags, &source, path, "ownership", format);
        if format == Format::Human {
            eprintln!(
                "\n{}",
                output::banner_fail("Ownership", ownership_result.errors.len())
            );
        }
        process::exit(1);
    }

    // Monomorphize
    let mono = match rask_mono::monomorphize(&typed, &parse_result.decls) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{}: monomorphization failed: {:?}", output::error_label(), e);
            process::exit(1);
        }
    };

    // Lower each monomorphized function to MIR
    if format == Format::Human {
        println!(
            "{} MIR ({} function{}, {} struct layout{}, {} enum layout{}) {}\n",
            "===".dimmed(),
            mono.functions.len(),
            if mono.functions.len() == 1 { "" } else { "s" },
            mono.struct_layouts.len(),
            if mono.struct_layouts.len() == 1 {
                ""
            } else {
                "s"
            },
            mono.enum_layouts.len(),
            if mono.enum_layouts.len() == 1 {
                ""
            } else {
                "s"
            },
            "===".dimmed()
        );
    }

    let mut mir_errors = 0;
    for mono_fn in &mono.functions {
        match rask_mir::lower::MirLowerer::lower_function(&mono_fn.body) {
            Ok(mir_fn) => {
                if format == Format::Human {
                    println!("{}", mir_fn);
                }
            }
            Err(e) => {
                eprintln!(
                    "{}: lowering function '{}': {:?}",
                    output::error_label(),
                    mono_fn.name,
                    e
                );
                mir_errors += 1;
            }
        }
    }

    if format == Format::Human {
        println!();
        if mir_errors == 0 {
            println!("{}", output::banner_ok("MIR lowering"));
        } else {
            eprintln!("{}", output::banner_fail("MIR lowering", mir_errors));
            process::exit(1);
        }
    }
}
