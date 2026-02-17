// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Rich terminal formatter for diagnostics.
//!
//! Produces multi-line, color-coded error output similar to Rust/Flix:
//!
//! ```text
//! error[E0308]: mismatched types
//!   --> main.rk:10:25
//!    |
//! 10 |     const result: string = calculate()
//!    |                   ------   ^^^^^^^^^^^ expected `string`, found `i32`
//!    |                   |
//!    |                   expected due to this type annotation
//!    |
//!    = note: these types have no automatic conversion
//!    = help: you can convert using `.to_string()` method
//! ```

use colored::Colorize;

use rask_ast::LineMap;

use crate::{Diagnostic, Help, LabelStyle, Severity};

/// Formats diagnostics for terminal output.
pub struct DiagnosticFormatter<'a> {
    source: &'a str,
    file_name: Option<&'a str>,
    line_map: LineMap,
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
        }
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
        let file = self.file_name.unwrap_or("<source>");
        let first_label = diagnostic.labels.first().unwrap();
        let (_, col) = self.offset_to_line_col(first_label.span.start);
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
                out.push_str(&format!(
                    "{} {} {}: {}\n",
                    " ".repeat(primary_gutter_width + 1),
                    "=".cyan(),
                    "fix".green().bold(),
                    fix
                ));
            }
            if let Some(ref why) = diagnostic.why {
                out.push_str(&format!(
                    "{} {} {}: {}\n",
                    " ".repeat(primary_gutter_width + 1),
                    "=".cyan(),
                    "why".cyan().bold(),
                    why
                ));
            }
        } else if let Some(ref help) = diagnostic.help {
            self.format_help(out, help, primary_gutter_width);
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
            let (line, col) = self.offset_to_line_col(suggestion.span.start);
            let source_line = self.get_line(line);
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
            let (line_num, col_start) = self.offset_to_line_col(label.span.start);
            let (end_line, col_end) = self.offset_to_line_col(label.span.end);

            // For multi-line spans, just annotate the start line
            let effective_col_end = if end_line == line_num {
                col_end
            } else {
                let line_text = self.get_line(line_num).unwrap_or("");
                line_text.len() + 1
            };

            let entry = lines_map.entry(line_num).or_insert_with(|| {
                let text = self.get_line(line_num).unwrap_or("").to_string();
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

    /// Convert byte offset to (line, col), both 1-based.
    fn offset_to_line_col(&self, offset: usize) -> (usize, usize) {
        let (line, col) = self.line_map.offset_to_line_col(offset);
        (line as usize, col as usize)
    }

    /// Get source line text by 1-based line number.
    fn get_line(&self, line_num: usize) -> Option<&str> {
        self.line_map.line_text(self.source, line_num as u32)
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
