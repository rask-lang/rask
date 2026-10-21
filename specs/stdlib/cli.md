<!-- id: std.cli -->
<!-- status: decided -->
<!-- summary: Command-line argument parsing: describe the arguments to a Parser, get back a resolved Args or a CliError, and --help for free -->
<!-- depends: stdlib/os.md -->

# CLI

A program describes its arguments to a `Parser`, and the parser hands back an `Args` with every word already sorted into flags, option values and positionals. The same description is the `--help` text.

Raw arguments are `os.args()` (`std.os/A1`). `cli` doesn't repeat it.

## Parser

| Rule | Description |
|------|-------------|
| **P1: Parser** | `cli.Parser.new(name)` starts a schema for the program called `name` |
| **P2: Builder methods** | `.version(v)`, `.description(text)`, `.flag(long, short, help)`, `.option(long, short, help, required:)`, `.positional(name, help)`. Each takes the parser and returns it, so they chain. `short` may be `""`: then only `--long` is accepted |
| **P3: Parse** | `.parse()` reads `os.args()` and returns `Args or CliError`. `.parse_from(argv)` does the same with a given list, `argv[0]` being the program name |
| **P4: Auto help** | `--help` and `-h` print `help()` and exit 0. `--version` prints `name version` and exits 0, and exists only when a version was set |
| **P5: Positionals** | Each declared positional is required, in declaration order. Words past the last one are kept, so it can stand for a list |
| **P6: Required options** | `required: true` makes a missing option `CliError.MissingRequired` |

<!-- test: skip -->
```rask
import cli

func main() -> void or cli.CliError {
    let args = try cli.Parser.new("mygrep")
        .version("1.0.0")
        .description("Search for patterns in files")
        .flag("ignore-case", "i", "Case-insensitive matching")
        .option("max-count", "m", "Stop after N matches")
        .positional("pattern", "Search pattern")
        .positional("files", "Files to search")
        .parse()
    let ignore_case = args.flag("ignore-case")
    let limit = args.option("max-count") ?? "0"
    let files = args.positional()
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

## Args

| Rule | Description |
|------|-------------|
| **A1: Resolved up front** | Every word is sorted before `parse` returns. Reading `Args` is plain reads, in any order |
| **A2: Long names** | Flags and options are looked up by long name, whichever spelling was typed: after `-im 5`, `args.flag("ignore-case")` and `args.option("max-count")` |
| **A3: Flags** | `args.flag(long)` is true when the flag was passed |
| **A4: Options** | `args.option(long)` returns `string?`. Repeated, the last one wins. A default is `?? value` |
| **A5: Positional** | `args.positional()` returns the words that aren't flags, options or option values, in order |
| **A6: Program name** | `args.program()` returns `argv[0]` |

<!-- surface -->
```rask
struct Args { }

extend Args {
    func flag(self, long: string) -> bool
    func option(self, long: string) -> string?
    func positional(self) -> Vec<string>
    func program(self) -> string
}
```

## Argument Syntax

| Rule | Description |
|------|-------------|
| **S1: Flag formats** | `--verbose`, `-v`, combined `-vn` (= `-v -n`) |
| **S2: Option formats** | `--output file`, `--output=file`, `-o file`, `-o=file`. The last letter of a group may be an option taking the next word: `-im 5` |
| **S3: Positional** | Non-flag arguments, and a lone `-` |
| **S4: End of flags** | `--` makes everything after it positional |

## Error Type

| Rule | Description |
|------|-------------|
| **E1: CliError** | `.parse()` returns `CliError` on invalid input. The payload is the argument as the user would type it: `--pattern`, `-x`, `<file>` |
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
| No args, no positionals declared | A5 | `positional()` returns empty Vec |
| Option at the end with no value | E1 | `CliError.MissingValue` |
| Option value starting with `-` | S2 | Taken as the value: `--offset -5` |
| Unknown flag | E1 | `CliError.UnknownFlag` |
| Flag given a value | E1 | `--verbose=yes` is `CliError.UnknownFlag("--verbose=yes")` |
| Missing positional | P5 | `CliError.MissingRequired("<name>")` |
| `--` followed by `--flag` | S4 | Treated as positional string `"--flag"` |
| `-` alone | S3 | Treated as positional |
| `--version` with no version set | P4 | An unknown flag, like any other |
| Asking for a name not in the schema | A3, A4 | `false` / `none` |

---

## Appendix (non-normative)

### Rationale

**No schema-less API.** There used to be a `cli.parse()` that took no schema. It can't know whether `--output out.txt` is an option and its value or a flag and a file, so every answer is wrong for someone; getopt, Go's `flag`, argparse and clap all make you declare it for the same reason.

**A1 (resolved up front):** With a schema there's nothing left to decide at lookup time, so `Args` is plain data and its accessors take `self`.

**A2 (long names):** The short letter is a spelling of the long name, not a second key. One name per thing keeps lookups from disagreeing with the schema.

**`parse_from`:** The parsing core takes a list, and `parse()` is that list being `os.args()`. It's what tests call, and what a program dispatching subcommands by hand would call on the tail.

**Dropped from the first draft:**

- `option_or(long, default)`. It was `option(long) ?? default` with a second name (`std.api/SD5`).
- `option_required(...)`. A name-variant for one boolean (`std.api/SD2`); it's `option(..., required: true)`.
- `CliError.InvalidValue`. "Value doesn't parse" needs typed options, which are deferred. A variant nothing can produce is a promise with nothing behind it. It comes back with typed options.
- `cli.args()`. It was `os.args()` again, built the same way.

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
- **Typed options:** `args.option_int("port")`, with `CliError.InvalidValue`
- **Struct derivation:** `cli.parse_into<Args>()`
- **Completion scripts:** Generate bash/zsh/fish completions

### See Also

- `std.os` — `os.args()` provides raw args, `os.exit()` for exit codes
- `type.error-types` — `CliError` uses standard error pattern
