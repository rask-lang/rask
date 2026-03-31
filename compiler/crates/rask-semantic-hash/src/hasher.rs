// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Core semantic hasher — walks desugared AST and produces a stable hash.
//!
//! Key design decisions:
//! - Uses a simple FNV-1a style hash (64-bit) for speed. Collision resistance
//!   isn't critical — we're detecting *changes*, not adversarial inputs.
//! - Variable names are replaced with de Bruijn-like positional indices (H4).
//!   Renaming `x` to `y` without changing structure produces the same hash.
//! - Source locations (spans) are excluded (H3).
//! - Node IDs are excluded (cosmetic).
//! - Literal values, types, operators, and control flow structure are included (H2).

use std::collections::HashMap;

use rask_ast::decl::{Decl, DeclKind, FnDecl, StructDecl, EnumDecl, Field, Param, TypeParam,
                     ContextClause};
use rask_ast::expr::{ArgMode, BinOp, CallArg, ClosureParam, Expr, ExprKind, FieldInit,
                     MatchArm, Pattern, SelectArm, SelectArmKind, TryElse, UnaryOp,
                     WithBinding};
use rask_ast::stmt::{ForBinding, Stmt, StmtKind};

/// A 64-bit semantic hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SemanticHash(pub u64);

impl SemanticHash {
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

/// Semantic hasher state.
struct Hasher {
    state: u64,
    /// Variable name → positional index (H4: normalized names).
    /// Reset per function scope.
    var_indices: HashMap<String, u32>,
    next_var_index: u32,
}

const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

impl Hasher {
    fn new() -> Self {
        Self {
            state: FNV_OFFSET,
            var_indices: HashMap::new(),
            next_var_index: 0,
        }
    }

    fn feed_byte(&mut self, b: u8) {
        self.state ^= b as u64;
        self.state = self.state.wrapping_mul(FNV_PRIME);
    }

    fn feed_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.feed_byte(b);
        }
    }

    fn feed_u8(&mut self, v: u8) {
        self.feed_byte(v);
    }

    fn feed_u32(&mut self, v: u32) {
        self.feed_bytes(&v.to_le_bytes());
    }

    fn feed_u64(&mut self, v: u64) {
        self.feed_bytes(&v.to_le_bytes());
    }

    fn feed_i64(&mut self, v: i64) {
        self.feed_bytes(&v.to_le_bytes());
    }

    fn feed_f64(&mut self, v: f64) {
        self.feed_bytes(&v.to_bits().to_le_bytes());
    }

    fn feed_bool(&mut self, v: bool) {
        self.feed_byte(v as u8);
    }

    fn feed_str(&mut self, s: &str) {
        self.feed_u32(s.len() as u32);
        self.feed_bytes(s.as_bytes());
    }

    fn feed_char(&mut self, c: char) {
        self.feed_u32(c as u32);
    }

    /// Tag byte to distinguish AST node kinds in the hash stream.
    fn feed_tag(&mut self, tag: u8) {
        self.feed_byte(tag);
    }

    /// H4: Get or assign a positional index for a variable name.
    fn var_index(&mut self, name: &str) -> u32 {
        if let Some(&idx) = self.var_indices.get(name) {
            idx
        } else {
            let idx = self.next_var_index;
            self.next_var_index += 1;
            self.var_indices.insert(name.to_string(), idx);
            idx
        }
    }

    /// Hash a variable name as its positional index.
    fn feed_var(&mut self, name: &str) {
        let idx = self.var_index(name);
        self.feed_u32(idx);
    }

    fn finish(self) -> SemanticHash {
        SemanticHash(self.state)
    }

    // ── Declaration hashing ───────────────────────────────────────

    fn hash_decl(&mut self, decl: &Decl) {
        match &decl.kind {
            DeclKind::Fn(f) => {
                self.feed_tag(1);
                self.hash_fn_decl(f);
            }
            DeclKind::Struct(s) => {
                self.feed_tag(2);
                self.hash_struct_decl(s);
            }
            DeclKind::Enum(e) => {
                self.feed_tag(3);
                self.hash_enum_decl(e);
            }
            DeclKind::Trait(t) => {
                self.feed_tag(4);
                self.feed_str(&t.name);
                self.feed_bool(t.is_pub);
                for st in &t.super_traits {
                    self.feed_str(st);
                }
                for m in &t.methods {
                    self.hash_fn_decl(m);
                }
            }
            DeclKind::Impl(i) => {
                self.feed_tag(5);
                if let Some(tn) = &i.trait_name {
                    self.feed_bool(true);
                    self.feed_str(tn);
                } else {
                    self.feed_bool(false);
                }
                self.feed_str(&i.target_ty);
                for m in &i.methods {
                    self.hash_fn_decl(m);
                }
            }
            DeclKind::Import(imp) => {
                self.feed_tag(6);
                for seg in &imp.path {
                    self.feed_str(seg);
                }
                self.feed_bool(imp.is_glob);
                self.feed_bool(imp.is_lazy);
            }
            DeclKind::Const(c) => {
                self.feed_tag(7);
                self.feed_str(&c.name);
                self.feed_bool(c.is_pub);
                if let Some(ty) = &c.ty {
                    self.feed_str(ty);
                }
                self.hash_expr(&c.init);
            }
            DeclKind::Test(t) => {
                self.feed_tag(8);
                self.feed_str(&t.name);
                self.hash_stmts(&t.body);
            }
            DeclKind::Benchmark(b) => {
                self.feed_tag(9);
                self.feed_str(&b.name);
                self.hash_stmts(&b.body);
            }
            DeclKind::Union(u) => {
                self.feed_tag(10);
                self.feed_str(&u.name);
                self.hash_fields(&u.fields);
            }
            DeclKind::Extern(e) => {
                self.feed_tag(11);
                self.feed_str(&e.abi);
                self.feed_str(&e.name);
                for p in &e.params {
                    self.hash_param(p);
                }
                if let Some(rt) = &e.ret_ty {
                    self.feed_str(rt);
                }
            }
            DeclKind::Export(_) => {
                self.feed_tag(12);
            }
            DeclKind::Package(_) => {
                self.feed_tag(13);
            }
            DeclKind::TypeAlias(ta) => {
                self.feed_tag(14);
                self.feed_str(&ta.name);
                self.feed_str(&ta.target);
                self.feed_bool(ta.is_pub);
            }
        }
    }

    fn hash_fn_decl(&mut self, f: &FnDecl) {
        // H1: Function name is part of the hash (not normalized)
        self.feed_str(&f.name);
        self.feed_bool(f.is_pub);
        self.feed_bool(f.is_comptime);
        self.feed_bool(f.is_unsafe);

        // Type parameters
        self.feed_u32(f.type_params.len() as u32);
        for tp in &f.type_params {
            self.hash_type_param(tp);
        }

        // Parameters (names normalized, types included)
        self.feed_u32(f.params.len() as u32);
        for p in &f.params {
            self.hash_param(p);
        }

        // Return type
        if let Some(rt) = &f.ret_ty {
            self.feed_bool(true);
            self.feed_str(rt);
        } else {
            self.feed_bool(false);
        }

        // Context clauses
        for cc in &f.context_clauses {
            self.hash_context_clause(cc);
        }

        // Attributes
        for attr in &f.attrs {
            self.feed_str(attr);
        }

        // ABI
        if let Some(abi) = &f.abi {
            self.feed_bool(true);
            self.feed_str(abi);
        } else {
            self.feed_bool(false);
        }

        // Body
        self.hash_stmts(&f.body);
    }

    fn hash_struct_decl(&mut self, s: &StructDecl) {
        self.feed_str(&s.name);
        self.feed_bool(s.is_pub);
        for tp in &s.type_params {
            self.hash_type_param(tp);
        }
        self.hash_fields(&s.fields);
        for attr in &s.attrs {
            self.feed_str(attr);
        }
        for m in &s.methods {
            self.hash_fn_decl(m);
        }
    }

    fn hash_enum_decl(&mut self, e: &EnumDecl) {
        self.feed_str(&e.name);
        self.feed_bool(e.is_pub);
        for tp in &e.type_params {
            self.hash_type_param(tp);
        }
        self.feed_u32(e.variants.len() as u32);
        for v in &e.variants {
            self.feed_str(&v.name);
            self.hash_fields(&v.fields);
        }
        for m in &e.methods {
            self.hash_fn_decl(m);
        }
    }

    fn hash_fields(&mut self, fields: &[Field]) {
        self.feed_u32(fields.len() as u32);
        for f in fields {
            self.feed_str(&f.name);
            self.feed_str(&f.ty);
            self.feed_u8(f.visibility as u8);
        }
    }

    fn hash_param(&mut self, p: &Param) {
        // H4: Parameter names normalized
        self.feed_var(&p.name);
        self.feed_str(&p.ty);
        self.feed_bool(p.is_take);
        self.feed_bool(p.is_mutate);
        if let Some(d) = &p.default {
            self.feed_bool(true);
            self.hash_expr(d);
        } else {
            self.feed_bool(false);
        }
    }

    fn hash_type_param(&mut self, tp: &TypeParam) {
        self.feed_str(&tp.name);
        self.feed_bool(tp.is_comptime);
        if let Some(ct) = &tp.comptime_type {
            self.feed_str(ct);
        }
        for b in &tp.bounds {
            self.feed_str(b);
        }
    }

    fn hash_context_clause(&mut self, cc: &ContextClause) {
        if let Some(n) = &cc.name {
            self.feed_bool(true);
            self.feed_str(n);
        } else {
            self.feed_bool(false);
        }
        self.feed_str(&cc.ty);
        self.feed_bool(cc.is_frozen);
    }

    // ── Statement hashing ─────────────────────────────────────────

    fn hash_stmts(&mut self, stmts: &[Stmt]) {
        self.feed_u32(stmts.len() as u32);
        for stmt in stmts {
            self.hash_stmt(stmt);
        }
    }

    fn hash_stmt(&mut self, stmt: &Stmt) {
        // H3: NodeId and Span excluded
        match &stmt.kind {
            StmtKind::Expr(e) => {
                self.feed_tag(20);
                self.hash_expr(e);
            }
            StmtKind::Let { name, ty, init, .. } => {
                self.feed_tag(21);
                self.feed_var(name);
                if let Some(t) = ty {
                    self.feed_bool(true);
                    self.feed_str(t);
                } else {
                    self.feed_bool(false);
                }
                self.hash_expr(init);
            }
            StmtKind::LetTuple { names, init } => {
                self.feed_tag(22);
                self.feed_u32(names.len() as u32);
                for n in names {
                    self.feed_var(n);
                }
                self.hash_expr(init);
            }
            StmtKind::Const { name, ty, init, .. } => {
                self.feed_tag(23);
                self.feed_var(name);
                if let Some(t) = ty {
                    self.feed_bool(true);
                    self.feed_str(t);
                } else {
                    self.feed_bool(false);
                }
                self.hash_expr(init);
            }
            StmtKind::ConstTuple { names, init } => {
                self.feed_tag(24);
                self.feed_u32(names.len() as u32);
                for n in names {
                    self.feed_var(n);
                }
                self.hash_expr(init);
            }
            StmtKind::Assign { target, value } => {
                self.feed_tag(25);
                self.hash_expr(target);
                self.hash_expr(value);
            }
            StmtKind::Return(val) => {
                self.feed_tag(26);
                if let Some(e) = val {
                    self.feed_bool(true);
                    self.hash_expr(e);
                } else {
                    self.feed_bool(false);
                }
            }
            StmtKind::Break { label, value } => {
                self.feed_tag(27);
                if let Some(l) = label {
                    self.feed_bool(true);
                    self.feed_str(l);
                } else {
                    self.feed_bool(false);
                }
                if let Some(v) = value {
                    self.feed_bool(true);
                    self.hash_expr(v);
                } else {
                    self.feed_bool(false);
                }
            }
            StmtKind::Continue(label) => {
                self.feed_tag(28);
                if let Some(l) = label {
                    self.feed_bool(true);
                    self.feed_str(l);
                } else {
                    self.feed_bool(false);
                }
            }
            StmtKind::While { cond, body } => {
                self.feed_tag(29);
                self.hash_expr(cond);
                self.hash_stmts(body);
            }
            StmtKind::WhileLet { pattern, expr, body } => {
                self.feed_tag(30);
                self.hash_pattern(pattern);
                self.hash_expr(expr);
                self.hash_stmts(body);
            }
            StmtKind::Loop { label, body } => {
                self.feed_tag(31);
                if let Some(l) = label {
                    self.feed_bool(true);
                    self.feed_str(l);
                } else {
                    self.feed_bool(false);
                }
                self.hash_stmts(body);
            }
            StmtKind::For { label, binding, mutate, iter, body } => {
                self.feed_tag(32);
                if let Some(l) = label {
                    self.feed_bool(true);
                    self.feed_str(l);
                } else {
                    self.feed_bool(false);
                }
                self.feed_bool(*mutate);
                match binding {
                    ForBinding::Single(n) => {
                        self.feed_u8(0);
                        self.feed_var(n);
                    }
                    ForBinding::Tuple(ns) => {
                        self.feed_u8(1);
                        self.feed_u32(ns.len() as u32);
                        for n in ns {
                            self.feed_var(n);
                        }
                    }
                }
                self.hash_expr(iter);
                self.hash_stmts(body);
            }
            StmtKind::Ensure { body, else_handler } => {
                self.feed_tag(33);
                self.hash_stmts(body);
                if let Some((param, handler)) = else_handler {
                    self.feed_bool(true);
                    self.feed_var(param);
                    self.hash_stmts(handler);
                } else {
                    self.feed_bool(false);
                }
            }
            StmtKind::Comptime(body) => {
                self.feed_tag(34);
                self.hash_stmts(body);
            }
        }
    }

    // ── Expression hashing ────────────────────────────────────────

    fn hash_expr(&mut self, expr: &Expr) {
        // H3: NodeId and Span excluded
        match &expr.kind {
            ExprKind::Int(v, suffix) => {
                self.feed_tag(40);
                self.feed_i64(*v);
                self.feed_u8(suffix.map_or(0, |s| s as u8 + 1));
            }
            ExprKind::Float(v, suffix) => {
                self.feed_tag(41);
                self.feed_f64(*v);
                self.feed_u8(suffix.map_or(0, |s| s as u8 + 1));
            }
            ExprKind::String(s) => {
                self.feed_tag(42);
                self.feed_str(s);
            }
            ExprKind::Char(c) => {
                self.feed_tag(43);
                self.feed_char(*c);
            }
            ExprKind::Bool(b) => {
                self.feed_tag(44);
                self.feed_bool(*b);
            }
            ExprKind::Null => {
                self.feed_tag(45);
            }
            ExprKind::Ident(name) => {
                self.feed_tag(46);
                // H4: Normalize local variable references
                self.feed_var(name);
            }
            ExprKind::Binary { op, left, right } => {
                self.feed_tag(47);
                self.feed_u8(binop_tag(*op));
                self.hash_expr(left);
                self.hash_expr(right);
            }
            ExprKind::Unary { op, operand } => {
                self.feed_tag(48);
                self.feed_u8(unaryop_tag(*op));
                self.hash_expr(operand);
            }
            ExprKind::Call { func, args } => {
                self.feed_tag(49);
                self.hash_expr(func);
                self.hash_call_args(args);
            }
            ExprKind::MethodCall { object, method, type_args, args } => {
                self.feed_tag(50);
                self.hash_expr(object);
                self.feed_str(method);
                if let Some(tas) = type_args {
                    self.feed_bool(true);
                    self.feed_u32(tas.len() as u32);
                    for ta in tas {
                        self.feed_str(ta);
                    }
                } else {
                    self.feed_bool(false);
                }
                self.hash_call_args(args);
            }
            ExprKind::Field { object, field } => {
                self.feed_tag(51);
                self.hash_expr(object);
                self.feed_str(field);
            }
            ExprKind::OptionalField { object, field } => {
                self.feed_tag(52);
                self.hash_expr(object);
                self.feed_str(field);
            }
            ExprKind::Index { object, index } => {
                self.feed_tag(53);
                self.hash_expr(object);
                self.hash_expr(index);
            }
            ExprKind::Block(stmts) => {
                self.feed_tag(54);
                self.hash_stmts(stmts);
            }
            ExprKind::If { cond, then_branch, else_branch } => {
                self.feed_tag(55);
                self.hash_expr(cond);
                self.hash_expr(then_branch);
                if let Some(eb) = else_branch {
                    self.feed_bool(true);
                    self.hash_expr(eb);
                } else {
                    self.feed_bool(false);
                }
            }
            ExprKind::IfLet { expr, pattern, then_branch, else_branch } => {
                self.feed_tag(56);
                self.hash_expr(expr);
                self.hash_pattern(pattern);
                self.hash_expr(then_branch);
                if let Some(eb) = else_branch {
                    self.feed_bool(true);
                    self.hash_expr(eb);
                } else {
                    self.feed_bool(false);
                }
            }
            ExprKind::GuardPattern { expr, pattern, else_branch } => {
                self.feed_tag(57);
                self.hash_expr(expr);
                self.hash_pattern(pattern);
                self.hash_expr(else_branch);
            }
            ExprKind::IsPattern { expr, pattern } => {
                self.feed_tag(58);
                self.hash_expr(expr);
                self.hash_pattern(pattern);
            }
            ExprKind::Match { scrutinee, arms } => {
                self.feed_tag(59);
                self.hash_expr(scrutinee);
                self.feed_u32(arms.len() as u32);
                for arm in arms {
                    self.hash_match_arm(arm);
                }
            }
            ExprKind::Try { expr, else_clause } => {
                self.feed_tag(60);
                self.hash_expr(expr);
                if let Some(ec) = else_clause {
                    self.feed_bool(true);
                    self.hash_try_else(ec);
                } else {
                    self.feed_bool(false);
                }
            }
            ExprKind::Unwrap { expr, message } => {
                self.feed_tag(61);
                self.hash_expr(expr);
                if let Some(m) = message {
                    self.feed_bool(true);
                    self.feed_str(m);
                } else {
                    self.feed_bool(false);
                }
            }
            ExprKind::NullCoalesce { value, default } => {
                self.feed_tag(62);
                self.hash_expr(value);
                self.hash_expr(default);
            }
            ExprKind::Range { start, end, inclusive } => {
                self.feed_tag(63);
                if let Some(s) = start {
                    self.feed_bool(true);
                    self.hash_expr(s);
                } else {
                    self.feed_bool(false);
                }
                if let Some(e) = end {
                    self.feed_bool(true);
                    self.hash_expr(e);
                } else {
                    self.feed_bool(false);
                }
                self.feed_bool(*inclusive);
            }
            ExprKind::StructLit { name, fields, spread } => {
                self.feed_tag(64);
                self.feed_str(name);
                self.feed_u32(fields.len() as u32);
                for fi in fields {
                    self.hash_field_init(fi);
                }
                if let Some(sp) = spread {
                    self.feed_bool(true);
                    self.hash_expr(sp);
                } else {
                    self.feed_bool(false);
                }
            }
            ExprKind::Array(elems) => {
                self.feed_tag(65);
                self.feed_u32(elems.len() as u32);
                for e in elems {
                    self.hash_expr(e);
                }
            }
            ExprKind::ArrayRepeat { value, count } => {
                self.feed_tag(66);
                self.hash_expr(value);
                self.hash_expr(count);
            }
            ExprKind::Tuple(elems) => {
                self.feed_tag(67);
                self.feed_u32(elems.len() as u32);
                for e in elems {
                    self.hash_expr(e);
                }
            }
            ExprKind::UsingBlock { name, args, body } => {
                self.feed_tag(68);
                self.feed_str(name);
                self.hash_call_args(args);
                self.hash_stmts(body);
            }
            ExprKind::WithAs { bindings, body } => {
                self.feed_tag(69);
                self.feed_u32(bindings.len() as u32);
                for wb in bindings {
                    self.hash_with_binding(wb);
                }
                self.hash_stmts(body);
            }
            ExprKind::Closure { params, ret_ty, body } => {
                self.feed_tag(70);
                self.feed_u32(params.len() as u32);
                for p in params {
                    self.hash_closure_param(p);
                }
                if let Some(rt) = ret_ty {
                    self.feed_bool(true);
                    self.feed_str(rt);
                } else {
                    self.feed_bool(false);
                }
                self.hash_expr(body);
            }
            ExprKind::Cast { expr, ty } => {
                self.feed_tag(71);
                self.hash_expr(expr);
                self.feed_str(ty);
            }
            ExprKind::Spawn { body } => {
                self.feed_tag(72);
                self.hash_stmts(body);
            }
            ExprKind::BlockCall { name, body } => {
                self.feed_tag(73);
                self.feed_str(name);
                self.hash_stmts(body);
            }
            ExprKind::Unsafe { body } => {
                self.feed_tag(74);
                self.hash_stmts(body);
            }
            ExprKind::Comptime { body } => {
                self.feed_tag(75);
                self.hash_stmts(body);
            }
            ExprKind::Loop { label, body } => {
                self.feed_tag(79);
                if let Some(lbl) = label {
                    self.feed_bool(true);
                    self.feed_str(lbl);
                } else {
                    self.feed_bool(false);
                }
                self.hash_stmts(body);
            }
            ExprKind::Select { arms, is_priority } => {
                self.feed_tag(76);
                self.feed_bool(*is_priority);
                self.feed_u32(arms.len() as u32);
                for arm in arms {
                    self.hash_select_arm(arm);
                }
            }
            ExprKind::Assert { condition, message } => {
                self.feed_tag(77);
                self.hash_expr(condition);
                if let Some(m) = message {
                    self.feed_bool(true);
                    self.hash_expr(m);
                } else {
                    self.feed_bool(false);
                }
            }
            ExprKind::Check { condition, message } => {
                self.feed_tag(78);
                self.hash_expr(condition);
                if let Some(m) = message {
                    self.feed_bool(true);
                    self.hash_expr(m);
                } else {
                    self.feed_bool(false);
                }
            }
        }
    }

    // ── Compound node hashing ─────────────────────────────────────

    fn hash_call_args(&mut self, args: &[CallArg]) {
        self.feed_u32(args.len() as u32);
        for arg in args {
            if let Some(n) = &arg.name {
                self.feed_bool(true);
                self.feed_str(n);
            } else {
                self.feed_bool(false);
            }
            self.feed_u8(match arg.mode {
                ArgMode::Default => 0,
                ArgMode::Own => 1,
                ArgMode::Mutate => 2,
            });
            self.hash_expr(&arg.expr);
        }
    }

    fn hash_match_arm(&mut self, arm: &MatchArm) {
        self.hash_pattern(&arm.pattern);
        if let Some(g) = &arm.guard {
            self.feed_bool(true);
            self.hash_expr(g);
        } else {
            self.feed_bool(false);
        }
        self.hash_expr(&arm.body);
    }

    fn hash_pattern(&mut self, pat: &Pattern) {
        match pat {
            Pattern::Wildcard => self.feed_tag(80),
            Pattern::Ident(name) => {
                self.feed_tag(81);
                self.feed_var(name);
            }
            Pattern::Literal(e) => {
                self.feed_tag(82);
                self.hash_expr(e);
            }
            Pattern::Constructor { name, fields } => {
                self.feed_tag(83);
                self.feed_str(name);
                self.feed_u32(fields.len() as u32);
                for f in fields {
                    self.hash_pattern(f);
                }
            }
            Pattern::Struct { name, fields, rest } => {
                self.feed_tag(84);
                self.feed_str(name);
                self.feed_u32(fields.len() as u32);
                for (fname, fpat) in fields {
                    self.feed_str(fname);
                    self.hash_pattern(fpat);
                }
                self.feed_bool(*rest);
            }
            Pattern::Tuple(pats) => {
                self.feed_tag(85);
                self.feed_u32(pats.len() as u32);
                for p in pats {
                    self.hash_pattern(p);
                }
            }
            Pattern::Or(pats) => {
                self.feed_tag(86);
                self.feed_u32(pats.len() as u32);
                for p in pats {
                    self.hash_pattern(p);
                }
            }
        }
    }

    fn hash_try_else(&mut self, te: &TryElse) {
        self.feed_var(&te.error_binding);
        self.hash_expr(&te.body);
    }

    fn hash_field_init(&mut self, fi: &FieldInit) {
        self.feed_str(&fi.name);
        self.hash_expr(&fi.value);
    }

    fn hash_with_binding(&mut self, wb: &WithBinding) {
        self.hash_expr(&wb.source);
        self.feed_var(&wb.name);
        self.feed_bool(wb.mutable);
    }

    fn hash_closure_param(&mut self, cp: &ClosureParam) {
        self.feed_var(&cp.name);
        if let Some(ty) = &cp.ty {
            self.feed_bool(true);
            self.feed_str(ty);
        } else {
            self.feed_bool(false);
        }
    }

    fn hash_select_arm(&mut self, arm: &SelectArm) {
        match &arm.kind {
            SelectArmKind::Recv { channel, binding } => {
                self.feed_u8(0);
                self.hash_expr(channel);
                self.feed_var(binding);
            }
            SelectArmKind::Send { channel, value } => {
                self.feed_u8(1);
                self.hash_expr(channel);
                self.hash_expr(value);
            }
            SelectArmKind::Default => {
                self.feed_u8(2);
            }
        }
        self.hash_expr(&arm.body);
    }
}

// ── Operator tag mappings ─────────────────────────────────────────

fn binop_tag(op: BinOp) -> u8 {
    match op {
        BinOp::Add => 0,
        BinOp::Sub => 1,
        BinOp::Mul => 2,
        BinOp::Div => 3,
        BinOp::Mod => 4,
        BinOp::Eq => 5,
        BinOp::Ne => 6,
        BinOp::Lt => 7,
        BinOp::Gt => 8,
        BinOp::Le => 9,
        BinOp::Ge => 10,
        BinOp::And => 11,
        BinOp::Or => 12,
        BinOp::BitAnd => 13,
        BinOp::BitOr => 14,
        BinOp::BitXor => 15,
        BinOp::Shl => 16,
        BinOp::Shr => 17,
    }
}

fn unaryop_tag(op: UnaryOp) -> u8 {
    match op {
        UnaryOp::Neg => 0,
        UnaryOp::Not => 1,
        UnaryOp::BitNot => 2,
        UnaryOp::Ref => 3,
        UnaryOp::Deref => 4,
    }
}

// ── Public API ────────────────────────────────────────────────────

/// Hash a function declaration. Variable names are normalized.
pub fn hash_function(f: &FnDecl) -> SemanticHash {
    let mut h = Hasher::new();
    h.hash_fn_decl(f);
    h.finish()
}

/// Hash any top-level declaration.
pub fn hash_decl(decl: &Decl) -> SemanticHash {
    let mut h = Hasher::new();
    h.hash_decl(decl);
    h.finish()
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rask_ast::decl::{DeclKind, FnDecl, Param, TypeParam};
    use rask_ast::expr::{CallArg, ArgMode, Expr, ExprKind};
    use rask_ast::stmt::{Stmt, StmtKind};
    use rask_ast::{NodeId, Span};

    fn sp() -> Span { Span::new(0, 0) }

    fn ident(name: &str) -> Expr {
        Expr { id: NodeId(0), kind: ExprKind::Ident(name.into()), span: sp() }
    }

    fn int(v: i64) -> Expr {
        Expr { id: NodeId(0), kind: ExprKind::Int(v, None), span: sp() }
    }

    fn call(func: &str, args: Vec<Expr>) -> Expr {
        Expr {
            id: NodeId(0),
            kind: ExprKind::Call {
                func: Box::new(ident(func)),
                args: args.into_iter().map(|e| CallArg { name: None, mode: ArgMode::Default, expr: e }).collect(),
            },
            span: sp(),
        }
    }

    fn return_stmt(val: Option<Expr>) -> Stmt {
        Stmt { id: NodeId(0), kind: StmtKind::Return(val), span: sp() }
    }

    fn const_stmt(name: &str, init: Expr) -> Stmt {
        Stmt {
            id: NodeId(0),
            kind: StmtKind::Const { name: name.into(), name_span: sp(), ty: None, init },
            span: sp(),
        }
    }

    fn make_fn(name: &str, params: Vec<(&str, &str)>, body: Vec<Stmt>) -> FnDecl {
        FnDecl {
            name: name.into(),
            type_params: vec![],
            params: params.into_iter().map(|(n, ty)| Param {
                name: n.into(),
                name_span: sp(),
                ty: ty.into(),
                is_take: false,
                is_mutate: false,
                default: None,
            }).collect(),
            ret_ty: None,
            context_clauses: vec![],
            body,
            is_pub: false,
            is_private: false,
            is_comptime: false,
            is_unsafe: false,
            abi: None,
            attrs: vec![],
            doc: None,
            span: sp(),
        }
    }

    #[test]
    fn same_function_same_hash() {
        let f1 = make_fn("add", vec![("a", "i32"), ("b", "i32")], vec![
            return_stmt(Some(call("add", vec![ident("a"), ident("b")])))
        ]);
        let f2 = make_fn("add", vec![("a", "i32"), ("b", "i32")], vec![
            return_stmt(Some(call("add", vec![ident("a"), ident("b")])))
        ]);
        assert_eq!(hash_function(&f1), hash_function(&f2));
    }

    #[test]
    fn renamed_variables_same_hash() {
        // H4: Renaming x→a, y→b should produce the same hash
        let f1 = make_fn("add", vec![("x", "i32"), ("y", "i32")], vec![
            return_stmt(Some(call("add", vec![ident("x"), ident("y")])))
        ]);
        let f2 = make_fn("add", vec![("a", "i32"), ("b", "i32")], vec![
            return_stmt(Some(call("add", vec![ident("a"), ident("b")])))
        ]);
        assert_eq!(hash_function(&f1), hash_function(&f2));
    }

    #[test]
    fn different_body_different_hash() {
        let f1 = make_fn("add", vec![("a", "i32")], vec![
            return_stmt(Some(ident("a")))
        ]);
        let f2 = make_fn("add", vec![("a", "i32")], vec![
            return_stmt(Some(int(42)))
        ]);
        assert_ne!(hash_function(&f1), hash_function(&f2));
    }

    #[test]
    fn different_name_different_hash() {
        let f1 = make_fn("foo", vec![], vec![return_stmt(None)]);
        let f2 = make_fn("bar", vec![], vec![return_stmt(None)]);
        assert_ne!(hash_function(&f1), hash_function(&f2));
    }

    #[test]
    fn different_type_different_hash() {
        let f1 = make_fn("f", vec![("x", "i32")], vec![return_stmt(None)]);
        let f2 = make_fn("f", vec![("x", "i64")], vec![return_stmt(None)]);
        assert_ne!(hash_function(&f1), hash_function(&f2));
    }

    #[test]
    fn node_id_irrelevant() {
        // H3: Different NodeIds should produce the same hash
        let f1 = make_fn("f", vec![], vec![Stmt {
            id: NodeId(100),
            kind: StmtKind::Return(None),
            span: sp(),
        }]);
        let f2 = make_fn("f", vec![], vec![Stmt {
            id: NodeId(999),
            kind: StmtKind::Return(None),
            span: sp(),
        }]);
        assert_eq!(hash_function(&f1), hash_function(&f2));
    }

    #[test]
    fn span_irrelevant() {
        // H3: Different Spans should produce the same hash
        let f1 = make_fn("f", vec![], vec![Stmt {
            id: NodeId(0),
            kind: StmtKind::Return(None),
            span: Span::new(0, 10),
        }]);
        let f2 = make_fn("f", vec![], vec![Stmt {
            id: NodeId(0),
            kind: StmtKind::Return(None),
            span: Span::new(100, 200),
        }]);
        assert_eq!(hash_function(&f1), hash_function(&f2));
    }

    #[test]
    fn extra_statement_different_hash() {
        let f1 = make_fn("f", vec![], vec![return_stmt(None)]);
        let f2 = make_fn("f", vec![], vec![
            const_stmt("x", int(1)),
            return_stmt(None),
        ]);
        assert_ne!(hash_function(&f1), hash_function(&f2));
    }

    #[test]
    fn swapped_params_different_hash() {
        // Same var names, different parameter order → different positional indices
        let f1 = make_fn("f", vec![("a", "i32"), ("b", "i64")], vec![
            return_stmt(Some(ident("a")))
        ]);
        let f2 = make_fn("f", vec![("b", "i64"), ("a", "i32")], vec![
            return_stmt(Some(ident("b")))  // "b" at position 0, like "a" was in f1
        ]);
        // These have different types in different parameter positions → different hash
        assert_ne!(hash_function(&f1), hash_function(&f2));
    }

    #[test]
    fn visibility_matters() {
        let mut f1 = make_fn("f", vec![], vec![return_stmt(None)]);
        let mut f2 = make_fn("f", vec![], vec![return_stmt(None)]);
        f1.is_pub = false;
        f2.is_pub = true;
        assert_ne!(hash_function(&f1), hash_function(&f2));
    }

    #[test]
    fn literal_values_matter() {
        let f1 = make_fn("f", vec![], vec![return_stmt(Some(int(1)))]);
        let f2 = make_fn("f", vec![], vec![return_stmt(Some(int(2)))]);
        assert_ne!(hash_function(&f1), hash_function(&f2));
    }
}
