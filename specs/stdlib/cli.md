<!-- id: std.cli -->
<!-- status: decided -->
<!-- summary: Command-line argument parsing with quick and builder APIs -->
<!-- depends: stdlib/os.md -->

# CLI

Two-level API: quick ad-hoc parsing for scripts, builder API for tools needing help text and validation.

## Quick API

| Rule | Description |
|------|-------------|
| **Q1: Parse** | `cli.parse()` returns an `Args` struct from `os.args()` |
| **Q2: Flags** | `args.flag(long, short)` returns `bool` |
| **Q3: Options** | `args.option(long, short)` returns `string?`; `args.option_or(long, short, default)` returns `string` |
| **Q4: Positional** | `args.positional()` returns `Vec<string>` of remaining non-flag args |
| **Q5: Program name** | `args.program()` returns `string` (args[0]) |

<!-- test: skip -->
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
}
```

## Builder API

| Rule | Description |
|------|-------------|
| **B1: Parser** | `cli.Parser.new(name)` returns a builder for structured parsing |
| **B2: Builder methods** | `.version()`, `.description()`, `.flag()`, `.option()`, `.option_required()`, `.positional()` configure the parser |
| **B3: Parse result** | `.parse()` returns `Args or CliError` |
| **B4: Auto help** | Builder auto-generates `--help` and `--version` output |

<!-- test: skip -->
```rask
import cli

func main() -> () or CliError {
    const args = try cli.Parser.new("mygrep")
        .version("1.0.0")
        .description("Search for patterns in files")
        .flag("ignore-case", "i", "Case-insensitive matching")
        .option("max-count", "m", "Stop after N matches")
        .positional("pattern", "Search pattern")
        .positional("files", "Files to search")
        .parse()
}
```

## Argument Syntax

| Rule | Description |
|------|-------------|
| **S1: Flag formats** | `--verbose`, `-v`, combined `-vn` (= `-v -n`) |
| **S2: Option formats** | `--output file`, `--output=file`, `-o file`, `-o=file` |
| **S3: Positional** | Non-flag arguments |
| **S4: End of flags** | `--` makes everything after positional |

## Error Type

| Rule | Description |
|------|-------------|
| **E1: CliError** | Builder `.parse()` returns `CliError` on invalid input |

<!-- test: skip -->
```rask
enum CliError {
    MissingRequired(string)    // required option not provided
    UnknownFlag(string)        // unrecognized --flag
    MissingValue(string)       // --option without value
    InvalidValue(string)       // value doesn't parse
}
```

## Error Messages

```
ERROR [std.cli/E1]: missing required option
   |
   $ mygrep --ignore-case
   ^^^^^^^^ missing required option: --pattern

WHY: option_required() options must be provided.

FIX: Add the missing option: mygrep --pattern "search term"
```

```
ERROR [std.cli/E1]: unknown flag
   |
   $ mygrep --colour
   ^^^^^^^^ unknown flag: --colour

WHY: Builder rejects flags not registered via .flag() or .option().

FIX: Check --help for valid options.
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| No args | Q4 | `positional()` returns empty Vec |
| Unknown flag (quick API) | Q2 | Silently ignored (no schema) |
| Unknown flag (builder API) | E1 | Returns `CliError.UnknownFlag` |
| `--` followed by `--flag` | S4 | Treated as positional string `"--flag"` |
| `-` alone | S3 | Treated as positional |

---

## Appendix (non-normative)

### Rationale

**Q1-Q5 (quick API):** Scripts shouldn't need a builder to check a flag. `cli.parse()` covers 80% of CLI needs in 3 lines.

**B1-B4 (builder API):** Real tools need help text and validation. The builder generates `--help` from the same source of truth used for parsing.

### Patterns & Guidance

**Auto-generated help output:**

```
$ mygrep --help
mygrep 1.0.0
Search for patterns in files

Usage: mygrep [options] <pattern> <files>

Options:
  -i, --ignore-case    Case-insensitive matching
  -m, --max-count <value>  Stop after N matches
  -h, --help           Show this help
      --version        Show version
```

### Deferred

- **Subcommands:** `parser.subcommand("init", ...)`
- **Typed options:** `args.option_int("port", "p")`
- **Struct derivation:** `cli.parse_into<Args>()`
- **Completion scripts:** Generate bash/zsh/fish completions

### See Also

- `std.os` — `os.args()` provides raw args, `os.exit()` for exit codes
- `type.error-types` — `CliError` uses standard error pattern
