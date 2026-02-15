// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Pattern type checking.

use rask_ast::expr::Pattern;
use rask_ast::Span;

use super::errors::TypeError;
use super::inference::TypeConstraint;
use super::type_defs::TypeDef;
use super::TypeChecker;

use crate::types::Type;

impl TypeChecker {
    // ------------------------------------------------------------------------
    // Pattern Checking
    // ------------------------------------------------------------------------

    pub(super) fn check_pattern(&mut self, pattern: &Pattern, scrutinee_ty: &Type, span: Span) -> Vec<(String, Type)> {
        match pattern {
            Pattern::Wildcard => vec![],

            Pattern::Ident(name) => {
                // Qualified enum variant (e.g., "Status.Active") — match, don't bind
                if name.contains('.') {
                    return self.check_constructor_pattern(name, &[], scrutinee_ty, span);
                }
                vec![(name.clone(), scrutinee_ty.clone())]
            }

            Pattern::Literal(expr) => {
                let lit_ty = self.infer_expr(expr);
                self.ctx.add_constraint(TypeConstraint::Equal(
                    scrutinee_ty.clone(),
                    lit_ty,
                    span,
                ));
                vec![]
            }

            Pattern::Constructor { name, fields } => {
                self.check_constructor_pattern(name, fields, scrutinee_ty, span)
            }

            Pattern::Struct { name, fields, .. } => {
                // Look up the struct type
                if let Some(type_id) = self.types.get_type_id(name) {
                    // Constrain scrutinee to be this struct type
                    self.ctx.add_constraint(TypeConstraint::Equal(
                        scrutinee_ty.clone(),
                        Type::Named(type_id),
                        span,
                    ));
                    // Check each field pattern
                    let struct_fields = self.types.get(type_id).and_then(|def| {
                        if let TypeDef::Struct { fields, .. } = def {
                            Some(fields.clone())
                        } else {
                            None
                        }
                    });
                    let mut bindings = vec![];
                    if let Some(struct_fields) = struct_fields {
                        for (field_name, field_pattern) in fields {
                            let field_ty = struct_fields
                                .iter()
                                .find(|(n, _)| n == field_name)
                                .map(|(_, t)| t.clone())
                                .unwrap_or_else(|| {
                                    self.errors.push(TypeError::NoSuchField {
                                        ty: Type::Named(type_id),
                                        field: field_name.clone(),
                                        span,
                                    });
                                    Type::Error
                                });
                            bindings.extend(self.check_pattern(field_pattern, &field_ty, span));
                        }
                    }
                    bindings
                } else {
                    let mut bindings = vec![];
                    for (_, field_pattern) in fields {
                        let fresh = self.ctx.fresh_var();
                        bindings.extend(self.check_pattern(field_pattern, &fresh, span));
                    }
                    bindings
                }
            }

            Pattern::Tuple(patterns) => {
                let elem_types: Vec<_> = patterns.iter().map(|_| self.ctx.fresh_var()).collect();
                self.ctx.add_constraint(TypeConstraint::Equal(
                    scrutinee_ty.clone(),
                    Type::Tuple(elem_types.clone()),
                    span,
                ));
                let mut bindings = vec![];
                for (pat, elem_ty) in patterns.iter().zip(elem_types.iter()) {
                    bindings.extend(self.check_pattern(pat, elem_ty, span));
                }
                bindings
            }

            Pattern::Or(alternatives) => {
                if let Some(first) = alternatives.first() {
                    let bindings = self.check_pattern(first, scrutinee_ty, span);
                    for alt in &alternatives[1..] {
                        let _alt_bindings = self.check_pattern(alt, scrutinee_ty, span);
                        // TODO: verify same names and compatible types
                    }
                    bindings
                } else {
                    vec![]
                }
            }
        }
    }

    pub(super) fn check_constructor_pattern(
        &mut self,
        name: &str,
        fields: &[Pattern],
        scrutinee_ty: &Type,
        span: Span,
    ) -> Vec<(String, Type)> {
        let resolved_scrutinee = self.ctx.apply(scrutinee_ty);

        match name {
            "Ok" => {
                match &resolved_scrutinee {
                    Type::Result { ok, .. } => {
                        if fields.len() == 1 {
                            return self.check_pattern(&fields[0], ok, span);
                        }
                    }
                    Type::Var(_) => {
                        let ok_ty = self.ctx.fresh_var();
                        let err_ty = self.ctx.fresh_var();
                        self.ctx.add_constraint(TypeConstraint::Equal(
                            scrutinee_ty.clone(),
                            Type::Result {
                                ok: Box::new(ok_ty.clone()),
                                err: Box::new(err_ty),
                            },
                            span,
                        ));
                        if fields.len() == 1 {
                            return self.check_pattern(&fields[0], &ok_ty, span);
                        }
                    }
                    _ => {}
                }
            }
            "Err" => {
                match &resolved_scrutinee {
                    Type::Result { err, .. } => {
                        if fields.len() == 1 {
                            return self.check_pattern(&fields[0], err, span);
                        }
                    }
                    Type::Var(_) => {
                        let ok_ty = self.ctx.fresh_var();
                        let err_ty = self.ctx.fresh_var();
                        self.ctx.add_constraint(TypeConstraint::Equal(
                            scrutinee_ty.clone(),
                            Type::Result {
                                ok: Box::new(ok_ty),
                                err: Box::new(err_ty.clone()),
                            },
                            span,
                        ));
                        if fields.len() == 1 {
                            return self.check_pattern(&fields[0], &err_ty, span);
                        }
                    }
                    _ => {}
                }
            }
            "Some" => {
                match &resolved_scrutinee {
                    Type::Option(inner) => {
                        if fields.len() == 1 {
                            return self.check_pattern(&fields[0], inner, span);
                        }
                    }
                    Type::Var(_) => {
                        let inner_ty = self.ctx.fresh_var();
                        self.ctx.add_constraint(TypeConstraint::Equal(
                            scrutinee_ty.clone(),
                            Type::Option(Box::new(inner_ty.clone())),
                            span,
                        ));
                        if fields.len() == 1 {
                            return self.check_pattern(&fields[0], &inner_ty, span);
                        }
                    }
                    _ => {}
                }
            }
            "None" => {
                if fields.is_empty() {
                    // Constrain scrutinee to Option unless already known to be one.
                    // Var types need the constraint too — otherwise a standalone
                    // None arm won't propagate the Option requirement.
                    if !matches!(&resolved_scrutinee, Type::Option(_)) {
                        let inner_ty = self.ctx.fresh_var();
                        self.ctx.add_constraint(TypeConstraint::Equal(
                            scrutinee_ty.clone(),
                            Type::Option(Box::new(inner_ty)),
                            span,
                        ));
                    }
                    return vec![];
                }
            }
            _ => {}
        }

        match &resolved_scrutinee {
            Type::Named(type_id) => {
                let variant_fields = self.types.get(*type_id).and_then(|def| {
                    if let TypeDef::Enum { variants, .. } = def {
                        variants.iter()
                            .find(|(n, _)| n == name)
                            .map(|(_, f)| f.clone())
                    } else {
                        None
                    }
                });

                if let Some(variant_field_types) = variant_fields {
                    if fields.len() != variant_field_types.len() {
                        self.errors.push(TypeError::ArityMismatch {
                            expected: variant_field_types.len(),
                            found: fields.len(),
                            span,
                        });
                        return vec![];
                    }
                    let mut bindings = vec![];
                    for (pat, field_ty) in fields.iter().zip(variant_field_types.iter()) {
                        bindings.extend(self.check_pattern(pat, field_ty, span));
                    }
                    return bindings;
                }
            }
            _ => {}
        }

        let mut bindings = vec![];
        for pat in fields {
            let fresh = self.ctx.fresh_var();
            bindings.extend(self.check_pattern(pat, &fresh, span));
        }
        bindings
    }
}
