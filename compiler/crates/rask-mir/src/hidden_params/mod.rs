// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Hidden parameter compiler pass (comp.hidden-params).
//!
//! Desugars `using` clauses into explicit hidden function parameters.
//! Runs after type checking, before monomorphization.
//!
//! Operations:
//! 1. Collect context requirements from explicit `using` clauses
//! 2. Build call graph and propagate requirements (CC5)
//! 3. Infer contexts for private functions from handle field access (CC7)
//! 4. Resolve contexts at call sites using scope search (CC4)
//! 5. Detect ambiguity errors (CC8)
//! 6. Rewrite signatures + call sites + using blocks

mod callgraph;
mod collect;
mod resolve;
mod rewrite;

use std::collections::{HashMap, HashSet};

use rask_ast::decl::{ContextClause, Decl, DeclKind};
use rask_ast::expr::{Expr, ExprKind};
use rask_ast::NodeId;
use rask_diagnostics::Diagnostic;
use rask_types::Type;

// ── Types ───────────────────────────────────────────────────────────────

/// A context requirement derived from a `using` clause.
#[derive(Debug, Clone)]
pub(crate) struct ContextReq {
    /// Hidden parameter name: `__ctx_pool_Player`, etc.
    pub param_name: String,
    /// Type string emitted into the AST parameter: `&Pool<Player>`.
    pub param_type: String,
    /// The pool type, resolved through the type table so it compares equal to
    /// the types recorded for locals/params/fields (`Pool<Player>`).
    pub clause_type: Type,
    /// Named alias from `using players: Pool<Player>`
    pub alias: Option<String>,
}

/// A pool found in scope during CC4 resolution.
#[derive(Debug, Clone)]
pub(crate) struct ScopePool {
    /// Variable name in user code: `players`, `self.players`, etc.
    pub var_name: String,
    /// Where it came from (for priority ordering / error messages).
    pub source: PoolSource,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PoolSource {
    Local,
    Parameter,
    SelfField,
    UsingClause,
}

/// Qualified function name for call graph: "damage" or "Player.take_damage".
pub(crate) type FuncName = String;

/// Information about a function for context resolution. Types are resolved
/// through the type table so they compare equal regardless of how a name was
/// spelled at each site.
#[derive(Debug, Clone)]
pub(crate) struct FuncInfo {
    /// Explicit context requirements (from `using` clauses or propagation).
    pub reqs: Vec<ContextReq>,
    /// Parameters: (name, type).
    pub params: Vec<(String, Type)>,
    /// Fields of `self` type (if a method): (field name, type).
    pub self_fields: Vec<(String, Type)>,
    /// Local variable declarations: (name, type).
    pub locals: Vec<(String, Type)>,
}

// ── Public API ──────────────────────────────────────────────────────────

/// Run the hidden parameter pass on a set of declarations.
///
/// Mutates the AST in place:
/// - Functions with `using` clauses gain hidden `__ctx_*` parameters
/// - Call sites to those functions gain hidden arguments
/// - `using Multitasking { }` blocks become context construction + teardown
///
/// Returns any diagnostics the pass raised (e.g. CC8 ambiguity). A non-empty
/// error list means the caller must stop before monomorphization.
///
/// Without a TypedProgram, falls back to name-based call-graph keying (still
/// correct for free functions; method callees can't be keyed consistently).
pub fn desugar_hidden_params(decls: &mut [Decl]) -> Vec<Diagnostic> {
    desugar_hidden_params_with_types(decls, None)
}

/// Run the hidden parameter pass with the typed program.
///
/// The typed program supplies the recorded call targets (CALL6): each call
/// resolves to a structured id, so method callees key by `Type.method` exactly
/// as their declarations do — no reconstruction from a bare method name.
///
/// Returns diagnostics raised during the pass (CC8 ambiguity today).
pub fn desugar_hidden_params_with_types(
    decls: &mut [Decl],
    typed: Option<&rask_types::TypedProgram>,
) -> Vec<Diagnostic> {
    let mut pass = HiddenParamPass::new(typed);
    pass.run(decls);
    pass.diagnostics
}

// ── Pass Implementation ─────────────────────────────────────────────────

pub(crate) struct HiddenParamPass<'a> {
    /// Function name → context requirements (from explicit using clauses).
    pub func_contexts: HashMap<FuncName, Vec<ContextReq>>,
    /// Function name → full info (params, locals, self fields).
    pub func_info: HashMap<FuncName, FuncInfo>,
    /// Call graph: caller → callees (by function name).
    pub call_graph: HashMap<FuncName, HashSet<FuncName>>,
    /// Functions that are public (context propagation stops here).
    pub public_funcs: HashSet<FuncName>,
    /// Struct name → field list (name, type string).
    pub struct_fields: HashMap<String, Vec<(String, String)>>,
    /// The type checker's output — recorded call targets, symbols, and type
    /// table. The single source of truth for which function a call resolves to.
    pub typed: Option<&'a rask_types::TypedProgram>,
    /// Diagnostics raised during the pass (CC8 ambiguity, CC10 closures).
    pub diagnostics: Vec<Diagnostic>,
    /// CC10: when rewriting a storable closure's body, its own pool-typed
    /// parameters. `Some` means contexts must resolve from these — the closure
    /// can outlive the enclosing pool scope, so it can't inherit ambient ones.
    pub storable_closure: Option<Vec<(String, Type)>>,
    /// In-scope `for` loop variables → their element type. The checker leaves a
    /// loop variable's type an inference var, so uses of it inside the body have
    /// no recorded type; this recovers it structurally from the iterable so the
    /// `h.field` handle-deref rewrite can fire on a loop variable.
    pub loop_var_types: HashMap<String, Type>,
    /// Fresh NodeId counter (high range to avoid parser collisions).
    pub next_id: u32,
}

impl<'a> HiddenParamPass<'a> {
    pub fn new(typed: Option<&'a rask_types::TypedProgram>) -> Self {
        Self {
            func_contexts: HashMap::new(),
            func_info: HashMap::new(),
            call_graph: HashMap::new(),
            public_funcs: HashSet::new(),
            struct_fields: HashMap::new(),
            typed,
            diagnostics: Vec::new(),
            storable_closure: None,
            loop_var_types: HashMap::new(),
            next_id: 2_000_000,
        }
    }

    /// Element type of an iterable expression: `Vec<T>`/`Slice<T>` → `T`,
    /// fixed arrays → their element. Used to type `for` loop variables the
    /// checker left as inference vars.
    pub fn iterable_elem_type(&self, iter: &Expr) -> Option<Type> {
        use rask_types::GenericArg;
        let first_type_arg = |args: &[GenericArg]| match args.first()? {
            GenericArg::Type(t) => Some((**t).clone()),
            _ => None,
        };
        match self.node_ty(iter.id)? {
            Type::Generic { base, args } => {
                let name = self.typed?.types.type_name(base);
                match name.as_str() {
                    "Vec" | "Slice" => first_type_arg(&args),
                    _ => None,
                }
            }
            Type::UnresolvedGeneric { name, args } if name == "Vec" || name == "Slice" => {
                first_type_arg(&args)
            }
            Type::Slice(e) => Some(*e),
            Type::Array { elem, .. } => Some(*elem),
            _ => None,
        }
    }

    /// Handle element type for an identifier, first via its node type, then via
    /// a tracked `for` loop variable (the checker leaves loop vars untyped).
    pub fn ident_handle_elem(&self, name: &str, id: NodeId) -> Option<Type> {
        self.node_handle_elem(id).or_else(|| {
            self.loop_var_types
                .get(name)
                .and_then(|t| self.handle_elem_of_type(t))
        })
    }

    /// Parse a source type string, resolving names through the type table so the
    /// result compares equal to types recorded elsewhere. A leading `&` (hidden
    /// context params) is stripped — the backend has no reference types.
    pub fn parse_ty(&self, s: &str) -> Option<Type> {
        let typed = self.typed?;
        let ty = rask_types::parse_type_string(s.trim_start_matches('&'), &typed.types).ok()?;
        Some(self.canonical_type(&ty))
    }

    /// The recorded type of an expression node.
    pub fn node_ty(&self, id: NodeId) -> Option<Type> {
        let ty = self.typed?.node_types.get(&id)?;
        Some(self.canonical_type(ty))
    }

    /// The element type `T` of a `Handle<T>`, in either canonical (`Generic`) or
    /// unresolved form. `None` for anything else. `WeakHandle` is excluded — it
    /// can't auto-deref (mem.context/CC1).
    pub fn handle_elem_of_type(&self, ty: &Type) -> Option<Type> {
        use rask_types::GenericArg;
        let (name, args) = match ty {
            Type::Generic { base, args } => (self.typed?.types.type_name(*base), args.as_slice()),
            Type::UnresolvedGeneric { name, args } => (name.clone(), args.as_slice()),
            _ => return None,
        };
        if name != "Handle" {
            return None;
        }
        match args.first()? {
            GenericArg::Type(t) => Some((**t).clone()),
            _ => None,
        }
    }

    /// If the node has type `Handle<T>`, its element type `T`. Drives the
    /// `h.field` rewrite: only strong-handle field access lowers to `pool[h].field`.
    pub fn node_handle_elem(&self, id: NodeId) -> Option<Type> {
        self.handle_elem_of_type(&self.node_ty(id)?)
    }

    /// Put a type in canonical form so two spellings of the same type compare
    /// equal: every `UnresolvedNamed`/`UnresolvedGeneric` head that the type
    /// table knows becomes its `Named`/`Generic` form, recursively. The checker
    /// leaves some inner types unresolved (`Pool<UnresolvedNamed("Player")>`)
    /// while a freshly parsed annotation resolves them (`Pool<Named(70)>`);
    /// canonicalizing both sides bridges that.
    pub fn canonical_type(&self, ty: &Type) -> Type {
        use rask_types::GenericArg;
        let arg = |a: &GenericArg| match a {
            GenericArg::Type(t) => GenericArg::Type(Box::new(self.canonical_type(t))),
            other => other.clone(),
        };
        let type_id = |name: &str| self.typed.and_then(|t| t.types.get_type_id(name));
        match ty {
            Type::UnresolvedNamed(n) => type_id(n).map(Type::Named).unwrap_or_else(|| ty.clone()),
            Type::UnresolvedGeneric { name, args } => {
                let args: Vec<GenericArg> = args.iter().map(arg).collect();
                match type_id(name) {
                    Some(base) => Type::Generic { base, args },
                    None => Type::UnresolvedGeneric { name: name.clone(), args },
                }
            }
            Type::Generic { base, args } => Type::Generic {
                base: *base,
                args: args.iter().map(arg).collect(),
            },
            Type::Result { ok, err } => Type::Result {
                ok: Box::new(self.canonical_type(ok)),
                err: Box::new(self.canonical_type(err)),
            },
            Type::Tuple(elems) => Type::Tuple(elems.iter().map(|e| self.canonical_type(e)).collect()),
            Type::Slice(e) => Type::Slice(Box::new(self.canonical_type(e))),
            Type::Array { elem, len } => Type::Array {
                elem: Box::new(self.canonical_type(elem)),
                len: *len,
            },
            Type::RawPtr(e) => Type::RawPtr(Box::new(self.canonical_type(e))),
            _ => ty.clone(),
        }
    }

    /// User-facing name of a type's head, resolving `Named(id)` through the type
    /// table. Used to build readable hidden-param names (`__ctx_pool_Player`).
    pub fn type_head_name(&self, ty: &Type) -> Option<String> {
        match ty {
            Type::Named(id) => self.typed.map(|t| t.types.type_name(*id)),
            Type::UnresolvedNamed(n) => Some(n.clone()),
            _ => None,
        }
    }

    /// Render a type back to a parseable source string, resolving `Named(id)`
    /// through the type table (`Pool<Player>`, not `Pool<<type#3>>`). Used to
    /// emit hidden-param annotations that MIR lowering can resolve.
    pub fn type_to_source(&self, ty: &Type) -> String {
        use rask_types::GenericArg;
        let render_args = |args: &[GenericArg]| -> String {
            args.iter()
                .map(|a| match a {
                    GenericArg::Type(t) => self.type_to_source(t),
                    GenericArg::ConstUsize(n) => n.to_string(),
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        match ty {
            Type::Named(id) => self
                .typed
                .map(|t| t.types.type_name(*id))
                .unwrap_or_else(|| format!("{}", ty)),
            Type::UnresolvedNamed(n) => n.clone(),
            Type::UnresolvedGeneric { name, args } => {
                format!("{}<{}>", name, render_args(args))
            }
            Type::Generic { base, args } => {
                // `type_name` returns the base head (`Pool`), not the stored
                // declaration signature (`Pool<T>`), so appending the concrete
                // args renders `Pool<Player>` directly.
                let head = self
                    .typed
                    .map(|t| t.types.type_name(*base))
                    .unwrap_or_else(|| format!("{}", ty));
                format!("{}<{}>", head, render_args(args))
            }
            _ => format!("{}", ty),
        }
    }

    /// CALL6: the canonical call-graph key for the call at `call_node`, from the
    /// recorded dispatch target. Free functions key by their symbol name; methods
    /// by `Type.method` — matching how declarations are keyed. Returns `None` when
    /// no target was recorded (builtins, or no typed program), so callers fall
    /// back to name-based extraction.
    pub fn callee_key(&self, call_node: NodeId) -> Option<FuncName> {
        let typed = self.typed?;
        match typed.call_targets.get(&call_node)? {
            rask_types::Callee::Free(sym) => {
                typed.symbols.get(*sym).map(|s| s.name.clone())
            }
            rask_types::Callee::Method { type_id, method } => {
                Some(format!("{}.{}", typed.types.type_name(*type_id), method))
            }
        }
    }

    pub fn fresh_id(&mut self) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        id
    }

    fn run(&mut self, decls: &mut [Decl]) {
        // Phase 0: Collect struct field info (for self.field resolution)
        self.collect_struct_fields(decls);

        // Phase 1: Collect context requirements from explicit `using` clauses
        self.collect_contexts(decls);

        // Phase 2: Build call graph from function bodies
        callgraph::build_call_graph(self, decls);

        // Phase 3: Propagate — functions calling context-needing functions
        // also need the context if they can't resolve it locally (CC5, PUB2)
        callgraph::propagate(self);

        // Phase 3b: CC7 — infer unnamed contexts for private functions
        // that access handle fields without a `using` clause
        resolve::infer_private_contexts(self, decls);

        // Phase 4-6: Rewrite signatures, call sites, using blocks
        rewrite::rewrite_decls(self, decls);
    }

    fn collect_struct_fields(&mut self, decls: &[Decl]) {
        for decl in decls {
            if let DeclKind::Struct(s) = &decl.kind {
                let fields: Vec<(String, String)> = s
                    .fields
                    .iter()
                    .map(|f| (f.name.clone(), f.ty.clone()))
                    .collect();
                self.struct_fields.insert(s.name.clone(), fields);
            }
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

impl HiddenParamPass<'_> {
    /// Convert a ContextClause into a ContextReq, resolving the pool type
    /// through the type table. Only pool contexts reach here — runtime types
    /// (Multitasking, ThreadPool) are filtered by `collect::is_runtime_context`.
    pub(crate) fn context_clause_to_req(&self, cc: &ContextClause) -> ContextReq {
        let clause_type = self
            .parse_ty(&cc.ty)
            .unwrap_or_else(|| Type::UnresolvedNamed(cc.ty.clone()));
        let param_name = if let Some(alias) = &cc.name {
            format!("__ctx_{}", alias)
        } else {
            let elem = self.pool_elem_name(&clause_type).unwrap_or_default();
            format!("__ctx_pool_{}", elem)
        };
        ContextReq {
            param_name,
            param_type: format!("&{}", cc.ty),
            clause_type,
            alias: cc.name.clone(),
        }
    }

    /// The element name of a `Pool<T>` type (`Player`), for naming a hidden
    /// param. Resolves `Named(id)` through the type table.
    pub(crate) fn pool_elem_name(&self, pool_ty: &Type) -> Option<String> {
        let args = match pool_ty {
            Type::UnresolvedGeneric { args, .. } | Type::Generic { args, .. } => args,
            _ => return None,
        };
        match args.first()? {
            rask_types::GenericArg::Type(t) => self.type_head_name(t),
            _ => None,
        }
    }
}

/// Extract the function name from a Call expression's func field.
pub(crate) fn extract_callee_name(func: &Expr) -> Option<String> {
    match &func.kind {
        ExprKind::Ident(name) => Some(name.clone()),
        ExprKind::Field { object, field } => {
            // Type.method style: extract "Type.method"
            if let ExprKind::Ident(obj_name) = &object.kind {
                Some(format!("{}.{}", obj_name, field))
            } else {
                None
            }
        }
        _ => None,
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_context_filtered() {
        use crate::hidden_params::collect::is_runtime_context;
        assert!(is_runtime_context("Multitasking"));
        assert!(is_runtime_context("multitasking"));
        assert!(is_runtime_context("ThreadPool"));
        assert!(is_runtime_context("threadpool"));
        assert!(!is_runtime_context("Pool<Player>"));
    }
}
