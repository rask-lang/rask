# CLI — Command-Line Argument Parsing

Two-level API: quick ad-hoc parsing for scripts, builder API for tools needing help text and validation.

## Specification

### Quick API

For scripts and simple tools — parse `os.args()` into a queryable `Args` struct:

```rask
cli.parse() -> Args
```

### Args Methods

```rask
args.flag(long: string, short: string) -> bool       // --verbose or -v
args.option(long: string, short: string) -> string?   // --output=file or -o file
args.option_or(long: string, short: string, default: string) -> string
args.positional() -> Vec<string>                      // remaining non-flag args
args.program() -> string                              // program name (args[0])
```

### Quick API Usage

```rask
import cli

func main() -> () or string {
    const args = cli.parse()

    const verbose = args.flag("verbose", "v")
    const output = args.option_or("output", "o", "out.txt")
    const files = args.positional()

    if files.is_empty() {
        println("Usage: {args.program()} [options] <files...>")
        os.exit(1)
    }

    for file in files {
        process(file, output, verbose)
    }
}
```

### Builder API

For tools that need help text, validation, and `--help` auto-generation:

```rask
cli.Parser.new(name: string) -> Parser
```

### Parser Methods (Builder Pattern)

```rask
parser.version(v: string) -> Parser
parser.description(d: string) -> Parser
parser.flag(long: string, short: string, help: string) -> Parser
parser.option(long: string, short: string, help: string) -> Parser
parser.option_required(long: string, short: string, help: string) -> Parser
parser.positional(name: string, help: string) -> Parser
parser.parse() -> Args or CliError
```

### Builder API Usage

```rask
import cli

func main() -> () or CliError {
    const args = try cli.Parser.new("mygrep")
        .version("1.0.0")
        .description("Search for patterns in files")
        .flag("ignore-case", "i", "Case-insensitive matching")
        .flag("count", "c", "Print match count only")
        .flag("line-number", "n", "Show line numbers")
        .flag("invert", "v", "Invert match")
        .option("max-count", "m", "Stop after N matches")
        .positional("pattern", "Search pattern")
        .positional("files", "Files to search")
        .parse()

    const ignore_case = args.flag("ignore-case", "i")
    const pattern = args.positional()[0]
    const files = args.positional()[1..]
    // ...
}
```

### Error Type

```rask
enum CliError {
    MissingRequired(string)    // required option not provided
    UnknownFlag(string)        // unrecognized --flag
    MissingValue(string)       // --option without value
    InvalidValue(string)       // value doesn't parse (future: typed options)
}
```

### Auto-Generated Help

Builder auto-generates `--help` and `--version`:

```
$ mygrep --help
mygrep 1.0.0
Search for patterns in files

Usage: mygrep [options] <pattern> <files>

Options:
  -i, --ignore-case    Case-insensitive matching
  -c, --count          Print match count only
  -n, --line-number    Show line numbers
  -v, --invert         Invert match
  -m, --max-count <value>  Stop after N matches
  -h, --help           Show this help
      --version        Show version
```

### Argument Syntax

Supported formats:

| Format | Example | Meaning |
|--------|---------|---------|
| Long flag | `--verbose` | Boolean flag |
| Short flag | `-v` | Boolean flag |
| Combined short | `-vn` | Multiple flags: `-v -n` |
| Long option | `--output file` | Option with space |
| Long option = | `--output=file` | Option with equals |
| Short option | `-o file` | Option with space |
| Short option = | `-o=file` | Option with equals |
| Positional | `file.txt` | Non-flag argument |
| End of flags | `--` | Everything after is positional |

## Examples

### Grep Clone

```rask
import cli
import fs
import os

func main() -> () or string {
    const args = cli.parse()

    const ignore_case = args.flag("ignore-case", "i")
    const show_line_num = args.flag("line-number", "n")
    const count_only = args.flag("count", "c")
    const invert = args.flag("invert-match", "v")

    const positional = args.positional()
    if positional.len() < 2 {
        println("Usage: grep [options] <pattern> <file...>")
        os.exit(1)
    }

    const pattern = positional[0]
    const files = positional[1..]

    for file in files {
        const lines = try fs.read_lines(file)
        let match_count = 0

        for i in 0..lines.len() {
            const line = lines[i]
            let matches = line.contains(pattern)
            if ignore_case {
                matches = line.to_lowercase().contains(pattern.to_lowercase())
            }
            if invert {
                matches = !matches
            }

            if matches {
                match_count += 1
                if !count_only {
                    if show_line_num {
                        println("{i + 1}:{line}")
                    } else {
                        println(line)
                    }
                }
            }
        }

        if count_only {
            println("{match_count}")
        }
    }
}
```

### Simple Script

```rask
import cli
import os

func main() {
    const args = cli.parse()

    if args.flag("help", "h") {
        println("Usage: deploy [--env staging|prod] [--dry-run]")
        os.exit(0)
    }

    const env = args.option_or("env", "e", "staging")
    const dry_run = args.flag("dry-run", "n")

    println("Deploying to {env}{if dry_run: " (dry run)" else: ""}")
}
```

## Deferred

- **Subcommands**: `parser.subcommand("init", ...)` — not needed for Phase 2 litmus tests
- **Typed options**: `args.option_int("port", "p")`, `args.option_float(...)` — parse to typed values
- **Struct derivation**: `cli.parse_into<Args>()` — auto-map struct fields to CLI args
- **Completion scripts**: Generate bash/zsh/fish completions from Parser definition

## References

- specs/stdlib/os.md — `os.args()` provides raw args, `os.exit()` for exit codes
- specs/types/error-types.md — `CliError` uses standard error pattern

## Status

**Specified** — ready for implementation in interpreter.
