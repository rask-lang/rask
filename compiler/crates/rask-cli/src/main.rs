//! Rask CLI - REPL and file runner.

mod output;

use colored::Colorize;
use std::env;
use std::fs;
use std::path::Path;
use std::process;

fn main() {
    output::init();
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        print_usage();
        return;
    }

    match args[1].as_str() {
        "lex" => {
            if args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("lex"), output::arg("<file.rask>"));
                process::exit(1);
            }
            cmd_lex(&args[2]);
        }
        "parse" => {
            if args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("parse"), output::arg("<file.rask>"));
                process::exit(1);
            }
            cmd_parse(&args[2]);
        }
        "resolve" => {
            if args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("resolve"), output::arg("<file.rask>"));
                process::exit(1);
            }
            cmd_resolve(&args[2]);
        }
        "typecheck" | "check" => {
            if args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("typecheck"), output::arg("<file.rask>"));
                process::exit(1);
            }
            cmd_typecheck(&args[2]);
        }
        "ownership" => {
            if args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("ownership"), output::arg("<file.rask>"));
                process::exit(1);
            }
            cmd_ownership(&args[2]);
        }
        "run" => {
            if args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("run"), output::arg("<file.rask>"));
                process::exit(1);
            }
            // Pass remaining args to the program
            let program_args: Vec<String> = args[2..].to_vec();
            cmd_run(&args[2], program_args);
        }
        "test-specs" => {
            let path = args.get(2).map(|s| s.as_str());
            cmd_test_specs(path);
        }
        "build" => {
            let path = args.get(2).map(|s| s.as_str()).unwrap_or(".");
            cmd_build(path);
        }
        "help" | "--help" | "-h" => {
            print_usage();
        }
        "version" | "--version" | "-V" => {
            println!("{} {}", output::title("rask"), output::version("0.1.0"));
        }
        other => {
            // Treat as filename
            if other.ends_with(".rask") {
                cmd_parse(other);
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
    println!(
        "  {} {}       Run a Rask program",
        output::command("run"),
        output::arg("<file>")
    );
    println!(
        "  {} {}       Tokenize a file and print tokens",
        output::command("lex"),
        output::arg("<file>")
    );
    println!(
        "  {} {}     Parse a file and print AST",
        output::command("parse"),
        output::arg("<file>")
    );
    println!(
        "  {} {}   Resolve names and print symbols",
        output::command("resolve"),
        output::arg("<file>")
    );
    println!(
        "  {} {} Type check a file",
        output::command("typecheck"),
        output::arg("<file>")
    );
    println!(
        "  {} {} Check ownership and borrowing rules",
        output::command("ownership"),
        output::arg("<file>")
    );
    println!(
        "  {} {}      Build a package",
        output::command("build"),
        output::arg("[dir]")
    );
    println!(
        "  {} {} Run spec documentation tests",
        output::command("test-specs"),
        output::arg("[dir]")
    );
    println!("  {}             Show this help", output::command("help"));
    println!("  {}          Show version", output::command("version"));
}

fn cmd_lex(path: &str) {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: reading {}: {}", output::error_label(), output::file_path(path), e);
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
        println!("{} Tokens ({}) {}\n", "===".dimmed(), result.tokens.len(), "===".dimmed());
        for tok in &result.tokens {
            // Skip newlines for cleaner output (optional)
            if matches!(tok.kind, rask_ast::token::TokenKind::Newline) {
                continue;
            }
            println!("{:4}:{:<3} {:?}", tok.span.start, tok.span.end, tok.kind);
        }
        println!("\n{}", output::banner_ok(&format!("Lex: {} tokens", result.tokens.len())));
    } else {
        eprintln!("\n{}", output::banner_fail("Lex", result.errors.len()));
        process::exit(1);
    }
}

fn cmd_parse(path: &str) {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: reading {}: {}", output::error_label(), output::file_path(path), e);
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

    println!("{} Lexed {} tokens {}\n", "===".dimmed(), lex_result.tokens.len(), "===".dimmed());

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
        eprintln!("\n{}", output::banner_fail("Parse", error_count));
        process::exit(1);
    }

    println!("{} AST ({} declarations) {}\n", "===".dimmed(), parse_result.decls.len(), "===".dimmed());
    for (i, decl) in parse_result.decls.iter().enumerate() {
        println!("--- Declaration {} ---", i + 1);
        println!("{:#?}", decl);
        println!();
    }
    println!("{}", output::banner_ok("Parse"));
}

fn cmd_resolve(path: &str) {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: reading {}: {}", output::error_label(), output::file_path(path), e);
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
        eprintln!("\n{}", output::banner_fail("Lex", lex_result.errors.len()));
        process::exit(1);
    }

    // Parse
    let mut parser = rask_parser::Parser::new(lex_result.tokens);
    let parse_result = parser.parse();

    if !parse_result.is_ok() {
        for error in &parse_result.errors {
            show_error(&source, error.span.start, &error.message, error.hint.as_deref());
        }
        eprintln!("\n{}", output::banner_fail("Parse", parse_result.errors.len()));
        process::exit(1);
    }

    // Resolve
    match rask_resolve::resolve(&parse_result.decls) {
        Ok(resolved) => {
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
        Err(errors) => {
            for error in &errors {
                show_error(&source, error.span.start, &format!("{}", error.kind), None);
            }
            eprintln!("\n{}", output::banner_fail("Resolve", errors.len()));
            process::exit(1);
        }
    }
}

fn cmd_typecheck(path: &str) {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: reading {}: {}", output::error_label(), output::file_path(path), e);
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
        eprintln!("\n{}", output::banner_fail("Lex", lex_result.errors.len()));
        process::exit(1);
    }

    // Parse
    let mut parser = rask_parser::Parser::new(lex_result.tokens);
    let mut parse_result = parser.parse();

    if !parse_result.is_ok() {
        for error in &parse_result.errors {
            show_error(&source, error.span.start, &error.message, error.hint.as_deref());
        }
        eprintln!("\n{}", output::banner_fail("Parse", parse_result.errors.len()));
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
            eprintln!("\n{}", output::banner_fail("Resolve", errors.len()));
            process::exit(1);
        }
    };

    // Type check
    match rask_types::typecheck(resolved, &parse_result.decls) {
        Ok(typed) => {
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

            println!("\n{}", output::banner_ok("Typecheck"));
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
            eprintln!("\n{}", output::banner_fail("Typecheck", errors.len()));
            process::exit(1);
        }
    }
}

fn cmd_ownership(path: &str) {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: reading {}: {}", output::error_label(), output::file_path(path), e);
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
        eprintln!("\n{}", output::banner_fail("Lex", lex_result.errors.len()));
        process::exit(1);
    }

    // Parse
    let mut parser = rask_parser::Parser::new(lex_result.tokens);
    let mut parse_result = parser.parse();

    if !parse_result.is_ok() {
        for error in &parse_result.errors {
            show_error(&source, error.span.start, &error.message, error.hint.as_deref());
        }
        eprintln!("\n{}", output::banner_fail("Parse", parse_result.errors.len()));
        process::exit(1);
    }

    // Desugar operators
    rask_desugar::desugar(&mut parse_result.decls);

    // Resolve
    let resolved = match rask_resolve::resolve(&parse_result.decls) {
        Ok(r) => r,
        Err(errors) => {
            for error in &errors {
                show_error(&source, error.span.start, &format!("{}", error.kind), None);
            }
            eprintln!("\n{}", output::banner_fail("Resolve", errors.len()));
            process::exit(1);
        }
    };

    // Type check
    let typed = match rask_types::typecheck(resolved, &parse_result.decls) {
        Ok(t) => t,
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
            eprintln!("\n{}", output::banner_fail("Typecheck", errors.len()));
            process::exit(1);
        }
    };

    // Ownership analysis
    let ownership_result = rask_ownership::check_ownership(&typed, &parse_result.decls);

    if ownership_result.is_ok() {
        println!("{}", output::banner_ok("Ownership"));
        println!();
        println!("All ownership and borrowing rules verified:");
        println!("  {} No use-after-move errors", output::status_pass());
        println!("  {} Borrow scopes valid", output::status_pass());
        println!("  {} Aliasing rules satisfied", output::status_pass());
    } else {
        for error in &ownership_result.errors {
            show_error(&source, error.span.start, &error.kind.to_string(), None);
        }
        eprintln!("\n{}", output::banner_fail("Ownership", ownership_result.errors.len()));
        process::exit(1);
    }
}

fn cmd_run(path: &str, program_args: Vec<String>) {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: reading {}: {}", output::error_label(), output::file_path(path), e);
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
        eprintln!("\n{}", output::banner_fail("Lex", lex_result.errors.len()));
        process::exit(1);
    }

    // Parse
    let mut parser = rask_parser::Parser::new(lex_result.tokens);
    let mut parse_result = parser.parse();

    if !parse_result.is_ok() {
        for error in &parse_result.errors {
            show_error(&source, error.span.start, &error.message, error.hint.as_deref());
        }
        eprintln!("\n{}", output::banner_fail("Parse", parse_result.errors.len()));
        process::exit(1);
    }

    // Desugar operators (a + b → a.add(b))
    rask_desugar::desugar(&mut parse_result.decls);

    // Run the interpreter with CLI args
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

    // Discover packages
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

            // Compile each package (for now, just run the full pipeline on each)
            let mut total_errors = 0;
            for pkg in registry.packages() {
                println!("{} Compiling package: {} {}", "===".dimmed(), pkg.path_string().green(), "===".dimmed());

                // Collect all decls from all files
                let mut all_decls: Vec<_> = pkg.all_decls().cloned().collect();

                // Desugar
                rask_desugar::desugar(&mut all_decls);

                // Resolve with package context
                match rask_resolve::resolve_package(&all_decls, &registry, pkg.id) {
                    Ok(resolved) => {
                        // Type check
                        match rask_types::typecheck(resolved, &all_decls) {
                            Ok(typed) => {
                                // Ownership check
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
    eprintln!("{}: {}", output::error_label(), message.bold());
    eprintln!("  {} line {}:{}", output::error_arrow(), line_num, col);
    eprintln!("   {}", output::pipe());
    eprintln!("{} {} {}", output::line_number(line_num), output::pipe(), line);
    eprintln!(
        "   {} {}{}",
        output::pipe(),
        " ".repeat(col.saturating_sub(1)),
        output::caret()
    );

    if let Some(hint) = hint {
        eprintln!("   {}", output::pipe());
        eprintln!(
            "   {} {}: {}",
            output::hint_equals(),
            output::hint_label(),
            output::hint_text(hint)
        );
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

    // Collect all markdown files
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

    // Print summary
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

/// Validate entry points: exactly one @entry required, multiple is compile error.
fn validate_entry_points(decls: &[rask_ast::decl::Decl]) -> Result<(), String> {
    use rask_ast::decl::DeclKind;

    let mut entry_count = 0;
    let mut entry_names = Vec::new();

    for decl in decls {
        if let DeclKind::Fn(f) = &decl.kind {
            if f.attrs.iter().any(|a| a == "entry") {
                entry_count += 1;
                entry_names.push(f.name.clone());
            }
        }
    }

    match entry_count {
        0 => Err("no @entry function found (add @entry to mark the program entry point)".to_string()),
        1 => Ok(()),
        _ => Err(format!(
            "multiple @entry functions found: {} (only one allowed per program)",
            entry_names.join(", ")
        )),
    }
}
