// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Pass 1: declaration collection and checking.

use rask_ast::decl::{Decl, DeclKind, EnumDecl, FnDecl, ImplDecl, StructDecl, TraitDecl};
use rask_resolve::SymbolKind;
use super::type_defs::{TypeDef, MethodSig, SelfParam, ParamMode};
use super::inference::TypeConstraint;
use super::parse_type::parse_type_string;
use super::TypeChecker;

use crate::types::Type;

impl TypeChecker {
    // ------------------------------------------------------------------------
    // Pass 1: Declaration Collection
    // ------------------------------------------------------------------------

    pub(super) fn collect_type_declarations(&mut self, decls: &[Decl]) {
        for decl in decls {
            match &decl.kind {
                DeclKind::Struct(s) => self.register_struct(s),
                DeclKind::Enum(e) => self.register_enum(e),
                DeclKind::Trait(t) => self.register_trait(t),
                DeclKind::Fn(f) if !f.type_params.is_empty() => {
                    // Find this function's SymbolId by matching name + Function kind
                    let type_param_names: Vec<String> = f.type_params.iter()
                        .map(|p| p.name.clone())
                        .collect();
                    if let Some(sym) = self.resolved.symbols.iter()
                        .find(|s| s.name == f.name && matches!(s.kind, SymbolKind::Function { .. }))
                    {
                        self.fn_type_params.insert(sym.id, type_param_names);
                    }
                }
                _ => {}
            }
        }
        for decl in decls {
            if let DeclKind::Impl(i) = &decl.kind {
                self.register_impl_methods(i);
            }
        }
    }

    pub(super) fn register_impl_methods(&mut self, i: &ImplDecl) {
        let base_name = i.target_ty.split('<').next().unwrap_or(&i.target_ty);
        let type_id = match self.types.get_type_id(base_name) {
            Some(id) => id,
            None => return,
        };
        let new_methods: Vec<_> = i.methods.iter().map(|m| self.method_signature(m)).collect();
        if let Some(def) = self.types.get_mut(type_id) {
            match def {
                TypeDef::Struct { methods, .. } | TypeDef::Enum { methods, .. } => {
                    methods.extend(new_methods);
                }
                _ => {}
            }
        }
    }

    pub(super) fn register_struct(&mut self, s: &StructDecl) {
        let fields: Vec<_> = s
            .fields
            .iter()
            .map(|f| {
                let ty = parse_type_string(&f.ty, &self.types).unwrap_or(Type::Error);
                (f.name.clone(), ty)
            })
            .collect();

        let methods = s.methods.iter().map(|m| self.method_signature(m)).collect();

        let type_params: Vec<String> = s.type_params.iter().map(|p| p.name.clone()).collect();
        let is_resource = s.attrs.iter().any(|a| a == "resource");
        self.types.register_type(TypeDef::Struct {
            name: s.name.clone(),
            type_params,
            fields,
            methods,
            is_resource,
        });
    }

    pub(super) fn register_enum(&mut self, e: &EnumDecl) {
        let variants: Vec<_> = e
            .variants
            .iter()
            .map(|v| {
                let field_types: Vec<_> = v
                    .fields
                    .iter()
                    .map(|f| parse_type_string(&f.ty, &self.types).unwrap_or(Type::Error))
                    .collect();
                (v.name.clone(), field_types)
            })
            .collect();

        let methods = e.methods.iter().map(|m| self.method_signature(m)).collect();

        let type_params: Vec<String> = e.type_params.iter().map(|p| p.name.clone()).collect();
        self.types.register_type(TypeDef::Enum {
            name: e.name.clone(),
            type_params,
            variants,
            methods,
        });
    }

    pub(super) fn register_trait(&mut self, t: &TraitDecl) {
        let methods = t.methods.iter().map(|m| self.method_signature(m)).collect();

        self.types.register_type(TypeDef::Trait {
            name: t.name.clone(),
            super_traits: t.super_traits.clone(),
            methods,
        });
    }

    pub(super) fn method_signature(&self, m: &FnDecl) -> MethodSig {
        let self_param_decl = m.params.iter().find(|p| p.name == "self");
        let self_param = match self_param_decl {
            Some(p) if p.is_take => SelfParam::Take,
            Some(p) if p.is_mutate => SelfParam::Mutate,
            Some(_) => SelfParam::Value,
            None => SelfParam::None,
        };

        let params: Vec<_> = m
            .params
            .iter()
            .filter(|p| p.name != "self")
            .map(|p| {
                let ty = parse_type_string(&p.ty, &self.types).unwrap_or(Type::Error);
                let mode = if p.is_take {
                    ParamMode::Take
                } else if p.is_mutate {
                    ParamMode::Mutate
                } else {
                    ParamMode::Default
                };
                (ty, mode)
            })
            .collect();

        let ret = m
            .ret_ty
            .as_ref()
            .map(|t| parse_type_string(t, &self.types).unwrap_or(Type::Error))
            .unwrap_or(Type::Unit);

        MethodSig {
            name: m.name.clone(),
            self_param,
            params,
            ret,
        }
    }

    // ------------------------------------------------------------------------
    // Pass 2: Check Declarations
    // ------------------------------------------------------------------------

    pub(super) fn check_decl(&mut self, decl: &Decl) {
        match &decl.kind {
            DeclKind::Fn(f) => self.check_fn(f),
            DeclKind::Struct(s) => {
                self.current_self_type = self.types.get_type_id(&s.name).map(Type::Named);
                for method in &s.methods {
                    self.check_fn(method);
                }
                self.current_self_type = None;
            }
            DeclKind::Enum(e) => {
                self.current_self_type = self.types.get_type_id(&e.name).map(Type::Named);
                for method in &e.methods {
                    self.check_fn(method);
                }
                self.current_self_type = None;
            }
            DeclKind::Impl(i) => {
                self.current_self_type = self.resolve_impl_self_type(&i.target_ty);
                for method in &i.methods {
                    self.check_fn(method);
                }
                self.current_self_type = None;
            }
            DeclKind::Const(c) => {
                let init_ty = self.infer_expr(&c.init);
                if let Some(ty_str) = &c.ty {
                    if let Ok(declared) = parse_type_string(ty_str, &self.types) {
                        self.ctx.add_constraint(TypeConstraint::Equal(
                            declared,
                            init_ty,
                            decl.span,
                        ));
                    }
                }
            }
            DeclKind::Test(t) => {
                for stmt in &t.body {
                    self.check_stmt(stmt);
                }
            }
            DeclKind::Benchmark(b) => {
                for stmt in &b.body {
                    self.check_stmt(stmt);
                }
            }
            DeclKind::Import(imp) => {
                // Register module name as local for field/method resolution.
                // Modules handled by BuiltinModules (net, json, fs) route through
                // check_method_call directly. Others like 'time' need local registration
                // so field access (time.Instant) flows through resolve_field.
                if imp.path.len() == 1 {
                    let module_name = imp.alias.as_ref().unwrap_or(&imp.path[0]).clone();
                    if !self.types.builtin_modules.is_module(&module_name) {
                        self.define_local(
                            module_name.clone(),
                            Type::UnresolvedNamed(format!("__module_{}", module_name)),
                        );
                    }
                }
            }
            _ => {}
        }
    }
}
