// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Compiler phase inspection commands: lex, parse, resolve.

use colored::Colorize;
use rask_diagnostics::{Diagnostic, ToDiagnostic};
use std::fs;
use std::process;

use crate::{output, Format, show_diagnostics, get_line_number};

pub fn cmd_lex(path: &str, format: Format) {
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

pub fn cmd_parse(path: &str, format: Format) {
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

pub fn cmd_resolve(path: &str, format: Format) {
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
