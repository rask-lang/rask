// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Rask CLI - REPL and file runner.

mod help;
mod output;

use colored::Colorize;
use rask_diagnostics::{formatter::DiagnosticFormatter, json, Diagnostic, ToDiagnostic};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
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
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                print_lex_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("lex"), output::arg("<file.rk>"));
                process::exit(1);
            }
            cmd_lex(cmd_args[2], format);
        }
        "parse" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                print_parse_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("parse"), output::arg("<file.rk>"));
                process::exit(1);
            }
            cmd_parse(cmd_args[2], format);
        }
        "resolve" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                print_resolve_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("resolve"), output::arg("<file.rk>"));
                process::exit(1);
            }
            cmd_resolve(cmd_args[2], format);
        }
        "typecheck" | "check" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                print_typecheck_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("typecheck"), output::arg("<file.rk>"));
                process::exit(1);
            }
            cmd_typecheck(cmd_args[2], format);
        }
        "ownership" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                print_ownership_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("ownership"), output::arg("<file.rk>"));
                process::exit(1);
            }
            cmd_ownership(cmd_args[2], format);
        }
        "comptime" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                print_comptime_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("comptime"), output::arg("<file.rk>"));
                process::exit(1);
            }
            cmd_comptime(cmd_args[2], format);
        }
        "run" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                print_run_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("run"), output::arg("<file.rk>"));
                process::exit(1);
            }
            let program_args: Vec<String> = prog_args.iter().map(|s| s.to_string()).collect();
            cmd_run(cmd_args[2], program_args, format);
        }
        "test" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                print_test_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {} {}", "Usage".yellow(), output::command("rask"), output::command("test"), output::arg("<file.rk>"), output::arg("[-f pattern]"));
                process::exit(1);
            }
            let filter = extract_filter(&cmd_args);
            cmd_test(cmd_args[2], filter, format);
        }
        "benchmark" | "bench" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                print_benchmark_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {} {}", "Usage".yellow(), output::command("rask"), output::command("benchmark"), output::arg("<file.rk>"), output::arg("[-f pattern]"));
                process::exit(1);
            }
            let filter = extract_filter(&cmd_args);
            cmd_benchmark(cmd_args[2], filter, format);
        }
        "test-specs" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                print_test_specs_help();
                return;
            }
            let path = cmd_args.get(2).copied();
            cmd_test_specs(path);
        }
        "build" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                print_build_help();
                return;
            }
            let path = cmd_args.get(2).copied().unwrap_or(".");
            cmd_build(path);
        }
        "fmt" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                print_fmt_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("fmt"), output::arg("<file.rk>"));
                process::exit(1);
            }
            let check_only = cmd_args.iter().any(|a| *a == "--check");
            cmd_fmt(cmd_args[2], check_only);
        }
        "describe" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                print_describe_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("describe"), output::arg("<file.rk>"));
                process::exit(1);
            }
            let show_all = cmd_args.iter().any(|a| *a == "--all");
            cmd_describe(cmd_args[2], format, show_all);
        }
        "lint" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                print_lint_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file or directory argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("lint"), output::arg("<file.rk | dir>"));
                process::exit(1);
            }
            let rules = extract_repeated_flag(&cmd_args, "--rule");
            let excludes = extract_repeated_flag(&cmd_args, "--exclude");
            cmd_lint(cmd_args[2], format, rules, excludes);
        }
        "explain" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                print_explain_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing error code argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("explain"), output::arg("<ERROR_CODE>"));
                process::exit(1);
            }
            cmd_explain(cmd_args[2]);
        }
        "help" | "--help" | "-h" => {
            if cmd_args.len() > 2 && (cmd_args[2] == "--help" || cmd_args[2] == "-h") {
                print_help_help();
                return;
            }
            print_usage();
        }
        "version" | "--version" | "-V" => {
            println!("{} {}", output::title("rask"), output::version("0.1.0"));
        }
        other => {
            eprintln!("{}: Unknown command '{}'", output::error_label(), other);
            print_usage();
            process::exit(1);
        }
    }
}

fn print_usage() {
    println!(
        "{} {} - Safety and performance without the pain",
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
    println!("{}", output::section_header("Common:"));
    println!("  {} {}       Run a Rask program", output::command("run"), output::arg("<file>"));
    println!("  {} {}      Build a package", output::command("build"), output::arg("[dir]"));
    println!("  {} {}       Format source files", output::command("fmt"), output::arg("<file>"));
    println!("  {} {}   Explain an error code", output::command("explain"), output::arg("<code>"));
    println!("  {}             Show this help", output::command("help"));
    println!("  {}          Show version", output::command("version"));
    
    println!();
    println!("{}", output::section_header("Testing:"));
    println!("  {} {}      Run tests in a file", output::command("test"), output::arg("<file>"));
    println!("  {} {} Run benchmarks in a file", output::command("benchmark"), output::arg("<file>"));
    println!("  {} {} Run spec documentation tests", output::command("test-specs"), output::arg("[dir]"));
    
    println!();
    println!("{}", output::section_header("Debugging and Exploration:"));
    println!("  {} {}  Lint source files for conventions", output::command("lint"), output::arg("<file|dir>"));
    println!("  {} {}  Describe a module's public API", output::command("describe"), output::arg("<file>"));

    println!();
    println!("{}", output::section_header("Compilation Phases:"));
    println!("  {} {}       Tokenize a file and print tokens", output::command("lex"), output::arg("<file>"));
    println!("  {} {}     Parse a file and print AST", output::command("parse"), output::arg("<file>"));
    println!("  {} {}   Resolve names and print symbols", output::command("resolve"), output::arg("<file>"));
    println!("  {} {} Type check a file", output::command("typecheck"), output::arg("<file>"));
    println!("  {} {} Check ownership and borrowing rules", output::command("ownership"), output::arg("<file>"));
    println!("  {} {}  Evaluate comptime blocks", output::command("comptime"), output::arg("<file>"));
 
    
    println!();
    println!("{}", output::section_header("Options:"));
    println!("  {}   Output diagnostics as structured JSON", output::arg("--json"));
}

fn print_help_help() {
    println!("{}", output::section_header("Help"));
    println!();
    println!("Display help information about Rask commands.");
    println!();
    println!("{}: {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("help"));
    println!();
    println!("Shows the main help screen with all available commands.");
    println!();
    println!("{}", output::section_header("Getting Help for Specific Commands:"));
    println!("  {} {} {}  Show help for a specific command",
        output::command("rask"),
        output::arg("<command>"),
        output::arg("--help"));
    println!();
    println!("{}", output::section_header("Examples:"));
    println!("  {} {}            Show main help",
        output::command("rask"),
        output::command("help"));
    println!("  {} {} {}      Show help for lint command",
        output::command("rask"),
        output::command("lint"),
        output::arg("--help"));
    println!("  {} {} {}  Show help for check command",
        output::command("rask"),
        output::command("check"),
        output::arg("-h"));
}

fn print_run_help() {
    println!("{}", output::section_header("Run"));
    println!();
    println!("Execute a Rask program.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("run"),
        output::arg("<file.rk> [-- <program args>]"));
    println!();
    println!("{}", output::section_header("Options:"));
    println!("  {}        Output diagnostics as structured JSON", output::arg("--json"));
    println!("  {}             Pass arguments to the program (after --))", output::arg("--"));
    println!();
    println!("{}", output::section_header("Examples:"));
    println!("  {} {} {}              Run a program",
        output::command("rask"),
        output::command("run"),
        output::arg("main.rk"));
    println!("  {} {} {} {} {}   Pass args to program",
        output::command("rask"),
        output::command("run"),
        output::arg("main.rk"),
        output::arg("--"),
        output::arg("arg1 arg2"));
}

fn print_build_help() {
    println!("{}", output::section_header("Build"));
    println!();
    println!("Build a Rask package. Discovers all .rk files in the directory,");
    println!("resolves imports, and runs type checking and ownership analysis.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("build"),
        output::arg("[directory]"));
    println!();
    println!("If no directory is specified, builds the current directory.");
}

fn print_test_help() {
    println!("{}", output::section_header("Test"));
    println!();
    println!("Run test functions (functions with @test attribute) in a file.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("test"),
        output::arg("<file.rk> [-f <pattern>]"));
    println!();
    println!("{}", output::section_header("Options:"));
    println!("  {}       Output as structured JSON", output::arg("--json"));
    println!("  {} {} Filter tests by name pattern", output::arg("-f"), output::arg("<pattern>"));
    println!();
    println!("{}", output::section_header("Examples:"));
    println!("  {} {} {}        Run all tests",
        output::command("rask"),
        output::command("test"),
        output::arg("tests.rk"));
    println!("  {} {} {} {} {}  Run tests matching pattern",
        output::command("rask"),
        output::command("test"),
        output::arg("tests.rk"),
        output::arg("-f"),
        output::arg("parse"));
}

fn print_benchmark_help() {
    println!("{}", output::section_header("Benchmark"));
    println!();
    println!("Run benchmark functions (functions with @benchmark attribute) in a file.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("benchmark"),
        output::arg("<file.rk> [-f <pattern>]"));
    println!();
    println!("{}", output::section_header("Options:"));
    println!("  {}       Output as structured JSON", output::arg("--json"));
    println!("  {} {} Filter benchmarks by name pattern", output::arg("-f"), output::arg("<pattern>"));
    println!();
    println!("{}", output::section_header("Examples:"));
    println!("  {} {} {}   Run all benchmarks",
        output::command("rask"),
        output::command("benchmark"),
        output::arg("bench.rk"));
    println!("  {} {} {} {} {}  Run benchmarks matching pattern",
        output::command("rask"),
        output::command("benchmark"),
        output::arg("bench.rk"),
        output::arg("-f"),
        output::arg("sort"));
}

fn print_test_specs_help() {
    println!("{}", output::section_header("Test Specs"));
    println!();
    println!("Validate code examples in spec documentation. Runs parser on all");
    println!("code blocks and checks for staleness based on git commit dates.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("test-specs"),
        output::arg("[directory]"));
    println!();
    println!("If no directory is specified, tests the 'specs' directory.");
}

fn print_fmt_help() {
    println!("{}", output::section_header("Format"));
    println!();
    println!("Format a Rask source file according to standard style.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("fmt"),
        output::arg("<file.rk> [--check]"));
    println!();
    println!("{}", output::section_header("Options:"));
    println!("  {}  Check if file is formatted without modifying", output::arg("--check"));
    println!();
    println!("{}", output::section_header("Examples:"));
    println!("  {} {} {}          Format a file",
        output::command("rask"),
        output::command("fmt"),
        output::arg("main.rk"));
    println!("  {} {} {} {}  Check formatting",
        output::command("rask"),
        output::command("fmt"),
        output::arg("main.rk"),
        output::arg("--check"));
}

fn print_describe_help() {
    println!("{}", output::section_header("Describe"));
    println!();
    println!("Show a module's public API including structs, functions, and enums.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("describe"),
        output::arg("<file.rk> [--all]"));
    println!();
    println!("{}", output::section_header("Options:"));
    println!("  {}     Show all items including private ones", output::arg("--all"));
    println!("  {}   Output as structured JSON", output::arg("--json"));
    println!();
    println!("{}", output::section_header("Examples:"));
    println!("  {} {} {}        Show public API",
        output::command("rask"),
        output::command("describe"),
        output::arg("module.rk"));
    println!("  {} {} {} {}  Show all items",
        output::command("rask"),
        output::command("describe"),
        output::arg("module.rk"),
        output::arg("--all"));
}

fn print_lint_help() {
    println!("{}", output::section_header("Lint"));
    println!();
    println!("Check Rask code for naming conventions, style issues, and idiom violations.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("lint"),
        output::arg("<file.rk | directory>"));
    println!();
    println!("{}", output::section_header("Options:"));
    println!("  {}           Output as structured JSON", output::arg("--json"));
    println!("  {} {}     Run specific lint rule(s)", output::arg("--rule"), output::arg("<pattern>"));
    println!("  {} {} Exclude specific rule(s)", output::arg("--exclude"), output::arg("<pattern>"));
    println!();
    println!("{}", output::section_header("Examples:"));
    println!("  {} {} {}           Lint a file",
        output::command("rask"),
        output::command("lint"),
        output::arg("main.rk"));
    println!("  {} {} {}            Lint all files in directory",
        output::command("rask"),
        output::command("lint"),
        output::arg("src/"));
    println!("  {} {} {} {} {}  Run only naming rules",
        output::command("rask"),
        output::command("lint"),
        output::arg("main.rk"),
        output::arg("--rule"),
        output::arg("naming/*"));
}

fn print_explain_help() {
    println!("{}", output::section_header("Explain"));
    println!();
    println!("Display detailed information about a compiler error code.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("explain"),
        output::arg("<ERROR_CODE>"));
    println!();
    println!("{}", output::section_header("Examples:"));
    println!("  {} {} {}     Explain error E0308",
        output::command("rask"),
        output::command("explain"),
        output::arg("E0308"));
}

fn print_lex_help() {
    println!("{}", output::section_header("Lex"));
    println!();
    println!("Tokenize a Rask source file and display the token stream.");
    println!("First phase of compilation - converts source text into tokens.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("lex"),
        output::arg("<file.rk>"));
    println!();
    println!("{}", output::section_header("Options:"));
    println!("  {}  Output tokens as structured JSON", output::arg("--json"));
}

fn print_parse_help() {
    println!("{}", output::section_header("Parse"));
    println!();
    println!("Parse a Rask source file and display the abstract syntax tree.");
    println!("Second phase of compilation - builds AST from tokens.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("parse"),
        output::arg("<file.rk>"));
    println!();
    println!("{}", output::section_header("Options:"));
    println!("  {}  Output AST as structured JSON", output::arg("--json"));
}

fn print_resolve_help() {
    println!("{}", output::section_header("Resolve"));
    println!();
    println!("Resolve names and build symbol table for a Rask source file.");
    println!("Third phase of compilation - links names to declarations.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("resolve"),
        output::arg("<file.rk>"));
    println!();
    println!("{}", output::section_header("Options:"));
    println!("  {}  Output symbols as structured JSON", output::arg("--json"));
}

fn print_typecheck_help() {
    println!("{}", output::section_header("Typecheck"));
    println!();
    println!("Type check a Rask source file and validate type correctness.");
    println!("Fourth phase of compilation - ensures type safety.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("typecheck"),
        output::arg("<file.rk>"));
    println!();
    println!("Alias: {} {} {}",
        output::command("rask"),
        output::command("check"),
        output::arg("<file.rk>"));
    println!();
    println!("{}", output::section_header("Options:"));
    println!("  {}  Output type information as structured JSON", output::arg("--json"));
}

fn print_ownership_help() {
    println!("{}", output::section_header("Ownership"));
    println!();
    println!("Check ownership and borrowing rules for a Rask source file.");
    println!("Fifth phase of compilation - validates memory safety.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("ownership"),
        output::arg("<file.rk>"));
    println!();
    println!("{}", output::section_header("Options:"));
    println!("  {}  Output ownership errors as structured JSON", output::arg("--json"));
}

fn print_comptime_help() {
    println!("{}", output::section_header("Comptime"));
    println!();
    println!("Evaluate compile-time blocks in a Rask source file.");
    println!("Runs comptime blocks and displays their results.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("comptime"),
        output::arg("<file.rk>"));
    println!();
    println!("{}", output::section_header("Options:"));
    println!("  {}  Output comptime results as structured JSON", output::arg("--json"));
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

fn cmd_comptime(path: &str, format: Format) {
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

fn cmd_describe(path: &str, format: Format, show_all: bool) {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: reading {}: {}", output::error_label(), output::file_path(path), e);
            process::exit(1);
        }
    };

    let opts = rask_describe::DescribeOpts { show_all };
    let desc = rask_describe::describe(&source, path, opts);

    match format {
        Format::Human => print!("{}", rask_describe::describe_text(&desc)),
        Format::Json => println!("{}", rask_describe::describe_json(&desc)),
    }
}

fn cmd_lint(path: &str, format: Format, rules: Vec<String>, excludes: Vec<String>) {
    let p = Path::new(path);
    let files: Vec<String> = if p.is_dir() {
        collect_rk_files(p)
    } else {
        vec![path.to_string()]
    };

    if files.is_empty() {
        eprintln!("{}: no .rk files found in {}", output::error_label(), output::file_path(path));
        process::exit(1);
    }

    let mut total_errors = 0;
    let mut total_warnings = 0;

    for file in &files {
        let source = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{}: reading {}: {}", output::error_label(), output::file_path(file), e);
                continue;
            }
        };

        let opts = rask_lint::LintOpts {
            rules: rules.clone(),
            excludes: excludes.clone(),
        };
        let report = rask_lint::lint(&source, file, opts);

        total_errors += report.error_count;
        total_warnings += report.warning_count;

        match format {
            Format::Human => {
                for d in &report.diagnostics {
                    let severity_str = match d.severity {
                        rask_lint::Severity::Error => "error".red().bold().to_string(),
                        rask_lint::Severity::Warning => "warning".yellow().bold().to_string(),
                    };
                    eprintln!(
                        "{}[{}]: {}",
                        severity_str,
                        d.rule.dimmed(),
                        d.message
                    );
                    eprintln!(
                        "  {} {}:{}",
                        "-->".cyan(),
                        output::file_path(file),
                        d.location.line
                    );
                    eprintln!("   |");
                    eprintln!(
                        " {} | {}",
                        d.location.line,
                        d.location.source_line
                    );
                    eprintln!("   |");
                    eprintln!("   = {}: {}", "fix".green().bold(), d.fix);
                    eprintln!();
                }
            }
            Format::Json => {
                println!("{}", rask_lint::lint_json(&report));
            }
        }
    }

    if format == Format::Human {
        if total_errors == 0 && total_warnings == 0 {
            println!("{} No lint issues found", output::status_pass());
        } else {
            eprintln!(
                "{} {} error(s), {} warning(s)",
                output::status_fail(),
                total_errors,
                total_warnings
            );
        }
    }

    if total_errors > 0 {
        process::exit(1);
    }
}

/// Collect all .rk files in a directory recursively.
fn collect_rk_files(dir: &Path) -> Vec<String> {
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(collect_rk_files(&path));
            } else if path.extension().map(|e| e == "rk").unwrap_or(false) {
                if let Some(s) = path.to_str() {
                    files.push(s.to_string());
                }
            }
        }
    }
    files.sort();
    files
}

/// Extract repeated flag values (e.g., --rule naming/* --rule style/*)
fn extract_repeated_flag(args: &[&str], flag: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == flag {
            if i + 1 < args.len() {
                values.push(args[i + 1].to_string());
                i += 2;
                continue;
            }
        }
        i += 1;
    }
    values
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
        // Description
        for line in info.description.lines() {
            println!("  {}", line);
        }
        println!();
        // Example
        if !info.example.is_empty() {
            println!("  {}:", "Example".bold());
            println!();
            for line in info.example.lines() {
                println!("    {}", line);
            }
            println!();
        }
        println!("  Run `rask check <file>` to see this error in context.");
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
    use rask_spec_test::{extract_tests, run_test, extract_deps, check_staleness, TestSummary};

    let specs_dir = path.unwrap_or("specs");
    let specs_path = Path::new(specs_dir);

    if !specs_path.exists() {
        eprintln!("{}: specs directory not found: {}", output::error_label(), output::file_path(specs_dir));
        process::exit(1);
    }

    let mut summary = TestSummary::default();
    let mut all_results = Vec::new();
    let mut all_deps = Vec::new();

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

        // Collect dependency headers
        let deps = extract_deps(md_path, &content);
        if !deps.depends.is_empty() || !deps.implemented_by.is_empty() {
            all_deps.push(deps);
        }

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
    }

    // Staleness check — find project root (where .git lives)
    if !all_deps.is_empty() {
        let project_root = find_project_root(specs_path);
        if let Some(root) = project_root {
            let warnings = check_staleness(&all_deps, &root);
            if !warnings.is_empty() {
                println!("\n{}", "Staleness warnings:".yellow().bold());
                for w in &warnings {
                    println!(
                        "  {} {} may be stale",
                        "!".yellow().bold(),
                        output::file_path(&w.spec.display().to_string()),
                    );
                    println!(
                        "    {} {} (modified more recently: {})",
                        w.direction,
                        output::file_path(&w.dependency),
                        w.dep_commit.dimmed(),
                    );
                }
            }
        }
    }

    if summary.failed > 0 {
        process::exit(1);
    }
}

/// Walk up from a path to find the project root (directory containing .git).
fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };
    loop {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
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
