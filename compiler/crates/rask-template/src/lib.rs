// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Templates: a generic function whose body has comptime control, written out
//! as a plain function for each instance the program uses (ctrl.comptime/CT72).
//!
//! A `comptime for` over `reflect.fields<T>()` can't be checked once for every
//! `T`: what `v.(f.name)` is, and so what a generic call fed from it infers,
//! differs per field and per instance. So the body isn't checked as written.
//! For `gather<Moving>`, the copy `gather$Moving` has `Moving` where `Q` was,
//! the loop unrolled once per field with each field's facts written in
//! (`f.name` → `"p"`), `q.(f.name)` written `q.p`, and each `comptime if` on
//! the item decided. That's code someone could have written by hand, and it
//! is checked and compiled like code written by hand.
//!
//! The driver (`rask-compiler`) checks, asks monomorphization which template
//! instances are reachable, expands the new ones with `expand_round`, and
//! checks again until a round adds none. Monomorphization takes the copy for
//! the instance it stands for (`reachability.rs`, `enqueue`).

use std::collections::{HashMap, HashSet};

use rask_ast::attr::AttrValue;
use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::expr::{Expr, ExprKind, Pattern};
use rask_ast::rewrite::{self, Rewrite};
use rask_ast::stmt::{ForBinding, Stmt, StmtKind};
use rask_ast::token::IntSuffix;
use rask_ast::ty::TypeExpr;
use rask_ast::{NodeId, Span};
use rask_check::TypedProgram;
use rask_mono::MonoProgram;
use rask_types::{Type, TypeBinding, TypeDef};

/// Node ids for template copies start here, above every other producer's
/// range (`rask_desugar::DESUGAR_ID_BASE` and the rest).
pub const TEMPLATE_ID_BASE: u32 = 60_000_000;

/// One field of a struct, as `reflect.fields<T>()` reports it. The same facts
/// the backends unrolled with, so a copy says what they said.
#[derive(Debug, Clone)]
pub struct FieldFact {
    pub name: String,
    pub type_name: String,
    pub offset: u32,
    pub size: u32,
    pub is_public: bool,
    pub serial_name: String,
    pub is_skipped: bool,
    pub has_default: bool,
    pub attrs: Vec<rask_ast::attr::Attr>,
}

/// The fields of `ty` in declaration order, from the layout monomorphization
/// gave it.
pub fn field_facts(typed: &TypedProgram, mono: &MonoProgram, ty: &Type) -> Option<Vec<FieldFact>> {
    use rask_ast::decl::field_attrs;
    let types = &typed.types;
    let resolved = ty.clone();
    let layout = rask_layout::struct_for(&resolved, &mono.struct_layouts)?;
    let (declared, subst) = match &resolved {
        Type::Named(id) | Type::Generic { base: id, .. } => match types.get(*id) {
            Some(TypeDef::Struct { fields, type_params, .. }) => {
                let args: Vec<TypeExpr> = match &resolved {
                    Type::Generic { args, .. } => args
                        .iter()
                        .filter_map(|a| match a {
                            rask_types::GenericArg::Type(t) => Some(rask_check::reflect::type_label(types, t)),
                            _ => None,
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                (Some(fields), type_params.iter().cloned().zip(args).collect::<Vec<_>>())
            }
            _ => (None, Vec::new()),
        },
        _ => (None, Vec::new()),
    };
    let mut ordered: Vec<&rask_layout::FieldLayout> = layout.fields.iter().collect();
    ordered.sort_by_key(|fl| fl.decl_index);
    Some(
        ordered
            .into_iter()
            .map(|fl| {
                let declared_ty = declared
                    .and_then(|fs| fs.iter().find(|(n, _)| *n == fl.name))
                    .map_or(&fl.ty, |(_, t)| t);
                FieldFact {
                    name: fl.name.clone(),
                    type_name: rask_check::reflect::substituted_label(types, declared_ty, &subst),
                    offset: fl.offset,
                    size: fl.size,
                    is_public: fl.is_public,
                    serial_name: field_attrs::serial_name(&fl.attrs, &fl.name),
                    is_skipped: field_attrs::is_skipped(&fl.attrs),
                    has_default: fl.has_declared_default || field_attrs::default_value(&fl.attrs).is_some(),
                    attrs: fl.attrs.clone(),
                }
            })
            .collect(),
    )
}

/// A generic function whose body has comptime control to expand per instance.
pub fn is_template(f: &FnDecl) -> bool {
    !f.type_params.is_empty() && has_comptime_for(&f.body)
}

fn has_comptime_for(body: &[Stmt]) -> bool {
    struct Find(bool);
    impl Rewrite for Find {
        fn body(&mut self, b: &mut Vec<Stmt>) {
            if b.iter().any(|s| matches!(s.kind, StmtKind::ComptimeFor { .. })) {
                self.0 = true;
            }
        }
    }
    let mut copy = body.to_vec();
    let mut find = Find(false);
    find.body(&mut copy);
    for s in &mut copy {
        rewrite::rewrite_stmt(s, &mut find);
    }
    find.0
}

/// Something a copy couldn't be written for.
#[derive(Debug, Clone)]
pub struct ExpandError {
    pub message: String,
    pub span: Span,
}

/// What one round added: the copies, and each copied node's original, so a
/// fact recorded against the original (`DesugarOutput::operator_calls`)
/// carries over.
#[derive(Default)]
pub struct Expansion {
    pub decls: Vec<Decl>,
    pub copied_from: Vec<(NodeId, NodeId)>,
    /// Each copy's template, by its span, and the instance it was written
    /// for as a reader would write it (`zero_all<Point>`): what an error
    /// inside the copy names (CT72).
    pub instances: Vec<(Span, String)>,
    /// A function or test with comptime control and no type parameters,
    /// written out in its own place: the declaration it replaces, by id, and
    /// the replacement.
    pub replaced: Vec<(NodeId, Decl)>,
}

/// Whether a declaration has anything for this crate to write out: a
/// template, or a body written out in its own place.
pub fn has_comptime_control(d: &Decl) -> bool {
    matches!(&d.kind, DeclKind::Fn(f) if is_template(f)) || in_place(d)
}

/// A body with comptime control the program can write out where it stands:
/// a function with no type parameters, or a test.
fn in_place(d: &Decl) -> bool {
    match &d.kind {
        DeclKind::Fn(f) => f.type_params.is_empty() && has_comptime_for(&f.body),
        DeclKind::Test(t) => has_comptime_for(&t.body),
        _ => false,
    }
}

/// Write out each reachable template instance not written out yet.
///
/// `templates` are the declarations as desugaring left them, before checking
/// changed them; `done` names the copies already made, by the instance's
/// mangled name, which is also the copy's.
pub fn expand_round(
    templates: &[Decl],
    typed: &TypedProgram,
    mono: &MonoProgram,
    done: &mut HashSet<String>,
    next_id: &mut u32,
) -> Result<Expansion, Vec<ExpandError>> {
    // By the function's own id: a method sharing a template's name (`show`
    // on a generic type, `map` on a range) is some other function.
    let by_id: HashMap<NodeId, &Decl> = templates
        .iter()
        .filter_map(|d| match &d.kind {
            DeclKind::Fn(f) if is_template(f) => Some((f.id, d)),
            _ => None,
        })
        .collect();
    let mut out = Expansion::default();
    let mut errors = Vec::new();
    let known = Known::new(templates, typed);
    for inst in &mono.functions {
        if inst.type_args.is_empty() || done.contains(&inst.name) {
            continue;
        }
        let DeclKind::Fn(generic) = &inst.body.kind else { continue };
        let Some(template) = by_id.get(&generic.id) else { continue };
        // An argument not settled yet (`count<_>`, reached through a field
        // the first round couldn't type) waits for the round whose copy
        // names it.
        if inst.type_args.iter().any(|b| b.ty.has_unsolved_var() || b.ty.contains(&|t| matches!(t, Type::Error))) {
            continue;
        }
        done.insert(inst.name.clone());
        let shown: Vec<String> = inst
            .type_args
            .iter()
            .map(|b| rask_check::reflect::type_label(&typed.types, &b.ty).source())
            .collect();
        out.instances.push((template.span, format!("{}<{}>", generic.name, shown.join(", "))));
        let mut ex = Expander { typed, mono, next_id, copied_from: Vec::new(), errors: Vec::new(), known: &known };
        let copy = ex.expand(template, &inst.name, &inst.type_args);
        let shown_instance = out.instances.last().map(|(_, n)| n.clone()).unwrap_or_default();
        errors.extend(ex.errors.into_iter().map(|mut e| {
            e.message = format!("{} (writing out `{}`)", e.message, shown_instance);
            e
        }));
        out.copied_from.extend(ex.copied_from);
        out.decls.push(copy);
    }
    // Nothing to instantiate: the loop's type is already known. One that
    // can't be written out yet (a layout mono didn't make, for code nothing
    // reaches) stays as it is, for the backends' own unrolling.
    for decl in templates.iter().filter(|d| in_place(d)) {
        let mut ex = Expander { typed, mono, next_id, copied_from: Vec::new(), errors: Vec::new(), known: &known };
        let copy = ex.expand_in_place(decl);
        if ex.errors.is_empty() {
            out.copied_from.extend(ex.copied_from);
            out.replaced.push((decl.id, copy));
        }
    }
    if errors.is_empty() {
        Ok(out)
    } else {
        Err(errors)
    }
}

struct Expander<'a> {
    typed: &'a TypedProgram,
    mono: &'a MonoProgram,
    next_id: &'a mut u32,
    copied_from: Vec<(NodeId, NodeId)>,
    errors: Vec<ExpandError>,
    known: &'a Known<'a>,
}

/// What a `comptime if` can read besides the loop item: the module's consts,
/// and a reflect question with one answer (`reflect.name_of<string>()`).
struct Known<'a> {
    consts: HashMap<String, rask_ast::comptime_if::Fact>,
    decls: TemplateDecls<'a>,
}

impl<'a> Known<'a> {
    fn new(templates: &'a [Decl], typed: &'a TypedProgram) -> Self {
        let consts = rask_ast::comptime_if::const_facts(templates.iter().filter_map(|d| match &d.kind {
            DeclKind::Const(c) => Some((c.name.as_str(), &c.init, c.ty.as_ref().and_then(|t| t.bare_name()))),
            _ => None,
        }));
        Known { consts, decls: TemplateDecls { decls: templates, typed } }
    }

    fn fact(&self, e: &Expr) -> Option<rask_ast::comptime_if::Fact> {
        use rask_ast::comptime_if::Fact;
        use rask_check::reflect::ReflectAnswer;
        match &e.kind {
            ExprKind::Ident(n) => self.consts.get(n).cloned(),
            ExprKind::MethodCall { object, method, type_args: Some(args), .. }
                if matches!(&object.kind, ExprKind::Ident(n) if n == "reflect") =>
            {
                match rask_check::reflect::answer(method, args.first()?, &self.decls) {
                    ReflectAnswer::Bool(b) => Some(Fact::Bool(b)),
                    ReflectAnswer::Int(n) => Some(Fact::Int(n as i128, None)),
                    ReflectAnswer::Str(s) => Some(Fact::Str(s)),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

/// The program's declarations, as reflection asks about them.
struct TemplateDecls<'a> {
    decls: &'a [Decl],
    typed: &'a TypedProgram,
}

impl TemplateDecls<'_> {
    fn named(&self, name: &str) -> Option<&Decl> {
        self.decls.iter().find(|d| d.name() == Some(name))
    }
}

impl rask_check::reflect::ReflectDecls for TemplateDecls<'_> {
    fn declares_struct(&self, name: &str) -> bool {
        matches!(self.named(name).map(|d| &d.kind), Some(DeclKind::Struct(_)))
    }
    fn declares_enum(&self, name: &str) -> bool {
        matches!(self.named(name).map(|d| &d.kind), Some(DeclKind::Enum(_)))
    }
    fn is_resource(&self, name: &str) -> bool {
        match self.named(name).map(|d| &d.kind) {
            Some(DeclKind::Struct(s)) => s.attrs.iter().any(|a| a.is("resource")),
            Some(DeclKind::Enum(e)) => e.attrs.iter().any(|a| a.is("resource")),
            _ => false,
        }
    }
    fn member_types(&self, name: &str) -> Option<Vec<TypeExpr>> {
        match self.named(name).map(|d| &d.kind) {
            Some(DeclKind::Struct(s)) => Some(s.fields.iter().map(|f| f.ty.clone()).collect()),
            Some(DeclKind::Enum(e)) => {
                Some(e.variants.iter().flat_map(|v| v.fields.iter().map(|f| f.ty.clone())).collect())
            }
            _ => None,
        }
    }
    fn type_params(&self, name: &str) -> Vec<String> {
        match self.named(name).map(|d| &d.kind) {
            Some(DeclKind::Struct(s)) => s.type_params.iter().map(|t| t.name.clone()).collect(),
            Some(DeclKind::Enum(e)) => e.type_params.iter().map(|t| t.name.clone()).collect(),
            _ => Vec::new(),
        }
    }
    fn type_table(&self) -> &rask_check::TypeTable {
        &self.typed.types
    }
}

impl Expander<'_> {
    fn expand_in_place(&mut self, decl: &Decl) -> Decl {
        let mut decl = decl.clone();
        let none = HashMap::new();
        rewrite::rewrite_decl(&mut decl, &mut Unroll { ex: self, concrete: &HashMap::new(), written: &none });
        let mut fresh = Fresh { next: self.next_id, copied_from: &mut self.copied_from };
        rewrite::rewrite_decl(&mut decl, &mut fresh);
        decl.id = fresh.fresh(decl.id);
        decl
    }

    fn expand(&mut self, template: &Decl, name: &str, type_args: &[TypeBinding]) -> Decl {
        let mut decl = template.clone();
        let DeclKind::Fn(f) = &mut decl.kind else { unreachable!("templates are functions") };
        let types = &self.typed.types;
        let written: HashMap<String, TypeExpr> = type_args
            .iter()
            .map(|b| {
                let named = rask_mono::Monomorphizer::nameable_type(&b.ty, types).unwrap_or_else(|| b.ty.clone());
                (b.param.clone(), named.to_type_expr())
            })
            .collect();
        let concrete: HashMap<String, Type> = type_args.iter().map(|b| (b.param.clone(), b.ty.clone())).collect();
        f.name = name.to_string();
        f.type_params.clear();
        f.attrs.push(copy_marker(f.id));
        rewrite::rewrite_decl(&mut decl, &mut TypeParams { written: &written });
        rewrite::rewrite_decl(&mut decl, &mut Unroll { ex: self, concrete: &concrete, written: &written });
        let mut fresh = Fresh { next: self.next_id, copied_from: &mut self.copied_from };
        rewrite::rewrite_decl(&mut decl, &mut fresh);
        decl.id = fresh.fresh(decl.id);
        decl
    }
}

/// Marks a copy with the template it was written out from, by the template
/// function's id: another function can share the template's name (a method
/// `dump` beside a free `dump`), and a caller looking for the copy has to
/// find this template's.
pub const COPY_OF: &str = "template_copy_of";

fn copy_marker(template: NodeId) -> rask_ast::attr::Attr {
    let id = Expr { id: NodeId::DUMMY, kind: ExprKind::Int(template.0 as i128, None), span: Span::new(0, 0) };
    rask_ast::attr::Attr::synthetic(
        COPY_OF,
        Some(vec![rask_ast::attr::AttrArg { label: None, value: AttrValue::Expr(id) }]),
    )
}

/// The template a copy was written out from, by its function's id.
pub fn copy_of(f: &FnDecl) -> Option<NodeId> {
    let attr = f.attrs.iter().find(|a| a.is(COPY_OF))?;
    match &attr.args.as_ref()?.first()?.value {
        AttrValue::Expr(Expr { kind: ExprKind::Int(n, _), .. }) => Some(NodeId(*n as u32)),
        _ => None,
    }
}

/// The instance's types where the template names its parameters: in written
/// types, as a call's receiver (`T.zero()`) and as a literal's name (`T(…)`).
struct TypeParams<'a> {
    written: &'a HashMap<String, TypeExpr>,
}

impl Rewrite for TypeParams<'_> {
    fn ty(&mut self, t: &mut TypeExpr) {
        *t = t.substitute(&|n| self.written.get(n).cloned());
    }

    fn expr(&mut self, e: &mut Expr) {
        match &mut e.kind {
            ExprKind::Ident(n) => {
                if let Some(t) = self.written.get(n.as_str()) {
                    e.kind = name_expr(t);
                }
            }
            ExprKind::StructLit { name, type_args, .. } => {
                if let Some(t) = self.written.get(name.as_str()) {
                    if let Some(base) = t.name() {
                        *name = base;
                        *type_args = t.args().to_vec();
                    }
                }
            }
            _ => {}
        }
    }
}

/// A type as the expression naming it: `Meters`, or `Wrap<i64>`.
fn name_expr(t: &TypeExpr) -> ExprKind {
    let name = t.name().unwrap_or_else(|| t.source());
    if t.args().is_empty() {
        ExprKind::Ident(name)
    } else {
        ExprKind::GenericName { name, type_args: t.args().to_vec() }
    }
}

/// Each `comptime for` over `reflect.fields<X>()`, once per field.
struct Unroll<'a, 'b> {
    ex: &'a mut Expander<'b>,
    concrete: &'a HashMap<String, Type>,
    written: &'a HashMap<String, TypeExpr>,
}

impl Rewrite for Unroll<'_, '_> {
    fn body(&mut self, b: &mut Vec<Stmt>) {
        let mut out = Vec::with_capacity(b.len());
        for stmt in b.drain(..) {
            match self.unrolled(&stmt) {
                Some(stmts) => out.extend(stmts),
                None => out.push(stmt),
            }
        }
        *b = out;
    }
}

impl Unroll<'_, '_> {
    /// The statements a `comptime for` stands for, or `None` for any other
    /// statement.
    fn unrolled(&mut self, stmt: &Stmt) -> Option<Vec<Stmt>> {
        let StmtKind::ComptimeFor { binding, iter, body } = &stmt.kind else { return None };
        let ForBinding::Single(item) = binding else { return None };
        let ExprKind::MethodCall { object, method, type_args: Some(args), .. } = &iter.kind else {
            return None;
        };
        if !matches!(&object.kind, ExprKind::Ident(n) if n == "reflect") || !matches!(method.as_str(), "fields" | "params") {
            return None;
        }
        let ty = args.first()?;
        let checked = match rask_check::resolve_type_expr(ty, &self.ex.typed.types) {
            Ok(t) => t,
            Err(_) => {
                let _ = (self.concrete, self.written);
                self.ex.errors.push(ExpandError {
                    message: format!("reflect.fields<{}>(): not a type this instance knows", ty.source()),
                    span: iter.span,
                });
                return Some(Vec::new());
            }
        };
        let items: Vec<Item> = if method == "params" {
            match rask_check::reflect::params(&self.ex.typed.types, ty) {
                Ok(params) => params.into_iter().map(Item::Param).collect(),
                Err(message) => {
                    self.ex.errors.push(ExpandError { message, span: iter.span });
                    return Some(Vec::new());
                }
            }
        } else {
            match field_facts(self.ex.typed, self.ex.mono, &checked) {
                Some(fields) => fields.into_iter().enumerate().map(|(i, f)| Item::Field(f, i)).collect(),
                None => {
                    self.ex.errors.push(ExpandError {
                        message: format!("reflect.fields<{}>(): not a struct type", ty.source()),
                        span: iter.span,
                    });
                    return Some(Vec::new());
                }
            }
        };
        let mut out = Vec::new();
        for it in &items {
            let mut copy = body.clone();
            let mut facts = ItemFacts {
                item,
                it,
                types: &self.ex.typed.types,
                typed: self.ex.typed,
                loop_span: stmt.span,
                errors: Vec::new(),
            };
            for s in &mut copy {
                rewrite::rewrite_stmt(s, &mut facts);
            }
            self.ex.errors.extend(facts.errors);
            decide_comptime_ifs(&mut copy, self.ex.known);
            // One block per iteration, so a `let` in the body is that
            // iteration's.
            out.push(Stmt {
                id: stmt.id,
                kind: StmtKind::Expr(Expr { id: iter.id, kind: ExprKind::Block(copy), span: stmt.span }),
                span: stmt.span,
            });
        }
        Some(out)
    }
}

/// What one iteration's item is: a field, with its position, or a parameter.
enum Item {
    Field(FieldFact, usize),
    Param(rask_check::reflect::ParamFact),
}

/// The loop item's facts written in, for one iteration.
struct ItemFacts<'a> {
    item: &'a str,
    it: &'a Item,
    types: &'a rask_check::TypeTable,
    typed: &'a TypedProgram,
    /// The loop's own span, which the resolver gave its binding: a name that
    /// resolves elsewhere (an inner `for p in …`) is some other `p`.
    loop_span: Span,
    errors: Vec<ExpandError>,
}

impl ItemFacts<'_> {
    fn is_item(&self, e: &Expr) -> bool {
        if !matches!(&e.kind, ExprKind::Ident(n) if n == self.item) {
            return false;
        }
        match self.typed.resolutions.get(&e.id).and_then(|sym| self.typed.symbols.get(*sym)) {
            Some(sym) => sym.span == self.loop_span,
            None => true,
        }
    }

    /// `f.name`, `f.offset`, `p.is_mutate`, … as the literal it is here.
    fn property(&self, field: &str) -> Option<ExprKind> {
        let (f, index) = match self.it {
            Item::Field(f, index) => (f, *index),
            Item::Param(p) => {
                use rask_ast::ty::ParamMode;
                return Some(match field {
                    "index" => ExprKind::Int(p.index as i128, Some(IntSuffix::Usize)),
                    "type_name" => ExprKind::String(p.type_name.clone()),
                    "is_mutate" => ExprKind::Bool(p.mode == ParamMode::Mutate),
                    "is_take" => ExprKind::Bool(p.mode == ParamMode::Take),
                    _ => return None,
                });
            }
        };
        Some(match field {
            "name" => ExprKind::String(f.name.clone()),
            "type_name" => ExprKind::String(f.type_name.clone()),
            "serial_name" => ExprKind::String(f.serial_name.clone()),
            "offset" => ExprKind::Int(f.offset as i128, Some(IntSuffix::Usize)),
            "size" => ExprKind::Int(f.size as i128, Some(IntSuffix::Usize)),
            "is_public" => ExprKind::Bool(f.is_public),
            "is_skipped" => ExprKind::Bool(f.is_skipped),
            "has_default" => ExprKind::Bool(f.has_default),
            "index" => ExprKind::Int(index as i128, Some(IntSuffix::Usize)),
            _ => return None,
        })
    }

    /// `f.get<A>().x`: what the attachment says `x` is. A field with no `@A`
    /// has nothing to read, and only a `comptime if` drops the read for it
    /// (type.annotations/AN6).
    fn annotation_read(&mut self, call: &Expr, field: &str) -> Option<ExprKind> {
        let ExprKind::MethodCall { object, method, type_args: Some(args), .. } = &call.kind else { return None };
        if method != "get" || !self.is_item(object) {
            return None;
        }
        let annotation = args.first()?.bare_name()?;
        let Item::Field(fact, _) = self.it else { return None };
        let Some(attr) = fact.attrs.iter().find(|a| a.is(annotation)) else {
            self.errors.push(ExpandError {
                message: format!(
                    "`{}` has no `@{}` to read `{}` from — guard the read with `comptime if {}.has<{}>()`",
                    fact.name, annotation, field, self.item, annotation
                ),
                span: call.span,
            });
            return None;
        };
        let arg = attr.args.as_ref()?.iter().find(|a| a.label.as_deref() == Some(field))?;
        match &arg.value {
            AttrValue::Expr(e) => Some(e.kind.clone()),
            AttrValue::RuleName(_) => None,
        }
    }
}

impl Rewrite for ItemFacts<'_> {
    fn expr(&mut self, e: &mut Expr) {
        let replaced = match &e.kind {
            // `v.(f.name)` is `v.p`.
            ExprKind::DynamicField { object, field_expr } => match (&field_expr.kind, self.it) {
                (ExprKind::Field { object: item, field }, Item::Field(fact, _)) if self.is_item(item) && field == "name" => {
                    Some(ExprKind::Field { object: object.clone(), field: fact.name.clone() })
                }
                _ => None,
            },
            ExprKind::Field { object, field } if self.is_item(object) => self.property(field),
            ExprKind::Field { object, field } => self.annotation_read(object, field),
            ExprKind::MethodCall { object, method, type_args: Some(args), .. } if self.is_item(object) => {
                match (method.as_str(), self.it) {
                    ("has", Item::Field(fact, _)) => {
                        let annotation = args.first().and_then(TypeExpr::bare_name);
                        annotation.map(|a| ExprKind::Bool(fact.attrs.iter().any(|attr| attr.is(a))))
                    }
                    ("is", Item::Param(p)) => {
                        args.first().map(|t| ExprKind::Bool(rask_check::reflect::param_is(self.types, &p.ty, t)))
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        if let Some(kind) = replaced {
            e.kind = kind;
        }
    }
}

/// Each `comptime if` in `stmts` whose condition the written-in facts settle,
/// replaced by its chosen branch. One that reads something else (a const) is
/// left for the backends, which fold those as they always have.
fn decide_comptime_ifs(stmts: &mut [Stmt], known: &Known) {
    struct Decide<'a, 'b>(&'a Known<'b>);
    impl Rewrite for Decide<'_, '_> {
        fn body(&mut self, b: &mut Vec<Stmt>) {
            for stmt in b.iter_mut() {
                // `comptime if` as a statement, or as an expression statement.
                let body = match &stmt.kind {
                    StmtKind::Comptime(body) => body,
                    StmtKind::Expr(Expr { kind: ExprKind::Comptime { body }, .. }) => body,
                    _ => continue,
                };
                let Some((cond, then_branch, else_branch)) = rask_ast::comptime_if::parts(body) else { continue };
                let known = self.0;
                let Some(chosen) = rask_ast::comptime_if::choose(cond, then_branch, else_branch, &mut |e| known.fact(e)) else {
                    continue;
                };
                let block = Expr { id: stmt.id, kind: ExprKind::Block(chosen.to_vec()), span: stmt.span };
                stmt.kind = StmtKind::Expr(block);
            }
        }
    }
    let mut d = Decide(known);
    for s in stmts.iter_mut() {
        rewrite::rewrite_stmt(s, &mut DecideValue(known));
    }
    let mut wrapper = stmts.to_vec();
    d.body(&mut wrapper);
    for s in &mut wrapper {
        rewrite::rewrite_stmt(s, &mut d);
    }
    stmts.clone_from_slice(&wrapper);
}

/// `comptime if` used as a value (`let s = comptime if c { a } else { b }`):
/// the chosen branch, as a block giving its value.
struct DecideValue<'a, 'b>(&'a Known<'b>);

impl Rewrite for DecideValue<'_, '_> {
    fn expr(&mut self, e: &mut Expr) {
        let ExprKind::Comptime { body } = &e.kind else { return };
        let Some((cond, then_branch, else_branch)) = rask_ast::comptime_if::parts(body) else { return };
        let known = self.0;
        let Some(chosen) = rask_ast::comptime_if::choose(cond, then_branch, else_branch, &mut |x| known.fact(x)) else {
            return;
        };
        e.kind = ExprKind::Block(chosen.to_vec());
    }
}

/// Fresh ids for every node of the copy, each remembered against the one it
/// was copied from.
struct Fresh<'a> {
    next: &'a mut u32,
    copied_from: &'a mut Vec<(NodeId, NodeId)>,
}

impl Fresh<'_> {
    fn fresh(&mut self, old: NodeId) -> NodeId {
        let new = NodeId(*self.next);
        *self.next += 1;
        self.copied_from.push((old, new));
        new
    }
}

impl Rewrite for Fresh<'_> {
    fn expr(&mut self, e: &mut Expr) {
        e.id = self.fresh(e.id);
    }

    fn pattern(&mut self, p: &mut Pattern) {
        p.id = self.fresh(p.id);
    }

    fn body(&mut self, b: &mut Vec<Stmt>) {
        for stmt in b.iter_mut() {
            stmt.id = self.fresh(stmt.id);
        }
    }

    fn func(&mut self, f: &mut FnDecl) {
        f.id = self.fresh(f.id);
    }
}
