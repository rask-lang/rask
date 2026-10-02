// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Derived `eq` and `hash` (EQ1/EQ3/HA1), written out as ordinary methods.
//!
//! A struct or enum that doesn't write its own gets a body here, before name
//! resolution, so it is resolved, checked, monomorphized and run like any
//! method somebody typed. Both backends then answer `==`, `.hash()` and a map
//! key's bucket from the same code. They used to answer from three places: the
//! interpreter walked values structurally, native codegen compared struct
//! fields itself and had no `hash` at all, and neither ever called a `Vec`
//! field's own comparison (#1391).
//!
//! Whether the type actually is `Equal`/`Hashable` is still the checker's
//! call: it only knows once every field type is known. The methods carry
//! `@derived`, and the checker keeps the ones it derives and ignores the rest
//! (a struct with a closure field gets an `eq` body here that is never
//! checked and never called).

use std::collections::{HashMap, HashSet};

use rask_ast::decl::{Decl, DeclKind, FnDecl, Param, TypeParam, DERIVED_ATTR};
use rask_ast::expr::{BinOp, CallArg, ArgMode, Expr, ExprKind, MatchArm, Pattern};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::token::IntSuffix;
use rask_ast::ty::TypeExpr;
use rask_ast::{NodeId, Span};

/// FNV-1a's offset basis and prime — what `rask_int_hash` and the string hash
/// use, so a struct's hash is built from the same mixing as its fields'.
const FNV_OFFSET: i128 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: i128 = 0x0000_0100_0000_01b3;

const SP: Span = Span::new(0, 0);

pub(crate) fn inject(decls: &mut [Decl]) {
    // Method names each type has from anywhere: its declaration, and every
    // `extend` block on it, interface defaults included.
    let mut owned: HashMap<String, HashSet<String>> = HashMap::new();
    for decl in decls.iter() {
        let (ty, methods) = match &decl.kind {
            DeclKind::Struct(s) => (s.name.clone(), &s.methods),
            DeclKind::Enum(e) => (e.name.clone(), &e.methods),
            DeclKind::Impl(i) => (i.target_ty.name().unwrap_or_default(), &i.methods),
            _ => continue,
        };
        owned.entry(ty).or_default().extend(methods.iter().map(|m| m.name.clone()));
    }
    let annotations: HashSet<String> = decls
        .iter()
        .filter_map(|d| match &d.kind {
            DeclKind::Annotation(a) => Some(a.name.clone()),
            _ => None,
        })
        .collect();

    for decl in decls.iter_mut() {
        match &mut decl.kind {
            DeclKind::Struct(s) => {
                // An annotation declares a comptime-only struct: no layout,
                // nothing to compare.
                if annotations.contains(&s.name) {
                    continue;
                }
                let has = owned.get(&s.name).cloned().unwrap_or_default();
                let self_ty = self_type(&s.name, &s.type_params);
                let fields: Vec<(String, TypeExpr)> =
                    s.fields.iter().map(|f| (f.name.clone(), f.ty.clone())).collect();
                if !has.contains("eq") {
                    s.methods.push(method("eq", &self_ty, true, Type::Bool, struct_eq(&fields), s.is_pub));
                }
                if !has.contains("hash") {
                    s.methods.push(method("hash", &self_ty, false, Type::U64, struct_hash(&fields), s.is_pub));
                }
            }
            DeclKind::Enum(e) if !e.variants.is_empty() => {
                let has = owned.get(&e.name).cloned().unwrap_or_default();
                let self_ty = self_type(&e.name, &e.type_params);
                let variants: Vec<(String, Vec<TypeExpr>)> = e
                    .variants
                    .iter()
                    .map(|v| (format!("{}.{}", e.name, v.name), v.fields.iter().map(|f| f.ty.clone()).collect()))
                    .collect();
                if !has.contains("eq") {
                    e.methods.push(method("eq", &self_ty, true, Type::Bool, enum_eq(&variants), e.is_pub));
                }
                if !has.contains("hash") {
                    e.methods.push(method("hash", &self_ty, false, Type::U64, enum_hash(&variants), e.is_pub));
                }
            }
            _ => {}
        }
    }
}

enum Type {
    Bool,
    U64,
}

fn self_type(name: &str, params: &[TypeParam]) -> TypeExpr {
    TypeExpr::generic(name, params.iter().map(|p| TypeExpr::named(p.name.as_str())).collect())
}

fn method(name: &str, self_ty: &TypeExpr, takes_other: bool, ret: Type, body: Vec<Stmt>, is_pub: bool) -> FnDecl {
    let param = |n: &str| Param {
        name: n.to_string(),
        name_span: SP,
        ty: Some(self_ty.clone()),
        is_take: false,
        is_mutate: false,
        is_deleting: false,
        default: None,
    };
    let mut params = vec![param("self")];
    if takes_other {
        params.push(param("other"));
    }
    FnDecl {
        name: name.to_string(),
        type_params: Vec::new(),
        params,
        ret_ty: Some(TypeExpr::named(match ret {
            Type::Bool => "bool",
            Type::U64 => "u64",
        })),
        body,
        is_pub,
        is_private: false,
        is_comptime: false,
        is_unsafe: false,
        abi: None,
        attrs: vec![DERIVED_ATTR.to_string()],
        doc: None,
        span: SP,
        decl_start: 0,
    }
}

// ─── Bodies ────────────────────────────────────────────────────

/// `return self.a == other.a && self.b == other.b`
fn struct_eq(fields: &[(String, TypeExpr)]) -> Vec<Stmt> {
    let mut names = Names::default();
    let compares: Vec<Expr> = fields
        .iter()
        .map(|(f, ty)| eq_of(field(ident("self"), f), field(ident("other"), f), ty, &mut names))
        .collect();
    vec![ret(all(compares.into_iter()))]
}

/// FNV over the fields' own hashes, in declaration order.
fn struct_hash(fields: &[(String, TypeExpr)]) -> Vec<Stmt> {
    let mut names = Names::default();
    let hashes: Vec<Expr> = fields
        .iter()
        .map(|(f, ty)| hash_of(field(ident("self"), f), ty, &mut names))
        .collect();
    vec![ret(mix(u64_lit(FNV_OFFSET), hashes.into_iter()))]
}

/// ```text
/// match self {
///     E.A(a0, a1) => match other {
///         E.A(b0, b1) => return a0 == b0 && a1 == b1
///         _ => return false
///     }
///     E.B => match other { E.B => return true, _ => return false }
/// }
/// ```
fn enum_eq(variants: &[(String, Vec<TypeExpr>)]) -> Vec<Stmt> {
    let single = variants.len() == 1;
    let mut names = Names::default();
    let arms = variants
        .iter()
        .map(|(name, tys)| {
            let n = tys.len();
            let compares: Vec<Expr> = tys
                .iter()
                .enumerate()
                .map(|(i, ty)| eq_of(ident(&format!("a{i}")), ident(&format!("b{i}")), ty, &mut names))
                .collect();
            let mut inner = vec![arm(variant_pattern(name, n, "b"), ret_expr(all(compares.into_iter())))];
            // One variant: a wildcard would never match, and the checker says so.
            if !single {
                inner.push(arm(Pattern::Wildcard, ret_expr(bool_lit(false))));
            }
            arm(variant_pattern(name, n, "a"), expr(ExprKind::Match { scrutinee: Box::new(ident("other")), arms: inner }))
        })
        .collect();
    vec![stmt(StmtKind::Expr(expr(ExprKind::Match { scrutinee: Box::new(ident("self")), arms })))]
}

/// The variant's position, then its payload's hashes.
fn enum_hash(variants: &[(String, Vec<TypeExpr>)]) -> Vec<Stmt> {
    let mut names = Names::default();
    let arms = variants
        .iter()
        .enumerate()
        .map(|(index, (name, tys))| {
            let mut parts = vec![u64_lit(index as i128)];
            for (i, ty) in tys.iter().enumerate() {
                parts.push(hash_of(ident(&format!("a{i}")), ty, &mut names));
            }
            arm(variant_pattern(name, tys.len(), "a"), ret_expr(mix(u64_lit(FNV_OFFSET), parts.into_iter())))
        })
        .collect();
    vec![stmt(StmtKind::Expr(expr(ExprKind::Match { scrutinee: Box::new(ident("self")), arms })))]
}

/// Fresh binding names for the matches `hash_of` writes.
#[derive(Default)]
struct Names(usize);

impl Names {
    fn next(&mut self) -> String {
        self.0 += 1;
        format!("__h{}", self.0)
    }
}

/// The hash of `value`, a field or payload written as `ty`.
///
/// A named type hashes through its own `hash`. An optional, a result and a
/// tuple have no methods (the wrappers are operator-only), so their hash is
/// spelled out here from their parts, the way `==` on them compares parts.
/// The type as written is what's known before name resolution; an alias that
/// hides a wrapper still reaches `.hash()`, and the checker says so there.
fn hash_of(value: Expr, ty: &TypeExpr, names: &mut Names) -> Expr {
    match ty {
        // `if value? as v { mix(1, v) } else { 0 }`
        TypeExpr::Optional(inner) => {
            let v = names.next();
            let some = mix(u64_lit(1), std::iter::once(hash_of(ident(&v), inner, names)));
            present(value, v, some, u64_lit(0))
        }
        // `match value { T as o => mix(1, o), E as e => mix(2, e) }`. Not
        // `value?`: on a result that asks whether it failed, and is refused.
        TypeExpr::Result { ok, err } => {
            let (o, e) = (names.next(), names.next());
            let ok_hash = mix(u64_lit(1), std::iter::once(hash_of(ident(&o), ok, names)));
            let err_hash = mix(u64_lit(2), std::iter::once(hash_of(ident(&e), err, names)));
            expr(ExprKind::Match {
                scrutinee: Box::new(value),
                arms: vec![
                    arm(Pattern::TypePat { ty: (**ok).clone(), binding: Some(o) }, ok_hash),
                    arm(Pattern::TypePat { ty: (**err).clone(), binding: Some(e) }, err_hash),
                ],
            })
        }
        TypeExpr::Tuple(elems) => {
            let parts: Vec<Expr> = elems
                .iter()
                .enumerate()
                .map(|(i, t)| hash_of(field(value.clone(), &i.to_string()), t, names))
                .collect();
            mix(u64_lit(FNV_OFFSET), parts.into_iter())
        }
        _ => call(value, "hash", vec![]),
    }
}

/// `((seed ^ x0).wrapping_mul(P) ^ x1).wrapping_mul(P) …`
fn mix(seed: Expr, parts: impl Iterator<Item = Expr>) -> Expr {
    parts.fold(seed, |h, x| call(binary(BinOp::BitXor, h, x), "wrapping_mul", vec![u64_lit(FNV_PRIME)]))
}

fn all(mut compares: impl Iterator<Item = Expr>) -> Expr {
    match compares.next() {
        None => bool_lit(true),
        Some(first) => compares.fold(first, |acc, c| binary(BinOp::And, acc, c)),
    }
}

/// `a == b` for two values written as `ty`.
///
/// `==` covers everything but a result: `T or E` has no `eq` (the wrappers
/// are operator-only, and equality on one isn't an operator the language
/// defines), yet a struct holding one is Equal when both sides are. So the
/// comparison is written out, side by side.
fn eq_of(a: Expr, b: Expr, ty: &TypeExpr, names: &mut Names) -> Expr {
    let TypeExpr::Result { ok, err } = ty else {
        return binary(BinOp::Eq, a, b);
    };
    let side = |pat_ty: &TypeExpr, names: &mut Names| {
        let (x, y) = (names.next(), names.next());
        let inner = expr(ExprKind::Match {
            scrutinee: Box::new(b.clone()),
            arms: vec![
                arm(Pattern::TypePat { ty: pat_ty.clone(), binding: Some(y.clone()) },
                    eq_of(ident(&x), ident(&y), pat_ty, names)),
                arm(Pattern::Wildcard, bool_lit(false)),
            ],
        });
        arm(Pattern::TypePat { ty: pat_ty.clone(), binding: Some(x) }, inner)
    };
    let arms = vec![side(ok, names), side(err, names)];
    expr(ExprKind::Match { scrutinee: Box::new(a), arms })
}

fn present(value: Expr, bind: String, then: Expr, otherwise: Expr) -> Expr {
    expr(ExprKind::If {
        cond: Box::new(expr(ExprKind::IsPresent { expr: Box::new(value), binding: Some(bind) })),
        then_branch: Box::new(block_value(then)),
        else_branch: Some(Box::new(block_value(otherwise))),
        else_binding: None,
    })
}

fn block_value(e: Expr) -> Expr {
    expr(ExprKind::Block(vec![stmt(StmtKind::Expr(e))]))
}

fn variant_pattern(name: &str, n: usize, prefix: &str) -> Pattern {
    if n == 0 {
        Pattern::Ident(name.to_string())
    } else {
        Pattern::Constructor {
            name: name.to_string(),
            fields: (0..n).map(|i| Pattern::Ident(format!("{prefix}{i}"))).collect(),
        }
    }
}

// ─── AST shorthands ────────────────────────────────────────────

fn expr(kind: ExprKind) -> Expr {
    Expr { id: NodeId(0), kind, span: SP }
}

fn stmt(kind: StmtKind) -> Stmt {
    Stmt { id: NodeId(0), kind, span: SP }
}

fn ident(name: &str) -> Expr {
    expr(ExprKind::Ident(name.to_string()))
}

fn field(object: Expr, name: &str) -> Expr {
    expr(ExprKind::Field { object: Box::new(object), field: name.to_string() })
}

fn binary(op: BinOp, left: Expr, right: Expr) -> Expr {
    expr(ExprKind::Binary { op, left: Box::new(left), right: Box::new(right) })
}

fn call(object: Expr, method: &str, args: Vec<Expr>) -> Expr {
    expr(ExprKind::MethodCall {
        object: Box::new(object),
        method: method.to_string(),
        type_args: None,
        args: args.into_iter().map(|e| CallArg { name: None, mode: ArgMode::Default, expr: e }).collect(),
    })
}

fn u64_lit(v: i128) -> Expr {
    expr(ExprKind::Int(v, Some(IntSuffix::U64)))
}

fn bool_lit(b: bool) -> Expr {
    expr(ExprKind::Bool(b))
}

fn ret(e: Expr) -> Stmt {
    stmt(StmtKind::Return(Some(e)))
}

fn ret_expr(e: Expr) -> Expr {
    expr(ExprKind::Block(vec![ret(e)]))
}

fn arm(pattern: Pattern, body: Expr) -> MatchArm {
    MatchArm { pattern, guard: None, body: Box::new(body) }
}
