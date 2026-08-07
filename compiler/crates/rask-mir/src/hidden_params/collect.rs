// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Phase 1: Collect context requirements from `using` clauses.

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_types::Type;

use super::{ContextReq, FuncInfo, HiddenParamPass};

impl HiddenParamPass<'_> {
    /// Collect explicit context requirements from all function declarations.
    pub fn collect_contexts(&mut self, decls: &[Decl]) {
        for decl in decls {
            match &decl.kind {
                DeclKind::Fn(f) => {
                    self.collect_fn_context(&f.name, f, None);
                }
                DeclKind::Struct(s) => {
                    for method in &s.methods {
                        let qname = format!("{}.{}", s.name, method.name);
                        self.collect_fn_context(&qname, method, Some(&s.name));
                    }
                }
                DeclKind::Enum(e) => {
                    for method in &e.methods {
                        let qname = format!("{}.{}", e.name, method.name);
                        self.collect_fn_context(&qname, method, Some(&e.name));
                    }
                }
                DeclKind::Impl(i) => {
                    for method in &i.methods {
                        let qname = format!("{}.{}", i.target_ty, method.name);
                        self.collect_fn_context(&qname, method, Some(&i.target_ty));
                    }
                }
                DeclKind::Trait(t) => {
                    for method in &t.methods {
                        let qname = format!("{}.{}", t.name, method.name);
                        self.collect_fn_context(&qname, method, Some(&t.name));
                    }
                }
                _ => {}
            }
        }
    }

    fn collect_fn_context(&mut self, qname: &str, f: &FnDecl, self_type: Option<&str>) {
        if f.is_pub {
            self.public_funcs.insert(qname.to_string());
        }

        // Parameter types, resolved through the type table.
        let params: Vec<(String, Type)> = f
            .params
            .iter()
            .map(|p| (p.name.clone(), self.resolve_ty_str(&p.ty)))
            .collect();

        // Local variable types (from annotations or inferred node types).
        let mut locals = Vec::new();
        for stmt in &f.body {
            self.collect_locals_from_stmt(stmt, &mut locals);
        }

        // Fields of `self` (if a method on a struct).
        let self_fields: Vec<(String, Type)> = self_type
            .and_then(|ty_name| self.struct_fields.get(ty_name).cloned())
            .unwrap_or_default()
            .into_iter()
            .map(|(name, ty_str)| (name, self.resolve_ty_str(&ty_str)))
            .collect();

        // Explicit context requirements. Runtime contexts (Multitasking,
        // ThreadPool) use the process-global slot, not hidden params.
        let reqs: Vec<ContextReq> = f
            .context_clauses
            .iter()
            .filter(|cc| !is_runtime_context(&cc.ty))
            .map(|cc| self.context_clause_to_req(cc))
            .collect();

        if !reqs.is_empty() {
            self.func_contexts.insert(qname.to_string(), reqs.clone());
        }

        self.func_info.insert(
            qname.to_string(),
            FuncInfo {
                reqs,
                params,
                self_fields,
                locals,
            },
        );
    }

    /// Parse a type string, keeping the unresolved name as a fallback so a
    /// still-unregistered type never silently drops out of scope matching.
    fn resolve_ty_str(&self, s: &str) -> Type {
        self.parse_ty(s)
            .unwrap_or_else(|| Type::UnresolvedNamed(s.to_string()))
    }

    /// Record the type of every `const`/`mut` binding in a body, recursing into
    /// nested blocks. Annotated bindings parse their annotation; inferred ones
    /// take the type the checker recorded for the initializer.
    fn collect_locals_from_stmt(&self, stmt: &Stmt, locals: &mut Vec<(String, Type)>) {
        match &stmt.kind {
            StmtKind::Mut { name, ty, init, .. } | StmtKind::Let { name, ty, init, .. } => {
                let ty = match ty {
                    Some(ann) => self.parse_ty(ann),
                    None => self.node_ty(init.id),
                };
                if let Some(ty) = ty {
                    locals.push((name.clone(), ty));
                }
            }
            StmtKind::While { body, .. }
            | StmtKind::WhileLet { body, .. }
            | StmtKind::Loop { body, .. }
            | StmtKind::For { body, .. }
            | StmtKind::Comptime(body)
            | StmtKind::ComptimeFor { body, .. }
            | StmtKind::Ensure { body, .. } => {
                for s in body {
                    self.collect_locals_from_stmt(s, locals);
                }
            }
            _ => {}
        }
    }
}

/// True for runtime contexts that use the process-global slot, not hidden params.
pub(crate) fn is_runtime_context(ty: &str) -> bool {
    matches!(ty, "Multitasking" | "MultiTasking" | "multitasking" | "ThreadPool" | "threadpool")
}
