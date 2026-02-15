// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! MIR lowering - transform AST to MIR CFG.

mod expr;
mod stmt;

use crate::{
    BlockBuilder, MirFunction, MirOperand, MirRValue, MirStmt, MirTerminator, MirType, BlockId,
    LocalId,
};
use crate::types::{StructLayoutId, EnumLayoutId};
use rask_ast::{
    decl::{Decl, DeclKind},
    expr::{BinOp, Expr, UnaryOp},
    NodeId,
};
use rask_mono::{StructLayout, EnumLayout};
use rask_types::Type;
use std::collections::HashMap;

/// Typed expression result from lowering
type TypedOperand = (MirOperand, MirType);

/// Function signature for type inference
#[derive(Clone)]
struct FuncSig {
    ret_ty: MirType,
}

/// Loop context for break/continue
struct LoopContext {
    label: Option<String>,
    /// Block to jump to on `continue`
    continue_block: BlockId,
    /// Block to jump to on `break`
    exit_block: BlockId,
    /// For `break value` - local to assign the value to
    result_local: Option<LocalId>,
}

/// Layout context for MIR lowering — struct/enum metadata from monomorphization.
pub struct MirContext<'a> {
    pub struct_layouts: &'a [StructLayout],
    pub enum_layouts: &'a [EnumLayout],
    /// Type information for each expression node from type checking
    pub node_types: &'a HashMap<NodeId, Type>,
}

impl<'a> MirContext<'a> {
    /// Empty context for tests that don't need layouts or type information.
    pub fn empty_with_map(map: &'a HashMap<NodeId, Type>) -> MirContext<'a> {
        MirContext {
            struct_layouts: &[],
            enum_layouts: &[],
            node_types: map,
        }
    }

    pub fn find_struct(&self, name: &str) -> Option<(u32, &StructLayout)> {
        self.struct_layouts
            .iter()
            .enumerate()
            .find(|(_, s)| s.name == name)
            .map(|(i, s)| (i as u32, s))
    }

    pub fn find_enum(&self, name: &str) -> Option<(u32, &EnumLayout)> {
        self.enum_layouts
            .iter()
            .enumerate()
            .find(|(_, e)| e.name == name)
            .map(|(i, e)| (i as u32, e))
    }

    /// Resolve a type string to MirType, looking up struct/enum names in layouts.
    pub fn resolve_type_str(&self, s: &str) -> MirType {
        match s.trim() {
            "i8" => MirType::I8,
            "i16" => MirType::I16,
            "i32" => MirType::I32,
            "i64" => MirType::I64,
            "u8" => MirType::U8,
            "u16" => MirType::U16,
            "u32" => MirType::U32,
            "u64" => MirType::U64,
            "f32" => MirType::F32,
            "f64" => MirType::F64,
            "bool" => MirType::Bool,
            "char" => MirType::Char,
            "string" => MirType::String,
            "()" | "" => MirType::Void,
            name => {
                if let Some((idx, _)) = self.find_struct(name) {
                    MirType::Struct(StructLayoutId(idx))
                } else if let Some((idx, _)) = self.find_enum(name) {
                    MirType::Enum(EnumLayoutId(idx))
                } else {
                    MirType::Ptr
                }
            }
        }
    }

    /// Convert a Type from the type checker to MirType.
    pub fn type_to_mir(&self, ty: &Type) -> MirType {
        match ty {
            Type::Unit => MirType::Void,
            Type::Bool => MirType::Bool,
            Type::I8 => MirType::I8,
            Type::I16 => MirType::I16,
            Type::I32 => MirType::I32,
            Type::I64 | Type::I128 => MirType::I64,
            Type::U8 => MirType::U8,
            Type::U16 => MirType::U16,
            Type::U32 => MirType::U32,
            Type::U64 | Type::U128 => MirType::U64,
            Type::F32 => MirType::F32,
            Type::F64 => MirType::F64,
            Type::Char => MirType::Char,
            Type::String => MirType::String,
            Type::Never => MirType::Void,
            // Named types — look up in struct/enum layouts by name
            Type::UnresolvedNamed(name) => self.resolve_type_str(name),
            // Resolved named types — need monomorphization name, fall back to Ptr
            Type::Named(_) | Type::Generic { .. } | Type::UnresolvedGeneric { .. } => {
                let type_str = format!("{}", ty);
                self.resolve_type_str(&type_str)
            }
            // Compound types not yet lowered to MIR — pointer representation
            Type::Fn { .. } | Type::Tuple(_) | Type::Array { .. } | Type::Slice(_)
            | Type::Option(_) | Type::Result { .. } | Type::Union(_) => MirType::Ptr,
            // Should not reach MIR lowering
            Type::Var(_) | Type::Error => MirType::Ptr,
        }
    }

    /// Look up the MIR type for an expression node.
    pub fn lookup_node_type(&self, node_id: NodeId) -> Option<MirType> {
        self.node_types.get(&node_id).map(|ty| self.type_to_mir(ty))
    }

    /// Look up the raw Type for an expression node (preserves generic info).
    pub fn lookup_raw_type(&self, node_id: NodeId) -> Option<&Type> {
        self.node_types.get(&node_id)
    }
}

pub struct MirLowerer<'a> {
    builder: BlockBuilder,
    /// Variable name → (local id, type)
    locals: HashMap<String, (LocalId, MirType)>,
    /// Function name → signature (for call return types)
    func_sigs: HashMap<String, FuncSig>,
    /// Stack of enclosing loops (innermost last)
    loop_stack: Vec<LoopContext>,
    /// Layout context from monomorphization
    ctx: &'a MirContext<'a>,
    /// Synthesized closure functions produced during lowering
    synthesized_functions: Vec<MirFunction>,
    /// Counter for generating unique closure function names
    closure_counter: u32,
    /// Name of the function being lowered (for closure naming)
    parent_name: String,
    /// Variable names known to hold closure values
    closure_locals: std::collections::HashSet<String>,
}

impl<'a> MirLowerer<'a> {
    /// Lower a monomorphized function declaration to MIR.
    ///
    /// `all_decls` provides function signatures for resolving call return types.
    /// `ctx` provides struct/enum layout data for resolving field types and offsets.
    ///
    /// Returns the lowered function plus any synthesized closure functions.
    pub fn lower_function(
        decl: &Decl,
        all_decls: &[Decl],
        ctx: &MirContext,
    ) -> Result<Vec<MirFunction>, LoweringError> {
        let fn_decl = match &decl.kind {
            DeclKind::Fn(f) => f,
            _ => {
                return Err(LoweringError::InvalidConstruct(
                    "Expected function declaration".to_string(),
                ))
            }
        };

        let ret_ty = fn_decl
            .ret_ty
            .as_deref()
            .map(|s| ctx.resolve_type_str(s))
            .unwrap_or(MirType::Void);

        // Build function signature table from all declarations
        let mut func_sigs = HashMap::new();
        for d in all_decls {
            if let DeclKind::Fn(f) = &d.kind {
                let sig_ret = f
                    .ret_ty
                    .as_deref()
                    .map(|s| ctx.resolve_type_str(s))
                    .unwrap_or(MirType::Void);
                func_sigs.insert(f.name.clone(), FuncSig { ret_ty: sig_ret });
            }
        }

        let mut lowerer = MirLowerer {
            builder: BlockBuilder::new(fn_decl.name.clone(), ret_ty),
            locals: HashMap::new(),
            func_sigs,
            loop_stack: Vec::new(),
            ctx,
            synthesized_functions: Vec::new(),
            closure_counter: 0,
            parent_name: fn_decl.name.clone(),
            closure_locals: std::collections::HashSet::new(),
        };

        // Add parameters
        for param in &fn_decl.params {
            let param_ty = ctx.resolve_type_str(&param.ty);
            let local_id = lowerer.builder.add_param(param.name.clone(), param_ty.clone());
            lowerer.locals.insert(param.name.clone(), (local_id, param_ty));
        }

        // Lower function body
        for stmt in &fn_decl.body {
            lowerer.lower_stmt(stmt)?;
        }

        // Implicit void return for functions that don't explicitly return
        if lowerer.builder.current_block_unterminated() {
            lowerer.builder.terminate(MirTerminator::Return { value: None });
        }

        let main_fn = lowerer.builder.finish();
        let mut result = vec![main_fn];
        result.extend(lowerer.synthesized_functions);
        Ok(result)
    }

    /// Look up the type of an expression from the type checker.
    /// Returns None if type info is unavailable (e.g., in tests without full type checking).
    fn lookup_expr_type(&self, expr: &Expr) -> Option<MirType> {
        self.ctx.lookup_node_type(expr.id)
    }

    /// Extract the element type from an iterator type using raw type info.
    /// For Range<i32>, returns I32. Falls back to None for unknown types.
    fn extract_iterator_elem_type(&self, expr: &Expr) -> Option<MirType> {
        if let Some(ty) = self.ctx.lookup_raw_type(expr.id) {
            match ty {
                // Range<T> iterates over T
                Type::UnresolvedGeneric { name, args } if name == "Range" => {
                    args.first().and_then(|arg| {
                        if let rask_types::GenericArg::Type(t) = arg {
                            Some(self.ctx.type_to_mir(t))
                        } else {
                            None
                        }
                    })
                }
                // Array iterates over its element type
                Type::Array { elem, .. } => Some(self.ctx.type_to_mir(elem)),
                Type::Slice(elem) => Some(self.ctx.type_to_mir(elem)),
                _ => None,
            }
        } else {
            None
        }
    }

    /// Extract the Ok/Some payload type from the raw type of an expression.
    /// For Option<T>, returns T. For Result<T, E>, returns T.
    fn extract_payload_type(&self, expr: &Expr) -> Option<MirType> {
        if let Some(ty) = self.ctx.lookup_raw_type(expr.id) {
            match ty {
                Type::Option(inner) => Some(self.ctx.type_to_mir(inner)),
                Type::Result { ok, .. } => Some(self.ctx.type_to_mir(ok)),
                _ => None,
            }
        } else {
            None
        }
    }

    /// Extract the Err payload type from the raw type of an expression.
    fn extract_err_type(&self, expr: &Expr) -> Option<MirType> {
        if let Some(ty) = self.ctx.lookup_raw_type(expr.id) {
            match ty {
                Type::Result { err, .. } => Some(self.ctx.type_to_mir(err)),
                _ => None,
            }
        } else {
            None
        }
    }

    /// Resolve a pattern to its expected discriminant tag value.
    fn pattern_tag(&self, pattern: &rask_ast::expr::Pattern) -> i64 {
        use rask_ast::expr::Pattern;
        match pattern {
            Pattern::Constructor { name, .. } => self.variant_tag(name),
            Pattern::Ident(name) => {
                // Could be a variant name (Some, None, Ok, Err) or a binding
                if is_variant_name(name) {
                    self.variant_tag(name)
                } else {
                    0
                }
            }
            _ => 0,
        }
    }

    /// Look up the tag value for a variant name.
    fn variant_tag(&self, name: &str) -> i64 {
        // Well-known built-in variant tags
        match name {
            "Some" | "Ok" => 0,
            "None" | "Err" => 1,
            _ => {
                // Search enum layouts for user-defined variants
                for layout in self.ctx.enum_layouts {
                    for variant in &layout.variants {
                        if variant.name == name {
                            return variant.tag as i64;
                        }
                    }
                }
                0
            }
        }
    }

    /// Bind pattern payload variables into the current scope.
    ///
    /// After confirming a tag match, extracts payload fields from the
    /// enum value and inserts them as named locals.
    fn bind_pattern_payload(
        &mut self,
        pattern: &rask_ast::expr::Pattern,
        value: MirOperand,
        payload_ty: MirType,
    ) {
        use rask_ast::expr::Pattern;
        match pattern {
            Pattern::Constructor { fields, .. } => {
                for (i, field_pat) in fields.iter().enumerate() {
                    if let Pattern::Ident(name) = field_pat {
                        let field_ty = payload_ty.clone();
                        let local = self.builder.alloc_local(name.clone(), field_ty.clone());
                        self.locals.insert(name.clone(), (local, field_ty));
                        self.builder.push_stmt(MirStmt::Assign {
                            dst: local,
                            rvalue: MirRValue::Field {
                                base: value.clone(),
                                field_index: i as u32,
                            },
                        });
                    }
                    // Wildcard, Literal in field position — skip binding
                }
            }
            // Ident that is a variant name: no binding (pure match)
            // Ident that is a variable: this shouldn't reach here (it's a binding, not a match)
            _ => {}
        }
    }

    /// Collect free variables in a closure body — names used but not defined
    /// within the closure itself (params or local bindings).
    fn collect_free_vars(
        &self,
        body: &Expr,
        params: &[rask_ast::expr::ClosureParam],
    ) -> Vec<(String, LocalId, MirType)> {
        let mut free = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let bound: std::collections::HashSet<String> =
            params.iter().map(|p| p.name.clone()).collect();
        self.walk_free_vars(body, &bound, &mut seen, &mut free);
        free
    }

    /// Recursive walk to find free variable references.
    fn walk_free_vars(
        &self,
        expr: &Expr,
        bound: &std::collections::HashSet<String>,
        seen: &mut std::collections::HashSet<String>,
        free: &mut Vec<(String, LocalId, MirType)>,
    ) {
        use rask_ast::expr::ExprKind;
        match &expr.kind {
            ExprKind::Ident(name) => {
                if !bound.contains(name) && !seen.contains(name) {
                    if let Some((local_id, ty)) = self.locals.get(name) {
                        seen.insert(name.clone());
                        free.push((name.clone(), *local_id, ty.clone()));
                    }
                }
            }
            ExprKind::Block(stmts) => {
                self.walk_free_vars_block(stmts, bound, seen, free);
            }
            ExprKind::Binary { left, right, .. } => {
                self.walk_free_vars(left, bound, seen, free);
                self.walk_free_vars(right, bound, seen, free);
            }
            ExprKind::Unary { operand, .. } => {
                self.walk_free_vars(operand, bound, seen, free);
            }
            ExprKind::Call { func, args } => {
                self.walk_free_vars(func, bound, seen, free);
                for arg in args {
                    self.walk_free_vars(&arg.expr, bound, seen, free);
                }
            }
            ExprKind::MethodCall { object, args, .. } => {
                self.walk_free_vars(object, bound, seen, free);
                for arg in args {
                    self.walk_free_vars(&arg.expr, bound, seen, free);
                }
            }
            ExprKind::If { cond, then_branch, else_branch } => {
                self.walk_free_vars(cond, bound, seen, free);
                self.walk_free_vars(then_branch, bound, seen, free);
                if let Some(e) = else_branch {
                    self.walk_free_vars(e, bound, seen, free);
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                self.walk_free_vars(scrutinee, bound, seen, free);
                for arm in arms {
                    let mut arm_bound = bound.clone();
                    collect_pattern_names(&arm.pattern, &mut arm_bound);
                    self.walk_free_vars(&arm.body, &arm_bound, seen, free);
                }
            }
            ExprKind::Field { object, .. } => {
                self.walk_free_vars(object, bound, seen, free);
            }
            ExprKind::Index { object, index } => {
                self.walk_free_vars(object, bound, seen, free);
                self.walk_free_vars(index, bound, seen, free);
            }
            ExprKind::Array(elems) => {
                for e in elems { self.walk_free_vars(e, bound, seen, free); }
            }
            ExprKind::Tuple(elems) => {
                for e in elems { self.walk_free_vars(e, bound, seen, free); }
            }
            ExprKind::StructLit { fields, spread, .. } => {
                for f in fields { self.walk_free_vars(&f.value, bound, seen, free); }
                if let Some(s) = spread { self.walk_free_vars(s, bound, seen, free); }
            }
            ExprKind::Closure { params: inner_params, body, .. } => {
                let mut inner_bound = bound.clone();
                for p in inner_params { inner_bound.insert(p.name.clone()); }
                self.walk_free_vars(body, &inner_bound, seen, free);
            }
            ExprKind::Try(inner) | ExprKind::Unwrap { expr: inner, .. } => {
                self.walk_free_vars(inner, bound, seen, free);
            }
            ExprKind::Cast { expr: inner, .. } => {
                self.walk_free_vars(inner, bound, seen, free);
            }
            ExprKind::NullCoalesce { value, default } => {
                self.walk_free_vars(value, bound, seen, free);
                self.walk_free_vars(default, bound, seen, free);
            }
            ExprKind::Range { start, end, .. } => {
                if let Some(s) = start { self.walk_free_vars(s, bound, seen, free); }
                if let Some(e) = end { self.walk_free_vars(e, bound, seen, free); }
            }
            ExprKind::IfLet { expr: inner, pattern, then_branch, else_branch } => {
                self.walk_free_vars(inner, bound, seen, free);
                let mut then_bound = bound.clone();
                collect_pattern_names(pattern, &mut then_bound);
                self.walk_free_vars(then_branch, &then_bound, seen, free);
                if let Some(e) = else_branch { self.walk_free_vars(e, bound, seen, free); }
            }
            ExprKind::GuardPattern { expr: inner, else_branch, .. } => {
                self.walk_free_vars(inner, bound, seen, free);
                self.walk_free_vars(else_branch, bound, seen, free);
            }
            ExprKind::IsPattern { expr: inner, .. } => {
                self.walk_free_vars(inner, bound, seen, free);
            }
            ExprKind::Assert { condition, message } | ExprKind::Check { condition, message } => {
                self.walk_free_vars(condition, bound, seen, free);
                if let Some(m) = message { self.walk_free_vars(m, bound, seen, free); }
            }
            ExprKind::OptionalField { object, .. } => {
                self.walk_free_vars(object, bound, seen, free);
            }
            ExprKind::ArrayRepeat { value, count } => {
                self.walk_free_vars(value, bound, seen, free);
                self.walk_free_vars(count, bound, seen, free);
            }
            ExprKind::UsingBlock { args, body, .. } => {
                for arg in args {
                    self.walk_free_vars(&arg.expr, bound, seen, free);
                }
                self.walk_free_vars_block(body, bound, seen, free);
            }
            ExprKind::Unsafe { body } | ExprKind::Comptime { body } => {
                self.walk_free_vars_block(body, bound, seen, free);
            }
            ExprKind::WithAs { bindings, body } => {
                for (bind_expr, _) in bindings {
                    self.walk_free_vars(bind_expr, bound, seen, free);
                }
                self.walk_free_vars_block(body, bound, seen, free);
            }
            ExprKind::Spawn { body } | ExprKind::BlockCall { body, .. } => {
                self.walk_free_vars_block(body, bound, seen, free);
            }
            ExprKind::Select { arms, .. } => {
                for arm in arms {
                    match &arm.kind {
                        rask_ast::expr::SelectArmKind::Recv { channel, .. } => {
                            self.walk_free_vars(channel, bound, seen, free);
                        }
                        rask_ast::expr::SelectArmKind::Send { channel, value } => {
                            self.walk_free_vars(channel, bound, seen, free);
                            self.walk_free_vars(value, bound, seen, free);
                        }
                        rask_ast::expr::SelectArmKind::Default => {}
                    }
                    self.walk_free_vars(&arm.body, bound, seen, free);
                }
            }
            // Literals — no free variables
            ExprKind::Int(..) | ExprKind::Float(..) | ExprKind::String(..)
            | ExprKind::Char(..) | ExprKind::Bool(..) => {}
        }
    }

    fn walk_free_vars_block(
        &self,
        stmts: &[rask_ast::stmt::Stmt],
        bound: &std::collections::HashSet<String>,
        seen: &mut std::collections::HashSet<String>,
        free: &mut Vec<(String, LocalId, MirType)>,
    ) {
        let mut local_bound = bound.clone();
        for stmt in stmts {
            self.walk_free_vars_stmt(stmt, &local_bound, seen, free);
            match &stmt.kind {
                rask_ast::stmt::StmtKind::Let { name, .. }
                | rask_ast::stmt::StmtKind::Const { name, .. } => {
                    local_bound.insert(name.clone());
                }
                rask_ast::stmt::StmtKind::LetTuple { names, .. }
                | rask_ast::stmt::StmtKind::ConstTuple { names, .. } => {
                    for n in names { local_bound.insert(n.clone()); }
                }
                _ => {}
            }
        }
    }

    fn walk_free_vars_stmt(
        &self,
        stmt: &rask_ast::stmt::Stmt,
        bound: &std::collections::HashSet<String>,
        seen: &mut std::collections::HashSet<String>,
        free: &mut Vec<(String, LocalId, MirType)>,
    ) {
        use rask_ast::stmt::StmtKind;
        match &stmt.kind {
            StmtKind::Expr(e) => self.walk_free_vars(e, bound, seen, free),
            StmtKind::Let { init, .. } | StmtKind::Const { init, .. } => {
                self.walk_free_vars(init, bound, seen, free);
            }
            StmtKind::LetTuple { init, .. } | StmtKind::ConstTuple { init, .. } => {
                self.walk_free_vars(init, bound, seen, free);
            }
            StmtKind::Return(Some(e)) => self.walk_free_vars(e, bound, seen, free),
            StmtKind::Return(None) => {}
            StmtKind::Assign { target, value } => {
                self.walk_free_vars(target, bound, seen, free);
                self.walk_free_vars(value, bound, seen, free);
            }
            StmtKind::While { cond, body } => {
                self.walk_free_vars(cond, bound, seen, free);
                self.walk_free_vars_block(body, bound, seen, free);
            }
            StmtKind::WhileLet { pattern, expr, body } => {
                self.walk_free_vars(expr, bound, seen, free);
                let mut body_bound = bound.clone();
                collect_pattern_names(pattern, &mut body_bound);
                self.walk_free_vars_block(body, &body_bound, seen, free);
            }
            StmtKind::For { binding, iter, body, .. } => {
                self.walk_free_vars(iter, bound, seen, free);
                let mut inner_bound = bound.clone();
                inner_bound.insert(binding.clone());
                self.walk_free_vars_block(body, &inner_bound, seen, free);
            }
            StmtKind::Loop { body, .. } => {
                self.walk_free_vars_block(body, bound, seen, free);
            }
            StmtKind::Break { value, .. } => {
                if let Some(v) = value { self.walk_free_vars(v, bound, seen, free); }
            }
            StmtKind::Continue(_) => {}
            StmtKind::Ensure { body, else_handler } => {
                self.walk_free_vars_block(body, bound, seen, free);
                if let Some((name, handler)) = else_handler {
                    let mut inner_bound = bound.clone();
                    inner_bound.insert(name.clone());
                    self.walk_free_vars_block(handler, &inner_bound, seen, free);
                }
            }
            StmtKind::Comptime(body) => {
                self.walk_free_vars_block(body, bound, seen, free);
            }
        }
    }
}

/// Collect variable names bound by a pattern into a set.
fn collect_pattern_names(
    pattern: &rask_ast::expr::Pattern,
    names: &mut std::collections::HashSet<String>,
) {
    use rask_ast::expr::Pattern;
    match pattern {
        Pattern::Ident(name) => { names.insert(name.clone()); }
        Pattern::Constructor { fields, .. } => {
            for p in fields { collect_pattern_names(p, names); }
        }
        Pattern::Struct { fields, .. } => {
            for (_, p) in fields { collect_pattern_names(p, names); }
        }
        Pattern::Tuple(elems) => {
            for p in elems { collect_pattern_names(p, names); }
        }
        Pattern::Or(alts) => {
            // All alternatives bind the same names; just collect from the first
            if let Some(first) = alts.first() { collect_pattern_names(first, names); }
        }
        Pattern::Wildcard | Pattern::Literal(_) => {}
    }
}

// =================================================================
// MIR type size (for computing offsets in aggregates)
// =================================================================

/// Return byte size for a MirType. Used for array/tuple/struct store offsets.
fn mir_type_size(ty: &MirType) -> u32 {
    match ty {
        MirType::Void => 0,
        MirType::Bool | MirType::I8 | MirType::U8 => 1,
        MirType::I16 | MirType::U16 => 2,
        MirType::I32 | MirType::U32 | MirType::F32 | MirType::Char => 4,
        MirType::I64 | MirType::U64 | MirType::F64 | MirType::Ptr | MirType::FuncPtr(_) => 8,
        MirType::String => 16,
        MirType::Struct(id) => {
            // Can't look up the layout without context — fallback to pointer size
            let _ = id;
            8
        }
        MirType::Enum(id) => {
            let _ = id;
            8
        }
        MirType::Array { elem, len } => mir_type_size(elem) * len,
    }
}

// =================================================================
// Operator mappings
// =================================================================

/// Recognize operator method names produced by desugar (e.g. "add", "sub", "eq")
fn operator_method_to_binop(method: &str) -> Option<crate::operand::BinOp> {
    use crate::operand::BinOp as MirBinOp;
    match method {
        "add" => Some(MirBinOp::Add),
        "sub" => Some(MirBinOp::Sub),
        "mul" => Some(MirBinOp::Mul),
        "div" => Some(MirBinOp::Div),
        "rem" => Some(MirBinOp::Mod),
        "eq" => Some(MirBinOp::Eq),
        "lt" => Some(MirBinOp::Lt),
        "gt" => Some(MirBinOp::Gt),
        "le" => Some(MirBinOp::Le),
        "ge" => Some(MirBinOp::Ge),
        "bit_and" => Some(MirBinOp::BitAnd),
        "bit_or" => Some(MirBinOp::BitOr),
        "bit_xor" => Some(MirBinOp::BitXor),
        "shl" => Some(MirBinOp::Shl),
        "shr" => Some(MirBinOp::Shr),
        _ => None,
    }
}

/// Recognize unary operator method names produced by desugar
fn operator_method_to_unaryop(method: &str) -> Option<crate::operand::UnaryOp> {
    use crate::operand::UnaryOp as MirUnaryOp;
    match method {
        "neg" => Some(MirUnaryOp::Neg),
        "bit_not" => Some(MirUnaryOp::BitNot),
        _ => None,
    }
}

/// Map AST binary operator to MIR binary operator (for &&/|| that survive desugar)
fn lower_binop(op: BinOp) -> crate::operand::BinOp {
    use crate::operand::BinOp as MirBinOp;
    match op {
        BinOp::Add => MirBinOp::Add,
        BinOp::Sub => MirBinOp::Sub,
        BinOp::Mul => MirBinOp::Mul,
        BinOp::Div => MirBinOp::Div,
        BinOp::Mod => MirBinOp::Mod,
        BinOp::Eq => MirBinOp::Eq,
        BinOp::Ne => MirBinOp::Ne,
        BinOp::Lt => MirBinOp::Lt,
        BinOp::Gt => MirBinOp::Gt,
        BinOp::Le => MirBinOp::Le,
        BinOp::Ge => MirBinOp::Ge,
        BinOp::And => MirBinOp::And,
        BinOp::Or => MirBinOp::Or,
        BinOp::BitAnd => MirBinOp::BitAnd,
        BinOp::BitOr => MirBinOp::BitOr,
        BinOp::BitXor => MirBinOp::BitXor,
        BinOp::Shl => MirBinOp::Shl,
        BinOp::Shr => MirBinOp::Shr,
    }
}

/// Map AST unary operator to MIR unary operator.
fn lower_unaryop(op: UnaryOp) -> crate::operand::UnaryOp {
    use crate::operand::UnaryOp as MirUnaryOp;
    match op {
        UnaryOp::Neg => MirUnaryOp::Neg,
        UnaryOp::Not => MirUnaryOp::Not,
        UnaryOp::BitNot => MirUnaryOp::BitNot,
        UnaryOp::Ref | UnaryOp::Deref => unreachable!(),
    }
}

/// Check if a name is a known enum variant (not a variable binding).
fn is_variant_name(name: &str) -> bool {
    matches!(name, "Some" | "None" | "Ok" | "Err")
        || name.contains('.')  // Qualified variant like "Status.Active"
        || name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
}

/// Detect identifiers that name types rather than values.
///
/// Uppercase-initial names are user-defined types (structs, enums, traits).
/// A few lowercase names (`string`) are built-in types that support static methods.
fn is_type_constructor_name(name: &str) -> bool {
    name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
        || matches!(name, "string")
}

/// Determine result type for a binary operation.
/// Comparison ops return Bool, arithmetic returns the operand type.
fn binop_result_type(op: &crate::operand::BinOp, operand_ty: &MirType) -> MirType {
    use crate::operand::BinOp as B;
    match op {
        B::Eq | B::Ne | B::Lt | B::Gt | B::Le | B::Ge | B::And | B::Or => MirType::Bool,
        _ => operand_ty.clone(),
    }
}

#[derive(Debug)]
pub enum LoweringError {
    UnresolvedVariable(String),
    UnresolvedGeneric(String),
    InvalidConstruct(String),
}

// ─── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{operand::MirConst, MirRValue, MirStmt};
    use rask_ast::decl::{Decl, DeclKind, FnDecl, Param};
    use rask_ast::expr::{ArgMode, CallArg, Expr, ExprKind, MatchArm, Pattern};
    use rask_ast::stmt::{Stmt, StmtKind};
    use rask_ast::{NodeId, Span};

    // ── AST construction helpers ────────────────────────────────

    fn sp() -> Span {
        Span::new(0, 0)
    }

    fn int_expr(val: i64) -> Expr {
        Expr { id: NodeId(100), kind: ExprKind::Int(val, None), span: sp() }
    }

    fn float_expr(val: f64) -> Expr {
        Expr { id: NodeId(101), kind: ExprKind::Float(val, None), span: sp() }
    }

    fn string_expr(s: &str) -> Expr {
        Expr { id: NodeId(102), kind: ExprKind::String(s.to_string()), span: sp() }
    }

    fn bool_expr(val: bool) -> Expr {
        Expr { id: NodeId(103), kind: ExprKind::Bool(val), span: sp() }
    }

    fn ident_expr(name: &str) -> Expr {
        Expr { id: NodeId(105), kind: ExprKind::Ident(name.to_string()), span: sp() }
    }

    fn call_expr(func: &str, args: Vec<Expr>) -> Expr {
        Expr {
            id: NodeId(106),
            kind: ExprKind::Call {
                func: Box::new(ident_expr(func)),
                args: args.into_iter().map(|expr| CallArg { mode: ArgMode::Default, expr }).collect(),
            },
            span: sp(),
        }
    }

    fn binary_expr(op: BinOp, left: Expr, right: Expr) -> Expr {
        Expr {
            id: NodeId(107),
            kind: ExprKind::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            },
            span: sp(),
        }
    }

    fn unary_expr(op: UnaryOp, operand: Expr) -> Expr {
        Expr {
            id: NodeId(108),
            kind: ExprKind::Unary {
                op,
                operand: Box::new(operand),
            },
            span: sp(),
        }
    }

    fn method_call_expr(obj: Expr, method: &str, args: Vec<Expr>) -> Expr {
        Expr {
            id: NodeId(109),
            kind: ExprKind::MethodCall {
                object: Box::new(obj),
                method: method.to_string(),
                type_args: None,
                args: args.into_iter().map(|expr| CallArg { mode: ArgMode::Default, expr }).collect(),
            },
            span: sp(),
        }
    }

    fn if_expr(cond: Expr, then_br: Expr, else_br: Option<Expr>) -> Expr {
        Expr {
            id: NodeId(110),
            kind: ExprKind::If {
                cond: Box::new(cond),
                then_branch: Box::new(then_br),
                else_branch: else_br.map(Box::new),
            },
            span: sp(),
        }
    }

    fn match_expr(scrutinee: Expr, arms: Vec<MatchArm>) -> Expr {
        Expr {
            id: NodeId(111),
            kind: ExprKind::Match {
                scrutinee: Box::new(scrutinee),
                arms,
            },
            span: sp(),
        }
    }

    fn try_expr(inner: Expr) -> Expr {
        Expr {
            id: NodeId(112),
            kind: ExprKind::Try(Box::new(inner)),
            span: sp(),
        }
    }

    fn return_stmt(val: Option<Expr>) -> Stmt {
        Stmt { id: NodeId(200), kind: StmtKind::Return(val), span: sp() }
    }

    fn let_stmt(name: &str, ty: Option<&str>, init: Expr) -> Stmt {
        Stmt {
            id: NodeId(201),
            kind: StmtKind::Let {
                name: name.to_string(),
                name_span: sp(),
                ty: ty.map(|s| s.to_string()),
                init,
            },
            span: sp(),
        }
    }

    fn const_stmt(name: &str, ty: Option<&str>, init: Expr) -> Stmt {
        Stmt {
            id: NodeId(202),
            kind: StmtKind::Const {
                name: name.to_string(),
                name_span: sp(),
                ty: ty.map(|s| s.to_string()),
                init,
            },
            span: sp(),
        }
    }

    fn expr_stmt(e: Expr) -> Stmt {
        Stmt { id: NodeId(203), kind: StmtKind::Expr(e), span: sp() }
    }

    fn while_stmt(cond: Expr, body: Vec<Stmt>) -> Stmt {
        Stmt {
            id: NodeId(204),
            kind: StmtKind::While { cond, body },
            span: sp(),
        }
    }

    fn loop_stmt(label: Option<&str>, body: Vec<Stmt>) -> Stmt {
        Stmt {
            id: NodeId(205),
            kind: StmtKind::Loop {
                label: label.map(|s| s.to_string()),
                body,
            },
            span: sp(),
        }
    }

    fn for_stmt(binding: &str, iter: Expr, body: Vec<Stmt>) -> Stmt {
        Stmt {
            id: NodeId(206),
            kind: StmtKind::For {
                label: None,
                binding: binding.to_string(),
                iter,
                body,
            },
            span: sp(),
        }
    }

    fn break_stmt(label: Option<&str>, value: Option<Expr>) -> Stmt {
        Stmt {
            id: NodeId(207),
            kind: StmtKind::Break {
                label: label.map(|s| s.to_string()),
                value,
            },
            span: sp(),
        }
    }

    fn continue_stmt(label: Option<&str>) -> Stmt {
        Stmt {
            id: NodeId(208),
            kind: StmtKind::Continue(label.map(|s| s.to_string())),
            span: sp(),
        }
    }

    fn ensure_stmt(body: Vec<Stmt>, handler: Option<(&str, Vec<Stmt>)>) -> Stmt {
        Stmt {
            id: NodeId(209),
            kind: StmtKind::Ensure {
                body,
                else_handler: handler.map(|(n, s)| (n.to_string(), s)),
            },
            span: sp(),
        }
    }

    fn assign_stmt(target: Expr, value: Expr) -> Stmt {
        Stmt {
            id: NodeId(210),
            kind: StmtKind::Assign { target, value },
            span: sp(),
        }
    }

    fn make_fn(name: &str, params: Vec<(&str, &str)>, ret_ty: Option<&str>, body: Vec<Stmt>) -> Decl {
        Decl {
            id: NodeId(0),
            kind: DeclKind::Fn(FnDecl {
                name: name.to_string(),
                type_params: vec![],
                params: params
                    .into_iter()
                    .map(|(n, ty)| Param {
                        name: n.to_string(),
                        name_span: sp(),
                        ty: ty.to_string(),
                        is_take: false,
                        is_mutate: false,
                        default: None,
                    })
                    .collect(),
                ret_ty: ret_ty.map(|s| s.to_string()),
                context_clauses: vec![],
                body,
                is_pub: false,
                is_comptime: false,
                is_unsafe: false,
                attrs: vec![],
            }),
            span: sp(),
        }
    }

    fn lower(decl: &Decl, all_decls: &[Decl]) -> MirFunction {
        let node_types = HashMap::new();
        let ctx = MirContext::empty_with_map(&node_types);
        let mut fns = MirLowerer::lower_function(decl, all_decls, &ctx).expect("lowering failed");
        fns.remove(0) // Return the main function (first element)
    }

    fn lower_one(decl: &Decl) -> MirFunction {
        lower(decl, &[decl.clone()])
    }

    // ── helpers for inspecting MIR ──────────────────────────────

    fn has_return(f: &MirFunction) -> bool {
        f.blocks.iter().any(|b| matches!(b.terminator, MirTerminator::Return { .. }))
    }

    fn has_branch(f: &MirFunction) -> bool {
        f.blocks.iter().any(|b| matches!(b.terminator, MirTerminator::Branch { .. }))
    }

    fn has_switch(f: &MirFunction) -> bool {
        f.blocks.iter().any(|b| matches!(b.terminator, MirTerminator::Switch { .. }))
    }

    fn has_goto(f: &MirFunction) -> bool {
        f.blocks.iter().any(|b| matches!(b.terminator, MirTerminator::Goto { .. }))
    }

    fn count_blocks(f: &MirFunction) -> usize {
        f.blocks.len()
    }

    fn find_call(f: &MirFunction, func_name: &str) -> bool {
        f.blocks.iter().any(|b| {
            b.statements.iter().any(|s| matches!(s, MirStmt::Call { func, .. } if func.name == func_name))
        })
    }

    fn find_assign_binop(f: &MirFunction) -> bool {
        f.blocks.iter().any(|b| {
            b.statements.iter().any(|s| matches!(s, MirStmt::Assign { rvalue: MirRValue::BinaryOp { .. }, .. }))
        })
    }

    fn find_assign_unaryop(f: &MirFunction) -> bool {
        f.blocks.iter().any(|b| {
            b.statements.iter().any(|s| matches!(s, MirStmt::Assign { rvalue: MirRValue::UnaryOp { .. }, .. }))
        })
    }

    fn find_ensure_push(f: &MirFunction) -> bool {
        f.blocks.iter().any(|b| {
            b.statements.iter().any(|s| matches!(s, MirStmt::EnsurePush { .. }))
        })
    }

    fn find_ensure_pop(f: &MirFunction) -> bool {
        f.blocks.iter().any(|b| {
            b.statements.iter().any(|s| matches!(s, MirStmt::EnsurePop))
        })
    }

    fn find_enum_tag(f: &MirFunction) -> bool {
        f.blocks.iter().any(|b| {
            b.statements.iter().any(|s| matches!(s, MirStmt::Assign { rvalue: MirRValue::EnumTag { .. }, .. }))
        })
    }

    // ═══════════════════════════════════════════════════════════
    // Literals
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn lower_integer_literal() {
        let decl = make_fn("f", vec![], Some("i64"), vec![return_stmt(Some(int_expr(42)))]);
        let f = lower_one(&decl);
        let ret_block = f.blocks.iter().find(|b| matches!(b.terminator, MirTerminator::Return { .. })).unwrap();
        if let MirTerminator::Return { value: Some(MirOperand::Constant(MirConst::Int(42))) } = &ret_block.terminator {
            // good
        } else {
            panic!("Expected return 42, got: {:?}", ret_block.terminator);
        }
    }

    #[test]
    fn lower_string_literal() {
        let decl = make_fn("f", vec![], Some("string"), vec![return_stmt(Some(string_expr("hello")))]);
        let f = lower_one(&decl);
        assert_eq!(f.ret_ty, MirType::String);
    }

    #[test]
    fn lower_bool_literal() {
        let decl = make_fn("f", vec![], Some("bool"), vec![return_stmt(Some(bool_expr(true)))]);
        let f = lower_one(&decl);
        let ret_block = f.blocks.iter().find(|b| matches!(b.terminator, MirTerminator::Return { .. })).unwrap();
        if let MirTerminator::Return { value: Some(MirOperand::Constant(MirConst::Bool(true))) } = &ret_block.terminator {
            // good
        } else {
            panic!("Expected return true, got: {:?}", ret_block.terminator);
        }
    }

    // ═══════════════════════════════════════════════════════════
    // Variables and bindings
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn lower_variable_reference() {
        let decl = make_fn("f", vec![], Some("i32"), vec![
            const_stmt("x", Some("i32"), int_expr(42)),
            return_stmt(Some(ident_expr("x"))),
        ]);
        let f = lower_one(&decl);
        assert!(f.locals.iter().any(|l| l.name.as_deref() == Some("x")));
    }

    #[test]
    fn lower_unresolved_variable_errors() {
        let decl = make_fn("f", vec![], None, vec![return_stmt(Some(ident_expr("no_such_var")))]);
        let node_types = HashMap::new();
        let ctx = MirContext::empty_with_map(&node_types);
        let result = MirLowerer::lower_function(&decl, &[decl.clone()], &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn lower_let_binding() {
        let decl = make_fn("f", vec![], None, vec![
            let_stmt("x", Some("i32"), int_expr(10)),
            return_stmt(None),
        ]);
        let f = lower_one(&decl);
        let x_local = f.locals.iter().find(|l| l.name.as_deref() == Some("x"));
        assert!(x_local.is_some());
        assert_eq!(x_local.unwrap().ty, MirType::I32);
    }

    #[test]
    fn lower_let_infers_type() {
        let decl = make_fn("f", vec![], None, vec![
            let_stmt("x", None, int_expr(42)),
            return_stmt(None),
        ]);
        let f = lower_one(&decl);
        let x_local = f.locals.iter().find(|l| l.name.as_deref() == Some("x")).unwrap();
        assert_eq!(x_local.ty, MirType::I64);
    }

    #[test]
    fn lower_assignment() {
        let decl = make_fn("f", vec![], None, vec![
            let_stmt("x", Some("i32"), int_expr(0)),
            assign_stmt(ident_expr("x"), int_expr(42)),
            return_stmt(None),
        ]);
        let f = lower_one(&decl);
        let assign_count = f.blocks.iter()
            .flat_map(|b| b.statements.iter())
            .filter(|s| matches!(s, MirStmt::Assign { .. }))
            .count();
        assert!(assign_count >= 2);
    }

    // ═══════════════════════════════════════════════════════════
    // Operators
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn lower_binary_op_and_or() {
        let decl = make_fn("f", vec![], Some("bool"), vec![
            return_stmt(Some(binary_expr(BinOp::And, bool_expr(true), bool_expr(false)))),
        ]);
        let f = lower_one(&decl);
        assert!(find_assign_binop(&f));
    }

    #[test]
    fn lower_desugared_add_method() {
        let decl = make_fn("f", vec![("a", "i32"), ("b", "i32")], Some("i32"), vec![
            return_stmt(Some(method_call_expr(ident_expr("a"), "add", vec![ident_expr("b")]))),
        ]);
        let f = lower_one(&decl);
        assert!(find_assign_binop(&f));
    }

    #[test]
    fn lower_desugared_neg_method() {
        let decl = make_fn("f", vec![("a", "i32")], Some("i32"), vec![
            return_stmt(Some(method_call_expr(ident_expr("a"), "neg", vec![]))),
        ]);
        let f = lower_one(&decl);
        assert!(find_assign_unaryop(&f));
    }

    #[test]
    fn lower_unary_not() {
        let decl = make_fn("f", vec![], Some("bool"), vec![
            return_stmt(Some(unary_expr(UnaryOp::Not, bool_expr(true)))),
        ]);
        let f = lower_one(&decl);
        assert!(find_assign_unaryop(&f));
    }

    // ═══════════════════════════════════════════════════════════
    // Function calls
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn lower_function_call() {
        let callee = make_fn("greet", vec![], None, vec![return_stmt(None)]);
        let decl = make_fn("main", vec![], None, vec![
            expr_stmt(call_expr("greet", vec![])),
            return_stmt(None),
        ]);
        let f = lower(&decl, &[decl.clone(), callee]);
        assert!(find_call(&f, "greet"));
    }

    #[test]
    fn lower_call_with_args() {
        let add = make_fn("add", vec![("a", "i32"), ("b", "i32")], Some("i32"), vec![
            return_stmt(Some(int_expr(0))),
        ]);
        let decl = make_fn("main", vec![], Some("i32"), vec![
            return_stmt(Some(call_expr("add", vec![int_expr(1), int_expr(2)]))),
        ]);
        let f = lower(&decl, &[decl.clone(), add]);
        assert!(find_call(&f, "add"));
    }

    // ═══════════════════════════════════════════════════════════
    // Function metadata
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn lower_function_params() {
        let decl = make_fn("add", vec![("a", "i32"), ("b", "i32")], Some("i32"), vec![
            return_stmt(Some(int_expr(0))),
        ]);
        let f = lower_one(&decl);
        assert_eq!(f.params.len(), 2);
        assert_eq!(f.params[0].name.as_deref(), Some("a"));
        assert_eq!(f.params[0].ty, MirType::I32);
        assert_eq!(f.params[1].name.as_deref(), Some("b"));
        assert!(f.params[0].is_param);
        assert!(f.params[1].is_param);
    }

    #[test]
    fn lower_function_name_and_ret_ty() {
        let decl = make_fn("compute", vec![], Some("f64"), vec![return_stmt(Some(float_expr(0.0)))]);
        let f = lower_one(&decl);
        assert_eq!(f.name, "compute");
        assert_eq!(f.ret_ty, MirType::F64);
    }

    #[test]
    fn lower_void_return() {
        let decl = make_fn("f", vec![], None, vec![return_stmt(None)]);
        let f = lower_one(&decl);
        let ret = f.blocks.iter().find(|b| matches!(b.terminator, MirTerminator::Return { .. })).unwrap();
        assert!(matches!(ret.terminator, MirTerminator::Return { value: None }));
    }

    // ═══════════════════════════════════════════════════════════
    // parse_type_str
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn lower_parse_type_str_coverage() {
        let node_types = HashMap::new();
        let ctx = MirContext::empty_with_map(&node_types);
        assert_eq!(ctx.resolve_type_str("i8"), MirType::I8);
        assert_eq!(ctx.resolve_type_str("i16"), MirType::I16);
        assert_eq!(ctx.resolve_type_str("i32"), MirType::I32);
        assert_eq!(ctx.resolve_type_str("i64"), MirType::I64);
        assert_eq!(ctx.resolve_type_str("u8"), MirType::U8);
        assert_eq!(ctx.resolve_type_str("u16"), MirType::U16);
        assert_eq!(ctx.resolve_type_str("u32"), MirType::U32);
        assert_eq!(ctx.resolve_type_str("u64"), MirType::U64);
        assert_eq!(ctx.resolve_type_str("f32"), MirType::F32);
        assert_eq!(ctx.resolve_type_str("f64"), MirType::F64);
        assert_eq!(ctx.resolve_type_str("bool"), MirType::Bool);
        assert_eq!(ctx.resolve_type_str("char"), MirType::Char);
        assert_eq!(ctx.resolve_type_str("string"), MirType::String);
        assert_eq!(ctx.resolve_type_str("()"), MirType::Void);
        assert_eq!(ctx.resolve_type_str(""), MirType::Void);
        assert_eq!(ctx.resolve_type_str("SomeStruct"), MirType::Ptr);
    }

    // ═══════════════════════════════════════════════════════════
    // Control flow
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn lower_if_creates_branch() {
        let decl = make_fn("f", vec![], Some("i64"), vec![
            return_stmt(Some(if_expr(bool_expr(true), int_expr(1), Some(int_expr(2))))),
        ]);
        let f = lower_one(&decl);
        assert!(has_branch(&f));
        assert!(count_blocks(&f) >= 4);
    }

    #[test]
    fn lower_if_without_else() {
        let decl = make_fn("f", vec![], None, vec![
            return_stmt(Some(if_expr(bool_expr(true), int_expr(1), None))),
        ]);
        let f = lower_one(&decl);
        assert!(has_branch(&f));
    }

    #[test]
    fn lower_match_creates_switch() {
        let decl = make_fn("f", vec![("x", "i32")], Some("i64"), vec![
            return_stmt(Some(match_expr(
                ident_expr("x"),
                vec![
                    MatchArm { pattern: Pattern::Ident("a".to_string()), guard: None, body: Box::new(int_expr(1)) },
                    MatchArm { pattern: Pattern::Ident("b".to_string()), guard: None, body: Box::new(int_expr(2)) },
                ],
            ))),
        ]);
        let f = lower_one(&decl);
        assert!(has_switch(&f));
        assert!(find_enum_tag(&f));
    }

    #[test]
    fn lower_while_loop_cfg() {
        let decl = make_fn("f", vec![], None, vec![
            let_stmt("x", Some("i32"), int_expr(10)),
            while_stmt(
                binary_expr(BinOp::Gt, ident_expr("x"), int_expr(0)),
                vec![assign_stmt(ident_expr("x"), int_expr(0))],
            ),
            return_stmt(None),
        ]);
        let f = lower_one(&decl);
        assert!(has_branch(&f));
        assert!(has_goto(&f));
        assert!(count_blocks(&f) >= 4);
    }

    #[test]
    fn lower_for_loop() {
        let range = Expr {
            id: NodeId(300),
            kind: ExprKind::Range {
                start: Some(Box::new(int_expr(0))),
                end: Some(Box::new(int_expr(10))),
                inclusive: false,
            },
            span: sp(),
        };
        let decl = make_fn("f", vec![], None, vec![
            for_stmt("i", range, vec![expr_stmt(call_expr("process", vec![ident_expr("i")]))]),
            return_stmt(None),
        ]);
        let f = lower_one(&decl);
        assert!(has_branch(&f));
        assert!(find_call(&f, "next"));
    }

    #[test]
    fn lower_infinite_loop() {
        let decl = make_fn("f", vec![], None, vec![
            loop_stmt(None, vec![break_stmt(None, None)]),
            return_stmt(None),
        ]);
        let f = lower_one(&decl);
        assert!(has_goto(&f));
        assert!(has_return(&f));
    }

    #[test]
    fn lower_continue() {
        let decl = make_fn("f", vec![], None, vec![
            let_stmt("x", Some("i32"), int_expr(0)),
            while_stmt(
                binary_expr(BinOp::Lt, ident_expr("x"), int_expr(10)),
                vec![continue_stmt(None)],
            ),
            return_stmt(None),
        ]);
        let f = lower_one(&decl);
        let goto_count = f.blocks.iter()
            .filter(|b| matches!(b.terminator, MirTerminator::Goto { .. }))
            .count();
        assert!(goto_count >= 2);
    }

    #[test]
    fn lower_break_outside_loop_errors() {
        let decl = make_fn("f", vec![], None, vec![break_stmt(None, None)]);
        let node_types = HashMap::new();
        let ctx = MirContext::empty_with_map(&node_types);
        let result = MirLowerer::lower_function(&decl, &[decl.clone()], &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn lower_continue_outside_loop_errors() {
        let decl = make_fn("f", vec![], None, vec![continue_stmt(None)]);
        let node_types = HashMap::new();
        let ctx = MirContext::empty_with_map(&node_types);
        let result = MirLowerer::lower_function(&decl, &[decl.clone()], &ctx);
        assert!(result.is_err());
    }

    // ═══════════════════════════════════════════════════════════
    // Error handling
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn lower_try_creates_tag_check() {
        let callee = make_fn("fallible", vec![], Some("i32"), vec![return_stmt(Some(int_expr(0)))]);
        let decl = make_fn("f", vec![], Some("i32"), vec![
            return_stmt(Some(try_expr(call_expr("fallible", vec![])))),
        ]);
        let f = lower(&decl, &[decl.clone(), callee]);
        assert!(find_enum_tag(&f));
        assert!(has_branch(&f));
    }

    #[test]
    fn lower_ensure_push_pop() {
        let decl = make_fn("f", vec![], None, vec![
            ensure_stmt(
                vec![expr_stmt(call_expr("do_work", vec![]))],
                None,
            ),
            return_stmt(None),
        ]);
        let f = lower_one(&decl);
        assert!(find_ensure_push(&f));
        assert!(find_ensure_pop(&f));
    }

    #[test]
    fn lower_ensure_with_handler() {
        let decl = make_fn("f", vec![], None, vec![
            ensure_stmt(
                vec![expr_stmt(call_expr("work", vec![]))],
                Some(("err", vec![expr_stmt(call_expr("cleanup", vec![ident_expr("err")]))])),
            ),
            return_stmt(None),
        ]);
        let f = lower_one(&decl);
        assert!(find_ensure_push(&f));
        assert!(find_ensure_pop(&f));
        assert!(f.locals.iter().any(|l| l.name.as_deref() == Some("err")));
    }

    #[test]
    fn lower_unwrap_panics_on_err() {
        let decl = make_fn("f", vec![("x", "i32")], Some("i32"), vec![
            return_stmt(Some(Expr {
                id: NodeId(400),
                kind: ExprKind::Unwrap {
                    expr: Box::new(ident_expr("x")),
                    message: None,
                },
                span: sp(),
            })),
        ]);
        let f = lower_one(&decl);
        assert!(find_enum_tag(&f));
        assert!(has_branch(&f));
        assert!(find_call(&f, "panic_unwrap"));
        assert!(f.blocks.iter().any(|b| matches!(b.terminator, MirTerminator::Unreachable)));
    }

    // ═══════════════════════════════════════════════════════════
    // End-to-end
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn e2e_hello_world() {
        let print_fn = make_fn("print", vec![("s", "string")], None, vec![return_stmt(None)]);
        let decl = make_fn("main", vec![], None, vec![
            expr_stmt(call_expr("print", vec![string_expr("Hello, world!")])),
        ]);
        let f = lower(&decl, &[decl.clone(), print_fn]);
        assert_eq!(f.name, "main");
        assert!(find_call(&f, "print"));
    }

    #[test]
    fn e2e_mir_display_roundtrip() {
        let decl = make_fn("factorial", vec![("n", "i32")], Some("i32"), vec![
            return_stmt(Some(ident_expr("n"))),
        ]);
        let f = lower_one(&decl);
        let output = format!("{}", f);
        assert!(output.contains("func factorial"));
        assert!(output.contains("n: i32"));
        assert!(output.contains("-> i32"));
        assert!(output.contains("bb0:"));
        assert!(output.contains("return"));
    }

    #[test]
    fn e2e_nested_calls() {
        let g = make_fn("g", vec![("a", "i32")], Some("i32"), vec![return_stmt(Some(ident_expr("a")))]);
        let h = make_fn("h", vec![("a", "i32")], Some("i32"), vec![return_stmt(Some(ident_expr("a")))]);
        let decl = make_fn("f", vec![("x", "i32")], Some("i32"), vec![
            return_stmt(Some(call_expr("g", vec![call_expr("h", vec![ident_expr("x")])]))),
        ]);
        let all = vec![decl.clone(), g, h];
        let f = lower(&decl, &all);
        assert!(find_call(&f, "g"));
        assert!(find_call(&f, "h"));
    }

    #[test]
    fn e2e_assert_generates_branch() {
        let decl = make_fn("f", vec![("x", "i32")], None, vec![
            expr_stmt(Expr {
                id: NodeId(500),
                kind: ExprKind::Assert {
                    condition: Box::new(binary_expr(BinOp::Gt, ident_expr("x"), int_expr(0))),
                    message: Some(Box::new(string_expr("x must be positive"))),
                },
                span: sp(),
            }),
            return_stmt(None),
        ]);
        let f = lower_one(&decl);
        assert!(has_branch(&f));
        assert!(find_call(&f, "assert_fail"));
        assert!(f.blocks.iter().any(|b| matches!(b.terminator, MirTerminator::Unreachable)));
    }

    #[test]
    fn e2e_cast_expression() {
        let decl = make_fn("f", vec![("x", "i32")], Some("i64"), vec![
            return_stmt(Some(Expr {
                id: NodeId(600),
                kind: ExprKind::Cast {
                    expr: Box::new(ident_expr("x")),
                    ty: "i64".to_string(),
                },
                span: sp(),
            })),
        ]);
        let f = lower_one(&decl);
        let has_cast = f.blocks.iter().any(|b| {
            b.statements.iter().any(|s| matches!(s, MirStmt::Assign { rvalue: MirRValue::Cast { .. }, .. }))
        });
        assert!(has_cast);
    }

    // ═══════════════════════════════════════════════════════════
    // Type constructors + enum variants
    // ═══════════════════════════════════════════════════════════

    fn lower_with_ctx(decl: &Decl, all_decls: &[Decl], ctx: &MirContext) -> MirFunction {
        let mut fns = MirLowerer::lower_function(decl, all_decls, ctx).expect("lowering failed");
        fns.remove(0)
    }

    fn find_store(f: &MirFunction) -> bool {
        f.blocks.iter().any(|b| {
            b.statements.iter().any(|s| matches!(s, MirStmt::Store { .. }))
        })
    }

    fn count_stores(f: &MirFunction) -> usize {
        f.blocks.iter()
            .flat_map(|b| b.statements.iter())
            .filter(|s| matches!(s, MirStmt::Store { .. }))
            .count()
    }

    #[test]
    fn lower_enum_variant_construct() {
        // Shape.Circle(5.0) → store tag 0, store payload f64
        use rask_mono::{EnumLayout, VariantLayout, FieldLayout};

        let shape_enum = EnumLayout {
            name: "Shape".to_string(),
            size: 16,
            align: 8,
            tag_ty: rask_types::Type::U8,
            tag_offset: 0,
            variants: vec![
                VariantLayout {
                    name: "Circle".to_string(),
                    tag: 0,
                    payload_offset: 8,
                    payload_size: 8,
                    fields: vec![FieldLayout {
                        name: "f0".to_string(),
                        ty: rask_types::Type::F64,
                        offset: 0,
                        size: 8,
                        align: 8,
                    }],
                },
                VariantLayout {
                    name: "Square".to_string(),
                    tag: 1,
                    payload_offset: 8,
                    payload_size: 8,
                    fields: vec![FieldLayout {
                        name: "f0".to_string(),
                        ty: rask_types::Type::F64,
                        offset: 0,
                        size: 8,
                        align: 8,
                    }],
                },
            ],
        };

        let enum_layouts = vec![shape_enum];
        let node_types = HashMap::new();
        let ctx = MirContext {
            struct_layouts: &[],
            enum_layouts: &enum_layouts,
            node_types: &node_types,
        };

        let decl = make_fn("f", vec![], None, vec![
            expr_stmt(method_call_expr(ident_expr("Shape"), "Circle", vec![float_expr(5.0)])),
            return_stmt(None),
        ]);
        let f = lower_with_ctx(&decl, &[decl.clone()], &ctx);

        // Should emit stores for tag + payload, not a Call
        assert!(find_store(&f));
        assert_eq!(count_stores(&f), 2); // tag store + payload store
        assert!(!find_call(&f, "Circle"));
    }

    #[test]
    fn lower_enum_variant_no_payload() {
        // Color.Red() → store tag only
        use rask_mono::{EnumLayout, VariantLayout};

        let color_enum = EnumLayout {
            name: "Color".to_string(),
            size: 1,
            align: 1,
            tag_ty: rask_types::Type::U8,
            tag_offset: 0,
            variants: vec![
                VariantLayout { name: "Red".to_string(), tag: 0, payload_offset: 0, payload_size: 0, fields: vec![] },
                VariantLayout { name: "Green".to_string(), tag: 1, payload_offset: 0, payload_size: 0, fields: vec![] },
                VariantLayout { name: "Blue".to_string(), tag: 2, payload_offset: 0, payload_size: 0, fields: vec![] },
            ],
        };

        let enum_layouts = vec![color_enum];
        let node_types = HashMap::new();
        let ctx = MirContext {
            struct_layouts: &[],
            enum_layouts: &enum_layouts,
            node_types: &node_types,
        };

        let decl = make_fn("f", vec![], None, vec![
            expr_stmt(method_call_expr(ident_expr("Color"), "Red", vec![])),
            return_stmt(None),
        ]);
        let f = lower_with_ctx(&decl, &[decl.clone()], &ctx);

        assert!(find_store(&f));
        assert_eq!(count_stores(&f), 1); // tag only
    }

    #[test]
    fn lower_enum_variant_multi_field() {
        // Msg.Pair(1, 2) → store tag + 2 payload fields
        use rask_mono::{EnumLayout, VariantLayout, FieldLayout};

        let msg_enum = EnumLayout {
            name: "Msg".to_string(),
            size: 12,
            align: 4,
            tag_ty: rask_types::Type::U8,
            tag_offset: 0,
            variants: vec![
                VariantLayout { name: "Empty".to_string(), tag: 0, payload_offset: 4, payload_size: 0, fields: vec![] },
                VariantLayout {
                    name: "Pair".to_string(),
                    tag: 1,
                    payload_offset: 4,
                    payload_size: 8,
                    fields: vec![
                        FieldLayout { name: "f0".to_string(), ty: rask_types::Type::I32, offset: 0, size: 4, align: 4 },
                        FieldLayout { name: "f1".to_string(), ty: rask_types::Type::I32, offset: 4, size: 4, align: 4 },
                    ],
                },
            ],
        };

        let enum_layouts = vec![msg_enum];
        let node_types = HashMap::new();
        let ctx = MirContext {
            struct_layouts: &[],
            enum_layouts: &enum_layouts,
            node_types: &node_types,
        };

        let decl = make_fn("f", vec![], None, vec![
            expr_stmt(method_call_expr(ident_expr("Msg"), "Pair", vec![int_expr(1), int_expr(2)])),
            return_stmt(None),
        ]);
        let f = lower_with_ctx(&decl, &[decl.clone()], &ctx);

        assert_eq!(count_stores(&f), 3); // tag + 2 fields
    }

    #[test]
    fn lower_static_method_call_on_type() {
        // Vec.new() → Call to Vec_new
        let decl = make_fn("f", vec![], None, vec![
            expr_stmt(method_call_expr(ident_expr("Vec"), "new", vec![])),
            return_stmt(None),
        ]);
        let f = lower_one(&decl);
        assert!(find_call(&f, "Vec_new"));
    }

    #[test]
    fn lower_string_static_method() {
        // string.new() → Call to string_new
        let decl = make_fn("f", vec![], None, vec![
            expr_stmt(method_call_expr(ident_expr("string"), "new", vec![])),
            return_stmt(None),
        ]);
        let f = lower_one(&decl);
        assert!(find_call(&f, "string_new"));
    }

    #[test]
    fn lower_method_on_value_still_works() {
        // a.add(b) where a is a local variable → BinaryOp (not static call)
        let decl = make_fn("f", vec![("a", "i32"), ("b", "i32")], Some("i32"), vec![
            return_stmt(Some(method_call_expr(ident_expr("a"), "add", vec![ident_expr("b")]))),
        ]);
        let f = lower_one(&decl);
        assert!(find_assign_binop(&f));
        assert!(!find_call(&f, "i32_add"));
    }
}
