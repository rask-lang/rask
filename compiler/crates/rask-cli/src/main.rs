//! Rask CLI - REPL and file runner.

use std::env;
use std::fs;
use std::path::Path;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        print_usage();
        return;
    }

    match args[1].as_str() {
        "lex" => {
            if args.len() < 3 {
                eprintln!("Usage: rask lex <file.rask>");
                process::exit(1);
            }
            cmd_lex(&args[2]);
        }
        "parse" => {
            if args.len() < 3 {
                eprintln!("Usage: rask parse <file.rask>");
                process::exit(1);
            }
            cmd_parse(&args[2]);
        }
        "resolve" => {
            if args.len() < 3 {
                eprintln!("Usage: rask resolve <file.rask>");
                process::exit(1);
            }
            cmd_resolve(&args[2]);
        }
        "typecheck" | "check" => {
            if args.len() < 3 {
                eprintln!("Usage: rask typecheck <file.rask>");
                process::exit(1);
            }
            cmd_typecheck(&args[2]);
        }
        "test-specs" => {
            let path = args.get(2).map(|s| s.as_str());
            cmd_test_specs(path);
        }
        "help" | "--help" | "-h" => {
            print_usage();
        }
        "version" | "--version" | "-V" => {
            println!("rask 0.1.0");
        }
        other => {
            // Treat as filename
            if other.ends_with(".rask") {
                cmd_parse(other);
            } else {
                eprintln!("Unknown command: {}", other);
                print_usage();
                process::exit(1);
            }
        }
    }
}

fn print_usage() {
    println!("Rask 0.1.0 - A systems language where safety is invisible");
    println!();
    println!("Usage: rask <command> [args]");
    println!();
    println!("Commands:");
    println!("  lex <file>       Tokenize a file and print tokens");
    println!("  parse <file>     Parse a file and print AST");
    println!("  resolve <file>   Resolve names and print symbols");
    println!("  typecheck <file> Type check a file and show inferred types");
    println!("  test-specs [dir] Run spec documentation tests");
    println!("  help             Show this help");
    println!("  version          Show version");
}

fn cmd_lex(path: &str) {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading {}: {}", path, e);
            process::exit(1);
        }
    };

    let mut lexer = rask_lexer::Lexer::new(&source);
    let result = lexer.tokenize();

    // Show any errors
    for error in &result.errors {
        show_error(&source, error.span.start, &error.message, error.hint.as_deref());
    }

    if result.is_ok() {
        println!("=== Tokens ({}) ===\n", result.tokens.len());
        for tok in &result.tokens {
            // Skip newlines for cleaner output (optional)
            if matches!(tok.kind, rask_ast::token::TokenKind::Newline) {
                continue;
            }
            println!("{:4}:{:<3} {:?}", tok.span.start, tok.span.end, tok.kind);
        }
        println!("\n=== Lex OK: {} tokens ===", result.tokens.len());
    } else {
        eprintln!("\n=== Lex FAILED: {} error(s) ===", result.errors.len());
        process::exit(1);
    }
}

fn cmd_parse(path: &str) {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading {}: {}", path, e);
            process::exit(1);
        }
    };

    let mut has_errors = false;
    let mut error_count = 0;

    // First lex
    let mut lexer = rask_lexer::Lexer::new(&source);
    let lex_result = lexer.tokenize();

    // Show lex errors (deduplicated - one per line)
    let mut last_line: Option<usize> = None;
    for error in &lex_result.errors {
        let line = get_line_number(&source, error.span.start);
        if last_line != Some(line) {
            show_error(&source, error.span.start, &error.message, error.hint.as_deref());
            error_count += 1;
            has_errors = true;
            last_line = Some(line);
        }
    }

    println!("=== Lexed {} tokens ===\n", lex_result.tokens.len());

    // Then parse
    let mut parser = rask_parser::Parser::new(lex_result.tokens);
    let parse_result = parser.parse();

    // Show parse errors (deduplicated - one per line)
    last_line = None;
    for error in &parse_result.errors {
        let line = get_line_number(&source, error.span.start);
        if last_line != Some(line) {
            show_error(&source, error.span.start, &error.message, error.hint.as_deref());
            error_count += 1;
            has_errors = true;
            last_line = Some(line);
        }
    }

    if has_errors {
        eprintln!("\n=== FAILED: {} error(s) ===", error_count);
        process::exit(1);
    }

    println!("=== AST ({} declarations) ===\n", parse_result.decls.len());
    for (i, decl) in parse_result.decls.iter().enumerate() {
        println!("--- Declaration {} ---", i + 1);
        println!("{:#?}", decl);
        println!();
    }
    println!("=== Parse OK ===");
}

fn cmd_resolve(path: &str) {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading {}: {}", path, e);
            process::exit(1);
        }
    };

    // Lex
    let mut lexer = rask_lexer::Lexer::new(&source);
    let lex_result = lexer.tokenize();

    if !lex_result.is_ok() {
        for error in &lex_result.errors {
            show_error(&source, error.span.start, &error.message, error.hint.as_deref());
        }
        eprintln!("\n=== Lex FAILED ===");
        process::exit(1);
    }

    // Parse
    let mut parser = rask_parser::Parser::new(lex_result.tokens);
    let parse_result = parser.parse();

    if !parse_result.is_ok() {
        for error in &parse_result.errors {
            show_error(&source, error.span.start, &error.message, error.hint.as_deref());
        }
        eprintln!("\n=== Parse FAILED ===");
        process::exit(1);
    }

    // Resolve
    match rask_resolve::resolve(&parse_result.decls) {
        Ok(resolved) => {
            println!("=== Symbols ({}) ===\n", resolved.symbols.iter().count());
            for symbol in resolved.symbols.iter() {
                println!("{:4} {} ({:?})", symbol.id.0, symbol.name, symbol.kind);
            }
            println!("\n=== Resolutions ({}) ===\n", resolved.resolutions.len());
            for (node_id, sym_id) in &resolved.resolutions {
                if let Some(sym) = resolved.symbols.get(*sym_id) {
                    println!("  NodeId({}) -> {} (SymbolId {})", node_id.0, sym.name, sym_id.0);
                }
            }
            println!("\n=== Resolve OK ===");
        }
        Err(errors) => {
            for error in &errors {
                show_error(&source, error.span.start, &format!("{}", error.kind), None);
            }
            eprintln!("\n=== Resolve FAILED: {} error(s) ===", errors.len());
            process::exit(1);
        }
    }
}

fn cmd_typecheck(path: &str) {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading {}: {}", path, e);
            process::exit(1);
        }
    };

    // Lex
    let mut lexer = rask_lexer::Lexer::new(&source);
    let lex_result = lexer.tokenize();

    if !lex_result.is_ok() {
        for error in &lex_result.errors {
            show_error(&source, error.span.start, &error.message, error.hint.as_deref());
        }
        eprintln!("\n=== Lex FAILED ===");
        process::exit(1);
    }

    // Parse
    let mut parser = rask_parser::Parser::new(lex_result.tokens);
    let mut parse_result = parser.parse();

    if !parse_result.is_ok() {
        for error in &parse_result.errors {
            show_error(&source, error.span.start, &error.message, error.hint.as_deref());
        }
        eprintln!("\n=== Parse FAILED ===");
        process::exit(1);
    }

    // Desugar operators (a + b → a.add(b), etc.)
    rask_desugar::desugar(&mut parse_result.decls);

    // Resolve
    let resolved = match rask_resolve::resolve(&parse_result.decls) {
        Ok(r) => r,
        Err(errors) => {
            for error in &errors {
                show_error(&source, error.span.start, &format!("{}", error.kind), None);
            }
            eprintln!("\n=== Resolve FAILED: {} error(s) ===", errors.len());
            process::exit(1);
        }
    };

    // Type check
    match rask_types::typecheck(resolved, &parse_result.decls) {
        Ok(typed) => {
            println!("=== Types ({} registered) ===\n", typed.types.iter().count());
            for (i, type_def) in typed.types.iter().enumerate() {
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

            println!("\n=== Expression Types ({}) ===\n", typed.node_types.len());
            // Show some sample types
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

            println!("\n=== Typecheck OK ===");
        }
        Err(errors) => {
            for error in &errors {
                let span = match error {
                    rask_types::TypeError::Mismatch { span, .. } => *span,
                    rask_types::TypeError::ArityMismatch { span, .. } => *span,
                    rask_types::TypeError::NotCallable { span, .. } => *span,
                    rask_types::TypeError::NoSuchField { span, .. } => *span,
                    rask_types::TypeError::NoSuchMethod { span, .. } => *span,
                    rask_types::TypeError::InfiniteType { span, .. } => *span,
                    rask_types::TypeError::CannotInfer { span } => *span,
                    _ => rask_ast::Span::new(0, 0),
                };
                show_error(&source, span.start, &error.to_string(), None);
            }
            eprintln!("\n=== Typecheck FAILED: {} error(s) ===", errors.len());
            process::exit(1);
        }
    }
}

/// Get the line number for a byte offset.
fn get_line_number(source: &str, pos: usize) -> usize {
    source[..pos.min(source.len())].chars().filter(|&c| c == '\n').count() + 1
}

/// Show an error with source context.
fn show_error(source: &str, pos: usize, message: &str, hint: Option<&str>) {
    let mut line_num = 1;
    let mut line_start = 0;

    for (i, c) in source.char_indices() {
        if i >= pos {
            break;
        }
        if c == '\n' {
            line_num += 1;
            line_start = i + 1;
        }
    }

    let col = pos - line_start + 1;

    // Find end of line
    let line_end = source[line_start..].find('\n')
        .map(|i| line_start + i)
        .unwrap_or(source.len());

    let line = &source[line_start..line_end];

    eprintln!();
    eprintln!("error: {}", message);
    eprintln!("  --> line {}:{}", line_num, col);
    eprintln!("   |");
    eprintln!("{:3}| {}", line_num, line);
    eprintln!("   | {}^", " ".repeat(col.saturating_sub(1)));

    if let Some(hint) = hint {
        eprintln!("   |");
        eprintln!("   = hint: {}", hint);
    }
}

fn cmd_test_specs(path: Option<&str>) {
    use rask_spec_test::{extract_tests, run_test, TestSummary};

    let specs_dir = path.unwrap_or("specs");
    let specs_path = Path::new(specs_dir);

    if !specs_path.exists() {
        eprintln!("Error: specs directory not found: {}", specs_dir);
        process::exit(1);
    }

    let mut summary = TestSummary::default();
    let mut all_results = Vec::new();

    // Collect all markdown files
    let md_files = collect_md_files(specs_path);
    summary.files = md_files.len();

    for md_path in &md_files {
        let content = match fs::read_to_string(md_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Error reading {}: {}", md_path.display(), e);
                continue;
            }
        };

        let tests = extract_tests(&md_path, &content);
        if tests.is_empty() {
            continue;
        }

        println!("{}", md_path.display());

        for test in tests {
            let result = run_test(test);
            summary.add(&result);

            let status = if result.passed { "✓" } else { "✗" };
            println!(
                "  {} line {}: {:?} - {}",
                status,
                result.test.line,
                result.test.expectation,
                result.message
            );

            if !result.passed {
                all_results.push(result);
            }
        }
        println!();
    }

    // Print summary
    println!("{}", "─".repeat(50));
    println!(
        "{} files, {} tests, {} passed, {} failed",
        summary.files, summary.total, summary.passed, summary.failed
    );

    if summary.failed > 0 {
        println!("\nFailed tests:");
        for result in &all_results {
            println!(
                "  {}:{} - {}",
                result.test.path.display(),
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
