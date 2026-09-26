// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Ownership and borrowing analysis for the Rask language.
//!
//! This crate verifies memory safety by tracking:
//! - Move semantics: detecting use-after-move
//! - Borrow scopes: persistent (block) vs instant (semicolon)
//! - Aliasing rules: shared XOR exclusive access

mod state;
mod error;

pub use state::{BindingState, BorrowMode, BorrowScope, ActiveBorrow};
pub use error::{
    AccessKind, LinearDiscardPosition, LinkEscape, MoveReason, OwnershipError, OwnershipErrorKind,
};

use std::collections::{HashMap, HashSet};

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::expr::{ArgMode, Expr, ExprKind, Pattern, UnaryOp};
use rask_ast::stmt::{ForBinding, Stmt, StmtKind};
use rask_ast::Span;
use rask_types::{ParamMode, Type, TypedProgram};

/// Result of ownership analysis.
#[derive(Debug)]
pub struct OwnershipResult {
    /// Any errors found during analysis.
    pub errors: Vec<OwnershipError>,
    /// CM1: closure literals that outlive the frame that built them. Lowering
    /// and the interpreter read this to decide whether a capture is the value
    /// or a pointer to it — it is the whole of what `own` used to say.
    pub escaping_closures: HashSet<rask_ast::NodeId>,
}

impl OwnershipResult {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// W2 tracking: active `with` block binding info.
#[derive(Debug, Clone)]
struct WithBindingInfo {
    /// Collection variable name (e.g. "rows" from `with rows[i] as r`)
    collection_name: String,
    /// Span of the `with` binding for error messages
    span: Span,
}

/// LP14/LP16: Tracks the collection being iterated in a `for mutate` loop.
#[derive(Debug, Clone)]
struct ForMutateInfo {
    /// Collection variable name (e.g. "items" from `for mutate item in items`)
    collection_name: String,
    /// Binding variable names (e.g. ["item"])
    binding_names: Vec<String>,
    /// Span for error messages
    span: Span,
}

/// Ownership and borrow checker.
pub struct OwnershipChecker<'a> {
    /// The typed program from type checking.
    program: &'a TypedProgram,
    /// State of each binding (owned, moved, borrowed).
    /// Key is the binding name (since we don't have SymbolId in scope).
    bindings: HashMap<String, BindingState>,
    /// Currently active borrows.
    borrows: Vec<ActiveBorrow>,
    /// Current block ID for tracking borrow scopes.
    current_block: u32,
    /// Current statement ID for instant borrows.
    current_stmt: u32,
    /// Type of each binding, for generating move-reason diagnostics.
    binding_types: HashMap<String, Type>,
    /// Bindings that are @resource types (must be consumed).
    resource_bindings: HashSet<String>,
    /// Where each resource binding was acquired, so a diagnostic about a later
    /// statement can point back at it.
    resource_acquired_at: HashMap<String, Span>,
    /// Resources already owed on entry to each enclosing loop body, so a
    /// `break` or `continue` can tell the ones it is walking out on from the
    /// ones the code after the loop still consumes.
    loop_entry_resources: Vec<HashSet<String>>,
    /// For a binding that isn't a resource itself but *holds* one, the field
    /// paths that still owe consumption.
    ///
    /// `struct Pair { a: Conn, b: Conn }` gives `p` two debts, and `p.a.close()`
    /// pays one of them. Tracking the obligation on the binding alone can only be
    /// all-or-nothing: either a projection discharges the whole holder — so
    /// `p.b` leaks silently — or it discharges nothing, and `w.conn.close()` on a
    /// wrapper with one resource can't be written at all, because there is no
    /// `w.close()` to call (#828).
    resource_field_debts: HashMap<String, Vec<Vec<String>>>,
    /// Of those, the ones `Heap(…)` allocated. They follow the same rules — L1–L6 are
    /// shared between `@resource` and `Owned<T>` — but the fix is `drop(name)`,
    /// so the diagnostic differs (#819).
    owned_bindings: HashSet<String>,
    /// Locals bound to a value a container lent out: name → what lent it.
    /// `let v = m.get(k) ?? Vec.new()` is fine where it stands — the borrow
    /// lives in the block — and wrong the moment it is returned (#1206).
    lent_locals: HashMap<String, LentValue>,
    /// Parameters the caller only lent out: name → (declaration span, `mutate`).
    /// A borrow can't be given away, so consuming one is an error rather than a
    /// move (#804).
    borrowed_params: HashMap<String, (Span, bool)>,
    /// Linear values a non-`own` closure captured: name → where the closure is.
    /// `mem.closures`' edge-case table says a non-`own` closure *borrows* a
    /// resource, and L3 says a borrow isn't a consumption — so a `close()` in
    /// the body is the #804 error one door along. Only live while the body is
    /// being walked.
    borrowed_captures: HashMap<String, Span>,
    /// `mutate` parameters: name → declaration span. Consuming one is allowed —
    /// that's what exclusive access is for — but the value has to be back before
    /// the function returns (#815).
    mutate_params: HashMap<String, Span>,
    /// Resource bindings registered with `ensure` (consumption committed).
    ensure_registered: HashSet<String>,
    /// Span of the `ensure` statement that registered each resource (C4 diagnostics).
    ensure_spans: HashMap<String, Span>,
    /// True when inside an `ensure` body (defer moves).
    in_ensure: bool,
    /// Active `with` block bindings for W2 checking.
    active_with_bindings: Vec<WithBindingInfo>,
    /// LP14/LP16: Active `for mutate` loops for structural mutation checking.
    active_for_mutates: Vec<ForMutateInfo>,
    /// Link locals whose initializer was a `rack.insert(...)` — they name a node
    /// nothing else in this body has a name for, so a delete elsewhere can't be
    /// deleting them. Every other link local is *derived* (a field read, an
    /// iteration binding, a call result) and may alias whatever a delete names.
    /// `take` parameters whose type is a link: the caller consumed the name at the
    /// call site, so deleting one here is already visible to them.
    /// Bindings whose resource the field walk could not name, and the shape that
    /// stopped it. These owe the whole binding; the reason rides along so the
    /// error can say why it named the root.
    coarse_resources: HashMap<String, String>,
    /// Parameters declared `deleting`: the caller was told this call may delete
    /// nodes it never named, so an unnamed delete through them is allowed.
    /// Names already reported by an exit check, so a body with several returns
    /// doesn't repeat itself.
    exit_reported: HashSet<String>,
    deleting_params: HashSet<String>,
    /// Which rack a link local came out of, by root name. A link's *type* can't
    /// say — two `Rack<Node>` parameters give links of the same type — so the
    /// origin is carried from wherever the link was derived.
    link_rack_root: HashMap<String, String>,
    /// A container that has had a link put into it, and which rack that link
    /// came from. E0379 walks *expressions* to find a link leaving, and a
    /// container is neither a link nor built where the link went in — `v.push(n)`
    /// several statements before `return v` (#941). The rack rides on the
    /// container name instead, so the same escape test covers it.
    container_link_rack: HashMap<String, String>,
    /// Rack bindings this body may write nodes through: `mut` locals, and
    /// `mutate`/`deleting` parameters. A link is an access path into a rack, not
    /// a permission of its own, so this is what a node write is checked against.
    writable_racks: HashSet<String>,
    /// Link parameters, by name. A link handed in this way has no rack in scope,
    /// so its writability was settled at the call site instead.
    link_params: HashSet<String>,
    /// Links this body may write nodes through: a `mutate`/`deleting` link
    /// parameter, and anything reached from one by following edges.
    ///
    /// Permission propagates *outward* along edges, which is the direction that
    /// makes sense: an edge only connects co-owned nodes, so if you may write
    /// this node you may write the ones it points at. The read-only case needs no
    /// propagation at all — no permission is the default, so a link derived from
    /// a plain borrow inherits nothing and stays a view.
    writable_links: HashSet<String>,
    take_link_params: HashSet<String>,
    identified_links: HashSet<String>,
    /// Spans where a link was consumed by `rack.delete(...)`, so a use-after-move
    /// on a link can tell "the node was deleted here" from "you moved it here".
    link_delete_spans: std::collections::HashSet<Span>,
    /// Loops iterating a `Rack`'s own nodes: (element type key, binding names).
    /// A `delete` of one of those bindings is picking an arbitrary node rather
    /// than a node the caller named, so it invalidates every other link local.
    rack_iterations: Vec<(Option<String>, Vec<String>)>,
    /// Parameter type strings: param name → type annotation (e.g. "Vec<Entity>").
    param_type_strings: HashMap<String, String>,
    /// SL1: Bindings created by `const` from non-copy expressions (block-scoped borrows).
    /// Maps binding name → block_id where the borrow was created.
    borrow_bindings: HashMap<String, u32>,
    /// Block where each binding was declared (first introduced via let/const).
    binding_decl_blocks: HashMap<String, u32>,
    /// SL1: Bindings that hold scope-limited closures.
    /// Maps binding name → (borrow_block, binding_block).
    /// borrow_block: the block where the captured borrow lives.
    /// binding_block: the block where the closure binding was declared.
    /// Escape: binding_block < borrow_block (binding outlives borrow).
    scope_limited_closures: HashMap<String, (u32, u32)>,
    /// O11: module-level const names. A const is never given away, so a
    /// consumption of one is an error rather than a move.
    module_consts: std::collections::HashSet<String>,
    /// SL1: each non-`own` closure expression's scope limit, keyed by the
    /// closure's own node.
    ///
    /// This was one `Option<u32>`, published before the body was walked with
    /// "record scope limit for the next binding to pick up". The next binding
    /// was often *inside* the body: the closure's own first `let` took the
    /// flag, so the closure itself came back unlimited and an innocent local
    /// got the limit instead. `mut sum = extra` inside a closure that captured
    /// a Vec was enough to reject the whole thing (#869). Keyed by node, a
    /// binding asks about its own initializer and the body can't answer for it.
    closure_scope_limits: HashMap<rask_ast::NodeId, u32>,
    /// Closure literals bound to a name, so `spawn(f)` can be checked the same
    /// way `spawn(|| …)` is. Only the literal case is in here — a closure that
    /// arrives through a parameter or a call has no body to read.
    closure_literals: HashMap<String, Expr>,
    /// CM1: closure literals that outlive the frame that built them, so they
    /// carry their captures instead of pointing at them. Collected before any
    /// body is walked — see `collect_escaping_closures`.
    escaping_closures: HashSet<rask_ast::NodeId>,
    /// Closure literals whose body assigns to something they captured. MC4
    /// promises the caller sees those writes, which only a borrow capture can
    /// deliver, so these are never swept up by the call-result rule in
    /// `closure_ids_of`.
    closure_writes_a_capture: HashSet<rask_ast::NodeId>,
    /// `RASK_ESCAPE_AUDIT=1` reports where the inferred answer and the written
    /// `own` disagree. Read once — this sits on the walk of every closure.
    escape_audit: bool,
    /// Free-function parameter modes by name → per-position `take` flags.
    ///
    /// Lets a call consume arguments to `take` params without call-site `own`
    /// (#296). Every function is in here, including those with no `take` at
    /// all: SL4 needs to know a parameter positively *is* a borrow, and an
    /// absent entry means "no signature in reach", which stays conservative.
    fn_take_params: HashMap<String, Vec<bool>>,
    /// Per function name, which parameters carry `deleting`.
    fn_deleting_params: HashMap<String, Vec<bool>>,
    /// (type, method) -> (is the receiver `deleting`, which parameters are). Built
    /// here rather than carried on `MethodSig`, which has 46 construction sites.
    method_deleting: HashMap<(String, String), (bool, Vec<bool>)>,
    /// MC2: closure bindings that hold a mutable capture, and what they hold.
    /// `(binding, variables, where the closure was written, last statement that
    /// mentions the binding)` — the record dies at that statement, so reading
    /// the variable after the closure's last call is fine.
    mutable_captures: Vec<MutableCapture>,
    /// Errors accumulated during analysis.
    errors: Vec<OwnershipError>,
}

/// A value a container lent out, carried from wherever the walk found it to
/// the `return` that hands it on.
#[derive(Debug, Clone)]
struct LentValue {
    /// `index.get(…)`, for the message.
    call: String,
    /// The container it came out of.
    holder: String,
    /// `Vec` or `Map` — which one, for naming the copying spelling.
    lender: String,
    payload_ty: String,
    /// The method that hands back a copy, when the container has one.
    clone_form: Option<String>,
    /// The lookup itself, which may be several lines from the `return`.
    span: Span,
}

/// One live mutable capture: a closure binding and the variables its body writes.
#[derive(Debug, Clone)]
struct MutableCapture {
    /// The name the closure is bound to.
    holder: String,
    /// Variables from the enclosing scope the body assigns to.
    vars: Vec<String>,
    /// Where the closure literal is.
    span: Span,
    /// Index of the last statement in the holder's block that mentions it.
    dies_after: usize,
    /// Which block's statement list `dies_after` counts in.
    block: u32,
}

impl<'a> OwnershipChecker<'a> {
    pub fn new(program: &'a TypedProgram) -> Self {
        Self {
            program,
            bindings: HashMap::new(),
            binding_types: HashMap::new(),
            borrows: Vec::new(),
            current_block: 0,
            current_stmt: 0,
            resource_bindings: HashSet::new(),
            resource_acquired_at: HashMap::new(),
            loop_entry_resources: Vec::new(),
            resource_field_debts: HashMap::new(),
            owned_bindings: HashSet::new(),
            lent_locals: HashMap::new(),
            borrowed_params: HashMap::new(),
            borrowed_captures: HashMap::new(),
            mutate_params: HashMap::new(),
            ensure_registered: HashSet::new(),
            ensure_spans: HashMap::new(),
            in_ensure: false,
            active_with_bindings: Vec::new(),
            active_for_mutates: Vec::new(),
            coarse_resources: HashMap::new(),
            exit_reported: HashSet::new(),
            deleting_params: HashSet::new(),
            link_rack_root: HashMap::new(),
            container_link_rack: HashMap::new(),
            writable_racks: HashSet::new(),
            link_params: HashSet::new(),
            writable_links: HashSet::new(),
            take_link_params: HashSet::new(),
            identified_links: HashSet::new(),
            link_delete_spans: std::collections::HashSet::new(),
            rack_iterations: Vec::new(),
            param_type_strings: HashMap::new(),
            borrow_bindings: HashMap::new(),
            binding_decl_blocks: HashMap::new(),
            scope_limited_closures: HashMap::new(),
            module_consts: std::collections::HashSet::new(),
            closure_scope_limits: HashMap::new(),
            closure_literals: HashMap::new(),
            escaping_closures: HashSet::new(),
            closure_writes_a_capture: HashSet::new(),
            escape_audit: std::env::var("RASK_ESCAPE_AUDIT").is_ok(),
            mutable_captures: Vec::new(),
            fn_take_params: HashMap::new(),
            fn_deleting_params: HashMap::new(),
            method_deleting: HashMap::new(),
            errors: Vec::new(),
        }
    }

    /// Run ownership analysis on all declarations.
    pub fn check(self, decls: &[Decl]) -> OwnershipResult {
        return self.check_with_signatures(decls, &[]);
    }

    /// Run ownership analysis, reading parameter modes from `extra` as well.
    ///
    /// `extra` is the stdlib. Its bodies are not walked — only its signatures
    /// are read, so a call to `spawn` can see that it takes its closure. The
    /// ownership checker had never been handed them: `stdlib_decls` was built
    /// for the type checker and stopped there, so PM3 had never fired for a
    /// stdlib function called by name, and `mem.closures/SL4` had to guess a
    /// mode it could have read. A user function of the same name still wins —
    /// the program's own declarations are collected second.
    pub fn check_with_signatures(mut self, decls: &[Decl], extra: &[Decl]) -> OwnershipResult {
        self.collect_signatures(extra);
        self.collect_signatures(decls);
        return self.run(decls);
    }

    /// Parameter modes, per function and per method. No bodies.
    fn collect_signatures(&mut self, decls: &[Decl]) {
        // Collect `take`-parameter positions for every free function so calls
        // can consume the matching argument (PM3) without call-site `own` (#296).
        for decl in decls {
            // Methods too: `sc.purge()` has to revoke the caller's links when
            // `purge` is declared `deleting self`, and the receiver is where that
            // declaration sits.
            if let DeclKind::Impl(impl_decl) = &decl.kind {
                let ty = impl_decl
                    .target_ty
                    .split('<')
                    .next()
                    .unwrap_or(&impl_decl.target_ty)
                    .to_string();
                for m in &impl_decl.methods {
                    let self_deleting = m
                        .params
                        .iter()
                        .any(|p| p.name == "self" && p.is_deleting);
                    let params: Vec<bool> = m
                        .params
                        .iter()
                        .filter(|p| p.name != "self")
                        .map(|p| p.is_deleting)
                        .collect();
                    if self_deleting || params.iter().any(|d| *d) {
                        self.method_deleting
                            .insert((ty.clone(), m.name.clone()), (self_deleting, params));
                    }
                }
            }
            if let DeclKind::Fn(fn_decl) = &decl.kind {
                let takes: Vec<bool> = fn_decl.params.iter().map(|p| p.is_take).collect();
                let deletings: Vec<bool> = fn_decl.params.iter().map(|p| p.is_deleting).collect();
                self.fn_deleting_params.insert(fn_decl.name.clone(), deletings);
                self.fn_take_params.insert(fn_decl.name.clone(), takes);
            }
        }
    }

    fn run(mut self, decls: &[Decl]) -> OwnershipResult {
        // O11: which names are module-level consts. Collected before any body
        // is walked, because a const is visible in every function whether or
        // not its declaration came first in the file.
        for decl in decls {
            if let DeclKind::Const(c) = &decl.kind {
                self.module_consts.insert(c.name.clone());
            }
        }
        self.collect_escaping_closures(decls);
        for decl in decls {
            self.check_decl(decl);
        }
        self.check_size_fences(decls);
        self.check_generic_size_fences(decls);
        OwnershipResult {
            errors: self.errors,
            escaping_closures: self.escaping_closures,
        }
    }

    /// SM1/SM2: a `@small` type asserts it stays within the copy threshold.
    ///
    /// The annotation buys one thing — the error lands *here* rather than at
    /// every use site. Adding a field that pushes a struct past 16 bytes flips
    /// every assignment from copy to move (VS1/VS6), and those errors surface
    /// wherever the type is passed, with only the `MoveReason` note connecting
    /// them back to a field nobody was looking at.
    ///
    /// Nothing else changes: `@small` never raises the threshold and never
    /// makes a type copy that otherwise wouldn't. It's an assertion about
    /// layout, so the check is a size comparison and nothing more (SM1).
    fn check_size_fences(&mut self, decls: &[Decl]) {
        for decl in decls {
            let DeclKind::Struct(s) = &decl.kind else { continue };
            if !s.attrs.iter().any(|a| a == "small") {
                continue;
            }
            let Some(type_id) = self.program.types.get_type_id(&s.name) else { continue };
            let ty = rask_types::Type::Named(type_id);
            let size = self.type_size(&ty);
            if size <= 16 {
                continue;
            }
            // Name the field that took it over: walking in declaration order,
            // the first one whose end crosses 16 is the one a reader would
            // delete or shrink. More useful than the total alone, which says
            // the type is too big without saying where to look.
            let offending_field = self
                .program
                .types
                .get(type_id)
                .and_then(|def| match def {
                    rask_types::TypeDef::Struct { fields, .. } => Some(fields.clone()),
                    _ => None,
                })
                .and_then(|fields| {
                    let mut running = 0usize;
                    for (name, field_ty) in &fields {
                        let field_size = self.type_size(field_ty);
                        running += field_size;
                        if running > 16 {
                            return Some((name.clone(), field_size));
                        }
                    }
                    None
                });
            self.errors.push(OwnershipError {
                kind: OwnershipErrorKind::SmallTypeTooBig {
                    type_name: s.name.clone(),
                    size,
                    offending_field,
                },
                span: decl.span,
            });
        }
    }

    /// SM3: a `@small` generic type has to fit at *every* instantiation.
    ///
    /// The annotation is read at the definition, but it's a promise about the
    /// concrete types callers plug in. `@small struct Pair<T>` is 16 bytes at
    /// `Pair<i64>` and 32 at `Pair<string>` — the same source text, one of
    /// which quietly breaks the promise. Checking only the declaration would
    /// pass both, which is the worst of the two: the annotation says "cheap to
    /// copy" and the reader has no way to tell it isn't.
    ///
    /// So every instantiation the program actually mentions gets sized with
    /// its type arguments substituted in. The error lands on the declaration
    /// rather than the use site, because that's where the promise was made and
    /// where either fix goes — narrow what the type holds, or drop `@small`.
    fn check_generic_size_fences(&mut self, decls: &[Decl]) {
        // Only generic structs carrying the fence. Concrete ones already went
        // through check_size_fences.
        let mut fenced: HashMap<String, Span> = HashMap::new();
        for decl in decls {
            let DeclKind::Struct(s) = &decl.kind else { continue };
            if s.type_params.is_empty() || !s.attrs.iter().any(|a| a == "small") {
                continue;
            }
            fenced.insert(s.name.clone(), decl.span);
        }
        if fenced.is_empty() {
            return;
        }

        // Every generic type the program mentions, from the types the checker
        // settled on. Nested ones count too — `Vec<Pair<string>>` instantiates
        // `Pair<string>` just as much as a bare binding does.
        let mut instances: Vec<(rask_types::TypeId, Vec<rask_types::GenericArg>)> = Vec::new();
        for ty in self.program.node_types.values() {
            collect_generic_instances(ty, &mut instances);
        }

        let mut reported: HashSet<String> = HashSet::new();
        for (base, args) in instances {
            let Some(rask_types::TypeDef::Struct { name, type_params, fields, .. }) =
                self.program.types.get(base)
            else {
                continue;
            };
            let (name, type_params, fields) =
                (name.clone(), type_params.clone(), fields.clone());
            let Some(decl_span) = fenced.get(&name).copied() else { continue };
            if type_params.len() != args.len() {
                continue;
            }

            let full = Type::Generic { base, args: args.clone() };
            let rendered = format!("{}", self.program.types.resolve_type_names(&full));
            if !reported.insert(rendered.clone()) {
                continue;
            }

            let subst: HashMap<&str, &Type> = type_params
                .iter()
                .zip(args.iter())
                .filter_map(|(p, a)| match a {
                    rask_types::GenericArg::Type(t) => Some((p.as_str(), t.as_ref())),
                    _ => None,
                })
                .collect();

            let mut total = 0usize;
            let mut offender = None;
            for (field_name, field_ty) in &fields {
                let concrete = substitute_params(field_ty, &subst);
                let field_size = self.type_size(&concrete);
                total += field_size;
                if total > 16 && offender.is_none() {
                    let ty_text =
                        format!("{}", self.program.types.resolve_type_names(&concrete));
                    offender = Some((field_name.clone(), field_size, ty_text));
                }
            }
            if total <= 16 {
                continue;
            }

            self.errors.push(OwnershipError {
                kind: OwnershipErrorKind::SmallInstantiationTooBig {
                    type_name: rendered,
                    base_name: name.split('<').next().unwrap_or(&name).to_string(),
                    size: total,
                    offending_field: offender,
                },
                span: decl_span,
            });
        }
    }

    fn check_decl(&mut self, decl: &Decl) {
        match &decl.kind {
            DeclKind::Fn(fn_decl) => self.check_fn(fn_decl),
            DeclKind::Struct(s) => {
                // Check methods
                for method in &s.methods {
                    self.check_fn(method);
                }
            }
            DeclKind::Enum(e) => {
                for method in &e.methods {
                    self.check_fn(method);
                }
            }
            DeclKind::Interface(_) => {}
            DeclKind::Extern(_) => {}
            DeclKind::Impl(impl_decl) => {
                for method in &impl_decl.methods {
                    self.check_fn(method);
                }
            }
            DeclKind::Import(_) => {}
            DeclKind::Export(_) => {}
            DeclKind::Const(_) => {} // Module-level consts handled differently
            // "Like a function" has to include the exit check, or a `test` block
            // is the one place linearity doesn't apply — which is exactly where
            // resources get exercised.
            DeclKind::Test(test_decl) => {
                self.check_body(&test_decl.body);
            }
            DeclKind::Benchmark(bench_decl) => {
                self.check_body(&bench_decl.body);
            }
            DeclKind::Package(_) | DeclKind::CImport(_) => {}
            DeclKind::Union(_) => {}
            DeclKind::TypeAlias(_) => {}
            DeclKind::Annotation(_) => {}
        }
    }

    /// Per-body state. `binding_types` and `binding_decl_blocks` were not being
    /// reset, so a name declared in one body stayed typed in the next — latent
    /// until something iterated the map to decide which names to invalidate, at
    /// which point a `first` from one test body got marked dead in another.
    fn reset_body_state(&mut self) {
        self.bindings.clear();
        self.binding_types.clear();
        self.binding_decl_blocks.clear();
        self.borrow_bindings.clear();
        self.borrows.clear();
        self.resource_bindings.clear();
        // Keyed by name, so a `c` in one function would otherwise hand the next
        // function's `c` its line: E0881 in one body cited another body's `let`
        // as where the resource was acquired.
        self.resource_acquired_at.clear();
        self.owned_bindings.clear();
        self.ensure_registered.clear();
        self.ensure_spans.clear();
        self.in_ensure = false;
        self.active_with_bindings.clear();
        self.active_for_mutates.clear();
        self.scope_limited_closures.clear();
        self.closure_scope_limits.clear();
        // Keyed by name too, so an `f` in one body would otherwise answer for
        // the next body's `f`: a `spawn(f)` in a function that borrowed its
        // closure got checked against a body from somewhere else, and reported
        // that body's write at that body's line.
        self.closure_literals.clear();
        self.mutable_captures.clear();
        self.param_type_strings.clear();
        self.identified_links.clear();
        self.coarse_resources.clear();
        self.resource_field_debts.clear();
        self.borrowed_params.clear();
        self.mutate_params.clear();
        self.exit_reported.clear();
        self.deleting_params.clear();
        self.link_rack_root.clear();
        self.container_link_rack.clear();
        self.writable_racks.clear();
        self.link_params.clear();
        self.writable_links.clear();
        self.take_link_params.clear();
        self.link_delete_spans.clear();
        self.rack_iterations.clear();
        self.current_block = 0;
        self.current_stmt = 0;
    }

    fn check_fn(&mut self, fn_decl: &FnDecl) {
        self.reset_body_state();

        for param in &fn_decl.params {
            if param.is_take && param.ty.starts_with("Link<") {
                self.take_link_params.insert(param.name.clone());
            }
            if param.is_deleting {
                self.deleting_params.insert(param.name.clone());
            }
            if param.ty.starts_with("Link<") {
                self.link_params.insert(param.name.clone());
                if param.is_mutate || param.is_deleting {
                    self.writable_links.insert(param.name.clone());
                }
            }
        }

        // Register parameter type strings for W2 pool detection
        self.param_type_strings.clear();
        for param in &fn_decl.params {
            self.param_type_strings.insert(param.name.clone(), param.ty.clone());
        }

        // A rack reached through a `mutate`/`deleting` parameter is writable; one
        // reached through a plain borrow is the caller's to write. Has to come
        // after the map above — `name_holds_rack` reads it, so running this in the
        // earlier loop silently answered "no" for every parameter.
        for param in &fn_decl.params {
            if (param.is_mutate || param.is_deleting) && self.name_holds_rack(&param.name) {
                self.writable_racks.insert(param.name.clone());
            }
        }

        // What each parameter's type is, the same way a `let` records one.
        //
        // Nothing did this, so `consume_arg` — which lets a Copy value through,
        // because handing one over copies it and the caller keeps theirs — found
        // no type for any parameter and treated every one as move-only. Passing
        // a borrowed `n: i32` to a `take` parameter was rejected as giving away
        // something that isn't yours, when an i32 is four bytes of copy.
        for param in &fn_decl.params {
            if param.name == "self" {
                continue;
            }
            if let Some(ty) = self.type_from_name(&param.ty) {
                self.binding_types.insert(param.name.clone(), ty);
            }
        }

        // Register parameters as owned or borrowed bindings
        self.borrowed_params.clear();
        self.mutate_params.clear();
        for param in &fn_decl.params {
            if param.is_mutate {
                // "Resource types must be consumed exactly once. Only `take`
                // parameters can consume them" (mem.parameters). A resource behind
                // a `mutate` borrow can't be given away even if something is put
                // back, so it joins the borrows instead — consume-and-replace is
                // for ordinary move-only values, where the spec is silent.
                if self.is_resource_type_name(&param.ty) {
                    self.borrowed_params
                        .insert(param.name.clone(), (param.name_span, true));
                } else {
                    self.mutate_params.insert(param.name.clone(), param.name_span);
                }
            }
            if !param.is_take && !param.is_mutate {
                // What the caller lent out. Giving it away is an error — they keep
                // it and go on using it (#804).
                //
                // A `mutate` parameter is left out on purpose. It's exclusive
                // access, so taking the value out and writing a replacement back is
                // a legitimate pattern — `out.push(b.build()); b = StringBuilder.new()`
                // is what `mutate` is *for*. What's missing there is the other half
                // of the rule: consuming one and putting nothing back leaves the
                // caller holding a hole, and nothing checks that yet.
                self.borrowed_params
                    .insert(param.name.clone(), (param.name_span, param.is_mutate));
            }
            if param.is_take {
                // `take` parameter: owned
                self.bindings.insert(param.name.clone(), BindingState::Owned);
                // Check if it's a resource type
                if self.is_resource_type_name(&param.ty) {
                    let ty = self.declared_type_from_name(&param.ty);
                    self.register_resource_binding(&param.name.clone(), ty.as_ref());
                    // A `take` parameter arrives owed, same as a local the body
                    // acquired. The signature is where it came from, so that's
                    // what L7 points back at.
                    self.resource_acquired_at
                        .insert(param.name.clone(), param.name_span);
                }
            } else if param.is_mutate {
                // Mutate parameters: treat as owned within the body.
                // The caller holds the exclusive borrow; within the function
                // we can freely read and write the parameter.
                self.bindings.insert(param.name.clone(), BindingState::Owned);
            } else {
                // Shared (non-mutate, non-take): borrowed for call duration
                self.bindings.insert(
                    param.name.clone(),
                    BindingState::Borrowed {
                        mode: BorrowMode::Shared,
                        scope: BorrowScope::Persistent { block_id: 0 },
                    },
                );
                let borrow = ActiveBorrow::new(
                    param.name.clone(),
                    BorrowMode::Shared,
                    BorrowScope::Persistent { block_id: 0 },
                    Span::new(0, 0),
                );
                self.borrows.push(borrow);
            }
        }

        // Check function body
        self.check_block(&fn_decl.body);

        // Check for unconsumed resources at function exit. The closing brace, not
        // the last statement — "goes out of scope here" pointing at whatever
        // happened to be written last reads as an accusation of that line.
        let exit_span = if fn_decl.span.end > fn_decl.span.start {
            Span::new(fn_decl.span.end.saturating_sub(1), fn_decl.span.end)
        } else {
            fn_decl.body.last().map(|s| s.span).unwrap_or(Span::new(0, 0))
        };
        self.check_resource_consumption(exit_span);
        self.check_mutate_params_refilled(exit_span);
    }

    /// A body with no parameters: reset, walk, then the scope-exit check.
    fn check_body(&mut self, body: &[Stmt]) {
        self.reset_body_state();
        self.check_block(body);
        self.check_resource_consumption(
            body.last().map(|s| s.span).unwrap_or(Span::new(0, 0)),
        );
    }

    fn check_block(&mut self, stmts: &[Stmt]) {
        let block_id = self.current_block;
        self.current_block += 1;
        let resources_on_entry: HashSet<String> = self.resource_bindings.clone();

        // MC2 needs to know where each name is mentioned for the last time, so
        // a mutable capture can stop being exclusive once the closure holding
        // it is done. Cheaper here than during the walk: the whole list is in
        // hand, and it is one pass.
        let last_mention = Self::last_mentions(stmts);

        for (index, stmt) in stmts.iter().enumerate() {
            self.check_mutable_capture_access(stmt);
            let owed_before = self.resource_bindings.len();
            let pending_before = self.commit_state();
            let errors_before = self.errors.len();
            self.check_stmt(stmt);
            // Where each obligation started, for a diagnostic about a later
            // statement to point back at. One place rather than at each of the
            // ten registration sites, and it reads the same: whatever statement
            // put the name on the list is where the resource came from.
            if self.resource_bindings.len() != owed_before {
                let fresh: Vec<String> = self
                    .resource_bindings
                    .iter()
                    .filter(|n| !self.resource_acquired_at.contains_key(*n))
                    .cloned()
                    .collect();
                for name in fresh {
                    self.resource_acquired_at.insert(name, stmt.span);
                }
            }
            self.check_commit_window(&pending_before, errors_before, stmt);
            // After the walk, not before: the closure's own body is where the
            // write lives, and checking it against its own record would report
            // every mutable capture as a conflict with itself.
            self.register_mutable_capture(stmt, index, block_id, &last_mention);
            self.current_stmt += 1;

            // Release instant borrows at statement end
            self.release_instant_borrows(self.current_stmt - 1);
        }
        self.mutable_captures.retain(|c| c.block != block_id);

        // Release persistent borrows at block end
        self.release_persistent_borrows(block_id);

        // Resources this block introduced go out of scope here.
        self.check_block_resources(
            &resources_on_entry,
            stmts.last().map(|s| s.span).unwrap_or(Span::new(0, 0)),
        );

        // SL2: Check if any scope-limited closures would escape this block.
        // A closure escapes if its borrow_block is inside the block being exited
        // but its binding_block is outside (binding outlives the borrow).
        let block_inner = block_id + 1;
        let escaping: Vec<String> = self.scope_limited_closures.iter()
            .filter(|(_, &(borrow_block, binding_block))| {
                // Borrow was created in this block or deeper, but binding is in outer scope
                borrow_block >= block_inner && binding_block < block_inner
            })
            .map(|(name, _)| name.clone())
            .collect();
        for name in &escaping {
            if matches!(self.bindings.get(name), Some(BindingState::Owned)) {
                self.errors.push(OwnershipError {
                    kind: OwnershipErrorKind::ScopeLimitedClosureEscapes {
                        name: name.clone(),
                    },
                    span: stmts.last().map(|s| s.span).unwrap_or(Span::new(0, 0)),
                });
            }
            self.scope_limited_closures.remove(name);
        }

        // Clean up scope-limited closures whose bindings were in this block
        // (they naturally die with the block — no escape).
        let dying: Vec<String> = self.scope_limited_closures.iter()
            .filter(|(_, &(_, binding_block))| binding_block >= block_inner)
            .map(|(name, _)| name.clone())
            .collect();
        for name in dying {
            self.scope_limited_closures.remove(&name);
        }

        self.current_block = block_id;
    }

    /// Collect the names a pattern binds (for loop per-iteration exclusion).
    fn collect_pattern_binding_names(pattern: &Pattern, out: &mut Vec<String>) {
        match pattern {
            Pattern::Ident(name) if !name.contains('.') => out.push(name.clone()),
            Pattern::Tuple(pats) | Pattern::Constructor { fields: pats, .. } => {
                for p in pats {
                    Self::collect_pattern_binding_names(p, out);
                }
            }
            Pattern::Struct { fields, .. } => {
                for (_, p) in fields {
                    Self::collect_pattern_binding_names(p, out);
                }
            }
            Pattern::Or(pats) => {
                for p in pats {
                    Self::collect_pattern_binding_names(p, out);
                }
            }
            Pattern::TypePat { binding: Some(name), .. } => out.push(name.clone()),
            _ => {}
        }
    }

    /// Check a loop body accounting for values carried across iterations.
    ///
    /// A move inside a loop body is a use-after-move on the next iteration.
    /// One linear pass misses it (the move looks like it happens once), so
    /// after discovering which pre-loop bindings the body moves, re-run the
    /// body with those pre-moved to catch the second-iteration use. `exclude`
    /// names the loop's own per-iteration bindings (for/while-let), which are
    /// freshly bound each iteration and are not carried.
    fn check_loop_body(&mut self, body: &[Stmt], exclude: &[String]) {
        let pre_loop = self.bindings.clone();
        let saved_errors = self.errors.len();
        self.loop_entry_resources.push(self.resource_bindings.clone());

        // Pass 1: discover which pre-loop bindings the body consumes.
        self.check_block(body);

        let carried: Vec<(String, Span)> = pre_loop
            .iter()
            .filter(|(name, pre)| Self::is_available(pre) && !exclude.contains(name))
            .filter_map(|(name, _)| match self.bindings.get(name) {
                Some(state) if !Self::is_available(state) => {
                    Self::unavailable_span(state).map(|at| (name.clone(), at))
                }
                _ => None,
            })
            .collect();

        if !carried.is_empty() {
            // Pass 2: re-analyze with carried values pre-moved. Discard pass-1
            // errors — pass 2 sees a strict superset (stricter entry state).
            self.errors.truncate(saved_errors);
            self.bindings = pre_loop;
            for (name, at) in &carried {
                self.bindings
                    .insert(name.clone(), BindingState::MaybeMoved { at: *at });
            }
            self.check_block(body);
        }

        // After the loop, a value the body consumes is only maybe-consumed —
        // the loop may run zero times (mem.linear/L1, ctrl.ensure/C3). Don't
        // clobber loop-local binding states.
        for (name, at) in &carried {
            self.bindings
                .insert(name.clone(), BindingState::MaybeMoved { at: *at });
        }
        self.loop_entry_resources.pop();
    }

    /// E4: `let x = collection[key]` copies when the element is Copy and is a
    /// compile error when it isn't.
    ///
    /// Indexing hands back the element in place, so a non-Copy binding is a
    /// second name for storage the collection still owns — writing through
    /// either writes both, on both backends. `with` is the form that says the
    /// access is scoped; `.clone()` is the form that says a second value is
    /// wanted and pays for it.
    ///
    /// A field projection is a different rule and stays allowed: a view into a
    /// struct field lives until the block ends (S1), and the field's owner is
    /// right there in the same scope.
    fn check_index_binding(&mut self, name: &str, init: &Expr, is_mut: bool) {
        if let ExprKind::Field { .. } = &init.kind {
            self.check_mutable_field_view(name, init, is_mut);
            return;
        }
        if !matches!(init.kind, ExprKind::Index { .. }) {
            return;
        }
        let Some(ty) = self.program.node_types.get(&init.id).cloned() else {
            return;
        };
        if !self.definitely_not_copy(&ty) {
            return;
        }
        let collection = match &init.kind {
            ExprKind::Index { object, .. } => Self::render_place(object),
            _ => None,
        };
        self.errors.push(OwnershipError {
            kind: OwnershipErrorKind::NonCopyElementCopiedOut {
                binding: name.to_string(),
                elem_ty: self.resource_type_display(&ty),
                collection,
            },
            span: init.span,
        });
    }

    /// S5: `mut x = value.field` on a field that isn't Copy.
    ///
    /// A field read is a view that lives until the block ends (S1), and a
    /// read-only view is fine — the source stays readable beside it. Binding one
    /// as `mut` asks for something else: S5 says a mutable borrow excludes all
    /// other access to the source, and a plain binding has no way to say for how
    /// long. So the two names stay live together and a write through either is a
    /// write through both — `escaped.push(1)` puts an element in `h.data`.
    /// `with` is the form that scopes the exclusion; `.clone()` is the form that
    /// asks for a separate value.
    fn check_mutable_field_view(&mut self, name: &str, init: &Expr, is_mut: bool) {
        if !is_mut {
            return;
        }
        let (Some(root), Some(fields)) = Self::extract_root_and_fields(init) else {
            return;
        };
        if fields.is_empty() || !self.names_a_value(&root) {
            return;
        }
        let Some(ty) = self.program.node_types.get(&init.id).cloned() else {
            return;
        };
        if !self.definitely_not_copy(&ty) {
            return;
        }
        self.errors.push(OwnershipError {
            kind: OwnershipErrorKind::MutableFieldView {
                binding: name.to_string(),
                path: format!("{}.{}", root, fields.join(".")),
                field_ty: self.resource_type_display(&ty),
            },
            span: init.span,
        });
    }

    /// `is_copy` answers "treat this as a move", so a type it can't place — a
    /// name the type table never resolved, an inference variable, a generic it
    /// has no declaration for — comes back non-Copy. That is the safe direction
    /// for a move analysis and the wrong one for a rejection: it would reject on
    /// "couldn't tell". This asks the narrower question, and says yes only for a
    /// type the pass can actually look up.
    fn definitely_not_copy(&self, ty: &Type) -> bool {
        let placed = match ty {
            Type::Result { .. } | Type::Union(_) => true,
            Type::Named(id) => self.program.types.get(*id).is_some(),
            Type::Generic { base, .. } => {
                let name = self.program.types.type_name(*base);
                Self::is_native_opaque_generic(&name) || self.program.types.get(*base).is_some()
            }
            _ => false,
        };
        placed && !self.is_copy(ty)
    }

    /// A place expression rendered back to source, for a message. `None` for
    /// anything that isn't a plain name or field chain.
    fn render_place(expr: &Expr) -> Option<String> {
        match &expr.kind {
            ExprKind::Ident(n) => Some(n.clone()),
            ExprKind::Field { object, field } => {
                Some(format!("{}.{}", Self::render_place(object)?, field))
            }
            _ => None,
        }
    }

    fn check_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Mut { name, name_span: _, ty, init } => {
                // `mut` is what makes a rack's nodes writable here.
                self.writable_racks.insert(name.clone());
                self.check_expr(init);
                self.check_index_binding(name, init, true);
                // let: Copy types are copied (source stays valid),
                // non-Copy types are moved (source invalidated)
                self.handle_assignment(init, stmt.span, true);
                self.bindings.insert(name.clone(), BindingState::Owned);
                self.binding_decl_blocks.insert(name.clone(), self.current_block);
                self.record_lent_binding(name, init);
                if let Some(t) = self.program.node_types.get(&init.id).cloned() {
                    self.record_link_provenance(name, &t, init);
                    self.binding_types.insert(name.clone(), t);
                }
                // SL1: inherit scope limit from closure expression
                if let Some(&borrow_block) = self.closure_scope_limits.get(&init.id) {
                    self.scope_limited_closures.insert(name.clone(), (borrow_block, self.current_block));
                }
                if matches!(init.kind, ExprKind::Closure { .. }) {
                    self.closure_literals.insert(name.clone(), init.clone());
                }
                // Track resource types. The annotation and the initializer are
                // both asked: an annotation the name table can't place — a
                // wrapper, an alias — used to suppress what the checker had
                // already worked out about the value (#827).
                //
                // `= none` holds nothing, so there is nothing to consume yet. A
                // later assignment registers the binding when it puts a resource
                // in — see the `Assign` arm.
                if !matches!(init.kind, ExprKind::None)
                    && (ty.as_ref().is_some_and(|t| self.is_resource_type_name(t))
                        || self.expr_is_resource_type(init))
                {
                    let value_ty = self.program.node_types.get(&init.id).cloned();
                    self.register_resource_binding(name, value_ty.as_ref());
                }
                self.track_owned_binding(name, init);
            }
            StmtKind::MutTuple { patterns, init } => {
                self.check_expr(init);
                self.handle_assignment(init, stmt.span, true);
                let names = rask_ast::stmt::tuple_pats_flat_names(patterns);
                let elem_types = match self.program.node_types.get(&init.id) {
                    Some(Type::Tuple(elems)) => Some(elems.clone()),
                    _ => None,
                };
                for (i, name) in names.iter().enumerate() {
                    self.bindings.insert(name.to_string(), BindingState::Owned);
                    if let Some(ref elems) = elem_types {
                        if let Some(elem_ty) = elems.get(i) {
                            self.binding_types.insert(name.to_string(), elem_ty.clone());
                            if self.type_is_resource(elem_ty) {
                                self.resource_bindings.insert(name.to_string());
                            }
                        }
                    }
                }
            }
            StmtKind::Let { name, name_span: _, ty, init } => {
                self.check_expr(init);
                self.check_index_binding(name, init, false);
                // non-Copy types are moved (O3); field/index projections create borrows.
                self.handle_assignment(init, stmt.span, false);
                self.bindings.insert(name.clone(), BindingState::Owned);
                self.binding_decl_blocks.insert(name.clone(), self.current_block);
                self.record_lent_binding(name, init);
                if let Some(t) = self.program.node_types.get(&init.id).cloned() {
                    self.binding_types.insert(name.clone(), t.clone());
                    self.record_link_provenance(name, &t, init);
                    // SL1: Only a projection (field/index) borrow creates a borrow view.
                    // Whole-variable moves create owned bindings; closures can capture freely.
                    let (_, projection) = Self::extract_root_and_fields(init);
                    if !self.is_copy(&t) && projection.is_some() {
                        self.borrow_bindings.insert(name.clone(), self.current_block);
                    }
                }
                // SL1: inherit scope limit from closure expression
                if let Some(&borrow_block) = self.closure_scope_limits.get(&init.id) {
                    self.scope_limited_closures.insert(name.clone(), (borrow_block, self.current_block));
                }
                if matches!(init.kind, ExprKind::Closure { .. }) {
                    self.closure_literals.insert(name.clone(), init.clone());
                }
                // Track resource types. The annotation and the initializer are
                // both asked: an annotation the name table can't place — a
                // wrapper, an alias — used to suppress what the checker had
                // already worked out about the value (#827).
                //
                // `= none` holds nothing, so there is nothing to consume yet. A
                // later assignment registers the binding when it puts a resource
                // in — see the `Assign` arm.
                if !matches!(init.kind, ExprKind::None)
                    && (ty.as_ref().is_some_and(|t| self.is_resource_type_name(t))
                        || self.expr_is_resource_type(init))
                {
                    let value_ty = self.program.node_types.get(&init.id).cloned();
                    self.register_resource_binding(name, value_ty.as_ref());
                }
                self.track_owned_binding(name, init);
            }
            // `let Point { x, .. } = p` — reading fields out of the source is a
            // projection, so the source is borrowed rather than moved, exactly as
            // `let x = p.x` would be (F1). Moving the whole struct instead would
            // make one destructuring the last use of the value, which is not what
            // reading two of its fields means.
            StmtKind::LetStruct { pattern, init, is_mut } => {
                self.check_expr(init);
                if let (ExprKind::Ident(source), Pattern::Struct { fields, .. }) =
                    (&init.kind, pattern)
                {
                    let mode = if *is_mut {
                        BorrowMode::Exclusive
                    } else {
                        BorrowMode::Shared
                    };
                    for (field_name, _) in fields {
                        self.create_borrow_with_projection(
                            source.clone(),
                            mode,
                            stmt.span,
                            Some(vec![field_name.clone()]),
                        );
                    }
                } else {
                    self.handle_assignment(init, stmt.span, *is_mut);
                }
                for name in rask_ast::stmt::pattern_binding_names(pattern) {
                    self.bindings.insert(name.clone(), BindingState::Owned);
                    self.binding_decl_blocks.insert(name, self.current_block);
                }
            }
            StmtKind::LetTuple { patterns, init } => {
                self.check_expr(init);
                self.handle_assignment(init, stmt.span, false);
                let names = rask_ast::stmt::tuple_pats_flat_names(patterns);
                let elem_types = match self.program.node_types.get(&init.id) {
                    Some(Type::Tuple(elems)) => Some(elems.clone()),
                    _ => None,
                };
                for (i, name) in names.iter().enumerate() {
                    self.bindings.insert(name.to_string(), BindingState::Owned);
                    if let Some(ref elems) = elem_types {
                        if let Some(elem_ty) = elems.get(i) {
                            self.binding_types.insert(name.to_string(), elem_ty.clone());
                            if self.type_is_resource(elem_ty) {
                                self.resource_bindings.insert(name.to_string());
                            }
                        }
                    }
                }
            }
            StmtKind::Expr(expr) => {
                self.check_expr(expr);
                // H1/L1: a resource-typed value with nothing to bind it to is
                // dropped the instant it's produced — e.g. `spawn(f)` used as
                // a bare statement, with the TaskHandle never joined/detached.
                // A bare `Ident` is never a *fresh* value — it names an
                // existing binding, which the end-of-scope check (E0805)
                // already tracks; flagging it here too would double-report
                // the same leak.
                if !matches!(expr.kind, ExprKind::Ident(_)) && self.expr_is_resource_type(expr) {
                    let type_name = self.program.node_types.get(&expr.id)
                        .map(|ty| self.resource_type_display(ty))
                        .unwrap_or_else(|| "?".to_string());
                    self.errors.push(OwnershipError {
                        kind: OwnershipErrorKind::ResourceDiscardedAsStatement { type_name },
                        span: expr.span,
                    });
                }
            }
            StmtKind::Assign { target, value, .. } => {
                self.check_expr(value);
                if let ExprKind::Ident(name) = &target.kind {
                    self.record_lent_binding(name, value);
                }
                // A whole-variable assignment reinitializes the target — it is
                // not a use of the old value, so don't flag a moved/maybe-moved
                // target here (the type checker already forbids assigning a
                // `const`). Field/index targets are a genuine use of the root.
                let reinit_target = match &target.kind {
                    ExprKind::Ident(_) => true,
                    _ => {
                        self.check_expr(target);
                        false
                    }
                };
                // Assignments move the value — except a link into a field.
                //
                // A link is a pointer, so both cases copy the same pointer; what
                // differs is who keeps it honest afterwards. The rack maintains a
                // field-held link (it nulls it at delete), so the source name stays
                // good. Nothing maintains a local, so a local-to-local copy revokes
                // the source and `delete` takes it — which is what makes
                // use-after-delete a compile error (analysis.fourth-option).
                // A node write asks the *rack*, not the link. A link is an
                // access path — following an edge doesn't grant anything the
                // rack didn't already grant, which is why read-only needs no
                // second link type and can't be laundered by one hop.
                if !reinit_target {
                    self.check_node_write(target, stmt.span);
                }
                if let Some(target_root) = Self::extract_root_and_fields(target).0 {
                    self.check_link_escape(
                        value,
                        LinkEscape::Assignment { target: target_root },
                        stmt.span,
                    );
                }
                let link_into_field = !reinit_target
                    && self
                        .program
                        .node_types
                        .get(&value.id)
                        .cloned()
                        .is_some_and(|ty| self.is_link_type(&ty));
                if !link_into_field {
                    self.handle_assignment(value, stmt.span, true);
                }
                if reinit_target {
                    if let ExprKind::Ident(target_name) = &target.kind {
                        // Rebinding replaces what the name holds, links included:
                        // `v = Vec.new()` after a `v.push(n)` points at nothing.
                        match self.link_bearing_root(value) {
                            Some(rack) => {
                                self.container_link_rack.insert(target_name.clone(), rack);
                            }
                            None => {
                                self.container_link_rack.remove(target_name);
                            }
                        }
                        self.bindings.insert(target_name.clone(), BindingState::Owned);
                        // Putting a resource into a binding gives it the
                        // obligation, whether or not it had one before. This is
                        // what makes `mut c: Conn? = none` work: nothing to
                        // consume at the declaration, and a real one to consume
                        // once something fills it (#827).
                        // Unless the target is a parameter: PM2 hands a `mutate`
                        // slot back to the caller, so refilling one is the
                        // consume-and-replace pattern, not a new obligation this
                        // body owes. Registering it made `c = Conn { … }` inside
                        // `churn(mutate c: Conn)` report a leak of the very value
                        // the caller is about to get back.
                        if self.expr_is_resource_type(value)
                            && !self.mutate_params.contains_key(target_name)
                            && !self.borrowed_params.contains_key(target_name)
                        {
                            self.resource_bindings.insert(target_name.clone());
                        }
                    }
                }
                // SL2: propagate or reject scope-limited closure on assignment.
                // If the target is a plain binding, propagate the scope limit so
                // later uses of that binding are still caught. If the target is a
                // field/index (can't be tracked), treat it as an escape.
                // A name rebound stands for what it holds now. Without this a
                // `mut f` reassigned to a second closure still answered with
                // the first one's body, so `spawn(f)` was checked against a
                // closure the program had thrown away.
                if let ExprKind::Ident(target_name) = &target.kind {
                    if matches!(value.kind, ExprKind::Closure { .. }) {
                        self.closure_literals.insert(target_name.clone(), value.clone());
                    } else {
                        self.closure_literals.remove(target_name);
                    }
                }
                if let ExprKind::Ident(value_name) = &value.kind {
                    if let Some(&(borrow_block, _)) = self.scope_limited_closures.get(value_name) {
                        if let ExprKind::Ident(target_name) = &target.kind {
                            let decl_block = self.binding_decl_blocks
                                .get(target_name).copied()
                                .unwrap_or(self.current_block);
                            self.scope_limited_closures.insert(
                                target_name.clone(),
                                (borrow_block, decl_block),
                            );
                        } else {
                            // Field/index assignment — cannot track, treat as escape
                            self.errors.push(OwnershipError {
                                kind: OwnershipErrorKind::ScopeLimitedClosureEscapes {
                                    name: value_name.clone(),
                                },
                                span: value.span,
                            });
                            self.scope_limited_closures.remove(value_name);
                        }
                    }
                }
                if let Some(&borrow_block) = self.closure_scope_limits.get(&value.id) {
                    if let ExprKind::Ident(target_name) = &target.kind {
                        let decl_block = self.binding_decl_blocks
                            .get(target_name).copied()
                            .unwrap_or(self.current_block);
                        self.scope_limited_closures.insert(
                            target_name.clone(),
                            (borrow_block, decl_block),
                        );
                    } else if self.is_borrowing_closure(value) {
                        // Direct closure literal assigned to a field/index — escape
                        self.errors.push(OwnershipError {
                            kind: OwnershipErrorKind::ScopeLimitedClosureEscapes {
                                name: "<closure>".to_string(),
                            },
                            span: value.span,
                        });
                    }
                }
            }
            StmtKind::Return(expr) => {
                if let Some(expr) = expr {
                    self.check_expr(expr);
                    self.consume_returned_resources(expr);
                    self.check_borrowed_field_escape(expr);
                    self.check_lent_return(expr);
                    self.check_link_escape(expr, LinkEscape::Return, stmt.span);
                    // Control leaves here, so this is an exit like any other. The
                    // end-of-body check alone misses an early return that skips a
                    // consume or a replacement.
                    self.check_exit_obligations(stmt.span);
                    // SL2: Check if returning a scope-limited closure
                    if let ExprKind::Ident(name) = &expr.kind {
                        if self.scope_limited_closures.contains_key(name) {
                            self.errors.push(OwnershipError {
                                kind: OwnershipErrorKind::ScopeLimitedClosureEscapes {
                                    name: name.clone(),
                                },
                                span: stmt.span,
                            });
                            // Remove to avoid double-reporting at block exit
                            self.scope_limited_closures.remove(name);
                        }
                    } else if self.closure_scope_limits.contains_key(&expr.id) {
                        // A returned expression carrying a scope limit: a
                        // non-`own` closure literal over a local, or (SL3) a
                        // call that built one over an argument this frame owns.
                        // `return make(v)` hands back a closure over `v`, and
                        // `v` dies here.
                        self.errors.push(OwnershipError {
                            kind: OwnershipErrorKind::ScopeLimitedClosureEscapes {
                                name: "<closure>".to_string(),
                            },
                            span: stmt.span,
                        });
                    }
                } else {
                    // A bare `return` is an exit too.
                    self.check_exit_obligations(stmt.span);
                }
            }
            StmtKind::While { cond, body, .. } => {
                self.check_expr(cond);
                self.check_loop_body(body, &[]);
            }
            StmtKind::WhileLet { pattern, expr, body, .. } => {
                self.check_expr(expr);
                let scrutinee_ty = self.program.node_types.get(&expr.id).cloned();
                self.register_pattern_bindings_typed(pattern, scrutinee_ty.as_ref(), expr.span);
                let mut bound = Vec::new();
                Self::collect_pattern_binding_names(pattern, &mut bound);
                self.check_loop_body(body, &bound);
            }
            StmtKind::For { label: _, binding, mutate, iter, body, .. } => {
                self.check_expr(iter);
                let binding_names: Vec<String> = binding.names().iter().map(|s| s.to_string()).collect();
                match binding {
                    ForBinding::Single(name) => {
                        self.bindings.insert(name.clone(), BindingState::Owned);
                    }
                    ForBinding::Tuple(names) => {
                        for name in names {
                            self.bindings.insert(String::clone(name), BindingState::Owned);
                        }
                    }
                }
                let iterates_rack = self.rack_iteration_elem(iter);
                if let Some(elem) = iterates_rack {
                    // The element of a rack iteration is a link, and the binding
                    // needs the type recorded or nothing downstream can tell it is
                    // one — that is what makes it a *derived* link rather than an
                    // untyped name.
                    if let ForBinding::Single(name) = binding {
                        if let Some(ty) = self
                            .program
                            .node_types
                            .get(&iter.id)
                            .and_then(|t| Self::sequence_element(t))
                        {
                            self.binding_types.insert(name.clone(), ty);
                        }
                        if let Some(root) = self.link_root_of_expr(iter) {
                            self.link_rack_root.insert(name.clone(), root);
                        }
                    }
                    self.rack_iterations.push((elem, binding_names.clone()));
                }
                // LP14/LP16: track for-mutate context
                if *mutate {
                    let collection_name = Self::extract_iter_collection(iter);
                    if let Some(coll) = collection_name {
                        self.active_for_mutates.push(ForMutateInfo {
                            collection_name: coll,
                            binding_names: binding_names.clone(),
                            span: stmt.span,
                        });
                    }
                }
                self.check_loop_body(body, &binding_names);
                if *mutate {
                    self.active_for_mutates.pop();
                }
                if self.rack_iteration_elem(iter).is_some() {
                    self.rack_iterations.pop();
                }
            }
            StmtKind::Loop { label: _, body } => {
                self.check_loop_body(body, &[]);
            }
            StmtKind::Break { value, .. } => {
                if let Some(v) = value {
                    self.check_expr(v);
                }
                self.check_loop_exit_obligations(stmt.span);
            }
            StmtKind::Continue(_) => {
                self.check_loop_exit_obligations(stmt.span);
            }
            StmtKind::Ensure { body, else_handler } => {
                // Mark resources referenced in ensure body as consumption-committed
                for s in body {
                    self.mark_ensure_resources(s, stmt.span);
                }
                let prev = self.in_ensure;
                self.in_ensure = true;
                self.check_block(body);
                self.in_ensure = prev;
                if let Some((_name, handler)) = else_handler {
                    self.check_block(handler);
                }
            }
            StmtKind::Comptime(body) => {
                self.check_block(body);
            }
            StmtKind::ComptimeFor { iter, body, .. } => {
                self.check_expr(iter);
                self.check_block(body);
            }
            StmtKind::Discard { name, .. } => {
                // D3: resource types cannot be discarded
                if self.resource_bindings.contains(name) {
                    self.errors.push(OwnershipError {
                        kind: OwnershipErrorKind::DiscardResource {
                            name: name.clone(),
                        },
                        span: stmt.span,
                    });
                } else {
                    // Mark the binding as discarded — subsequent uses are errors
                    self.bindings.insert(name.clone(), BindingState::Discarded { at: stmt.span });
                }
            }
        }
    }

    fn check_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Ident(name) => {
                // Check if this identifier is used after move
                if let Some(state) = self.bindings.get(name) {
                    match state {
                        BindingState::Moved { at } => {
                            // The binding's own reason first: an `Owned` box reads
                            // as its payload type, so going by the node's type
                            // blamed the copy threshold and suggested `.clone()`
                            // on something that had just been freed (#819).
                            let reason = if self.owned_bindings.contains(name) {
                                MoveReason::Owned
                            } else {
                                self.program.node_types.get(&expr.id)
                                    .map(|ty| self.move_reason_at(ty, *at))
                                    .unwrap_or_else(|| self.move_reason_for_at(name, *at))
                            };
                            self.errors.push(OwnershipError {
                                kind: OwnershipErrorKind::UseAfterMove {
                                    name: name.clone(),
                                    moved_at: *at,
                                    reason,
                                },
                                span: expr.span,
                            });
                        }
                        BindingState::MaybeMoved { at } => {
                            // The binding's own reason first: an `Owned` box reads
                            // as its payload type, so going by the node's type
                            // blamed the copy threshold and suggested `.clone()`
                            // on something that had just been freed (#819).
                            let reason = if self.owned_bindings.contains(name) {
                                MoveReason::Owned
                            } else {
                                self.program.node_types.get(&expr.id)
                                    .map(|ty| self.move_reason_at(ty, *at))
                                    .unwrap_or_else(|| self.move_reason_for_at(name, *at))
                            };
                            self.errors.push(OwnershipError {
                                kind: OwnershipErrorKind::UseAfterMaybeMove {
                                    name: name.clone(),
                                    moved_at: *at,
                                    reason,
                                },
                                span: expr.span,
                            });
                        }
                        BindingState::Discarded { at } => {
                            self.errors.push(OwnershipError {
                                kind: OwnershipErrorKind::UseAfterDiscard {
                                    name: name.clone(),
                                    discarded_at: *at,
                                },
                                span: expr.span,
                            });
                        }
                        _ => {}
                    }
                }
            }
            ExprKind::Int(_, _) | ExprKind::Float(_, _) | ExprKind::String(_)
            | ExprKind::StringInterp(_)
            | ExprKind::Char(_) | ExprKind::Bool(_) | ExprKind::Null | ExprKind::None => {}

            ExprKind::Binary { left, op: _, right } => {
                self.check_expr(left);
                self.check_expr(right);
            }
            ExprKind::Unary { op: _, operand } => {
                self.check_expr(operand);
            }
            ExprKind::Call { func, args } => {
                self.check_expr(func);
                // #296/PM3: a `take` parameter consumes its argument regardless of
                // call-site `own`. Look up the callee's take-parameter positions.
                let callee_takes: Option<Vec<bool>> = if let ExprKind::Ident(name) = &func.kind {
                    // `drop` is a compiler builtin, so it has no declaration in
                    // `decls` for the take-parameter scan to find — and without
                    // that, `drop(p)` didn't consume `p`: the leak was reported on
                    // a value that had just been freed, and freeing it twice drew
                    // no error at all (#819). mem.owned/OW3 says it consumes.
                    if name == "drop" {
                        self.check_drop_of_a_field(args);
                        Some(vec![true])
                    } else {
                        self.fn_take_params.get(name).cloned()
                    }
                } else {
                    None
                };
                let callee_deletings: Option<Vec<bool>> = if let ExprKind::Ident(name) = &func.kind {
                    self.fn_deleting_params.get(name).cloned()
                } else {
                    None
                };
                let mut deleting_args: Vec<Expr> = Vec::new();
                let rack_args = self.rack_arg_roots(args);
                for (i, arg) in args.iter().enumerate() {
                    self.check_expr(&arg.expr);
                    let known_mode = callee_takes.as_ref().and_then(|t| t.get(i)).copied();
                    let is_take_param = known_mode.unwrap_or(false);
                    // SL4: a scope-limited closure handed to a *borrow*
                    // parameter doesn't escape. PM6 says a borrowed parameter
                    // can't be given away or stored in an aggregate, so the
                    // only way out of the callee is the return value — and the
                    // limit rides along with it. That's the whole middleware
                    // shape: `logging(handler)` borrows the handler and answers
                    // a closure over it, which lives as long as the handler
                    // does. A `take` parameter is the real escape: the callee
                    // keeps it and the caller can't see where it goes (SL2).
                    self.check_closure_arg_escape(expr.id, &arg.expr, known_mode);
                    if matches!(&func.kind, ExprKind::Ident(n) if n == "spawn") {
                        self.check_spawn_lost_writes(&arg.expr);
                    }
                    // Passing a rack to a `deleting` parameter revokes every link
                    // local into it — but not until the rest of the arguments have
                    // been checked, or a link passed alongside it reads as already
                    // dead at its own call.
                    if callee_deletings
                        .as_ref()
                        .and_then(|d| d.get(i))
                        .copied()
                        .unwrap_or(false)
                    {
                        deleting_args.push(arg.expr.clone());
                    }
                    let callee_name = match &func.kind {
                        ExprKind::Ident(n) => Some(n.clone()),
                        _ => None,
                    };
                    if is_take_param {
                        // LP16: reject passing for-mutate binding to take parameter
                        if let ExprKind::Ident(name) = &arg.expr.kind {
                            if let Some(fm) = self.active_for_mutates.iter().find(|fm| fm.binding_names.contains(name)) {
                                self.errors.push(OwnershipError {
                                    kind: OwnershipErrorKind::ForMutateTakeItem {
                                        item: name.clone(),
                                        collection: fm.collection_name.clone(),
                                        loop_span: fm.span,
                                    },
                                    span: arg.expr.span,
                                });
                            }
                        }
                        self.require_deleting_for_derived_consume(&arg.expr, &rack_args, expr.span);
                        // Inside an `ensure` body the call runs at scope exit,
                        // so the value is still the frame's until then — that
                        // deferral is what `ensure` is. The method form already
                        // said so (`is_take_self_method` below is guarded the
                        // same way); a plain call was not, so `ensure drop(p)`
                        // marked the box moved on the spot and every later read
                        // of `p` was a use-after-move. That is the form
                        // `mem.heap` documents for exactly this, and the only
                        // reason to write it is to go on using the box (#882).
                        if !self.in_ensure {
                            self.consume_arg(&arg.expr, callee_name.as_deref());
                        }
                    }
                }
                for rack_arg in &deleting_args {
                    self.kill_links_for_deleting_arg(rack_arg, expr.span);
                }
            }
            ExprKind::MethodCall { object, method, type_args: _, args } => {
                self.check_expr(object);
                self.note_link_into_container(object, method, args);
                // #296/PM3: consume arguments bound to `take` parameters of user
                // methods. T1: a channel `send` transfers ownership of its value.
                let method_takes: Option<Vec<ParamMode>> = self.method_param_modes(object, method);
                let channel_send = self.is_channel_send(object, method, expr.span);
                let mut rack_args = self.rack_arg_roots(args);
                // For a method call the receiver is an argument too.
                if let Some(root) = Self::extract_root_and_fields(object).0 {
                    if self.name_holds_rack(&root) && !rack_args.contains(&root) {
                        rack_args.push(root);
                    }
                }
                // `deleting` on a method: the receiver carries it as often as a
                // parameter does, and a call has to revoke the caller's links
                // either way.
                let mut method_deleting_args: Vec<Expr> = Vec::new();
                if let Some(recv_ty) = self.receiver_type_name(object) {
                    if let Some((self_deleting, param_deleting)) =
                        self.method_deleting.get(&(recv_ty, method.clone())).cloned()
                    {
                        if self_deleting {
                            method_deleting_args.push((**object).clone());
                        }
                        for (i, arg) in args.iter().enumerate() {
                            if param_deleting.get(i).copied().unwrap_or(false) {
                                method_deleting_args.push(arg.expr.clone());
                            }
                        }
                    }
                }
                for (i, arg) in args.iter().enumerate() {
                    self.check_expr(&arg.expr);
                    // SL2: scope-limited closure passed as method argument
                    let is_take_param = matches!(
                        method_takes.as_ref().and_then(|t| t.get(i)),
                        Some(ParamMode::Take)
                    ) || (channel_send && i == 0);
                    // SL4, as above: only a `take` parameter keeps it.
                    let known_mode = method_takes
                        .as_ref()
                        .and_then(|t| t.get(i))
                        .map(|m| matches!(m, ParamMode::Take) || (channel_send && i == 0));
                    self.check_closure_arg_escape(expr.id, &arg.expr, known_mode);
                    if self.is_task_spawn(object, method) {
                        self.check_spawn_lost_writes(&arg.expr);
                    }
                    if is_take_param {
                        // LP16: reject passing for-mutate binding to take parameter
                        if let ExprKind::Ident(name) = &arg.expr.kind {
                            if let Some(fm) = self.active_for_mutates.iter().find(|fm| fm.binding_names.contains(name)) {
                                self.errors.push(OwnershipError {
                                    kind: OwnershipErrorKind::ForMutateTakeItem {
                                        item: name.clone(),
                                        collection: fm.collection_name.clone(),
                                        loop_span: fm.span,
                                    },
                                    span: arg.expr.span,
                                });
                            }
                        }
                        if method != "delete" {
                            self.require_deleting_for_derived_consume(&arg.expr, &rack_args, expr.span);
                        }
                        self.consume_arg(&arg.expr, Some(method.as_str()));
                    }
                }
                // `List.Cons(1, rest)` — a variant constructor takes its payload by
                // value, so a box handed to one is consumed. After the args have
                // been read, same reason as the struct literal above.
                if self.names_a_variant(object, method) {
                    self.consume_owned_into_aggregate(expr);
                }
                // A delete names its victim only if the argument is a link this
                // body can vouch for: one `insert` handed over, or one a caller
                // passed by `take`. Anything else — a field read, an iteration
                // binding, a call result — may alias any node in the rack, so
                // every *derived* link local has to die with it. And a delete of
                // an unvouched-for link is not a named delete at all: it takes
                // everything, the same way `clear` does.
                if method == "delete" && self.receiver_type_name(object).as_deref() == Some("Rack")
                {
                    let named = args.first().is_some_and(|a| self.is_identified_link(&a.expr));
                    if named {
                        self.kill_derived_links_into_rack(object, expr.span);
                    } else {
                        self.kill_links_into_rack(object, expr.span);
                    }
                    if let Some(a) = args.first() {
                        if let ExprKind::Ident(name) = &a.expr.kind {
                            // The link named here dies with its node (RK5). It
                            // used to die as a side effect of `delete`'s `take
                            // link` parameter consuming it — but a `Link<T>` is
                            // a machine word naming a node, so it copies, and
                            // `consume_arg` rightly leaves a Copy value alone.
                            // Killing it here says what is actually true: the
                            // node is gone, so every name for it is dead.
                            self.bindings
                                .insert(name.clone(), BindingState::Moved { at: expr.span });
                            self.link_delete_spans.insert(a.expr.span);
                        }
                    }
                    self.link_delete_spans.insert(expr.span);
                    if !named {
                        self.require_deleting(object, "`delete` here", expr.span);
                    }
                }
                // `rack.clear()` deletes every node at once. It names no link,
                // so a local link into that rack has to die here the same way
                // it would at an explicit `delete` — otherwise the checkless
                // read the whole model rests on reads freed memory.
                if method == "clear" && self.receiver_type_name(object).as_deref() == Some("Rack")
                {
                    self.kill_links_into_rack(object, expr.span);
                    // A clear is a delete, so the names it kills must report as
                    // freed rather than as moved.
                    self.link_delete_spans.insert(expr.span);
                    self.require_deleting(object, "`clear` here", expr.span);
                }
                for rack_arg in &method_deleting_args {
                    self.kill_links_for_deleting_arg(rack_arg, expr.span);
                }
                // W2: Check structural mutations inside `with` blocks
                if matches!(method.as_str(), "insert" | "remove" | "clear" | "push" | "pop") {
                    if let ExprKind::Ident(coll_name) = &object.kind {
                        for wb in &self.active_with_bindings {
                            if wb.collection_name == *coll_name {
                                // W2: a structural mutation can reallocate the
                                // buffer the binding names.
                                self.errors.push(OwnershipError {
                                    kind: OwnershipErrorKind::WithBlockStructuralMutation {
                                        collection: coll_name.clone(),
                                        operation: method.clone(),
                                        binding_span: wb.span,
                                    },
                                    span: expr.span,
                                });
                                break;
                            }
                        }
                    }
                }
                // LP14: Check structural mutations on collection during `for mutate`
                if matches!(method.as_str(), "insert" | "remove" | "clear" | "push" | "pop" | "drain") {
                    if let ExprKind::Ident(coll_name) = &object.kind {
                        for fm in &self.active_for_mutates {
                            if fm.collection_name == *coll_name {
                                self.errors.push(OwnershipError {
                                    kind: OwnershipErrorKind::ForMutateStructuralMutation {
                                        collection: coll_name.clone(),
                                        operation: method.clone(),
                                        loop_span: fm.span,
                                    },
                                    span: expr.span,
                                });
                                break;
                            }
                        }
                    }
                }
                // If this is a `take self` method, mark the object as moved
                // (skip in ensure bodies — ensure defers execution)
                if !self.in_ensure && self.is_take_self_method(object, method) {
                    match &object.kind {
                        ExprKind::Ident(name) => {
                            let name = name.clone();
                            let sink = method.clone();
                            self.consume_binding(&name, expr.span, Some(&sink));
                        }
                        // `w.conn.close()` — the receiver is a field, so the
                        // binding itself isn't moved (`w.label` stays readable),
                        // but the field it owed is paid. Only the named field:
                        // discharging the whole holder is how `p.a.close()` came
                        // to silently cover `p.b` (#828).
                        ExprKind::Field { .. } => {
                            let (root, path) = Self::extract_root_and_fields(object);
                            if let (Some(root), Some(path)) = (root, path) {
                                self.pay_field_debt(&root, &path);
                            }
                        }
                        _ => {}
                    }
                }
            }
            ExprKind::Field { object, field: _ } => {
                self.check_expr(object);
            }
            ExprKind::DynamicField { object, field_expr } => {
                self.check_expr(object);
                self.check_expr(field_expr);
            }
            ExprKind::OptionalField { object, field: _ } => {
                self.check_expr(object);
            }
            ExprKind::Index { object, index } => {
                self.check_expr(object);
                self.check_expr(index);
                // Index creates an instant borrow for growable types
            }
            ExprKind::StructLit { name: _, fields, spread } => {
                for field in fields {
                    self.check_expr(&field.value);
                    // SL2: scope-limited closure stored in a struct field
                    if let ExprKind::Ident(name) = &field.value.kind {
                        if self.scope_limited_closures.contains_key(name) {
                            self.errors.push(OwnershipError {
                                kind: OwnershipErrorKind::ScopeLimitedClosureEscapes {
                                    name: name.clone(),
                                },
                                span: field.value.span,
                            });
                            self.scope_limited_closures.remove(name);
                        }
                    } else if self.is_borrowing_closure(&field.value)
                        && self.closure_scope_limits.contains_key(&field.value.id)
                    {
                        self.errors.push(OwnershipError {
                            kind: OwnershipErrorKind::ScopeLimitedClosureEscapes {
                                name: "<closure>".to_string(),
                            },
                            span: field.value.span,
                        });
                    }
                }
                if let Some(spread) = spread {
                    self.check_expr(spread);
                }
                // Storing a box or a resource in a field hands ownership to the
                // aggregate. After the reads, not before — marking it moved first
                // turns the very read that moves it into a use-after-move.
                self.consume_owned_into_aggregate(expr);
            }
            ExprKind::Array(elements) => {
                for elem in elements {
                    self.check_expr(elem);
                }
                self.consume_owned_into_aggregate(expr);
            }
            ExprKind::ArrayRepeat { value, count } => {
                self.check_expr(value);
                self.check_expr(count);
            }
            ExprKind::Tuple(elements) => {
                for elem in elements {
                    self.check_expr(elem);
                }
                self.consume_owned_into_aggregate(expr);
            }
            ExprKind::Range { start, end, inclusive: _ } => {
                if let Some(start) = start {
                    self.check_expr(start);
                }
                if let Some(end) = end {
                    self.check_expr(end);
                }
            }
            ExprKind::Closure { params, body, .. } => {
                // CM1: a closure that outlives its frame carries its captures;
                // one that doesn't points at them. Worked out in
                // `collect_escaping_closures`, not written at the literal —
                // there was never a second legal answer for the compiler to be
                // told.
                let carries = self.closure_carries_captures(expr.id);
                // Collect names from closure params (these shadow outer bindings)
                let param_names: HashSet<String> = params.iter().map(|p| p.name.clone()).collect();

                // Scan body for free variables with field projection tracking (F4)
                let mut captures = Vec::new();
                let mut capture_projections: HashMap<String, Option<Vec<String>>> = HashMap::new();
                self.collect_free_vars_with_projections(body, &param_names, &mut captures, &mut capture_projections);

                // Separate resource captures from non-resource captures
                let resource_captures: Vec<String> = captures.iter()
                    .filter(|name| self.resource_bindings.contains(*name))
                    .cloned()
                    .collect();

                // A carrying closure takes a captured resource in; one that
                // points at its captures only borrows it. `mem.closures`' edge
                // case table has always drawn that line — "Resource consumed by
                // closure" on one side, "Resource borrowed; can't escape scope"
                // on the other — and the pass treated both as a move, which is
                // what let a closure consume something it had only borrowed.
                if carries {
                    for name in &resource_captures {
                        self.bindings.insert(name.clone(), BindingState::Moved { at: expr.span });
                    }
                }

                // Non-resource captures move in the same way.
                if carries {
                    for name in &captures {
                        if !resource_captures.contains(name) {
                            if self.bindings.contains_key(name) {
                                // Copy types stay valid in the outer scope (VS1/VS2).
                                //
                                // Through `capture_is_copy`, which also reads a
                                // parameter's declared type. `binding_types` alone
                                // holds only `let`/`mut` bindings, so a captured
                                // *parameter* looked non-Copy and was marked moved:
                                // `v.filter(|c| c != n)` inside a branch then
                                // reported `n` maybe-moved at the next use, while
                                // the identical code with `n` a local was fine
                                // (#768). Same lookup the borrowing path below
                                // already used.
                                //
                                // SL3: a lent parameter is not the frame's to
                                // give. It is the caller's and is still there
                                // when the call returns, so an escaping closure
                                // borrows it and the limit rides the return
                                // (SL4) — which is the whole sequence protocol:
                                // `Vec.filter(self, pred) -> Sequence<T>`
                                // answers a closure over a borrowed receiver.
                                if !self.capture_is_copy(name) && !self.outlives_this_call(name) {
                                    self.bindings.insert(name.clone(), BindingState::Moved { at: expr.span });
                                }
                            }
                        }
                    }
                } else {
                    // A closure that stays in its frame borrows its captures.
                    // Any such closure with non-resource captures is
                    // scope-limited to its creation block —
                    // returning or storing it past that scope would dangle the borrow.
                    // A Copy capture is copied into the closure env (MIR captures
                    // by value), so it can't dangle — only a non-Copy borrow can
                    // outlive its scope. This mirrors the `own`-closure path above,
                    // which already leaves Copy captures in place. Without this a
                    // closure capturing an f64/i64 local was wrongly scope-limited,
                    // so `v.iter().filter(|x| x >= budget).count()` failed SL2 even
                    // though the closure never escapes the expression.
                    let mut scope_limit: Option<u32> = None;
                    let has_escaping_captures = captures.iter().any(|name| {
                        !resource_captures.contains(name)
                            && !self.outlives_this_call(name)
                            && self.bindings.contains_key(name)
                            && !self.capture_is_copy(name)
                    });
                    if has_escaping_captures {
                        scope_limit = Some(self.current_block);
                    }
                    // Tighten further if any capture is itself scope-limited (borrow binding
                    // or persistent borrow): the closure inherits the inner constraint.
                    // Copy captures are skipped — a shared param like `budget: f64` is
                    // modeled as a persistent borrow, but it's copied into the closure,
                    // so it never dangles and must not scope-limit the closure.
                    for name in &captures {
                        if resource_captures.contains(name) { continue; }
                        if self.outlives_this_call(name) { continue; }
                        if self.capture_is_copy(name) { continue; }
                        if let Some(&block_id) = self.borrow_bindings.get(name) {
                            scope_limit = Some(match scope_limit {
                                None => block_id,
                                Some(existing) => existing.max(block_id),
                            });
                        }
                        for borrow in &self.borrows {
                            if borrow.source == *name {
                                if let BorrowScope::Persistent { block_id } = borrow.scope {
                                    scope_limit = Some(match scope_limit {
                                        None => block_id,
                                        Some(existing) => existing.max(block_id),
                                    });
                                }
                            }
                        }
                    }
                    // Shared borrow for non-resource captures (F4: with field projections)
                    for name in &captures {
                        if !resource_captures.contains(name) {
                            if self.bindings.contains_key(name) {
                                let projection = capture_projections.get(name).cloned().flatten();
                                let mut borrow = ActiveBorrow::new(
                                    name.clone(),
                                    BorrowMode::Shared,
                                    BorrowScope::Persistent { block_id: self.current_block },
                                    expr.span,
                                );
                                if let Some(fields) = projection {
                                    borrow = borrow.with_projection(fields);
                                }
                                self.borrows.push(borrow);
                            }
                        }
                    }
                    // SL1: record this closure's scope limit against its own
                    // node, so whoever binds, assigns, returns or passes *this
                    // expression* reads it and nothing else can.
                    if let Some(limit) = scope_limit {
                        self.closure_scope_limits.insert(expr.id, limit);
                    }
                }

                // Check closure body with isolated state
                let saved_bindings = self.bindings.clone();
                let saved_borrows = self.borrows.clone();
                let saved_resources = self.resource_bindings.clone();
                let saved_ensure = self.ensure_registered.clone();

                self.resource_bindings.clear();
                self.ensure_registered.clear();

                // Register closure params as owned
                for p in params {
                    self.bindings.insert(p.name.clone(), BindingState::Owned);
                }

                // And what each one's type is. `|x|` writes no annotation, so
                // the only place that knows is the checker — it typed the
                // closure itself. Without this a closure parameter had no
                // recorded type, so `is_copy` said no and `pair.push(x)` on an
                // `x: i64` read as a move: the next `pair.push(x * 2)` was
                // rejected as a use after move.
                if let Some(Type::Fn { params: param_tys, .. }) =
                    self.program.node_types.get(&expr.id).cloned()
                {
                    for (p, ty) in params.iter().zip(param_tys.iter()) {
                        self.binding_types.insert(p.name.clone(), ty.clone());
                    }
                }

                // Register resource captures in closure's resource set.
                //
                // Only a carrying closure owns one, so only a carrying closure
                // owes its consumption. A borrowing closure has it on loan: the
                // body may read it, the outer scope still owes it, and a
                // consume in the body is an error.
                //
                // Nothing bounds how many times a closure runs, which is why
                // the borrow reading has to be the strict one. `twice(|| {
                // c.close() })` type-checked, and the interpreter's runtime
                // flag caught the second close while native closed the handle
                // twice and carried on (#882).
                let saved_borrowed_captures = std::mem::take(&mut self.borrowed_captures);
                for name in &resource_captures {
                    self.bindings.insert(name.clone(), BindingState::Owned);
                    if carries {
                        self.resource_bindings.insert(name.clone());
                    } else {
                        self.borrowed_captures.insert(name.clone(), expr.span);
                    }
                }
                // Register non-resource captures as owned
                for name in &captures {
                    if !resource_captures.contains(name) {
                        self.bindings.insert(name.clone(), BindingState::Owned);
                    }
                }

                self.check_expr(body);

                // Check resource consumption at closure exit
                self.check_resource_consumption_in_closure(expr.span, "closure");

                // Restore outer scope
                self.bindings = saved_bindings;
                self.borrows = saved_borrows;
                self.resource_bindings = saved_resources;
                self.ensure_registered = saved_ensure;
                self.borrowed_captures = saved_borrowed_captures;

                // A carrying closure took the resource, so the outer scope
                // stops owing it. A borrowing one still owes what it lent.
                if carries {
                    for name in &resource_captures {
                        self.resource_bindings.remove(name);
                    }
                }
            }
            ExprKind::If { cond, then_branch, else_branch, .. } => {
                self.check_expr(cond);
                // OPT19: `if x? as c` reads the payload out of `x`. For a linear
                // payload "read out" can only mean moved — a resource can't be
                // copied — so `c` carries the obligation from here and `x` no
                // longer does. Neither half held before: the binding leaked
                // silently and the optional it came from was never registered at
                // all (#827).
                let present_resource = self.optional_payload_resource(cond);
                let pre_branch = self.bindings.clone();
                self.check_expr(then_branch);
                if let Some(ref binding) = present_resource {
                    self.check_present_binding_consumed(binding, then_branch.span);
                }
                let then_terminal = Self::is_terminal_expr(then_branch);
                if let Some(else_branch) = else_branch {
                    let after_then = self.bindings.clone();
                    self.bindings = pre_branch;
                    self.check_expr(else_branch);
                    let else_terminal = Self::is_terminal_expr(else_branch);
                    if then_terminal && !else_terminal {
                        // then returns — only else state survives
                    } else if else_terminal && !then_terminal {
                        // else returns — only then state survives
                        self.bindings = after_then;
                    } else {
                        self.merge_branch_bindings(&after_then);
                    }
                } else {
                    // No else — the implicit empty branch keeps the pre-branch
                    // state. Merge the then-branch against it so a move or
                    // consumption in the then-branch becomes maybe-moved (#294).
                    let after_then = self.bindings.clone();
                    self.bindings = pre_branch;
                    if !then_terminal {
                        self.merge_branch_bindings(&after_then);
                    }
                }
            }
            ExprKind::IfLet { expr: scrutinee, pattern, then_branch, else_branch, else_binding } => {
                self.check_expr(scrutinee);
                let pre_branch = self.bindings.clone();
                let scrutinee_ty = self.program.node_types.get(&scrutinee.id).cloned();
                self.register_pattern_bindings_typed(pattern, scrutinee_ty.as_ref(), scrutinee.span);
                self.check_expr(then_branch);
                let then_terminal = Self::is_terminal_expr(then_branch);
                if let Some(else_branch) = else_branch {
                    let after_then = self.bindings.clone();
                    self.bindings = pre_branch;
                    self.check_expr(else_branch);
                    let else_terminal = Self::is_terminal_expr(else_branch);
                    if then_terminal && !else_terminal {
                        // then returns — only else state survives
                    } else if else_terminal && !then_terminal {
                        self.bindings = after_then;
                    } else {
                        self.merge_branch_bindings(&after_then);
                    }
                } else {
                    // No else — merge against the implicit empty branch (#294).
                    let after_then = self.bindings.clone();
                    self.bindings = pre_branch;
                    if !then_terminal {
                        self.merge_branch_bindings(&after_then);
                    }
                }
            }
            ExprKind::Block(stmts) => {
                self.check_block(stmts);
            }
            ExprKind::Match { scrutinee, arms } => {
                self.check_expr(scrutinee);
                let scrutinee_ty = self.program.node_types.get(&scrutinee.id).cloned();
                // L5: matching destructures the scrutinee. For a non-Copy
                // owned binding, ownership transfers into the arms — the
                // arm patterns receive the parts. Mark the scrutinee Moved
                // so the function-exit check doesn't ask the caller to also
                // consume it. Borrowed scrutinees stay borrowed.
                if let ExprKind::Ident(name) = &scrutinee.kind {
                    let owned = matches!(self.bindings.get(name), Some(BindingState::Owned));
                    let needs_move = scrutinee_ty
                        .as_ref()
                        .map_or(false, |ty| !self.is_copy(ty));
                    if owned && needs_move {
                        self.bindings.insert(
                            name.clone(),
                            BindingState::Moved { at: scrutinee.span },
                        );
                    }
                }
                // Arms are alternatives, not sequential code: each is checked
                // from the same pre-match state and their end-states join
                // (mem.linear/L1 — every arm must consume). A diverging arm
                // (returns/breaks) contributes nothing to the join.
                let pre_arms = self.bindings.clone();
                let mut merged: Option<HashMap<String, BindingState>> = None;
                for arm in arms {
                    self.bindings = pre_arms.clone();
                    // A pattern's bindings belong to their own arm. Left on the
                    // books they were still owed while the *next* arm was being
                    // checked, which reads as that arm standing in a window it
                    // has nothing to do with.
                    let before_arm = self.resource_bindings.clone();
                    self.register_pattern_bindings_typed(
                        &arm.pattern,
                        scrutinee_ty.as_ref(),
                        scrutinee.span,
                    );
                    if let Some(guard) = &arm.guard {
                        self.check_expr(guard);
                    }
                    self.check_expr(&arm.body);
                    let terminal = Self::is_terminal_expr(&arm.body);
                    self.close_arm_resources(&before_arm, terminal, arm.body.span);
                    if terminal {
                        continue;
                    }
                    let after_arm = self.bindings.clone();
                    merged = Some(match merged {
                        None => after_arm,
                        Some(acc) => {
                            self.bindings = acc;
                            self.merge_branch_bindings(&after_arm);
                            self.bindings.clone()
                        }
                    });
                }
                // All arms diverge → code after is unreachable; keep pre-match.
                self.bindings = merged.unwrap_or(pre_arms);
            }
            ExprKind::Try { expr: inner } | ExprKind::Take { place: inner } => {
                self.check_expr(inner);
                if matches!(expr.kind, ExprKind::Try { .. }) {
                    self.check_try_leaks_a_resource(expr.span);
                }
            }
            ExprKind::Catch { value, ref clause } => {
                self.check_expr(value);
                // The handler runs only on failure, so what it consumes doesn't
                // count against the success path.
                let pre_handler = self.bindings.clone();
                self.check_expr(&clause.body);
                self.bindings = pre_handler;
            }
            ExprKind::IsPresent { expr: inner, binding } => {
                self.check_expr(inner);
                // OPT19: `x? as v` introduces `v` in the then-branch, and the pass
                // was dropping the name entirely. Moves happened to be caught
                // anyway — `consume_arg` marks by name whether or not the name was
                // registered — but the *type* was missing, so anything reasoning
                // about what `v` is saw nothing.
                if let Some(name) = binding {
                    if let Some(ty) = self.program.node_types.get(&inner.id).cloned() {
                        let narrowed = ty.as_option().cloned().unwrap_or(ty);
                        self.bindings.insert(name.clone(), BindingState::Owned);
                        if self.type_is_resource(&narrowed) {
                            self.resource_bindings.insert(name.clone());
                        }
                        self.identified_links.remove(name);
                        if let Some(root) = self.link_root_of_expr(inner) {
                            self.link_rack_root.insert(name.clone(), root);
                        }
                        // `if n.child? as c` is how an edge is followed, so this
                        // is the main place write permission has to carry over.
                        if self.expr_root_is_writable_link(inner) {
                            self.writable_links.insert(name.clone());
                        } else {
                            self.writable_links.remove(name);
                        }
                        self.binding_types.insert(name.clone(), narrowed);
                    }
                }
            }
            ExprKind::Unwrap { expr: inner, .. } => {
                self.check_expr(inner);
            }
            ExprKind::GuardPattern { expr, pattern: _, else_branch } => {
                self.check_expr(expr);
                self.check_expr(else_branch);
            }
            ExprKind::IsPattern { expr, pattern: _ } => {
                self.check_expr(expr);
            }
            ExprKind::NullCoalesce { value, default } => {
                self.check_expr(value);
                self.check_expr(default);
            }
            ExprKind::Cast { expr: inner, ty: _ } => {
                self.check_expr(inner);
            }
            ExprKind::Convert { expr: inner, .. } => {
                self.check_expr(inner);
            }
            ExprKind::UsingBlock { name: _, args, body } => {
                for arg in args {
                    self.check_expr(&arg.expr);
                }
                self.check_block(body);
            }
            ExprKind::WithAs { bindings, body } => {
                let prev_count = self.active_with_bindings.len();
                for binding in bindings {
                    self.check_expr(&binding.source);
                    // W2: Track binding info for structural mutation checking
                    if let ExprKind::Index { object, .. } = &binding.source.kind {
                        if let ExprKind::Ident(coll_name) = &object.kind {
                            self.active_with_bindings.push(WithBindingInfo {
                                collection_name: coll_name.clone(),
                                span: binding.source.span,
                            });
                        }
                    }
                }
                self.check_block(body);
                self.active_with_bindings.truncate(prev_count);
            }
            ExprKind::BlockCall { name: _, body } => {
                self.check_block(body);
            }
            ExprKind::Unsafe { body } => {
                self.check_block(body);
            }
            ExprKind::Comptime { body } => {
                self.check_block(body);
            }
            ExprKind::Loop { body, .. } => {
                self.check_loop_body(body, &[]);
            }
            ExprKind::Assert { condition, message } | ExprKind::Check { condition, message } => {
                self.check_expr(condition);
                if let Some(msg) = message {
                    self.check_expr(msg);
                }
            }
            ExprKind::Select { arms, .. } => {
                for arm in arms {
                    match &arm.kind {
                        rask_ast::expr::SelectArmKind::Recv { channel, .. } => {
                            self.check_expr(channel);
                        }
                        rask_ast::expr::SelectArmKind::Send { channel, value } => {
                            self.check_expr(channel);
                            self.check_expr(value);
                        }
                        rask_ast::expr::SelectArmKind::Default => {}
                    }
                    self.check_expr(&arm.body);
                }
            }
        }
    }

    /// Check if an expression is terminal (always returns/breaks/continues).
    /// Used to determine that code after a branch is unreachable from that branch.
    fn is_terminal_expr(expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::Block(stmts) => Self::is_terminal_block(stmts),
            _ => false,
        }
    }

    fn is_terminal_block(stmts: &[Stmt]) -> bool {
        stmts.last().map_or(false, |s| match &s.kind {
            StmtKind::Return(_) | StmtKind::Break { .. } | StmtKind::Continue(_) => true,
            StmtKind::Expr(e) => Self::is_terminal_expr(e),
            _ => false,
        })
    }

    /// Whether a binding can still be used (not moved/maybe-moved/discarded).
    fn is_available(state: &BindingState) -> bool {
        matches!(state, BindingState::Owned | BindingState::Borrowed { .. })
    }

    /// Extract the move/discard span from an unavailable state.
    fn unavailable_span(state: &BindingState) -> Option<Span> {
        match state {
            BindingState::Moved { at }
            | BindingState::MaybeMoved { at }
            | BindingState::Discarded { at } => Some(*at),
            _ => None,
        }
    }

    /// Join two binding states at a control-flow merge (O3, mem.linear/L1).
    /// A value gone on some incoming paths but live on others becomes
    /// `MaybeMoved`: later use is an error, and a linear value is not
    /// definitely consumed.
    fn join_binding_states(a: &BindingState, b: &BindingState) -> BindingState {
        let a_gone = !Self::is_available(a);
        let b_gone = !Self::is_available(b);
        match (a_gone, b_gone) {
            // Live on both paths — keep the state (borrows already released).
            (false, false) => a.clone(),
            // Gone on both paths. Definitely unavailable, unless one side is
            // only maybe-gone, which keeps the result maybe.
            (true, true) => {
                if matches!(a, BindingState::MaybeMoved { .. })
                    || matches!(b, BindingState::MaybeMoved { .. })
                {
                    let at = Self::unavailable_span(a)
                        .or_else(|| Self::unavailable_span(b))
                        .unwrap_or(Span::new(0, 0));
                    BindingState::MaybeMoved { at }
                } else {
                    a.clone()
                }
            }
            // Gone on exactly one path — maybe-moved.
            _ => {
                let at = if a_gone {
                    Self::unavailable_span(a)
                } else {
                    Self::unavailable_span(b)
                }
                .unwrap_or(Span::new(0, 0));
                BindingState::MaybeMoved { at }
            }
        }
    }

    /// Merge binding states after two branches join (O3, mem.linear/L1).
    /// `other` holds one branch's post-state; `self.bindings` holds the other's.
    /// Only bindings that existed before the branch (present in both maps) are
    /// merged — branch-local bindings are out of scope after the join.
    /// A value moved on either branch becomes maybe-moved, so any later use is
    /// an error and a linear value consumed on only one path is not treated as
    /// consumed.
    fn merge_branch_bindings(&mut self, other: &HashMap<String, BindingState>) {
        for (name, then_state) in other {
            if let Some(else_state) = self.bindings.get(name) {
                let merged = Self::join_binding_states(else_state, then_state);
                self.bindings.insert(name.clone(), merged);
            }
        }
    }

    /// Handle assignment semantics based on Copy status:
    ///
    /// Copy types (VS1/VS2): implicit bitwise copy, source stays valid.
    /// Non-Copy + `let` (is_mutable=true): move, source invalidated.
    /// Non-Copy + `const` (is_mutable=false): block-scoped borrow.

    /// Is this a `Link<T>`? Both spellings: `Link` resolves to a declared stdlib
    /// struct, so the type arrives as `Generic` after resolution and
    /// `UnresolvedGeneric` before it.
    fn is_link_type(&self, ty: &rask_types::Type) -> bool {
        // `Link<T>?` counts too — an edge field is optional, so reading one out
        // yields the optional shape rather than a bare link, and it is still just
        // a pointer being copied.
        if let Some(inner) = ty.as_option() {
            return self.is_link_type(inner);
        }
        match ty {
            rask_types::Type::UnresolvedGeneric { name, .. } => name == "Link",
            rask_types::Type::Generic { base, .. } => {
                self.program.types.type_name(*base) == "Link"
            }
            _ => false,
        }
    }

    /// First type argument of a generic type, as a comparable string. Used to
    /// pair a `Rack<T>` with the `Link<T>` locals pointing into it.
    fn elem_key(&self, ty: &rask_types::Type) -> Option<String> {
        let ty = ty.as_option().unwrap_or(ty);
        let arg = match ty {
            rask_types::Type::UnresolvedGeneric { args, .. } => args.first()?,
            rask_types::Type::Generic { args, .. } => args.first()?,
            _ => return None,
        };
        match arg {
            rask_types::GenericArg::Type(t) => {
                Some(format!("{}", self.program.types.resolve_type_names(t)))
            }
            rask_types::GenericArg::ConstUsize(n) => Some(n.to_string()),
        }
    }

    /// Record whether a new link local names a node nothing else in this body
    /// knows about. Only a direct `rack.insert(...)` qualifies: a field read, an
    /// iteration binding or a call result may be a second name for a node some
    /// other local also names, and a delete of either invalidates both.
    fn record_link_provenance(&mut self, name: &str, ty: &rask_types::Type, init: &Expr) {
        // A container binding takes its links from whatever it is bound to, and
        // loses them when it's bound to something else. Recomputed rather than
        // accumulated, so `v = Vec.new()` after a `v.push(n)` starts clean.
        if !self.is_link_type(ty) {
            match self.link_bearing_root(init) {
                Some(rack) => {
                    self.container_link_rack.insert(name.to_string(), rack);
                }
                None => {
                    self.container_link_rack.remove(name);
                }
            }
            return;
        }
        self.container_link_rack.remove(name);
        let from_insert = matches!(
            &init.kind,
            ExprKind::MethodCall { object, method, .. }
                if method == "insert" && self.receiver_type_name(object).as_deref() == Some("Rack")
        );
        if from_insert {
            self.identified_links.insert(name.to_string());
        } else {
            self.identified_links.remove(name);
        }
        match self.link_root_of_expr(init) {
            Some(root) => {
                self.link_rack_root.insert(name.to_string(), root);
            }
            None => {
                self.link_rack_root.remove(name);
            }
        }
        // Following an edge off a writable link yields another writable link.
        if self.expr_root_is_writable_link(init) {
            self.writable_links.insert(name.to_string());
        } else {
            self.writable_links.remove(name);
        }
    }

    /// A link handed to a container keeps the container alive no longer than its
    /// rack. `v.push(n)`, `m.insert(k, n)`, `v[i] = n` — anything that puts a
    /// link somewhere the container outlives the statement.
    fn note_link_into_container(
        &mut self,
        object: &Expr,
        method: &str,
        args: &[rask_ast::expr::CallArg],
    ) {
        let Some(root) = Self::extract_root_and_fields(object).0 else { return };
        // `clear` throws the links away with everything else, so what's left
        // points at nothing and outlives nothing.
        if method == "clear" && args.is_empty() {
            self.container_link_rack.remove(&root);
            return;
        }
        for arg in args {
            if let Some(rack) = self.link_bearing_root(&arg.expr) {
                self.container_link_rack.insert(root.clone(), rack);
                return;
            }
        }
    }

    /// The rack behind a value that is, or contains, a link. Walks the literal
    /// forms the same way the escape check does, so `[n]` and `(n, 7)` count.
    fn link_bearing_root(&self, expr: &Expr) -> Option<String> {
        match &expr.kind {
            ExprKind::Tuple(elems) | ExprKind::Array(elems) => {
                elems.iter().find_map(|e| self.link_bearing_root(e))
            }
            ExprKind::StructLit { fields, spread, .. } => fields
                .iter()
                .find_map(|f| self.link_bearing_root(&f.value))
                .or_else(|| spread.as_ref().and_then(|sp| self.link_bearing_root(sp))),
            ExprKind::Ident(name) => self
                .container_link_rack
                .get(name)
                .cloned()
                .or_else(|| self.link_carrying_expr_root(expr)),
            _ => self.link_carrying_expr_root(expr),
        }
    }

    /// The rack behind an expression whose *own* type is a link.
    fn link_carrying_expr_root(&self, expr: &Expr) -> Option<String> {
        let ty = self.program.node_types.get(&expr.id)?;
        if !self.is_link_type(ty) {
            return None;
        }
        self.link_root_of_expr(expr)
    }

    /// Whether this expression is reached from a link this body may write through.
    fn expr_root_is_writable_link(&self, expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::Ident(name) => self.writable_links.contains(name),
            ExprKind::Field { object, .. }
            | ExprKind::OptionalField { object, .. }
            | ExprKind::Index { object, .. }
            | ExprKind::MethodCall { object, .. } => self.expr_root_is_writable_link(object),
            ExprKind::IsPresent { expr: inner, .. } | ExprKind::Unwrap { expr: inner, .. } => {
                self.expr_root_is_writable_link(inner)
            }
            _ => false,
        }
    }

    /// A write through a link is permitted by the rack the node lives in.
    ///
    /// `n.value = 5` doesn't change `n` — it changes a node inside a rack, so
    /// the question is whether *that rack* is writable here. This is the rule
    /// `Handle` has always had (`scene.nodes[h].f = x` needs `mutate scene`); a
    /// link just spells the path differently. Asking the rack rather than the
    /// link is what makes a read-only view free: a function taking `s: Rack<T>`
    /// can read every node and write none, with nothing to propagate along edges
    /// and no way to launder a read-only link into a writable one.
    fn check_node_write(&mut self, target: &Expr, span: Span) {
        let Some(root) = Self::extract_root_and_fields(target).0 else { return };
        let is_link = self
            .binding_types
            .get(&root)
            .is_some_and(|ty| self.is_link_type(ty));
        if !is_link && !self.take_link_params.contains(&root) {
            let param_link = self
                .param_type_strings
                .get(&root)
                .is_some_and(|t| t.starts_with("Link<"));
            if !param_link {
                return;
            }
        }
        // A `mutate` link parameter is the callee's licence to write the node —
        // that's the whole point of passing links around instead of racks. The
        // caller proved it may at the call site; here the signature says it will.
        if self.mutate_params.contains_key(&root)
            || self.deleting_params.contains(&root)
            || self.writable_links.contains(&root)
        {
            return;
        }
        match self.link_root_of_expr(target) {
            // The rack has a name here, so the answer is about that name.
            Some(rack) => {
                if !self.writable_racks.contains(&rack) {
                    self.errors.push(OwnershipError {
                        kind: OwnershipErrorKind::NodeWriteNeedsWritableRack {
                            link: root,
                            rack: Some(rack),
                        },
                        span,
                    });
                }
            }
            // The link arrived as a parameter, so its rack isn't named here. One
            // writable rack parameter is unambiguous — that's the one it must
            // belong to, since an edge can only connect co-owned nodes. None means
            // nothing granted this write.
            None => {
                if self.writable_racks.is_empty() {
                    self.errors.push(OwnershipError {
                        kind: OwnershipErrorKind::NodeWriteNeedsWritableRack {
                            link: root,
                            rack: None,
                        },
                        span,
                    });
                }
            }
        }
    }


    /// A link may not outlive the rack it points into.
    ///
    /// Nothing else catches this. The use-after-delete rule tracks deletes, and
    /// no delete happened — the rack just went out of scope and took its nodes
    /// with it. Block-scoped borrowing would have caught it, except a link is
    /// Copy and escapes freely, which is the point of a link. So the escape has
    /// to be checked directly: a link whose rack this body declared can't be
    /// returned, and can't be assigned into a name that outlives that rack.
    ///
    /// A link into a *parameter* rack is fine — the caller owns it, so it
    /// outlives the call. That's the case that has to keep working:
    /// `func first(mutate s: Rack<T>) -> Link<T>` is an ordinary accessor.
    fn check_link_escape(&mut self, expr: &Expr, via: LinkEscape, span: Span) {
        // A link inside an aggregate escapes just as well as a bare one —
        // `return Holder { link: n }` is the same dangle with a wrapper on it.
        match &expr.kind {
            ExprKind::Tuple(elems) | ExprKind::Array(elems) => {
                for e in elems {
                    self.check_link_escape(e, via.clone(), span);
                }
                return;
            }
            ExprKind::StructLit { fields, spread, .. } => {
                for f in fields {
                    self.check_link_escape(&f.value, via.clone(), span);
                }
                if let Some(sp) = spread {
                    self.check_link_escape(sp, via.clone(), span);
                }
                return;
            }
            _ => {}
        }
        // A container that had a link put into it escapes with the link inside
        // it. Asked first, because the container's own type isn't a link and the
        // walk below would stop at that (#941).
        let via_container = match &expr.kind {
            ExprKind::Ident(name) => self.container_link_rack.get(name).cloned(),
            _ => None,
        };
        let carried = via_container.is_some();
        let rack = match via_container {
            Some(rack) => rack,
            None => {
                let Some(ty) = self.program.node_types.get(&expr.id) else { return };
                if !self.is_link_type(ty) {
                    return;
                }
                let Some(rack) = self.link_root_of_expr(expr) else { return };
                rack
            }
        };
        // A parameter rack outlives this body, so nothing can escape it here.
        if self.param_type_strings.contains_key(&rack) {
            return;
        }
        let Some(&rack_block) = self.binding_decl_blocks.get(&rack) else { return };
        let escapes = match &via {
            LinkEscape::Return => true,
            LinkEscape::Assignment { target } => self
                .binding_decl_blocks
                .get(target)
                .is_some_and(|&target_block| rack_block > target_block),
        };
        if !escapes {
            return;
        }
        let link = match &expr.kind {
            ExprKind::Ident(n) => n.clone(),
            _ => rack.clone(),
        };
        self.errors.push(OwnershipError {
            kind: OwnershipErrorKind::LinkOutlivesRack { link, rack, via, carried },
            span,
        });
    }

    /// Which rack a link expression came out of, by root name. Follows the two
    /// ways a link is obtained — from a rack (`g.nodes.insert(…)`,
    /// `g.nodes.nodes()`) and from another link (`n.peer`, or a name that already
    /// has an origin recorded).
    fn link_root_of_expr(&self, expr: &Expr) -> Option<String> {
        match &expr.kind {
            ExprKind::Ident(name) => self.link_rack_root.get(name).cloned(),
            ExprKind::MethodCall { object, method, .. } => {
                if matches!(method.as_str(), "insert" | "nodes" | "links" | "snapshot")
                    && self.receiver_type_name(object).as_deref() == Some("Rack")
                {
                    return Self::extract_root_and_fields(object).0;
                }
                self.link_root_of_expr(object)
            }
            ExprKind::Field { object, .. }
            | ExprKind::OptionalField { object, .. }
            | ExprKind::Index { object, .. } => self.link_root_of_expr(object),
            ExprKind::IsPresent { expr: inner, .. } | ExprKind::Unwrap { expr: inner, .. } => {
                self.link_root_of_expr(inner)
            }
            _ => None,
        }
    }

    /// True if this argument is a link the body can vouch for: an `insert` result,
    /// or a `take` parameter the caller already gave up.
    fn is_identified_link(&self, arg: &Expr) -> bool {
        let ExprKind::Ident(name) = &arg.kind else {
            return false;
        };
        self.identified_links.contains(name) || self.take_link_params.contains(name)
    }

    /// Kill only the *derived* link locals — the ones that may be a second name
    /// for whatever just died. Locals with their own `insert` behind them name
    /// distinct nodes and survive.
    fn kill_derived_links_into_rack(&mut self, rack: &Expr, span: Span) {
        let elem = self
            .program
            .node_types
            .get(&rack.id)
            .and_then(|ty| self.elem_key(ty));
        let dead: Vec<String> = self
            .binding_types
            .iter()
            .filter(|(name, ty)| {
                self.is_link_type(ty)
                    && !self.identified_links.contains(name.as_str())
                    && !self.take_link_params.contains(name.as_str())
                    && matches!(
                        self.bindings.get(name.as_str()),
                        None | Some(BindingState::Owned)
                    )
                    && (elem.is_none() || self.elem_key(ty) == elem)
            })
            .map(|(name, _)| name.clone())
            .collect();
        for name in dead {
            self.bindings.insert(name, BindingState::Moved { at: span });
        }
    }

    /// Element type key if this expression iterates a `Rack`'s nodes — either
    /// `rack.nodes()`/`rack.links()` or the rack itself.
    fn rack_iteration_elem(&self, iter: &Expr) -> Option<Option<String>> {
        if let ExprKind::MethodCall { object, method, .. } = &iter.kind {
            if matches!(method.as_str(), "nodes" | "links")
                && self.receiver_type_name(object).as_deref() == Some("Rack")
            {
                return Some(
                    self.program
                        .node_types
                        .get(&object.id)
                        .and_then(|ty| self.elem_key(ty)),
                );
            }
        }
        if self.receiver_type_name(iter).as_deref() == Some("Rack") {
            return Some(
                self.program
                    .node_types
                    .get(&iter.id)
                    .and_then(|ty| self.elem_key(ty)),
            );
        }
        None
    }

    /// True if this argument names a binding introduced by iterating a rack.
    fn is_rack_iteration_binding(&self, arg: &Expr) -> bool {
        let ExprKind::Ident(name) = &arg.kind else {
            return false;
        };
        self.rack_iterations
            .iter()
            .any(|(_, names)| names.contains(name))
    }

    /// Handing a link to a `take` parameter is handing it to something that may
    /// delete it. If the link is one this body derived — out of an edge, out of
    /// iteration — the caller never named it, so this is an unnamed delete wearing
    /// a call's clothing and needs the same declaration.
    fn require_deleting_for_derived_consume(
        &mut self,
        arg: &Expr,
        rack_args: &[String],
        span: Span,
    ) {
        let ExprKind::Ident(name) = &arg.kind else { return };
        let is_link = match self.binding_types.get(name) {
            Some(ty) => self.is_link_type(ty),
            // Parameters aren't in `binding_types`, so fall back to the declared
            // type name.
            None => self
                .param_type_strings
                .get(name)
                .is_some_and(|t| t.starts_with("Link<")),
        };
        if !is_link || self.is_identified_link(arg) {
            return;
        }
        // Which rack it belongs to isn't recoverable from the link, so this is
        // exact only when the body has one rack-bearing parameter — the ordinary
        // case. With none, there is nothing the caller could be holding.
        // Which rack will the callee delete from? Whichever one this same call
        // hands it — a callee can't delete a link without a rack to delete it
        // from. That makes the blame exact however many racks are in scope, and
        // needs no guess about where the link came from. A call that passes no
        // rack can't delete the caller's node at all.
        for param in rack_args.to_vec() {
            if self.deleting_params.contains(&param) {
                continue;
            }
            self.errors.push(OwnershipError {
                kind: OwnershipErrorKind::UndeclaredDelete {
                    param,
                    operation: format!("handing `{}` to something that consumes it", name),
                },
                span,
            });
        }
    }

    /// An unnamed delete reaches nodes the caller never handed over, so the
    /// parameter it goes through has to say so. A rack this body owns outright is
    /// exempt — the caller has no links into it to lose.
    fn require_deleting(&mut self, rack: &Expr, op: &str, span: Span) {
        let Some(root) = Self::extract_root_and_fields(rack).0 else {
            return;
        };
        if !self.param_type_strings.contains_key(&root) || self.deleting_params.contains(&root) {
            return;
        }
        self.errors.push(OwnershipError {
            kind: OwnershipErrorKind::UndeclaredDelete {
                param: root,
                operation: op.to_string(),
            },
            span,
        });
    }

    /// At a call passing `arg` to a `deleting` parameter, the callee picks which
    /// nodes die, so every link local into that rack dies here.
    fn kill_links_for_deleting_arg(&mut self, arg: &Expr, span: Span) {
        let Some(root) = Self::extract_root_and_fields(arg).0 else {
            return;
        };
        let elem = self
            .binding_types
            .get(&root)
            .and_then(|ty| self.rack_elem_of(ty));
        let dead: Vec<String> = self
            .binding_types
            .iter()
            .filter(|(name, ty)| {
                if !self.is_link_type(ty)
                    || !matches!(
                        self.bindings.get(name.as_str()),
                        None | Some(BindingState::Owned)
                    )
                {
                    return false;
                }
                // A link whose origin is recorded dies only if it came out of
                // *this* rack — two racks of the same node type hand out links
                // of the same type, so the element type alone can't separate them.
                // An unrecorded origin has to die: over-killing is a rejected
                // program, under-killing is a use after free.
                match self.link_rack_root.get(name.as_str()) {
                    Some(origin) => *origin == root,
                    None => elem.is_none() || self.elem_key(ty) == elem,
                }
            })
            .map(|(name, _)| name.clone())
            .collect();
        for name in dead {
            self.bindings.insert(name.clone(), BindingState::Moved { at: span });
        }
        self.link_delete_spans.insert(span);
    }

    /// Element type of a sequence type (`Vec<T>`, `Rack<T>`), if it has one.
    fn sequence_element(ty: &rask_types::Type) -> Option<rask_types::Type> {
        let args = match ty {
            rask_types::Type::Generic { args, .. } => args,
            rask_types::Type::UnresolvedGeneric { args, .. } => args,
            _ => return None,
        };
        match args.first()? {
            rask_types::GenericArg::Type(t) => Some((**t).clone()),
            rask_types::GenericArg::ConstUsize(_) => None,
        }
    }

    /// Root names of the arguments that carry a `Rack`, deduplicated in order.
    fn rack_arg_roots(&self, args: &[rask_ast::expr::CallArg]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for arg in args {
            if let Some(root) = Self::extract_root_and_fields(&arg.expr).0 {
                if self.name_holds_rack(&root) && !out.contains(&root) {
                    out.push(root);
                }
            }
        }
        out
    }

    /// Is this name a parameter whose type carries a `Rack`?
    fn name_holds_rack(&self, name: &str) -> bool {
        self.param_type_strings
            .get(name)
            .is_some_and(|ty| self.type_string_holds_rack(ty))
    }

    /// Does a declared type name hold a `Rack` — directly or in a field?
    fn type_string_holds_rack(&self, ty_str: &str) -> bool {
        let base = ty_str.split('<').next().unwrap_or(ty_str).trim();
        if base == "Rack" {
            return true;
        }
        match self.program.types.get_type_id(base) {
            Some(id) => self.rack_elem_of(&rask_types::Type::Named(id)).is_some(),
            None => false,
        }
    }

    /// Element type of a `Rack<T>`, looking through a struct that holds one.
    fn rack_elem_of(&self, ty: &rask_types::Type) -> Option<String> {
        if self.type_name_of(ty).as_deref() == Some("Rack") {
            return self.elem_key(ty);
        }
        let id = match ty {
            rask_types::Type::Named(id) => *id,
            rask_types::Type::Generic { base, .. } => *base,
            _ => return None,
        };
        let rask_types::TypeDef::Struct { fields, .. } = self.program.types.get(id)? else {
            return None;
        };
        fields
            .clone()
            .into_iter()
            .find_map(|(_, fty)| self.rack_elem_of(&fty))
    }

    fn type_name_of(&self, ty: &rask_types::Type) -> Option<String> {
        match ty {
            rask_types::Type::UnresolvedGeneric { name, .. } => {
                Some(name.split('<').next().unwrap_or(name).to_string())
            }
            rask_types::Type::Generic { base, .. } => {
                let n = self.program.types.type_name(*base);
                Some(n.split('<').next().unwrap_or(&n).to_string())
            }
            rask_types::Type::Named(id) => {
                let n = self.program.types.type_name(*id);
                Some(n.split('<').next().unwrap_or(&n).to_string())
            }
            _ => None,
        }
    }

    /// Mark every live local link with the rack's element type as deleted.
    /// Conservative on the rack: links are not tracked back to the rack they
    /// came from, so two racks of the same node type kill each other's locals.
    fn kill_links_into_rack(&mut self, rack: &Expr, span: Span) {
        let elem = self
            .program
            .node_types
            .get(&rack.id)
            .and_then(|ty| self.elem_key(ty));
        let dead: Vec<String> = self
            .binding_types
            .iter()
            .filter(|(name, ty)| {
                self.is_link_type(ty)
                    && matches!(
                        self.bindings.get(name.as_str()),
                        None | Some(BindingState::Owned)
                    )
                    && (elem.is_none() || self.elem_key(ty) == elem)
            })
            .map(|(name, _)| name.clone())
            .collect();
        for name in dead {
            self.bindings.insert(name, BindingState::Moved { at: span });
        }
    }

    fn handle_assignment(&mut self, expr: &Expr, span: Span, is_mutable: bool) {
        if let Some(ty) = self.program.node_types.get(&expr.id) {
            // Copy types: both source and target remain valid (VS1/VS2). An
            // `Owned` box is never one of them however small its payload — there
            // is one owner, and moving it hands that over (#819).
            let names_an_owned_box = match &expr.kind {
                ExprKind::Ident(name) => self.owned_bindings.contains(name),
                _ => false,
            };
            if self.is_copy(ty) && !names_an_owned_box {
                return;
            }

            // Non-Copy types: whole-variable access moves the source (O3);
            // field/index projections create a borrow (mode depends on is_mutable).
            // F1: Extract root binding and optional field projection
            let (root, projection) = Self::extract_root_and_fields(expr);
            if let Some(source_name) = root {
                if projection.is_some() {
                    // Reading a link out of a field copies a pointer and leaves the
                    // field exactly as it was — so it borrows nothing, the same way
                    // a Copy value above borrows nothing. Without this, the ordinary
                    // graph splice `p.next = n.next` reads as an exclusive borrow of
                    // `n` (correct when the field is owned data being moved out) and
                    // collides with the shared borrow `if n.prev? as p` already
                    // holds (analysis.fourth-option).
                    if self.is_link_type(ty) {
                        return;
                    }
                    // F1: Field-projected — borrow the source
                    let mode = if is_mutable { BorrowMode::Exclusive } else { BorrowMode::Shared };
                    self.create_borrow_with_projection(source_name, mode, span, projection);
                } else {
                    // Whole-variable assignment: move source regardless of const/mut
                    if let Some(state) = self.bindings.get(&source_name) {
                        match state {
                            // A borrowed *parameter* being moved out of is PM6 —
                            // giving away something the caller still owns. Reporting
                            // it as a borrow conflict named the wrong event: nothing
                            // is being mutated in `o.only = next`, and neither the
                            // message nor the fix said `take next` (#818).
                            BindingState::Borrowed { .. }
                                if self.borrowed_params.contains_key(&source_name) =>
                            {
                                self.consume_binding(&source_name, span, None);
                                return;
                            }
                            BindingState::Borrowed { .. } => {
                                self.errors.push(OwnershipError {
                                    kind: OwnershipErrorKind::MutateWhileBorrowed {
                                        name: source_name.clone(),
                                        borrow_span: span,
                                    },
                                    span,
                                });
                                return;
                            }
                            // Already reported. Every caller walks this same
                            // expression with `check_expr` first, and its `Ident`
                            // arm reports the use — at the name rather than at
                            // the whole statement, which is the better underline.
                            // Reporting again here gave one `let z = x` two
                            // identical E0800s at two spans (#1092). There is
                            // still nothing to move.
                            BindingState::Moved { .. } | BindingState::MaybeMoved { .. } => {
                                return;
                            }
                            BindingState::Discarded { at } => {
                                self.errors.push(OwnershipError {
                                    kind: OwnershipErrorKind::UseAfterDiscard {
                                        name: source_name.clone(),
                                        discarded_at: *at,
                                    },
                                    span,
                                });
                                return;
                            }
                            BindingState::Owned => {}
                        }
                    }
                    self.bindings.insert(source_name, BindingState::Moved { at: span });
                }
            }
        }
    }

    /// Does this name hold a value, rather than name a type?
    ///
    /// `Shape.Empty` has the shape of `p.x` and is not a field read at all — it
    /// is a constructor, and there is no source for the binding to be a second
    /// name for. Reading it as a projection rejected `mut out = List.Nil` with
    /// "a field read is a view" and offered `List.Nil.clone()` as the fix, which
    /// means nothing (#1212).
    ///
    /// Every binding and every parameter is registered, so a root that isn't
    /// there is a type, a module, or something else that owns nothing.
    fn names_a_value(&self, root: &str) -> bool {
        self.bindings.contains_key(root)
    }

    /// F1: Extract root binding name and field projection from a field expression.
    /// `state.health` → (Some("state"), Some(["health"]))
    /// `state` → (Some("state"), None)
    /// Complex expressions → (None, None)
    fn extract_root_and_fields(expr: &Expr) -> (Option<String>, Option<Vec<String>>) {
        match &expr.kind {
            ExprKind::Ident(name) => (Some(name.clone()), None),
            ExprKind::Field { object, field } => {
                let (root, fields) = Self::extract_root_and_fields(object);
                if let Some(root) = root {
                    let mut projection = fields.unwrap_or_default();
                    projection.push(field.clone());
                    (Some(root), Some(projection))
                } else {
                    (None, None)
                }
            }
            _ => (None, None),
        }
    }

    /// Container methods that hand the element back where it lives, and the
    /// spelling that hands back a copy instead.
    ///
    /// `m.get(k)` reads the map's value in place — the map still holds it,
    /// which is what makes a plain read cheap. `pop`, `remove` and `take_all`
    /// are deliberately not here: those take the element out, so what comes
    /// back is the caller's.
    const LENDING_METHODS: &'static [(&'static str, &'static str, Option<&'static str>)] = &[
        ("Vec", "get", Some("get_clone")),
        ("Vec", "first", None),
        ("Vec", "last", None),
        ("Map", "get", Some("get_clone")),
    ];

    /// Is this expression a value a container lent out?
    ///
    /// Written as a walk rather than a flag on the type, because the borrow
    /// travels through the shapes a program actually writes around a lookup:
    /// `m.get(k) ?? Vec.new()`, `m.get(k)!`, a local bound to either, an `if`
    /// whose arms both look one up. The value's type says nothing — a
    /// `Vec<i64>` out of `get` and one out of `Vec.new()` are the same type and
    /// different ownership, which is the whole bug.
    fn lent_value(&self, expr: &Expr) -> Option<LentValue> {
        match &expr.kind {
            ExprKind::MethodCall { object, method, args, .. } => {
                let recv = self.program.node_types.get(&object.id)?;
                let head = self.type_name_of(recv)?;
                let (_, _, clone_form) = Self::LENDING_METHODS
                    .iter()
                    .find(|(t, m, _)| *t == head && m == method)?;
                let ty = self.program.node_types.get(&expr.id)?;
                if !self.lendable_payload(ty) {
                    return None;
                }
                let source = Self::render_place(object).unwrap_or_else(|| head.to_lowercase());
                let written = if args.is_empty() { "()" } else { "(…)" };
                Some(LentValue {
                    call: format!("{}.{}{}", source, method, written),
                    holder: source,
                    lender: head,
                    payload_ty: self.lent_payload_display(ty),
                    clone_form: clone_form.map(|c| c.to_string()),
                    span: expr.span,
                })
            }
            // `rows[0]` is the same read `rows.get(0)` is, minus the `T?`.
            // Binding one is already E0871; this is the return.
            ExprKind::Index { object, .. } => {
                let recv = self.program.node_types.get(&object.id)?;
                let head = self.type_name_of(recv)?;
                if !matches!(head.as_str(), "Vec" | "Map") {
                    return None;
                }
                let ty = self.program.node_types.get(&expr.id)?;
                if !self.lendable_payload(ty) {
                    return None;
                }
                let holder = Self::render_place(object).unwrap_or_else(|| head.to_lowercase());
                Some(LentValue {
                    call: format!("{}[…]", holder),
                    holder,
                    lender: head,
                    payload_ty: self.lent_payload_display(ty),
                    clone_form: Some("get_clone".to_string()),
                    span: expr.span,
                })
            }
            // The default side is fresh; the lookup side is not, and one path
            // out of two is enough to make the return ambiguous.
            ExprKind::NullCoalesce { value, .. } => self.lent_value(value),
            ExprKind::Try { expr } => self.lent_value(expr),
            ExprKind::Unwrap { expr, .. } => self.lent_value(expr),
            ExprKind::Ident(name) => self.lent_locals.get(name).cloned(),
            ExprKind::If { then_branch, else_branch, .. } => self
                .lent_value(then_branch)
                .or_else(|| else_branch.as_ref().and_then(|b| self.lent_value(b))),
            ExprKind::Block(stmts) => {
                Self::stmts_tail(stmts).and_then(|e| self.lent_value(e))
            }
            ExprKind::Match { arms, .. } => {
                arms.iter().find_map(|arm| self.lent_value(&arm.body))
            }
            _ => None,
        }
    }

    /// The tail expression of a block's statements, when it has one.
    fn stmts_tail(stmts: &[Stmt]) -> Option<&Expr> {
        match stmts.last().map(|s| &s.kind) {
            Some(StmtKind::Expr(e)) => Some(e),
            Some(StmtKind::Return(Some(e))) => Some(e),
            _ => None,
        }
    }

    /// Is what came back a container the holder still owns?
    ///
    /// A `T?` around it is the usual shape — `get` answers with one — so the
    /// question is about the payload.
    ///
    /// Containers only, deliberately. A `Vec` is one handle onto one buffer and
    /// there is no second owner to be had, so a lookup's result can only be the
    /// holder's. A string or an interface box out of the same lookup is a different
    /// question with a different answer — those carry a count, and the fix for
    /// them is to take a reference on the read (#1035), not to reject the
    /// program.
    fn lendable_payload(&self, ty: &Type) -> bool {
        match ty {
            Type::Result { ok, err } if **err == Type::None => self.lendable_payload(ok),
            other => self
                .type_name_of(other)
                .is_some_and(|n| matches!(n.as_str(), "Vec" | "Map" | "Set")),
        }
    }

    /// `Vec<i64>`, not `Vec`. The name table drops the arguments and `Display`
    /// on a resolved generic prints `<type#7><i64>`, so neither alone is a
    /// type a reader recognises.
    fn lent_payload_display(&self, ty: &Type) -> String {
        match ty {
            Type::Result { ok, err } if **err == Type::None => self.lent_payload_display(ok),
            Type::Generic { base, args } if !args.is_empty() => {
                let head = self.program.types.type_name(*base);
                let inner: Vec<String> = args
                    .iter()
                    .map(|a| match a {
                        rask_types::GenericArg::Type(t) => self.lent_payload_display(t),
                        other => other.to_string(),
                    })
                    .collect();
                format!("{}<{}>", head, inner.join(", "))
            }
            other => self.resource_type_display(other),
        }
    }

    /// Remember whether a binding holds a borrowed value, or forget that it
    /// did. A `mut` rebound to something fresh is no longer lent, and a stale
    /// entry would reject a program that is fine.
    fn record_lent_binding(&mut self, name: &str, init: &Expr) {
        match self.lent_value(init) {
            Some(lent) => {
                self.lent_locals.insert(name.to_string(), lent);
            }
            None => {
                self.lent_locals.remove(name);
            }
        }
    }

    /// `drop(h.inner)` on a field of an aggregate — the aggregate owns it.
    ///
    /// Storing a box in a field moves it in, and the aggregate's release gives
    /// it back when the aggregate dies. A hand-drop of the same field is a
    /// second owner, and the two of them ran in that order: the hand-drop freed
    /// the block and the struct's release freed it again, which glibc reports
    /// as "double free detected in tcache 2".
    ///
    /// Making `drop` quietly do nothing on a field was the other way out, and
    /// it is worse — the author wrote a consume and got none, with nowhere to
    /// learn it, and whether `drop(x)` frees anything would depend on whether
    /// `x` is a binding or a projection. So the shape doesn't compile (#1202).
    ///
    /// Only a field of something this frame can see. A local you own is yours
    /// to take apart — `drop(p)` on a `Heap` binding is exactly what
    /// mem.heap/HP3 asks for.
    fn check_drop_of_a_field(&mut self, args: &[rask_ast::expr::CallArg]) {
        let Some(arg) = args.first() else { return };
        let (Some(root), Some(fields)) = Self::extract_root_and_fields(&arg.expr) else {
            return;
        };
        if fields.is_empty() || !self.names_a_value(&root) {
            return;
        }
        let Some(ty) = self.program.node_types.get(&arg.expr.id).cloned() else { return };
        self.errors.push(OwnershipError {
            kind: OwnershipErrorKind::DropOfAnOwnedField {
                path: format!("{}.{}", root, fields.join(".")),
                root,
                field_ty: self.resource_type_display(&ty),
            },
            span: arg.expr.span,
        });
    }

    /// A value the container still holds, on its way out of the function.
    ///
    /// The signature says the caller owns what comes back, and on the lookup
    /// path it doesn't: two names for one buffer, and whoever frees it second
    /// frees it twice. The compiler's answer today is to free neither, which is
    /// safe and leaks — so the shape is rejected instead and the copy is
    /// written down (#1206).
    ///
    /// This is mem.borrowing/S3, which already says a view can't be stored in a
    /// struct, returned, or sent to another task. Only the return is checked
    /// here; storing a lookup in a field is the same mistake and still gets
    /// through.
    fn check_lent_return(&mut self, expr: &Expr) {
        let Some(lent) = self.lent_value(expr) else { return };
        self.errors.push(OwnershipError {
            kind: OwnershipErrorKind::LentValueEscapes {
                call: lent.call,
                holder: lent.holder,
                lender: lent.lender,
                payload_ty: lent.payload_ty,
                clone_form: lent.clone_form,
                lent_at: lent.span,
            },
            span: expr.span,
        });
    }

    /// S3: a view into a borrowed parameter's field, handed back to the caller.
    ///
    /// A parameter without `take` is on loan (PM1), and a field of it is a view
    /// that lives until the block ends (S1). Returning that view gives the
    /// caller a second name for the field: `return self.value` on a `Vec` hands
    /// back the same buffer, so a `push` through the returned value is a `push`
    /// into the struct — identically on both backends. Nothing said so, and
    /// whoever frees it second frees it twice.
    ///
    /// Only a field of a *borrowed* root: a local you own is yours to take
    /// apart, and a `take` parameter was given to you.
    fn check_borrowed_field_escape(&mut self, expr: &Expr) {
        let (Some(root), Some(fields)) = Self::extract_root_and_fields(expr) else {
            return;
        };
        if fields.is_empty() {
            return; // whole-value return is `consume_binding`'s rule
        }
        let Some(&(declared_at, is_mutate)) = self.borrowed_params.get(&root) else {
            return;
        };
        let Some(ty) = self.program.node_types.get(&expr.id).cloned() else {
            return;
        };
        if !self.definitely_not_copy(&ty) {
            return;
        }
        self.errors.push(OwnershipError {
            kind: OwnershipErrorKind::BorrowedFieldEscapes {
                path: format!("{}.{}", root, fields.join(".")),
                root,
                field_ty: self.resource_type_display(&ty),
                declared_at,
                is_mutate,
            },
            span: expr.span,
        });
    }

    /// LP14: Extract the collection name from a for-loop iterator expression.
    /// `items` → Some("items"), `items.iter()` → Some("items")
    fn extract_iter_collection(iter: &Expr) -> Option<String> {
        match &iter.kind {
            ExprKind::Ident(name) => Some(name.clone()),
            ExprKind::MethodCall { object, .. } => {
                if let ExprKind::Ident(name) = &object.kind {
                    Some(name.clone())
                } else {
                    None
                }
            }
            _ => None,
        }
    }


    fn create_borrow_with_projection(
        &mut self,
        source_name: String,
        mode: BorrowMode,
        span: Span,
        projection: Option<Vec<String>>,
    ) {
        if let Some(state) = self.bindings.get(&source_name) {
            match state {
                BindingState::Owned => {
                    self.bindings.insert(
                        source_name.clone(),
                        BindingState::Borrowed {
                            mode,
                            scope: BorrowScope::Persistent { block_id: self.current_block },
                        },
                    );
                    let mut borrow = ActiveBorrow::new(
                        source_name,
                        mode,
                        BorrowScope::Persistent { block_id: self.current_block },
                        span,
                    );
                    if let Some(fields) = projection {
                        borrow = borrow.with_projection(fields);
                    }
                    self.borrows.push(borrow);
                }
                BindingState::Borrowed { mode: existing_mode, .. } => {
                    // Check if there's an actual conflict considering field-level borrows.
                    // Non-overlapping field borrows on the same binding don't conflict.
                    let has_conflict = self.borrows.iter().any(|b| {
                        b.source == source_name && b.overlaps(&projection) && (
                            *existing_mode == BorrowMode::Exclusive
                            || mode == BorrowMode::Exclusive
                        )
                    });

                    if has_conflict {
                        self.errors.push(OwnershipError {
                            kind: OwnershipErrorKind::BorrowConflict {
                                name: source_name,
                                requested: if mode == BorrowMode::Shared { AccessKind::Read } else { AccessKind::Write },
                                existing: if *existing_mode == BorrowMode::Shared { AccessKind::Read } else { AccessKind::Write },
                                existing_span: span,
                            },
                            span,
                        });
                    } else {
                        let mut borrow = ActiveBorrow::new(
                            source_name,
                            mode,
                            BorrowScope::Persistent { block_id: self.current_block },
                            span,
                        );
                        if let Some(fields) = projection {
                            borrow = borrow.with_projection(fields);
                        }
                        self.borrows.push(borrow);
                    }
                }
                BindingState::Moved { at } => {
                    let reason = self.move_reason_for(&source_name);
                    self.errors.push(OwnershipError {
                        kind: OwnershipErrorKind::UseAfterMove {
                            name: source_name,
                            moved_at: *at,
                            reason,
                        },
                        span,
                    });
                }
                BindingState::MaybeMoved { at } => {
                    let reason = self.move_reason_for(&source_name);
                    self.errors.push(OwnershipError {
                        kind: OwnershipErrorKind::UseAfterMaybeMove {
                            name: source_name,
                            moved_at: *at,
                            reason,
                        },
                        span,
                    });
                }
                BindingState::Discarded { at } => {
                    self.errors.push(OwnershipError {
                        kind: OwnershipErrorKind::UseAfterDiscard {
                            name: source_name,
                            discarded_at: *at,
                        },
                        span,
                    });
                }
            }
        }
    }

    /// Check if a type is Copy (implicit copy on assignment).
    /// Copy-ness of a captured name. Locals carry a resolved `Type`; a bare
    /// parameter only has its type-annotation string, so fall back to that.
    fn capture_is_copy(&self, name: &str) -> bool {
        // A link counts here for the same reason a Copy value does: capturing one
        // copies a pointer into the closure, so it can't dangle and must not
        // scope-limit the closure. Without this, `children.filter(|c| c != n)` —
        // the ordinary "drop this child" idiom — is rejected because `n` reads as
        // a scoped borrow (analysis.fourth-option).
        if let Some(t) = self.binding_types.get(name) {
            return self.is_copy(t) || self.is_link_type(t);
        }
        if let Some(tn) = self.param_type_strings.get(name) {
            if let Some(t) = self.type_from_name(tn) {
                return self.is_copy(&t) || self.is_link_type(&t);
            }
        }
        // A parameter whose annotation didn't resolve: fall back to the spelling,
        // so a `Link<T>` param behaves the same as a resolved one.
        self.param_type_strings
            .get(name)
            .is_some_and(|tn| tn.starts_with("Link<"))
    }

    /// Resolve a simple type-annotation string to a `Type`. Handles primitives,
    /// plain named types, and a generic spelling reduced to its base name;
    /// anything else returns None (treated as non-Copy, the safe default).
    fn type_from_name(&self, name: &str) -> Option<Type> {
        // `Handle<Item>` has to reach `is_copy`, which answers by base name for
        // `Link` and stays conservative for every other container.
        // Returning None here made a captured `n: Handle<Item>` parameter look
        // non-Copy, so an `own` closure marked it moved (#768). The arguments
        // aren't needed — nothing downstream inspects them.
        if let Some(base) = name.split('<').next().filter(|b| *b != name) {
            let base = base.trim();
            if base.is_empty() {
                return None;
            }
            return Some(Type::UnresolvedGeneric {
                name: base.to_string(),
                args: Vec::new(),
            });
        }
        Some(match name {
            "bool" => Type::Bool,
            "char" => Type::Char,
            "string" => Type::String,
            "i8" => Type::I8,
            "i16" => Type::I16,
            "i32" => Type::I32,
            "i64" => Type::I64,
            "i128" => Type::I128,
            "u8" => Type::U8,
            "u16" => Type::U16,
            "u32" => Type::U32,
            "u64" => Type::U64,
            "u128" => Type::U128,
            "f32" => Type::F32,
            "f64" => Type::F64,
            "int" => Type::I64,
            "isize" => Type::isize_ty(),
            "uint" => Type::U64,
            "usize" => Type::usize_ty(),
            _ => return self.program.types.get_type_id(name).map(Type::Named),
        })
    }

    /// Compiler-native generic containers whose layout lives in the runtime
    /// rather than in a visible struct decl (an empty `struct Vec<T> { }`
    /// stub) — field-based size/Copy inference can't see them, so they're
    /// named explicitly instead.
    fn is_native_opaque_generic(base_name: &str) -> bool {
        matches!(base_name,
            "Vec" | "Map" | "Wide" | "Cell"
            | "Rack" | "Link"
            | "TaskHandle" | "TaskGroup" | "Sender" | "Receiver" | "ThreadHandle")
    }

    /// Map a generic struct/enum's own type parameter names to the concrete
    /// types plugged in at this instantiation (`Wrapping<u32>`'s `T` -> `u32`).
    /// Const-generic args have nothing to bind to a type parameter and are
    /// skipped.
    fn generic_field_subst(type_params: &[String], args: &[rask_types::GenericArg]) -> std::collections::HashMap<String, Type> {
        type_params.iter().zip(args.iter()).filter_map(|(name, arg)| match arg {
            rask_types::GenericArg::Type(t) => Some((name.clone(), (**t).clone())),
            rask_types::GenericArg::ConstUsize(_) => None,
        }).collect()
    }

    /// Replace a struct/enum field's type parameter with the concrete type
    /// from `subst`, recursing through the same compound shapes
    /// `substitute_type_params` (rask-types) handles for method signatures —
    /// duplicated here rather than shared because that one is
    /// checker-internal (`pub(super)`).
    fn substitute_generic_field(ty: &Type, subst: &std::collections::HashMap<String, Type>) -> Type {
        match ty {
            Type::UnresolvedNamed(name) => subst.get(name).cloned().unwrap_or_else(|| ty.clone()),
            Type::Array { elem, len } => Type::Array {
                elem: Box::new(Self::substitute_generic_field(elem, subst)),
                len: *len,
            },
            Type::Tuple(elems) => Type::Tuple(elems.iter().map(|e| Self::substitute_generic_field(e, subst)).collect()),
            ty if ty.is_option() => Type::option(Self::substitute_generic_field(ty.as_option().unwrap(), subst)),
            Type::Generic { base, args } => Type::Generic {
                base: *base,
                args: args.iter().map(|a| match a {
                    rask_types::GenericArg::Type(t) => rask_types::GenericArg::Type(Box::new(Self::substitute_generic_field(t, subst))),
                    other => other.clone(),
                }).collect(),
            },
            _ => ty.clone(),
        }
    }

    fn is_copy(&self, ty: &Type) -> bool {
        // L1: a linear value is never Copy, whatever its size or its fields.
        // `@resource struct Conn { id: i64 }` is eight bytes of Copy field, so
        // this said Copy — and `consume_arg` skips a Copy argument, so passing a
        // connection to a `take` parameter consumed nothing and the caller was
        // then told it had leaked the value it had just handed away.
        if self.program.types.is_linear_value(ty) {
            return false;
        }
        match ty {
            // Primitives are always Copy
            Type::Unit | Type::None | Type::Bool | Type::Char => true,
            Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128 => true,
            Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128 => true,
            Type::F32 | Type::F64 => true,
            Type::Never => true,

            // String is Copy (immutable, refcounted, 16 bytes — std.strings/S1)
            Type::String => true,

            // Arrays: Copy if element is Copy and size <= 16 bytes
            Type::Array { elem, len: _ } => {
                self.is_copy(elem) && self.type_size(ty) <= 16
            }

            // Tuples: Copy if all elements are Copy and size <= 16 bytes
            Type::Tuple(elems) => {
                elems.iter().all(|t| self.is_copy(t)) && self.type_size(ty) <= 16
            }

            // Option (T or none): Copy if inner is Copy and size <= 16 bytes
            ty if ty.is_option() => {
                let inner = ty.as_option().unwrap();
                self.is_copy(inner) && self.type_size(ty) <= 16
            }

            // Result: NOT Copy (usually contains error info)
            Type::Result { .. } => false,

            // Union: NOT Copy (error union types)
            Type::Union(_) => false,

            // User-defined types: need to check size and fields
            Type::Named(type_id) => {
                if let Some(def) = self.program.types.get(*type_id) {
                    match def {
                        rask_types::TypeDef::Struct { fields, is_unique, .. } => {
                            // U1: @unique disables implicit copy regardless of size
                            if *is_unique { return false; }
                            fields.iter().all(|(_, t)| self.is_copy(t))
                                && self.type_size(ty) <= 16
                        }
                        rask_types::TypeDef::Enum { variants, .. } => {
                            variants.iter().all(|(_, data)| data.iter().all(|t| self.is_copy(t)))
                                && self.type_size(ty) <= 16
                        }
                        // A primitive is always Copy; it never reaches here as
                        // a `Named` anyway.
                        rask_types::TypeDef::Primitive { .. } => true,
                        rask_types::TypeDef::Interface { .. } => false,
                        rask_types::TypeDef::Union { fields, .. } => {
                            fields.iter().all(|(_, t)| self.is_copy(t))
                                && self.type_size(ty) <= 16
                        }
                        rask_types::TypeDef::NominalAlias { underlying, .. } => {
                            self.is_copy(underlying)
                        }
                    }
                } else {
                    false
                }
            }

            // A `Link` is Copy: a machine word naming a node,
            // whose whole point is to be duplicated freely (mem.racks). For
            // `Link<T>` the rack spec says so from the other side — RK5 has
            // using one after its node is deleted reported "as a use after free
            // rather than as a move", which only reads as a rule if links copy.
            // Without it, `v.push(link)` consumed the name and a later
            // `rack.delete(link)` drew a bogus use-after-move.
            //
            // The other compiler-native generics (Vec, Map, Rack, ...) have no
            // fields visible to the type system — their layout lives in the
            // runtime, not in a struct decl — so field-based inference can't
            // see them and they stay hardcoded move-only.
            //
            // A user-defined generic struct (`struct Wrapping<T> { value: T }`)
            // *does* have real fields, so its Copy-ness depends on what T ends
            // up being at this instantiation — same rule as a non-generic
            // struct, just substituted first (W4).
            Type::Generic { base, args } => {
                let base_name = self.program.types.type_name(*base);
                if Self::is_native_opaque_generic(&base_name) {
                    base_name.as_str() == "Link"
                } else if let Some(def) = self.program.types.get(*base) {
                    match def {
                        rask_types::TypeDef::Struct { type_params, fields, is_unique, .. } => {
                            if *is_unique { return false; }
                            let subst = Self::generic_field_subst(type_params, args);
                            fields.iter().all(|(_, t)| self.is_copy(&Self::substitute_generic_field(t, &subst)))
                                && self.type_size(ty) <= 16
                        }
                        rask_types::TypeDef::Enum { type_params, variants, .. } => {
                            let subst = Self::generic_field_subst(type_params, args);
                            variants.iter().all(|(_, data)| data.iter().all(|t| self.is_copy(&Self::substitute_generic_field(t, &subst))))
                                && self.type_size(ty) <= 16
                        }
                        // A primitive is always Copy; it never reaches here as
                        // a `Named` anyway.
                        rask_types::TypeDef::Primitive { .. } => true,
                        rask_types::TypeDef::Interface { .. } => false,
                        // Unions aren't generic (no type_params to substitute) —
                        // reaching this arm through a `Type::Generic` would mean
                        // a union name got parsed with type arguments, which
                        // shouldn't happen.
                        rask_types::TypeDef::Union { .. } => false,
                        rask_types::TypeDef::NominalAlias { underlying, .. } => {
                            self.is_copy(underlying)
                        }
                    }
                } else {
                    false
                }
            }

            // Function types are Copy (just a pointer)
            Type::Fn { .. } => true,

            // Type variables: conservative
            Type::Var(_) => false,

            // AT6: a projection is read off a conformance during type
            // checking, so one reaching here never resolved. Conservative,
            // same as a type variable.
            Type::Assoc { .. } => false,

            // Raw pointers are always Copy (just an address)
            Type::RawPtr(_) => true,

            // SIMD vectors: NOT Copy (large, stack-allocated)
            Type::SimdVector { .. } => false,

            // Unresolved types: conservative, except `Link`,
            // which are Copy regardless of how the name was spelled — same
            // three as the resolved `Type::Generic` arm above.
            Type::UnresolvedGeneric { name, .. } => {
                name.as_str() == "Link"
            }
            Type::UnresolvedNamed(_) => false,

            // Interface objects: never Copy (TR11 — owns heap data)
            Type::InterfaceObject { .. } => false,

            // Error: don't report more errors
            Type::Error => true,
        }
    }

    /// Estimate type size in bytes (simplified).
    fn type_size(&self, ty: &Type) -> usize {
        match ty {
            Type::Unit | Type::None => 0,
            Type::Bool | Type::I8 | Type::U8 => 1,
            Type::I16 | Type::U16 => 2,
            Type::I32 | Type::U32 | Type::F32 | Type::Char => 4,
            Type::I64 | Type::U64 | Type::F64 => 8,
            // Two words, and the only scalar that is. Falling through to the
            // 8-byte default made `struct Wide { a: i128, b: i64 }` measure 16
            // instead of 24, so it sat on the Copy threshold instead of over it
            // and two bindings aliased one value with nothing said (#936).
            Type::I128 | Type::U128 => 16,
            Type::Tuple(elems) => elems.iter().map(|t| self.type_size(t)).sum(),
            Type::Array { elem, len } => self.type_size(elem) * len,
            ty if ty.is_option() => self.type_size(ty.as_option().unwrap()) + 1, // tag byte
            Type::Named(type_id) => {
                if let Some(def) = self.program.types.get(*type_id) {
                    match def {
                        rask_types::TypeDef::Struct { fields, .. } => {
                            fields.iter().map(|(_, t)| self.type_size(t)).sum()
                        }
                        rask_types::TypeDef::Enum { variants, .. } => {
                            let max_variant = variants
                                .iter()
                                .map(|(_, data)| data.iter().map(|t| self.type_size(t)).sum::<usize>())
                                .max()
                                .unwrap_or(0);
                            max_variant + 1
                        }
                        _ => 8,
                    }
                } else {
                    8
                }
            }
            // A user-defined generic struct/enum is sized the same way as a
            // non-generic one, once its own type parameter is substituted
            // with the type argument at this instantiation (`Wrapping<u8>` is
            // one byte, not whatever the unsubstituted `T` would default to).
            // The compiler-native generics (Vec, Map, ...) declare an empty
            // field list — their real layout lives in the runtime, not in the
            // struct decl — so summing fields would say 0 instead of their
            // actual size. Keep them at the old flat 8-byte guess rather than
            // let an empty sum silently answer 0.
            Type::Generic { base, args } if !Self::is_native_opaque_generic(&self.program.types.type_name(*base)) => {
                if let Some(def) = self.program.types.get(*base) {
                    match def {
                        rask_types::TypeDef::Struct { type_params, fields, .. } => {
                            let subst = Self::generic_field_subst(type_params, args);
                            fields.iter().map(|(_, t)| self.type_size(&Self::substitute_generic_field(t, &subst))).sum()
                        }
                        rask_types::TypeDef::Enum { type_params, variants, .. } => {
                            let subst = Self::generic_field_subst(type_params, args);
                            let max_variant = variants
                                .iter()
                                .map(|(_, data)| data.iter().map(|t| self.type_size(&Self::substitute_generic_field(t, &subst))).sum::<usize>())
                                .max()
                                .unwrap_or(0);
                            max_variant + 1
                        }
                        _ => 8,
                    }
                } else {
                    8
                }
            }
            // Strings, closures and interface objects: fat pointer
            Type::String | Type::Fn { .. } | Type::InterfaceObject { .. } => 16,
            _ => 8,
        }
    }

    /// Determine why a type is move-only (not Copy).
    /// `move_reason`, but told where the invalidation happened. A link is only
    /// reported as deleted if that span really was a `delete` — an ordinary move
    /// into another name is a move, and saying "deleted here" about a `let` that
    /// deleted nothing is worse than saying nothing at all.
    fn move_reason_at(&self, ty: &Type, at: Span) -> MoveReason {
        if self.is_link_type(ty) && !self.link_delete_spans.contains(&at) {
            return MoveReason::LinkMoved;
        }
        self.move_reason(ty)
    }

    fn move_reason_for_at(&self, name: &str, at: Span) -> MoveReason {
        match self.binding_types.get(name) {
            Some(ty) => self.move_reason_at(ty, at),
            None => MoveReason::Unknown,
        }
    }

    fn move_reason(&self, ty: &Type) -> MoveReason {
        // A link isn't moved anywhere — `delete` frees its node, which kills every
        // name for it. Report it as what it is rather than as a transfer.
        if self.is_link_type(ty) {
            return MoveReason::LinkDeleted;
        }
        let type_name = format!("{}", self.program.types.resolve_type_names(ty));
        match ty {
            // String is Copy (S1) — this branch shouldn't be reached
            Type::String => MoveReason::Unknown,
            Type::Generic { base, args } => {
                let base_name = self.program.types.type_name(*base);
                // The compiler-native generics have no fields to blame — same
                // gap `is_copy` has to work around for the same reason.
                if Self::is_native_opaque_generic(&base_name) {
                    if matches!(base_name.as_str(), "Vec" | "Map") {
                        MoveReason::OwnsHeapMemory { type_name }
                    } else {
                        MoveReason::Unknown
                    }
                } else if let Some(def) = self.program.types.get(*base) {
                    match def {
                        rask_types::TypeDef::Struct { type_params, fields, is_unique, .. } => {
                            if *is_unique {
                                return MoveReason::Unique { type_name };
                            }
                            let subst = Self::generic_field_subst(type_params, args);
                            let all_fields_copy = fields.iter()
                                .all(|(_, t)| self.is_copy(&Self::substitute_generic_field(t, &subst)));
                            if all_fields_copy {
                                MoveReason::SizeExceedsThreshold { type_name, size: self.type_size(ty) }
                            } else {
                                MoveReason::OwnsHeapMemory { type_name }
                            }
                        }
                        rask_types::TypeDef::Enum { type_params, variants, .. } => {
                            let subst = Self::generic_field_subst(type_params, args);
                            let all_copy = variants.iter().all(|(_, data)| data.iter()
                                .all(|t| self.is_copy(&Self::substitute_generic_field(t, &subst))));
                            if all_copy {
                                MoveReason::SizeExceedsThreshold { type_name, size: self.type_size(ty) }
                            } else {
                                MoveReason::OwnsHeapMemory { type_name }
                            }
                        }
                        _ => MoveReason::Unknown,
                    }
                } else {
                    MoveReason::Unknown
                }
            }
            Type::Named(type_id) => {
                if let Some(def) = self.program.types.get(*type_id) {
                    match def {
                        rask_types::TypeDef::Struct { fields, is_unique, .. } => {
                            // U1: @unique types report as Unique, not size/heap
                            if *is_unique {
                                return MoveReason::Unique { type_name };
                            }
                            let all_fields_copy = fields.iter().all(|(_, t)| self.is_copy(t));
                            if all_fields_copy {
                                MoveReason::SizeExceedsThreshold { type_name, size: self.type_size(ty) }
                            } else {
                                MoveReason::OwnsHeapMemory { type_name }
                            }
                        }
                        rask_types::TypeDef::Enum { variants, .. } => {
                            let all_copy = variants.iter().all(|(_, data)| data.iter().all(|t| self.is_copy(t)));
                            if all_copy {
                                MoveReason::SizeExceedsThreshold { type_name, size: self.type_size(ty) }
                            } else {
                                MoveReason::OwnsHeapMemory { type_name }
                            }
                        }
                        _ => MoveReason::Unknown,
                    }
                } else {
                    MoveReason::Unknown
                }
            }
            Type::Result { .. } | Type::Union(_) => MoveReason::OwnsHeapMemory { type_name },
            _ => {
                let size = self.type_size(ty);
                if size > 16 {
                    MoveReason::SizeExceedsThreshold { type_name, size }
                } else {
                    MoveReason::Unknown
                }
            }
        }
    }


    /// Look up the move reason for a binding by name.
    fn move_reason_for(&self, name: &str) -> MoveReason {
        // Checked before the type: an `Owned<Big>` binding reads as a `Big`, so
        // the type alone would blame the copy threshold and suggest `.clone()` —
        // which for an already-dropped box is the wrong advice entirely.
        if self.owned_bindings.contains(name) {
            return MoveReason::Owned;
        }
        if let Some(ty) = self.binding_types.get(name) {
            self.move_reason(ty)
        } else {
            MoveReason::Unknown
        }
    }

    /// Release instant borrows that end at the given statement.
    fn release_instant_borrows(&mut self, stmt_id: u32) {
        self.borrows.retain(|b| {
            !matches!(b.scope, BorrowScope::Instant { stmt_id: id } if id == stmt_id)
        });
    }

    /// Release persistent borrows that end at the given block.
    fn release_persistent_borrows(&mut self, block_id: u32) {
        let mut released_bindings = HashSet::new();

        // Remove borrows for this block
        self.borrows.retain(|b| {
            if matches!(b.scope, BorrowScope::Persistent { block_id: id } if id == block_id) {
                released_bindings.insert(b.source.clone());
                false
            } else {
                true
            }
        });

        // Restore bindings to Owned if no borrows remain
        for binding_name in released_bindings {
            let remaining_borrows = self.borrows.iter().filter(|b| b.source == binding_name).count();

            if remaining_borrows == 0 {
                if let Some(state) = self.bindings.get(&binding_name) {
                    if matches!(state, BindingState::Borrowed { .. }) {
                        self.bindings.insert(binding_name.clone(), BindingState::Owned);
                    }
                }
            }
        }
    }

    /// Resolve a scrutinee `Type` to a struct's `(name, type)` field list, if
    /// the type names a struct (or a `Generic` whose base is a struct).
    fn struct_fields_for_type(&self, ty: &Type) -> Option<Vec<(String, Type)>> {
        let id = match ty {
            Type::Named(id) => *id,
            Type::Generic { base, .. } => *base,
            Type::UnresolvedNamed(name) | Type::UnresolvedGeneric { name, .. } => {
                let base = name.split('<').next().unwrap_or(name);
                self.program.types.get_type_id(base)?
            }
            _ => return None,
        };
        match self.program.types.get(id)? {
            rask_types::TypeDef::Struct { fields, .. } => Some(fields.clone()),
            _ => None,
        }
    }

    /// Look up struct fields by struct name. Used when a struct pattern names
    /// the struct directly but the scrutinee type is unresolved.
    fn struct_fields_by_name(&self, name: &str) -> Option<Vec<(String, Type)>> {
        let id = self.program.types.get_type_id(name)?;
        match self.program.types.get(id)? {
            rask_types::TypeDef::Struct { fields, .. } => Some(fields.clone()),
            _ => None,
        }
    }

    /// Find a variant's payload types in the enum that `scrutinee_ty` points to,
    /// or — when scrutinee is a `Result { ok, err }` — search inside `ok` then
    /// `err`. The constructor name may be qualified (`FileError.ReadFailed`)
    /// or bare (`ReadFailed`); the qualified prefix is honored when present.
    fn variant_payload_for(&self, scrutinee_ty: &Type, ctor: &str) -> Option<Vec<Type>> {
        let (enum_name, variant_name) = match ctor.split_once('.') {
            Some((e, v)) => (Some(e.to_string()), v.to_string()),
            None => (None, ctor.to_string()),
        };

        // Qualified: jump straight to the named enum.
        if let Some(name) = &enum_name {
            return self.variant_payload_by_enum(name, &variant_name);
        }

        match scrutinee_ty {
            Type::Named(id) => self.variant_payload_in_def(*id, &variant_name),
            Type::Generic { base, .. } => self.variant_payload_in_def(*base, &variant_name),
            Type::Result { ok, err } => self
                .variant_payload_for(ok, &variant_name)
                .or_else(|| self.variant_payload_for(err, &variant_name)),
            Type::Union(variants) => variants
                .iter()
                .find_map(|v| self.variant_payload_for(v, &variant_name)),
            Type::UnresolvedNamed(name) | Type::UnresolvedGeneric { name, .. } => {
                let base = name.split('<').next().unwrap_or(name);
                let id = self.program.types.get_type_id(base)?;
                self.variant_payload_in_def(id, &variant_name)
            }
            _ => None,
        }
    }

    fn variant_payload_in_def(&self, id: rask_types::TypeId, variant: &str) -> Option<Vec<Type>> {
        match self.program.types.get(id)? {
            rask_types::TypeDef::Enum { variants, .. } => variants
                .iter()
                .find(|(n, _)| n == variant)
                .map(|(_, ts)| ts.clone()),
            _ => None,
        }
    }

    fn variant_payload_by_enum(&self, enum_name: &str, variant: &str) -> Option<Vec<Type>> {
        let id = self.program.types.get_type_id(enum_name)?;
        self.variant_payload_in_def(id, variant)
    }

    /// Search every registered enum for a variant by bare name. Used as a
    /// fallback when the scrutinee type is unavailable.
    fn variant_payload_by_name(&self, ctor: &str) -> Option<Vec<Type>> {
        if let Some((e, v)) = ctor.split_once('.') {
            return self.variant_payload_by_enum(e, v);
        }
        for def in self.program.types.iter() {
            if let rask_types::TypeDef::Enum { variants, .. } = def {
                if let Some((_, ts)) = variants.iter().find(|(n, _)| n == ctor) {
                    return Some(ts.clone());
                }
            }
        }
        None
    }

    /// Register pattern bindings, walking with scrutinee type info so a linear
    /// position's `_` becomes ER43 and a binding at a linear position is added
    /// to `resource_bindings`. The `pattern_span` is the scrutinee/match-arm
    /// span used for diagnostics. `scrutinee_ty: None` skips ER42/ER43 checks
    /// at the top level — callers without a known type pass None.
    fn register_pattern_bindings_typed(
        &mut self,
        pattern: &Pattern,
        scrutinee_ty: Option<&Type>,
        pattern_span: Span,
    ) {
        match pattern {
            Pattern::Wildcard => {
                if let Some(ty) = scrutinee_ty {
                    if self.type_is_resource(ty) && !self.pattern_payload_is_borrowed(ty) {
                        self.errors.push(OwnershipError {
                            kind: OwnershipErrorKind::LinearWildcardDiscard {
                                position: error::LinearDiscardPosition::Scrutinee,
                                type_name: format!(
                                    "{}",
                                    self.program.types.resolve_type_names(ty)
                                ),
                            },
                            span: pattern_span,
                        });
                    }
                }
            }
            Pattern::Ident(name) => {
                // Qualified path "Enum.Variant" without parens is a constructor
                // pattern with zero fields, not a binding (parser rule).
                if name.contains('.') {
                    return;
                }
                self.bindings.insert(name.clone(), BindingState::Owned);
                if let Some(ty) = scrutinee_ty {
                    self.binding_types.insert(name.clone(), ty.clone());
                    if self.type_is_resource(ty) && !self.pattern_payload_is_borrowed(ty) {
                        self.resource_bindings.insert(name.clone());
                    }
                }
            }
            Pattern::Literal(_) | Pattern::Range { .. } => {
                // Literal/range matches don't bind. Linear values are not
                // comparable, so this position must be primitive — nothing to do.
            }
            Pattern::Tuple(pats) => {
                let elem_tys = scrutinee_ty.and_then(|ty| match ty {
                    Type::Tuple(elems) => Some(elems.clone()),
                    _ => None,
                });
                for (i, pat) in pats.iter().enumerate() {
                    let pos_ty = elem_tys.as_ref().and_then(|tys| tys.get(i));
                    self.register_pattern_bindings_typed(pat, pos_ty, pattern_span);
                }
            }
            Pattern::Struct { name, fields, rest } => {
                let struct_fields = scrutinee_ty
                    .and_then(|ty| self.struct_fields_for_type(ty))
                    .or_else(|| self.struct_fields_by_name(name));
                for (field_name, pat) in fields {
                    let pos_ty = struct_fields
                        .as_ref()
                        .and_then(|fs| fs.iter().find(|(n, _)| n == field_name))
                        .map(|(_, t)| t.clone());
                    self.register_pattern_bindings_typed(pat, pos_ty.as_ref(), pattern_span);
                }
                // ER43: `..` rest discards every unmentioned linear field.
                if *rest {
                    if let Some(struct_fields) = struct_fields {
                        let mentioned: std::collections::HashSet<&str> =
                            fields.iter().map(|(n, _)| n.as_str()).collect();
                        for (fname, fty) in &struct_fields {
                            if !mentioned.contains(fname.as_str())
                                && self.type_is_resource(fty)
                                && !self.pattern_payload_is_borrowed(fty)
                            {
                                self.errors.push(OwnershipError {
                                    kind: OwnershipErrorKind::LinearWildcardDiscard {
                                        position: error::LinearDiscardPosition::Field {
                                            constructor: name.clone(),
                                            field: Some(fname.clone()),
                                            index: None,
                                        },
                                        type_name: format!(
                                            "{}",
                                            self.program.types.resolve_type_names(fty)
                                        ),
                                    },
                                    span: pattern_span,
                                });
                            }
                        }
                    }
                }
            }
            Pattern::Constructor { name, fields } => {
                let payload_tys = scrutinee_ty
                    .and_then(|ty| self.variant_payload_for(ty, name))
                    .or_else(|| self.variant_payload_by_name(name));
                for (i, pat) in fields.iter().enumerate() {
                    let pos_ty = payload_tys.as_ref().and_then(|tys| tys.get(i));
                    if let Pattern::Wildcard = pat {
                        if let Some(ty) = pos_ty {
                            if self.type_is_resource(ty)
                                && !self.pattern_payload_is_borrowed(ty)
                            {
                                self.errors.push(OwnershipError {
                                    kind: OwnershipErrorKind::LinearWildcardDiscard {
                                        position: error::LinearDiscardPosition::Field {
                                            constructor: name.clone(),
                                            field: None,
                                            index: Some(i),
                                        },
                                        type_name: format!(
                                            "{}",
                                            self.program.types.resolve_type_names(ty)
                                        ),
                                    },
                                    span: pattern_span,
                                });
                            }
                        }
                        continue;
                    }
                    self.register_pattern_bindings_typed(pat, pos_ty, pattern_span);
                }
            }
            Pattern::Or(pats) => {
                // Each alternative binds the same names; let the typed walk
                // mark resources on the first, then de-dup with the rest.
                for pat in pats {
                    self.register_pattern_bindings_typed(pat, scrutinee_ty, pattern_span);
                }
            }
            Pattern::TypePat { ty_name, binding } => {
                if let Some(name) = binding {
                    self.bindings.insert(name.clone(), BindingState::Owned);
                    // Resolve the narrowed type to determine linearity. Strip
                    // generic args ("FileError<T>" → "FileError") for lookup.
                    let base = ty_name.split('<').next().unwrap_or(ty_name);
                    if let Some(id) = self.program.types.get_type_id(base) {
                        let narrow_ty = Type::Named(id);
                        self.binding_types.insert(name.clone(), narrow_ty.clone());
                        if self.type_is_resource(&narrow_ty) {
                            self.resource_bindings.insert(name.clone());
                        }
                    }
                }
            }
        }
    }

    /// Collect free variables referenced in an expression (excluding local bindings).
    /// Also collects field projections for each capture (F4: closure field-level captures).
    fn collect_free_vars(&self, expr: &Expr, locals: &HashSet<String>, out: &mut Vec<String>) {
        self.collect_free_vars_inner(expr, locals, out, &mut HashMap::new());
    }

    /// Collect free variables with field projection tracking.
    /// `projections` maps captured var name → narrowest field projection used in the closure.
    fn collect_free_vars_with_projections(
        &self,
        expr: &Expr,
        locals: &HashSet<String>,
        out: &mut Vec<String>,
        projections: &mut HashMap<String, Option<Vec<String>>>,
    ) {
        self.collect_free_vars_inner(expr, locals, out, projections);
    }

    // ---- Which closures outlive the frame that built them ----

    /// A closure that outlives its frame has to carry its captures; one that
    /// doesn't can point at them. That isn't a choice — a closure handed to a
    /// task or stored in a field would be reading a dead frame otherwise — so
    /// the compiler decides it rather than asking for a word at the literal.
    ///
    /// Collected up front, before any body is walked, because the literal is
    /// where the captures are taken and the escape is usually a line or two
    /// further down: `let f = || { … }` says nothing, `spawn(f)` says it all.
    /// Still function-local — nothing here reads past the body it is walking.
    ///
    /// Where a closure ends up outliving the frame:
    ///
    /// - handed to a `take` parameter, which is where `spawn` lives (its
    ///   signature is `spawn(take f: func() -> T)`)
    /// - returned
    /// - stored into a struct field, or assigned through a field or an index
    ///
    /// A borrow parameter is not on the list: PM6 says the callee can't keep
    /// what it borrowed, so the closure dies with the call.
    fn collect_escaping_closures(&mut self, decls: &[Decl]) {
        for decl in decls {
            match &decl.kind {
                DeclKind::Fn(f) => self.escapes_in_body(&f.body),
                DeclKind::Struct(s) => {
                    for m in &s.methods { self.escapes_in_body(&m.body); }
                }
                DeclKind::Enum(e) => {
                    for m in &e.methods { self.escapes_in_body(&m.body); }
                }
                DeclKind::Impl(i) => {
                    for m in &i.methods { self.escapes_in_body(&m.body); }
                }
                // `test` and `benchmark` bodies are function bodies, and the
                // spawn tests live in them.
                DeclKind::Test(t) => self.escapes_in_body(&t.body),
                DeclKind::Benchmark(b) => self.escapes_in_body(&b.body),
                _ => {}
            }
        }
    }

    fn escapes_in_body(&mut self, body: &[Stmt]) {
        let mut named: HashMap<String, Vec<rask_ast::NodeId>> = HashMap::new();
        self.escapes_in_stmts(body, &mut named);
    }

    fn escapes_in_stmts(&mut self, body: &[Stmt], named: &mut HashMap<String, Vec<rask_ast::NodeId>>) {
        for stmt in body {
            self.escapes_in_stmt(stmt, named);
        }
    }

    fn escapes_in_stmt(&mut self, stmt: &Stmt, named: &mut HashMap<String, Vec<rask_ast::NodeId>>) {
        match &stmt.kind {
            StmtKind::Let { name, init, .. } | StmtKind::Mut { name, init, .. } => {
                self.escapes_in_expr(init, named);
                let carried = self.closure_ids_of(init, named);
                if carried.is_empty() { named.remove(name); }
                else { named.insert(name.clone(), carried); }
            }
            StmtKind::Assign { target, value, .. } => {
                self.escapes_in_expr(value, named);
                self.escapes_in_expr(target, named);
                // Through a field or an index the closure lands in something
                // that outlives the assignment; a plain name is just a rebind.
                if let ExprKind::Ident(name) = &target.kind {
                    let carried = self.closure_ids_of(value, named);
                    if carried.is_empty() { named.remove(name); }
                    else { named.insert(name.clone(), carried); }
                } else {
                    self.mark_escaping(value, named);
                }
            }
            StmtKind::Return(Some(e)) => {
                self.escapes_in_expr(e, named);
                self.mark_escaping(e, named);
            }
            StmtKind::Break { value: Some(e), .. } => {
                self.escapes_in_expr(e, named);
                self.mark_escaping(e, named);
            }
            StmtKind::Expr(e) => self.escapes_in_expr(e, named),
            StmtKind::LetTuple { init, .. } | StmtKind::MutTuple { init, .. }
            | StmtKind::LetStruct { init, .. } => self.escapes_in_expr(init, named),
            StmtKind::While { cond, body, .. } => {
                self.escapes_in_expr(cond, named);
                self.escapes_in_stmts(body, named);
            }
            StmtKind::WhileLet { expr, body, .. } => {
                self.escapes_in_expr(expr, named);
                self.escapes_in_stmts(body, named);
            }
            StmtKind::For { iter, body, .. } | StmtKind::ComptimeFor { iter, body, .. } => {
                self.escapes_in_expr(iter, named);
                self.escapes_in_stmts(body, named);
            }
            StmtKind::Loop { body, .. } | StmtKind::Comptime(body) => {
                self.escapes_in_stmts(body, named)
            }
            StmtKind::Ensure { body, else_handler } => {
                self.escapes_in_stmts(body, named);
                if let Some((_, handler)) = else_handler {
                    self.escapes_in_stmts(handler, named);
                }
            }
            StmtKind::Return(None) | StmtKind::Break { value: None, .. }
            | StmtKind::Continue(_) | StmtKind::Discard { .. } => {}
        }
    }

    fn escapes_in_expr(&mut self, expr: &Expr, named: &mut HashMap<String, Vec<rask_ast::NodeId>>) {
        rask_ast::visit::walk_expr_pruned(expr, &mut |e| {
            match &e.kind {
                ExprKind::Call { func, args } => {
                    let takes = match &func.kind {
                        ExprKind::Ident(name) => self.fn_take_params.get(name).cloned(),
                        _ => None,
                    };
                    for (i, arg) in args.iter().enumerate() {
                        if takes.as_ref().and_then(|t| t.get(i)).copied().unwrap_or(false) {
                            self.mark_escaping(&arg.expr, named);
                        }
                    }
                    true
                }
                ExprKind::MethodCall { object, method, args, .. } => {
                    let modes = self.method_param_modes(object, method);
                    for (i, arg) in args.iter().enumerate() {
                        let takes = matches!(
                            modes.as_ref().and_then(|m| m.get(i)),
                            Some(ParamMode::Take)
                        );
                        // A method the signature table can't place is the
                        // common case for `spawn` on a handle or a group.
                        // Reading an unplaceable `spawn` as a borrow would hand
                        // the task a pointer into the frame that spawned it.
                        if takes || (modes.is_none() && method == "spawn") {
                            self.mark_escaping(&arg.expr, named);
                        }
                    }
                    true
                }
                ExprKind::StructLit { fields, .. } => {
                    for field in fields {
                        self.mark_escaping(&field.value, named);
                    }
                    true
                }
                // Statement order decides what a name stands for, and the
                // expression walk has none, so the statement walker takes these.
                // Every shape that holds statements has to be here: `using
                // Multitasking { … }` is where the spawns live, and routing it
                // through the plain walk instead lost the `let f = || …` that
                // `spawn(f)` two lines down needs.
                ExprKind::Block(body) | ExprKind::BlockCall { body, .. }
                | ExprKind::Unsafe { body } | ExprKind::Comptime { body }
                | ExprKind::Loop { body, .. } => {
                    self.escapes_in_stmts(body, named);
                    false
                }
                ExprKind::UsingBlock { args, body, .. } => {
                    for a in args { self.escapes_in_expr(&a.expr, named); }
                    self.escapes_in_stmts(body, named);
                    false
                }
                ExprKind::WithAs { bindings, body } => {
                    for b in bindings { self.escapes_in_expr(&b.source, named); }
                    self.escapes_in_stmts(body, named);
                    false
                }
                // A closure body is its own frame's business. What it stores or
                // returns escapes *its* frame, and the names out here mean
                // nothing inside it.
                ExprKind::Closure { params, body, .. } => {
                    let locals: HashSet<String> =
                        params.iter().map(|p| p.name.clone()).collect();
                    if self.body_assigns_a_free_name(body, &locals) {
                        self.closure_writes_a_capture.insert(e.id);
                    }
                    let mut inner = HashMap::new();
                    self.escapes_in_expr(body, &mut inner);
                    false
                }
                _ => true,
            }
        });
    }

    /// The closure literals an expression may be carrying: written there, bound
    /// to a name that holds one, or handed to a call whose result carries it.
    ///
    /// That last one is `return v.filter(|m| m > want)`. `filter` only borrows
    /// its closure — PM6 says it can't keep it — so the closure reaches the
    /// caller through the `Sequence` it answers, and `want` has to travel with
    /// it. Reading only the parameter mode there left the closure pointing at a
    /// frame that was already gone.
    ///
    /// A call that answers something unrelated to the closure it was handed
    /// gets caught in this net too, and carrying captures it could have pointed
    /// at is the harmless direction — except for a closure that *writes* one,
    /// where carrying would drop the write-back MC4 promises. Those are left
    /// alone: a mutating closure that genuinely rides out on a result is
    /// throwing the write away whatever it captures by, which is E0892's
    /// complaint rather than this one's.
    fn closure_ids_of(
        &self,
        expr: &Expr,
        named: &HashMap<String, Vec<rask_ast::NodeId>>,
    ) -> Vec<rask_ast::NodeId> {
        match &expr.kind {
            ExprKind::Closure { .. } => vec![expr.id],
            ExprKind::Ident(name) => named.get(name).cloned().unwrap_or_default(),
            ExprKind::Call { args, .. } | ExprKind::MethodCall { args, .. } => args
                .iter()
                .flat_map(|a| self.closure_ids_of(&a.expr, named))
                .filter(|id| !self.closure_writes_a_capture.contains(id))
                .collect(),
            _ => Vec::new(),
        }
    }

    fn mark_escaping(&mut self, expr: &Expr, named: &HashMap<String, Vec<rask_ast::NodeId>>) {
        for id in self.closure_ids_of(expr, named) {
            self.escaping_closures.insert(id);
        }
    }

    /// Whether a closure body assigns to a name it did not declare — a write
    /// the enclosing frame is promised to see (MC4).
    fn body_assigns_a_free_name(&self, body: &Expr, locals: &HashSet<String>) -> bool {
        let mut found = false;
        let mut declared = locals.clone();
        rask_ast::visit::walk_expr_pruned(body, &mut |e| {
            if let ExprKind::Block(stmts) = &e.kind {
                for stmt in stmts {
                    if let StmtKind::Assign { target, .. } = &stmt.kind {
                        if let Some(root) = Self::extract_root_and_fields(target).0 {
                            if !declared.contains(&root) {
                                found = true;
                            }
                        }
                    }
                    Self::names_declared_by(stmt, &mut declared);
                }
            }
            true
        });
        found
    }

    /// Whether this method call starts a task: `Thread.spawn`,
    /// `ThreadPool.spawn`, or `spawn` on a `TaskGroup`.
    ///
    /// The receiver decides, not the name. A program may have a `Runner` with a
    /// synchronous `spawn(cb)` that just calls what it was handed, and matching
    /// the bare name reported a lost write in a closure nothing ran on a task.
    ///
    /// Reading this the other way — escape — stays conservative on purpose. A
    /// capture carried into something that turns out not to be a task costs a
    /// copy; a capture pointed at from something that *is* one reads a dead
    /// frame, so `collect_escaping_closures` treats an unplaceable `spawn` as
    /// escaping and this one says nothing.
    fn is_task_spawn(&self, object: &Expr, method: &str) -> bool {
        if method != "spawn" {
            return false;
        }
        match &object.kind {
            ExprKind::Ident(name) if name == "Thread" || name == "ThreadPool" => true,
            _ => self
                .receiver_type_name(object)
                .is_some_and(|t| t == "Thread" || t == "ThreadPool" || t == "TaskGroup"),
        }
    }

    /// A closure literal the ownership pass decided points at its captures
    /// rather than carrying them (CM1).
    ///
    /// Every escape site asks this before reporting. A closure already known to
    /// outlive its frame carries what it captured, so there is nothing left to
    /// dangle; one that reaches an escape site without being known is a gap in
    /// `collect_escaping_closures`, and saying so beats lowering it as a borrow
    /// and handing it a dead frame.
    fn is_borrowing_closure(&self, expr: &Expr) -> bool {
        matches!(expr.kind, ExprKind::Closure { .. })
            && !self.escaping_closures.contains(&expr.id)
    }

    /// Whether a closure literal carries its captures rather than pointing at
    /// them. `mem.closures/CM1`: it does exactly when it outlives its frame.
    fn closure_carries_captures(&self, id: rask_ast::NodeId) -> bool {
        self.escaping_closures.contains(&id)
    }

    // ---- A task's write to a capture nothing reads back ----

    /// A closure handed to `spawn` gets a **copy** of every capture, and the
    /// task's environment dies when the task does. So a write to a capture the
    /// task never puts to use goes nowhere: the counter in the task is not the
    /// counter the parent prints, and `join()` is not a write-back.
    ///
    /// Only Copy captures ever reach here. Anything bigger is already rejected
    /// as an escaping borrow (SL2) or moved in, and a move leaves the parent
    /// nothing to read.
    ///
    /// The write is dead, and deadness is decidable from the body alone, so it
    /// is an error rather than a lint. The fix is a value that outlives the
    /// task: `Shared` reached through a clone, a channel, or the closure's
    /// return value.
    fn check_spawn_lost_writes(&mut self, arg: &Expr) {
        let closure = match &arg.kind {
            ExprKind::Closure { .. } => arg.clone(),
            ExprKind::Ident(name) => match self.closure_literals.get(name) {
                Some(c) => c.clone(),
                None => return,
            },
            _ => return,
        };
        let ExprKind::Closure { params, body, .. } = &closure.kind else { return };
        let locals: HashSet<String> = params.iter().map(|p| p.name.clone()).collect();

        // Nothing in the environment survives the task, so every capture starts
        // out with no use ahead of it. A read walking backwards is what puts one
        // there.
        let all_reads = self.task_reads(body, &locals);
        let mut lost = Vec::new();
        self.task_used_expr(body, &locals, &HashSet::new(), &all_reads, &mut lost);

        lost.sort_by_key(|(_, span)| (span.start, span.end));
        lost.dedup();
        for (name, span) in lost {
            self.errors.push(OwnershipError {
                kind: OwnershipErrorKind::TaskWriteLost { name, spawn_span: arg.span },
                span,
            });
        }
    }

    /// Backward walk over a task body answering, at each point, which captures
    /// still have a use ahead of them.
    ///
    /// Plain liveness is not enough, and the loop is why. In
    ///
    /// ```text
    /// for i in 0..10 { total += i }
    /// ```
    ///
    /// every write is read — by the next iteration. Liveness calls that live and
    /// the whole accumulation is still thrown away, because the only thing the
    /// reads feed is another write that goes nowhere. So a read only counts when
    /// it reaches a *use*: a write whose target has no use ahead of it
    /// contributes none of its own reads, which lets the chain collapse to
    /// nothing in one fixpoint. (Compilers call these faint variables — dead
    /// ones, plus the ones that only keep themselves alive.)
    ///
    /// Over-approximating is the safe direction: a capture in the set is one
    /// this reports nothing about.
    fn task_used_expr(
        &self,
        expr: &Expr,
        locals: &HashSet<String>,
        used_out: &HashSet<String>,
        all_reads: &HashSet<String>,
        lost: &mut Vec<(String, Span)>,
    ) -> HashSet<String> {
        match &expr.kind {
            ExprKind::Block(stmts) => {
                self.task_used_body(stmts, locals, used_out, all_reads, lost)
            }
            // Branches are alternatives, so what either one needs is needed
            // before the test.
            ExprKind::If { cond, then_branch, else_branch, .. } => {
                let mut used = self.task_used_expr(then_branch, locals, used_out, all_reads, lost);
                match else_branch {
                    Some(e) => used.extend(self.task_used_expr(e, locals, used_out, all_reads, lost)),
                    None => used.extend(used_out.iter().cloned()),
                }
                used.extend(self.task_reads(cond, locals));
                used
            }
            ExprKind::IfLet { expr: scrutinee, then_branch, else_branch, .. } => {
                let mut used = self.task_used_expr(then_branch, locals, used_out, all_reads, lost);
                match else_branch {
                    Some(e) => used.extend(self.task_used_expr(e, locals, used_out, all_reads, lost)),
                    None => used.extend(used_out.iter().cloned()),
                }
                used.extend(self.task_reads(scrutinee, locals));
                used
            }
            ExprKind::Match { scrutinee, arms } => {
                let mut used = HashSet::new();
                for arm in arms {
                    used.extend(self.task_used_expr(&arm.body, locals, used_out, all_reads, lost));
                    if let Some(g) = &arm.guard {
                        used.extend(self.task_reads(g, locals));
                    }
                }
                if arms.is_empty() {
                    used.extend(used_out.iter().cloned());
                }
                used.extend(self.task_reads(scrutinee, locals));
                used
            }
            // A nested closure borrows the task's environment, so a write in
            // there is one the task can still read back. Everything it names
            // counts as used.
            _ => {
                let mut used = used_out.clone();
                used.extend(self.task_reads(expr, locals));
                used
            }
        }
    }

    fn task_used_body(
        &self,
        body: &[Stmt],
        locals: &HashSet<String>,
        used_out: &HashSet<String>,
        all_reads: &HashSet<String>,
        lost: &mut Vec<(String, Span)>,
    ) -> HashSet<String> {
        // A backward walk needs each statement's scope, which only a forward
        // one knows: a name a `let` introduces shadows the capture below it.
        let mut scopes = Vec::with_capacity(body.len());
        let mut scope = locals.clone();
        for stmt in body {
            scopes.push(scope.clone());
            Self::names_declared_by(stmt, &mut scope);
        }

        let mut used = used_out.clone();
        for (stmt, scope) in body.iter().zip(scopes.iter()).rev() {
            used = self.task_used_stmt(stmt, scope, &used, all_reads, lost);
        }
        used
    }

    fn task_used_stmt(
        &self,
        stmt: &Stmt,
        locals: &HashSet<String>,
        used_out: &HashSet<String>,
        all_reads: &HashSet<String>,
        lost: &mut Vec<(String, Span)>,
    ) -> HashSet<String> {
        match &stmt.kind {
            StmtKind::Assign { target, value, .. } => {
                let root = Self::extract_root_and_fields(target).0;
                let captured = root
                    .as_ref()
                    .filter(|r| !locals.contains(*r) && self.bindings.contains_key(*r));
                let Some(name) = captured else {
                    let mut used = used_out.clone();
                    used.extend(self.task_reads(target, locals));
                    used.extend(self.task_reads(value, locals));
                    return used;
                };
                if !used_out.contains(name) {
                    // Nothing ahead wants what this writes, so the write is
                    // thrown away — and so is everything it read to get there.
                    lost.push((name.clone(), stmt.span));
                    return used_out.clone();
                }
                let mut used = used_out.clone();
                // The write replaces the value, so what was there is wanted
                // only where the new value is built from it — `n = n + 1`, or a
                // write to one field of a struct that keeps the others.
                used.remove(name);
                used.extend(self.task_reads(value, locals));
                if !matches!(target.kind, ExprKind::Ident(_)) {
                    used.insert(name.clone());
                    used.extend(self.task_reads(target, locals));
                }
                used
            }
            StmtKind::Expr(e) => self.task_used_expr(e, locals, used_out, all_reads, lost),
            StmtKind::Mut { init, .. } | StmtKind::Let { init, .. }
            | StmtKind::MutTuple { init, .. } | StmtKind::LetTuple { init, .. }
            | StmtKind::LetStruct { init, .. } => {
                let mut used = used_out.clone();
                used.extend(self.task_reads(init, locals));
                used
            }
            // A return leaves the task, so nothing after it runs and only what
            // it hands back is wanted.
            StmtKind::Return(Some(e)) => self.task_reads(e, locals),
            StmtKind::Return(None) => HashSet::new(),
            // Where the jump lands takes a CFG to say. Everything the body
            // reads is the answer that can't be wrong.
            StmtKind::Break { .. } | StmtKind::Continue(_) => all_reads.clone(),
            StmtKind::While { cond, body, .. } => {
                let mut used = self.task_used_loop(body, locals, used_out, all_reads, lost);
                used.extend(self.task_reads(cond, locals));
                used
            }
            StmtKind::WhileLet { expr, body, .. } => {
                let mut used = self.task_used_loop(body, locals, used_out, all_reads, lost);
                used.extend(self.task_reads(expr, locals));
                used
            }
            StmtKind::Loop { body, .. } => {
                self.task_used_loop(body, locals, used_out, all_reads, lost)
            }
            StmtKind::For { binding, iter, body, .. }
            | StmtKind::ComptimeFor { binding, iter, body, .. } => {
                // The loop's own binding shadows a capture of the same name
                // inside the body, so the body's reads of it are not reads of
                // the capture. Without this `spawn(|| { i = 5  for i in 0..3 {
                // println("{i}") } })` looked like the write was put to use, by
                // the loop variable that replaced it.
                let mut inner = locals.clone();
                for name in binding.names() {
                    inner.insert(name.to_string());
                }
                let mut used = self.task_used_loop(body, &inner, used_out, all_reads, lost);
                used.extend(self.task_reads(iter, locals));
                used
            }
            // An `ensure` body runs at every exit, including ones this walk has
            // no edge for, so read it and claim nothing about writes inside.
            StmtKind::Ensure { body, else_handler } => {
                let mut used = used_out.clone();
                used.extend(self.task_reads_body(body, locals));
                if let Some((_, handler)) = else_handler {
                    used.extend(self.task_reads_body(handler, locals));
                }
                used
            }
            StmtKind::Comptime(body) => {
                let mut used = used_out.clone();
                used.extend(self.task_reads_body(body, locals));
                used
            }
            StmtKind::Discard { .. } => used_out.clone(),
        }
    }

    /// A loop body runs again, so what it needs on entry it also needs on the
    /// way out. Grow the set until that stops adding anything, then walk it once
    /// more — the reports only mean something at the fixpoint.
    fn task_used_loop(
        &self,
        body: &[Stmt],
        locals: &HashSet<String>,
        used_out: &HashSet<String>,
        all_reads: &HashSet<String>,
        lost: &mut Vec<(String, Span)>,
    ) -> HashSet<String> {
        let mut body_out = used_out.clone();
        loop {
            let mark = lost.len();
            let body_in = self.task_used_body(body, locals, &body_out, all_reads, lost);
            lost.truncate(mark);
            let grown: HashSet<String> = body_out.union(&body_in).cloned().collect();
            if grown == body_out {
                break;
            }
            body_out = grown;
        }
        self.task_used_body(body, locals, &body_out, all_reads, lost)
    }

    fn task_reads(&self, expr: &Expr, locals: &HashSet<String>) -> HashSet<String> {
        let mut names = Vec::new();
        self.collect_free_vars_inner(expr, locals, &mut names, &mut HashMap::new());
        names.into_iter().collect()
    }

    fn task_reads_body(&self, body: &[Stmt], locals: &HashSet<String>) -> HashSet<String> {
        let mut names = Vec::new();
        self.collect_free_vars_body_inner(body, locals, &mut names, &mut HashMap::new());
        names.into_iter().collect()
    }


    fn collect_free_vars_inner(
        &self,
        expr: &Expr,
        locals: &HashSet<String>,
        out: &mut Vec<String>,
        projections: &mut HashMap<String, Option<Vec<String>>>,
    ) {
        // F4: For field access expressions, try to extract root + projection
        // and record the field-level capture instead of whole-object.
        if let ExprKind::Field { .. } = &expr.kind {
            let (root, fields) = Self::extract_root_and_fields(expr);
            if let Some(ref root_name) = root {
                if !locals.contains(root_name) && self.bindings.contains_key(root_name) {
                    if !out.contains(root_name) {
                        out.push(root_name.clone());
                    }
                    // Record or merge field projection for this capture.
                    // If this capture already has a whole-object access (None), keep it.
                    // If it has a different field, widen to whole-object.
                    let entry = projections.entry(root_name.clone());
                    match entry {
                        std::collections::hash_map::Entry::Vacant(e) => {
                            e.insert(fields);
                        }
                        std::collections::hash_map::Entry::Occupied(mut e) => {
                            // Merge: if existing is None (whole-object), keep None.
                            // If existing is Some(fields_a) and new is Some(fields_b),
                            // union the field sets. If new is None, widen to None.
                            match (e.get_mut(), &fields) {
                                (None, _) => {} // already whole-object
                                (existing @ Some(_), None) => { *existing = None; }
                                (Some(ref mut existing_fields), Some(new_fields)) => {
                                    for f in new_fields {
                                        if !existing_fields.contains(f) {
                                            existing_fields.push(f.clone());
                                        }
                                    }
                                }
                            }
                        }
                    }
                    return; // Don't recurse into Field — we already handled the root
                }
            }
        }

        match &expr.kind {
            ExprKind::Ident(name) => {
                if !locals.contains(name) && self.bindings.contains_key(name) {
                    if !out.contains(name) {
                        out.push(name.clone());
                        // Whole-object access (no field projection)
                        projections.entry(name.clone()).or_insert(None);
                    } else {
                        // Already captured — widen to whole-object if accessed directly
                        projections.insert(name.clone(), None);
                    }
                }
            }
            ExprKind::Binary { left, right, .. } => {
                self.collect_free_vars_inner(left, locals, out, projections);
                self.collect_free_vars_inner(right, locals, out, projections);
            }
            ExprKind::Unary { operand, .. } => {
                self.collect_free_vars_inner(operand, locals, out, projections);
            }
            ExprKind::Call { func, args } => {
                self.collect_free_vars_inner(func, locals, out, projections);
                for arg in args { self.collect_free_vars_inner(&arg.expr, locals, out, projections); }
            }
            ExprKind::MethodCall { object, args, .. } => {
                self.collect_free_vars_inner(object, locals, out, projections);
                for arg in args { self.collect_free_vars_inner(&arg.expr, locals, out, projections); }
            }
            ExprKind::Field { object, .. } | ExprKind::OptionalField { object, .. } => {
                // Field access on non-free-var roots (handled above for free vars)
                self.collect_free_vars_inner(object, locals, out, projections);
            }
            ExprKind::Index { object, index } => {
                self.collect_free_vars_inner(object, locals, out, projections);
                self.collect_free_vars_inner(index, locals, out, projections);
            }
            ExprKind::If { cond, then_branch, else_branch, .. } => {
                self.collect_free_vars_inner(cond, locals, out, projections);
                self.collect_free_vars_inner(then_branch, locals, out, projections);
                if let Some(e) = else_branch { self.collect_free_vars_inner(e, locals, out, projections); }
            }
            ExprKind::Block(stmts) => {
                self.collect_free_vars_body_inner(stmts, locals, out, projections);
            }
            ExprKind::Closure { params, body, .. } => {
                let mut inner_locals = locals.clone();
                for p in params { inner_locals.insert(p.name.clone()); }
                self.collect_free_vars_inner(body, &inner_locals, out, projections);
            }
            ExprKind::Match { scrutinee, arms } => {
                self.collect_free_vars_inner(scrutinee, locals, out, projections);
                for arm in arms {
                    self.collect_free_vars_inner(&arm.body, locals, out, projections);
                    if let Some(g) = &arm.guard { self.collect_free_vars_inner(g, locals, out, projections); }
                }
            }
            ExprKind::IfLet { expr: scrutinee, then_branch, else_branch, .. } => {
                self.collect_free_vars_inner(scrutinee, locals, out, projections);
                self.collect_free_vars_inner(then_branch, locals, out, projections);
                if let Some(e) = else_branch { self.collect_free_vars_inner(e, locals, out, projections); }
            }
            ExprKind::GuardPattern { expr: scrutinee, else_branch, .. } => {
                self.collect_free_vars_inner(scrutinee, locals, out, projections);
                self.collect_free_vars_inner(else_branch, locals, out, projections);
            }
            ExprKind::IsPattern { expr: scrutinee, .. } => {
                self.collect_free_vars_inner(scrutinee, locals, out, projections);
            }
            ExprKind::Try { expr: inner } | ExprKind::Take { place: inner } => {
                self.collect_free_vars_inner(inner, locals, out, projections);
            }
            ExprKind::Catch { value, clause } => {
                self.collect_free_vars_inner(value, locals, out, projections);
                self.collect_free_vars_inner(&clause.body, locals, out, projections);
            }
            ExprKind::IsPresent { expr: inner, .. } => {
                self.collect_free_vars_inner(inner, locals, out, projections);
            }
            ExprKind::Unwrap { expr: inner, .. }
            | ExprKind::Cast { expr: inner, .. }
            | ExprKind::Convert { expr: inner, .. } => {
                self.collect_free_vars_inner(inner, locals, out, projections);
            }
            ExprKind::NullCoalesce { value, default } => {
                self.collect_free_vars_inner(value, locals, out, projections);
                self.collect_free_vars_inner(default, locals, out, projections);
            }
            ExprKind::Range { start, end, .. } => {
                if let Some(s) = start { self.collect_free_vars_inner(s, locals, out, projections); }
                if let Some(e) = end { self.collect_free_vars_inner(e, locals, out, projections); }
            }
            ExprKind::StructLit { fields, spread, .. } => {
                for f in fields { self.collect_free_vars_inner(&f.value, locals, out, projections); }
                if let Some(s) = spread { self.collect_free_vars_inner(s, locals, out, projections); }
            }
            ExprKind::Array(elems) | ExprKind::Tuple(elems) => {
                for e in elems { self.collect_free_vars_inner(e, locals, out, projections); }
            }
            ExprKind::ArrayRepeat { value, count } => {
                self.collect_free_vars_inner(value, locals, out, projections);
                self.collect_free_vars_inner(count, locals, out, projections);
            }
            ExprKind::UsingBlock { args, body, .. } => {
                for arg in args { self.collect_free_vars_inner(&arg.expr, locals, out, projections); }
                self.collect_free_vars_body_inner(body, locals, out, projections);
            }
            ExprKind::WithAs { bindings, body } => {
                for b in bindings { self.collect_free_vars_inner(&b.source, locals, out, projections); }
                self.collect_free_vars_body_inner(body, locals, out, projections);
            }
            ExprKind::Assert { condition, message } | ExprKind::Check { condition, message, .. } => {
                self.collect_free_vars_inner(condition, locals, out, projections);
                if let Some(m) = message { self.collect_free_vars_inner(m, locals, out, projections); }
            }
            ExprKind::Select { arms, .. } => {
                for arm in arms {
                    self.collect_free_vars_inner(&arm.body, locals, out, projections);
                }
            }
            ExprKind::Unsafe { body } | ExprKind::Comptime { body } | ExprKind::BlockCall { body, .. } | ExprKind::Loop { body, .. } => {
                self.collect_free_vars_body_inner(body, locals, out, projections);
            }
            _ => {
                // Literals, string interpolation, etc.
            }
        }
    }

    /// Free variables of a statement list, with each statement seeing the names
    /// the ones above it declared.
    ///
    /// Without this a closure's own `mut total = 0` didn't shadow an outer
    /// `total`, so the outer one was reported as a capture the closure writes —
    /// which registered a borrow on a variable the closure never touches, and
    /// under MC2 rejected the outer name's next use.
    fn collect_free_vars_body_inner(
        &self,
        body: &[Stmt],
        locals: &HashSet<String>,
        out: &mut Vec<String>,
        projections: &mut HashMap<String, Option<Vec<String>>>,
    ) {
        let mut scope = locals.clone();
        for stmt in body {
            self.collect_free_vars_stmt_inner(stmt, &scope, out, projections);
            Self::names_declared_by(stmt, &mut scope);
        }
    }

    /// The names a statement introduces into the rest of its block.
    fn names_declared_by(stmt: &Stmt, scope: &mut HashSet<String>) {
        match &stmt.kind {
            StmtKind::Let { name, .. } | StmtKind::Mut { name, .. } => {
                scope.insert(name.clone());
            }
            StmtKind::LetTuple { patterns, .. } | StmtKind::MutTuple { patterns, .. } => {
                for name in rask_ast::stmt::tuple_pats_flat_names(patterns) {
                    scope.insert(name.to_string());
                }
            }
            StmtKind::LetStruct { pattern, .. } => {
                for name in pattern.bound_names() {
                    scope.insert(name.to_string());
                }
            }
            _ => {}
        }
    }

    fn collect_free_vars_stmt(&self, stmt: &Stmt, locals: &HashSet<String>, out: &mut Vec<String>) {
        self.collect_free_vars_stmt_inner(stmt, locals, out, &mut HashMap::new());
    }

    fn collect_free_vars_stmt_inner(
        &self,
        stmt: &Stmt,
        locals: &HashSet<String>,
        out: &mut Vec<String>,
        projections: &mut HashMap<String, Option<Vec<String>>>,
    ) {
        match &stmt.kind {
            StmtKind::Expr(e) => self.collect_free_vars_inner(e, locals, out, projections),
            StmtKind::Mut { init, .. } | StmtKind::Let { init, .. } => {
                self.collect_free_vars_inner(init, locals, out, projections);
            }
            StmtKind::MutTuple { init, .. }
            | StmtKind::LetTuple { init, .. }
            | StmtKind::LetStruct { init, .. } => {
                self.collect_free_vars_inner(init, locals, out, projections);
            }
            StmtKind::Assign { target, value, .. } => {
                self.collect_free_vars_inner(target, locals, out, projections);
                self.collect_free_vars_inner(value, locals, out, projections);
            }
            StmtKind::Return(Some(e)) | StmtKind::Break { value: Some(e), .. } => {
                self.collect_free_vars_inner(e, locals, out, projections);
            }
            StmtKind::While { cond, body, .. } => {
                self.collect_free_vars_inner(cond, locals, out, projections);
                self.collect_free_vars_body_inner(body, locals, out, projections);
            }
            StmtKind::WhileLet { expr, body, .. } => {
                self.collect_free_vars_inner(expr, locals, out, projections);
                self.collect_free_vars_body_inner(body, locals, out, projections);
            }
            StmtKind::Loop { body, .. } => {
                self.collect_free_vars_body_inner(body, locals, out, projections);
            }
            StmtKind::For { iter, body, .. } => {
                self.collect_free_vars_inner(iter, locals, out, projections);
                self.collect_free_vars_body_inner(body, locals, out, projections);
            }
            StmtKind::Ensure { body, else_handler } => {
                self.collect_free_vars_body_inner(body, locals, out, projections);
                if let Some((_, handler_body)) = else_handler {
                    self.collect_free_vars_body_inner(handler_body, locals, out, projections);
                }
            }
            StmtKind::Comptime(body) => {
                self.collect_free_vars_body_inner(body, locals, out, projections);
            }
            StmtKind::ComptimeFor { iter, body, .. } => {
                self.collect_free_vars_inner(iter, locals, out, projections);
                self.collect_free_vars_body_inner(body, locals, out, projections);
            }
            StmtKind::Return(None) | StmtKind::Break { value: None, .. }
            | StmtKind::Continue(_) | StmtKind::Discard { .. } => {}
        }
    }

    // ---- MC2: a mutable capture is exclusive while it lasts ----
    //
    // `mem.closures/MC2` says that while a mutable capture exists, nothing else
    // may reach the variable. Two closures writing one counter, or a read
    // between two calls that change it, is the same aliasing bug the borrow
    // rules exist to stop — and nothing checked it, so both compiled (#1087).
    //
    // The capture lasts until the closure's last use, not to the end of the
    // block. MC4 is the reason: "caller sees mutations after the closure
    // completes" is the whole point of a mutable capture, so a read after the
    // last call has to stay legal.
    //
    // Only a closure bound to a name is tracked. One written inline —
    // `v.each(|x| { total = total + x })` — dies at the semicolon, so there is
    // never a second thing reaching the variable while it lives.

    /// Every name any expression in this statement mentions.
    fn names_in(stmt: &Stmt) -> HashSet<String> {
        let mut out = HashSet::new();
        rask_ast::visit::walk_stmt(stmt, &mut |e| {
            if let ExprKind::Ident(name) = &e.kind {
                out.insert(name.clone());
            }
        });
        out
    }

    /// For each name, the last statement in this list that mentions it.
    fn last_mentions(stmts: &[Stmt]) -> HashMap<String, usize> {
        let mut out = HashMap::new();
        for (index, stmt) in stmts.iter().enumerate() {
            for name in Self::names_in(stmt) {
                out.insert(name, index);
            }
        }
        out
    }

    /// The name at the root of an assignment target: `x`, `x.f`, `x[i].f`.
    fn assign_root(target: &Expr) -> Option<String> {
        match &target.kind {
            ExprKind::Ident(name) => Some(name.clone()),
            ExprKind::Field { object, .. }
            | ExprKind::OptionalField { object, .. }
            | ExprKind::Index { object, .. }
            | ExprKind::DynamicField { object, .. } => Self::assign_root(object),
            _ => None,
        }
    }

    /// Names this statement list assigns to, including through the bodies that
    /// hang off a statement rather than off a block expression.
    fn assigned_in(stmts: &[Stmt], out: &mut Vec<String>) {
        for stmt in stmts {
            if let StmtKind::Assign { target, .. } = &stmt.kind {
                if let Some(root) = Self::assign_root(target) {
                    out.push(root);
                }
            }
            match &stmt.kind {
                StmtKind::While { body, .. }
                | StmtKind::WhileLet { body, .. }
                | StmtKind::Loop { body, .. }
                | StmtKind::For { body, .. }
                | StmtKind::ComptimeFor { body, .. }
                | StmtKind::Comptime(body) => Self::assigned_in(body, out),
                StmtKind::Ensure { body, else_handler } => {
                    Self::assigned_in(body, out);
                    if let Some((_, handler)) = else_handler {
                        Self::assigned_in(handler, out);
                    }
                }
                _ => {}
            }
        }
    }

    /// Which of a closure's captures its body writes.
    ///
    /// Intersecting with the captures is what makes the scan safe to keep
    /// flat: a name the body declares itself is not a capture, so a local
    /// `total` inside can't be mistaken for the outer one.
    fn written_captures(&self, body: &Expr, captures: &[String]) -> Vec<String> {
        let mut written: Vec<String> = Vec::new();
        rask_ast::visit::walk_expr(body, &mut |e| match &e.kind {
            // Every block expression anywhere below, so the assignments inside
            // an `if` arm or a nested closure are seen too.
            ExprKind::Block(stmts) => Self::assigned_in(stmts, &mut written),
            // `v.push(1)` writes `v` as surely as `v[0] = 1` does, and
            // `h.items.push(1)` writes `h`.
            ExprKind::MethodCall { object, method, .. } => {
                if self.is_mutate_self_method(object, method) {
                    if let Some(root) = Self::assign_root(object) {
                        written.push(root);
                    }
                }
            }
            _ => {}
        });
        captures
            .iter()
            .filter(|name| written.contains(name))
            .cloned()
            .collect()
    }

    /// The closure a `let`/`mut` statement binds, if it binds one that borrows
    /// its captures. A carrying closure moved them instead, and a use of the
    /// variable afterwards is already a use after move.
    fn closure_binding<'s>(&self, stmt: &'s Stmt) -> Option<(&'s String, &'s Expr)> {
        let (name, init) = match &stmt.kind {
            StmtKind::Let { name, init, .. } | StmtKind::Mut { name, init, .. } => (name, init),
            _ => return None,
        };
        if self.is_borrowing_closure(init) { Some((name, init)) } else { None }
    }

    /// MC2: report anything in this statement that reaches a variable a live
    /// closure is holding.
    fn check_mutable_capture_access(&mut self, stmt: &Stmt) {
        let touched = Self::names_in(stmt);
        // What a closure *this* statement binds would write, so the message can
        // say "captured again" only when that is what happened — a second
        // closure that merely reads the variable is still a conflict, but a
        // different sentence.
        let would_write = self.captures_a_statement_writes(stmt);

        let mut hit: Vec<(String, String, Span, bool)> = Vec::new();
        for capture in &self.mutable_captures {
            for name in &capture.vars {
                if touched.contains(name) {
                    hit.push((
                        name.clone(),
                        capture.holder.clone(),
                        capture.span,
                        would_write.contains(name),
                    ));
                }
            }
        }
        for (name, holder, captured_at, second_closure) in hit {
            self.errors.push(OwnershipError {
                kind: OwnershipErrorKind::MutableCaptureConflict {
                    name: name.clone(),
                    holder: holder.clone(),
                    captured_at,
                    second_closure,
                },
                span: stmt.span,
            });
            // One report per conflict. The second access to the same variable
            // is the same mistake, and a loop body would otherwise say it once
            // per statement.
            self.mutable_captures.retain(|c| c.holder != holder);
        }
    }

    /// The captures a closure bound by this statement writes, if it binds one.
    fn captures_a_statement_writes(&self, stmt: &Stmt) -> Vec<String> {
        let Some((_, closure)) = self.closure_binding(stmt) else { return Vec::new() };
        let ExprKind::Closure { params, body, .. } = &closure.kind else { return Vec::new() };
        let param_names: HashSet<String> = params.iter().map(|p| p.name.clone()).collect();
        let mut captures = Vec::new();
        self.collect_free_vars(body, &param_names, &mut captures);
        self.written_captures(body, &captures)
    }

    /// Record what a closure this statement binds holds, and retire the
    /// records whose closure has now seen its last use.
    fn register_mutable_capture(
        &mut self,
        stmt: &Stmt,
        index: usize,
        block_id: u32,
        last_mention: &HashMap<String, usize>,
    ) {
        if let Some((holder, closure)) = self.closure_binding(stmt) {
            let vars = self.captures_a_statement_writes(stmt);
            if !vars.is_empty() {
                self.mutable_captures.push(MutableCapture {
                    holder: holder.clone(),
                    vars,
                    span: closure.span,
                    dies_after: last_mention.get(holder).copied().unwrap_or(index),
                    block: block_id,
                });
            }
        }

        self.mutable_captures
            .retain(|c| !(c.block == block_id && c.dies_after <= index));
    }

    /// Check if a method call uses `take self`.
    fn is_take_self_method(&self, object: &Expr, method_name: &str) -> bool {
        self.self_param_of(object, method_name) == Some(rask_types::SelfParam::Take)
    }

    /// Check if a method call uses `mutate self` — it writes its receiver.
    fn is_mutate_self_method(&self, object: &Expr, method_name: &str) -> bool {
        self.self_param_of(object, method_name) == Some(rask_types::SelfParam::Mutate)
    }

    /// How a method takes its receiver, looked up from the receiver's type.
    fn self_param_of(&self, object: &Expr, method_name: &str) -> Option<rask_types::SelfParam> {
        if let Some(ty) = self.program.node_types.get(&object.id) {
            let type_id = match ty {
                Type::Named(id) => Some(*id),
                Type::Generic { base, .. } => Some(*base),
                // A stdlib type arrives here as its own *name* when the call
                // that produced it went through the module-function path — a
                // stub's return type is parsed before the type table exists, so
                // `io.stdout()` gives back `UnresolvedNamed("Stdout")`. Method
                // resolution already looks those up by name; this didn't, so
                // `out.close()` discharged nothing and every program that
                // acquired a standard stream was told it leaked the handle it had
                // just closed (#859).
                Type::UnresolvedNamed(name) => self.program.types.get_type_id(name),
                _ => None,
            };
            if let Some(id) = type_id {
                if let Some(def) = self.program.types.get(id) {
                    let methods = match def {
                        rask_types::TypeDef::Struct { methods, .. } => methods,
                        rask_types::TypeDef::Enum { methods, .. } => methods,
                        _ => return None,
                    };
                    for m in methods {
                        if m.name == method_name {
                            return Some(m.self_param);
                        }
                    }
                }
            }
        }
        None
    }

    /// Mark an argument as consumed (moved) when it names a binding.
    /// Copy values (VS1/VS2) stay valid — passing them to `take`/`own` copies.
    fn consume_arg(&mut self, arg_expr: &Expr, sink: Option<&str>) {
        if let ExprKind::Ident(name) = &arg_expr.kind {
            // An `Owned` box reads as its payload, so a small payload made the
            // binding look Copy and `drop(p)` consumed nothing — the leak was
            // reported on a freed value and `drop(p); drop(p)` drew no error at
            // all. Linearity is a property of the box, not of what's in it (#819).
            // The checker's type for this very node, when the binding table
            // has nothing. A `for` binding is recorded only for a rack
            // iteration, so `for i in 1..4 { v.push(i); println("{i}") }` read
            // as a move of `i` and asked for `i.clone()` on an integer — the
            // table's silence means "not recorded", and treating it as "not
            // Copy" is the right default only where there is nothing else to
            // ask.
            let ty = self
                .binding_types
                .get(name)
                .or_else(|| self.program.node_types.get(&arg_expr.id));
            let is_copy = !self.owned_bindings.contains(name)
                && ty.map(|t| self.is_copy(t)).unwrap_or(false);
            if !is_copy {
                self.consume_binding(name, arg_expr.span, sink);
            }
        }
    }
    /// Best-effort display name for a resource-typed value, recursing through
    /// `T or E` to name whichever side is actually linear (E0834: a bare
    /// statement whose type is `TaskHandle<T> or E` still leaks the handle).
    fn resource_type_display(&self, ty: &Type) -> String {
        match ty {
            Type::Named(id) | Type::Generic { base: id, .. } => self.program.types.type_name(*id),
            Type::Result { ok, err } => {
                if self.program.types.is_linear_value(ok) {
                    self.resource_type_display(ok)
                } else {
                    self.resource_type_display(err)
                }
            }
            _ => ty.to_string(),
        }
    }

    /// What must hold wherever control leaves the body: every resource consumed,
    /// every `mutate` parameter refilled. Reported at each `return` as well as at
    /// the end, because an early return is an exit and the end-of-body check can't
    /// see it.
    fn check_exit_obligations(&mut self, span: Span) {
        self.check_resource_consumption(span);
        self.check_mutate_params_refilled(span);
    }

    /// `try` is a way out of the function, so a resource still open when one
    /// runs leaks whenever that call fails.
    ///
    /// ```text
    /// let file = try File.open(path)
    /// let header = try file.read_header()   // `file` leaks on failure
    /// ```
    ///
    /// The happy path closes it, which is why this is invisible in testing. The
    /// answer is the one `ensure` exists for: commit the cleanup at the point of
    /// acquisition and every exit runs it, `try` included. Consuming explicitly
    /// later cancels it (L6), so the happy path reads unchanged.
    ///
    /// Only bindings still owed and not yet committed. An ensured one is
    /// covered by definition, and one already consumed owes nothing.
    fn check_try_leaks_a_resource(&mut self, span: Span) {
        let open: Vec<(String, Span)> = self
            .uncommitted_linears()
            .into_iter()
            .map(|n| {
                let at = self.resource_acquired_at.get(&n).copied().unwrap_or(span);
                (n, at)
            })
            .collect();
        for (name, acquired_at) in open {
            if !self.exit_reported.insert(format!("try:{}", name)) {
                continue;
            }
            self.errors.push(OwnershipError {
                kind: OwnershipErrorKind::ResourceLeaksOnTry { name, acquired_at },
                span,
            });
        }
    }


    /// Linear bindings that still owe a consumption and haven't committed to
    /// one. Sorted, so a function with two of them reports in a stable order.
    ///
    /// Committed means either `ensure` (the consumption is scheduled) or gone
    /// (already moved or consumed). Both leave nothing for an abnormal exit to
    /// lose.
    fn uncommitted_linears(&self) -> Vec<String> {
        let mut open: Vec<String> = self
            .resource_bindings
            .iter()
            .filter(|n| !self.ensure_registered.contains(*n))
            .filter(|n| !matches!(self.bindings.get(*n), Some(BindingState::Moved { .. })))
            .cloned()
            .collect();
        open.sort();
        open
    }

    /// What each uncommitted linear still owes, as a count.
    ///
    /// A plain struct holding two resources owes one debt per field, and an
    /// `ensure` pays them one at a time — so "is it still on the list" can't
    /// tell the first of two `ensure`s from a statement that committed nothing.
    /// The count can.
    fn commit_state(&self) -> Vec<(String, usize)> {
        self.uncommitted_linears()
            .into_iter()
            .map(|n| {
                let debts = self.resource_field_debts.get(&n).map_or(0, |d| d.len());
                (n, debts)
            })
            .collect()
    }

    /// mem.linear/L7: nothing may stand between acquiring a linear value and
    /// committing its cleanup.
    ///
    /// ```text
    /// let file = try File.open(path)
    /// let limit = config.read_limit()   // panics here and `file` is gone
    /// ensure file.close()
    /// ```
    ///
    /// `try` was already refused in that window, because the error exit is
    /// written in the source and can be pointed at. A panic is the same leak
    /// through an exit nothing marks — any index, any overflow, any call — and
    /// Rask has no destructor to catch what falls out. So the rule is the
    /// window itself: the statement after an acquisition commits it, or the
    /// program doesn't build.
    ///
    /// One commitment per statement is enough. `let (a, b) = pair()` is allowed
    /// to take two statements to ensure both — each one shortens the window,
    /// and registration order stays acquisition order, which is what makes the
    /// LIFO teardown come out right.
    fn check_commit_window(
        &mut self,
        pending_before: &[(String, usize)],
        errors_before: usize,
        stmt: &Stmt,
    ) {
        if pending_before.is_empty() {
            return;
        }
        // An `ensure` body is the cleanup, not a statement racing it. It runs at
        // scope exit, so a sibling resource still owed while it is being walked
        // is the ordinary state of a block with two `ensure`s in it.
        if self.in_ensure {
            return;
        }
        // Leaving the scope is not standing in the window. L1 and the `try`
        // check own those paths and phrase it better.
        if matches!(
            stmt.kind,
            StmtKind::Return(_) | StmtKind::Break { .. } | StmtKind::Continue(_)
        ) {
            return;
        }
        let still = self.commit_state();
        let progressed = pending_before.iter().any(|(name, debts)| {
            match still.iter().find(|(n, _)| n == name) {
                None => true,
                Some((_, now)) => now < debts,
            }
        });
        if progressed {
            return;
        }
        // The `try` in this statement already said it, with the better message.
        if self.errors[errors_before..]
            .iter()
            .any(|e| matches!(e.kind, OwnershipErrorKind::ResourceLeaksOnTry { .. }))
        {
            return;
        }
        for (name, _) in pending_before {
            if !self.exit_reported.insert(format!("commit:{}", name)) {
                continue;
            }
            let acquired_at = self
                .resource_acquired_at
                .get(name)
                .copied()
                .unwrap_or(stmt.span);
            self.errors.push(OwnershipError {
                kind: OwnershipErrorKind::ResourceCommitDeferred {
                    name: name.clone(),
                    acquired_at,
                },
                span: stmt.span,
            });
        }
    }

    /// `break` and `continue` leave the loop body, so a resource the body
    /// acquired this turn dies there.
    ///
    /// ```text
    /// while i < 3 {
    ///     let c = Conn { id: i }
    ///     if i == 1 { break }      // `c` is never closed
    ///     c.close()
    /// }
    /// ```
    ///
    /// The closing brace is the only exit the check knew about, and on the
    /// path that breaks, control never reaches it. `return` had been taught
    /// this; the two jumps that leave a loop had not (#882).
    ///
    /// Only what the loop introduced. A resource acquired before the loop and
    /// closed after it is still owed at the `break` and is nobody's problem
    /// there — judging the whole set would report every one of those.
    fn check_loop_exit_obligations(&mut self, span: Span) {
        let Some(entered_with) = self.loop_entry_resources.last().cloned() else {
            return;
        };
        let mut names: Vec<String> = self
            .resource_bindings
            .iter()
            .filter(|n| !entered_with.contains(*n))
            .cloned()
            .collect();
        names.sort();
        self.check_resource_names(names, span);
    }

    /// End-of-arm for the resources a match arm's pattern introduced: L1 where
    /// the arm falls through, then off the books either way.
    ///
    /// A diverging arm is somebody else's problem — the `return` it ends on ran
    /// the exit checks already.
    fn close_arm_resources(&mut self, before: &HashSet<String>, terminal: bool, span: Span) {
        let introduced: Vec<String> = self
            .resource_bindings
            .iter()
            .filter(|n| !before.contains(*n))
            .cloned()
            .collect();
        for name in introduced {
            if !terminal {
                let consumed =
                    matches!(self.bindings.get(&name), Some(BindingState::Moved { .. }))
                        || self.ensure_registered.contains(&name);
                if !consumed {
                    self.errors.push(OwnershipError {
                        kind: OwnershipErrorKind::ResourceNotConsumed { name: name.clone() },
                        span,
                    });
                }
            }
            self.resource_bindings.remove(&name);
            self.ensure_registered.remove(&name);
            self.resource_acquired_at.remove(&name);
            self.resource_field_debts.remove(&name);
        }
    }

    /// Give a binding away: mark it moved, or refuse if it was only borrowed.
    ///
    /// A parameter without `take` is the caller's value on loan (PM1). Handing it
    /// to a `take` parameter or a `take self` method used to just overwrite the
    /// state with `Moved`, so the callee consumed something it didn't own and the
    /// caller was never told — a double-close for a `@resource`, and a use of a
    /// given-away value for anything else (#804).
    /// SL2/SL4: a scope-limited closure passed as an argument.
    ///
    /// Handed to a `take` parameter it escapes, and that's the error. Handed to
    /// a borrow the callee cannot keep it (PM6), so the only way it reaches the
    /// caller again is the return value — record the limit against the call, and
    /// whatever binds the result inherits it.
    fn check_closure_arg_escape(
        &mut self,
        call_id: rask_ast::NodeId,
        arg: &Expr,
        param_is_take: Option<bool>,
    ) {
        // The signature decides, and only the signature. This used to read
        // "trust the mode, unless no mode is in reach, in which case assume the
        // worst", which made a language rule mean different things depending on
        // what the compiler managed to look up — and the case it was protecting
        // was `spawn`, whose declaration said it borrowed the closure while the
        // task it starts keeps it. That declaration says `take` now, so the
        // guess has nothing left to protect and SL4 can be read off the
        // signature at every call site (conc.tasks/T3 holds because `spawn`
        // says what it does, not because this line distrusts it).
        //
        // A callee with no mode in reach — a call through a closure variable —
        // is a call whose argument the callee cannot store either: it is a
        // Rask body, so PM6 binds it.
        let is_take = param_is_take.unwrap_or(false);
        let limit = match &arg.kind {
            ExprKind::Ident(name) => self
                .scope_limited_closures
                .get(name)
                .map(|&(b, _)| (b, name.clone()))
                // SL3: a plain local lent to a call that answers a closure. The
                // closure may be built over it — `make_filter(tags)` — so the
                // result is limited to the local's block. A parameter needs no
                // entry: it outlives the whole body already.
                .or_else(|| {
                    if self.outlives_this_call(name) || self.capture_is_copy(name) {
                        return None;
                    }
                    self.binding_decl_blocks.get(name).map(|&b| (b, name.clone()))
                }),
            ExprKind::Closure { .. } if self.is_borrowing_closure(arg) => self
                .closure_scope_limits
                .get(&arg.id)
                .map(|&b| (b, "<closure>".to_string())),
            _ => None,
        };
        let Some((borrow_block, name)) = limit else { return };
        // A `take` argument was given away, so it lends the result nothing.
        // Anything else would be the rule contradicting itself: SL4 says a
        // `take` argument contributes no limit, and recording one here made
        //
        //     let f = make_filter(tags)    // take tags: Vec<string>
        //     return f
        //
        // an escape, when `tags` is the closure's own and there is no borrow
        // left to outlive. The one thing still wrong at a `take` is handing
        // over a closure that is itself scope-limited: the callee keeps it and
        // the caller can't see where it goes (SL2).
        if is_take {
            if matches!(&arg.kind, ExprKind::Ident(n) if self.scope_limited_closures.contains_key(n))
                || self.is_borrowing_closure(arg)
            {
                self.errors.push(OwnershipError {
                    kind: OwnershipErrorKind::ScopeLimitedClosureEscapes { name: name.clone() },
                    span: arg.span,
                });
                self.scope_limited_closures.remove(&name);
            }
            return;
        }
        // Only if the call hands a closure back. `apply(f, 10) -> i64` borrows
        // the closure and answers a number — the number carries no borrow, and
        // marking the call limited made `return n` on an `i64` read as a
        // closure escaping.
        if !self.call_answers_a_closure(call_id) {
            return;
        }
        let existing = self.closure_scope_limits.get(&call_id).copied();
        self.closure_scope_limits
            .insert(call_id, existing.map_or(borrow_block, |e| e.max(borrow_block)));
    }

    /// Does this call's result hold a closure, and so a borrow worth tracking?
    fn call_answers_a_closure(&self, call_id: rask_ast::NodeId) -> bool {
        let Some(ty) = self.program.node_types.get(&call_id) else { return false };
        return Self::type_is_callable(ty);
    }

    fn type_is_callable(ty: &Type) -> bool {
        match ty {
            Type::Fn { .. } => true,
            Type::UnresolvedNamed(n) => {
                n.starts_with("func(") || n.starts_with('|') || n.starts_with("Sequence<")
            }
            Type::UnresolvedGeneric { name, .. } => name == "Sequence",
            _ => false,
        }
    }

    /// SL3: does this name outlive the call, so a returned closure may borrow it?
    ///
    /// A lent parameter does. The caller holds the value across the call and
    /// keeps holding it after — that's what `param: T` and `mutate param: T`
    /// mean (PM1, PM2) — so a closure built over one is still pointing at
    /// something live when the function returns. `self` is the case the whole
    /// sequence surface rests on: `vec.filter(|u| u.active)` returns a
    /// `Sequence<T>` over a borrowed receiver, and refusing that would cost
    /// every adapter chain a `take self`.
    ///
    /// A local does not, and a `take` parameter does not either — the frame
    /// owns it and the frame is going away. Those still scope-limit the
    /// closure, and `own` is still the fix for them.
    fn outlives_this_call(&self, name: &str) -> bool {
        self.borrowed_params.contains_key(name) || self.mutate_params.contains_key(name)
    }

    fn consume_binding(&mut self, name: &str, span: Span, sink: Option<&str>) {
        // O11: a const is not the function's to give away. Every function sees
        // the same one, so a move would leave the others holding nothing —
        // which is what happened: the move was tracked per function, so the
        // same consume was an error in a body that read the const again and
        // silently fine in one that didn't (#1079).
        //
        // Only reached for a non-Copy const. A scalar, a `string` or a small
        // struct is copied into the `take` rather than given, under PM6b, and
        // never gets here.
        if self.module_consts.contains(name) {
            self.errors.push(OwnershipError {
                kind: OwnershipErrorKind::ConsumeConst {
                    name: name.to_string(),
                    sink: sink.map(str::to_string),
                },
                span,
            });
            return;
        }
        if let Some(&(declared_at, is_mutate)) = self.borrowed_params.get(name) {
            self.errors.push(OwnershipError {
                kind: OwnershipErrorKind::ConsumeBorrowedParam {
                    name: name.to_string(),
                    declared_at,
                    is_mutate,
                    sink: sink.map(str::to_string),
                },
                span,
            });
            return;
        }
        if let Some(&closure_at) = self.borrowed_captures.get(name) {
            self.errors.push(OwnershipError {
                kind: OwnershipErrorKind::ConsumeBorrowedCapture {
                    name: name.to_string(),
                    closure_at,
                },
                span,
            });
            return;
        }
        self.bindings.insert(name.to_string(), BindingState::Moved { at: span });
    }

    /// Name of the type a receiver expression evaluates to. Handles resolved
    /// (`Named`/`Generic`) and still-unresolved (`UnresolvedNamed`/
    /// `UnresolvedGeneric`) forms. Returns the base name without generic params.
    fn receiver_type_name(&self, object: &Expr) -> Option<String> {
        let ty = self.program.node_types.get(&object.id)?;
        let id = match ty {
            Type::Named(id) => *id,
            Type::Generic { base, .. } => *base,
            Type::UnresolvedNamed(name) => {
                return Some(name.split('<').next().unwrap_or(name).to_string());
            }
            Type::UnresolvedGeneric { name, .. } => return Some(name.clone()),
            _ => return None,
        };
        match self.program.types.get(id)? {
            rask_types::TypeDef::Struct { name, .. }
            | rask_types::TypeDef::Enum { name, .. }
            | rask_types::TypeDef::Interface { name, .. }
            | rask_types::TypeDef::Union { name, .. }
            | rask_types::TypeDef::NominalAlias { name, .. }
            | rask_types::TypeDef::Primitive { name, .. } => {
                Some(name.split('<').next().unwrap_or(name).to_string())
            }
        }
    }

    /// Parameter modes of a user method on the receiver's type, if resolvable.
    fn method_param_modes(&self, object: &Expr, method_name: &str) -> Option<Vec<ParamMode>> {
        let ty = self.program.node_types.get(&object.id)?;
        let id = match ty {
            Type::Named(id) => *id,
            Type::Generic { base, .. } => *base,
            _ => return None,
        };
        let methods = match self.program.types.get(id)? {
            rask_types::TypeDef::Struct { methods, .. } => methods,
            rask_types::TypeDef::Enum { methods, .. } => methods,
            _ => return None,
        };
        methods
            .iter()
            .find(|m| m.name == method_name)
            .map(|m| m.params.iter().map(|(_, mode)| *mode).collect())
    }

    /// T1: a channel `send` transfers ownership of the sent value.
    /// `send` is a builtin (no user MethodSig), so recognize it structurally.
    fn is_channel_send(&self, object: &Expr, method_name: &str, call_span: Span) -> bool {
        if method_name != "send" {
            return false;
        }
        // Type checker recorded this call as a `Sender.send` — authoritative even
        // when inference left the receiver as a bare var in node_types (T1).
        if self.program.channel_send_sites.contains(&call_span) {
            return true;
        }
        // Fallback: receiver type is concrete. TypeDef names carry their generic
        // params ("Sender<T>"); match the base.
        self.receiver_type_name(object)
            .map(|n| n.split('<').next() == Some("Sender"))
            .unwrap_or(false)
    }

    /// L1/ER42: a type-name annotation refers to a transitively-linear type.
    /// Strips generic args ("File<T>" → "File") and asks the type table.
    ///
    /// An optional is stripped first. A `Conn?` is still a connection that has to
    /// be closed on the path where it exists — but `get_type_id("Conn?")` finds
    /// nothing, so the annotation said "not a resource" and the binding was never
    /// registered. `mut maybe: Conn? = Conn { … }` then dropped it with no
    /// diagnostic at all (#827).
    fn is_resource_type_name(&self, ty_name: &str) -> bool {
        let name = Self::strip_optional(ty_name);
        let base = name.split('<').next().unwrap_or(name);
        if let Some(id) = self.program.types.get_type_id(base.trim()) {
            return self.program.types.is_transitive_resource_by_id(id);
        }
        false
    }

    /// `Conn?` and `Conn or none` → `Conn`. Repeats, so `Conn??` gets there too.
    fn strip_optional(ty_name: &str) -> &str {
        let mut name = ty_name.trim();
        loop {
            let next = name
                .strip_suffix('?')
                .or_else(|| name.strip_suffix(" or none"))
                .map(str::trim);
            match next {
                Some(inner) if inner != name => name = inner,
                _ => return name,
            }
        }
    }

    /// L1/ER42: a `Type` value must be consumed exactly once — `@resource`
    /// directly, or through nested fields/variants/tuples/optionals.
    ///
    /// Uses `is_linear_value`, not `type_is_transitive_resource`: the latter
    /// recurses into *every* generic arg and so would treat `Link<File>` as
    /// linear. A link is a copyable reference, so binding one must not demand
    /// consumption. (No container takes a linear value at all now —
    /// `mem.resource-types/RC1`–RC3 reject it at the type.)

    /// Why the field walk can't reach a resource inside this type — `None` when it
    /// can, i.e. the type is a plain struct with named fields. Every shape that
    /// holds values without giving them a field path is listed, so a type matching
    /// none of them is reported as unrecognised rather than joining the bucket in
    /// silence.
    fn opaque_resource_shape(&self, ty: &Type) -> Option<String> {
        if ty.as_option().is_some() {
            return Some("an optional".to_string());
        }
        if let Type::Tuple(_) = ty {
            return Some("a tuple".to_string());
        }
        if let Type::UnresolvedGeneric { name, args } = ty {
            if !args.is_empty() {
                let base = name.split('<').next().unwrap_or(name);
                return Some(Self::container_shape(base));
            }
        }
        let (id, args) = match ty {
            Type::Named(id) => (*id, Vec::new()),
            Type::Generic { base, args } => (*base, args.clone()),
            _ => return Some("a type the checker does not recognise".to_string()),
        };
        let name = self.program.types.type_name(id);
        let base = name.split('<').next().unwrap_or(&name).to_string();
        if !args.is_empty() {
            return Some(Self::container_shape(&base));
        }
        match self.program.types.get(id) {
            Some(rask_types::TypeDef::Enum { .. }) => Some("an enum payload".to_string()),
            Some(rask_types::TypeDef::Union { .. }) => Some("a union member".to_string()),
            Some(rask_types::TypeDef::Struct { .. }) => None,
            _ => Some("a type the checker does not recognise".to_string()),
        }
    }

    fn container_shape(base: &str) -> String {
        match base {
            "Vec" => "a `Vec` element".to_string(),
            "Map" => "a `Map` entry".to_string(),
            "Set" => "a `Set` element".to_string(),
            other => format!("a `{}` payload", other),
        }
    }

    /// The type carries `@resource` itself, as opposed to merely containing one.
    fn is_directly_resource(&self, ty: &Type) -> bool {
        let id = match ty {
            Type::Named(id) => *id,
            Type::Generic { base, .. } => *base,
            _ => return false,
        };
        matches!(
            self.program.types.get(id),
            Some(rask_types::TypeDef::Struct { is_resource: true, .. })
        )
    }

    fn type_is_resource(&self, ty: &Type) -> bool {
        self.program.types.is_linear_value(ty)
    }

    /// Is a value of this type, read out of an aggregate by a pattern, the
    /// aggregate's rather than the reader's?
    ///
    /// A `Heap<T>` is. Storing one in a field or an enum payload consumed it
    /// (mem.heap/HP4) and the aggregate's release gives the block back — which
    /// is what makes `Cons(i64, Heap<List>)` free its whole chain. So
    /// `match l { Cons(head, rest) => … }` borrows `rest`: it owes nothing, and
    /// `Cons(_, rest)` discards nothing.
    ///
    /// A `@resource` is not. There are no destructors, so nothing but an
    /// explicit consume ever closes one, and matching it out of an enum is the
    /// last chance to.
    fn pattern_payload_is_borrowed(&self, ty: &Type) -> bool {
        ty.heap_payload().is_some()
    }

    /// Whether an expression's inferred type is transitively linear.
    fn expr_is_resource_type(&self, expr: &Expr) -> bool {
        if let Some(ty) = self.program.node_types.get(&expr.id) {
            if self.type_is_resource(ty) {
                return true;
            }
        }
        // `let c = File.open(p) catch e => { … }` binds the ok side, and the
        // node the checker typed is the call, not the fallback — so the type of
        // the `catch` itself was nothing and the obligation was never created.
        // A resource that arrives through a fallback is still a resource (#882).
        match &expr.kind {
            ExprKind::Catch { value, .. } => {
                let Some(ty) = self.program.node_types.get(&value.id) else { return false };
                match ty {
                    Type::Result { ok, .. } => self.type_is_resource(ok),
                    _ => self.type_is_resource(ty),
                }
            }
            _ => false,
        }
    }

    /// Scan ensure body for resource references and mark them.
    fn mark_ensure_resources(&mut self, stmt: &Stmt, ensure_span: Span) {
        match &stmt.kind {
            StmtKind::Expr(expr) => {
                self.mark_ensure_expr(expr, ensure_span);
            }
            _ => {}
        }
    }

    /// Register a resource as ensure-committed, recording where.
    fn register_ensure(&mut self, name: &str, ensure_span: Span) {
        self.ensure_registered.insert(name.to_string());
        self.ensure_spans.entry(name.to_string()).or_insert(ensure_span);
    }

    /// A `return` hands its value to the caller, which consumes any resource in
    /// it (mem.linear/L2).
    ///
    /// Reading a name isn't a move, so `return conn` left the binding Owned and
    /// scope exit reported it as leaked — with a suggested fix (`close()` it
    /// first) that would hand the caller a dead connection. There was no version
    /// of the function that satisfied the checker and still worked (#792).
    ///
    /// Aggregates count, because handing back `(request, responder)` is what the
    /// flagship `Responder` design does. A projection doesn't: `return conn.fd`
    /// reads a field and leaves the resource behind, which really is a leak.
    fn consume_returned_resources(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Ident(name) => {
                if self.resource_bindings.contains(name)
                    && matches!(self.bindings.get(name), Some(BindingState::Owned))
                {
                    self.bindings.insert(name.clone(), BindingState::Moved { at: expr.span });
                }
            }
            ExprKind::Tuple(elems) | ExprKind::Array(elems) => {
                for e in elems {
                    self.consume_returned_resources(e);
                }
            }
            ExprKind::StructLit { fields, spread, .. } => {
                for f in fields {
                    self.consume_returned_resources(&f.value);
                }
                if let Some(s) = spread {
                    self.consume_returned_resources(s);
                }
            }
            // `Holder.Full(conn)` — an enum variant carrying a payload, which
            // parses as a method call on the type name. A free function call
            // isn't here on purpose: passing a resource to one already moves it
            // through the argument path, which knows the parameter's mode and
            // this doesn't.
            ExprKind::MethodCall { object, method, args, .. }
                if self.names_a_variant(object, method) =>
            {
                for arg in args {
                    self.consume_returned_resources(&arg.expr);
                }
            }
            // The value a branch produces is the value returned.
            ExprKind::If { then_branch, else_branch, .. } => {
                self.consume_returned_resources(then_branch);
                if let Some(e) = else_branch {
                    self.consume_returned_resources(e);
                }
            }
            _ => {}
        }
    }

    /// Is `object.method(…)` an enum variant construction rather than a call?
    /// Read off the type table, which is authoritative for type names — a
    /// variant can share its name with one from another enum.
    fn names_a_variant(&self, object: &Expr, method: &str) -> bool {
        let ExprKind::Ident(name) = &object.kind else { return false };
        let Some(type_id) = self.program.types.get_type_id(name) else { return false };
        matches!(
            self.program.types.get(type_id),
            Some(rask_types::TypeDef::Enum { variants, .. })
                if variants.iter().any(|(v, _)| v == method)
        )
    }

    /// Extract resource names from ensure expressions (e.g., `file.close()`).
    fn mark_ensure_expr(&mut self, expr: &Expr, ensure_span: Span) {
        match &expr.kind {
            ExprKind::MethodCall { object, .. } => {
                match &object.kind {
                    ExprKind::Ident(name) => {
                        if self.resource_bindings.contains(name) {
                            self.register_ensure(name, ensure_span);
                        }
                    }
                    // `ensure w.conn.close()` — the receiver is a field, so
                    // what it commits is that field's debt, exactly as the
                    // direct call pays it. Left out, a holder's field could be
                    // consumed but never *ensured*, which L7 needs (#828's
                    // per-field debts are what this walks).
                    ExprKind::Field { .. } => {
                        let (root, path) = Self::extract_root_and_fields(object);
                        if let (Some(root), Some(path)) = (root, path) {
                            self.pay_field_debt(&root, &path);
                        }
                    }
                    _ => {}
                }
            }
            ExprKind::Call { func, args } => {
                // Check args for resource identifiers
                for arg in args {
                    if let ExprKind::Ident(name) = &arg.expr.kind {
                        if self.resource_bindings.contains(name) {
                            self.register_ensure(name, ensure_span);
                        }
                    }
                }
                self.mark_ensure_expr(func, ensure_span);
            }
            _ => {}
        }
    }

    /// At closure/spawn exit, emit errors for unconsumed @resource captures.
    fn check_resource_consumption_in_closure(&mut self, span: Span, context: &str) {
        let mut names: Vec<String> = self.resource_bindings.iter().cloned().collect();
        names.sort();
        for name in names {
            // C4: an ensured resource consumed on some paths but not all is a
            // compile error — its cleanup can't be decided statically.
            if self.ensure_registered.contains(&name) {
                if let Some(BindingState::MaybeMoved { at }) = self.bindings.get(&name) {
                    let consumed_at = *at;
                    let ensure_at = self.ensure_spans.get(&name).copied().unwrap_or(span);
                    self.errors.push(OwnershipError {
                        kind: OwnershipErrorKind::EnsureMaybeConsumed {
                            name: name.clone(),
                            ensure_at,
                            consumed_at,
                        },
                        span,
                    });
                }
                continue;
            }
            if !matches!(self.bindings.get(&name), Some(BindingState::Moved { .. })) {
                self.errors.push(OwnershipError {
                    kind: OwnershipErrorKind::ResourceNotConsumedInClosure {
                        name,
                        context: context.to_string(),
                    },
                    span,
                });
            }
        }
    }

    /// PM2: a `mutate` parameter is still there when the call returns, so one
    /// that was consumed has to have been replaced on every path.
    ///
    /// Consuming one is legitimate — `out.push(b.build()); b = StringBuilder.new()`
    /// is what exclusive access is for. Consuming it and putting nothing back is
    /// not: the caller reads a hole, and nothing checked for it (#815). A
    /// reassignment sets the binding back to `Owned`, so the state at exit is the
    /// whole test.
    fn check_mutate_params_refilled(&mut self, span: Span) {
        let mut names: Vec<(String, Span)> =
            self.mutate_params.iter().map(|(n, s)| (n.clone(), *s)).collect();
        names.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, declared_at) in names {
            // Every `return` runs this as well as the closing brace, so without a
            // dedupe one empty slot reports once per exit.
            if !self.exit_reported.insert(format!("mutate:{}", name)) {
                continue;
            }
            let (consumed_at, maybe) = match self.bindings.get(&name) {
                Some(BindingState::Moved { at }) => (*at, false),
                Some(BindingState::MaybeMoved { at }) => (*at, true),
                _ => continue,
            };
            self.errors.push(OwnershipError {
                kind: OwnershipErrorKind::MutateParamLeftEmpty {
                    name,
                    consumed_at,
                    declared_at,
                    maybe,
                },
                span,
            });
        }
    }

    /// `Heap(expr)` allocates, so the binding it initializes is linear (L1–L6,
    /// same rules as `@resource`).
    ///
    /// Every payload. The scalar exception used to live here — a `Heap<i32>`
    /// really was an `i32`, so there was nothing to free (#819) — and it went
    /// when `Heap(…)` stopped skipping the allocation for payloads that fit the
    /// slot (#1256). A block is a block whatever is in it.
    fn track_owned_binding(&mut self, name: &str, init: &Expr) {
        if !matches!(&init.kind, ExprKind::Unary { op: UnaryOp::Heap, .. }) {
            return;
        }
        self.resource_bindings.insert(name.to_string());
        self.owned_bindings.insert(name.to_string());
    }

    /// Consume any `own` box stored into an aggregate being built here.
    ///
    /// The same walk `consume_returned_resources` does, restricted to owned
    /// bindings: a box in a struct field, a tuple or array element, or an enum
    /// variant payload belongs to the aggregate now, so the binding it came from
    /// has given it away.
    fn consume_owned_into_aggregate(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Ident(name) => {
                // A `@resource` counts too. L5 says assigning to another binding
                // consumes, and a field is another binding — the aggregate takes
                // on the debt, reported as `h.c`. Leaving the source binding owing
                // as well made the program unwritable: consuming `h` satisfies
                // `h.c` and there is nothing left for `c` to be consumed by (#882).
                if self.owned_bindings.contains(name) || self.resource_bindings.contains(name) {
                    self.consume_binding(name, expr.span, None);
                }
            }
            ExprKind::Tuple(elems) | ExprKind::Array(elems) => {
                for e in elems {
                    self.consume_owned_into_aggregate(e);
                }
            }
            ExprKind::StructLit { fields, spread, .. } => {
                for f in fields {
                    self.consume_owned_into_aggregate(&f.value);
                }
                if let Some(s) = spread {
                    self.consume_owned_into_aggregate(s);
                }
            }
            ExprKind::MethodCall { object, method, args, .. }
                if self.names_a_variant(object, method) =>
            {
                for arg in args {
                    self.consume_owned_into_aggregate(&arg.expr);
                }
            }
            _ => {}
        }
    }


    /// At function exit, emit errors for unconsumed @resource bindings, and C4
    /// errors for ensured resources whose consumption isn't statically definite.

    /// `if x? as c` where the payload is a resource: register `c` and take the
    /// obligation off `x`. Returns the binding name when it did.
    ///
    /// OPT19 calls `c` "the payload read out of the scrutinee". For a Copy payload
    /// that reads as a copy, which is why writing through `c` is rejected. A linear
    /// payload can't be copied, so the only coherent reading there is a move: `c`
    /// holds the resource inside the branch and `x` doesn't hold it afterwards —
    /// on the absent path there was nothing to hold (#827).
    fn optional_payload_resource(&mut self, cond: &Expr) -> Option<String> {
        let ExprKind::IsPresent { expr: inner, binding: Some(name) } = &cond.kind else {
            return None;
        };
        let payload = self
            .program
            .node_types
            .get(&inner.id)
            .and_then(|ty| ty.as_option())?
            .clone();
        if !self.type_is_resource(&payload) {
            return None;
        }
        // The scrutinee gave the payload away. A call result had no binding to
        // charge in the first place.
        if let ExprKind::Ident(source) = &inner.kind {
            if self.resource_bindings.contains(source) {
                self.bindings.insert(source.clone(), BindingState::Moved { at: cond.span });
            }
        }
        self.bindings.insert(name.clone(), BindingState::Owned);
        self.binding_types.insert(name.clone(), payload);
        self.resource_bindings.insert(name.clone());
        Some(name.clone())
    }

    /// The `? as` binding lives only inside the branch, so its obligation is
    /// checked there rather than at function exit — otherwise the branch merge
    /// would leave it maybe-moved and report a leak on a program that closed it.
    fn check_present_binding_consumed(&mut self, name: &str, span: Span) {
        let consumed = matches!(self.bindings.get(name), Some(BindingState::Moved { .. }))
            || self.ensure_registered.contains(name);
        if !consumed {
            self.errors.push(OwnershipError {
                kind: OwnershipErrorKind::ResourceNotConsumed { name: name.to_string() },
                span,
            });
        }
        self.resource_bindings.remove(name);
        self.bindings.remove(name);
    }


    /// Register a resource binding, recording per-field debts when the value
    /// isn't a resource itself but holds one.
    ///
    /// A `@resource` value owes itself and there is nothing to break down. A
    /// plain struct that happens to carry one owes exactly those fields, and
    /// consuming `p.a` should pay that debt and no other (#828).
    fn register_resource_binding(&mut self, name: &str, ty: Option<&Type>) {
        self.resource_bindings.insert(name.to_string());
        let Some(ty) = ty else { return };
        let paths = self.resource_field_paths(ty, 0);
        if !paths.is_empty() {
            self.resource_field_debts.insert(name.to_string(), paths);
            return;
        }
        if !self.is_directly_resource(ty) {
            if let Some(shape) = self.opaque_resource_shape(ty) {
                self.coarse_resources.insert(name.to_string(), shape);
            }
        }
    }

    /// The field paths inside `ty` that owe consumption, or empty when `ty` owes
    /// as a whole.
    ///
    /// Empty for an `@resource` type — the value *is* the obligation — and empty
    /// for anything the walk can't name a path into: a tuple, an optional, an
    /// enum payload. Those keep the whole-value obligation, which is the
    /// conservative answer.
    fn resource_field_paths(&self, ty: &Type, depth: u32) -> Vec<Vec<String>> {
        if depth > 8 {
            return Vec::new();
        }
        let Some(id) = self.named_type_id(ty) else { return Vec::new() };
        let Some(rask_types::TypeDef::Struct { fields, is_resource, .. }) =
            self.program.types.get(id)
        else {
            return Vec::new();
        };
        // The value is the obligation; there is nothing to split.
        if *is_resource {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (fname, fty) in fields {
            if !self.program.types.is_linear_value(fty) {
                continue;
            }
            let inner = self.resource_field_paths(fty, depth + 1);
            if inner.is_empty() {
                out.push(vec![fname.clone()]);
            } else {
                for mut path in inner {
                    let mut full = vec![fname.clone()];
                    full.append(&mut path);
                    out.push(full);
                }
            }
        }
        out
    }

    /// The declared type a written name refers to, optional stripped — the shape
    /// `resource_field_paths` needs, which `type_from_name` doesn't give for a
    /// declared struct (it answers `UnresolvedGeneric` for anything with `<`).
    fn declared_type_from_name(&self, ty_name: &str) -> Option<Type> {
        let name = Self::strip_optional(ty_name);
        let base = name.split('<').next().unwrap_or(name).trim();
        self.program.types.get_type_id(base).map(Type::Named)
    }

    /// The `TypeId` behind a named or unresolved-named type.
    fn named_type_id(&self, ty: &Type) -> Option<rask_types::TypeId> {
        match ty {
            Type::Named(id) => Some(*id),
            Type::UnresolvedNamed(name) => {
                let base = name.split('<').next().unwrap_or(name).trim();
                self.program.types.get_type_id(base)
            }
            Type::Generic { base, .. } => Some(*base),
            _ => None,
        }
    }

    /// A `take self` call on `root.a.b` pays that debt. Returns true when it
    /// matched one, so the caller knows not to fall through to the whole-binding
    /// consumption.
    fn pay_field_debt(&mut self, root: &str, path: &[String]) -> bool {
        let Some(debts) = self.resource_field_debts.get_mut(root) else {
            return false;
        };
        let before = debts.len();
        // Consuming `w.inner` pays for everything under it, since the value that
        // held them is gone.
        debts.retain(|d| !d.starts_with(path));
        if debts.len() == before {
            return false;
        }
        if debts.is_empty() {
            // Everything it owed is paid, so the binding itself is discharged.
            self.resource_bindings.remove(root);
            self.resource_field_debts.remove(root);
        }
        true
    }

    fn check_resource_consumption(&mut self, span: Span) {
        let names: Vec<String> = {
            let mut v: Vec<String> = self.resource_bindings.iter().cloned().collect();
            v.sort();
            v
        };
        self.check_resource_names(names, span);
    }

    /// The obligations a block introduced, judged where that block ends.
    ///
    /// A resource declared inside a block goes out of scope at the closing brace,
    /// so that's where "was it consumed" has an answer. Deferring it to the
    /// function's exit worked for a plain nested block — the binding state
    /// survives — but not for a branch: the merge drops branch-local names, so at
    /// the function's exit `c` was absent from `bindings` entirely, "absent" isn't
    /// `Moved`, and a resource opened *and* closed inside an `if` was reported as
    /// leaked.
    ///
    /// Which names belong to this block comes from a snapshot taken on entry, not
    /// from `binding_decl_blocks`. Block ids are depth-like rather than unique —
    /// a nested block is numbered with the depth its enclosing block was at — so
    /// a binding declared in the enclosing block matches the nested block's id,
    /// and an id comparison judged it early, while it was still `Moved` inside the
    /// branch. That silently *dropped* a real maybe-consumed leak.
    fn check_block_resources(&mut self, entered_with: &HashSet<String>, span: Span) {
        let mut names: Vec<String> = self
            .resource_bindings
            .iter()
            .filter(|n| !entered_with.contains(*n))
            .cloned()
            .collect();
        if names.is_empty() {
            return;
        }
        names.sort();
        self.check_resource_names(names.clone(), span);
        // Judged, so the function-exit pass must not judge them again — by then
        // the state they were judged on is gone.
        for name in names {
            self.resource_bindings.remove(&name);
        }
    }

    fn check_resource_names(&mut self, names: Vec<String>, span: Span) {
        for name in names {
            if self.ensure_registered.contains(&name) {
                // C3/C4: ensure commits consumption. At scope exit the receiver
                // must be definitely consumed (ensure cancelled) or definitely
                // not (ensure runs) — never maybe. Maybe-consumed is a C4 error.
                if let Some(BindingState::MaybeMoved { at }) = self.bindings.get(&name) {
                    let consumed_at = *at;
                    let ensure_at = self.ensure_spans.get(&name).copied().unwrap_or(span);
                    self.errors.push(OwnershipError {
                        kind: OwnershipErrorKind::EnsureMaybeConsumed {
                            name: name.clone(),
                            ensure_at,
                            consumed_at,
                        },
                        span,
                    });
                }
                continue;
            }
            // Not registered with ensure: must be consumed (Moved) before exit.
            if !matches!(self.bindings.get(&name), Some(BindingState::Moved { .. })) {
                if !self.exit_reported.insert(name.clone()) {
                    continue;
                }
                // A holder that owes named fields is reported by field, so the
                // message points at what's actually still open rather than at a
                // binding with no `close()` of its own (#828).
                if let Some(debts) = self.resource_field_debts.get(&name) {
                    let mut paths: Vec<String> = debts
                        .iter()
                        .map(|d| format!("{}.{}", name, d.join(".")))
                        .collect();
                    paths.sort();
                    for path in paths {
                        self.errors.push(OwnershipError {
                            kind: OwnershipErrorKind::ResourceNotConsumed { name: path },
                            span,
                        });
                    }
                    continue;
                }
                let kind = if self.owned_bindings.contains(&name) {
                    OwnershipErrorKind::OwnedNotConsumed { name }
                } else if let Some(where_) = self.coarse_resources.get(&name).cloned() {
                    // The walk found a resource it had no field path to, so the
                    // obligation fell back to the whole binding. Saying which shape
                    // stopped it is what keeps that from reading like a bug.
                    OwnershipErrorKind::ResourceNotConsumedOpaque { name, where_ }
                } else {
                    OwnershipErrorKind::ResourceNotConsumed { name }
                };
                self.errors.push(OwnershipError { kind, span });
            }
        }
    }
}

/// Run ownership analysis on a typed program.
pub fn check_ownership(program: &TypedProgram, decls: &[Decl]) -> OwnershipResult {
    let checker = OwnershipChecker::new(program);
    checker.check(decls)
}

/// Ownership analysis that can also read the stdlib's parameter modes.
pub fn check_ownership_with_stdlib(
    program: &TypedProgram,
    decls: &[Decl],
    stdlib_decls: &[Decl],
) -> OwnershipResult {
    let checker = OwnershipChecker::new(program);
    checker.check_with_signatures(decls, stdlib_decls)
}

/// Every `Name<Args…>` reachable inside a type, including nested ones.
fn collect_generic_instances(
    ty: &Type,
    out: &mut Vec<(rask_types::TypeId, Vec<rask_types::GenericArg>)>,
) {
    use rask_types::GenericArg;
    match ty {
        Type::Generic { base, args } => {
            out.push((*base, args.clone()));
            for a in args {
                if let GenericArg::Type(t) = a {
                    collect_generic_instances(t, out);
                }
            }
        }
        Type::UnresolvedGeneric { args, .. } => {
            for a in args {
                if let GenericArg::Type(t) = a {
                    collect_generic_instances(t, out);
                }
            }
        }
        Type::Result { ok, err } => {
            collect_generic_instances(ok, out);
            collect_generic_instances(err, out);
        }
        Type::Array { elem, .. } => collect_generic_instances(elem, out),
        Type::RawPtr(inner) => collect_generic_instances(inner, out),
        Type::Tuple(elems) | Type::Union(elems) => {
            for e in elems {
                collect_generic_instances(e, out);
            }
        }
        Type::Fn { params, ret } => {
            for p in params {
                collect_generic_instances(p, out);
            }
            collect_generic_instances(ret, out);
        }
        _ => {}
    }
}

/// Replace a generic struct's type parameters with the instantiation's
/// arguments. Parameters reach here as `UnresolvedNamed("T")` — the checker
/// never resolves them to a TypeId because there's nothing to resolve to.
fn substitute_params(ty: &Type, subst: &HashMap<&str, &Type>) -> Type {
    use rask_types::GenericArg;
    match ty {
        Type::UnresolvedNamed(name) => match subst.get(name.as_str()) {
            Some(t) => (*t).clone(),
            None => ty.clone(),
        },
        Type::Result { ok, err } => Type::Result {
            ok: Box::new(substitute_params(ok, subst)),
            err: Box::new(substitute_params(err, subst)),
        },
        Type::Array { elem, len } => Type::Array {
            elem: Box::new(substitute_params(elem, subst)),
            len: *len,
        },
        Type::RawPtr(inner) => Type::RawPtr(Box::new(substitute_params(inner, subst))),
        Type::Tuple(elems) => {
            Type::Tuple(elems.iter().map(|e| substitute_params(e, subst)).collect())
        }
        Type::Generic { base, args } => Type::Generic {
            base: *base,
            args: args
                .iter()
                .map(|a| match a {
                    GenericArg::Type(t) => {
                        GenericArg::Type(Box::new(substitute_params(t, subst)))
                    }
                    other => other.clone(),
                })
                .collect(),
        },
        _ => ty.clone(),
    }
}
