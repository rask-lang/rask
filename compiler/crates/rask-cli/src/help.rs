// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Help text for CLI commands.

use colored::Colorize;
use crate::output;

pub fn print_usage() {
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
    println!("  {} {}      Dump monomorphized functions + layouts", output::command("mono"), output::arg("<file>"));
    println!("  {} {}       Dump MIR (mid-level IR)", output::command("mir"), output::arg("<file>"));

    println!();
    println!("{}", output::section_header("Options:"));
    println!("  {}   Output diagnostics as structured JSON", output::arg("--json"));
}

pub fn print_help_help() {
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

pub fn print_run_help() {
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

pub fn print_build_help() {
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

pub fn print_test_help() {
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

pub fn print_benchmark_help() {
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

pub fn print_test_specs_help() {
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

pub fn print_fmt_help() {
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

pub fn print_describe_help() {
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

pub fn print_lint_help() {
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

pub fn print_explain_help() {
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

pub fn print_lex_help() {
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

pub fn print_parse_help() {
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

pub fn print_resolve_help() {
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

pub fn print_typecheck_help() {
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

pub fn print_ownership_help() {
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

pub fn print_comptime_help() {
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

pub fn print_mono_help() {
    println!("{}", output::section_header("Mono"));
    println!();
    println!("Monomorphize a Rask source file — eliminate generics and compute layouts.");
    println!("Runs pipeline: lex → parse → resolve → typecheck → ownership → monomorphize,");
    println!("then prints reachable functions with struct/enum memory layouts.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("mono"),
        output::arg("<file.rk>"));
    println!();
    println!("{}", output::section_header("Options:"));
    println!("  {}  Output monomorphization results as structured JSON", output::arg("--json"));
}

pub fn print_mir_help() {
    println!("{}", output::section_header("MIR"));
    println!();
    println!("Lower a Rask source file to MIR (mid-level intermediate representation).");
    println!("Runs full pipeline: lex → parse → resolve → typecheck → ownership →");
    println!("monomorphize → MIR lowering, then prints the control-flow graph.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("mir"),
        output::arg("<file.rk>"));
    println!();
    println!("{}", output::section_header("Options:"));
    println!("  {}  Output MIR as structured JSON", output::arg("--json"));
}
