// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Execution commands: run, test, benchmark.

use colored::Colorize;
use rask_diagnostics::{Diagnostic, ToDiagnostic};
use std::fs;
use std::process;

use crate::{output, Format, show_diagnostics};

pub fn cmd_run(path: &str, program_args: Vec<String>, format: Format) {
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

    // Name resolution
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

    // Type checking
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

    // Ownership checking
    let ownership_result = rask_ownership::check_ownership(&typed, &parse_result.decls);
    if !ownership_result.is_ok() {
        let diags: Vec<Diagnostic> = ownership_result.errors.iter().map(|e| e.to_diagnostic()).collect();
        show_diagnostics(&diags, &source, path, "ownership", format);
        if format == Format::Human {
            eprintln!("\n{}", output::banner_fail("Ownership", ownership_result.errors.len()));
        }
        process::exit(1);
    }

    // Run interpreter
    let mut interp = rask_interp::Interpreter::with_args(program_args);
    match interp.run(&parse_result.decls) {
        Ok(_) => {}
        Err(rask_interp::RuntimeError::Exit(code)) => {
            process::exit(code);
        }
        Err(e) => {
            eprintln!("{}: {}", "Runtime error".red().bold(), e);
            process::exit(1);
        }
    }
}

pub fn cmd_test(path: &str, filter: Option<String>, format: Format) {
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

    // Name resolution
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

    // Type checking
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

    // Ownership checking
    let ownership_result = rask_ownership::check_ownership(&typed, &parse_result.decls);
    if !ownership_result.is_ok() {
        let diags: Vec<Diagnostic> = ownership_result.errors.iter().map(|e| e.to_diagnostic()).collect();
        show_diagnostics(&diags, &source, path, "ownership", format);
        if format == Format::Human {
            eprintln!("\n{}", output::banner_fail("Ownership", ownership_result.errors.len()));
        }
        process::exit(1);
    }

    let mut interp = rask_interp::Interpreter::new();
    let results = interp.run_tests(&parse_result.decls, filter.as_deref());

    if results.is_empty() {
        if format == Format::Human {
            println!("{} Testing {} {}\n", "===".dimmed(), output::file_path(path), "===".dimmed());
            println!("  No tests found.");
        }
        return;
    }

    if format == Format::Human {
        println!("{} Testing {} {}\n", "===".dimmed(), output::file_path(path), "===".dimmed());

        let mut passed = 0;
        let mut failed = 0;
        let mut total_duration = std::time::Duration::ZERO;

        for result in &results {
            total_duration += result.duration;
            if result.passed {
                passed += 1;
                println!("  {} {} {}",
                    output::status_pass(),
                    result.name,
                    format!("({}ms)", result.duration.as_millis()).dimmed(),
                );
            } else {
                failed += 1;
                println!("  {} {}",
                    output::status_fail(),
                    result.name,
                );
                for err in &result.errors {
                    println!("      {}", err.red());
                }
            }
        }

        println!();
        println!("{}", output::separator(50));
        println!(
            "{} tests, {}, {} ({}ms)",
            results.len(),
            output::passed_count(passed),
            output::failed_count(failed),
            total_duration.as_millis(),
        );

        if failed > 0 {
            process::exit(1);
        }
    }
}

pub fn cmd_benchmark(path: &str, filter: Option<String>, format: Format) {
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

    // Name resolution
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

    // Type checking
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

    // Ownership checking
    let ownership_result = rask_ownership::check_ownership(&typed, &parse_result.decls);
    if !ownership_result.is_ok() {
        let diags: Vec<Diagnostic> = ownership_result.errors.iter().map(|e| e.to_diagnostic()).collect();
        show_diagnostics(&diags, &source, path, "ownership", format);
        if format == Format::Human {
            eprintln!("\n{}", output::banner_fail("Ownership", ownership_result.errors.len()));
        }
        process::exit(1);
    }

    let mut interp = rask_interp::Interpreter::new();
    let results = interp.run_benchmarks(&parse_result.decls, filter.as_deref());

    if results.is_empty() {
        if format == Format::Human {
            println!("{} Benchmarking {} {}\n", "===".dimmed(), output::file_path(path), "===".dimmed());
            println!("  No benchmarks found.");
        }
        return;
    }

    if format == Format::Human {
        println!("{} Benchmarking {} {}\n", "===".dimmed(), output::file_path(path), "===".dimmed());

        for result in &results {
            let ops_per_sec = if result.mean.as_nanos() > 0 {
                1_000_000_000 / result.mean.as_nanos()
            } else {
                0
            };
            println!("  {} ({} iterations)",
                result.name,
                result.iterations,
            );
            println!("      min: {:>10.3}us  max: {:>10.3}us",
                result.min.as_nanos() as f64 / 1000.0,
                result.max.as_nanos() as f64 / 1000.0,
            );
            println!("     mean: {:>10.3}us  median: {:>7.3}us  ({} ops/sec)",
                result.mean.as_nanos() as f64 / 1000.0,
                result.median.as_nanos() as f64 / 1000.0,
                ops_per_sec,
            );
            println!();
        }
    }
}
