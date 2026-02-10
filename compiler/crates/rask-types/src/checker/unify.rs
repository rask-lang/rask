// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Constraint solving and type unification.

use rask_ast::Span;

use super::inference::TypeConstraint;
use super::errors::TypeError;
use super::TypeChecker;

use crate::types::{GenericArg, Type};

impl TypeChecker {
    pub(super) fn solve_constraints(&mut self) {
        let mut changed = true;
        let mut iterations = 0;
        const MAX_ITERATIONS: usize = 100;

        while changed && iterations < MAX_ITERATIONS {
            changed = false;
            iterations += 1;

            let constraints = std::mem::take(&mut self.ctx.constraints);
            for constraint in constraints {
                match self.solve_constraint(constraint) {
                    Ok(true) => changed = true,
                    Ok(false) => {}
                    Err(e) => self.errors.push(e),
                }
            }
        }
    }

    pub(super) fn solve_constraint(&mut self, constraint: TypeConstraint) -> Result<bool, TypeError> {
        match constraint {
            TypeConstraint::Equal(t1, t2, span) => self.unify(&t1, &t2, span),
            TypeConstraint::HasField {
                ty,
                field,
                expected,
                span,
            } => self.resolve_field(ty, field, expected, span),
            TypeConstraint::HasMethod {
                ty,
                method,
                args,
                ret,
                span,
            } => self.resolve_method(ty, method, args, ret, span),
        }
    }

    pub(super) fn unify(&mut self, t1: &Type, t2: &Type, span: Span) -> Result<bool, TypeError> {
        let t1 = self.ctx.apply(t1);
        let t2 = self.ctx.apply(t2);

        match (&t1, &t2) {
            (a, b) if a == b => Ok(false),

            // Empty tuple and Unit are equivalent
            (Type::Tuple(elems), Type::Unit) | (Type::Unit, Type::Tuple(elems))
                if elems.is_empty() =>
            {
                Ok(false)
            }

            (Type::Var(id), other) => {
                if self.ctx.occurs_in(*id, other) {
                    return Err(TypeError::InfiniteType {
                        var: *id,
                        ty: other.clone(),
                        span,
                    });
                }
                self.ctx.substitutions.insert(*id, other.clone());
                Ok(true)
            }

            (other, Type::Var(id)) => {
                if self.ctx.occurs_in(*id, other) {
                    return Err(TypeError::InfiniteType {
                        var: *id,
                        ty: other.clone(),
                        span,
                    });
                }
                self.ctx.substitutions.insert(*id, other.clone());
                Ok(true)
            }

            (Type::Generic { base: b1, args: a1 }, Type::Generic { base: b2, args: a2 }) => {
                if b1 != b2 || a1.len() != a2.len() {
                    return Err(TypeError::Mismatch {
                        expected: t1,
                        found: t2,
                        span,
                    });
                }
                let mut progress = false;
                for (arg1, arg2) in a1.iter().zip(a2.iter()) {
                    if self.unify_generic_arg(arg1, arg2, span)? {
                        progress = true;
                    }
                }
                Ok(progress)
            }

            // Function types
            (
                Type::Fn {
                    params: p1,
                    ret: r1,
                },
                Type::Fn {
                    params: p2,
                    ret: r2,
                },
            ) => {
                if p1.len() != p2.len() {
                    return Err(TypeError::Mismatch {
                        expected: t1,
                        found: t2,
                        span,
                    });
                }
                let mut progress = false;
                for (param1, param2) in p1.iter().zip(p2.iter()) {
                    if self.unify(param1, param2, span)? {
                        progress = true;
                    }
                }
                if self.unify(r1, r2, span)? {
                    progress = true;
                }
                Ok(progress)
            }

            (Type::Tuple(e1), Type::Tuple(e2)) => {
                if e1.len() != e2.len() {
                    return Err(TypeError::Mismatch {
                        expected: t1,
                        found: t2,
                        span,
                    });
                }
                let mut progress = false;
                for (elem1, elem2) in e1.iter().zip(e2.iter()) {
                    if self.unify(elem1, elem2, span)? {
                        progress = true;
                    }
                }
                Ok(progress)
            }

            (Type::Option(inner1), Type::Option(inner2)) => self.unify(inner1, inner2, span),

            (
                Type::Result { ok: o1, err: e1 },
                Type::Result { ok: o2, err: e2 },
            ) => {
                let p1 = self.unify(o1, o2, span)?;
                let p2 = self.unify(e1, e2, span)?;
                Ok(p1 || p2)
            }

            (
                Type::Array {
                    elem: e1,
                    len: l1,
                },
                Type::Array {
                    elem: e2,
                    len: l2,
                },
            ) => {
                // len 0 is a placeholder for comptime-dependent sizes
                if l1 != l2 && *l1 != 0 && *l2 != 0 {
                    return Err(TypeError::Mismatch {
                        expected: t1,
                        found: t2,
                        span,
                    });
                }
                self.unify(e1, e2, span)
            }

            (Type::Slice(e1), Type::Slice(e2)) => self.unify(e1, e2, span),

            (Type::Error, _) | (_, Type::Error) => Ok(false),

            (Type::Never, _) => Ok(false),
            (_, Type::Never) => Ok(false),

            (Type::Result { ok: _, err: _ }, Type::Named(id)) | (Type::Named(id), Type::Result { ok: _, err: _ }) => {
                if Some(*id) == self.types.get_result_type_id() {
                    Ok(false)
                } else {
                    Err(TypeError::Mismatch {
                        expected: t1,
                        found: t2,
                        span,
                    })
                }
            }

            (Type::Option(_inner), Type::Named(id)) | (Type::Named(id), Type::Option(_inner)) => {
                if Some(*id) == self.types.get_option_type_id() {
                    Ok(false)
                } else {
                    Err(TypeError::Mismatch {
                        expected: t1,
                        found: t2,
                        span,
                    })
                }
            }

            (Type::UnresolvedNamed(_), _) | (_, Type::UnresolvedNamed(_)) => {
                self.ctx
                    .add_constraint(TypeConstraint::Equal(t1, t2, span));
                Ok(false)
            }

            _ => Err(TypeError::Mismatch {
                expected: t1,
                found: t2,
                span,
            }),
        }
    }

    pub(super) fn unify_generic_arg(&mut self, arg1: &GenericArg, arg2: &GenericArg, span: Span) -> Result<bool, TypeError> {
        match (arg1, arg2) {
            (GenericArg::Type(t1), GenericArg::Type(t2)) => self.unify(t1, t2, span),
            (GenericArg::ConstUsize(n1), GenericArg::ConstUsize(n2)) => {
                if n1 == n2 {
                    Ok(false)
                } else {
                    Err(TypeError::GenericError(
                        format!("const generic mismatch: {} vs {}", n1, n2),
                        span,
                    ))
                }
            }
            _ => Err(TypeError::Mismatch {
                expected: Type::Error,  // TODO: Better error representation
                found: Type::Error,
                span,
            }),
        }
    }
}
