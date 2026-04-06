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
use rask_ast::{NodeId, Span};

// ── Types ───────────────────────────────────────────────────────────────

/// A context requirement derived from a `using` clause.
#[derive(Debug, Clone)]
pub(crate) struct ContextReq {
    /// Hidden parameter name: `__ctx_pool_Player`, `__ctx_runtime`, etc.
    pub param_name: String,
    /// Type string for the parameter: `&Pool<Player>`, `RuntimeContext`
    pub param_type: String,
    /// Original clause type string: `Pool<Player>`, `Multitasking`
    pub clause_type: String,
    /// Is this a runtime context (optional `?` param) vs pool (required)?
    pub is_runtime: bool,
    /// Named alias from `using players: Pool<Player>`
    pub alias: Option<String>,
}

/// A pool found in scope during CC4 resolution.
#[derive(Debug, Clone)]
pub(crate) struct ScopePool {
    /// Variable name in user code: `players`, `self.players`, etc.
    pub var_name: String,
    /// Pool type string: `Pool<Player>`
    pub pool_type: String,
    /// Where it came from (for error messages).
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

/// Information about a function for context resolution.
#[derive(Debug, Clone)]
pub(crate) struct FuncInfo {
    /// Explicit context requirements (from `using` clauses or propagation).
    pub reqs: Vec<ContextReq>,
    /// Is this function public?
    pub is_public: bool,
    /// Parameter names and type strings.
    pub params: Vec<(String, String)>,
    /// Fields of `self` type (if method): (field_name, field_type_string).
    pub self_fields: Vec<(String, String)>,
    /// Local variable declarations: (var_name, type_string).
    pub locals: Vec<(String, String)>,
}

/// Errors from the hidden parameter pass.
#[derive(Debug, Clone)]
pub struct HiddenParamError {
    pub message: String,
    pub span: Span,
}

// ── Public API ──────────────────────────────────────────────────────────

/// Run the hidden parameter pass on a set of declarations.
///
/// Mutates the AST in place:
/// - Functions with `using` clauses gain hidden `__ctx_*` parameters
/// - Call sites to those functions gain hidden arguments
/// - `using Multitasking { }` blocks become context construction + teardown
///
/// Accepts optional TypedProgram for proper CC4 scope resolution.
/// Without it, falls back to hidden-param-name matching (still correct
/// for the propagation case).
pub fn desugar_hidden_params(decls: &mut [Decl]) {
    desugar_hidden_params_with_types(decls, None);
}

/// Run the hidden parameter pass with type information for full CC4 resolution.
pub fn desugar_hidden_params_with_types(
    decls: &mut [Decl],
    node_types: Option<&HashMap<NodeId, rask_types::Type>>,
) {
    let mut pass = HiddenParamPass::new(node_types);
    pass.run(decls);
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
    /// Type information from the type checker (CC4 resolution).
    pub node_types: Option<&'a HashMap<NodeId, rask_types::Type>>,
    /// Fresh NodeId counter (high range to avoid parser collisions).
    pub next_id: u32,
    /// Errors collected during the pass.
    pub errors: Vec<HiddenParamError>,
}

impl<'a> HiddenParamPass<'a> {
    pub fn new(node_types: Option<&'a HashMap<NodeId, rask_types::Type>>) -> Self {
        Self {
            func_contexts: HashMap::new(),
            func_info: HashMap::new(),
            call_graph: HashMap::new(),
            public_funcs: HashSet::new(),
            struct_fields: HashMap::new(),
            node_types,
            next_id: 2_000_000,
            errors: Vec::new(),
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

/// Convert a ContextClause into a ContextReq.
pub(crate) fn context_clause_to_req(cc: &ContextClause) -> ContextReq {
    let is_runtime = cc.ty == "Multitasking" || cc.ty == "multitasking";

    let (param_name, param_type) = if is_runtime {
        ("__ctx_runtime".to_string(), "RuntimeContext".to_string())
    } else {
        // Pool<T> → __ctx_pool_T with type &Pool<T>
        let inner = extract_generic_arg(&cc.ty).unwrap_or_default();
        let name = if let Some(alias) = &cc.name {
            format!("__ctx_{}", alias)
        } else {
            format!("__ctx_pool_{}", inner)
        };
        let ty = format!("&{}", cc.ty);
        (name, ty)
    };

    ContextReq {
        param_name,
        param_type,
        clause_type: cc.ty.clone(),
        is_runtime,
        alias: cc.name.clone(),
    }
}

/// Extract T from "Pool<T>" → "T".
pub(crate) fn extract_generic_arg(ty: &str) -> Option<String> {
    let start = ty.find('<')?;
    let end = ty.rfind('>')?;
    Some(ty[start + 1..end].to_string())
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

/// Check if a type string looks like `Pool<...>`.
pub(crate) fn is_pool_type(ty: &str) -> bool {
    ty.starts_with("Pool<") && ty.ends_with('>')
}

/// Check if a type string looks like `Handle<...>`.
pub(crate) fn is_handle_type(ty: &str) -> bool {
    ty.starts_with("Handle<") && ty.ends_with('>')
}

/// Convert a Handle<T> type to the Pool<T> it requires.
pub(crate) fn handle_to_pool_type(handle_ty: &str) -> Option<String> {
    let inner = extract_generic_arg(handle_ty)?;
    Some(format!("Pool<{}>", inner))
}

/// Format a rask_types::Type into a string for matching.
pub(crate) fn format_type(ty: &rask_types::Type) -> String {
    format!("{}", ty)
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rask_ast::decl::ContextClause;

    #[test]
    fn test_extract_generic_arg() {
        assert_eq!(extract_generic_arg("Pool<Player>"), Some("Player".to_string()));
        assert_eq!(extract_generic_arg("Pool<Vec<i32>>"), Some("Vec<i32>".to_string()));
        assert_eq!(extract_generic_arg("Multitasking"), None);
    }

    #[test]
    fn test_context_clause_to_req_pool() {
        let cc = ContextClause {
            name: None,
            ty: "Pool<Player>".to_string(),
            is_frozen: false,
        };
        let req = context_clause_to_req(&cc);
        assert_eq!(req.param_name, "__ctx_pool_Player");
        assert_eq!(req.param_type, "&Pool<Player>");
        assert!(!req.is_runtime);
    }

    #[test]
    fn test_context_clause_to_req_named() {
        let cc = ContextClause {
            name: Some("players".to_string()),
            ty: "Pool<Player>".to_string(),
            is_frozen: false,
        };
        let req = context_clause_to_req(&cc);
        assert_eq!(req.param_name, "__ctx_players");
        assert_eq!(req.param_type, "&Pool<Player>");
    }

    #[test]
    fn test_context_clause_to_req_runtime() {
        let cc = ContextClause {
            name: None,
            ty: "Multitasking".to_string(),
            is_frozen: false,
        };
        let req = context_clause_to_req(&cc);
        assert_eq!(req.param_name, "__ctx_runtime");
        assert_eq!(req.param_type, "RuntimeContext");
        assert!(req.is_runtime);
    }

    #[test]
    fn test_is_pool_type() {
        assert!(is_pool_type("Pool<Player>"));
        assert!(!is_pool_type("Vec<Player>"));
        assert!(!is_pool_type("Pool"));
    }

    #[test]
    fn test_handle_to_pool_type() {
        assert_eq!(handle_to_pool_type("Handle<Player>"), Some("Pool<Player>".to_string()));
        assert_eq!(handle_to_pool_type("Handle<Vec<i32>>"), Some("Pool<Vec<i32>>".to_string()));
        assert_eq!(handle_to_pool_type("i32"), None);
    }
}
