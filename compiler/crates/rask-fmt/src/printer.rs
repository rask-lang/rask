// SPDX-License-Identifier: (MIT OR Apache-2.0)

use rask_ast::decl::*;
use rask_ast::expr::*;
use rask_ast::stmt::*;
use rask_ast::Span;

use crate::comment::{self, CommentList};
use crate::config::FormatConfig;

pub struct Printer<'a> {
    output: String,
    indent: usize,
    source: &'a str,
    comments: CommentList,
    config: &'a FormatConfig,
    /// End of the declaration being printed. The pending-comment cursor is
    /// global and flushed opportunistically, and the block-end drain accepted any
    /// comment indented at least as deep as the block — which is every comment in
    /// every *later* declaration too. One function's body swallowed the comments
    /// out of the next one (#805).
    decl_end: usize,
    /// End of the innermost statement or expression being printed. Same problem
    /// one level down: a comment written in the *next* `if` was drained into the
    /// previous one's body, because both are indented the same.
    block_end: usize,
}

impl<'a> Printer<'a> {
    pub fn new(source: &'a str, comments: CommentList, config: &'a FormatConfig) -> Self {
        Self {
            output: String::new(),
            indent: 0,
            source,
            comments,
            config,
            decl_end: source.len(),
            block_end: source.len(),
        }
    }

    pub fn finish(mut self) -> String {
        // Emit any remaining comments
        for c in self.comments.take_rest() {
            self.output.push_str(&c.text);
            self.output.push('\n');
        }
        // Ensure trailing newline
        if !self.output.ends_with('\n') {
            self.output.push('\n');
        }
        self.output
    }

    // --- Helpers ---

    fn emit(&mut self, s: &str) {
        self.output.push_str(s);
    }

    fn emit_newline(&mut self) {
        self.output.push('\n');
    }

    fn emit_indent(&mut self) {
        let spaces = self.indent * self.config.indent_width;
        for _ in 0..spaces {
            self.output.push(' ');
        }
    }

    fn emit_blank_line(&mut self) {
        if self.output.ends_with("\n\n") {
            return;
        }
        if !self.output.ends_with('\n') {
            self.output.push('\n');
        }
        self.output.push('\n');
    }

    fn source_text(&self, span: Span) -> &str {
        &self.source[span.start..span.end]
    }

    /// Check if there's a blank line in the source immediately before `pos`,
    /// scanning backward through whitespace only. Returns true if 2+ newlines
    /// are found before hitting non-whitespace content.
    fn has_blank_line_before(&self, pos: usize) -> bool {
        let bytes = self.source.as_bytes();
        let mut newlines = 0;
        let mut p = pos;
        while p > 0 {
            p -= 1;
            match bytes[p] {
                b'\n' => newlines += 1,
                b' ' | b'\t' | b'\r' => {}
                _ => break,
            }
        }
        newlines >= 2
    }

    /// Take comments before `pos`, emit them with proper blank lines.
    /// Returns the comments so caller can check blank line between last comment and next item.
    fn emit_comments_before(&mut self, pos: usize, emit_blank_before_first: bool) -> Vec<comment::Comment> {
        let comments = self.comments.take_before(pos);
        for (i, c) in comments.iter().enumerate() {
            if i == 0 && emit_blank_before_first && self.has_blank_line_before(c.span.start) {
                self.emit_blank_line();
            } else if i > 0 && self.has_blank_line_before(c.span.start) {
                self.emit_blank_line();
            }
            self.emit_indent();
            self.output.push_str(&c.text);
            self.emit_newline();
        }
        comments
    }

    /// Try to emit a trailing comment on the same line as the code.
    /// Returns true if a trailing comment was emitted.
    ///
    /// The test is on the comment's own line, not on the span it came after: a
    /// statement's span runs to the newline that terminates it, and that newline
    /// is *past* the trailing comment, so comparing against the span end said
    /// "not on this line" every time. Every trailing comment in the tree was
    /// getting moved onto a line of its own, which changes what it annotates —
    /// `let a = 4  // one` documents `a`, and above the next line it reads as
    /// documenting that instead (#801).
    fn try_emit_trailing_comment(&mut self, span_end: usize) -> bool {
        let Some(c) = self.comments.peek_next() else { return false };
        let (cstart, cend) = (c.span.start, c.span.end);
        if cstart >= self.source.len() {
            return false;
        }
        let bytes = self.source.as_bytes();

        // A comment with nothing but whitespace before it on its line is a
        // standalone comment and keeps its own line.
        let mut line_start = cstart;
        while line_start > 0 && bytes[line_start - 1] != b'\n' {
            line_start -= 1;
        }
        let before = &self.source[line_start..cstart];
        if before.trim().is_empty() {
            return false;
        }

        if cstart >= span_end {
            // The span stopped before the comment, so "same line" is the gap.
            let mut content_end = span_end;
            while content_end > 0 && bytes[content_end - 1].is_ascii_whitespace() {
                content_end -= 1;
            }
            if self.source[content_end..cstart].contains('\n') {
                return false;
            }
        } else if cend < span_end && !self.only_whitespace_and_comments(cend, span_end) {
            // The span swallowed the comment. Nothing but whitespace and further
            // comments may follow it inside the span — real code after it means
            // the comment is partway through a multi-line statement and stays
            // where it is. Further *comments* are fine: a statement's span runs
            // past any standalone comments that follow it, so requiring bare
            // whitespace here made the trailing comment move on the second pass
            // whenever another comment came after it.
            return false;
        }

        let Some(c) = self.comments.advance() else { return false };
        // Preserve the original spacing, with a two-space minimum.
        let spaces = (before.len() - before.trim_end().len()).max(2);
        for _ in 0..spaces {
            self.output.push(' ');
        }
        self.output.push_str(&c.text);
        true
    }

    /// Whether `source[from..to]` holds nothing but whitespace and comments.
    fn only_whitespace_and_comments(&self, from: usize, to: usize) -> bool {
        let bytes = self.source.as_bytes();
        let mut i = from;
        while i < to {
            if bytes[i].is_ascii_whitespace() {
                i += 1;
            } else if bytes[i] == b'/' && i + 1 < to && bytes[i + 1] == b'/' {
                while i < to && bytes[i] != b'\n' {
                    i += 1;
                }
            } else if bytes[i] == b'/' && i + 1 < to && bytes[i + 1] == b'*' {
                i += 2;
                while i + 1 < to && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(to);
            } else {
                return false;
            }
        }
        true
    }

    /// Get the indentation level (in spaces) of a source position by scanning back to line start.
    fn source_indent_at(&self, pos: usize) -> usize {
        let bytes = self.source.as_bytes();
        let mut p = pos;
        while p > 0 && bytes[p - 1] != b'\n' {
            p -= 1;
        }
        let mut spaces = 0;
        while p + spaces < pos && bytes[p + spaces] == b' ' {
            spaces += 1;
        }
        spaces
    }

    /// Emit every pending comment that starts before `pos` and stands on a line of
    /// its own, each at the current indent.
    ///
    /// A struct field and an enum variant aren't statements, so nothing was
    /// flushing the comments written among them. They stayed pending until the
    /// *next declaration* emitted them — below the closing brace, where a comment
    /// about a variant reads as a comment about whatever comes next. `stdlib/os.rk`
    /// lost four doc comments out of `Metadata` that way (#805).
    fn emit_standalone_comments_before(&mut self, pos: usize) {
        loop {
            let Some(c) = self.comments.peek_next() else { break };
            if c.span.start >= pos {
                break;
            }
            if !self.comment_is_standalone(c.span.start) {
                break;
            }
            let start = c.span.start;
            let Some(c) = self.comments.advance() else { break };
            if self.has_blank_line_before(start) {
                self.emit_blank_line();
            }
            self.emit_indent();
            self.output.push_str(&c.text);
            self.emit_newline();
        }
    }

    /// Emit the next pending comment inline when it's a trailing comment starting
    /// before `limit`.
    ///
    /// `try_emit_trailing_comment` tests "same line as this span", which a member
    /// with no span of its own can't answer. Source order does: members are printed
    /// in order, so the first trailing comment still pending inside the
    /// declaration belongs to the member just printed.
    fn try_emit_trailing_comment_before(&mut self, limit: usize) -> bool {
        let Some(c) = self.comments.peek_next() else { return false };
        let start = c.span.start;
        if start >= limit || start >= self.source.len() {
            return false;
        }
        if self.comment_is_standalone(start) {
            return false;
        }
        let before = self.line_prefix_before(start);
        let spaces = (before.len() - before.trim_end().len()).max(2);
        let Some(c) = self.comments.advance() else { return false };
        for _ in 0..spaces {
            self.output.push(' ');
        }
        self.output.push_str(&c.text);
        true
    }

    /// The text between the start of a comment's line and the comment itself.
    fn line_prefix_before(&self, pos: usize) -> &str {
        let bytes = self.source.as_bytes();
        let mut line_start = pos;
        while line_start > 0 && bytes[line_start - 1] != b'\n' {
            line_start -= 1;
        }
        &self.source[line_start..pos]
    }

    /// A comment with nothing but whitespace before it on its line.
    fn comment_is_standalone(&self, pos: usize) -> bool {
        self.line_prefix_before(pos).trim().is_empty()
    }

    /// Consume trailing comments that belong to the current block (at current indent or deeper).
    fn consume_trailing_block_comments(&mut self) {
        let min_indent = self.indent * self.config.indent_width;
        loop {
            let c = match self.comments.peek_next() {
                Some(c) => c,
                None => break,
            };
            // Never past the node being printed: a comment beyond it belongs to a
            // later statement or declaration, whatever its indent.
            if c.span.start >= self.decl_end.min(self.block_end) {
                break;
            }
            let comment_indent = self.source_indent_at(c.span.start);
            if comment_indent < min_indent {
                break;
            }
            let Some(c) = self.comments.advance() else { break; };
            if self.has_blank_line_before(c.span.start) {
                self.emit_blank_line();
            }
            self.emit_indent();
            self.output.push_str(&c.text);
            self.emit_newline();
        }
    }

    /// Strip type params from names (parser includes `<T, U>` in names).
    fn strip_type_params<'b>(&self, name: &'b str) -> &'b str {
        if let Some(idx) = name.find('<') {
            &name[..idx]
        } else {
            name
        }
    }

    /// Convert parser-normalized types back to Rask syntax.
    /// E.g., `Result<i32, string>` → `i32 or string`.
    fn format_type(&self, ty: &str) -> String {
        // The parser normalizes `void` to `()` (type.primitives/P6), which isn't
        // a type anyone can write — printing it back gave "`()` is not a type"
        // on the formatter's own output (#805).
        if ty == "()" {
            return "void".to_string();
        }
        if let Some(inner) = ty.strip_prefix("Result<") {
            if let Some(inner) = inner.strip_suffix('>') {
                // The top-level comma is the one separating value from error.
                // Only angle brackets were counted, so a tuple value type split
                // at its own comma: `(string, string) or E` came back out as
                // `(string or string), E`, which doesn't parse (#805).
                let mut depth = 0;
                for (i, ch) in inner.char_indices() {
                    match ch {
                        '<' | '(' | '[' => depth += 1,
                        '>' | ')' | ']' => depth -= 1,
                        ',' if depth == 0 => {
                            let ok_ty = inner[..i].trim();
                            let err_ty = inner[i + 1..].trim();
                            return format!(
                                "{} or {}",
                                self.format_type(ok_ty),
                                self.format_type(err_ty)
                            );
                        }
                        _ => {}
                    }
                }
            }
        }
        // A closure type has two spellings and the parser stores the `func(…)`
        // one, so that's what comes back out. Rewriting it to `|T| -> R` was
        // wrong for the zero-parameter case — `||` is the or-operator token, so
        // `|| -> Big` doesn't lex — and it's the minority spelling anyway.
        if let Some(rest) = ty.strip_prefix("func(") {
            let mut depth = 1;
            for (i, ch) in rest.char_indices() {
                match ch {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            let params = self.format_type_list(&rest[..i]);
                            let after = rest[i + 1..].trim();
                            return match after.strip_prefix("->").map(str::trim) {
                                // An omitted return type is stored as `()` too,
                                // so writing it back adds an arrow the source
                                // never had.
                                None | Some("()") => format!("func({})", params),
                                Some(ret_ty) => {
                                    format!("func({}) -> {}", params, self.format_type(ret_ty))
                                }
                            };
                        }
                    }
                    _ => {}
                }
            }
        }
        // A generic argument is a type too. `Receiver<void>` came back out as
        // `Receiver<()>`, which doesn't parse — the surface-spelling fix only
        // looked at the whole string (#805).
        if let Some(open) = ty.find('<') {
            if let Some(inner) = ty.strip_suffix('>') {
                let base = &ty[..open];
                let args = self.format_type_list(&inner[open + 1..]);
                return format!("{}<{}>", base, args);
            }
        }
        ty.to_string()
    }

    /// A comma-separated type list, split at the top level only.
    fn format_type_list(&self, list: &str) -> String {
        if list.trim().is_empty() {
            return String::new();
        }
        let mut parts = Vec::new();
        let mut depth = 0;
        let mut start = 0;
        for (i, ch) in list.char_indices() {
            match ch {
                '<' | '(' | '[' => depth += 1,
                '>' | ')' | ']' => depth -= 1,
                ',' if depth == 0 => {
                    parts.push(self.format_type(list[start..i].trim()));
                    start = i + 1;
                }
                _ => {}
            }
        }
        parts.push(self.format_type(list[start..].trim()));
        parts.join(", ")
    }

    /// The offset of the `extern` keyword when this declaration came from a block.
    fn extern_block_start(decl: &Decl) -> Option<usize> {
        match &decl.kind {
            DeclKind::Extern(e) => e.block_start,
            _ => None,
        }
    }

    // --- File ---

    pub fn format_file(&mut self, decls: &[Decl]) {
        let mut is_first = true;
        let mut prev_was_import = false;

        // `extern "C" { … }` flattens into one declaration per member, so the
        // block has to be put back together here. Members carry the offset of
        // their own `extern` keyword, which groups them without merging two
        // blocks that were written apart (#805).
        let mut i = 0;
        while i < decls.len() {
            let decl = &decls[i];
            let block = Self::extern_block_start(decl);
            let mut run = 1;
            if block.is_some() {
                while i + run < decls.len() && Self::extern_block_start(&decls[i + run]) == block {
                    run += 1;
                }
            }
            let members = &decls[i..i + run];
            i += run;

            let is_import = matches!(decl.kind, DeclKind::Import(_));

            // Emit comments before this decl (with blank lines from source).
            // For a block, the bound is the `extern` keyword — a comment written
            // inside the braces belongs to the member it precedes, not above the
            // block.
            let decl_start = block.unwrap_or(decl.span.start);
            let comments = self.emit_comments_before(decl_start, !is_first);

            // Blank line between previous decl/comment and this decl
            if !is_first && comments.is_empty() {
                if !(prev_was_import && is_import) {
                    self.emit_blank_line();
                }
            }

            // Blank line between last comment and decl (if source had one)
            if !comments.is_empty() && self.has_blank_line_before(decl_start) {
                self.emit_blank_line();
            } else if !is_first && comments.is_empty() {
                // Already handled above
            }

            if block.is_some() {
                self.format_extern_block(members);
            } else {
                self.decl_end = decl.span.end;
                self.format_decl(decl);
                self.decl_end = self.source.len();
            }
            if !self.output.ends_with('\n') {
                self.emit_newline();
            }

            is_first = false;
            prev_was_import = is_import;
        }
    }

    /// Print the members of one `extern "C" { … }` back inside its braces.
    ///
    /// Each member keeps its own span, so a comment written beside one lands
    /// beside it rather than below the block — which is what the flattened form
    /// could not express.
    fn format_extern_block(&mut self, members: &[Decl]) {
        let abi = match &members[0].kind {
            DeclKind::Extern(e) => e.abi.clone(),
            _ => return,
        };
        self.emit_indent();
        self.emit("extern \"");
        self.emit(&abi);
        self.emit("\" {");
        self.emit_newline();
        self.indent += 1;
        let mut first = true;
        for m in members {
            let DeclKind::Extern(e) = &m.kind else { continue };
            let comments = self.emit_comments_before(m.span.start, !first);
            if !first && comments.is_empty() && self.has_blank_line_before(m.span.start) {
                self.emit_blank_line();
            }
            self.decl_end = m.span.end;
            self.format_extern_member(e);
            self.decl_end = self.source.len();
            self.emit_newline();
            first = false;
        }
        // A comment on the last line inside the braces belongs in here, not after.
        let close = self.source[members[members.len() - 1].span.end..]
            .find('}')
            .map(|off| members[members.len() - 1].span.end + off)
            .unwrap_or(self.source.len());
        self.emit_comments_before(close, false);
        self.indent -= 1;
        self.emit_indent();
        self.emit("}");
        self.emit_newline();
    }

    // --- Declarations ---

    fn format_decl(&mut self, decl: &Decl) {
        match &decl.kind {
            DeclKind::Fn(f) => self.format_fn_decl(f, false, false),
            DeclKind::Struct(s) => self.format_struct_decl(s, decl.span),
            DeclKind::Enum(e) => self.format_enum_decl(e, decl.span),
            DeclKind::Trait(t) => self.format_trait_decl(t),
            DeclKind::Impl(i) => self.format_impl_decl(i),
            DeclKind::Import(i) => self.format_import_decl(i),
            DeclKind::Export(e) => self.format_export_decl(e),
            DeclKind::Const(c) => self.format_const_decl(c),
            DeclKind::Test(t) => self.format_test_decl(t),
            DeclKind::Benchmark(b) => self.format_benchmark_decl(b),
            DeclKind::Extern(e) => self.format_extern_decl(e),
            DeclKind::Package(p) => self.format_package_decl(p),
            DeclKind::Union(u) => self.format_union_decl(u, decl.span),
            DeclKind::TypeAlias(t) => self.format_type_alias_decl(t),
            DeclKind::CImport(ci) => {
                self.emit("import c ");
                if ci.headers.len() == 1 {
                    self.emit(&format!("\"{}\"", ci.headers[0]));
                } else {
                    self.emit("{ ");
                    for (i, h) in ci.headers.iter().enumerate() {
                        if i > 0 { self.emit(", "); }
                        self.emit(&format!("\"{}\"", h));
                    }
                    self.emit(" }");
                }
                if ci.alias != "c" {
                    self.emit(&format!(" as {}", ci.alias));
                }
                if !ci.hiding.is_empty() {
                    self.emit(" hiding { ");
                    self.emit(&ci.hiding.join(", "));
                    self.emit(" }");
                }
                self.emit_newline();
            }
        }
    }

    fn format_fn_decl(&mut self, f: &FnDecl, is_method: bool, is_trait_decl: bool) {
        // A method inside an `extend` block is not a statement or an expression, so
        // without this its body's comment drain was bounded only by the whole
        // `extend` — and pulled the comments out of every method after it.
        // `stdlib/json.rk` collected seven of them into `JsonParser.new` (#805).
        let outer_block_end = self.block_end;
        if f.span.end > 0 {
            self.block_end = f.span.end;
        }
        self.format_fn_decl_inner(f, is_method, is_trait_decl);
        self.block_end = outer_block_end;
    }

    fn format_fn_decl_inner(&mut self, f: &FnDecl, is_method: bool, is_trait_decl: bool) {
        if !is_method {
            self.emit_indent();
        }

        for attr in &f.attrs {
            self.emit(&format!("@{attr}"));
            self.emit_newline();
            self.emit_indent();
        }

        if f.is_private {
            self.emit("private ");
        } else if f.is_pub {
            self.emit("public ");
        }
        if f.is_comptime {
            self.emit("comptime ");
        }
        if f.is_unsafe {
            self.emit("unsafe ");
        }
        self.emit("func ");
        let name = self.strip_type_params(&f.name);
        self.emit(name);

        if !f.type_params.is_empty() {
            self.emit("<");
            for (i, tp) in f.type_params.iter().enumerate() {
                if i > 0 {
                    self.emit(", ");
                }
                self.format_type_param(tp);
            }
            self.emit(">");
        }

        self.emit("(");
        for (i, param) in f.params.iter().enumerate() {
            if i > 0 {
                self.emit(", ");
            }
            self.format_param(param);
        }
        self.emit(")");

        if let Some(ref ret_ty) = f.ret_ty {
            self.emit(" -> ");
            let ty = self.format_type(ret_ty);
            self.emit(&ty);
        }

        // `using players: Pool<Player>` declares what the body reaches for
        // without naming it at the call site. The printer dropped the clause, so
        // the body then read a name nothing had declared (#805).
        for (i, clause) in f.context_clauses.iter().enumerate() {
            self.emit(if i == 0 { " using " } else { ", " });
            if clause.is_frozen {
                self.emit("frozen ");
            }
            if let Some(ref name) = clause.name {
                self.emit(name);
                self.emit(": ");
            }
            let ty = self.format_type(&clause.ty);
            self.emit(&ty);
        }

        if f.body.is_empty() && is_trait_decl {
            // Trait method declaration with no body — no braces
        } else if f.body.is_empty() && self.comments_within(f.span) {
            // A body that holds nothing but a comment. `{}` would drop the comment
            // out of the braces entirely — it escaped to column 0 below the
            // enclosing `extend` (#805).
            self.emit(" {");
            self.emit_newline();
            self.indent += 1;
            self.consume_trailing_block_comments();
            self.indent -= 1;
            self.emit_indent();
            self.emit("}");
        } else if f.body.is_empty() {
            // `{}` for a function body: that's what hand-written Rask uses (137
            // sites, 103 of them `func main() {}`), against `{ }` which appears
            // 378 times and only ever in `stdlib/`'s signature stubs. An empty
            // *type* body goes the other way, 47 to 4, and gets `{ }`.
            self.emit(" {}");
        } else {
            self.emit(" {");
            self.emit_newline();
            self.indent += 1;
            self.format_stmts(&f.body);
            self.indent -= 1;
            self.emit_indent();
            self.emit("}");
        }
    }

    fn format_type_param(&mut self, tp: &TypeParam) {
        if tp.is_comptime {
            self.emit("comptime ");
        }
        self.emit(&tp.name);
        if let Some(ref ct) = tp.comptime_type {
            self.emit(": ");
            self.emit(ct);
        }
        for (i, bound) in tp.bounds.iter().enumerate() {
            if i == 0 {
                self.emit(": ");
            } else {
                self.emit(" + ");
            }
            self.emit(bound);
        }
    }

    fn format_param(&mut self, param: &Param) {
        if param.name == "self" {
            if param.is_take {
                self.emit("take ");
            } else if param.is_mutate {
                self.emit("mutate ");
            }
            self.emit("self");
        } else {
            if param.is_take {
                self.emit("take ");
            } else if param.is_mutate {
                self.emit("mutate ");
            }
            self.emit(&param.name);
            if !param.ty.is_empty() {
                self.emit(": ");
                let ty = self.format_type(&param.ty);
                self.emit(&ty);
            }
            if let Some(ref default) = param.default {
                self.emit(" = ");
                self.format_expr(default);
            }
        }
    }

    fn format_struct_decl(&mut self, s: &StructDecl, span: Span) {
        self.emit_indent();

        for attr in &s.attrs {
            self.emit(&format!("@{attr}"));
            self.emit_newline();
            self.emit_indent();
        }

        if s.is_pub {
            self.emit("public ");
        }
        self.emit("struct ");
        let name = self.strip_type_params(&s.name);
        self.emit(name);

        if !s.type_params.is_empty() {
            self.emit("<");
            for (i, tp) in s.type_params.iter().enumerate() {
                if i > 0 {
                    self.emit(", ");
                }
                self.format_type_param(tp);
            }
            self.emit(">");
        }

        let source_is_multiline = self.source_text(span).contains('\n');
        let has_methods = !s.methods.is_empty();

        if !source_is_multiline && !has_methods && s.fields.is_empty() {
            self.emit(" { }");
        } else if !source_is_multiline && !has_methods && s.fields.len() <= 4 && self.struct_fields_fit_one_line(&s.fields) {
            // Inline style: struct Vec3 { x: f64, y: f64, z: f64 }
            self.emit(" { ");
            for (i, field) in s.fields.iter().enumerate() {
                if i > 0 {
                    self.emit(", ");
                }
                for attr in &field.attrs {
                    self.emit("@");
                    self.emit(attr);
                    self.emit(" ");
                }
                match field.visibility {
                    FieldVisibility::Private => self.emit("private "),
                    FieldVisibility::Public => self.emit("public "),
                    FieldVisibility::Package => {},
                }
                self.emit(&field.name);
                self.emit(": ");
                let ty = self.format_type(&field.ty);
                self.emit(&ty);
                if let Some(default) = &field.default {
                    self.emit(" = ");
                    self.format_expr(default);
                }
            }
            self.emit(" }");
        } else {
            // Multi-line style: no commas
            self.emit(" {");
            self.emit_newline();

            self.indent += 1;
            for (i, field) in s.fields.iter().enumerate() {
                // A trailing comment belongs to this field only if it comes before
                // the next one. Bounding by the declaration's end instead attached
                // the first pending comment to the first field, whatever line it
                // was really on — `Node { value, next // about next }` moved the
                // comment up onto `value`.
                let next_member = s
                    .fields
                    .get(i + 1)
                    .map(|f| f.name_span.start)
                    .unwrap_or(span.end);
                self.emit_standalone_comments_before(field.name_span.start);
                for attr in &field.attrs {
                    self.emit_indent();
                    self.emit("@");
                    self.emit(attr);
                    self.emit_newline();
                }
                self.emit_indent();
                match field.visibility {
                    FieldVisibility::Private => self.emit("private "),
                    FieldVisibility::Public => self.emit("public "),
                    FieldVisibility::Package => {},
                }
                self.emit(&field.name);
                self.emit(": ");
                let ty = self.format_type(&field.ty);
                self.emit(&ty);
                if let Some(default) = &field.default {
                    self.emit(" = ");
                    self.format_expr(default);
                }
                self.try_emit_trailing_comment_before(next_member);
                self.emit_newline();
            }
            if !s.methods.is_empty() {
                self.emit_newline();
                let mut first = true;
                for method in &s.methods {
                    if !first {
                        self.emit_blank_line();
                    }
                    self.emit_indent();
                    self.format_fn_decl(method, true, false);
                    self.emit_newline();
                    first = false;
                }
            }
            self.indent -= 1;
            self.emit_indent();
            self.emit("}");
        }
    }

    fn struct_fields_fit_one_line(&self, fields: &[Field]) -> bool {
        let est: usize = fields.iter().map(|f| {
            f.name.len() + 2 + f.ty.len() + match f.visibility {
                FieldVisibility::Private => 8,
                FieldVisibility::Public => 7,
                FieldVisibility::Package => 0,
            }
        }).sum::<usize>() + (fields.len().saturating_sub(1) * 2);
        est < 60
    }

    fn format_union_decl(&mut self, u: &UnionDecl, span: Span) {
        self.emit_indent();

        if u.is_pub {
            self.emit("public ");
        }
        self.emit("union ");
        self.emit(&u.name);

        let source_is_multiline = self.source_text(span).contains('\n');

        if !source_is_multiline && u.fields.is_empty() {
            self.emit(" { }");
        } else if !source_is_multiline && u.fields.len() <= 4 && self.struct_fields_fit_one_line(&u.fields) {
            self.emit(" { ");
            for (i, field) in u.fields.iter().enumerate() {
                if i > 0 {
                    self.emit(", ");
                }
                match field.visibility {
                    FieldVisibility::Private => self.emit("private "),
                    FieldVisibility::Public => self.emit("public "),
                    FieldVisibility::Package => {},
                }
                self.emit(&field.name);
                self.emit(": ");
                let ty = self.format_type(&field.ty);
                self.emit(&ty);
            }
            self.emit(" }");
        } else {
            self.emit(" {");
            self.emit_newline();

            self.indent += 1;
            for field in &u.fields {
                self.emit_indent();
                match field.visibility {
                    FieldVisibility::Private => self.emit("private "),
                    FieldVisibility::Public => self.emit("public "),
                    FieldVisibility::Package => {},
                }
                self.emit(&field.name);
                self.emit(": ");
                let ty = self.format_type(&field.ty);
                self.emit(&ty);
                self.emit_newline();
            }
            self.indent -= 1;
            self.emit_indent();
            self.emit("}");
        }
    }

    fn format_enum_decl(&mut self, e: &EnumDecl, span: Span) {
        self.emit_indent();

        // Enums were the one declaration whose attributes weren't printed, so
        // `@message` disappeared and the derived `message()` went with it — the
        // formatted file then failed to check on a call the source made (#805).
        for attr in &e.attrs {
            self.emit(&format!("@{attr}"));
            self.emit_newline();
            self.emit_indent();
        }

        if e.is_pub {
            self.emit("public ");
        }
        self.emit("enum ");
        let name = self.strip_type_params(&e.name);
        self.emit(name);

        if !e.type_params.is_empty() {
            self.emit("<");
            for (i, tp) in e.type_params.iter().enumerate() {
                if i > 0 {
                    self.emit(", ");
                }
                self.format_type_param(tp);
            }
            self.emit(">");
        }

        let source_is_multiline = self.source_text(span).contains('\n');
        let all_fieldless = e.variants.iter().all(|v| v.fields.is_empty());
        let has_methods = !e.methods.is_empty();

        if !source_is_multiline && !has_methods && e.variants.is_empty() {
            self.emit(" { }");
        } else if !source_is_multiline && !has_methods && all_fieldless && self.enum_variants_fit_one_line(&e.variants) {
            // Inline style: enum Dir { N, S, E, W }
            self.emit(" { ");
            for (i, variant) in e.variants.iter().enumerate() {
                if i > 0 {
                    self.emit(", ");
                }
                self.emit(&variant.name);
            }
            self.emit(" }");
        } else {
            // Multi-line style: no commas
            self.emit(" {");
            self.emit_newline();

            self.indent += 1;
            for (i, variant) in e.variants.iter().enumerate() {
                let next_member = e
                    .variants
                    .get(i + 1)
                    .map(|v| v.name_span.start)
                    .unwrap_or(span.end);
                self.emit_standalone_comments_before(variant.name_span.start);
                self.emit_indent();
                // A per-variant `@message("…")` is what the derived message
                // actually reads.
                for attr in &variant.attrs {
                    self.emit(&format!("@{attr}"));
                    self.emit_newline();
                    self.emit_indent();
                }
                self.emit(&variant.name);
                if !variant.fields.is_empty() {
                    let is_tuple = variant.fields.first().map_or(false, |f| {
                        f.name.starts_with('_') && f.name[1..].parse::<usize>().is_ok()
                            || f.name.parse::<usize>().is_ok()
                    });
                    if is_tuple {
                        self.emit("(");
                        for (i, field) in variant.fields.iter().enumerate() {
                            if i > 0 {
                                self.emit(", ");
                            }
                            let ty = self.format_type(&field.ty);
                            self.emit(&ty);
                        }
                        self.emit(")");
                    } else {
                        self.emit(" { ");
                        for (i, field) in variant.fields.iter().enumerate() {
                            if i > 0 {
                                self.emit(", ");
                            }
                            self.emit(&field.name);
                            self.emit(": ");
                            let ty = self.format_type(&field.ty);
                            self.emit(&ty);
                        }
                        self.emit(" }");
                    }
                }
                self.try_emit_trailing_comment_before(next_member);
                self.emit_newline();
            }
            if !e.methods.is_empty() {
                self.emit_newline();
                let mut first = true;
                for method in &e.methods {
                    if !first {
                        self.emit_blank_line();
                    }
                    self.emit_indent();
                    self.format_fn_decl(method, true, false);
                    self.emit_newline();
                    first = false;
                }
            }
            self.indent -= 1;
            self.emit_indent();
            self.emit("}");
        }
    }

    fn enum_variants_fit_one_line(&self, variants: &[Variant]) -> bool {
        let est: usize = variants.iter().map(|v| v.name.len()).sum::<usize>()
            + (variants.len().saturating_sub(1) * 2);
        est < 60
    }

    fn format_trait_decl(&mut self, t: &TraitDecl) {
        self.emit_indent();

        // Attributes, `unsafe`, `duck` and super-traits were all dropped. Losing
        // `duck` is the one that changes the program: a duck trait matches by
        // shape and a plain one has to be declared, so the conformance the
        // source relied on stopped existing (#805).
        for attr in &t.attrs {
            self.emit(&format!("@{attr}"));
            self.emit_newline();
            self.emit_indent();
        }

        if t.is_pub {
            self.emit("public ");
        }
        if t.is_unsafe {
            self.emit("unsafe ");
        }
        if t.is_duck {
            self.emit("duck ");
        }
        self.emit("trait ");
        self.emit(&t.name);
        for (i, sup) in t.super_traits.iter().enumerate() {
            self.emit(if i == 0 { ": " } else { ", " });
            self.emit(sup);
        }
        self.emit(" {");
        self.emit_newline();

        self.indent += 1;
        self.format_block_members(&t.methods, true);
        self.indent -= 1;
        self.emit_indent();
        self.emit("}");
    }

    /// The members of an `extend` or `trait` body.
    ///
    /// Comments between them belong where they were written. Nothing consumed
    /// them here, so a `///` on a method stayed unclaimed until the body's first
    /// statement picked it up — and the doc comment ended up *inside* the method
    /// it documented. Blank lines follow the source too, instead of one being
    /// inserted between every pair of members (#805).
    fn format_block_members(&mut self, methods: &[FnDecl], is_trait_decl: bool) {
        let mut is_first = true;
        for method in methods {
            let comments = self.emit_comments_before(method.span.start, !is_first);
            let blank_in_source = self.has_blank_line_before(method.span.start);
            if comments.is_empty() {
                if !is_first && blank_in_source {
                    self.emit_blank_line();
                }
            } else if blank_in_source {
                self.emit_blank_line();
            }
            self.emit_indent();
            self.format_fn_decl(method, true, is_trait_decl);
            self.emit_newline();
            is_first = false;
        }
    }

    fn format_impl_decl(&mut self, imp: &ImplDecl) {
        self.emit_indent();
        if imp.is_scoped {
            self.emit("scoped ");
        }
        self.emit("extend ");
        self.emit(&imp.target_ty);
        if !imp.trait_names.is_empty() {
            self.emit(" with ");
            self.emit(&imp.trait_names.join(", "));
        }
        if !imp.where_bounds.is_empty() {
            let clause: Vec<String> = imp.where_bounds.iter()
                .map(|tp| format!("{}: {}", tp.name, tp.bounds.join(" + ")))
                .collect();
            self.emit(" where ");
            self.emit(&clause.join(", "));
        }
        self.emit(" {");
        self.emit_newline();

        self.indent += 1;
        self.format_block_members(&imp.methods, false);
        self.indent -= 1;
        self.emit_indent();
        self.emit("}");
    }

    fn format_import_decl(&mut self, imp: &ImportDecl) {
        self.emit_indent();
        self.emit("import ");
        if imp.is_lazy {
            self.emit("lazy ");
        }
        self.emit(&imp.path.join("."));
        if imp.is_glob {
            self.emit(".*");
        }
        if let Some(ref alias) = imp.alias {
            self.emit(" as ");
            self.emit(alias);
        }
    }

    fn format_export_decl(&mut self, exp: &ExportDecl) {
        self.emit_indent();
        self.emit("export ");
        for (i, item) in exp.items.iter().enumerate() {
            if i > 0 {
                self.emit(", ");
            }
            self.emit(&item.path.join("."));
            if let Some(ref alias) = item.alias {
                self.emit(" as ");
                self.emit(alias);
            }
        }
    }

    fn format_type_alias_decl(&mut self, t: &TypeAliasDecl) {
        self.emit_indent();
        if t.is_pub {
            self.emit("public ");
        }
        if t.is_transparent {
            self.emit("type alias ");
        } else {
            self.emit("type ");
        }
        self.emit(&t.name);
        if !t.type_params.is_empty() {
            self.emit("<");
            for (i, tp) in t.type_params.iter().enumerate() {
                if i > 0 {
                    self.emit(", ");
                }
                self.emit(&tp.name);
            }
            self.emit(">");
        }
        self.emit(" = ");
        // The target is a type like any other, and printing it raw left the
        // parser's internal spellings in the output — `-> ()` where the source
        // said nothing at all (#805).
        let target = self.format_type(&t.target);
        self.emit(&target);
        if !t.with_traits.is_empty() {
            self.emit(" with (");
            for (i, trait_name) in t.with_traits.iter().enumerate() {
                if i > 0 {
                    self.emit(", ");
                }
                self.emit(trait_name);
            }
            self.emit(")");
        }
    }

    fn format_const_decl(&mut self, c: &ConstDecl) {
        self.emit_indent();
        if c.is_pub {
            self.emit("public ");
        }
        self.emit("const ");
        self.emit(&c.name);
        if let Some(ref ty) = c.ty {
            self.emit(": ");
            self.emit(ty);
        }
        self.emit(" = ");
        self.format_expr(&c.init);
    }

    fn format_test_decl(&mut self, t: &TestDecl) {
        self.emit_indent();
        if t.is_comptime {
            self.emit("comptime ");
        }
        self.emit("test \"");
        self.emit(&t.name);
        self.emit("\" {");
        self.emit_newline();

        self.indent += 1;
        self.format_stmts(&t.body);
        self.indent -= 1;
        self.emit_indent();
        self.emit("}");
    }

    fn format_benchmark_decl(&mut self, b: &BenchmarkDecl) {
        self.emit_indent();
        self.emit("benchmark \"");
        self.emit(&b.name);
        self.emit("\" {");
        self.emit_newline();

        self.indent += 1;
        self.format_stmts(&b.body);
        self.indent -= 1;
        self.emit_indent();
        self.emit("}");
    }

    fn format_package_decl(&mut self, p: &PackageDecl) {
        self.emit_indent();
        self.emit("package \"");
        self.emit(&p.name);
        self.emit("\" \"");
        self.emit(&p.version);
        self.emit("\"");

        let has_body = !p.metadata.is_empty()
            || !p.list_metadata.is_empty()
            || !p.deps.is_empty()
            || !p.features.is_empty()
            || !p.profiles.is_empty();

        if !has_body {
            return;
        }

        self.emit(" {");
        self.emit_newline();
        self.indent += 1;

        // Metadata (key: "value")
        for (key, value) in &p.metadata {
            self.emit_indent();
            self.emit(key);
            self.emit(": \"");
            self.emit(value);
            self.emit("\"");
            self.emit_newline();
        }

        // List metadata (key: ["a", "b"])
        for (key, values) in &p.list_metadata {
            self.emit_indent();
            self.emit(key);
            self.emit(": [");
            for (i, v) in values.iter().enumerate() {
                if i > 0 {
                    self.emit(", ");
                }
                self.emit("\"");
                self.emit(v);
                self.emit("\"");
            }
            self.emit("]");
            self.emit_newline();
        }

        // Dependencies
        for dep in &p.deps {
            self.format_dep_decl(dep);
        }

        // Features
        for feat in &p.features {
            self.format_feature_decl(feat);
        }

        // Profiles
        for prof in &p.profiles {
            self.format_profile_decl(prof);
        }

        self.indent -= 1;
        self.emit_indent();
        self.emit("}");
    }

    fn format_dep_decl(&mut self, dep: &DepDecl) {
        self.emit_indent();
        self.emit("dep \"");
        self.emit(&dep.name);
        self.emit("\"");

        if let Some(ref ver) = dep.version {
            self.emit(" \"");
            self.emit(ver);
            self.emit("\"");
        }

        let has_options = dep.path.is_some()
            || dep.git.is_some()
            || dep.branch.is_some()
            || !dep.with_features.is_empty()
            || dep.target.is_some()
            || !dep.allow.is_empty()
            || !dep.exclusive_selections.is_empty();

        if has_options {
            self.emit(" {");
            self.emit_newline();
            self.indent += 1;

            if let Some(ref path) = dep.path {
                self.emit_indent();
                self.emit("path: \"");
                self.emit(path);
                self.emit("\"");
                self.emit_newline();
            }
            if let Some(ref git) = dep.git {
                self.emit_indent();
                self.emit("git: \"");
                self.emit(git);
                self.emit("\"");
                self.emit_newline();
            }
            if let Some(ref branch) = dep.branch {
                self.emit_indent();
                self.emit("branch: \"");
                self.emit(branch);
                self.emit("\"");
                self.emit_newline();
            }
            if let Some(ref target) = dep.target {
                self.emit_indent();
                self.emit("target: \"");
                self.emit(target);
                self.emit("\"");
                self.emit_newline();
            }
            if !dep.with_features.is_empty() {
                self.emit_indent();
                self.emit("with: [");
                for (i, f) in dep.with_features.iter().enumerate() {
                    if i > 0 {
                        self.emit(", ");
                    }
                    self.emit("\"");
                    self.emit(f);
                    self.emit("\"");
                }
                self.emit("]");
                self.emit_newline();
            }
            if !dep.allow.is_empty() {
                self.emit_indent();
                self.emit("allow: [");
                for (i, a) in dep.allow.iter().enumerate() {
                    if i > 0 {
                        self.emit(", ");
                    }
                    self.emit("\"");
                    self.emit(a);
                    self.emit("\"");
                }
                self.emit("]");
                self.emit_newline();
            }
            for (key, val) in &dep.exclusive_selections {
                self.emit_indent();
                self.emit(key);
                self.emit(": \"");
                self.emit(val);
                self.emit("\"");
                self.emit_newline();
            }

            self.indent -= 1;
            self.emit_indent();
            self.emit("}");
        }
        self.emit_newline();
    }

    fn format_feature_decl(&mut self, feat: &FeatureDecl) {
        self.emit_indent();
        self.emit("feature \"");
        self.emit(&feat.name);
        self.emit("\"");

        if feat.exclusive {
            self.emit(" exclusive");
        }

        self.emit(" {");
        self.emit_newline();
        self.indent += 1;

        for dep in &feat.deps {
            self.format_dep_decl(dep);
        }

        for opt in &feat.options {
            self.emit_indent();
            self.emit("\"");
            self.emit(&opt.name);
            self.emit("\" {");
            self.emit_newline();
            self.indent += 1;
            for dep in &opt.deps {
                self.format_dep_decl(dep);
            }
            self.indent -= 1;
            self.emit_indent();
            self.emit("}");
            self.emit_newline();
        }

        if let Some(ref default) = feat.default {
            self.emit_indent();
            self.emit("default: \"");
            self.emit(default);
            self.emit("\"");
            self.emit_newline();
        }

        self.indent -= 1;
        self.emit_indent();
        self.emit("}");
        self.emit_newline();
    }

    fn format_profile_decl(&mut self, prof: &ProfileDecl) {
        self.emit_indent();
        self.emit("profile \"");
        self.emit(&prof.name);
        self.emit("\" {");
        self.emit_newline();
        self.indent += 1;

        for (key, value) in &prof.settings {
            self.emit_indent();
            self.emit(key);
            self.emit(": \"");
            self.emit(value);
            self.emit("\"");
            self.emit_newline();
        }

        self.indent -= 1;
        self.emit_indent();
        self.emit("}");
        self.emit_newline();
    }

    fn format_extern_decl(&mut self, e: &ExternDecl) {
        self.emit_indent();
        self.emit("extern \"");
        self.emit(&e.abi);
        self.emit("\" ");
        self.format_extern_signature(e);
    }

    /// One member of an `extern "C" { … }` — no `extern "C"` of its own, the
    /// block already said it.
    fn format_extern_member(&mut self, e: &ExternDecl) {
        self.emit_indent();
        self.format_extern_signature(e);
    }

    fn format_extern_signature(&mut self, e: &ExternDecl) {
        self.emit("func ");
        self.emit(&e.name);
        self.emit("(");
        for (i, param) in e.params.iter().enumerate() {
            if i > 0 {
                self.emit(", ");
            }
            self.format_param(param);
        }
        self.emit(")");
        if let Some(ref ret_ty) = e.ret_ty {
            self.emit(" -> ");
            let ty = self.format_type(ret_ty);
            self.emit(&ty);
        }
    }

    // --- Statements ---

    fn format_stmts(&mut self, stmts: &[Stmt]) {
        let mut is_first = true;

        for stmt in stmts {
            // Emit comments before this statement (with blank line detection)
            let comments = self.emit_comments_before(stmt.span.start, !is_first);

            // Blank line before statement (only if no comments emitted —
            // if comments were emitted, their blank line handling covers it)
            if !is_first && comments.is_empty() && self.has_blank_line_before(stmt.span.start) {
                self.emit_blank_line();
            }

            // Blank line between last comment and this statement
            if !comments.is_empty() && self.has_blank_line_before(stmt.span.start) {
                self.emit_blank_line();
            }

            self.emit_indent();
            self.format_stmt(stmt);
            // Try to emit a trailing comment on the same line
            self.try_emit_trailing_comment(stmt.span.end);
            if !self.output.ends_with('\n') {
                self.emit_newline();
            }

            is_first = false;
        }

        // Emit trailing comments inside this block (only if at current indent or deeper)
        self.consume_trailing_block_comments();
    }

    fn format_stmt(&mut self, stmt: &Stmt) {
        let outer = self.block_end;
        self.block_end = stmt.span.end;
        self.format_stmt_inner(stmt);
        self.block_end = outer;
    }

    fn format_stmt_inner(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Expr(expr) => {
                self.format_expr(expr);
            }
            StmtKind::Mut { name, name_span: _, ty, init } => {
                self.emit("mut ");
                self.emit(name);
                if let Some(ref ty) = ty {
                    self.emit(": ");
                    let t = self.format_type(ty);
                    self.emit(&t);
                }
                self.emit(" = ");
                self.format_expr(init);
            }
            StmtKind::MutTuple { patterns, init } => {
                self.emit("mut ");
                self.format_tuple_pat_list(patterns);
                self.emit(" = ");
                self.format_expr(init);
            }
            StmtKind::Let { name, name_span: _, ty, init } => {
                self.emit("let ");
                self.emit(name);
                if let Some(ref ty) = ty {
                    self.emit(": ");
                    let t = self.format_type(ty);
                    self.emit(&t);
                }
                self.emit(" = ");
                self.format_expr(init);
            }
            StmtKind::LetTuple { patterns, init } => {
                self.emit("let ");
                self.format_tuple_pat_list(patterns);
                self.emit(" = ");
                self.format_expr(init);
            }
            StmtKind::LetStruct { pattern, init, is_mut } => {
                self.emit(if *is_mut { "mut " } else { "let " });
                self.format_pattern(pattern);
                self.emit(" = ");
                self.format_expr(init);
            }
            StmtKind::Assign { target, value, op } => {
                self.format_expr(target);
                // A compound assignment is stored expanded — `i += 1` as
                // `i = i + 1` — so writing `value` out rewrote every `+=` in the
                // tree. `op` says which form was written; the right-hand side is
                // then the expansion's own right operand (#805).
                match (op, &value.kind) {
                    (Some(op), ExprKind::Binary { right, .. }) => {
                        self.emit(&format!(" {}= ", binop_str(op)));
                        self.format_expr(right);
                    }
                    _ => {
                        self.emit(" = ");
                        self.format_expr(value);
                    }
                }
            }
            StmtKind::Return(None) => {
                self.emit("return");
            }
            StmtKind::Return(Some(expr)) => {
                self.emit("return ");
                self.format_expr(expr);
            }
            StmtKind::Break { label, value } => {
                self.emit("break");
                if let Some(ref l) = label {
                    self.emit(" ");
                    self.emit(l);
                }
                if let Some(ref v) = value {
                    self.emit(" ");
                    self.format_expr(v);
                }
            }
            StmtKind::Continue(label) => {
                self.emit("continue");
                if let Some(ref l) = label {
                    self.emit(" ");
                    self.emit(l);
                }
            }
            StmtKind::While { label, cond, body } => {
                if let Some(l) = label {
                    self.emit(l);
                    self.emit(": ");
                }
                self.emit("while ");
                self.format_condition(cond);
                self.emit(" {");
                self.emit_newline();
                self.indent += 1;
                self.format_stmts(body);
                self.indent -= 1;
                self.emit_indent();
                self.emit("}");
            }
            StmtKind::WhileLet { label, pattern, expr, body } => {
                if let Some(l) = label {
                    self.emit(l);
                    self.emit(": ");
                }
                self.emit("while ");
                self.format_expr(expr);
                self.emit(" is ");
                self.format_pattern(pattern);
                self.emit(" {");
                self.emit_newline();
                self.indent += 1;
                self.format_stmts(body);
                self.indent -= 1;
                self.emit_indent();
                self.emit("}");
            }
            StmtKind::Loop { label, body } => {
                if let Some(ref l) = label {
                    self.emit(l);
                    self.emit(": ");
                }
                self.emit("loop {");
                self.emit_newline();
                self.indent += 1;
                self.format_stmts(body);
                self.indent -= 1;
                self.emit_indent();
                self.emit("}");
            }
            StmtKind::For { label, binding, mutate, iter, body } => {
                if let Some(ref l) = label {
                    self.emit(l);
                    self.emit(": ");
                }
                if *mutate {
                    self.emit("for mutate ");
                } else {
                    self.emit("for ");
                }
                match binding {
                    ForBinding::Single(name) => self.emit(name),
                    ForBinding::Tuple(names) => {
                        self.emit("(");
                        for (i, name) in names.iter().enumerate() {
                            if i > 0 { self.emit(", "); }
                            self.emit(name);
                        }
                        self.emit(")");
                    }
                }
                self.emit(" in ");
                self.format_expr(iter);
                self.emit(" {");
                self.emit_newline();
                self.indent += 1;
                self.format_stmts(body);
                self.indent -= 1;
                self.emit_indent();
                self.emit("}");
            }
            StmtKind::Ensure { body, else_handler } => {
                self.emit("ensure ");
                if body.len() == 1 && else_handler.is_none() {
                    self.format_stmt_inline(&body[0]);
                } else {
                    self.emit("{");
                    self.emit_newline();
                    self.indent += 1;
                    self.format_stmts(body);
                    self.indent -= 1;
                    self.emit_indent();
                    self.emit("}");
                    if let Some((param, handler)) = else_handler {
                        self.emit(" else |");
                        self.emit(param);
                        self.emit("| {");
                        self.emit_newline();
                        self.indent += 1;
                        self.format_stmts(handler);
                        self.indent -= 1;
                        self.emit_indent();
                        self.emit("}");
                    }
                }
            }
            StmtKind::Comptime(stmts) => {
                self.emit("comptime {");
                self.emit_newline();
                self.indent += 1;
                self.format_stmts(stmts);
                self.indent -= 1;
                self.emit_indent();
                self.emit("}");
            }
            StmtKind::ComptimeFor { binding, iter, body } => {
                self.emit("comptime for ");
                match binding {
                    ForBinding::Single(name) => self.emit(name),
                    ForBinding::Tuple(names) => {
                        self.emit("(");
                        for (i, name) in names.iter().enumerate() {
                            if i > 0 { self.emit(", "); }
                            self.emit(name);
                        }
                        self.emit(")");
                    }
                }
                self.emit(" in ");
                self.format_expr(iter);
                self.emit(" {");
                self.emit_newline();
                self.indent += 1;
                self.format_stmts(body);
                self.indent -= 1;
                self.emit_indent();
                self.emit("}");
            }
            StmtKind::Discard { name, .. } => {
                self.emit("discard ");
                self.emit(name);
            }
        }
    }

    fn format_tuple_pat_list(&mut self, pats: &[rask_ast::stmt::TuplePat]) {
        self.emit("(");
        for (i, pat) in pats.iter().enumerate() {
            if i > 0 {
                self.emit(", ");
            }
            self.format_tuple_pat(pat);
        }
        self.emit(")");
    }

    fn format_tuple_pat(&mut self, pat: &rask_ast::stmt::TuplePat) {
        match pat {
            rask_ast::stmt::TuplePat::Name(n) => self.emit(n),
            rask_ast::stmt::TuplePat::Wildcard => self.emit("_"),
            rask_ast::stmt::TuplePat::Nested(pats) => self.format_tuple_pat_list(pats),
        }
    }

    fn format_stmt_inline(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Expr(expr) => self.format_expr(expr),
            _ => self.format_stmt(stmt),
        }
    }

    // --- Expressions ---

    fn format_call_arg(&mut self, arg: &CallArg) {
        if let Some(ref name) = arg.name {
            self.emit(name);
            self.emit(": ");
        }
        match arg.mode {
            ArgMode::Mutate => self.emit("mutate "),
            ArgMode::Own => self.emit("own "),
            ArgMode::Default => {}
        }
        self.format_expr(&arg.expr);
    }

    fn format_expr(&mut self, expr: &Expr) {
        let outer = self.block_end;
        if expr.span.end > 0 {
            self.block_end = expr.span.end;
        }
        // A method chain or a struct literal written across lines, with a comment
        // beside one of them, is left exactly as written. Reflowing it deletes the
        // line the comment annotated, and the comment then lands wherever the
        // cursor happens to flush — `.filter(|n| n % 2 == 0)  // Keep evens` came
        // out as a bare `// Keep evens` above the whole chain (#805). Keeping the
        // comment attached is worth more than normalizing the layout.
        if matches!(expr.kind, ExprKind::MethodCall { .. } | ExprKind::StructLit { .. })
            && self.comments_within(expr.span)
            && self.source_text(expr.span).contains('\n')
        {
            self.emit_verbatim_consuming_comments(expr.span);
            self.block_end = outer;
            return;
        }
        self.format_expr_inner(expr, None);
        self.block_end = outer;
    }

    /// Emit the source for this node unchanged, dropping the comments inside it
    /// from the pending list so they aren't emitted a second time.
    fn emit_verbatim_consuming_comments(&mut self, span: Span) {
        while self
            .comments
            .peek_next()
            .is_some_and(|c| c.span.start >= span.start && c.span.start < span.end)
        {
            self.comments.advance();
        }
        let text = self.source_text(span).to_string();
        self.emit(&text);
    }

    fn format_expr_inner(&mut self, expr: &Expr, parent_prec: Option<u8>) {
        match &expr.kind {
            ExprKind::Int(_, _) | ExprKind::Float(_, _) | ExprKind::String(_) | ExprKind::Char(_) | ExprKind::Null => {
                let text = self.source_text(expr.span).to_string();
                self.emit(&text);
            }
            ExprKind::None => self.emit("none"),
            ExprKind::StringInterp(_) => {
                let text = self.source_text(expr.span).to_string();
                self.emit(&text);
            }
            ExprKind::Bool(b) => {
                self.emit(if *b { "true" } else { "false" });
            }
            ExprKind::Ident(name) => {
                self.emit(name);
            }
            ExprKind::Binary { op, left, right } => {
                let prec = precedence(op);
                let need_parens = parent_prec.map_or(false, |pp| prec < pp);

                if need_parens {
                    self.emit("(");
                }

                self.format_binary_operand(left, prec, op, false);
                self.emit(" ");
                self.emit(binop_str(op));
                self.emit(" ");
                self.format_binary_operand(right, prec, op, true);

                if need_parens {
                    self.emit(")");
                }
            }
            ExprKind::Unary { op, operand } => {
                self.emit(unaryop_str(op));
                // A prefix operator binds tighter than every binary one, so a
                // binary operand keeps its parentheses. Only `is` was handled,
                // and `!(a < b)` came back out as `!a < b` — "expected `bool`,
                // found `i64`" on the formatter's own output (#805).
                let needs_parens = !Self::binds_tighter_than_prefix(&operand.kind);
                if needs_parens { self.emit("("); }
                self.format_expr(operand);
                if needs_parens { self.emit(")"); }
            }
            ExprKind::Call { func, args } => {
                self.format_expr(func);
                self.emit("(");
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        self.emit(", ");
                    }
                    self.format_call_arg(arg);
                }
                self.emit(")");
            }
            ExprKind::MethodCall { object, method, type_args, args } => {
                self.format_postfix_receiver(object);
                self.emit(".");
                self.emit(method);
                if let Some(ref targs) = type_args {
                    self.emit("<");
                    self.emit(&targs.join(", "));
                    self.emit(">");
                }
                self.emit("(");
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        self.emit(", ");
                    }
                    self.format_call_arg(arg);
                }
                self.emit(")");
            }
            ExprKind::Field { object, field } => {
                self.format_postfix_receiver(object);
                self.emit(".");
                self.emit(field);
            }
            ExprKind::OptionalField { object, field } => {
                self.format_postfix_receiver(object);
                self.emit("?.");
                self.emit(field);
            }
            ExprKind::DynamicField { object, field_expr } => {
                self.format_postfix_receiver(object);
                self.emit(".(");
                self.format_expr(field_expr);
                self.emit(")");
            }
            ExprKind::Index { object, index } => {
                self.format_postfix_receiver(object);
                self.emit("[");
                self.format_expr(index);
                self.emit("]");
            }
            ExprKind::Block(stmts) => {
                if stmts.is_empty() {
                    self.emit("{}");
                } else {
                    self.emit("{");
                    self.emit_newline();
                    self.indent += 1;
                    self.format_stmts(stmts);
                    self.indent -= 1;
                    self.emit_indent();
                    self.emit("}");
                }
            }
            ExprKind::If { cond, then_branch, else_branch, else_binding } => {
                self.format_if_expr(cond, then_branch, else_branch, else_binding.as_deref());
            }
            ExprKind::IfLet { expr: scrutinee, pattern, then_branch, else_branch, else_binding } => {
                self.emit("if ");
                self.format_expr(scrutinee);
                self.emit(" is ");
                self.format_pattern(pattern);
                self.format_branch(then_branch);
                if let Some(ref else_br) = else_branch {
                    self.emit(" else");
                    // ER22: the `as e` names the value the handler is about to
                    // handle. `If` right above passes it through and this arm
                    // didn't, so `} else as e {` came back out as `} else {` and
                    // the body then referred to a name nothing declared (#805).
                    if let Some(name) = else_binding {
                        self.emit(" as ");
                        self.emit(name);
                    }
                    self.format_branch(else_br);
                }
            }
            ExprKind::IsPattern { expr, pattern } => {
                self.format_expr(expr);
                self.emit(" is ");
                self.format_pattern(pattern);
            }
            ExprKind::Match { scrutinee, arms } => {
                let source_is_multiline = self.source_text(expr.span).contains('\n');
                let all_arms_simple = arms.iter().all(|a| {
                    if a.guard.is_some() { return false; }
                    match &a.body.kind {
                        ExprKind::Block(stmts) => stmts.len() == 1 && matches!(stmts[0].kind, StmtKind::Expr(_)),
                        _ => true,
                    }
                });

                if !source_is_multiline && all_arms_simple && arms.len() <= 4 {
                    // Inline style: match x { 1 => "one", 2 => "two" }
                    self.emit("match ");
                    self.format_expr(scrutinee);
                    self.emit(" { ");
                    for (i, arm) in arms.iter().enumerate() {
                        if i > 0 {
                            self.emit(", ");
                        }
                        self.format_pattern(&arm.pattern);
                        self.emit(" => ");
                        // Unwrap single-expression blocks for inline display
                        if let ExprKind::Block(ref stmts) = arm.body.kind {
                            if stmts.len() == 1 {
                                if let StmtKind::Expr(ref inner) = stmts[0].kind {
                                    self.format_expr(inner);
                                } else {
                                    self.format_expr(&arm.body);
                                }
                            } else {
                                self.format_expr(&arm.body);
                            }
                        } else {
                            self.format_expr(&arm.body);
                        }
                    }
                    self.emit(" }");
                } else {
                    // Multi-line style: no commas
                    self.emit("match ");
                    self.format_expr(scrutinee);
                    self.emit(" {");
                    self.emit_newline();
                    self.indent += 1;
                    for arm in arms {
                        // An arm isn't a statement, so nothing was flushing the
                        // comments written among the arms — they stayed pending and
                        // came out below the closing brace, where a comment about
                        // one arm reads as a comment about the whole match (#805).
                        // An arm has no span of its own; its body's start is inside
                        // it and after anything written above it, which is all this
                        // test needs.
                        self.emit_standalone_comments_before(arm.body.span.start);
                        self.emit_indent();
                        self.format_pattern(&arm.pattern);
                        if let Some(ref guard) = arm.guard {
                            self.emit(" if ");
                            self.format_expr(guard);
                        }
                        self.emit(" => ");
                        self.format_match_arm_body(&arm.body);
                        self.try_emit_trailing_comment(arm.body.span.end);
                        self.emit_newline();
                    }
                    self.indent -= 1;
                    self.emit_indent();
                    self.emit("}");
                }
            }
            ExprKind::Try { expr: inner } => {
                self.emit("try ");
                self.format_expr(inner);
            }
            ExprKind::Take { place } => {
                self.emit("take ");
                self.format_expr(place);
            }
            ExprKind::Catch { value, ref clause } => {
                self.format_expr(value);
                self.emit(" catch ");
                self.emit(&clause.binder);
                self.emit(" => ");
                self.format_expr(&clause.body);
            }
            ExprKind::IsPresent { expr: inner, binding } => {
                self.format_postfix_receiver(inner);
                self.emit("?");
                // OPT19: `x?` is a plain bool and narrows nothing, so the `as v`
                // is the only way into the payload. Dropping it left the branch
                // body reading a name that no longer existed (#805).
                if let Some(name) = binding {
                    self.emit(" as ");
                    self.emit(name);
                }
            }
            ExprKind::Unwrap { expr: inner, message } => {
                self.format_postfix_receiver(inner);
                self.emit("!");
                if let Some(msg) = message {
                    self.emit(" ");
                    self.emit(&format!("\"{}\"", msg));
                }
            }
            ExprKind::GuardPattern { expr, pattern, else_branch } => {
                self.format_expr(expr);
                self.emit(" is ");
                self.format_pattern(pattern);
                self.emit(" else ");
                self.format_expr(else_branch);
            }
            ExprKind::NullCoalesce { value, default } => {
                self.format_expr(value);
                self.emit(" ?? ");
                self.format_expr(default);
            }
            ExprKind::Range { start, end, inclusive } => {
                if let Some(ref s) = start {
                    self.format_expr(s);
                }
                if *inclusive {
                    self.emit("..=");
                } else {
                    self.emit("..");
                }
                if let Some(ref e) = end {
                    self.format_expr(e);
                }
            }
            ExprKind::StructLit { name, fields, spread } => {
                self.emit(name);
                let source_is_multiline = self.source_text(expr.span).contains('\n');
                if fields.is_empty() && spread.is_none() {
                    self.emit(" {}");
                } else if !source_is_multiline && spread.is_none() && self.fields_fit_one_line(fields) {
                    self.emit(" { ");
                    for (i, field) in fields.iter().enumerate() {
                        if i > 0 {
                            self.emit(", ");
                        }
                        self.format_field_init(field);
                    }
                    self.emit(" }");
                } else {
                    self.emit(" {");
                    self.emit_newline();
                    self.indent += 1;
                    for field in fields {
                        self.emit_indent();
                        self.format_field_init(field);
                        self.emit(",");
                        self.emit_newline();
                    }
                    if let Some(ref spread) = spread {
                        self.emit_indent();
                        self.emit("..");
                        self.format_expr(spread);
                        self.emit_newline();
                    }
                    self.indent -= 1;
                    self.emit_indent();
                    self.emit("}");
                }
            }
            ExprKind::Array(elems) => {
                self.emit("[");
                for (i, elem) in elems.iter().enumerate() {
                    if i > 0 {
                        self.emit(", ");
                    }
                    self.format_expr(elem);
                }
                self.emit("]");
            }
            ExprKind::ArrayRepeat { value, count } => {
                self.emit("[");
                self.format_expr(value);
                self.emit("; ");
                self.format_expr(count);
                self.emit("]");
            }
            ExprKind::Tuple(elems) => {
                self.emit("(");
                for (i, elem) in elems.iter().enumerate() {
                    if i > 0 {
                        self.emit(", ");
                    }
                    self.format_expr(elem);
                }
                self.emit(")");
            }
            ExprKind::UsingBlock { name, args, body } => {
                self.emit("using ");
                self.emit(name);
                if !args.is_empty() {
                    self.emit("(");
                    for (i, arg) in args.iter().enumerate() {
                        if i > 0 {
                            self.emit(", ");
                        }
                        self.format_call_arg(arg);
                    }
                    self.emit(")");
                }
                self.emit(" {");
                self.emit_newline();
                self.indent += 1;
                self.format_stmts(body);
                self.indent -= 1;
                self.emit_indent();
                self.emit("}");
            }
            ExprKind::WithAs { bindings, body } => {
                self.emit("with ");
                for (i, binding) in bindings.iter().enumerate() {
                    if i > 0 {
                        self.emit(", ");
                    }
                    self.format_expr(&binding.source);
                    self.emit(" as ");
                    self.emit(&binding.name);
                }
                self.emit(" {");
                self.emit_newline();
                self.indent += 1;
                self.format_stmts(body);
                self.indent -= 1;
                self.emit_indent();
                self.emit("}");
            }
            ExprKind::Closure { params, ret_ty, body, is_own } => {
                if *is_own {
                    self.emit("own ");
                }
                self.emit("|");
                for (i, param) in params.iter().enumerate() {
                    if i > 0 {
                        self.emit(", ");
                    }
                    self.emit(&param.name);
                    if let Some(ref ty) = param.ty {
                        self.emit(": ");
                        self.emit(ty);
                    }
                }
                self.emit("|");
                if let Some(ref ty) = ret_ty {
                    self.emit(" -> ");
                    self.emit(ty);
                }
                self.emit(" ");
                self.format_expr(body);
            }
            ExprKind::Cast { expr: inner, ty } => {
                self.format_cast_operand(inner);
                self.emit(" as ");
                self.emit(ty);
            }
            ExprKind::Convert { expr: inner, target, kind } => {
                // `.wrap<T>()` is postfix, so its receiver needs the same
                // parentheses a `.method()` receiver does.
                self.format_postfix_receiver(inner);
                self.emit(&format!(".{}<{}>()", kind.surface(), target));
            }
            ExprKind::Spawn { body } => {
                self.emit("spawn {");
                self.emit_newline();
                self.indent += 1;
                self.format_stmts(body);
                self.indent -= 1;
                self.emit_indent();
                self.emit("}");
            }
            ExprKind::Loop { label, body } => {
                if let Some(lbl) = label {
                    self.emit(lbl);
                    self.emit(": ");
                }
                self.emit("loop {");
                self.emit_newline();
                self.indent += 1;
                self.format_stmts(body);
                self.indent -= 1;
                self.emit_indent();
                self.emit("}");
            }
            ExprKind::BlockCall { name, body } => {
                self.emit(name);
                self.emit(" {");
                self.emit_newline();
                self.indent += 1;
                self.format_stmts(body);
                self.indent -= 1;
                self.emit_indent();
                self.emit("}");
            }
            ExprKind::Unsafe { body } => {
                // `unsafe expr` and `unsafe { expr }` parse to the same node, and
                // the printer only knew the braced form — so `unsafe
                // path.as_c_str()` grew braces, and inside a condition the result
                // read as `if unsafe { … } { … }`, two braces for one `if`. The
                // source says which was written (#805).
                if self.wrote_braces(expr.span) {
                    self.emit("unsafe {");
                    self.emit_newline();
                    self.indent += 1;
                    self.format_stmts(body);
                    self.indent -= 1;
                    self.emit_indent();
                    self.emit("}");
                } else {
                    self.emit("unsafe ");
                    self.format_unbraced_body(body);
                }
            }
            ExprKind::Comptime { body } => {
                self.emit("comptime {");
                self.emit_newline();
                self.indent += 1;
                self.format_stmts(body);
                self.indent -= 1;
                self.emit_indent();
                self.emit("}");
            }
            ExprKind::Assert { condition, message } => {
                self.emit("assert ");
                self.format_expr(condition);
                if let Some(ref msg) = message {
                    self.emit(", ");
                    self.format_expr(msg);
                }
            }
            ExprKind::Check { condition, message } => {
                self.emit("check ");
                self.format_expr(condition);
                if let Some(ref msg) = message {
                    self.emit(", ");
                    self.format_expr(msg);
                }
            }
            ExprKind::Select { arms, is_priority } => {
                if *is_priority {
                    self.emit("select_priority {");
                } else {
                    self.emit("select {");
                }
                self.indent += 1;
                for arm in arms {
                    self.emit_newline();
                    match &arm.kind {
                        rask_ast::expr::SelectArmKind::Recv { channel, binding } => {
                            self.format_expr(channel);
                            self.emit(" -> ");
                            self.emit(binding);
                        }
                        rask_ast::expr::SelectArmKind::Send { channel, value } => {
                            self.format_expr(channel);
                            self.emit(" <- ");
                            self.format_expr(value);
                        }
                        rask_ast::expr::SelectArmKind::Default => {
                            self.emit("_");
                        }
                    }
                    self.emit(": ");
                    self.format_expr(&arm.body);
                    self.emit(",");
                }
                self.indent -= 1;
                self.emit_newline();
                self.emit("}");
            }
        }
    }

    /// Whether the source for this node opened with a `{` — the braced form of
    /// `unsafe`/`comptime`, which parse to the same node as the braceless one.
    fn wrote_braces(&self, span: Span) -> bool {
        let text = self.source_text(span).trim_start();
        // The keyword comes first; look at what follows it.
        let after = text
            .strip_prefix("unsafe")
            .or_else(|| text.strip_prefix("comptime"))
            .unwrap_or(text);
        after.trim_start().starts_with('{')
    }

    /// The single expression a braceless `unsafe`/`comptime` wraps.
    fn format_unbraced_body(&mut self, body: &[Stmt]) {
        match body {
            [stmt] => self.format_stmt(stmt),
            // More than one statement can't have come from the braceless form;
            // print the block so nothing is lost.
            _ => {
                self.emit("{");
                self.emit_newline();
                self.indent += 1;
                self.format_stmts(body);
                self.indent -= 1;
                self.emit_indent();
                self.emit("}");
            }
        }
    }

    /// A condition sits directly in front of a `{`, so a struct literal at its
    /// tail makes the brace ambiguous — `if c == Shape.Circle { r: 4 } {` reads
    /// as an `if` whose body starts at `{ r: 4 }`. Wrapping the condition is what
    /// the source has to do, and the printer has to keep doing it (#805).
    fn format_condition(&mut self, cond: &Expr) {
        if Self::ends_in_struct_literal(cond) {
            self.emit("(");
            self.format_expr(cond);
            self.emit(")");
        } else {
            self.format_expr(cond);
        }
    }

    /// Whether the last thing a condition prints is a `}` that a following `{`
    /// could attach to.
    fn ends_in_struct_literal(expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::StructLit { .. } => true,
            ExprKind::Binary { right, .. } => Self::ends_in_struct_literal(right),
            ExprKind::Unary { operand, .. } => Self::ends_in_struct_literal(operand),
            ExprKind::Cast { expr: inner, .. } | ExprKind::Convert { expr: inner, .. } => {
                Self::ends_in_struct_literal(inner)
            }
            ExprKind::NullCoalesce { default, .. } => Self::ends_in_struct_literal(default),
            _ => false,
        }
    }

    fn format_if_expr(
        &mut self,
        cond: &Expr,
        then_branch: &Expr,
        else_branch: &Option<Box<Expr>>,
        else_binding: Option<&str>,
    ) {
        self.emit("if ");
        self.format_condition(cond);

        if !matches!(then_branch.kind, ExprKind::Block(_)) {
            self.emit(": ");
            self.format_expr(then_branch);
            if let Some(ref else_br) = else_branch {
                self.emit(" else");
                if let Some(name) = else_binding {
                    self.emit(" as ");
                    self.emit(name);
                }
                self.emit(": ");
                self.format_expr(else_br);
            }
            return;
        }

        self.format_branch(then_branch);

        if let Some(ref else_br) = else_branch {
            if matches!(else_br.kind, ExprKind::If { .. } | ExprKind::IfLet { .. }) {
                self.emit(" else ");
                self.format_expr(else_br);
            } else {
                self.emit(" else");
                if let Some(name) = else_binding {
                    self.emit(" as ");
                    self.emit(name);
                }
                self.format_branch(else_br);
            }
        }
    }

    /// One operand of a binary operator. Precedence alone isn't enough. The
    /// operators are left-associative, so the *right* operand needs parentheses
    /// at equal precedence too — `a - (b - c)` is not `a - b - c`. And Rask
    /// forbids chained comparison, so a comparison inside a comparison keeps its
    /// parentheses whichever side it lands on, even though it binds tighter.
    fn format_binary_operand(&mut self, operand: &Expr, prec: u8, op: &BinOp, is_right: bool) {
        if is_comparison(op) {
            if let ExprKind::Binary { op: inner, .. } = &operand.kind {
                if is_comparison(inner) {
                    self.emit("(");
                    self.format_expr(operand);
                    self.emit(")");
                    return;
                }
            }
        }
        if Self::binary_operand_needs_parens(&operand.kind) {
            self.emit("(");
            self.format_expr(operand);
            self.emit(")");
            return;
        }
        let min = if is_right { prec + 1 } else { prec };
        self.format_expr_inner(operand, Some(min));
    }

    /// Forms that bind looser than any binary operator and aren't in the
    /// precedence table, so the numeric comparison can't speak for them.
    /// `(x catch _ => 0) == 0` came out as `x catch _ => 0 == 0`, which parses as
    /// `x catch _ => (0 == 0)` — a bool where a number belonged (#805).
    fn binary_operand_needs_parens(kind: &ExprKind) -> bool {
        // A `Cast` is *not* exempt, though since #817 that's for readability
        // rather than for correctness. `as` now binds tighter than every binary
        // operator (type.operators/P4), so the parens can't change the grouping —
        // but they make the grouping visible, and this is the one place in an
        // expression where getting it wrong is expensive. It cost
        // `sensor_processor` a factor of a hundred back when `as` sat between
        // `+ -` and `* / %`: `(base + noise) as f64 / 100.0` came out as
        // `base + noise as f64 / 100.0`, which was `base + ((noise as f64) /
        // 100.0)` and reported 2200.02 °C where the answer is 22.02 (#805).
        !matches!(
            kind,
            ExprKind::Binary { .. }
                | ExprKind::Unary { .. }
                | ExprKind::Convert { .. }
        ) && !Self::binds_tighter_than_postfix(kind)
    }

    /// Whether an expression can sit under a prefix operator unparenthesized:
    /// the postfix and primary forms, plus another prefix. `x?` is excluded even
    /// though it's postfix — `!x?` is forbidden outright (OPT17).
    fn binds_tighter_than_prefix(kind: &ExprKind) -> bool {
        Self::binds_tighter_than_postfix(kind) || matches!(kind, ExprKind::Unary { .. })
    }

    /// `as` binds tighter than every binary operator and looser than prefix and
    /// postfix (type.operators/P4), so anything but a postfix or primary operand
    /// keeps its parentheses — a unary operand included, which is why `-a as f64`
    /// prints as `(-a) as f64`.
    ///
    /// They were dropped once, and `(base + noise) as f64 / 100.0` came out as
    /// `base + noise as f64 / 100.0` — which was `base + ((noise as f64) / 100.0)`,
    /// a different number. `sensor_processor` reported 2200.02 °C instead of
    /// 22.02 (#805).
    fn format_cast_operand(&mut self, inner: &Expr) {
        if Self::binds_tighter_than_postfix(&inner.kind) {
            self.format_expr(inner);
            return;
        }
        self.emit("(");
        self.format_expr(inner);
        self.emit(")");
    }

    /// A postfix `.`, `[`, `!` or `?` binds tighter than almost everything, so a
    /// receiver that binds looser needs its parentheses back. The printer dropped
    /// them, and `(time.Instant.now() - start).as_nanos()` came out as
    /// `time.Instant.now() - start.as_nanos()` — a different call on a different
    /// receiver (#805).
    fn format_postfix_receiver(&mut self, object: &Expr) {
        if Self::binds_tighter_than_postfix(&object.kind) {
            self.format_expr(object);
            return;
        }
        self.emit("(");
        self.format_expr(object);
        self.emit(")");
    }

    /// Primary and postfix forms, which can carry a `.` directly. Anything else
    /// — an operator, a cast, a block, a prefix form — gets parenthesized. `x?`
    /// is in the second group even though it's postfix: `x?.y` lexes as `?.`.
    fn binds_tighter_than_postfix(kind: &ExprKind) -> bool {
        matches!(
            kind,
            ExprKind::Int(..)
                | ExprKind::Float(..)
                | ExprKind::String(_)
                | ExprKind::StringInterp(_)
                | ExprKind::Char(_)
                | ExprKind::Bool(_)
                | ExprKind::Null
                | ExprKind::None
                | ExprKind::Ident(_)
                | ExprKind::Call { .. }
                | ExprKind::MethodCall { .. }
                | ExprKind::Field { .. }
                | ExprKind::DynamicField { .. }
                | ExprKind::OptionalField { .. }
                | ExprKind::Index { .. }
                | ExprKind::StructLit { .. }
                | ExprKind::Array(_)
                | ExprKind::ArrayRepeat { .. }
                | ExprKind::Tuple(_)
                | ExprKind::Unwrap { .. }
        )
    }

    fn format_branch(&mut self, expr: &Expr) {
        if let ExprKind::Block(ref stmts) = expr.kind {
            if stmts.is_empty() {
                self.emit(" {}");
            } else if self.fits_one_line(expr.span, stmts) {
                self.emit(" { ");
                self.format_stmt_inline(&stmts[0]);
                self.emit(" }");
            } else {
                self.emit(" {");
                self.emit_newline();
                self.indent += 1;
                self.format_stmts(stmts);
                self.indent -= 1;
                self.emit_indent();
                self.emit("}");
            }
        } else {
            self.emit(" ");
            self.format_expr(expr);
        }
    }

    /// A one-statement block the source kept on one line stays on one line.
    /// Expanding them all is what made `fmt --check` red across the tree, and
    /// `if c { return 1 }` reads better as written than as three lines (#805).
    ///
    /// A comment anywhere in the block forces the expansion — a trailing comment
    /// has nowhere to go on a line that continues with `}`.
    fn fits_one_line(&self, span: Span, stmts: &[Stmt]) -> bool {
        stmts.len() == 1
            && !self.source_text(span).contains('\n')
            && !self.comments_within(span)
    }

    /// Whether any unconsumed comment starts inside `span`.
    fn comments_within(&self, span: Span) -> bool {
        self.comments
            .peek_next()
            .is_some_and(|c| c.span.start >= span.start && c.span.start < span.end)
    }

    /// One field of a struct literal.
    ///
    /// `Point { x, y }` is shorthand for `x: x, y: y` and the printer expanded
    /// it — the punning the parser has always accepted came back out long-form
    /// (#307). Both spellings mean the same thing, so the short one wins.
    fn format_field_init(&mut self, field: &FieldInit) {
        if let ExprKind::Ident(name) = &field.value.kind {
            if *name == field.name {
                self.emit(&field.name);
                return;
            }
        }
        self.emit(&field.name);
        self.emit(": ");
        self.format_expr(&field.value);
    }

    fn fields_fit_one_line(&self, fields: &[FieldInit]) -> bool {
        let est: usize = fields.iter().map(|f| f.name.len() + 4 + 10).sum();
        est < 60
    }

    /// Format match arm body, detecting inline vs block form from source.
    fn format_match_arm_body(&mut self, body: &Expr) {
        if let ExprKind::Block(ref stmts) = body.kind {
            // Check if the source had braces (block form) or not (inline expression)
            let source_text = self.source_text(body.span).trim_start();
            if source_text.starts_with('{') {
                self.format_expr(body);
            } else if stmts.len() == 1 {
                self.format_stmt_inline(&stmts[0]);
            } else {
                self.format_expr(body);
            }
        } else {
            self.format_expr(body);
        }
    }

    // --- Patterns ---

    fn format_pattern(&mut self, pattern: &Pattern) {
        match pattern {
            Pattern::Wildcard => self.emit("_"),
            Pattern::Ident(name) => self.emit(name),
            Pattern::Literal(expr) => self.format_expr(expr),
            Pattern::Constructor { name, fields } => {
                self.emit(name);
                if !fields.is_empty() {
                    self.emit("(");
                    for (i, f) in fields.iter().enumerate() {
                        if i > 0 {
                            self.emit(", ");
                        }
                        self.format_pattern(f);
                    }
                    self.emit(")");
                }
            }
            Pattern::Struct { name, fields, rest } => {
                self.emit(name);
                self.emit(" { ");
                for (i, (fname, fpat)) in fields.iter().enumerate() {
                    if i > 0 {
                        self.emit(", ");
                    }
                    self.emit(fname);
                    if !matches!(fpat, Pattern::Ident(ref n) if n == fname) {
                        self.emit(": ");
                        self.format_pattern(fpat);
                    }
                }
                if *rest {
                    if !fields.is_empty() {
                        self.emit(", ");
                    }
                    self.emit("..");
                }
                self.emit(" }");
            }
            Pattern::Tuple(elems) => {
                self.emit("(");
                for (i, e) in elems.iter().enumerate() {
                    if i > 0 {
                        self.emit(", ");
                    }
                    self.format_pattern(e);
                }
                self.emit(")");
            }
            Pattern::Or(alts) => {
                for (i, a) in alts.iter().enumerate() {
                    if i > 0 {
                        self.emit(" | ");
                    }
                    self.format_pattern(a);
                }
            }
            Pattern::Range { start, end } => {
                self.format_expr(start);
                self.emit("..=");
                self.format_expr(end);
            }
            Pattern::TypePat { ty_name, binding } => {
                self.emit(ty_name);
                if let Some(name) = binding {
                    self.emit(" as ");
                    self.emit(name);
                }
            }
        }
    }
}

// --- Operator helpers ---

/// Comparison and equality are non-associative in Rask — `a < b < c` is
/// rejected — so nesting one inside another always needs parentheses.
fn is_comparison(op: &BinOp) -> bool {
    matches!(
        op,
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge
    )
}

fn precedence(op: &BinOp) -> u8 {
    match op {
        BinOp::Or => 1,
        BinOp::And => 2,
        BinOp::BitOr => 3,
        BinOp::BitXor => 4,
        BinOp::BitAnd => 5,
        BinOp::Eq | BinOp::Ne => 6,
        BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => 7,
        BinOp::Shl | BinOp::Shr => 8,
        BinOp::Add | BinOp::Sub => 9,
        BinOp::Mul | BinOp::Div | BinOp::Mod => 10,
    }
}

fn binop_str(op: &BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Gt => ">",
        BinOp::Le => "<=",
        BinOp::Ge => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
        BinOp::BitAnd => "&",
        BinOp::BitOr => "|",
        BinOp::BitXor => "^",
        BinOp::Shl => "<<",
        BinOp::Shr => ">>",
    }
}

fn unaryop_str(op: &UnaryOp) -> &'static str {
    match op {
        UnaryOp::Neg => "-",
        UnaryOp::Not => "!",
        UnaryOp::BitNot => "~",
        UnaryOp::Ref => "&",
        UnaryOp::Deref => "*",
        UnaryOp::Own => "own ",
    }
}
