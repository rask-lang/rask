// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Rich terminal formatter for diagnostics.
//!
//! Produces multi-line, color-coded error output similar to Rust/Flix:
//!
//! ```text
//! error[E0308]: mismatched types
//!   --> main.rk:10:25
//!    |
//! 10 |     let result: string = calculate()
//!    |                   ------   ^^^^^^^^^^^ expected `string`, found `i32`
//!    |                   |
//!    |                   expected due to this type annotation
//!    |
//!    = note: these types have no automatic conversion
//!    = help: you can convert using `.to_string()` method
//! ```

use colored::Colorize;

use rask_ast::LineMap;

use crate::source_map::SourceMap;
use crate::{Diagnostic, Help, LabelStyle, Severity};

/// How wide a diagnostic is allowed to get.
///
/// The conventional terminal, and the number every other tool assumes. The
/// explanatory lines used to ignore it entirely: `= why:` is a paragraph, and
/// it went out as one line however long it ran — E0835's is 441 characters, so
/// in an 80-column terminal it arrived as five and a half unbroken rows with
/// the words landing wherever. It's the same text either way; this decides
/// where it breaks instead of leaving that to the window.
const TERMINAL_WIDTH: usize = 80;

/// Split into the pieces a wrap may not break apart.
///
/// Words, except that anything in backticks is one piece however many spaces
/// it contains. A diagnostic's backticks hold the code you are being told to
/// write — `type Id = … with (Equal, Hashable)` — and a suggestion broken
/// across a line break is one you can't read off and can't copy. Whole or on
/// its own line.
///
/// An unclosed backtick takes the rest of the text with it, which is the same
/// answer: don't guess where a code span ends.
fn unbreakable_pieces(text: &str) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut chars = text.chars().peekable();
    let mut current = String::new();

    while let Some(ch) = chars.next() {
        if ch.is_whitespace() {
            if !current.is_empty() {
                pieces.push(std::mem::take(&mut current));
            }
            continue;
        }

        current.push(ch);
        if ch == '`' {
            for inner in chars.by_ref() {
                current.push(inner);
                if inner == '`' {
                    break;
                }
            }
        }
    }

    if !current.is_empty() {
        pieces.push(current);
    }
    pieces
}

/// Greedy word wrap, to a width in characters.
///
/// Counts characters rather than bytes: an em-dash is one column and three
/// bytes, and the explanatory lines are full of them. A piece longer than the
/// budget — a path, a long code span — goes on its own line rather than being
/// cut.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let width = width.max(20);
    let mut lines = Vec::new();
    let mut current = String::new();

    for piece in unbreakable_pieces(text) {
        let would_be = if current.is_empty() {
            piece.chars().count()
        } else {
            current.chars().count() + 1 + piece.chars().count()
        };

        if !current.is_empty() && would_be > width {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(&piece);
    }

    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Formats diagnostics for terminal output.
pub struct DiagnosticFormatter<'a> {
    source: &'a str,
    file_name: Option<&'a str>,
    line_map: LineMap,
    /// Set for a multi-file build. Each label then resolves through its span's
    /// `file_id` instead of against the single source above.
    sources: Option<&'a SourceMap>,
}

/// A source line with its labels.
struct AnnotatedLine {
    /// Which file this line is in. Labels used to be keyed by line number
    /// alone, so a diagnostic naming two files rendered both under the first
    /// one's header — and where the two line numbers happened to match, one
    /// label landed on the other file's source text.
    file_id: u16,
    line_num: usize,
    text: String,
    annotations: Vec<Annotation>,
}

struct Annotation {
    col_start: usize,
    col_end: usize,
    style: LabelStyle,
    message: Option<String>,
}

impl<'a> DiagnosticFormatter<'a> {
    pub fn new(source: &'a str) -> Self {
        let line_map = LineMap::new(source);
        Self {
            source,
            file_name: None,
            line_map,
            sources: None,
        }
    }

    /// Resolve each label through its span's `file_id`.
    ///
    /// Without this a package's diagnostics all render against one file, so a
    /// span from any other lands at an arbitrary offset — the reason errors
    /// showed up on the wrong file at columns past the end of the line.
    pub fn with_sources(mut self, sources: &'a SourceMap) -> Self {
        self.sources = Some(sources);
        self
    }

    pub fn with_file_name(mut self, name: &'a str) -> Self {
        self.file_name = Some(name);
        self
    }

    pub fn format(&self, diagnostic: &Diagnostic) -> String {
        let mut out = String::new();

        // Line 1: severity[code]: message
        self.format_header(&mut out, diagnostic);

        if diagnostic.labels.is_empty() {
            // No source context, just print notes/help
            self.format_footer(&mut out, diagnostic);
            return out;
        }

        // Group labels by source line
        let annotated = self.collect_annotated_lines(diagnostic);

        if annotated.is_empty() {
            self.format_footer(&mut out, diagnostic);
            return out;
        }

        // Line 2: --> file:line:col — the primary label, which is what the
        // error is about. This used to take the line from the first *rendered*
        // line and the column from the first label, so a secondary label above
        // the primary put the wrong line in the header: E0808 ("cannot push
        // inside `with`") carets the push but pointed at the `with` two lines
        // up, and an editor jumping there landed on the wrong statement.
        // `labels` is non-empty here, so `primary_span` always answers.
        let anchor = diagnostic.primary_span().unwrap_or(diagnostic.labels[0].span);

        // Calculate gutter width from max line number
        let max_line = annotated.iter().map(|a| a.line_num).max().unwrap_or(1);
        let gutter_width = max_line.to_string().len().max(2);

        // One snippet per file, the one the error is *about* first. A
        // diagnostic that names two files — a dependency's declaration and the
        // consumer's, say — used to print every line under the primary's
        // header, so the other file's lines looked like they came from a file
        // they aren't in.
        let mut file_order: Vec<u16> = vec![anchor.file_id];
        for a in &annotated {
            if !file_order.contains(&a.file_id) {
                file_order.push(a.file_id);
            }
        }

        for file_id in file_order {
            let lines: Vec<&AnnotatedLine> =
                annotated.iter().filter(|a| a.file_id == file_id).collect();
            if lines.is_empty() {
                continue;
            }

            let (line, col) = if file_id == anchor.file_id {
                self.offset_to_line_col(anchor.start, anchor.file_id)
            } else {
                (lines[0].line_num, 1)
            };
            out.push_str(&format!(
                "  {} {}:{}:{}\n",
                "-->".blue(),
                self.name_of(file_id),
                line,
                col
            ));

            let mut prev_line_num: Option<usize> = None;
            for annotated_line in lines {
                // Gap indicator for non-consecutive lines
                if let Some(prev) = prev_line_num {
                    if annotated_line.line_num > prev + 1 {
                        out.push_str(&format!(
                            "{} {}\n",
                            " ".repeat(gutter_width),
                            "...".blue()
                        ));
                    }
                }

                // Empty pipe line before first source line
                if prev_line_num.is_none() {
                    out.push_str(&format!(
                        "{} {}\n",
                        " ".repeat(gutter_width + 1),
                        "|".blue()
                    ));
                }

                // Source line: NN | code
                out.push_str(&format!(
                    "{:>width$} {} {}\n",
                    annotated_line.line_num.to_string().blue().bold(),
                    "|".blue(),
                    annotated_line.text,
                    width = gutter_width + 1,
                ));

                // Annotation lines beneath
                self.format_annotations(&mut out, annotated_line, gutter_width);

                prev_line_num = Some(annotated_line.line_num);
            }
        }

        self.format_footer(&mut out, diagnostic);

        out
    }

    fn format_header(&self, out: &mut String, diagnostic: &Diagnostic) {
        let severity_str = match diagnostic.severity {
            Severity::Error => "error".red().bold(),
            Severity::Warning => "warning".yellow().bold(),
            Severity::Note => "note".blue().bold(),
        };

        if let Some(ref code) = diagnostic.code {
            out.push_str(&format!(
                "{}[{}]: {}\n",
                severity_str,
                code.0.clone().red().bold(),
                diagnostic.message.bold()
            ));
        } else {
            out.push_str(&format!("{}: {}\n", severity_str, diagnostic.message.bold()));
        }
    }

    fn format_footer(&self, out: &mut String, diagnostic: &Diagnostic) {
        let primary_gutter_width = 2;

        // Notes. Same shape as fix and why, so the same wrap: a note is prose
        // too, and one ran to 96 characters before this went through the
        // shared path.
        for note in &diagnostic.notes {
            Self::push_labelled(out, primary_gutter_width, &"note".cyan().bold().to_string(), 4, note);
        }

        // Fix/why supersede help when present
        if diagnostic.fix.is_some() || diagnostic.why.is_some() {
            if let Some(ref fix) = diagnostic.fix {
                Self::push_labelled(out, primary_gutter_width, &"fix".green().bold().to_string(), 3, fix);
            }
            if let Some(ref why) = diagnostic.why {
                Self::push_labelled(out, primary_gutter_width, &"why".cyan().bold().to_string(), 3, why);
            }
        } else if let Some(ref help) = diagnostic.help {
            self.format_help(out, help, primary_gutter_width);
        }
    }

    /// `= label: text`, with continuation lines indented under the text.
    ///
    /// A few fixes offer several alternatives and separate them with newlines.
    /// Emitting the string as-is dropped those lines out of the gutter
    /// entirely, so the alternatives read as stray source rather than as part
    /// of the message:
    ///
    /// ```text
    ///     = fix: x.wrap<u8>()   // bit-preserving
    ///   x.clamp<u8>()   // clamps
    /// ```
    ///
    /// `label_width` is the label's visible width — `.green().bold()` wraps it
    /// in escape codes, so its byte length is not what lines up on screen.
    fn push_labelled(
        out: &mut String,
        gutter_width: usize,
        label: &str,
        label_width: usize,
        text: &str,
    ) {
        // gutter + " = " + label + ": "
        let continuation = " ".repeat(gutter_width + 1 + 2 + label_width + 2);
        let width = TERMINAL_WIDTH.saturating_sub(continuation.chars().count());

        let mut first = true;
        for line in text.split('\n') {
            for piece in wrap_words(line.trim_start(), width) {
                if first {
                    out.push_str(&format!(
                        "{} {} {}: {}\n",
                        " ".repeat(gutter_width + 1),
                        "=".cyan(),
                        label,
                        piece
                    ));
                    first = false;
                } else {
                    out.push_str(&format!("{}{}\n", continuation, piece));
                }
            }
        }
    }

    fn format_help(&self, out: &mut String, help: &Help, gutter_width: usize) {
        out.push_str(&format!(
            "{} {} {}: {}\n",
            " ".repeat(gutter_width + 1),
            "=".cyan(),
            "help".cyan().bold(),
            help.message
        ));

        // Show code suggestion if available
        if let Some(ref suggestion) = help.suggestion {
            let (line, col) = self.offset_to_line_col(suggestion.span.start, suggestion.span.file_id);
            let source_line = self.get_line(line, suggestion.span.file_id);
            if let Some(source_line) = source_line {
                // Show the suggested replacement
                let prefix = &source_line[..col.saturating_sub(1).min(source_line.len())];
                let span_len = suggestion.span.end.saturating_sub(suggestion.span.start);
                let suffix_start = (col - 1 + span_len).min(source_line.len());
                let suffix = &source_line[suffix_start..];

                out.push_str(&format!(
                    "{} {}\n",
                    " ".repeat(gutter_width + 1),
                    "|".blue()
                ));
                out.push_str(&format!(
                    "{:>width$} {} {}{}{}\n",
                    line.to_string().blue().bold(),
                    "|".blue(),
                    prefix,
                    suggestion.replacement.green(),
                    suffix,
                    width = gutter_width,
                ));

                // Show tildes under the replacement
                let tilde_len = suggestion.replacement.len();
                out.push_str(&format!(
                    "{} {} {}{}\n",
                    " ".repeat(gutter_width + 1),
                    "|".blue(),
                    " ".repeat(col.saturating_sub(1)),
                    "~".repeat(tilde_len).green(),
                ));
            }
        }
    }

    fn collect_annotated_lines(&self, diagnostic: &Diagnostic) -> Vec<AnnotatedLine> {
        let mut lines_map: std::collections::BTreeMap<(u16, usize), AnnotatedLine> =
            std::collections::BTreeMap::new();

        for label in &diagnostic.labels {
            // Only resolve a span against a file that was actually registered
            // for its id. A caller with no SourceMap has one file and every
            // span belongs to it, so that case still resolves as before; a
            // caller with a map means a span whose id isn't in it came from
            // somewhere this report can't see, and guessing produced
            // `examples/19_unsafe.rk:152:767` on a 151-line file.
            if self.sources.is_some_and(|m| m.get(label.span.file_id).is_none()) {
                continue;
            }
            let (line_num, col_start) = self.offset_to_line_col(label.span.start, label.span.file_id);
            let (end_line, col_end) = self.offset_to_line_col(label.span.end, label.span.file_id);

            // For multi-line spans, just annotate the start line
            let effective_col_end = if end_line == line_num {
                col_end
            } else {
                let line_text = self.get_line(line_num, label.span.file_id).unwrap_or("");
                line_text.len() + 1
            };

            let entry = lines_map.entry((label.span.file_id, line_num)).or_insert_with(|| {
                let text = self.get_line(line_num, label.span.file_id).unwrap_or("").to_string();
                AnnotatedLine {
                    file_id: label.span.file_id,
                    line_num,
                    text,
                    annotations: Vec::new(),
                }
            });

            entry.annotations.push(Annotation {
                col_start,
                col_end: effective_col_end.max(col_start + 1), // At least 1 char wide
                style: label.style,
                message: label.message.clone(),
            });
        }

        lines_map.into_values().collect()
    }

    fn format_annotations(
        &self,
        out: &mut String,
        annotated_line: &AnnotatedLine,
        gutter_width: usize,
    ) {
        // Sort annotations: primary first, then by column
        let mut sorted: Vec<&Annotation> = annotated_line.annotations.iter().collect();
        sorted.sort_by(|a, b| {
            a.style
                .cmp_priority()
                .cmp(&b.style.cmp_priority())
                .then(a.col_start.cmp(&b.col_start))
        });

        // Build the underline characters
        let line_len = annotated_line.text.len() + 10;
        let mut underline = vec![' '; line_len];
        let mut messages: Vec<(usize, LabelStyle, &str)> = Vec::new();

        for ann in &sorted {
            let ch = match ann.style {
                LabelStyle::Primary => '^',
                LabelStyle::Secondary => '-',
            };

            for i in (ann.col_start - 1)..ann.col_end.saturating_sub(1).min(line_len) {
                underline[i] = ch;
            }

            if let Some(ref msg) = ann.message {
                messages.push((ann.col_end.saturating_sub(1), ann.style, msg));
            }
        }

        // Render underline with inline message for the rightmost annotation
        let underline_str: String = underline.iter().collect::<String>().trim_end().to_string();
        if underline_str.is_empty() {
            return;
        }

        // Color the underline
        let colored_underline = color_underline(&underline_str);

        // If there's only one annotation (or messages are simple), put message inline
        if messages.len() <= 1 {
            if let Some((_, style, msg)) = messages.first() {
                let styled_msg = match style {
                    LabelStyle::Primary => msg.red().bold().to_string(),
                    LabelStyle::Secondary => msg.blue().to_string(),
                };
                out.push_str(&format!(
                    "{} {} {} {}\n",
                    " ".repeat(gutter_width + 1),
                    "|".blue(),
                    colored_underline,
                    styled_msg,
                ));
            } else {
                out.push_str(&format!(
                    "{} {} {}\n",
                    " ".repeat(gutter_width + 1),
                    "|".blue(),
                    colored_underline,
                ));
            }
        } else {
            // Multiple annotations: underline first, then messages on separate lines
            out.push_str(&format!(
                "{} {} {}\n",
                " ".repeat(gutter_width + 1),
                "|".blue(),
                colored_underline,
            ));

            // Render messages with connector pipes, bottom-up for readability
            for (col, style, msg) in messages.iter().rev() {
                let styled_msg = match style {
                    LabelStyle::Primary => msg.red().bold().to_string(),
                    LabelStyle::Secondary => msg.blue().to_string(),
                };
                let pipe = match style {
                    LabelStyle::Primary => "|".red().bold().to_string(),
                    LabelStyle::Secondary => "|".blue().to_string(),
                };
                out.push_str(&format!(
                    "{} {} {}{} {}\n",
                    " ".repeat(gutter_width + 1),
                    "|".blue(),
                    " ".repeat(col.saturating_sub(1)),
                    pipe,
                    styled_msg,
                ));
            }
        }
    }

    /// The text and line index a span should be read against.
    fn file_of(&self, file_id: u16) -> (&str, &LineMap) {
        match self.sources.and_then(|m| m.get(file_id)) {
            Some(f) => (f.text.as_str(), &f.line_map),
            None => (self.source, &self.line_map),
        }
    }

    /// Convert byte offset to (line, col), both 1-based.
    fn offset_to_line_col(&self, offset: usize, file_id: u16) -> (usize, usize) {
        let (_, line_map) = self.file_of(file_id);
        let (line, col) = line_map.offset_to_line_col(offset);
        (line as usize, col as usize)
    }

    /// Get source line text by 1-based line number.
    fn get_line(&self, line_num: usize, file_id: u16) -> Option<&str> {
        let (text, line_map) = self.file_of(file_id);
        line_map.line_text(text, line_num as u32)
    }

    /// The name to print in the `-->` header for this span.
    fn name_of(&self, file_id: u16) -> &str {
        self.sources
            .and_then(|m| m.get(file_id))
            .map(|f| f.name.as_str())
            .or(self.file_name)
            .unwrap_or("<source>")
    }
}

impl LabelStyle {
    fn cmp_priority(&self) -> u8 {
        match self {
            LabelStyle::Primary => 0,
            LabelStyle::Secondary => 1,
        }
    }
}

/// Color the underline characters (^ in red, - in blue).
fn color_underline(s: &str) -> String {
    let mut result = String::new();
    let mut current_char = None;
    let mut run = String::new();

    for ch in s.chars() {
        let kind = match ch {
            '^' => Some('^'),
            '-' => Some('-'),
            _ => None,
        };

        if kind != current_char && !run.is_empty() {
            result.push_str(&flush_run(&run, current_char));
            run.clear();
        }
        run.push(ch);
        current_char = kind;
    }

    if !run.is_empty() {
        result.push_str(&flush_run(&run, current_char));
    }

    result
}

fn flush_run(run: &str, kind: Option<char>) -> String {
    match kind {
        Some('^') => run.red().bold().to_string(),
        Some('-') => run.blue().to_string(),
        _ => run.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Diagnostic;
    use rask_ast::Span;

    /// The `--> file:line:col` header names the primary label.
    ///
    /// It used to take the line from the first *rendered* line and the column
    /// from the first label in the list, which are two different things. A
    /// diagnostic whose secondary label sits above its primary — E0808, "cannot
    /// push inside `with`", underlines the push and points back at the `with` —
    /// printed the secondary's line with the primary's column, so jumping to it
    /// landed on the wrong statement.
    #[test]
    fn header_points_at_the_primary_label_not_the_earliest_one() {
        let source = "func f() {\n    with v[0] as item {\n        v.push(3)\n    }\n}\n";
        let push = source.find("v.push(3)").unwrap();
        let with = source.find("with v[0]").unwrap();

        let diag = Diagnostic::error("cannot push `v` inside `with` block")
            .with_primary(Span::new(push, push + 9), "push not allowed inside with block")
            .with_secondary(Span::new(with, with + 9), "element borrowed here");

        let out = DiagnosticFormatter::new(source).with_file_name("t.rk").format(&diag);
        let header = out.lines().find(|l| l.contains("-->")).expect("a header line");
        assert!(
            header.contains("t.rk:3:9"),
            "header should name the push on line 3, got: {header}"
        );
    }
}
