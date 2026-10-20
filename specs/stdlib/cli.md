<!-- id: std.cli -->
<!-- status: decided -->
<!-- summary: Command-line argument parsing: a schema-less quick API and a builder with --help, both producing Args -->
<!-- depends: stdlib/os.md -->

# CLI

Two ways in, one result. Scripts ask a schema-less `Args` what they want to know; tools that need `--help` and validation describe their arguments to a `Parser` first. Both hand back an `Args`.

Raw arguments are `os.args()` (`std.os/A1`). `cli` doesn't repeat it.

## Quick API

| Rule | Description |
|------|-------------|
| **Q1: Parse** | `cli.parse()` reads `os.args()`. `cli.parse_from(argv)` parses a list of the same shape: `argv[0]` is the program name |
| **Q2: Flags** | `args.flag(long, short)` is true for `--long`, `-short`, or `short` inside a group (`-vn`) |
| **Q3: Options** | `args.option(long, short)` returns `string?` from any S2 spelling. Repeated, the last one wins. A default is `?? value` |
| **Q4: Positional** | `args.positional()` returns the words that aren't flags, options or option values, in order |
| **Q5: Program name** | `args.program()` returns `argv[0]` |
| **Q6: Ask first** | Without a schema, nothing knows `--output` takes a value until someone asks. `option` is what claims the next word, so it takes `mutate self`, and an option nobody asked about leaves its value in `positional()` |
| **Q7: Empty short** | `short` may be `""`: only the long form matches |

<!-- test: skip -->
```rask
import cli

func main() {
    mut args = cli.parse()
    let verbose = args.flag("verbose", "v")
    let output = args.option("output", "o") ?? "out.txt"
    let files = args.positional()

    if files.is_empty() {
        println("Usage: {args.program()} [options] <files...>")
        os.exit(1)
    }
}
```

<!-- surface -->
```rask
struct Args { }

extend cli {
    func parse() -> Args
    func parse_from(argv: Vec<string>) -> Args
}

extend Args {
    func flag(self, long: string, short: string) -> bool
    func option(mutate self, long: string, short: string) -> string?
    func positional(self) -> Vec<string>
    func program(self) -> string
}
```

## Builder API

| Rule | Description |
|------|-------------|
| **B1: Parser** | `cli.Parser.new(name)` starts a schema for the program called `name` |
| **B2: Builder methods** | `.version(v)`, `.description(text)`, `.flag(long, short, help)`, `.option(long, short, help, required:)`, `.positional(name, help)`. Each takes the parser and returns it, so they chain |
| **B3: Parse result** | `.parse()` reads `os.args()` and returns `Args or CliError`; `.parse_from(argv)` does the same with a given list |
| **B4: Auto help** | `--help` and `-h` print `help()` and exit 0. `--version` prints `name version` and exits 0, and exists only when a version was set |
| **B5: Resolved up front** | The schema says which words are values, so the returned `Args` doesn't depend on the order it's asked in (Q6 doesn't apply). Flags and options come back in long form: `args.flag("ignore-case", "")` is enough |
| **B6: Positionals** | Each declared positional is required, in declaration order. Words past the last one are kept, so it can stand for a list |
| **B7: Required options** | `required: true` makes a missing option `CliError.MissingRequired` |

<!-- test: skip -->
```rask
import cli

func main() -> void or cli.CliError {
    mut args = try cli.Parser.new("mygrep")
        .version("1.0.0")
        .description("Search for patterns in files")
        .flag("ignore-case", "i", "Case-insensitive matching")
        .option("max-count", "m", "Stop after N matches")
        .positional("pattern", "Search pattern")
        .positional("files", "Files to search")
        .parse()
    let limit = args.option("max-count", "") ?? "0"
}
```

<!-- surface -->
```rask
struct Parser { }

extend Parser {
    func new(name: string) -> Parser
    func version(take self, version: string) -> Parser
    func description(take self, text: string) -> Parser
    func flag(take self, long: string, short: string, help: string) -> Parser
    func option(take self, long: string, short: string, help: string, required: bool = false) -> Parser
    func positional(take self, name: string, help: string) -> Parser
    func parse(self) -> Args or CliError
    func parse_from(self, argv: Vec<string>) -> Args or CliError
    func help(self) -> string
}
```

## Argument Syntax

| Rule | Description |
|------|-------------|
| **S1: Flag formats** | `--verbose`, `-v`, combined `-vn` (= `-v -n`) |
| **S2: Option formats** | `--output file`, `--output=file`, `-o file`, `-o=file`. With a schema, the last letter of a group may be an option taking the next word: `-im 5` |
| **S3: Positional** | Non-flag arguments, and a lone `-` |
| **S4: End of flags** | `--` makes everything after it positional |

## Error Type

| Rule | Description |
|------|-------------|
| **E1: CliError** | Builder `.parse()` returns `CliError` on invalid input. The payload is the argument as the user would type it: `--pattern`, `-x`, `<file>` |
| **E2: Message** | `message()` is one line: `missing required argument: --pattern`, `unknown flag: --colour`, `missing value for -m` |

<!-- surface -->
```rask
enum CliError {
    MissingRequired(string)    // a required option or positional wasn't given
    UnknownFlag(string)        // not in the schema, or a flag given a value
    MissingValue(string)       // an option with nothing after it
}

extend CliError {
    func message(self) -> string
}
```

## Error Messages

A `CliError` returned from `main` exits 1 with its message:

```
$ mygrep --ignore-case
error: missing required argument: <pattern>

$ mygrep --colour needle a.txt
error: unknown flag: --colour
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| No args | Q4 | `positional()` returns empty Vec |
| Unknown flag (quick API) | Q2 | Ignored: no schema to reject it against |
| Unknown option's value (quick API) | Q6 | Positional, since nothing asked for the option |
| `positional()` before `option()` (quick API) | Q6 | The value is still counted positional. Ask for options first |
| Option at the end with no value | Q3, E1 | Quick: `none`. Builder: `CliError.MissingValue` |
| Option value starting with `-` | S2 | Taken as the value: `--offset -5` |
| Unknown flag (builder API) | E1 | `CliError.UnknownFlag` |
| Flag given a value (builder API) | E1 | `--verbose=yes` is `CliError.UnknownFlag("--verbose=yes")` |
| Missing positional | B6 | `CliError.MissingRequired("<name>")` |
| `--` followed by `--flag` | S4 | Treated as positional string `"--flag"` |
| `-` alone | S3 | Treated as positional |
| `--version` with no version set | B4 | An unknown flag, like any other |

---

## Appendix (non-normative)

### Rationale

**Q1-Q7 (quick API):** Scripts shouldn't need a builder to check a flag. Three lines cover most scripts.

**Q6 (ask first):** The C and interpreter versions of the quick API disagreed on `--output out.txt`: C took `out.txt` as the value, the interpreter as a positional. Neither is right in general: `--verbose in.txt` breaks the first rule and `-o out.txt` breaks the second. Asking is the only thing that says which. The alternative was making the quick API reject `-o out.txt` and demand `-o=out.txt`, which is a silent misparse for anyone who types it the normal way. `mut args` is the visible cost.

**`parse_from`:** The parsing core takes a list, and `parse()` is that list being `os.args()`. It's what tests call, and what a program dispatching subcommands by hand would call on the tail.

**Dropped from the first draft:**

- `option_or(long, short, default)`. It was `option(long, short) ?? default` with a second name (`std.api/SD5`).
- `option_required(...)`. A name-variant for one boolean (`std.api/SD2`); it's `option(..., required: true)`.
- `CliError.InvalidValue`. "Value doesn't parse" needs typed options, which are deferred. A variant nothing can produce is a promise with nothing behind it. It comes back with typed options.
- `cli.args()`. It was `os.args()` again, built the same way.

**B5 (resolved up front):** With a schema there's no ambiguity left, so the builder settles every word before returning. Asking order only matters where there's no schema.

### Patterns & Guidance

**Auto-generated help output:**

```
$ mygrep --help
mygrep 1.0.0
Search for patterns in files

Usage: mygrep [options] <pattern> <files>

Arguments:
  <pattern>                Search pattern
  <files>                  Files to search

Options:
  -i, --ignore-case        Case-insensitive matching
  -m, --max-count <value>  Stop after N matches
  -h, --help               Show this help
      --version            Show version
```

A required option's line ends in `(required)`.

### Deferred

- **Subcommands:** `parser.subcommand("init", ...)`
- **Typed options:** `args.option_int("port", "p")`, with `CliError.InvalidValue`
- **Struct derivation:** `cli.parse_into<Args>()`
- **Completion scripts:** Generate bash/zsh/fish completions

### See Also

- `std.os` — `os.args()` provides raw args, `os.exit()` for exit codes
- `type.error-types` — `CliError` uses standard error pattern
