<!-- id: std.terminal -->
<!-- status: decided -->
<!-- summary: ANSI styling as a value, terminal detection and size, colour gated automatically on tty -->
<!-- depends: stdlib/strings.md, stdlib/fmt.md, stdlib/os.md -->

# Terminal

ANSI styling and terminal properties.

A style is a value you build once and apply where you print. It knows whether the
output is a terminal, so a program that pipes to a file doesn't spray escape codes
into it — which is the bug every "colour your output" helper ships with.

## Styling

<!-- test: skip -->
```rask
const ERROR = terminal.Style.new().color(.red).bold()
const PATH  = terminal.Style.new().color(.cyan)

println(format("{}: {} could not be read", ERROR.apply("error"), PATH.apply(file)))
```

<!-- test: skip -->
```rask
Style.new() -> Style
style.color(c: Color) -> Style       // foreground
style.on(c: Color) -> Style          // background
style.bold() -> Style
style.dim() -> Style
style.italic() -> Style
style.underline() -> Style
style.apply(s: string) -> string     // wrap in escapes, or return s unchanged
```

`Color` is an enum, so adding a colour doesn't add a method:

<!-- test: parse -->
```rask
enum Color {
    Black, Red, Green, Yellow, Blue, Magenta, Cyan, White,
    Bright(Color),
    Rgb(r: u8, g: u8, b: u8),
    Ansi(u8),
}
```

## Detection and Size

<!-- test: skip -->
```rask
terminal.is_tty() -> bool
terminal.width() -> u16?             // columns, none when not a terminal
terminal.height() -> u16?
terminal.set_color(mode: ColorMode)  // .auto (default), .always, .never
```

<!-- test: parse -->
```rask
enum ColorMode { Auto, Always, Never }
```

## Core Rules

| Rule | Description |
|------|-------------|
| **T1: Style is a value** | Styling is one type with chainable settings and one `apply`, not a family of module functions. `terminal.bold(terminal.blue(s))` reads inside-out and grows a name per colour; a `Style` reads left to right and is built once |
| **T2: Colour is gated automatically** | `apply` returns the string unchanged when the output isn't a terminal. Callers never write `if terminal.is_tty()` — that check being the caller's job is why so much tooling writes escape codes into log files |
| **T3: Auto means tty, `NO_COLOR` and `TERM`** | Under `.auto`, colour is on when stdout is a tty, `NO_COLOR` is unset, and `TERM` isn't `dumb`. `set_color` overrides it, which is what a `--color` flag wires to |
| **T4: Width is a probe** | `width()` returns `T?`. No terminal means no answer, not an error worth a branch (`std.api/SD3`). Pipe output has no width and that's normal |
| **T5: No cursor control** | No cursor movement, alternate screen, raw mode, or key handling. Those are what a TUI library is, and it belongs in a package |

## Styling by hand

`apply` is the escape hatch's opposite — when you need the codes themselves,
`style.codes()` gives the prefix and suffix, and honours the gate:

<!-- test: skip -->
```rask
let (start, end) = ERROR.codes()
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Not a tty | T2 | `apply` returns the input; `width()`/`height()` return `none` |
| `NO_COLOR` set to anything | T3 | Colour off, even when a tty |
| `set_color(.always)` while piping | T3 | Colour on — this is `--color=always` for piping into a pager |
| Nested styles | T1 | Inner `apply` runs first; the outer style's reset ends the run. Reapply the outer style after an inner span if you need it to continue |
| `Rgb` on a 16-colour terminal | — | Emitted as-is. Downsampling would need a terminal capability database |
| Terminal resized after start | T4 | `width()` re-reads each call, so it reflects the resize |
| Style applied to text with a newline | — | Escapes wrap the whole string. Some terminals lose background colour across lines — style per line for a coloured background |
| Windows console | T3 | Virtual terminal processing enabled on first use; `.auto` reports false if that fails |

---

## Appendix (non-normative)

### Rationale

**T1 (a value, not fifteen functions):** the earlier sketch had `terminal.red`,
`terminal.green`, `terminal.bold` and the rest as module-level functions, which is
about eighteen names for one idea and blows the module budget (`std.api/SD1`) on a
module that does almost nothing. It also composes badly: `terminal.bold(terminal.blue(s))`
puts the styling in reverse order from how you'd say it, and every combination is
built fresh at every call site.

Naming the colours as enum variants instead of methods keeps `Style` small and puts
named colours and RGB through one door — with methods you'd need `.red()` *and*
`.rgb()`, two mechanisms for one setting.

**T2 (automatic gating):** this is the actual bug. `is_tty()` existing as a public
function means the check is the caller's responsibility, and it gets forgotten at
one call site in ten — which is how escape codes end up in CI logs and redirected
output. Putting the check inside `apply` makes the common path correct with nothing
written, and `set_color` covers the case where the program knows better.

**T5 (no TUI):** cursor control, raw mode and input handling are a coherent library
with real opinions about event loops and rendering. The stdlib line is protocols and
formats (`std.stdlib` README), and ANSI styling is a format. A full-screen
application framework isn't.

### See Also

- `std.fmt` — `width()` here is terminal columns; `s.width()` there is text columns. Both feed table layout
- `std.cli` — help output uses this for headings and error markers
- `std.os` — environment variables behind `NO_COLOR` and `TERM`
