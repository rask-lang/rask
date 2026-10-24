// SPDX-License-Identifier: (MIT OR Apache-2.0)
// Rust port of examples/grep_clone.rk. Same flags, output and exit codes;
// rustfmt-formatted, standard library only.

use std::{env, fmt, fs, process};

struct GrepOptions {
    pattern: String,
    files: Vec<String>,
    ignore_case: bool,
    line_numbers: bool,
    count_only: bool,
    invert_match: bool,
}

enum GrepError {
    NoPattern,
    NoFiles,
    File(String, std::io::Error),
}

impl fmt::Display for GrepError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            GrepError::NoPattern => write!(f, "missing pattern argument"),
            GrepError::NoFiles => write!(f, "no files specified"),
            GrepError::File(path, e) => write!(f, "{path}: {e}"),
        }
    }
}

fn parse_args(args: Vec<String>) -> Result<GrepOptions, GrepError> {
    let mut ignore_case = false;
    let mut line_numbers = false;
    let mut count_only = false;
    let mut invert_match = false;
    let mut positional = Vec::new();

    for arg in args.into_iter().skip(1) {
        match arg.as_str() {
            "-i" => ignore_case = true,
            "-n" => line_numbers = true,
            "-c" => count_only = true,
            "-v" => invert_match = true,
            "-h" | "--help" => {
                print_usage();
                process::exit(0);
            }
            "--" => {}
            _ => positional.push(arg),
        }
    }

    if positional.is_empty() {
        return Err(GrepError::NoPattern);
    }
    if positional.len() < 2 {
        return Err(GrepError::NoFiles);
    }

    let pattern = positional.remove(0);
    Ok(GrepOptions {
        pattern,
        files: positional,
        ignore_case,
        line_numbers,
        count_only,
        invert_match,
    })
}

fn print_usage() {
    println!("Usage: rgrep [OPTIONS] PATTERN FILE...");
    println!();
    println!("Options:");
    println!("  -i    Ignore case");
    println!("  -n    Show line numbers");
    println!("  -c    Count matching lines only");
    println!("  -v    Invert match (show non-matching lines)");
    println!("  -h    Show this help");
}

fn line_matches(line: &str, pattern: &str, ignore_case: bool) -> bool {
    if ignore_case {
        line.to_lowercase().contains(&pattern.to_lowercase())
    } else {
        line.contains(pattern)
    }
}

fn grep_file(path: &str, opts: &GrepOptions) -> Result<i64, GrepError> {
    let content = fs::read_to_string(path).map_err(|e| GrepError::File(path.to_string(), e))?;

    let mut match_count = 0;
    for (i, line) in content.lines().enumerate() {
        let matches = line_matches(line, &opts.pattern, opts.ignore_case);
        if matches == opts.invert_match {
            continue;
        }
        match_count += 1;
        if opts.count_only {
            continue;
        }
        if opts.line_numbers {
            println!("{}:{line}", i + 1);
        } else {
            println!("{line}");
        }
    }

    if opts.count_only {
        println!("{match_count}");
    }
    Ok(match_count)
}

fn main() {
    let opts = parse_args(env::args().collect()).unwrap_or_else(|e| {
        println!("rgrep: {e}");
        print_usage();
        process::exit(2);
    });

    let mut total_matches = 0;
    let mut had_errors = false;
    for path in &opts.files {
        match grep_file(path, &opts) {
            Ok(found) => total_matches += found,
            Err(e) => {
                println!("rgrep: {e}");
                had_errors = true;
            }
        }
    }

    if had_errors {
        process::exit(2);
    }
    if total_matches == 0 {
        process::exit(1);
    }
}
