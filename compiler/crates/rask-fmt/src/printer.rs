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
}

impl<'a> Printer<'a> {
    pub fn new(source: &'a str, comments: CommentList, config: &'a FormatConfig) -> Self {
        Self {
            output: String::new(),
            indent: 0,
            source,
            comments,
            config,
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
    fn try_emit_trailing_comment(&mut self, span_end: usize) -> bool {
        if let Some(c) = self.comments.peek_next() {
            // Find actual content end (skip trailing whitespace in span)
            let bytes = self.source.as_bytes();
            let mut content_end = span_end;
            while content_end > 0 && bytes[content_end - 1].is_ascii_whitespace() {
                content_end -= 1;
            }
            // Check if comment is on the same line (no newline between content and comment)
            if c.span.start > content_end && c.span.start < self.source.len() {
                let gap = &self.source[content_end..c.span.start];
                if !gap.contains('\n') {
                    let Some(c) = self.comments.advance() else { return false; };
                    // Preserve original spacing or use standard 2-space gap
                    let spaces = gap.len().max(2);
                    for _ in 0..spaces {
                        self.output.push(' ');
                    }
                    self.output.push_str(&c.text);
                    return true;
                }
            }
        }
        false
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

    /// Consume trailing comments that belong to the current block (at current indent or deeper).
    fn consume_trailing_block_comments(&mut self) {
        let min_indent = self.indent * self.config.indent_width;
        loop {
            let c = match self.comments.peek_next() {
                Some(c) => c,
                None => break,
            };
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
        if let Some(inner) = ty.strip_prefix("Result<") {
            if let Some(inner) = inner.strip_suffix('>') {
                // Find the top-level comma (not inside nested angle brackets)
                let mut depth = 0;
                for (i, ch) in inner.char_indices() {
                    match ch {
                        '<' => depth += 1,
                        '>' => depth -= 1,
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
        // Convert func(T, U) -> R back to |T, U| -> R
        if let Some(rest) = ty.strip_prefix("func(") {
            // Find matching closing paren
            let mut depth = 1;
            for (i, ch) in rest.char_indices() {
                match ch {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            let params = &rest[..i];
                            let after = rest[i + 1..].trim();
                            if let Some(ret) = after.strip_prefix("->") {
                                let ret_ty = ret.trim();
                                return format!(
                                    "|{}| -> {}",
                                    params,
                                    self.format_type(ret_ty)
                                );
                            } else {
                                return format!("|{}|", params);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        ty.to_string()
    }

    // --- File ---

    pub fn format_file(&mut self, decls: &[Decl]) {
        let mut is_first = true;
        let mut prev_was_import = false;

        for decl in decls {
            let is_import = matches!(decl.kind, DeclKind::Import(_));

            // Emit comments before this decl (with blank lines from source)
            let comments = self.emit_comments_before(decl.span.start, !is_first);

            // Blank line between previous decl/comment and this decl
            if !is_first && comments.is_empty() {
                if !(prev_was_import && is_import) {
                    self.emit_blank_line();
                }
            }

            // Blank line between last comment and decl (if source had one)
            if !comments.is_empty() && self.has_blank_line_before(decl.span.start) {
                self.emit_blank_line();
            } else if !is_first && comments.is_empty() {
                // Already handled above
            }

            self.format_decl(decl);
            if !self.output.ends_with('\n') {
                self.emit_newline();
            }

            is_first = false;
            prev_was_import = is_import;
        }
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
            DeclKind::Package(_) => {} // Package blocks formatted by build.rk tooling
        }
    }

    fn format_fn_decl(&mut self, f: &FnDecl, is_method: bool, is_trait_decl: bool) {
        if !is_method {
            self.emit_indent();
        }

        for attr in &f.attrs {
            self.emit(&format!("@{attr}"));
            self.emit_newline();
            self.emit_indent();
        }

        if f.is_pub {
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

        if f.body.is_empty() && is_trait_decl {
            // Trait method declaration with no body — no braces
        } else if f.body.is_empty() {
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
            self.emit(": ");
            let ty = self.format_type(&param.ty);
            self.emit(&ty);
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

        if !source_is_multiline && !has_methods && s.fields.len() <= 4 && self.struct_fields_fit_one_line(&s.fields) {
            // Inline style: struct Vec3 { x: f64, y: f64, z: f64 }
            self.emit(" { ");
            for (i, field) in s.fields.iter().enumerate() {
                if i > 0 {
                    self.emit(", ");
                }
                if field.is_pub {
                    self.emit("public ");
                }
                self.emit(&field.name);
                self.emit(": ");
                let ty = self.format_type(&field.ty);
                self.emit(&ty);
            }
            self.emit(" }");
        } else {
            // Multi-line style: no commas
            self.emit(" {");
            self.emit_newline();

            self.indent += 1;
            for field in &s.fields {
                self.emit_indent();
                if field.is_pub {
                    self.emit("public ");
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

    fn struct_fields_fit_one_line(&self, fields: &[Field]) -> bool {
        let est: usize = fields.iter().map(|f| {
            f.name.len() + 2 + f.ty.len() + if f.is_pub { 7 } else { 0 }
        }).sum::<usize>() + (fields.len().saturating_sub(1) * 2);
        est < 60
    }

    fn format_enum_decl(&mut self, e: &EnumDecl, span: Span) {
        self.emit_indent();

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

        if !source_is_multiline && !has_methods && all_fieldless && self.enum_variants_fit_one_line(&e.variants) {
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
            for variant in &e.variants {
                self.emit_indent();
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
                self.emit_newline();
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
        if t.is_pub {
            self.emit("public ");
        }
        self.emit("trait ");
        self.emit(&t.name);
        self.emit(" {");
        self.emit_newline();

        self.indent += 1;
        let mut first = true;
        for method in &t.methods {
            if !first {
                self.emit_blank_line();
            }
            self.emit_indent();
            self.format_fn_decl(method, true, true);
            self.emit_newline();
            first = false;
        }
        self.indent -= 1;
        self.emit_indent();
        self.emit("}");
    }

    fn format_impl_decl(&mut self, imp: &ImplDecl) {
        self.emit_indent();
        self.emit("extend ");
        self.emit(&imp.target_ty);
        if let Some(ref trait_name) = imp.trait_name {
            self.emit(" with ");
            self.emit(trait_name);
        }
        self.emit(" {");
        self.emit_newline();

        self.indent += 1;
        let mut first = true;
        for method in &imp.methods {
            if !first {
                self.emit_blank_line();
            }
            self.emit_indent();
            self.format_fn_decl(method, true, false);
            self.emit_newline();
            first = false;
        }
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

    fn format_extern_decl(&mut self, e: &ExternDecl) {
        self.emit_indent();
        self.emit("extern \"");
        self.emit(&e.abi);
        self.emit("\" func ");
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
        match &stmt.kind {
            StmtKind::Expr(expr) => {
                self.format_expr(expr);
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
            StmtKind::LetTuple { names, init } => {
                self.emit("let (");
                self.emit(&names.join(", "));
                self.emit(") = ");
                self.format_expr(init);
            }
            StmtKind::Const { name, name_span: _, ty, init } => {
                self.emit("const ");
                self.emit(name);
                if let Some(ref ty) = ty {
                    self.emit(": ");
                    let t = self.format_type(ty);
                    self.emit(&t);
                }
                self.emit(" = ");
                self.format_expr(init);
            }
            StmtKind::ConstTuple { names, init } => {
                self.emit("const (");
                self.emit(&names.join(", "));
                self.emit(") = ");
                self.format_expr(init);
            }
            StmtKind::Assign { target, value } => {
                self.format_expr(target);
                self.emit(" = ");
                self.format_expr(value);
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
            StmtKind::While { cond, body } => {
                self.emit("while ");
                self.format_expr(cond);
                self.emit(" {");
                self.emit_newline();
                self.indent += 1;
                self.format_stmts(body);
                self.indent -= 1;
                self.emit_indent();
                self.emit("}");
            }
            StmtKind::WhileLet { pattern, expr, body } => {
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
            StmtKind::For { label, binding, iter, body } => {
                if let Some(ref l) = label {
                    self.emit(l);
                    self.emit(": ");
                }
                self.emit("for ");
                self.emit(binding);
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
        match arg.mode {
            ArgMode::Mutate => self.emit("mutate "),
            ArgMode::Own => self.emit("own "),
            ArgMode::Default => {}
        }
        self.format_expr(&arg.expr);
    }

    fn format_expr(&mut self, expr: &Expr) {
        self.format_expr_inner(expr, None);
    }

    fn format_expr_inner(&mut self, expr: &Expr, parent_prec: Option<u8>) {
        match &expr.kind {
            ExprKind::Int(_, _) | ExprKind::Float(_, _) | ExprKind::String(_) | ExprKind::Char(_) => {
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

                self.format_expr_inner(left, Some(prec));
                self.emit(" ");
                self.emit(binop_str(op));
                self.emit(" ");
                self.format_expr_inner(right, Some(prec));

                if need_parens {
                    self.emit(")");
                }
            }
            ExprKind::Unary { op, operand } => {
                self.emit(unaryop_str(op));
                let needs_parens = matches!(operand.kind, ExprKind::IsPattern { .. });
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
                self.format_expr(object);
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
                self.format_expr(object);
                self.emit(".");
                self.emit(field);
            }
            ExprKind::OptionalField { object, field } => {
                self.format_expr(object);
                self.emit("?.");
                self.emit(field);
            }
            ExprKind::Index { object, index } => {
                self.format_expr(object);
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
            ExprKind::If { cond, then_branch, else_branch } => {
                self.format_if_expr(cond, then_branch, else_branch);
            }
            ExprKind::IfLet { expr: scrutinee, pattern, then_branch, else_branch } => {
                self.emit("if ");
                self.format_expr(scrutinee);
                self.emit(" is ");
                self.format_pattern(pattern);
                self.format_branch(then_branch);
                if let Some(ref else_br) = else_branch {
                    self.emit(" else");
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
                        self.emit_indent();
                        self.format_pattern(&arm.pattern);
                        if let Some(ref guard) = arm.guard {
                            self.emit(" if ");
                            self.format_expr(guard);
                        }
                        self.emit(" => ");
                        self.format_match_arm_body(&arm.body);
                        self.emit_newline();
                    }
                    self.indent -= 1;
                    self.emit_indent();
                    self.emit("}");
                }
            }
            ExprKind::Try(inner) => {
                self.emit("try ");
                self.format_expr(inner);
            }
            ExprKind::Unwrap { expr: inner, message } => {
                self.format_expr(inner);
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
                        self.emit(&field.name);
                        self.emit(": ");
                        self.format_expr(&field.value);
                    }
                    self.emit(" }");
                } else {
                    self.emit(" {");
                    self.emit_newline();
                    self.indent += 1;
                    for field in fields {
                        self.emit_indent();
                        self.emit(&field.name);
                        self.emit(": ");
                        self.format_expr(&field.value);
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
                for (i, (expr, name)) in bindings.iter().enumerate() {
                    if i > 0 {
                        self.emit(", ");
                    }
                    self.format_expr(expr);
                    self.emit(" as ");
                    self.emit(name);
                }
                self.emit(" {");
                self.emit_newline();
                self.indent += 1;
                self.format_stmts(body);
                self.indent -= 1;
                self.emit_indent();
                self.emit("}");
            }
            ExprKind::Closure { params, ret_ty, body } => {
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
                self.format_expr(inner);
                self.emit(" as ");
                self.emit(ty);
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
                self.emit("unsafe {");
                self.emit_newline();
                self.indent += 1;
                self.format_stmts(body);
                self.indent -= 1;
                self.emit_indent();
                self.emit("}");
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

    fn format_if_expr(&mut self, cond: &Expr, then_branch: &Expr, else_branch: &Option<Box<Expr>>) {
        self.emit("if ");
        self.format_expr(cond);

        if !matches!(then_branch.kind, ExprKind::Block(_)) {
            self.emit(": ");
            self.format_expr(then_branch);
            if let Some(ref else_br) = else_branch {
                self.emit(" else: ");
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
                self.format_branch(else_br);
            }
        }
    }

    fn format_branch(&mut self, expr: &Expr) {
        if let ExprKind::Block(ref stmts) = expr.kind {
            if stmts.is_empty() {
                self.emit(" {}");
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
        }
    }
}

// --- Operator helpers ---

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
    }
}
