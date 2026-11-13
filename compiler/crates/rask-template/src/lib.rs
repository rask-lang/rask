// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Templates: a generic function whose body has comptime control, written out
//! as a plain function for each instance the program uses (ctrl.comptime/CT50).
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

/// One field of a struct, as `reflect.fields<T>()` reports it.
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
/// Generic counts a type parameter written without `<T>` (type.gradual/PC1).
pub fn is_template(f: &FnDecl) -> bool {
    is_generic(f) && has_comptime_for(&f.body)
}

fn is_generic(f: &FnDecl) -> bool {
    !rask_ast::type_params::signature_type_param_names(f).is_empty()
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
    /// Copies of the stdlib's templates. They resolve as the stdlib's own
    /// bodies do, where its private names and modules are in scope.
    pub library_decls: Vec<Decl>,
    pub copied_from: Vec<(NodeId, NodeId)>,
    /// Each copy's template, by its span, and the instance it was written
    /// for as a reader would write it (`zero_all<Point>`): what an error
    /// inside the copy names (CT50).
    pub instances: Vec<(Span, String)>,
    /// A function or test with comptime control and no type parameters,
    /// written out in its own place: the declaration it replaces, by id, and
    /// the replacement.
    pub replaced: Vec<(NodeId, Decl)>,
    /// Each copy's function, by id, with the template it was written out
    /// from and the instance's arguments.
    pub copies_of: Vec<(NodeId, CopyOf)>,
}

/// What a copy is: the template function's id, and the type arguments it
/// was written out for. Another function can share the template's name (a
/// method `dump` beside a free `dump`), so a caller finds the copy by these.
#[derive(Debug, Clone)]
pub struct CopyOf {
    pub template: NodeId,
    pub type_args: Vec<TypeBinding>,
}

/// The driver's state across rounds: the program as written (with bodies
/// written out in place swapped in), and every copy so far.
pub struct Rounds {
    templates: Vec<Decl>,
    copies: Vec<Decl>,
    library_copies: Vec<Decl>,
    /// Desugaring's operator calls, the copies' included.
    pub operator_calls: HashSet<NodeId>,
    done: HashSet<String>,
    next_id: u32,
    /// Each copy's template and instance, for naming it in an error.
    pub instances: Vec<(Span, String)>,
    /// Every copy's function, by id: which template, at which arguments.
    pub copies_of: HashMap<NodeId, CopyOf>,
}

impl Rounds {
    pub fn new(decls: &[Decl], operator_calls: HashSet<NodeId>) -> Self {
        Rounds {
            templates: decls.to_vec(),
            copies: Vec::new(),
            library_copies: Vec::new(),
            operator_calls,
            done: HashSet::new(),
            next_id: TEMPLATE_ID_BASE,
            instances: Vec::new(),
            copies_of: HashMap::new(),
        }
    }

    pub fn templates(&self) -> &[Decl] {
        &self.templates
    }

    /// The program for this round: what was written, then the copies.
    pub fn program(&self) -> Vec<Decl> {
        self.templates.iter().chain(&self.copies).cloned().collect()
    }

    /// Copies of stdlib templates, resolved with the stdlib's bodies and
    /// checked and compiled with the program.
    pub fn library_copies(&self) -> &[Decl] {
        &self.library_copies
    }

    /// Write out what this round's program reaches. `false` when nothing new
    /// was added, so the round's check was the last.
    pub fn expand(
        &mut self,
        library: &[Decl],
        typed: &TypedProgram,
        mono: &MonoProgram,
    ) -> Result<bool, Vec<ExpandError>> {
        let round = expand_round(&self.templates, library, typed, mono, &mut self.done, &mut self.next_id)?;
        let changed = !round.decls.is_empty() || !round.library_decls.is_empty() || !round.replaced.is_empty();
        let mut from: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for (old, new) in round.copied_from {
            from.entry(old).or_default().push(new);
        }
        let carried: Vec<NodeId> =
            self.operator_calls.iter().flat_map(|id| from.get(id).into_iter().flatten().copied()).collect();
        self.operator_calls.extend(carried);
        self.copies.extend(round.decls);
        self.library_copies.extend(round.library_decls);
        self.instances.extend(round.instances);
        self.copies_of.extend(round.copies_of);
        for (was, now) in round.replaced {
            if let Some(slot) = self.templates.iter_mut().find(|d| d.id == was) {
                *slot = now;
            }
        }
        Ok(changed)
    }
}

/// Whether a declaration has anything for this crate to write out: a
/// template, or a body written out in its own place.
pub fn has_comptime_control(d: &Decl) -> bool {
    matches!(&d.kind, DeclKind::Fn(f) if is_template(f))
        || method_sites(d).iter().any(|(t, m)| is_method_template(t, m))
        || in_place(d)
}

/// A body with comptime control the program can write out where it stands:
/// a function with no type parameters, or a test.
fn in_place(d: &Decl) -> bool {
    match &d.kind {
        DeclKind::Fn(f) => !is_generic(f) && has_comptime_for(&f.body),
        DeclKind::Test(t) => has_comptime_for(&t.body),
        _ => method_sites(d).iter().any(|(target, m)| !is_method_template(target, m) && has_comptime_for(&m.body)),
    }
}

/// The methods a declaration holds, each with the type its block is for as
/// written: `Box<T>` for `extend Box<T>`, and for a type's own methods the
/// type with its parameters.
fn method_sites(d: &Decl) -> Vec<(TypeExpr, &FnDecl)> {
    use rask_ast::type_params::{enum_type_param_names, struct_type_param_names};
    let own = |name: &str, params: Vec<String>| {
        TypeExpr::generic(name.to_string(), params.into_iter().map(TypeExpr::named).collect())
    };
    match &d.kind {
        DeclKind::Impl(i) => i.methods.iter().map(|m| (i.target_ty.clone(), m)).collect(),
        DeclKind::Struct(st) => st.methods.iter().map(|m| (own(&st.name, struct_type_param_names(st)), m)).collect(),
        DeclKind::Enum(e) => e.methods.iter().map(|m| (own(&e.name, enum_type_param_names(e)), m)).collect(),
        _ => Vec::new(),
    }
}

/// A method with comptime control that is written out per instance: it, or
/// the type its block is for, has type parameters.
fn is_method_template(target: &TypeExpr, m: &FnDecl) -> bool {
    (is_generic(m) || !target.args().is_empty()) && has_comptime_for(&m.body)
}

fn is_struct(types: &rask_check::TypeTable, ty: &Type) -> bool {
    match ty {
        Type::Named(id) | Type::Generic { base: id, .. } => matches!(types.get(*id), Some(TypeDef::Struct { .. })),
        _ => false,
    }
}

/// A copy's one function: the function itself, or the method its block holds.
fn copy_fn_id(copy: &Decl) -> Option<NodeId> {
    match &copy.kind {
        DeclKind::Fn(f) => Some(f.id),
        DeclKind::Impl(i) => i.methods.first().map(|m| m.id),
        _ => None,
    }
}

/// Where a template is: a free function, or a method with the type its block
/// is for.
enum Site<'a> {
    Free(&'a Decl),
    Method { target: TypeExpr, method: &'a FnDecl, block: &'a Decl },
}

/// Write out each reachable template instance not written out yet.
///
/// `templates` are the declarations as desugaring left them, before checking
/// changed them; `done` names the copies already made, by the instance's
/// mangled name, which is also the copy's.
/// `library` is the stdlib's declarations with their checked ids: its
/// generic templates are written out for the program's instances too, and
/// nothing of it is written out in place.
pub fn expand_round(
    templates: &[Decl],
    library: &[Decl],
    typed: &TypedProgram,
    mono: &MonoProgram,
    done: &mut HashSet<String>,
    next_id: &mut u32,
) -> Result<Expansion, Vec<ExpandError>> {
    // By the function's own id: a method sharing a template's name (`show`
    // on a generic type, `map` on a range) is some other function.
    let mut by_id: HashMap<NodeId, (Site, bool)> = HashMap::new();
    let program = templates.iter().map(|d| (d, false));
    for (d, from_library) in program.chain(library.iter().map(|d| (d, true))) {
        match &d.kind {
            DeclKind::Fn(f) if is_template(f) => {
                by_id.insert(f.id, (Site::Free(d), from_library));
            }
            _ => {
                for (target, method) in method_sites(d) {
                    if is_method_template(&target, method) {
                        by_id.insert(method.id, (Site::Method { target, method, block: d }, from_library));
                    }
                }
            }
        }
    }
    let mut out = Expansion::default();
    let mut errors = Vec::new();
    let known = Known::new(templates, typed);
    for inst in &mono.functions {
        if inst.type_args.is_empty() || done.contains(&inst.name) {
            continue;
        }
        let DeclKind::Fn(generic) = &inst.body.kind else { continue };
        let Some((site, from_library)) = by_id.get(&generic.id) else { continue };
        let template_span = match site {
            Site::Free(d) => d.span,
            Site::Method { method, .. } => method.span,
        };
        // An argument not settled yet (`count<_>`, reached through a field
        // the first round couldn't type) waits for the round whose copy
        // names it.
        if inst.type_args.iter().any(|b| b.ty.has_unsolved_var() || b.ty.contains(&|t| matches!(t, Type::Error))) {
            continue;
        }
        done.insert(inst.name.clone());
        let label = |b: &TypeBinding| rask_check::reflect::type_label(&typed.types, &b.ty);
        let with_args = |name: &str, args: Vec<String>| {
            if args.is_empty() { name.to_string() } else { format!("{name}<{}>", args.join(", ")) }
        };
        // `show<Point>`; a method as `Box<Point>.names` or `Holder.dump<Point>`.
        let shown = match site {
            Site::Free(_) => with_args(&generic.name, inst.type_args.iter().map(|b| label(b).source()).collect()),
            Site::Method { target, method, .. } => {
                let receivers: Vec<String> =
                    target.args().iter().filter_map(|a| a.bare_name().map(str::to_string)).collect();
                let own = |b: &&TypeBinding| !receivers.contains(&b.param);
                let receiver = target.substitute(&|n| {
                    inst.type_args.iter().find(|b| b.param == n && !own(b)).map(label)
                });
                let args = inst.type_args.iter().filter(own).map(|b| label(b).source()).collect();
                format!("{receiver}.{}", with_args(&method.name, args))
            }
        };
        out.instances.push((template_span, shown));
        let mut ex = Expander { typed, mono, next_id, copied_from: Vec::new(), errors: Vec::new(), known: &known, unrolled: 0 };
        let copy = match site {
            Site::Free(template) => ex.expand(template, &inst.name, &inst.type_args),
            Site::Method { target, method, block } => ex.expand_method(target, method, block, &inst.name, &inst.type_args),
        };
        let shown_instance = out.instances.last().map(|(_, n)| n.clone()).unwrap_or_default();
        errors.extend(ex.errors.into_iter().map(|mut e| {
            e.message = format!("{} (writing out `{}`)", e.message, shown_instance);
            e
        }));
        out.copied_from.extend(ex.copied_from);
        if let Some(copy_fn) = copy_fn_id(&copy) {
            out.copies_of.push((copy_fn, CopyOf { template: generic.id, type_args: inst.type_args.clone() }));
        }
        if *from_library {
            out.library_decls.push(copy);
        } else {
            out.decls.push(copy);
        }
    }
    // Nothing to instantiate: the loop's type is already known. One that
    // can't be written out (a layout mono didn't make, because nothing
    // reaches the code) stays as written; nothing runs it.
    for decl in templates.iter().filter(|d| in_place(d)) {
        let mut ex = Expander { typed, mono, next_id, copied_from: Vec::new(), errors: Vec::new(), known: &known, unrolled: 0 };
        let copy = ex.expand_in_place(decl);
        if ex.unrolled == 0 && ex.errors.is_empty() {
            continue;
        }
        if ex.errors.is_empty() {
            out.copied_from.extend(ex.copied_from);
            out.replaced.push((decl.id, copy));
        } else {
            errors.extend(ex.errors);
        }
    }
    if errors.is_empty() {
        Ok(out)
    } else {
        // One iteration per item, so one mistake in the body is found once
        // per item.
        let mut seen = HashSet::new();
        errors.retain(|e| seen.insert((e.message.clone(), e.span)));
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
    /// Loops written out: none means a body written out in place came back
    /// as it was, every loop left for code nothing reaches.
    unrolled: usize,
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
            Some(DeclKind::Struct(s)) => rask_ast::type_params::struct_type_param_names(s),
            Some(DeclKind::Enum(e)) => rask_ast::type_params::enum_type_param_names(e),
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
        let concrete = HashMap::new();
        // A block's plain methods only: a template beside them keeps its
        // body and its id, which its copies are matched by.
        let bodies: Vec<&mut Vec<Stmt>> = match &mut decl.kind {
            DeclKind::Fn(f) => vec![&mut f.body],
            DeclKind::Test(t) => vec![&mut t.body],
            DeclKind::Impl(i) => {
                let target = i.target_ty.clone();
                i.methods.iter_mut().filter(|m| !is_method_template(&target, m)).map(|m| &mut m.body).collect()
            }
            DeclKind::Struct(st) => {
                let target = TypeExpr::generic(
                    st.name.clone(),
                    rask_ast::type_params::struct_type_param_names(st).into_iter().map(TypeExpr::named).collect(),
                );
                st.methods.iter_mut().filter(|m| !is_method_template(&target, m)).map(|m| &mut m.body).collect()
            }
            DeclKind::Enum(e) => {
                let target = TypeExpr::generic(
                    e.name.clone(),
                    rask_ast::type_params::enum_type_param_names(e).into_iter().map(TypeExpr::named).collect(),
                );
                e.methods.iter_mut().filter(|m| !is_method_template(&target, m)).map(|m| &mut m.body).collect()
            }
            _ => Vec::new(),
        };
        for body in bodies {
            rewrite::rewrite_body(body, &mut Unroll { ex: self, concrete: &concrete, lenient: true });
            rewrite::rewrite_body(body, &mut Fresh { next: self.next_id, copied_from: &mut self.copied_from });
        }
        decl
    }

    fn written(&self, type_args: &[TypeBinding]) -> HashMap<String, TypeExpr> {
        let types = &self.typed.types;
        type_args
            .iter()
            .map(|b| {
                let named = rask_mono::Monomorphizer::nameable_type(&b.ty, types).unwrap_or_else(|| b.ty.clone());
                (b.param.clone(), named.to_type_expr())
            })
            .collect()
    }

    /// A method's copy: a block of its own for the instance's receiver,
    /// holding the one method, named so its symbol is the instance's
    /// (`Box_names$Pair` → `names$Pair` in `extend Box<Pair>`).
    fn expand_method(
        &mut self,
        target: &TypeExpr,
        method: &FnDecl,
        block: &Decl,
        name: &str,
        type_args: &[TypeBinding],
    ) -> Decl {
        let written = self.written(type_args);
        let concrete: HashMap<String, Type> = type_args.iter().map(|b| (b.param.clone(), b.ty.clone())).collect();
        let base = target.last_segment().unwrap_or_default();
        let method_name = match name.strip_prefix(&format!("{base}_")) {
            Some(rest) => rest.to_string(),
            None => {
                self.errors.push(ExpandError {
                    message: format!("`{name}` isn't named for a method of `{base}`"),
                    span: method.span,
                });
                name.to_string()
            }
        };
        let mut copy = method.clone();
        copy.name = method_name;
        copy.type_params.clear();
        let target = target.substitute(&|n| written.get(n).cloned());
        let mut decl = Decl {
            id: block.id,
            span: block.span,
            kind: DeclKind::Impl(rask_ast::decl::ImplDecl {
                interface: None,
                target_ty: target,
                methods: vec![copy],
                is_unsafe: false,
                is_pub: false,
                where_bounds: Vec::new(),
                assoc_bindings: Vec::new(),
                doc: None,
            }),
        };
        rewrite::rewrite_decl(&mut decl, &mut TypeParams { written: &written });
        rewrite::rewrite_decl(&mut decl, &mut Unroll { ex: self, concrete: &concrete, lenient: false });
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
        rewrite::rewrite_decl(&mut decl, &mut TypeParams { written: &written });
        rewrite::rewrite_decl(&mut decl, &mut Unroll { ex: self, concrete: &concrete, lenient: false });
        let mut fresh = Fresh { next: self.next_id, copied_from: &mut self.copied_from };
        rewrite::rewrite_decl(&mut decl, &mut fresh);
        decl.id = fresh.fresh(decl.id);
        decl
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
    /// Writing a declaration out in place: a loop over a type it can't know
    /// (a generic method's `T` beside the plain one being written out) stays
    /// as written rather than failing the declaration.
    lenient: bool,
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
        let items = match &iter.kind {
            ExprKind::Array(elems) => {
                // Each entry is written in where the item is named, so it has
                // to be known while compiling, and naming it twice can't run
                // anything twice (ctrl.comptime/CT51).
                if let Some(e) = elems.iter().find(|e| !is_literal(e)) {
                    self.ex.errors.push(ExpandError {
                        message: "an array `comptime for` runs over is read while compiling, so each entry has to be a literal".to_string(),
                        span: e.span,
                    });
                    return Some(Vec::new());
                }
                elems.iter().cloned().map(Item::Value).collect()
            }
            ExprKind::MethodCall { object, method, type_args: Some(args), .. }
                if matches!(&object.kind, ExprKind::Ident(n) if n == "reflect")
                    && matches!(method.as_str(), "fields" | "params" | "variants") =>
            {
                let ty = args.first()?;
                // What the checker took the argument to be: the name alone
                // can find another type (the stdlib's `Color`).
                let checked = self
                    .ex
                    .typed
                    .call_type_args
                    .get(&iter.id)
                    .and_then(|b| b.first())
                    .map(|b| rask_check::substitute_type(&b.ty, self.concrete));
                match self.reflected(method, ty, checked)? {
                    Ok(items) => items,
                    Err(message) => {
                        self.ex.errors.push(ExpandError { message, span: iter.span });
                        return Some(Vec::new());
                    }
                }
            }
            _ => {
                self.ex.errors.push(ExpandError {
                    message: "`comptime for` runs over `reflect.fields<T>()`, `reflect.params<F>()`, \
                              `reflect.variants<T>()` or an array literal"
                        .to_string(),
                    span: iter.span,
                });
                return Some(Vec::new());
            }
        };
        let out = self.write_out(item, &items, body, stmt, iter.id);
        self.ex.unrolled += 1;
        Some(out)
    }

    /// The items `reflect.<method><ty>()` gives. `None` when the loop stays
    /// as written: a type this body can't know yet, or a struct nothing
    /// reaches.
    fn reflected(&mut self, method: &str, ty: &TypeExpr, checked: Option<Type>) -> Option<Result<Vec<Item>, String>> {
        let resolved = match checked {
            Some(t) if !t.has_unsolved_var() && !t.contains(&|t| matches!(t, Type::Error | Type::UnresolvedNamed(_))) => Ok(t),
            _ => rask_check::resolve_type_expr(ty, &self.ex.typed.types),
        };
        let checked = match resolved {
            Ok(t) => t,
            Err(_) if self.lenient => return None,
            Err(_) => return Some(Err(format!("reflect.{method}<{}>(): not a type this instance knows", ty.source()))),
        };
        let types = &self.ex.typed.types;
        Some(match method {
            "params" => rask_check::reflect::params(types, ty).map(|ps| ps.into_iter().map(Item::Param).collect()),
            "variants" => variant_facts(types, &checked)
                .map(|vs| vs.into_iter().map(Item::Variant).collect())
                .ok_or_else(|| format!("reflect.variants<{}>(): not an enum type", ty.source())),
            _ => match field_facts(self.ex.typed, self.ex.mono, &checked) {
                Some(fields) => Ok(fields.into_iter().enumerate().map(|(i, f)| Item::Field(f, i)).collect()),
                // A struct with no layout: nothing the program runs reaches
                // this code, so it stays as written.
                None if self.lenient && is_struct(types, &checked) => return None,
                None => Err(format!("reflect.fields<{}>(): not a struct type", ty.source())),
            },
        })
    }

    /// One block per item, the body with the item's facts written in and
    /// its `comptime if`s decided.
    fn write_out(&mut self, item: &str, items: &[Item], body: &[Stmt], stmt: &Stmt, iter_id: NodeId) -> Vec<Stmt> {
        let mut out = Vec::new();
        for it in items {
            let mut copy = body.to_vec();
            let mut facts = ItemFacts {
                item,
                it,
                types: &self.ex.typed.types,
                typed: self.ex.typed,
                loop_span: stmt.span,
                report: false,
                errors: Vec::new(),
            };
            for s in &mut copy {
                rewrite::rewrite_stmt(s, &mut facts);
            }
            decide_comptime_ifs(&mut copy, self.ex.known);
            // `comptime for p in v.fields`: a variant's payload, written out
            // with the variant.
            if let Item::Variant(v) = it {
                let payload: Vec<Item> = v.fields.iter().cloned().enumerate().map(|(i, f)| Item::Field(payload_fact(f), i)).collect();
                rewrite::rewrite_body(&mut copy, &mut PayloadLoops { unroll: self, variant: item, payload: &payload });
            }
            // What's left naming the item is an error: a read of an
            // annotation the field lacks that no `comptime if` dropped, or
            // the item used as a value.
            let mut facts = ItemFacts {
                item,
                it,
                types: &self.ex.typed.types,
                typed: self.ex.typed,
                loop_span: stmt.span,
                report: true,
                errors: Vec::new(),
            };
            for s in &mut copy {
                rewrite::rewrite_stmt(s, &mut facts);
            }
            self.ex.errors.extend(facts.errors);
            // One block per iteration, so a `let` in the body is that
            // iteration's.
            out.push(Stmt {
                id: stmt.id,
                kind: StmtKind::Expr(Expr { id: iter_id, kind: ExprKind::Block(copy), span: stmt.span }),
                span: stmt.span,
            });
        }
        out
    }
}

/// A number, string, character or bool written out, a negative number
/// included.
fn is_literal(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Int(..) | ExprKind::Float(..) | ExprKind::String(_) | ExprKind::Char(_) | ExprKind::Bool(_) => true,
        ExprKind::Unary { op: rask_ast::expr::UnaryOp::Neg, operand } => {
            matches!(operand.kind, ExprKind::Int(..) | ExprKind::Float(..))
        }
        _ => false,
    }
}

/// `comptime for p in v.fields` inside one variant's iteration.
struct PayloadLoops<'a, 'b, 'c> {
    unroll: &'a mut Unroll<'b, 'c>,
    variant: &'a str,
    payload: &'a [Item],
}

impl Rewrite for PayloadLoops<'_, '_, '_> {
    fn body(&mut self, b: &mut Vec<Stmt>) {
        let mut out = Vec::with_capacity(b.len());
        for stmt in b.drain(..) {
            let StmtKind::ComptimeFor { binding: ForBinding::Single(item), iter, body } = &stmt.kind else {
                out.push(stmt);
                continue;
            };
            let over_payload = matches!(&iter.kind, ExprKind::Field { object, field }
                if field == "fields" && matches!(&object.kind, ExprKind::Ident(n) if n == self.variant));
            if !over_payload {
                out.push(stmt);
                continue;
            }
            let written = self.unroll.write_out(item, self.payload, body, &stmt, iter.id);
            out.extend(written);
        }
        *b = out;
    }
}

/// A payload field as a field of the variant: it has no visibility or wire
/// annotations of its own.
fn payload_fact(p: rask_mono::reflect::PayloadField) -> FieldFact {
    FieldFact {
        serial_name: p.name.clone(),
        name: p.name,
        type_name: p.type_name,
        offset: p.offset,
        size: p.size,
        is_public: true,
        is_skipped: false,
        has_default: p.has_default,
        attrs: Vec::new(),
    }
}

/// An enum's variants, from the layout the checker's table gives it.
fn variant_facts(types: &rask_check::TypeTable, ty: &Type) -> Option<Vec<rask_mono::reflect::VariantFact>> {
    let (id, args): (_, Vec<Type>) = match ty {
        Type::Named(id) => (*id, Vec::new()),
        Type::Generic { base, args } => (
            *base,
            args.iter()
                .filter_map(|a| match a {
                    rask_types::GenericArg::Type(t) => Some((**t).clone()),
                    _ => None,
                })
                .collect(),
        ),
        _ => return None,
    };
    let layout = types.enum_layout(id, &args)?;
    let subst: Vec<(String, TypeExpr)> = match types.get(id) {
        Some(TypeDef::Enum { type_params, .. }) if type_params.len() == args.len() => type_params
            .iter()
            .cloned()
            .zip(args.iter().map(|t| rask_check::reflect::type_label(types, t)))
            .collect(),
        _ => Vec::new(),
    };
    Some(rask_mono::reflect::variant_facts(&layout, types, &subst))
}

/// What one iteration's item is: a field, with its position, or a parameter.
enum Item {
    Field(FieldFact, usize),
    Param(rask_check::reflect::ParamFact),
    Variant(rask_mono::reflect::VariantFact),
    /// An array literal's entry, standing in for the item where it's named.
    Value(Expr),
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
    /// Report reads of an annotation the field doesn't carry; off until the
    /// `comptime if`s are decided.
    report: bool,
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
            Item::Variant(v) => {
                return Some(match field {
                    "name" => ExprKind::String(v.name.clone()),
                    "serial_name" => ExprKind::String(v.serial_name.clone()),
                    "has_fields" => ExprKind::Bool(!v.fields.is_empty()),
                    "field_count" => ExprKind::Int(v.fields.len() as i128, None),
                    _ => return None,
                });
            }
            Item::Value(_) => return None,
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
            if !self.report {
                return None;
            }
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
            // An array's entry, where the loop names its item.
            ExprKind::Ident(_) if matches!(self.it, Item::Value(_)) && self.is_item(e) => match self.it {
                Item::Value(v) => Some(v.kind.clone()),
                _ => None,
            },
            // `v.(f.name)` is `v.p`.
            ExprKind::DynamicField { object, field_expr } => match (&field_expr.kind, self.it) {
                (ExprKind::Field { object: item, field }, Item::Field(fact, _)) if self.is_item(item) && field == "name" => {
                    Some(ExprKind::Field { object: object.clone(), field: fact.name.clone() })
                }
                _ => None,
            },
            ExprKind::Field { object, field } if self.is_item(object) => self.property(field),
            // The item alone, as a value: there isn't one at run time.
            ExprKind::Ident(_) if self.report && !matches!(self.it, Item::Value(_)) && self.is_item(e) => {
                self.errors.push(ExpandError {
                    message: format!(
                        "`{}` exists only while compiling; read one of its properties (`{}.name`, …) instead",
                        self.item, self.item
                    ),
                    span: e.span,
                });
                None
            }
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
/// replaced by its chosen branch. One it can't settle is left for the
/// backends, which fold a `comptime if` anywhere.
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
