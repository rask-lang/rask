// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Rask CLI - REPL and file runner.

mod output;

use colored::Colorize;
use rask_diagnostics::{formatter::DiagnosticFormatter, json, Diagnostic, ToDiagnostic};
use std::env;
use std::fs;
use std::path::Path;
use std::process;

/// Output format for diagnostics.
#[derive(Clone, Copy, PartialEq)]
enum Format {
    /// Rich terminal output with colors and underlines.
    Human,
    /// Structured JSON for IDEs and AI agents.
    Json,
}

fn show_diagnostic(source: &str, file_name: &str, diagnostic: &Diagnostic) {
    let formatter = DiagnosticFormatter::new(source).with_file_name(file_name);
    eprintln!("{}", formatter.format(diagnostic));
}

/// Show multiple diagnostics. In JSON mode, emit a single structured report.
fn show_diagnostics(
    diagnostics: &[Diagnostic],
    source: &str,
    file: &str,
    phase: &str,
    format: Format,
) {
    match format {
        Format::Human => {
            for d in diagnostics {
                show_diagnostic(source, file, d);
            }
        }
        Format::Json => {
            let report = json::to_json_report(diagnostics, source, file, phase);
            println!("{}", json::to_json_string(&report));
        }
    }
}

fn main() {
    output::init();
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        print_usage();
        return;
    }

    // Parse --format json / --json flag
    let format = if args.iter().any(|a| a == "--format=json" || a == "--json") {
        Format::Json
    } else if let Some(pos) = args.iter().position(|a| a == "--format") {
        if args.get(pos + 1).map(|s| s.as_str()) == Some("json") {
            Format::Json
        } else {
            Format::Human
        }
    } else {
        Format::Human
    };

    // Split at -- delimiter
    let delimiter_pos = args.iter().position(|a| a == "--");
    let (cli_args, prog_args) = match delimiter_pos {
        Some(pos) => {
            let cli = &args[..pos];
            let prog = &args[pos + 1..];  // Skip the "--" itself
            (cli, prog)
        }
        None => (&args[..], &[] as &[String])
    };

    // Filter out format flags for command dispatch
    let cmd_args: Vec<&str> = cli_args
        .iter()
        .enumerate()
        .filter(|(i, a)| {
            let s = a.as_str();
            if s == "--format=json" || s == "--json" {
                return false;
            }
            if s == "--format" {
                return false;
            }
            if *i > 0 && cli_args[i - 1] == "--format" {
                return false;
            }
            true
        })
        .map(|(_, a)| a.as_str())
        .collect();

    if cmd_args.len() < 2 {
        print_usage();
        return;
    }

    match cmd_args[1] {
        "lex" => {
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("lex"), output::arg("<file.rk>"));
                process::exit(1);
            }
            cmd_lex(cmd_args[2], format);
        }
        "parse" => {
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("parse"), output::arg("<file.rk>"));
                process::exit(1);
            }
            cmd_parse(cmd_args[2], format);
        }
        "resolve" => {
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("resolve"), output::arg("<file.rk>"));
                process::exit(1);
            }
            cmd_resolve(cmd_args[2], format);
        }
        "typecheck" | "check" => {
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("typecheck"), output::arg("<file.rk>"));
                process::exit(1);
            }
            cmd_typecheck(cmd_args[2], format);
        }
        "ownership" => {
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("ownership"), output::arg("<file.rk>"));
                process::exit(1);
            }
            cmd_ownership(cmd_args[2], format);
        }
        "run" => {
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("run"), output::arg("<file.rk>"));
                process::exit(1);
            }
            let program_args: Vec<String> = prog_args.iter().map(|s| s.to_string()).collect();
            cmd_run(cmd_args[2], program_args, format);
        }
        "test" => {
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {} {}", "Usage".yellow(), output::command("rask"), output::command("test"), output::arg("<file.rk>"), output::arg("[-f pattern]"));
                process::exit(1);
            }
            let filter = extract_filter(&cmd_args);
            cmd_test(cmd_args[2], filter, format);
        }
        "benchmark" | "bench" => {
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {} {}", "Usage".yellow(), output::command("rask"), output::command("benchmark"), output::arg("<file.rk>"), output::arg("[-f pattern]"));
                process::exit(1);
            }
            let filter = extract_filter(&cmd_args);
            cmd_benchmark(cmd_args[2], filter, format);
        }
        "test-specs" => {
            let path = cmd_args.get(2).copied();
            cmd_test_specs(path);
        }
        "build" => {
            let path = cmd_args.get(2).copied().unwrap_or(".");
            cmd_build(path);
        }
        "fmt" => {
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("fmt"), output::arg("<file.rk>"));
                process::exit(1);
            }
            let check_only = cmd_args.iter().any(|a| *a == "--check");
            cmd_fmt(cmd_args[2], check_only);
        }
        "explain" => {
            if cmd_args.len() < 3 {
                eprintln!("{}: missing error code argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("explain"), output::arg("<ERROR_CODE>"));
                process::exit(1);
            }
            cmd_explain(cmd_args[2]);
        }
        "help" | "--help" | "-h" => {
            print_usage();
        }
        "version" | "--version" | "-V" => {
            println!("{} {}", output::title("rask"), output::version("0.1.0"));
        }
        other => {
            if other.ends_with(".rk") {
                cmd_parse(other, format);
            } else {
                eprintln!("{}: Unknown command '{}'", output::error_label(), other);
                print_usage();
                process::exit(1);
            }
        }
    }
}

fn print_usage() {
    println!(
        "{} {} - A systems language where safety is invisible",
        output::title("Rask"),
        output::version("0.1.0")
    );
    println!();
    println!(
        "{}: {} {} {}",
        output::section_header("Usage"),
        output::command("rask"),
        output::arg("<command>"),
        output::arg("[args]")
    );
    println!();
    println!("{}", output::section_header("Commands:"));
    println!("  {} {}       Run a Rask program", output::command("run"), output::arg("<file>"));
    println!("  {} {}      Run tests in a file", output::command("test"), output::arg("<file>"));
    println!("  {} {} Run benchmarks in a file", output::command("benchmark"), output::arg("<file>"));
    println!("  {} {}       Tokenize a file and print tokens", output::command("lex"), output::arg("<file>"));
    println!("  {} {}     Parse a file and print AST", output::command("parse"), output::arg("<file>"));
    println!("  {} {}   Resolve names and print symbols", output::command("resolve"), output::arg("<file>"));
    println!("  {} {} Type check a file", output::command("typecheck"), output::arg("<file>"));
    println!("  {} {} Check ownership and borrowing rules", output::command("ownership"), output::arg("<file>"));
    println!("  {} {}       Format source files", output::command("fmt"), output::arg("<file>"));
    println!("  {} {}      Build a package", output::command("build"), output::arg("[dir]"));
    println!("  {} {} Run spec documentation tests", output::command("test-specs"), output::arg("[dir]"));
    println!("  {} {}  Explain an error code", output::command("explain"), output::arg("<code>"));
    println!("  {}             Show this help", output::command("help"));
    println!("  {}          Show version", output::command("version"));
    println!();
    println!("{}", output::section_header("Options:"));
    println!("  {}   Output diagnostics as structured JSON", output::arg("--json"));
}

fn cmd_lex(path: &str, format: Format) {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: reading {}: {}", output::error_label(), output::file_path(path), e);
            process::exit(1);
        }
    };

    let mut lexer = rask_lexer::Lexer::new(&source);
    let result = lexer.tokenize();

    if !result.errors.is_empty() {
        let diags: Vec<Diagnostic> = result.errors.iter().map(|e| e.to_diagnostic()).collect();
        show_diagnostics(&diags, &source, path, "lex", format);
    }

    if result.is_ok() {
        if format == Format::Human {
            println!("{} Tokens ({}) {}\n", "===".dimmed(), result.tokens.len(), "===".dimmed());
            for tok in &result.tokens {
                if matches!(tok.kind, rask_ast::token::TokenKind::Newline) {
                    continue;
                }
                println!("{:4}:{:<3} {:?}", tok.span.start, tok.span.end, tok.kind);
            }
            println!("\n{}", output::banner_ok(&format!("Lex: {} tokens", result.tokens.len())));
        }
    } else {
        if format == Format::Human {
            eprintln!("\n{}", output::banner_fail("Lex", result.errors.len()));
        }
        process::exit(1);
    }
}

fn cmd_parse(path: &str, format: Format) {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: reading {}: {}", output::error_label(), output::file_path(path), e);
            process::exit(1);
        }
    };

    let mut all_diags: Vec<Diagnostic> = Vec::new();

    let mut lexer = rask_lexer::Lexer::new(&source);
    let lex_result = lexer.tokenize();

    let mut last_line: Option<usize> = None;
    for error in &lex_result.errors {
        let line = get_line_number(&source, error.span.start);
        if last_line != Some(line) {
            all_diags.push(error.to_diagnostic());
            last_line = Some(line);
        }
    }

    if format == Format::Human {
        println!("{} Lexed {} tokens {}\n", "===".dimmed(), lex_result.tokens.len(), "===".dimmed());
    }

    let mut parser = rask_parser::Parser::new(lex_result.tokens);
    let parse_result = parser.parse();

    last_line = None;
    for error in &parse_result.errors {
        let line = get_line_number(&source, error.span.start);
        if last_line != Some(line) {
            all_diags.push(error.to_diagnostic());
            last_line = Some(line);
        }
    }

    if !all_diags.is_empty() {
        show_diagnostics(&all_diags, &source, path, "parse", format);
        if format == Format::Human {
            eprintln!("\n{}", output::banner_fail("Parse", all_diags.len()));
        }
        process::exit(1);
    }

    if format == Format::Human {
        println!("{} AST ({} declarations) {}\n", "===".dimmed(), parse_result.decls.len(), "===".dimmed());
        for (i, decl) in parse_result.decls.iter().enumerate() {
            println!("--- Declaration {} ---", i + 1);
            println!("{:#?}", decl);
            println!();
        }
        println!("{}", output::banner_ok("Parse"));
    }
}

fn cmd_resolve(path: &str, format: Format) {
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
    let parse_result = parser.parse();

    if !parse_result.is_ok() {
        let diags: Vec<Diagnostic> = parse_result.errors.iter().map(|e| e.to_diagnostic()).collect();
        show_diagnostics(&diags, &source, path, "parse", format);
        if format == Format::Human {
            eprintln!("\n{}", output::banner_fail("Parse", parse_result.errors.len()));
        }
        process::exit(1);
    }

    match rask_resolve::resolve(&parse_result.decls) {
        Ok(resolved) => {
            if format == Format::Human {
                println!("{} Symbols ({}) {}\n", "===".dimmed(), resolved.symbols.iter().count(), "===".dimmed());
                for symbol in resolved.symbols.iter() {
                    println!("{:4} {} ({:?})", symbol.id.0, symbol.name, symbol.kind);
                }
                println!("\n{} Resolutions ({}) {}\n", "===".dimmed(), resolved.resolutions.len(), "===".dimmed());
                for (node_id, sym_id) in &resolved.resolutions {
                    if let Some(sym) = resolved.symbols.get(*sym_id) {
                        println!("  NodeId({}) -> {} (SymbolId {})", node_id.0, sym.name, sym_id.0);
                    }
                }
                println!("\n{}", output::banner_ok("Resolve"));
            }
        }
        Err(errors) => {
            let diags: Vec<Diagnostic> = errors.iter().map(|e| e.to_diagnostic()).collect();
            show_diagnostics(&diags, &source, path, "resolve", format);
            if format == Format::Human {
                eprintln!("\n{}", output::banner_fail("Resolve", errors.len()));
            }
            process::exit(1);
        }
    }
}

fn cmd_typecheck(path: &str, format: Format) {
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

fn cmd_ownership(path: &str, format: Format) {
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

fn cmd_run(path: &str, program_args: Vec<String>, format: Format) {
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

fn extract_filter(args: &[&str]) -> Option<String> {
    if let Some(pos) = args.iter().position(|a| *a == "-f") {
        args.get(pos + 1).map(|s| s.to_string())
    } else {
        None
    }
}

fn cmd_test(path: &str, filter: Option<String>, format: Format) {
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

fn cmd_benchmark(path: &str, filter: Option<String>, format: Format) {
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

fn cmd_build(path: &str) {
    use rask_resolve::PackageRegistry;

    let root = Path::new(path);
    if !root.exists() {
        eprintln!("{}: directory not found: {}", output::error_label(), output::file_path(path));
        process::exit(1);
    }

    if !root.is_dir() {
        eprintln!("{}: not a directory: {}", output::error_label(), output::file_path(path));
        eprintln!("{}: {} {} {} for single files", "hint".cyan(), output::command("rask"), output::command("typecheck"), output::arg("<file>"));
        process::exit(1);
    }

    println!("{} Discovering packages in {} {}\n", "===".dimmed(), output::file_path(path), "===".dimmed());

    let mut registry = PackageRegistry::new();
    match registry.discover(root) {
        Ok(_root_id) => {
            println!("Discovered {} package(s):\n", registry.len());

            for pkg in registry.packages() {
                let file_count = pkg.files.len();
                let decl_count: usize = pkg.files.iter().map(|f| f.decls.len()).sum();
                println!(
                    "  {} ({} file{}, {} declaration{})",
                    pkg.path_string(),
                    file_count,
                    if file_count == 1 { "" } else { "s" },
                    decl_count,
                    if decl_count == 1 { "" } else { "s" }
                );

                for file in &pkg.files {
                    println!("    - {}", file.path.display());
                }
            }
            println!();

            let mut total_errors = 0;
            for pkg in registry.packages() {
                println!("{} Compiling package: {} {}", "===".dimmed(), pkg.path_string().green(), "===".dimmed());

                let mut all_decls: Vec<_> = pkg.all_decls().cloned().collect();
                rask_desugar::desugar(&mut all_decls);

                match rask_resolve::resolve_package(&all_decls, &registry, pkg.id) {
                    Ok(resolved) => {
                        match rask_types::typecheck(resolved, &all_decls) {
                            Ok(typed) => {
                                let ownership_result = rask_ownership::check_ownership(&typed, &all_decls);
                                if !ownership_result.is_ok() {
                                    for error in &ownership_result.errors {
                                        eprintln!("error: {}", error.kind);
                                    }
                                    total_errors += ownership_result.errors.len();
                                }
                            }
                            Err(errors) => {
                                for error in &errors {
                                    eprintln!("type error: {}", error);
                                }
                                total_errors += errors.len();
                            }
                        }
                    }
                    Err(errors) => {
                        for error in &errors {
                            eprintln!("resolve error: {}", error.kind);
                        }
                        total_errors += errors.len();
                    }
                }
            }

            println!();
            if total_errors == 0 {
                println!("{}", output::banner_ok(&format!("Build: {} package(s) compiled", registry.len())));
            } else {
                eprintln!("{}", output::banner_fail("Build", total_errors));
                process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("{}: {}", output::error_label(), e);
            process::exit(1);
        }
    }
}

fn cmd_fmt(path: &str, check_only: bool) {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: reading {}: {}", output::error_label(), output::file_path(path), e);
            process::exit(1);
        }
    };

    let formatted = rask_fmt::format_source(&source);

    if formatted == source {
        if check_only {
            println!("{} {}", output::status_pass(), output::file_path(path));
        }
        return;
    }

    if check_only {
        println!("{} {} (would reformat)", output::status_fail(), output::file_path(path));
        process::exit(1);
    }

    match fs::write(path, &formatted) {
        Ok(_) => {
            println!("Formatted {}", output::file_path(path));
        }
        Err(e) => {
            eprintln!("{}: writing {}: {}", output::error_label(), output::file_path(path), e);
            process::exit(1);
        }
    }
}

/// Get the line number for a byte offset.
fn get_line_number(source: &str, pos: usize) -> usize {
    source[..pos.min(source.len())].chars().filter(|&c| c == '\n').count() + 1
}

fn cmd_explain(code: &str) {
    use rask_diagnostics::codes::ErrorCodeRegistry;

    let registry = ErrorCodeRegistry::default();

    if let Some(info) = registry.get(code) {
        println!(
            "{}[{}]: {}",
            "error".red().bold(),
            info.code.red().bold(),
            info.title.bold()
        );
        println!();
        println!("  Category: {}", info.category);
        println!();
        println!("  Detailed explanation not yet available.");
        println!("  Run `rask typecheck <file>` to see this error in context.");
    } else {
        eprintln!(
            "{}: unknown error code `{}`",
            output::error_label(),
            code
        );
        eprintln!();
        eprintln!("Error codes use the format E0NNN (e.g., E0308, E0800).");
        eprintln!("Run `rask help` for available commands.");
        process::exit(1);
    }
}

fn cmd_test_specs(path: Option<&str>) {
    use rask_spec_test::{extract_tests, run_test, TestSummary};

    let specs_dir = path.unwrap_or("specs");
    let specs_path = Path::new(specs_dir);

    if !specs_path.exists() {
        eprintln!("{}: specs directory not found: {}", output::error_label(), output::file_path(specs_dir));
        process::exit(1);
    }

    let mut summary = TestSummary::default();
    let mut all_results = Vec::new();

    let md_files = collect_md_files(specs_path);
    summary.files = md_files.len();

    for md_path in &md_files {
        let content = match fs::read_to_string(md_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{}: reading {}: {}", output::error_label(), output::file_path(&md_path.display().to_string()), e);
                continue;
            }
        };

        let tests = extract_tests(&md_path, &content);
        if tests.is_empty() {
            continue;
        }

        println!("{}", output::file_path(&md_path.display().to_string()));

        for test in tests {
            let result = run_test(test);
            summary.add(&result);

            let status = if result.passed {
                output::status_pass()
            } else {
                output::status_fail()
            };
            println!(
                "  {} line {}: {:?} - {}",
                status,
                result.test.line.to_string().dimmed(),
                result.test.expectation,
                result.message
            );

            if !result.passed {
                all_results.push(result);
            }
        }
        println!();
    }

    println!("{}", output::separator(50));
    println!(
        "{} files, {} tests, {}, {}",
        summary.files,
        summary.total,
        output::passed_count(summary.passed),
        output::failed_count(summary.failed)
    );

    if summary.failed > 0 {
        println!("\n{}", "Failed tests:".red().bold());
        for result in &all_results {
            println!(
                "  {} {}:{} - {}",
                output::status_fail(),
                output::file_path(&result.test.path.display().to_string()),
                result.test.line,
                result.message
            );
        }
        process::exit(1);
    }
}

/// Recursively collect all .md files in a directory.
fn collect_md_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();

    if dir.is_file() && dir.extension().map(|e| e == "md").unwrap_or(false) {
        files.push(dir.to_path_buf());
        return files;
    }

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(collect_md_files(&path));
            } else if path.extension().map(|e| e == "md").unwrap_or(false) {
                files.push(path);
            }
        }
    }

    files.sort();
    files
}

/// Validate entry points: needs exactly one @entry or a func main().
#[allow(dead_code)]
fn validate_entry_points(decls: &[rask_ast::decl::Decl]) -> Result<(), String> {
    use rask_ast::decl::DeclKind;

    let mut entry_count = 0;
    let mut entry_names = Vec::new();
    let mut has_main = false;

    for decl in decls {
        if let DeclKind::Fn(f) = &decl.kind {
            if f.attrs.iter().any(|a| a == "entry") {
                entry_count += 1;
                entry_names.push(f.name.clone());
            }
            if f.name == "main" {
                has_main = true;
            }
        }
    }

    match entry_count {
        0 if has_main => Ok(()),
        0 => Err("no entry point found (add func main() or use @entry)".to_string()),
        1 => Ok(()),
        _ => Err(format!(
            "multiple @entry functions found: {} (only one allowed per program)",
            entry_names.join(", ")
        )),
    }
}
