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
    println!("  {} {}       Run a Rask program (interpreter)", output::command("run"), output::arg("<file>"));
    println!("  {} {}   Compile to native executable", output::command("compile"), output::arg("<file>"));
    println!("  {} {}      Build a package", output::command("build"), output::arg("[dir]"));
    println!("  {} {}     Remove build artifacts", output::command("clean"), output::arg("[dir]"));
    println!("  {}          List available compilation targets", output::command("targets"));
    println!("  {} {}       Format source files", output::command("fmt"), output::arg("<file>"));
    println!("  {} {}   Explain an error code", output::command("explain"), output::arg("<code>"));
    println!("  {}             Show this help", output::command("help"));
    println!("  {}          Show version", output::command("version"));

    println!();
    println!("{}", output::section_header("Project:"));
    println!("  {} {}     Create a new Rask project", output::command("init"), output::arg("[name]"));
    println!("  {} {}     Resolve and validate dependencies", output::command("fetch"), output::arg("[dir]"));
    println!("  {} {}    Regenerate rask.lock", output::command("update"), output::arg("[dir]"));
    println!();
    println!("{}", output::section_header("Dependencies:"));
    println!("  {} {}       Add a dependency to build.rk", output::command("add"), output::arg("<pkg>"));
    println!("  {} {}    Remove a dependency", output::command("remove"), output::arg("<pkg>"));
    println!("  {} {}   Copy deps to vendor/ for offline builds", output::command("vendor"), output::arg("[dir]"));
    println!("  {} {}    Check deps for known vulnerabilities", output::command("audit"), output::arg("[dir]"));

    println!();
    println!("{}", output::section_header("Publishing:"));
    println!("  {} {}  Publish a package to the registry", output::command("publish"), output::arg("[dir]"));
    println!("  {} {} Hide a version from new resolution", output::command("yank"), output::arg("<pkg> <ver>"));

    println!();
    println!("{}", output::section_header("Development:"));
    println!("  {} {}   Watch files and re-run on change", output::command("watch"), output::arg("[cmd]"));

    println!();
    println!("{}", output::section_header("Testing:"));
    println!("  {} {}      Run tests in a file", output::command("test"), output::arg("<file>"));
    println!("  {} {} Run benchmarks in a file", output::command("benchmark"), output::arg("<file>"));
    println!("  {} {} Run spec documentation tests", output::command("test-specs"), output::arg("[dir]"));

    println!();
    println!("{}", output::section_header("Debugging and Exploration:"));
    println!("  {} {}  Lint source files for conventions", output::command("lint"), output::arg("<file|dir>"));
    println!("  {} {}       Show a module's public API", output::command("api"), output::arg("<file>"));

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
    println!("Execute a Rask program. Uses the interpreter by default.");
    println!("With --native, compiles to a temp executable and runs it.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("run"),
        output::arg("<file.rk> [--native] [-- <program args>]"));
    println!();
    println!("{}", output::section_header("Options:"));
    println!("  {}    Compile and run as native executable", output::arg("--native"));
    println!("  {}        Output diagnostics as structured JSON", output::arg("--json"));
    println!("  {}             Pass arguments to the program (after --)", output::arg("--"));
    println!();
    println!("{}", output::section_header("Examples:"));
    println!("  {} {} {}              Run via interpreter",
        output::command("rask"),
        output::command("run"),
        output::arg("main.rk"));
    println!("  {} {} {} {}   Compile and run natively",
        output::command("rask"),
        output::command("run"),
        output::arg("main.rk"),
        output::arg("--native"));
    println!("  {} {} {} {} {}   Pass args to program",
        output::command("rask"),
        output::command("run"),
        output::arg("main.rk"),
        output::arg("--"),
        output::arg("arg1 arg2"));
}

pub fn print_compile_help() {
    println!("{}", output::section_header("Compile"));
    println!();
    println!("Compile a single .rk file to a native executable.");
    println!("Runs the full pipeline: lex, parse, resolve, typecheck, ownership,");
    println!("monomorphize, MIR lowering, Cranelift codegen, link with runtime.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("compile"),
        output::arg("<file.rk> [-o <output>]"));
    println!();
    println!("{}", output::section_header("Options:"));
    println!("  {} {}   Output executable path (default: input stem)", output::arg("-o"), output::arg("<path>"));
    println!("  {}        Output diagnostics as structured JSON", output::arg("--json"));
    println!();
    println!("{}", output::section_header("Examples:"));
    println!("  {} {} {}           Produces ./main",
        output::command("rask"),
        output::command("compile"),
        output::arg("main.rk"));
    println!("  {} {} {} {} {}  Produces ./app",
        output::command("rask"),
        output::command("compile"),
        output::arg("main.rk"),
        output::arg("-o"),
        output::arg("app"));
}

pub fn print_build_help() {
    println!("{}", output::section_header("Build"));
    println!();
    println!("Build a Rask package. Discovers .rk files, compiles, and links.");
    println!("Output goes to build/<profile>/.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("build"),
        output::arg("[directory] [options]"));
    println!();
    println!("{}", output::section_header("Options:"));
    println!("  {}          Build with release profile", output::arg("--release"));
    println!("  {} {}  Build with custom profile", output::arg("--profile"), output::arg("<name>"));
    println!("  {} {} Cross-compile for target", output::arg("--target"), output::arg("<triple>"));
    println!("  {}           Bypass all caching (build script + compilation)", output::arg("--force"));
    println!("  {} {}    Max parallel jobs (default: CPU count)", output::arg("--jobs"), output::arg("<N>"));
    println!("  {} {}       Verbose output", output::arg("-v"), output::arg("--verbose"));
    println!();
    println!("If no directory is specified, builds the current directory.");
    println!("Use {} to see available targets.", "rask targets".cyan());
}

pub fn print_targets_help() {
    println!("{}", output::section_header("Targets"));
    println!();
    println!("List available cross-compilation targets with tier info.");
    println!();
    println!("{}: {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("targets"));
    println!();
    println!("Targets are organized into three tiers:");
    println!("  {} Tested in CI, guaranteed to work", "Tier 1:".yellow().bold());
    println!("  {} Builds successfully, best-effort support", "Tier 2:".yellow());
    println!("  {} Community-maintained", "Tier 3:".dimmed());
    println!();
    println!("Use with: {} {} {}",
        output::command("rask"),
        output::command("build"),
        output::arg("--target <triple>"));
}

pub fn print_clean_help() {
    println!("{}", output::section_header("Clean"));
    println!();
    println!("Remove build artifacts.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("clean"),
        output::arg("[directory] [--all]"));
    println!();
    println!("{}", output::section_header("Options:"));
    println!("  {}  Also clean global cache entries for this project", output::arg("--all"));
}

pub fn print_add_help() {
    println!("{}", output::section_header("Add"));
    println!();
    println!("Add a dependency to build.rk.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("add"),
        output::arg("<package> [version] [--dev] [--feature <name>] [--path <dir>]"));
    println!();
    println!("{}", output::section_header("Options:"));
    println!("  {}          Add to scope \"dev\" (test dependencies)", output::arg("--dev"));
    println!("  {} {} Add to a feature block", output::arg("--feature"), output::arg("<name>"));
    println!("  {} {}    Local path dependency", output::arg("--path"), output::arg("<dir>"));
    println!();
    println!("{}", output::section_header("Examples:"));
    println!("  {} {} {}             Add latest version",
        output::command("rask"),
        output::command("add"),
        output::arg("http"));
    println!("  {} {} {} {}     Add with version",
        output::command("rask"),
        output::command("add"),
        output::arg("http"),
        output::arg("\"^2.0\""));
    println!("  {} {} {} {}    Add dev dependency",
        output::command("rask"),
        output::command("add"),
        output::arg("mock"),
        output::arg("--dev"));
}

pub fn print_remove_help() {
    println!("{}", output::section_header("Remove"));
    println!();
    println!("Remove a dependency from build.rk.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("remove"),
        output::arg("<package>"));
}

pub fn print_watch_help() {
    println!("{}", output::section_header("Watch"));
    println!();
    println!("Watch .rk files and build.rk for changes, re-running a command.");
    println!("Default: runs `rask check` (type-check only, fastest feedback).");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("watch"),
        output::arg("[command] [--no-clear]"));
    println!();
    println!("{}", output::section_header("Commands:"));
    println!("  {}    Type-check on change (default)", output::arg("check"));
    println!("  {}    Build on change", output::arg("build"));
    println!("  {}     Run tests on change", output::arg("test"));
    println!("  {}      Build and run on change", output::arg("run"));
    println!("  {}     Lint on change", output::arg("lint"));
    println!();
    println!("{}", output::section_header("Options:"));
    println!("  {}  Don't clear terminal on each rebuild", output::arg("--no-clear"));
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
    println!("Run benchmarks from a file or directory. Compiles natively by default.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("benchmark"),
        output::arg("<file.rk | dir/> [-f <pattern>]"));
    println!();
    println!("When given a directory, discovers all .rk files and runs their benchmarks.");
    println!("If a matching .c file exists, compiles it as a C baseline and shows ratios.");
    println!();
    println!("{}", output::section_header("Options:"));
    println!("  {}       Output as structured JSON", output::arg("--json"));
    println!("  {} {} Filter benchmarks by name pattern", output::arg("-f"), output::arg("<pattern>"));
    println!("  {} {}  Save results as a baseline", output::arg("--save"), output::arg("<file>"));
    println!("  {} {} Compare against a saved baseline", output::arg("--compare"), output::arg("<file>"));
    println!("  {}  Compile C baselines with -O0 (fair Cranelift comparison)", output::arg("--baseline-O0"));
    println!();
    println!("{}", output::section_header("Examples:"));
    println!("  {} {} {}        Run one benchmark file",
        output::command("rask"),
        output::command("benchmark"),
        output::arg("bench.rk"));
    println!("  {} {} {}  Run suite with C comparison",
        output::command("rask"),
        output::command("benchmark"),
        output::arg("benchmarks/micro/"));
    println!("  {} {} {} {} {}         Save baseline",
        output::command("rask"),
        output::command("benchmark"),
        output::arg("benchmarks/micro/"),
        output::arg("--save"),
        output::arg("base.json"));
    println!("  {} {} {} {} {}      Detect regressions",
        output::command("rask"),
        output::command("benchmark"),
        output::arg("benchmarks/micro/"),
        output::arg("--compare"),
        output::arg("base.json"));
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

pub fn print_api_help() {
    println!("{}", output::section_header("API"));
    println!();
    println!("Show a module's public API including structs, functions, and enums.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("api"),
        output::arg("<file.rk> [--all]"));
    println!();
    println!("{}", output::section_header("Options:"));
    println!("  {}     Show all items including private ones", output::arg("--all"));
    println!("  {}   Output as structured JSON", output::arg("--json"));
    println!();
    println!("{}", output::section_header("Examples:"));
    println!("  {} {} {}        Show public API",
        output::command("rask"),
        output::command("api"),
        output::arg("module.rk"));
    println!("  {} {} {} {}  Show all items",
        output::command("rask"),
        output::command("api"),
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

pub fn print_init_help() {
    println!("{}", output::section_header("Init"));
    println!();
    println!("Create a new Rask project with build.rk, main.rk, and .gitignore.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("init"),
        output::arg("[name]"));
    println!();
    println!("If a name is given, creates a new directory.");
    println!("If no name, initializes in the current directory.");
    println!();
    println!("{}", output::section_header("Examples:"));
    println!("  {} {} {}    Create new project in my-app/",
        output::command("rask"),
        output::command("init"),
        output::arg("my-app"));
    println!("  {} {}              Initialize current directory",
        output::command("rask"),
        output::command("init"));
}

pub fn print_fetch_help() {
    println!("{}", output::section_header("Fetch"));
    println!();
    println!("Resolve and validate all dependencies declared in build.rk.");
    println!("Checks version constraints, validates path deps exist,");
    println!("infers capabilities, and updates rask.lock.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("fetch"),
        output::arg("[directory] [--verbose]"));
    println!();
    println!("{}", output::section_header("Options:"));
    println!("  {} {} Verbose output", output::arg("-v"), output::arg("--verbose"));
    println!();
    println!("{}", output::section_header("Examples:"));
    println!("  {} {}          Fetch in current directory",
        output::command("rask"),
        output::command("fetch"));
    println!("  {} {} {}  Verbose output",
        output::command("rask"),
        output::command("fetch"),
        output::arg("-v"));
}

pub fn print_vendor_help() {
    println!("{}", output::section_header("Vendor"));
    println!();
    println!("Copy all registry dependencies to vendor/ for offline builds.");
    println!("Requires a rask.lock — run `rask fetch` first.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("vendor"),
        output::arg("[directory] [--verbose]"));
    println!();
    println!("{}", output::section_header("Options:"));
    println!("  {} {} Verbose output", output::arg("-v"), output::arg("--verbose"));
    println!();
    println!("After vendoring, add `vendor_dir: \"vendor\"` to build.rk");
    println!("to resolve dependencies from the vendor directory.");
}

pub fn print_publish_help() {
    println!("{}", output::section_header("Publish"));
    println!();
    println!("Publish a package to the registry.");
    println!("Runs check + test, builds a reproducible tarball, and uploads.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("publish"),
        output::arg("[directory] [--dry-run] [--verbose]"));
    println!();
    println!("{}", output::section_header("Options:"));
    println!("  {}    Show what would be published without uploading", output::arg("--dry-run"));
    println!("  {} {} Verbose output", output::arg("-v"), output::arg("--verbose"));
    println!();
    println!("{}", output::section_header("Requirements:"));
    println!("  build.rk must have `description` and `license` metadata.");
    println!("  Packages with path dependencies cannot be published.");
    println!("  Auth token via RASK_REGISTRY_TOKEN or ~/.rask/credentials.");
}

pub fn print_yank_help() {
    println!("{}", output::section_header("Yank"));
    println!();
    println!("Hide a published version from new dependency resolution.");
    println!("Existing lock files that pin this version are unaffected.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("yank"),
        output::arg("<package> <version>"));
    println!();
    println!("Auth token via RASK_REGISTRY_TOKEN or ~/.rask/credentials.");
}

pub fn print_audit_help() {
    println!("{}", output::section_header("Audit"));
    println!();
    println!("Check dependencies for known vulnerabilities.");
    println!("Reads exact versions from rask.lock and queries the advisory database.");
    println!();
    println!("{}: {} {} {}", "Usage".yellow(),
        output::command("rask"),
        output::command("audit"),
        output::arg("[directory] [--ignore CVE-ID] [--db path]"));
    println!();
    println!("{}", output::section_header("Options:"));
    println!("  {} {} Ignore a specific advisory", output::arg("--ignore"), output::arg("<CVE-ID>"));
    println!("  {}     {}  Use a local advisory database (offline)", output::arg("--db"), output::arg("<path>"));
    println!("  {} {} Verbose output", output::arg("-v"), output::arg("--verbose"));
    println!();
    println!("Returns non-zero exit code if vulnerabilities are found (CI-friendly).");
}
