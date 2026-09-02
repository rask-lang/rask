// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! The format-spec grammar for string interpolation — `{expr:spec}`.
//!
//! One definition, shared by everything that reads a `{...}`: the parser
//! deciding whether the braces are an interpolation at all, and the formatters
//! deciding what to do with the spec. They used to disagree. The parser
//! accepted any run of alphanumerics as "plausibly a spec", so
//! `"{\"k\": 1}"` — a one-pair JSON body — parsed as the expression `"k"` with
//! ` 1` as its spec, printed `k`, and dropped the rest without a word.
//!
//! Grammar (std.fmt/S1): `[[fill]align][0][width][.precision][type]`.

/// Where the padding goes when the rendered value is shorter than the width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
    Center,
}

impl Align {
    /// Codegen passes the alignment as a small integer; keep the mapping here
    /// so the C runtime and the interpreter can't drift apart.
    pub fn as_code(self) -> i64 {
        match self {
            Align::Left => 0,
            Align::Right => 1,
            Align::Center => 2,
        }
    }
}

/// The trailing type token — what the value renders as before padding (S3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecType {
    /// No token: `to_string()`.
    Display,
    /// `debug` — `debug()` (G4).
    Debug,
    /// `x` / `X` — hex, lower or upper.
    Hex { upper: bool },
    /// `b` — binary.
    Binary,
    /// `o` — octal.
    Octal,
    /// `e` — scientific.
    Exp,
}

impl SpecType {
    /// Numeric base for the integer tokens, `None` for the rest.
    pub fn base(self) -> Option<u32> {
        match self {
            SpecType::Hex { .. } => Some(16),
            SpecType::Binary => Some(2),
            SpecType::Octal => Some(8),
            _ => None,
        }
    }
}

/// A parsed `{:spec}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatSpec {
    pub fill: char,
    pub align: Option<Align>,
    /// 0 means "no width given" — nothing to pad to.
    pub width: usize,
    pub precision: Option<usize>,
    pub ty: SpecType,
}

impl Default for FormatSpec {
    fn default() -> Self {
        FormatSpec { fill: ' ', align: None, width: 0, precision: None, ty: SpecType::Display }
    }
}

impl FormatSpec {
    /// Nothing to do beyond `to_string()`.
    pub fn is_plain(&self) -> bool {
        *self == FormatSpec::default()
    }

    /// Alignment to apply, defaulting the way the value's own shape wants:
    /// text reads left, numbers read right.
    pub fn effective_align(&self, numeric: bool) -> Align {
        self.align.unwrap_or(if numeric { Align::Right } else { Align::Left })
    }
}

/// How a spec crosses from desugar into the backends: as five constants on a
/// `__fmt` call, not as text. Both backends decode it with [`FormatSpec::decode`],
/// so neither has to re-parse a spec string at runtime (std.fmt/CM5).
pub const ALIGN_DEFAULT: i64 = -1;
/// `precision` slot when the spec gave none.
pub const NO_PRECISION: i64 = -1;

impl SpecType {
    pub fn as_code(self) -> i64 {
        match self {
            SpecType::Display => 0,
            SpecType::Debug => 1,
            SpecType::Hex { upper: false } => 2,
            SpecType::Hex { upper: true } => 3,
            SpecType::Binary => 4,
            SpecType::Octal => 5,
            SpecType::Exp => 6,
        }
    }

    pub fn from_code(code: i64) -> SpecType {
        match code {
            1 => SpecType::Debug,
            2 => SpecType::Hex { upper: false },
            3 => SpecType::Hex { upper: true },
            4 => SpecType::Binary,
            5 => SpecType::Octal,
            6 => SpecType::Exp,
            _ => SpecType::Display,
        }
    }
}

impl FormatSpec {
    /// `(type, width, precision, align, fill)` — the `__fmt` argument list.
    pub fn encode(&self) -> (i64, i64, i64, i64, char) {
        (
            self.ty.as_code(),
            self.width as i64,
            self.precision.map_or(NO_PRECISION, |p| p as i64),
            self.align.map_or(ALIGN_DEFAULT, Align::as_code),
            self.fill,
        )
    }

    pub fn decode(ty: i64, width: i64, precision: i64, align: i64, fill: char) -> FormatSpec {
        FormatSpec {
            fill,
            align: match align {
                0 => Some(Align::Left),
                1 => Some(Align::Right),
                2 => Some(Align::Center),
                _ => None,
            },
            width: width.max(0) as usize,
            precision: if precision < 0 { None } else { Some(precision as usize) },
            ty: SpecType::from_code(ty),
        }
    }
}

/// Parse a format spec. `None` means the text isn't a spec at all — which, for
/// the parser, means the braces around it were never an interpolation.
pub fn parse_spec(spec: &str) -> Option<FormatSpec> {
    let chars: Vec<char> = spec.chars().collect();
    let mut out = FormatSpec::default();
    let mut pos = 0;

    let align_of = |c: char| match c {
        '<' => Some(Align::Left),
        '>' => Some(Align::Right),
        '^' => Some(Align::Center),
        _ => None,
    };

    // `[[fill]align]` — the two-char form first, so `0>` reads as fill `0`
    // rather than a zero-fill flag followed by a stray `>`.
    if chars.len() >= 2 {
        if let Some(a) = align_of(chars[1]) {
            out.fill = chars[0];
            out.align = Some(a);
            pos = 2;
        }
    }
    if pos == 0 {
        if let Some(a) = chars.first().copied().and_then(align_of) {
            out.align = Some(a);
            pos = 1;
        }
    }

    // A leading `0` on the width is the zero-fill flag. Not in the S1 grammar
    // as written, but the spec's own example — `format("0x{:08X}", 0xDEAD)` —
    // only produces `0x0000DEAD` if it means something.
    if out.align.is_none() && chars.get(pos) == Some(&'0') {
        out.fill = '0';
        out.align = Some(Align::Right);
        pos += 1;
    }

    // `[width]`
    let width_start = pos;
    while pos < chars.len() && chars[pos].is_ascii_digit() {
        pos += 1;
    }
    if pos > width_start {
        let digits: String = chars[width_start..pos].iter().collect();
        out.width = digits.parse().ok()?;
    }

    // `[.precision]`
    if chars.get(pos) == Some(&'.') {
        pos += 1;
        let prec_start = pos;
        while pos < chars.len() && chars[pos].is_ascii_digit() {
            pos += 1;
        }
        if pos == prec_start {
            return None; // A `.` with no digits isn't a precision.
        }
        let digits: String = chars[prec_start..pos].iter().collect();
        out.precision = Some(digits.parse().ok()?);
    }

    // `[type]` — the rest, and it has to be a token we know.
    let token: String = chars[pos..].iter().collect();
    out.ty = match token.as_str() {
        "" => SpecType::Display,
        "debug" => SpecType::Debug,
        "x" => SpecType::Hex { upper: false },
        "X" => SpecType::Hex { upper: true },
        "b" => SpecType::Binary,
        "o" => SpecType::Octal,
        "e" => SpecType::Exp,
        _ => return None,
    };

    Some(out)
}

/// The specs the formatters actually understand. Anything else means the
/// braces were never an interpolation.
pub fn is_valid_spec(spec: &str) -> bool {
    parse_spec(spec).is_some()
}

/// Split `expr:spec` at the colon that separates them, ignoring colons nested
/// inside brackets or a string literal. `None` means there's no spec.
pub fn split_spec(inner: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (i, c) in inner.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            _ if in_string => {}
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ':' if depth == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

/// Pad `s` out to `width` characters. Shared by the interpreter and the
/// compile-time constant folder so they can't render the same spec differently;
/// the C runtime carries a matching copy for the native path.
pub fn pad(s: &str, width: usize, align: Align, fill: char) -> String {
    let len = s.chars().count();
    if len >= width {
        return s.to_string();
    }
    let padding = width - len;
    match align {
        Align::Left => {
            let mut out = s.to_string();
            out.extend(std::iter::repeat(fill).take(padding));
            out
        }
        Align::Right => {
            let mut out: String = std::iter::repeat(fill).take(padding).collect();
            out.push_str(s);
            out
        }
        Align::Center => {
            let left = padding / 2;
            let mut out: String = std::iter::repeat(fill).take(left).collect();
            out.push_str(s);
            out.extend(std::iter::repeat(fill).take(padding - left));
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_and_tokens() {
        assert_eq!(parse_spec("").unwrap(), FormatSpec::default());
        assert_eq!(parse_spec("debug").unwrap().ty, SpecType::Debug);
        assert_eq!(parse_spec("x").unwrap().ty, SpecType::Hex { upper: false });
        assert_eq!(parse_spec("X").unwrap().ty, SpecType::Hex { upper: true });
        assert_eq!(parse_spec("b").unwrap().ty, SpecType::Binary);
        assert_eq!(parse_spec("o").unwrap().ty, SpecType::Octal);
        assert_eq!(parse_spec("e").unwrap().ty, SpecType::Exp);
    }

    #[test]
    fn precision() {
        let s = parse_spec(".2").unwrap();
        assert_eq!(s.precision, Some(2));
        assert_eq!(s.width, 0);
    }

    #[test]
    fn width_align_fill() {
        let s = parse_spec(">10").unwrap();
        assert_eq!((s.align, s.width, s.fill), (Some(Align::Right), 10, ' '));
        let s = parse_spec("0>10").unwrap();
        assert_eq!((s.align, s.width, s.fill), (Some(Align::Right), 10, '0'));
        let s = parse_spec("<10").unwrap();
        assert_eq!(s.align, Some(Align::Left));
        let s = parse_spec("^7").unwrap();
        assert_eq!(s.align, Some(Align::Center));
    }

    #[test]
    fn zero_fill_flag() {
        let s = parse_spec("08X").unwrap();
        assert_eq!((s.fill, s.align, s.width, s.ty), (
            '0',
            Some(Align::Right),
            8,
            SpecType::Hex { upper: true },
        ));
    }

    #[test]
    fn combined() {
        let s = parse_spec(">10.2").unwrap();
        assert_eq!((s.align, s.width, s.precision), (Some(Align::Right), 10, Some(2)));
    }

    /// #506: a one-pair JSON body must not read as a spec.
    #[test]
    fn rejects_non_specs() {
        assert!(parse_spec(" 1").is_none());
        assert!(parse_spec("1,\"y\":2").is_none());
        assert!(parse_spec("hello").is_none());
        assert!(parse_spec(".").is_none());
        assert!(parse_spec("10.").is_none());
        assert!(parse_spec("xy").is_none());
    }

    #[test]
    fn padding() {
        assert_eq!(pad("hi", 5, Align::Right, ' '), "   hi");
        assert_eq!(pad("hi", 5, Align::Left, ' '), "hi   ");
        assert_eq!(pad("hi", 6, Align::Center, '-'), "--hi--");
        assert_eq!(pad("toolong", 3, Align::Right, ' '), "toolong");
        // Width counts characters, not bytes.
        assert_eq!(pad("é", 3, Align::Right, ' '), "  é");
    }
}
