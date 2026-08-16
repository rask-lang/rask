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

        // Line 2: --> file:line:col
        let first = &annotated[0];
        let file = self.name_of(diagnostic.labels.first().map_or(0, |l| l.span.file_id));
        let first_label = diagnostic.labels.first().unwrap();
        let (_, col) = self.offset_to_line_col(first_label.span.start, first_label.span.file_id);
        out.push_str(&format!(
            "  {} {}:{}:{}\n",
            "-->".blue(),
            file,
            first.line_num,
            col
        ));

        // Calculate gutter width from max line number
        let max_line = annotated.last().map(|a| a.line_num).unwrap_or(1);
        let gutter_width = max_line.to_string().len().max(2);

        // Render each annotated line
        let mut prev_line_num: Option<usize> = None;
        for annotated_line in &annotated {
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

        // Notes
        for note in &diagnostic.notes {
            out.push_str(&format!(
                "{} {} {}: {}\n",
                " ".repeat(primary_gutter_width + 1),
                "=".cyan(),
                "note".cyan().bold(),
                note
            ));
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
    ///     = fix: x truncate to u8   // bit-preserving
    ///   x saturate to u8   // clamps
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
        for (i, line) in text.split('\n').enumerate() {
            if i == 0 {
                out.push_str(&format!(
                    "{} {} {}: {}\n",
                    " ".repeat(gutter_width + 1),
                    "=".cyan(),
                    label,
                    line
                ));
            } else {
                out.push_str(&format!("{}{}\n", continuation, line.trim_start()));
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
        let mut lines_map: std::collections::BTreeMap<usize, AnnotatedLine> =
            std::collections::BTreeMap::new();

        for label in &diagnostic.labels {
            // A span that runs past its file can't be pointed at. Rendering it
            // anyway invented a location: an error raised while checking a
            // stdlib body carried that file's byte offset but the user's
            // file_id, and came out as `examples/19_unsafe.rk:152:767` on a
            // 151-line file, quoting a blank line. Drop the label and let the
            // message stand on its own rather than send someone to a line that
            // isn't there.
            let (text, _) = self.file_of(label.span.file_id);
            if label.span.start > text.len() {
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

            let entry = lines_map.entry(line_num).or_insert_with(|| {
                let text = self.get_line(line_num, label.span.file_id).unwrap_or("").to_string();
                AnnotatedLine {
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
