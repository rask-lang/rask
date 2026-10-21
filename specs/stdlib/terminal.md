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
import terminal.{Style, Color}

const ERROR = Style.new().color(Color.Red).bold()
const PATH  = Style.new().color(Color.Cyan)

println(format("{}: {} could not be read", ERROR.apply("error"), PATH.apply(file)))
```

<!-- surface -->
```rask
struct Style { }

extend Style {
    func new() -> Style
    func color(self, c: Color) -> Style       // foreground
    func on(self, c: Color) -> Style          // background
    func bold(self) -> Style
    func dim(self) -> Style
    func italic(self) -> Style
    func underline(self) -> Style
    func apply(self, s: string) -> string     // wrap in escapes, or return s unchanged
    func codes(self) -> (string, string)      // (start, reset); see "Styling by hand"
}
```

`Color` is an enum, so adding a colour doesn't add a method. The first eight are
the base palette, the `Bright` eight its second half, `Rgb` is 24-bit and `Ansi`
indexes the 256-colour table:

<!-- surface -->
```rask
enum Color {
    Black, Red, Green, Yellow, Blue, Magenta, Cyan, White,
    BrightBlack, BrightRed, BrightGreen, BrightYellow,
    BrightBlue, BrightMagenta, BrightCyan, BrightWhite,
    Rgb(r: u8, g: u8, b: u8),
    Ansi(u8),
}
```

## Detection and Size

<!-- surface -->
```rask
extend terminal {
    func is_tty() -> bool
    func width() -> u16?             // columns, none when not a terminal
    func height() -> u16?
    func set_color(mode: ColorMode) -> void  // Auto until set
}

enum ColorMode { Auto, Always, Never }
```

## Core Rules

| Rule | Description |
|------|-------------|
| **T1: Style is a value** | Styling is one type with chainable settings and one `apply`, not a family of module functions. `terminal.bold(terminal.blue(s))` reads inside-out and grows a name per colour; a `Style` reads left to right and is built once |
| **T2: Colour is gated automatically** | `apply` returns the string unchanged when the output isn't a terminal. Callers never write `if terminal.is_tty()` — that check being the caller's job is why so much tooling writes escape codes into log files |
| **T3: Auto means tty, `NO_COLOR` and `TERM`** | Under `ColorMode.Auto`, colour is on when stdout is a tty, `NO_COLOR` is unset or empty, and `TERM` isn't `dumb`. It's asked again at every `apply`, so a program that changes its environment is seen. `set_color` overrides it for the whole process, which is what a `--color` flag wires to |
| **T4: Width is a probe** | `width()` returns `T?`. No terminal means no answer, not an error worth a branch (`std.api/SD3`). Pipe output has no width and that's normal |
| **T5: No cursor control** | No cursor movement, alternate screen, raw mode, or key handling. Those are what a TUI library is, and it belongs in a package |

## Styling by hand

`apply` is the escape hatch's opposite — when you need the codes themselves,
`style.codes()` gives the escape that starts the style and the one that resets it,
and honours the gate: both are empty when colour is off, and when the style sets
nothing.

<!-- test: skip -->
```rask
let (start, reset) = ERROR.codes()
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Not a tty | T2 | `apply` returns the input; `width()`/`height()` return `none` |
| `NO_COLOR` set to anything but the empty string | T3 | Colour off, even when a tty. An empty `NO_COLOR` counts as unset, as [no-color.org](https://no-color.org) specifies |
| `set_color(ColorMode.Always)` while piping | T3 | Colour on — this is `--color=always` for piping into a pager |
| A style that sets nothing | T2 | `apply` returns the input; `codes()` is two empty strings |
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

**Bright colours are variants, not a wrapper:** an earlier draft had `Bright(Color)`.
A variant holding its own enum inline has no finite size, so it would need
`Heap<Color>` (`type.enums/E6`) — a linear box to say "light red". It also let you
write `Bright(Rgb(..))` and `Bright(Bright(Red))`, which mean nothing. Eight more
names is the honest size of the palette.

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
