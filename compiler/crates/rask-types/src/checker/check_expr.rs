// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Expression type inference and specific type checks.

use rask_ast::coercion::CoercionSite;
use rask_ast::expr::{BinOp, CallArg, ConvertKind, Expr, ExprKind, MatchArm, Pattern};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::{NodeId, Span};
use rask_resolve::{SymbolId, SymbolKind};

use super::type_defs::TypeDef;
use super::borrow::BorrowMode;
use super::errors::{IndexErrorKind, InvalidCastClass, TypeError};
use super::inference::{LiteralKind, TypeConstraint};
use super::parse_type::parse_type_string;
use super::TypeChecker;

use crate::types::{GenericArg, Type};

/// Split a type argument string by commas, respecting nested angle brackets.
/// "Map<string, bool>, i64" → ["Map<string, bool>", "i64"]
fn split_type_args(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => {
                args.push(s[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    let last = s[start..].trim();
    if !last.is_empty() {
        args.push(last.to_string());
    }
    args
}

/// Parse a type argument string into a Type, handling nested generics.
/// "Map<string, bool>" → UnresolvedGeneric { name: "Map", args: [string, bool] }
/// "Route" → UnresolvedNamed("Route")
fn parse_type_arg(s: &str) -> Type {
    if let Some(open) = s.find('<') {
        let base = &s[..open];
        let inner = &s[open+1..s.len()-1];
        let args = split_type_args(inner)
            .into_iter()
            .map(|a| GenericArg::Type(Box::new(parse_type_arg(&a))))
            .collect();
        Type::UnresolvedGeneric {
            name: base.to_string(),
            args,
        }
    } else {
        // Map primitive names directly; otherwise they leak as UnresolvedNamed
        // and method resolution can't dispatch (e.g. `Vec<i32>.new()` followed
        // by `v[0] + 5` looking up `i32.add`).
        match s {
            "i8" => Type::I8,
            "i16" => Type::I16,
            "i32" => Type::I32,
            "i64" | "int" => Type::I64,
            "isize" => Type::isize_ty(),
            "i128" => Type::I128,
            "u8" => Type::U8,
            "u16" => Type::U16,
            "u32" => Type::U32,
            "u64" | "uint" => Type::U64,
            "usize" => Type::usize_ty(),
            "u128" => Type::U128,
            "f32" => Type::F32,
            "f64" => Type::F64,
            "bool" => Type::Bool,
            "char" => Type::Char,
            "string" => Type::String,
            "void" | "()" => Type::Unit,
            "none" => Type::None,
            _ => Type::UnresolvedNamed(s.to_string()),
        }
    }
}

impl TypeChecker {
    /// Walk a block body and return the type it produces.
    ///
    /// Every statement is walked exactly once. A trailing expression statement
    /// is walked in *value* position (`infer_expr`) instead of statement
    /// position (`check_stmt`) — never both. Doing both is what printed the
    /// same error twice at the same span: once per walk, and four times at two
    /// levels of nesting, since each block's last statement is re-walked by its
    /// parent (#695).
    ///
    /// The two walks were never interchangeable. `check_stmt` sets
    /// `in_stmt_expr`, which makes a trailing `if` return unit and leaves its
    /// branches unconstrained. Value position is the right one for a block's
    /// result, so that's the one kept — plus the two things `check_stmt` would
    /// have done afterwards for an expression statement.
    pub(super) fn check_block_body(&mut self, body: &[Stmt]) -> Type {
        let value_stmt_idx = match body.last() {
            Some(last) if matches!(last.kind, StmtKind::Expr(_)) => Some(body.len() - 1),
            _ => None,
        };
        for (i, stmt) in body.iter().enumerate() {
            if Some(i) == value_stmt_idx {
                break;
            }
            self.check_stmt(stmt);
            self.solve_constraints();
        }
        match body.last() {
            Some(last) => match &last.kind {
                StmtKind::Expr(e) => {
                    let ty = self.infer_expr(e);
                    self.check_bare_sync_access(e);
                    self.clear_expression_borrows();
                    self.solve_constraints();
                    ty
                }
                StmtKind::Return(_) | StmtKind::Break { .. } | StmtKind::Continue(_) => Type::Never,
                _ => Type::Unit,
            },
            None => Type::Unit,
        }
    }
    /// Infer expression type with an expected type hint for unsuffixed literals.
    /// Falls through to normal inference for non-literal or suffixed expressions.
    pub(super) fn infer_expr_expecting(&mut self, expr: &Expr, expected: &Type) -> Type {
        // A bare number filling a `T?` slot is the payload, not the slot. An
        // optional expectation says nothing an integer literal can take, so the
        // literal stayed open and defaulted to `i32`: `a[1] = 5` into an
        // `[i64?; 3]` stored four bytes into an eight-byte payload, and the
        // upper half came back as whatever the stack held (#835).
        if matches!(expr.kind, ExprKind::Int(..) | ExprKind::Float(..)) {
            if let Some(inner) = expected.as_option() {
                let inner = inner.clone();
                return self.infer_expr_expecting(expr, &inner);
            }
        }
        match &expr.kind {
            // An unsuffixed literal takes the slot's type, including the two
            // magnitude bands that only rule types out. Taking the expectation
            // is what turns `let a: i128 = <too big>` into "this literal is out
            // of range for `i128`" instead of a type mismatch against whatever
            // the literal would have defaulted to.
            ExprKind::Int(value, suffix)
                if Self::is_integer_type(expected) && Self::int_literal_is_open(suffix) =>
            {
                let ty = expected.clone();
                self.node_types.insert(expr.id, ty.clone());
                let bit_pattern = Self::int_literal_is_bit_pattern(*value, suffix);
                self.pending_int_literals.push((*value, bit_pattern, ty.clone(), expr.span));
                return ty;
            }
            ExprKind::Float(_, None) if Self::is_float_type(expected) => {
                let ty = expected.clone();
                self.node_types.insert(expr.id, ty.clone());
                return ty;
            }
            // `none` carries no payload type of its own, so the slot it lands in
            // is the only thing that can say what it's an absent *what*. Typed
            // from itself it comes out `Option<?>` and the variable is still
            // there at the end — the unification that should have closed it runs
            // after the node has been recorded.
            ExprKind::None if expected.is_option() => {
                let ty = expected.clone();
                self.node_types.insert(expr.id, ty.clone());
                self.note_node_origin(expr);
                return ty;
            }
            // CV1a: push the expectation into a tuple literal's elements, so each
            // one is checked against the slot it fills and the tuple's recorded
            // type is the annotated shape. Typed from its elements instead, the
            // literal's node type stayed `(u32, u16)` while the binding was
            // `(i64, i32)` — element-wise unification accepted the widening, but
            // MIR then built the slot at the *source* layout and the destination
            // read fields that were never at those offsets.
            ExprKind::Tuple(elements) => {
                if let Type::Tuple(expected_elems) = expected {
                    if expected_elems.len() == elements.len() && !elements.is_empty() {
                        let elem_types: Vec<_> = elements
                            .iter()
                            .zip(expected_elems.iter())
                            .map(|(e, want)| self.infer_expr_expecting(e, want))
                            .collect();
                        // The expectation only wins where the element actually
                        // coerced to it; anything else keeps its own type so a
                        // genuine mismatch is still reported downstream.
                        // Any lossless scalar widening, not just integers. The
                        // float case can't type-check yet, so it changes nothing
                        // today — it's here so `(f64, f32)` doesn't go back to
                        // being laid out at its elements' widths the moment #624
                        // makes `f32` → `f64` implicit (#660).
                        let ty = Type::Tuple(
                            elem_types
                                .iter()
                                .zip(expected_elems.iter())
                                .map(|(got, want)| {
                                    let got_r = self.ctx.apply(got);
                                    if Self::is_lossless_scalar_widening(&got_r, want) {
                                        want.clone()
                                    } else {
                                        got.clone()
                                    }
                                })
                                .collect(),
                        );
                        self.node_types.insert(expr.id, ty.clone());
                        return ty;
                    }
                }
            }
            // `return try n` is often the only thing in a body that can pin a
            // generic stub's success type — `let n = s.parse(); return try n`
            // has nothing else to say what `n` is. The expectation never reached
            // `try`, so the coercion queued behind the return waited on a
            // variable that only that same coercion could ever bind. The solver
            // stops when a pass makes no progress and drops what's left without
            // a word, so the binding came out "type is still open here" (#961).
            ExprKind::Try { .. } => {
                let got = self.infer_expr(expr);
                // `try` peels one branch, so what's expected of it is the
                // success side of what's expected of the `return`.
                let want = match expected {
                    Type::Result { ok, .. } => (**ok).clone(),
                    other => other.clone(),
                };
                if matches!(self.ctx.apply(&got), Type::Var(_))
                    && !matches!(self.ctx.apply(&want), Type::Var(_))
                {
                    let _ = self.unify(&got, &want, expr.span);
                }
                self.note_trait_coercion(expr, expected, &got);
                return got;
            }
            // std.collections: `[1, 2, 3]` is a collection literal, and the slot
            // it lands in says which collection and what element type. Typed from
            // its own elements instead, `let xs: [i64?; 3] = [1, 2, 3]` reported
            // "expected `i64?`, found `i64`" — the literal never learned that its
            // elements fill optional slots, so no element could widen (#771).
            ExprKind::Array(elements) => {
                if let Some(want_elem) = self.collection_elem_type(expected) {
                    for element in elements.iter() {
                        let got = self.infer_expr_expecting(element, &want_elem);
                        self.coerce_into(
                            CoercionSite::CollectionElement,
                            got,
                            want_elem.clone(),
                            element.span,
                        );
                    }
                    // The literal's own type is the destination's shape with the
                    // element type it was given — MIR builds the slot from this,
                    // so a `[i64?; 3]` has to say so here. An empty literal has
                    // nothing of its own to say and takes the shape whole.
                    let ty = match expected {
                        Type::Array { .. } => Type::Array {
                            elem: Box::new(want_elem),
                            len: elements.len(),
                        },
                        other => other.clone(),
                    };
                    self.node_types.insert(expr.id, ty.clone());
                    return ty;
                }
            }
            _ => {}
        }
        let ty = self.infer_expr(expr);
        self.note_trait_coercion(expr, expected, &ty);
        ty
    }

    /// The element type a collection literal's members fill, for the shapes
    /// `[…]` can take: a fixed array, a slice, or a `Vec`.
    ///
    /// `None` when the destination isn't one of those, or when its element type
    /// is still open — an unresolved element says nothing to push into the
    /// members, and forcing one would pin them to a variable.
    fn collection_elem_type(&self, expected: &Type) -> Option<Type> {
        let elem = match expected {
            Type::Array { elem, .. } | Type::Slice(elem) => (**elem).clone(),
            Type::Generic { base, args } if self.types.type_name(*base).split('<').next() == Some("Vec") => {
                match args.first()? {
                    GenericArg::Type(t) => (**t).clone(),
                    _ => return None,
                }
            }
            Type::UnresolvedGeneric { name, args } if name.split('<').next() == Some("Vec") => {
                match args.first()? {
                    GenericArg::Type(t) => (**t).clone(),
                    _ => return None,
                }
            }
            _ => return None,
        };
        if matches!(elem, Type::Var(_) | Type::Error) {
            return None;
        }
        Some(elem)
    }

    /// A generic *enum* named with its type arguments written out —
    /// `Holder<i64>` — as the instantiated type. `None` for anything else, so an
    /// undefined variable still reports as one and a struct or container keeps
    /// whatever path it already took.
    fn spelled_out_enum_name(&self, name: &str) -> Option<Type> {
        if !name.contains('<') {
            return None;
        }
        let base = name.split('<').next()?.trim();
        let type_id = self.types.get_type_id(base)?;
        if !matches!(self.types.get(type_id), Some(TypeDef::Enum { .. })) {
            return None;
        }
        match parse_type_string(name, &self.types) {
            Ok(ty @ Type::Generic { .. }) => Some(ty),
            _ => None,
        }
    }

    /// The `any Trait` type arguments a container was instantiated with.
    fn trait_object_type_args(ty: &Type) -> Vec<Type> {
        let args = match ty {
            Type::Generic { args, .. } | Type::UnresolvedGeneric { args, .. } => args,
            _ => return Vec::new(),
        };
        args.iter()
            .filter_map(|a| match a {
                GenericArg::Type(t) if matches!(**t, Type::TraitObject { .. }) => {
                    Some((**t).clone())
                }
                _ => None,
            })
            .collect()
    }

    /// TR5: a concrete value flowing into an `any Trait` position gets boxed
    /// with a vtable. The site has to be recorded by NodeId or MIR emits the
    /// bare value and the first method call dispatches through whatever
    /// happened to be in memory.
    ///
    /// This used to be recorded for call arguments only. Every other position
    /// that knows its expected type — an annotated binding, a struct field, a
    /// collection element, a return value — type-checked and then segfaulted
    /// at the first method call (#335, #474, #481).
    /// An explicit `x as any Trait` boxes itself, so it needs no second box.
    fn is_any_cast(expr: &Expr) -> bool {
        matches!(&expr.kind, ExprKind::Cast { ty, .. } if ty.starts_with("any "))
    }

    /// The same question for a collection element whose container only settled
    /// after the call was walked. Runs after solving, from the same list of
    /// deferred checks as the rest.
    pub(super) fn validate_pending_trait_elem_coercions(&mut self) {
        let pending = std::mem::take(&mut self.pending_trait_elem_coercions);
        for (node, is_any_cast, recv_ty, arg_ty) in pending {
            let applied = self.ctx.apply(&arg_ty);
            for elem in Self::trait_object_type_args(&self.ctx.apply(&recv_ty)) {
                let Type::TraitObject { ref trait_name } = elem else { continue };
                if crate::traits::implements_trait(&self.types, &applied, trait_name) {
                    self.note_trait_coercion_node(node, is_any_cast, &elem, &arg_ty);
                }
            }
        }
    }

    pub(super) fn note_trait_coercion(&mut self, expr: &Expr, expected: &Type, found: &Type) {
        self.note_trait_coercion_node(expr.id, Self::is_any_cast(expr), expected, found)
    }

    fn note_trait_coercion_node(
        &mut self,
        node: rask_ast::NodeId,
        is_any_cast: bool,
        expected: &Type,
        found: &Type,
    ) {
        let Type::TraitObject { trait_name } = expected else { return };
        if is_any_cast {
            return;
        }
        if matches!(self.ctx.apply(found), Type::TraitObject { .. } | Type::Error) {
            return;
        }
        // Only a value that actually implements the trait gets a vtable for it.
        //
        // The expected type arrives here already peeled of its wrappers, so a
        // value that isn't destined for the `any Trait` side looks like one that
        // is. Two ways that went wrong, both ending in a vtable that can't be
        // built:
        //
        //   `-> (any Shape)?` with `return none` — expected peels to `any Shape`
        //   and `none` is an `Option<_>`. MIR built the none Option, gave *that* a
        //   vtable, and wrapped the box in a second Option: "vtable method
        //   unknown.area".
        //
        //   `-> (any Shape) or Nope` with `return Nope {}` — the err branch, but
        //   expected peels to the ok side: "vtable method Nope.area".
        //
        // A value that doesn't implement the trait is either the other branch or
        // a type error reported elsewhere; boxing it is wrong either way (#764).
        // Anything still unresolved keeps the old behaviour — `implements_trait`
        // can't answer for a variable, and refusing on "don't know" would drop
        // boxes the checker had accepted.
        let resolved = self.ctx.apply(found);
        let undecided = matches!(
            resolved,
            Type::Var(_) | Type::UnresolvedNamed(_) | Type::UnresolvedGeneric { .. }
        );
        if !undecided && !crate::traits::implements_trait(&self.types, &resolved, trait_name) {
            return;
        }
        self.trait_coercions.insert(node, trait_name.clone());
    }

    /// True when the literal's own spelling doesn't pin a type, so the slot it
    /// lands in decides. The magnitude markers count: they say what a literal
    /// *can't* be, not what it is.
    fn int_literal_is_open(suffix: &Option<rask_ast::token::IntSuffix>) -> bool {
        use rask_ast::token::IntSuffix;
        matches!(
            suffix,
            None
                | Some(IntSuffix::U64ByMagnitude)
                | Some(IntSuffix::I128ByMagnitude)
                | Some(IntSuffix::U128ByMagnitude)
        )
    }

    /// Whether the token carries a bit pattern rather than a number. Only one
    /// band does: a `u128` above `i128::MAX`, which is the single value range
    /// the `i128` a token travels in can't represent.
    fn int_literal_is_bit_pattern(
        value: i128,
        suffix: &Option<rask_ast::token::IntSuffix>,
    ) -> bool {
        use rask_ast::token::IntSuffix;
        matches!(suffix, Some(IntSuffix::U128ByMagnitude))
            || (matches!(suffix, Some(IntSuffix::U128)) && value < 0)
    }

    fn is_integer_type(ty: &Type) -> bool {
        matches!(ty, Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128
                    | Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128)
    }

    fn is_float_type(ty: &Type) -> bool {
        matches!(ty, Type::F32 | Type::F64)
    }

    pub(super) fn infer_expr(&mut self, expr: &Expr) -> Type {
        let ty = match &expr.kind {
            // Literals
            ExprKind::Int(value, suffix) => {
                use rask_ast::token::IntSuffix;
                let ty = match suffix {
                    Some(IntSuffix::I8) => Type::I8,
                    Some(IntSuffix::I16) => Type::I16,
                    Some(IntSuffix::I32) => Type::I32,
                    Some(IntSuffix::I64) => Type::I64,
                    Some(IntSuffix::Isize) => Type::isize_ty(),
                    Some(IntSuffix::U8) => Type::U8,
                    Some(IntSuffix::U16) => Type::U16,
                    Some(IntSuffix::U32) => Type::U32,
                    Some(IntSuffix::U64) => Type::U64,
                    Some(IntSuffix::I128) => Type::I128,
                    // Past `i128::MAX` only `u128` is left, so the magnitude
                    // does pin the type. Below that it just rules types *out* —
                    // `100000000000000000000` is as good a `u128` as an `i128`,
                    // so it stays open like any other unsuffixed literal and
                    // `validate_pending_int_literals` catches a bad landing.
                    Some(IntSuffix::U128) | Some(IntSuffix::U128ByMagnitude) => Type::U128,
                    Some(IntSuffix::Usize) => Type::usize_ty(),
                    None
                    | Some(IntSuffix::U64ByMagnitude)
                    | Some(IntSuffix::I128ByMagnitude) => {
                        let var = self.ctx.fresh_literal_var(LiteralKind::Integer);
                        // The default is i32 (type.primitives/L1), but a literal
                        // too big for i32 has to land somewhere it fits, or
                        // codegen silently keeps the low 32 bits.
                        if let Type::Var(id) = var {
                            self.ctx.record_literal_int(id, *value);
                        }
                        var
                    }
                };
                // Tokens carry an `i128`, so only the very top band — above
                // `i128::MAX`, where just `u128` reaches — still arrives as a
                // bit pattern. That shows up as a negative value under an
                // unsigned suffix; `-1u128` parses as `neg(1)`, never as
                // `Int(-1, u128)`, so the two cases don't collide.
                let bit_pattern = Self::int_literal_is_bit_pattern(*value, suffix);
                self.pending_int_literals.push((*value, bit_pattern, ty.clone(), expr.span));
                ty
            }
            ExprKind::Float(_, suffix) => {
                use rask_ast::token::FloatSuffix;
                match suffix {
                    Some(FloatSuffix::F32) => Type::F32,
                    Some(FloatSuffix::F64) => Type::F64,
                    None => self.ctx.fresh_literal_var(LiteralKind::Float),
                }
            }
            ExprKind::String(_) | ExprKind::StringInterp(_) => Type::String,
            ExprKind::Char(_) => Type::Char,
            ExprKind::Bool(_) => Type::Bool,
            ExprKind::Null => Type::RawPtr(Box::new(self.ctx.fresh_var())),
            // OPT3: `none` is `T?` with inner type inferred from context.
            ExprKind::None => Type::option(self.ctx.fresh_var()),

            ExprKind::Ident(name) => {
                // D1: use after discard is a compile error
                if let Some(&discard_span) = self.discarded_bindings.get(name.as_str()) {
                    self.errors.push(TypeError::UseAfterDiscard {
                        name: name.clone(),
                        discarded_at: discard_span,
                        span: expr.span,
                    });
                    return Type::Error;
                }
                // `Holder<i64>.Empty` — the parser folds written type arguments into
                // the name, so the object of that field access is an identifier
                // nobody declared. The resolver still points it at the enum's
                // symbol, whose type carries no arguments, so the variant reference
                // came out with a fresh variable for `T` and the binding was
                // "type is still open". A fieldless variant has no payload to infer
                // from, so the written arguments are the only place `T` can come
                // from. Ahead of the ordinary lookups because the resolver's answer
                // is the one that loses them — and no variable is spelled with
                // angle brackets (#782).
                if let Some(ty) = self.spelled_out_enum_name(name) {
                    return ty;
                }
                if let Some(ty) = self.lookup_local(name) {
                    // SH7 needs to know which names reached a task-local box and
                    // where. Recorded here rather than re-walked at the `spawn`,
                    // which would have to know every expression shape to be
                    // right; judged after solving, because right now the type of
                    // a `let c = Shared.new(0)` is usually still a variable.
                    let resolved = self.resolve_named(&self.ctx.apply(&ty));
                    if matches!(resolved, Type::Var(_))
                        || Self::type_is_shared(&resolved, &self.types)
                    {
                        self.local_shared_uses.push((name.clone(), ty.clone(), expr.span));
                    }
                    ty
                } else if let Some(&sym_id) = self.resolved.resolutions.get(&expr.id) {
                    self.get_symbol_type(sym_id)
                } else if let Some(type_id) = self.types.get_type_id(name) {
                    // Imported type name (struct/enum) without resolver entry
                    Type::Named(type_id)
                } else {
                    self.errors.push(TypeError::UndefinedName {
                        name: name.clone(),
                        span: expr.span,
                    });
                    Type::Error
                }
            }

            ExprKind::Binary { op, left, right } => {
                self.check_binary(*op, left, right, expr.span)
            }

            ExprKind::Unary { op, operand } => {
                let operand_ty = self.infer_expr(operand);
                match op {
                    rask_ast::expr::UnaryOp::Deref => {
                        // What `*x` means depends on what `x` is, not on the
                        // `*`. mem.unsafe's rule is about raw pointers; on an
                        // `Owned` it's an ordinary borrow that doesn't consume
                        // (mem.owned/OW3), which is how owned.md's own examples
                        // are written — and they didn't compile, because this
                        // fired on the syntax alone (#737).
                        //
                        // An operand whose type isn't worked out yet still
                        // demands `unsafe`: not knowing isn't the same as
                        // knowing it's safe.
                        let resolved = self.ctx.apply(&operand_ty);
                        let needs_unsafe = matches!(resolved, Type::RawPtr(_) | Type::Var(_));
                        // `*p = v` is one operation. The assignment reports it
                        // as a deref *write*, which is the more precise of the
                        // two; reporting the read here as well printed two
                        // errors on the same span for the same `*`.
                        if needs_unsafe && !self.in_assign_target {
                            self.unsafe_ops.push((expr.span, super::UnsafeCategory::PointerDeref));
                            if !self.in_unsafe {
                                self.errors.push(TypeError::UnsafeRequired {
                                    operation: "pointer dereference".to_string(),
                                    span: expr.span,
                                });
                            }
                        }
                        // *ptr where ptr: *T yields T. An `Owned<T>` is already
                        // `T` to the checker, so a borrow through it yields the
                        // same type it started with.
                        match resolved {
                            Type::RawPtr(inner) => *inner,
                            // The operand's type isn't settled yet — it came
                            // out of another call, as in `*nums.as_ptr()`.
                            // Returning the open var left the whole binding
                            // untypeable ("couldn't work out the type of
                            // `value`"). Say "you're a pointer to something"
                            // now and let solving fill the something in (#696).
                            Type::Var(_) => {
                                let pointee = self.ctx.fresh_var();
                                self.ctx.add_constraint(TypeConstraint::Equal(
                                    operand_ty,
                                    Type::RawPtr(Box::new(pointee.clone())),
                                    expr.span,
                                ));
                                pointee
                            }
                            _ => operand_ty,
                        }
                    }
                    rask_ast::expr::UnaryOp::Not => {
                        // `!` negates a bool. `T?` doesn't coerce to `T` (OPT5), so a
                        // `bool?` operand must be rejected here rather than lifted through —
                        // there's no way to tell "negate the payload" from "test for absence".
                        let resolved = self.ctx.apply(&operand_ty);
                        if resolved.is_option() {
                            self.errors.push(TypeError::NotOnOptional {
                                found: resolved,
                                span: expr.span,
                            });
                            Type::Bool
                        } else {
                            self.ctx.add_constraint(TypeConstraint::Equal(
                                Type::Bool,
                                operand_ty,
                                expr.span,
                            ));
                            Type::Bool
                        }
                    }
                    _ => operand_ty,
                }
            }

            // `in_stmt_expr` describes the *call's* position, not its
            // arguments'. It used to reach them, so in `out.push(if b: "1" else:
            // "0")` — a bare expression statement — the argument's `if` decided
            // it was in statement position and answered `void`. The same
            // expression in `let s = out.push(…)` was fine, because a `let`
            // never sets the flag.
            ExprKind::Call { func, args } => {
                self.in_stmt_expr = false;
                self.check_call(expr.id, func, args, expr.span)
            }

            ExprKind::MethodCall {
                object,
                method,
                args,
                type_args,
            } => {
                self.in_stmt_expr = false;
                self.check_method_call(expr.id, object, method, args, type_args.as_deref(), expr.span)
            }

            ExprKind::Field { object, field } => self.check_field_access(object, field, expr.span),

            ExprKind::DynamicField { object, field_expr } => {
                // CT49: `value.("x")` is a field access spelled with the name in
                // quotes. When the name is right there in the source, check it as
                // one — that gives the expression the field's real type, and makes
                // a name that doesn't exist the same error `value.x` would give
                // instead of something for MIR to trip over later (#930).
                if let Some(name) = Self::literal_field_name(field_expr) {
                    self.infer_expr(field_expr);
                    return self.check_field_access(object, &name, expr.span);
                }
                // Anything else — a `comptime for` binding's `.name`, a binding
                // holding a comptime string — is only known once the loop is
                // unrolled, which happens after this pass. What *can* be
                // decided here is whether it will ever be knowable: a name that
                // came out of a call or an `if` never will (CT53). Native said
                // so as a MIR lowering failure and the interpreter looked the
                // field up at run time and carried on, so the same program ran
                // on one backend and wouldn't build on the other (#996).
                let _obj_ty = self.infer_expr(object);
                let _field_ty = self.infer_expr(field_expr);
                if !self.comptime_field_name_shape(field_expr) {
                    // The whole access, not the name inside it: an
                    // interpolation reparses its expression and the
                    // sub-expressions come back without source spans, so a
                    // caret on `field_expr` lands at the top of the file.
                    self.errors.push(TypeError::DynamicFieldNameNotComptime {
                        span: expr.span,
                    });
                }
                Type::Error
            }

            ExprKind::Index { object, index } => {
                let raw_obj_ty = self.infer_expr(object);
                let idx_ty = self.infer_expr(index);

                // Check if indexing with a range (slicing)
                let is_range = matches!(index.kind, rask_ast::expr::ExprKind::Range { .. });

                // Resolve type variables so Generic{} is visible
                let obj_ty = self.ctx.apply(&raw_obj_ty);
                self.check_index_types(&obj_ty, &idx_ty, is_range, index.span);
                match self.index_result_type(&obj_ty, is_range) {
                    Some(elem) => elem,
                    None => {
                        // Shape unknown here — `state.entities[h]` waits on the
                        // field's type, which arrives as a deferred constraint of
                        // its own. Record the relationship rather than handing
                        // back a fresh variable with nothing tying it to the
                        // container, which left `let e = state.entities[h]` with
                        // an open type however the field later resolved (#632).
                        let elem = self.ctx.fresh_var();
                        self.ctx.add_constraint(TypeConstraint::Index {
                            object: raw_obj_ty,
                            // Carried so #310's index-type check can run again
                            // once the container is known. The call above had an
                            // unresolved container, so it classified nothing and
                            // a field-reached index went unchecked.
                            index: idx_ty.clone(),
                            result: elem.clone(),
                            is_range,
                            span: expr.span,
                        });
                        elem
                    }
                }
            }

            ExprKind::If {
                cond,
                then_branch,
                else_branch,
                else_binding,
            } => {
                // Capture and clear the statement-position flag so it doesn't
                // leak into nested expressions (e.g. const x = if ... { ... }).
                let is_stmt = self.in_stmt_expr;
                self.in_stmt_expr = false;

                let cond_ty = self.infer_expr(cond);
                self.ctx
                    .add_constraint(TypeConstraint::Equal(Type::Bool, cond_ty, expr.span));

                // OPT19: `if x? as v` binds the payload in the then-branch.
                // The test itself narrows nothing — there is no flow typing.
                let presence_binding = self.extract_presence_binding(cond);

                if let Some((ref name, ref payload_ty, _)) = presence_binding {
                    self.push_scope();
                    self.define_local_bound(
                        name.clone(),
                        payload_ty.clone(),
                        super::BoundFrom::Payload,
                    );
                }
                let then_ty = self.infer_expr(then_branch);
                if presence_binding.is_some() {
                    self.pop_scope();
                }

                // ER22: `else as e` needs a scrutinee with a branch to bind.
                if let Some(name) = else_binding {
                    let has_err = matches!(presence_binding, Some((_, _, Some(_))));
                    if !has_err {
                        self.errors.push(TypeError::ElseBindingNotResult {
                            name: name.clone(),
                            span: expr.span,
                        });
                    }
                }

                if let Some(else_branch) = else_branch {
                    // ER22: `else as e` binds the complement. A binding, not a
                    // narrow — the scrutinee's own type never changes.
                    let else_narrow = match (else_binding, &presence_binding) {
                        (Some(e_name), Some((_, _, Some(err_ty)))) => {
                            Some((e_name.clone(), err_ty.clone()))
                        }
                        _ => None,
                    };
                    if let Some((ref name, ref err_ty)) = else_narrow {
                        self.push_scope();
                        self.define_local(name.clone(), err_ty.clone());
                    }
                    let else_ty = self.infer_expr(else_branch);
                    if else_narrow.is_some() {
                        self.pop_scope();
                    }
                    let resolved_then = self.ctx.apply(&then_ty);
                    let resolved_else = self.ctx.apply(&else_ty);
                    // Never coerces to any type (CF32) — don't constrain
                    if matches!(resolved_else, Type::Never) {
                        then_ty
                    } else if matches!(resolved_then, Type::Never) {
                        else_ty
                    } else if is_stmt {
                        // Statement position: value is discarded, branches
                        // don't need to agree. Return unit.
                        Type::Unit
                    } else {
                        self.ctx.add_constraint(TypeConstraint::Equal(
                            then_ty.clone(),
                            else_ty,
                            expr.span,
                        ));
                        then_ty
                    }
                } else {
                    Type::Unit
                }
            }

            ExprKind::IfLet {
                pattern,
                then_branch,
                else_branch,
                expr: value,
                else_binding,
            } => {
                let value_ty = self.infer_expr(value);
                self.push_scope();
                let bindings = self.check_pattern(pattern, &value_ty, expr.span);
                for (name, ty) in bindings {
                    if !name.is_empty() {
                        self.define_local_bound(name, ty, super::BoundFrom::Payload);
                    }
                }
                let then_ty = self.infer_expr(then_branch);
                self.pop_scope();
                if let Some(else_branch) = else_branch {
                    // ER22: `else as e` binds the branch the test ruled out —
                    // the complement of what the pattern named.
                    let complement = else_binding
                        .as_ref()
                        .and_then(|name| self.complement_branch(pattern, &value_ty).map(|t| (name.clone(), t)));
                    if let Some((name, ty)) = complement {
                        self.push_scope();
                        self.define_local_bound(name, ty, super::BoundFrom::Payload);
                    } else if let Some(name) = else_binding {
                        self.errors.push(TypeError::ElseBindingNotResult {
                            name: name.clone(),
                            span: expr.span,
                        });
                    }
                    let else_ty = self.infer_expr(else_branch);
                    if else_binding.is_some() {
                        self.pop_scope();
                    }
                    self.ctx.add_constraint(TypeConstraint::Equal(
                        then_ty.clone(),
                        else_ty,
                        expr.span,
                    ));
                }
                then_ty
            }

            ExprKind::GuardPattern {
                expr: value,
                pattern,
                else_branch,
            } => {
                let value_ty = self.infer_expr(value);

                // Check that else branch diverges (returns Never)
                let else_ty = self.infer_expr(else_branch);
                let resolved_else = self.ctx.apply(&else_ty);
                if !matches!(resolved_else, Type::Never) {
                    self.errors.push(TypeError::GuardElseMustDiverge {
                        found: resolved_else,
                        span: else_branch.span,
                    });
                }

                // Check pattern and extract bindings
                // Note: Bindings are NOT added to scope here - they're added by the stmt handler
                // We just return them via the expression type mechanism
                let bindings = self.check_pattern(pattern, &value_ty, expr.span);

                // For a guard pattern like `const v = opt is Some else { return }`,
                // the expression itself evaluates to the inner type
                // The pattern binding happens at the statement level
                if let Some((_, inner_ty)) = bindings.first() {
                    inner_ty.clone()
                } else {
                    // If no explicit bindings, extract inner type from Option/Result
                    // This handles patterns like `Some` or `Ok` without explicit field binding
                    let resolved_value_ty = self.ctx.apply(&value_ty);
                    match resolved_value_ty.as_option() {
                        Some(inner) => inner.clone(),
                        None => match &resolved_value_ty {
                            Type::Result { ok, .. } => *ok.clone(),
                            _ => Type::Unit,
                        },
                    }
                }
            }

            ExprKind::IsPattern { expr: value, pattern } => {
                let value_ty = self.infer_expr(value);
                // #256: a binding from an `is` test reaches the rest of the
                // condition and the branch body — `m is Msg.Text(t) && t.len() > 1`.
                // The resolver already puts the name in the enclosing scope, and
                // the checker was throwing the *type* away, so `t` resolved to a
                // name with nothing behind it. The program still compiled, because
                // MIR guessed the receiver's type from the variable's tracked
                // prefix — the last of the nine dispatch fallbacks, and this was
                // the gap holding it up (#425).
                let bindings = self.check_pattern(pattern, &value_ty, expr.span);
                for (name, ty) in bindings {
                    self.define_local_bound(name, ty, super::BoundFrom::Payload);
                }
                Type::Bool
            }

            ExprKind::Match { scrutinee, arms } => {
                let is_stmt = self.in_stmt_expr;
                self.in_stmt_expr = false;

                let scrutinee_ty = self.infer_expr(scrutinee);
                // OPT NO_MATCH: reject `match x?` on an Option — migration error.
                let resolved_sc = self.ctx.apply(&scrutinee_ty);
                if resolved_sc.is_option() {
                    self.errors.push(TypeError::MatchOnOption { span: expr.span });
                }
                let result_ty = self.ctx.fresh_var();
                for arm in arms {
                    self.push_scope();
                    let bindings = self.check_pattern(&arm.pattern, &scrutinee_ty, expr.span);
                    for (name, ty) in bindings {
                        self.define_local_bound(name, ty, super::BoundFrom::Payload);
                    }
                    if let Some(guard) = &arm.guard {
                        let guard_ty = self.infer_expr(guard);
                        self.ctx.add_constraint(TypeConstraint::Equal(
                            Type::Bool,
                            guard_ty,
                            expr.span,
                        ));
                    }
                    let arm_ty = self.infer_expr(&arm.body);
                    self.pop_scope();
                    let resolved_arm_ty = self.ctx.apply(&arm_ty);
                    // In statement position, arm types don't need to agree.
                    if !is_stmt && !matches!(resolved_arm_ty, Type::Never) {
                        self.ctx.add_constraint(TypeConstraint::Equal(
                            result_ty.clone(),
                            arm_ty,
                            expr.span,
                        ));
                    }
                }

                // Exhaustiveness check for enum scrutinees
                self.check_match_exhaustiveness(&scrutinee_ty, arms, expr.span);

                if is_stmt { Type::Unit } else { result_ty }
            }

            ExprKind::Block(stmts) => {
                self.push_scope();
                let result = self.check_block_body(stmts);
                self.pop_scope();
                result
            }

            ExprKind::StructLit { name, fields, spread } => {
                // A struct-lit name may carry explicit generic args:
                // `Ring<i64> { ... }`. Look up the base, remember the args.
                let base_name = name.split('<').next().unwrap_or(name);
                // Annotations are comptime data — attached with @name(...),
                // read through reflect, never constructed as runtime values.
                if self.annotation_types.contains(base_name) {
                    self.errors.push(TypeError::BadAnnotation {
                        name: base_name.to_string(),
                        problem: "annotations cannot be constructed as runtime values".to_string(),
                        fix: format!("attach it instead: @{}(...) — readers get the values through reflect", base_name),
                        why: "an annotation is metadata, not a value: nothing constructs one, so nothing at runtime can hold one either [type.annotations/AN3, AN8]",
                        span: expr.span,
                    });
                    return Type::Error;
                }
                let explicit_args: Option<Vec<GenericArg>> = if name.contains('<') {
                    match parse_type_string(name, &self.types) {
                        Ok(Type::Generic { args, .. }) => Some(args),
                        _ => None,
                    }
                } else {
                    None
                };
                if let Some(ty) = self.types.lookup(base_name) {
                    if let Type::Named(type_id) = &ty {
                        let (struct_fields, type_params, private_fields) = match self.types.get(*type_id) {
                            Some(TypeDef::Struct { fields: sf, type_params: tp, private_fields: pf, .. }) => {
                                (sf.clone(), tp.clone(), pf.clone())
                            }
                            _ => (vec![], vec![], vec![]),
                        };

                        // V5: check private fields in struct literal construction
                        let is_self_type = self.current_self_type.as_ref()
                            .is_some_and(|st| matches!(st, Type::Named(id) if id == type_id));
                        if !is_self_type {
                            for field_init in fields.iter() {
                                if private_fields.contains(&field_init.name) {
                                    self.errors.push(TypeError::PrivateFieldAccess {
                                        ty: name.clone(),
                                        field: field_init.name.clone(),
                                        span: field_init.value.span,
                                    });
                                }
                            }
                        }

                        // FD4: every declared field must be provided. Desugar already
                        // filled in fields with declared defaults, so anything still
                        // missing is a defaultless field. A spread (`..base`) supplies
                        // all unlisted fields, so it satisfies the rest.
                        if spread.is_none() {
                            let missing: Vec<String> = struct_fields.iter()
                                .filter(|(n, _)| !fields.iter().any(|fi| &fi.name == n))
                                .map(|(n, _)| n.clone())
                                .collect();
                            if !missing.is_empty() {
                                self.errors.push(TypeError::MissingFields {
                                    ty: base_name.to_string(),
                                    fields: missing,
                                    span: expr.span,
                                });
                            }
                        }

                        if type_params.is_empty() {
                            // Non-generic struct: constrain directly
                            for field_init in fields {
                                let expected_field = struct_fields.iter()
                                    .find(|(n, _)| n == &field_init.name)
                                    .map(|(_, t)| t.clone());
                                let field_ty = if let Some(ref exp) = expected_field {
                                    self.infer_expr_expecting(&field_init.value, exp)
                                } else {
                                    self.infer_expr(&field_init.value)
                                };
                                if let Some(expected) = expected_field {
                                    // OPT6: optional fields widen bare values at
                                    // initialization. Bind position keeps non-optional
                                    // sums strict (ER11).
                                    self.coerce_into(
                                        CoercionSite::StructField,
                                        field_ty,
                                        expected,
                                        field_init.value.span,
                                    );
                                }
                            }
                            ty
                        } else {
                            // Generic struct: use explicit args if written
                            // (`Ring<i64> { }`), else fresh inference vars.
                            let fresh_args: Vec<GenericArg> = match &explicit_args {
                                Some(args) if args.len() == type_params.len() => args.clone(),
                                _ => type_params.iter()
                                    .map(|_| GenericArg::Type(Box::new(self.ctx.fresh_var())))
                                    .collect(),
                            };
                            let subst = Self::build_type_param_subst(&type_params, &fresh_args);

                            for field_init in fields {
                                let substituted = struct_fields.iter()
                                    .find(|(n, _)| n == &field_init.name)
                                    .map(|(_, t)| Self::substitute_type_params(t, &subst));
                                let field_ty = if let Some(ref sub) = substituted {
                                    self.infer_expr_expecting(&field_init.value, sub)
                                } else {
                                    self.infer_expr(&field_init.value)
                                };
                                if let Some(sub) = substituted {
                                    self.coerce_into(
                                        CoercionSite::StructField,
                                        field_ty,
                                        sub,
                                        field_init.value.span,
                                    );
                                }
                            }

                            Type::Generic { base: *type_id, args: fresh_args }
                        }
                    } else {
                        ty
                    }
                } else if let Some((enum_name, variant_name)) = base_name.split_once('.') {
                    // Struct-style enum variant literal: `Shape.Circle { radius: 5.0 }`.
                    // The value's type is the enum, not the variant — so methods
                    // declared via `extend Enum` resolve. Variant field names aren't
                    // stored in the type table (variants carry positional types), so
                    // constrain each field value by declaration order.
                    if let Some(type_id) = self.types.get_type_id(enum_name) {
                        let variant_arity = match self.types.get(type_id) {
                            Some(TypeDef::Enum { variants, .. }) => variants.iter()
                                .find(|(v, _)| v == variant_name)
                                .map(|(_, tys)| tys.len()),
                            _ => None,
                        };
                        if let Some(arity) = variant_arity {
                            // Variant field names aren't stored, so field values can't
                            // be matched to declared types by name. Infer them (catches
                            // errors inside each value) and check arity only.
                            for field_init in fields.iter() {
                                self.infer_expr(&field_init.value);
                            }
                            if fields.len() != arity {
                                self.errors.push(TypeError::ArityMismatch {
                                    expected: arity,
                                    found: fields.len(),
                                    span: expr.span,
                                });
                            }
                            Type::Named(type_id)
                        } else {
                            Type::UnresolvedNamed(name.clone())
                        }
                    } else {
                        Type::UnresolvedNamed(name.clone())
                    }
                } else {
                    Type::UnresolvedNamed(name.clone())
                }
            }

            ExprKind::Array(elements) => {
                if elements.is_empty() {
                    let elem_ty = self.ctx.fresh_var();
                    Type::Array {
                        elem: Box::new(elem_ty),
                        len: 0,
                    }
                } else {
                    let elem_types: Vec<Type> =
                        elements.iter().map(|e| self.infer_expr(e)).collect();
                    // CV1a: the element type is the one every element fits, not
                    // whichever element came first. `[small_u8, big_u64]` took
                    // `u8` and silently narrowed the second — the interpreter
                    // read 300 back and native read 44 (#649).
                    let first_ty = self
                        .widest_integer(&elem_types)
                        .unwrap_or_else(|| elem_types[0].clone());
                    for elem_ty in elem_types.into_iter().skip(1) {
                        self.ctx.add_constraint(TypeConstraint::Equal(
                            first_ty.clone(),
                            elem_ty,
                            expr.span,
                        ));
                    }
                    Type::Array {
                        elem: Box::new(first_ty),
                        len: elements.len(),
                    }
                }
            }

            ExprKind::Tuple(elements) => {
                let elem_types: Vec<_> = elements.iter().map(|e| self.infer_expr(e)).collect();
                // Empty tuple () is Unit type
                if elem_types.is_empty() {
                    Type::Unit
                } else {
                    Type::Tuple(elem_types)
                }
            }

            ExprKind::Range { start, end, .. } => {
                // The bounds share one type, and it's the type the loop variable
                // takes. A bare `Range` threw that away, so `for i in 1..6` left
                // `i` a free variable — and anything it fed too:
                // `mut v = Vec.new()` filled by `v.push(i)` had no element type,
                // so `v[0]` and everything downstream of it came out open (#620).
                let elem = self.ctx.fresh_var();
                for bound in [start, end].into_iter().flatten() {
                    let bound_ty = self.infer_expr(bound);
                    if let Err(e) = self.unify(&bound_ty, &elem, bound.span) {
                        self.errors.push(e);
                    }
                }
                Type::UnresolvedGeneric {
                    name: "Range".to_string(),
                    args: vec![GenericArg::Type(Box::new(elem))],
                }
            }

            ExprKind::Try { expr: inner } => {
                // ER18: `try { … } catch e => …`. `catch` binds tighter than the
                // `try` prefix, so the parse is `Try(Catch(Block))` — the arm
                // below in `Catch` looks for the other nesting and never fired,
                // which is how the handler ended up covering nothing (#950).
                if let ExprKind::Catch { value: caught, clause } = &inner.kind {
                    if matches!(caught.kind, ExprKind::Block(_)) {
                        return self.check_try_block_with_handler(
                            caught, clause, expr.span,
                        );
                    }
                }
                // ER17: `try { … }` block form. Inner `try`s propagate on their
                // own; the block's value is this expression's.
                if matches!(&inner.kind, ExprKind::Block(_)) {
                    return self.infer_expr(inner);
                }
                // ER16a: `try` attaches to the fallible step of a postfix chain,
                // not to the whole of it — `try read_file(p).len()` is
                // `(try read_file(p)).len()`. Mark every step below the outermost
                // so the first one that comes back wrapped hands the rest of the
                // chain its payload instead of the wrapper.
                self.mark_try_chain_steps(inner);
                let inner_ty = self.infer_expr(inner);
                self.try_chain_steps.clear();
                if let Some((step_id, err_ty)) = self.try_chain_unwrapped.take() {
                    // The chain already left through that step; this `try` has
                    // nothing further to peel.
                    self.record_try_placement(expr.id, step_id, &err_ty, expr.span);
                    return inner_ty;
                }
                let resolved = self.ctx.apply(&inner_ty);
                match &resolved {
                    // ER16 on an optional: the `none` leaves to the caller.
                    Type::Result { ok, err } if **err == Type::None => {
                        self.check_absence_can_leave(expr.span);
                        *ok.clone()
                    }
                    Type::Result { ok, err } => {
                        // ER47: a flat `T? or E` has two branches that could
                        // leave. Only the `try … ??` composite says which.
                        if ok.is_option() && !self.flat_try_sites.contains(&expr.id) {
                            self.errors.push(TypeError::TryOnFlatShape {
                                found: resolved.clone(),
                                span: expr.span,
                            });
                            return Type::Error;
                        }
                        // ER18: inside `try { … } catch e => …` the handler
                        // covers the block, so this error goes to it and not to
                        // the enclosing function. It used to be matched against
                        // the function's error type — `try { try s.parse<f64>() }
                        // catch _e =>` in a `f64 or LowError` function propagated
                        // a `ParseError` past its own handler — and the mismatch
                        // was swallowed because a stdlib name deferred instead of
                        // resolving (#950). The block can leave through the
                        // handler whether or not the function has an error
                        // branch, so `error_can_leave` doesn't apply either.
                        if let Some(target) = self.try_block_errors.last().cloned() {
                            self.propagate_try_error(expr.id, err, &target, expr.span);
                            return *ok.clone();
                        }
                        if !self.error_can_leave(expr.span) {
                            return Type::Error;
                        }
                        if let Some(return_ty) = &self.current_return_type {
                            let resolved_ret = self.ctx.apply(return_ty);
                            if self.accumulate_errors {
                                self.inferred_errors.push(*err.clone());
                            } else if let Type::Result { err: ret_err, .. } = &resolved_ret {
                                let (err, ret_err) = (err.clone(), ret_err.clone());
                                self.propagate_try_error(expr.id, &err, &ret_err, expr.span);
                            } else if matches!(resolved_ret, Type::Var(_)) {
                                // GC7: the return type is still inferred — `try`
                                // says it has an error branch.
                                let ret_ok = self.ctx.fresh_var();
                                let ret_result = Type::Result {
                                    ok: Box::new(ret_ok),
                                    err: err.clone(),
                                };
                                let _ = self.unify(&resolved_ret, &ret_result, expr.span);
                            }
                        }
                        *ok.clone()
                    }
                    Type::Var(_) => {
                        // ER18 again, for an operand whose own type hasn't
                        // settled yet — `try s.parse<f64>()` inside a `try { … }
                        // catch`. The block's handler is the target, the same as
                        // in the resolved case above.
                        if let Some(target) = self.try_block_errors.last().cloned() {
                            let ok_ty = self.ctx.fresh_var();
                            let err_ty = self.ctx.fresh_var();
                            let result_ty = Type::Result {
                                ok: Box::new(ok_ty.clone()),
                                err: Box::new(err_ty.clone()),
                            };
                            let _ = self.unify(&inner_ty, &result_ty, expr.span);
                            self.propagate_try_error(expr.id, &err_ty, &target, expr.span);
                            return ok_ty;
                        }
                        if let Some(return_ty) = &self.current_return_type {
                            let resolved_ret = self.ctx.apply(return_ty);
                            match &resolved_ret {
                                _ if resolved_ret.is_option() => {
                                    let inner_opt_ty = self.ctx.fresh_var();
                                    let option_ty = Type::option(inner_opt_ty.clone());
                                    let _ = self.unify(&inner_ty, &option_ty, expr.span);
                                    inner_opt_ty
                                }
                                Type::Result { err: ret_err, .. } => {
                                    let ok_ty = self.ctx.fresh_var();
                                    let err_ty = self.ctx.fresh_var();
                                    let result_ty = Type::Result {
                                        ok: Box::new(ok_ty.clone()),
                                        err: Box::new(err_ty.clone()),
                                    };
                                    let _ = self.unify(&inner_ty, &result_ty, expr.span);
                                    if self.accumulate_errors {
                                        self.inferred_errors.push(err_ty);
                                    } else {
                                        let ret_err = ret_err.clone();
                                        self.propagate_try_error(expr.id, &err_ty, &ret_err, expr.span);
                                    }
                                    ok_ty
                                }
                                Type::Var(_) => {
                                    // GC7: neither side is pinned yet. `try`
                                    // implies a two-branch operand and return.
                                    let ok_ty = self.ctx.fresh_var();
                                    let err_ty = self.ctx.fresh_var();
                                    let inner_result = Type::Result {
                                        ok: Box::new(ok_ty.clone()),
                                        err: Box::new(err_ty.clone()),
                                    };
                                    let _ = self.unify(&inner_ty, &inner_result, expr.span);
                                    if self.accumulate_errors {
                                        self.inferred_errors.push(err_ty.clone());
                                    }
                                    let ret_ok = self.ctx.fresh_var();
                                    let ret_result = Type::Result {
                                        ok: Box::new(ret_ok),
                                        err: Box::new(err_ty),
                                    };
                                    let _ = self.unify(&resolved_ret, &ret_result, expr.span);
                                    ok_ty
                                }
                                _ => {
                                    self.errors.push(TypeError::TryInNonPropagatingContext {
                                        return_ty: resolved_ret.clone(),
                                        span: expr.span,
                                    });
                                    Type::Error
                                }
                            }
                        } else {
                            self.errors.push(TypeError::TryOutsideFunction { span: expr.span });
                            Type::Error
                        }
                    }
                    // The operand's own error was already reported — saying
                    // `try` needs a Result and found `<error>` on top of it
                    // just buries the real one.
                    Type::Error => Type::Error,
                    _ => {
                        self.errors.push(TypeError::TryOnNonResult {
                            found: resolved,
                            span: expr.span,
                        });
                        Type::Error
                    }
                }
            }

            // ER14: `r catch e => body` / `r catch _ => body`. Results only.
            ExprKind::Catch { value, ref clause } => {
                // ER18: `try { … } catch e => …`. The handler covers the block,
                // so what it binds is the enclosing function's error type — the
                // block itself has an ordinary value type.
                let is_try_block = match &value.kind {
                    ExprKind::Try { expr: inner } => matches!(inner.kind, ExprKind::Block(_)),
                    _ => false,
                };
                if is_try_block {
                    return self.check_try_block_with_handler(value, clause, expr.span);
                }

                let val_ty = self.infer_expr(value);
                let resolved = self.ctx.apply(&val_ty);
                let (ok_ty, err_ty) = match &resolved {
                    Type::Result { err, .. } if **err == Type::None => {
                        self.errors.push(TypeError::CatchOnOptional {
                            found: resolved.clone(),
                            span: expr.span,
                        });
                        return Type::Error;
                    }
                    Type::Result { ok, err } => (*ok.clone(), *err.clone()),
                    Type::Var(_) => {
                        let ok = self.ctx.fresh_var();
                        let err = self.ctx.fresh_var();
                        let shape = Type::Result {
                            ok: Box::new(ok.clone()),
                            err: Box::new(err.clone()),
                        };
                        let _ = self.unify(&val_ty, &shape, expr.span);
                        (ok, err)
                    }
                    Type::Error => return Type::Error,
                    _ => {
                        self.errors.push(TypeError::TryOnNonResult {
                            found: resolved.clone(),
                            span: expr.span,
                        });
                        return Type::Error;
                    }
                };

                self.push_scope();
                if !clause.is_discard() {
                    self.define_local_bound(
                        clause.binder.clone(),
                        err_ty,
                        super::BoundFrom::Payload,
                    );
                }
                let body_ty = self.infer_expr(&clause.body);
                self.pop_scope();
                let resolved_body = self.ctx.apply(&body_ty);

                // ER14a, three cases in order: a still-wrapped body with the
                // same success type keeps the shape; `Never` and a bare `T`
                // both collapse to `T`.
                if matches!(resolved_body, Type::Never) {
                    return ok_ty;
                }
                // `catch _ => none` is the acknowledged drop — the old `.ok()`.
                // The layers don't stack: on a success type that's already
                // optional, the drop lands on the outer one (OPT30).
                if matches!(clause.body.kind, ExprKind::None) {
                    let ok_ty = self.ctx.apply(&ok_ty);
                    // On a flat `T? or E` the success side is already the `T?`
                    // the whole expression produces, so it passes straight
                    // through — marking it "keeps shape" would make both
                    // backends re-wrap and hand back a `T??` (#634).
                    //
                    // Either way the `none` is that same optional. Its own node
                    // was typed before we knew which, from nothing but the word
                    // `none`, so it kept an empty payload variable unless it's
                    // tied to the answer here.
                    if ok_ty.is_option() {
                        let _ = self.unify(&body_ty, &ok_ty, clause.body.span);
                        return ok_ty;
                    }
                    self.fallback_keeps_shape.insert(expr.id);
                    let result = Type::option(ok_ty);
                    let _ = self.unify(&body_ty, &result, clause.body.span);
                    return result;
                }
                // Still wrapped with the same success type — the shape carries
                // on, and so does the chain.
                if let Type::Result { ok: body_ok, .. } = &resolved_body {
                    let body_ok = (**body_ok).clone();
                    if let Err(e) = self.unify(&body_ok, &ok_ty, expr.span) {
                        self.errors.push(e);
                    }
                    self.fallback_keeps_shape.insert(expr.id);
                    return resolved_body;
                }
                // A void body only fits a void success type, but here the
                // success type is still a guess (`resolved` was a `Type::Var`
                // above) — unifying eagerly would let this catch *decide* the
                // type instead of being checked against it. Whichever catch
                // ran first won that decision regardless of which one was
                // actually right, so a later catch or use of the same value
                // with a real type failed there instead (#876). Defer: settle
                // this once everything else that could pin the type has run.
                if matches!(resolved_body, Type::Unit)
                    && matches!(self.ctx.apply(&ok_ty), Type::Var(_))
                {
                    self.pending_catch_void_checks.push((ok_ty.clone(), expr.span));
                    return ok_ty;
                }
                self.ctx.add_constraint(TypeConstraint::Equal(
                    ok_ty.clone(),
                    body_ty,
                    expr.span,
                ));
                ok_ty
            }

            // OPT32: `take slot` moves the payload out and leaves `none`.
            ExprKind::Take { place } => {
                let place_ty = self.infer_expr(place);
                let resolved = self.ctx.apply(&place_ty);
                if let Some(name) = Self::place_root_name(place) {
                    if let Some(kind) = self.lookup_binding_kind(&name) {
                        if kind.is_read_only() {
                            self.errors.push(TypeError::TakeOnImmutablePlace {
                                name,
                                span: expr.span,
                            });
                        }
                    }
                }
                match &resolved {
                    Type::Result { err, .. } if **err == Type::None => resolved.clone(),
                    Type::Var(_) => {
                        // The place resolves later — `take conn.pending` waits
                        // on the field's own constraint. Unifying it with `T?`
                        // here *decided* the place was optional, so a place
                        // that turned out to be a plain `i64` reported
                        // "expected `_?`, found `i64`" from the field's
                        // constraint: the guess this line made, not the
                        // mistake. Ask again once the place has settled (#645).
                        let result = self.ctx.fresh_var();
                        self.ctx.add_constraint(TypeConstraint::TakePlace {
                            place: place_ty.clone(),
                            result: result.clone(),
                            span: expr.span,
                        });
                        result
                    }
                    Type::Error => Type::Error,
                    _ => {
                        self.errors.push(TypeError::TakeOnNonOptional {
                            found: resolved.clone(),
                            span: expr.span,
                        });
                        Type::Error
                    }
                }
            }

            // OPT9: postfix `?` is the presence test — a plain bool. It
            // narrows nothing; the payload comes from the `as v` bind.
            ExprKind::IsPresent { expr: inner, .. } => {
                let inner_ty = self.infer_expr(inner);
                let resolved = self.ctx.apply(&inner_ty);
                match &resolved {
                    // ER12: `?` marks absence. A result's other branch is an
                    // error, which is tested with `is` and handled with `catch`.
                    Type::Result { err, .. }
                        if **err != Type::None
                            && !matches!(**err, Type::Var(_) | Type::Error) =>
                    {
                        self.errors.push(TypeError::PresenceTestOnResult {
                            found: resolved.clone(),
                            span: expr.span,
                        });
                        Type::Error
                    }
                    Type::Result { .. } => Type::Bool,
                    Type::Var(_) => {
                        // Unresolved scrutinee — leave as bool, let later context constrain.
                        Type::Bool
                    }
                    // The operand's own error was already reported — saying
                    // `try` needs a Result and found `<error>` on top of it
                    // just buries the real one.
                    Type::Error => Type::Error,
                    _ => {
                        self.errors.push(TypeError::TryOnNonResult {
                            found: resolved,
                            span: expr.span,
                        });
                        Type::Error
                    }
                }
            }

            ExprKind::Unwrap { expr: inner, message: _ } => {
                let inner_ty = self.infer_expr(inner);
                let resolved = self.ctx.apply(&inner_ty);
                match &resolved {
                    Type::Result { ok, err: _ } => {
                        // Extract the ok type (works for T? and T or E)
                        *ok.clone()
                    }
                    Type::Var(_) => {
                        // Shape unknown here — `v.get(0)!` waits on the method's
                        // return type. Record the relationship rather than
                        // handing back a fresh variable with nothing tying it to
                        // the operand, which left `let got = v.get(0)!` with an
                        // open type no matter what the operand turned out to be.
                        let payload = self.ctx.fresh_var();
                        self.ctx.add_constraint(TypeConstraint::Unwrap {
                            value: inner_ty.clone(),
                            result: payload.clone(),
                            span: expr.span,
                        });
                        payload
                    }
                    _ => {
                        self.errors.push(TypeError::ForceUnwrapOnNonOptional {
                            found: resolved,
                            span: expr.span,
                        });
                        Type::Error
                    }
                }
            }

            ExprKind::Closure { params, ret_ty: declared_ret, body, .. } => {
                let param_types: Vec<_> = params
                    .iter()
                    .map(|p| {
                        p.ty.as_ref()
                            .and_then(|t| parse_type_string(t, &self.types).ok())
                            .unwrap_or_else(|| self.ctx.fresh_var())
                    })
                    .collect();

                // ESAD Phase 2: Check for aliasing violations in closure body
                self.check_closure_aliasing(params, body);

                // Bind each parameter to the same variable the closure's `Fn`
                // type carries, so a caller that pins it (`map` unifying the
                // param with the iterator's element type) also pins the body's
                // view of it. Without this the body saw an unrelated variable:
                // `views().iter().map(|r| r.id).collect()` could never work out
                // `r.id`, so the element type of the collected Vec stayed open
                // and the binding was reported as un-inferrable (#620).
                self.push_scope();
                for (p, ty) in params.iter().zip(&param_types) {
                    if p.is_mutate || p.is_take {
                        self.define_local(p.name.clone(), ty.clone());
                    } else {
                        self.define_local_param(p.name.clone(), ty.clone());
                    }
                }

                // Save enclosing return type — `return` inside a closure
                // returns from the closure, not the enclosing function
                let outer_return_type = self.current_return_type.take();
                let outer_try_blocks = std::mem::take(&mut self.try_block_errors);
                let outer_accumulate = self.accumulate_errors;
                let outer_inferred_errors = std::mem::take(&mut self.inferred_errors);
                self.accumulate_errors = false;
                let closure_return_type = self.ctx.fresh_var();
                self.current_return_type = Some(closure_return_type.clone());

                let inferred_ret = self.infer_expr(body);

                self.pop_scope();
                self.current_return_type = outer_return_type;
                self.try_block_errors = outer_try_blocks;
                self.accumulate_errors = outer_accumulate;
                self.inferred_errors = outer_inferred_errors;

                // Unify the closure body type with the return type from
                // return statements (if any)
                let _ = self.unify(&inferred_ret, &closure_return_type, expr.span);

                // Check declared return type if present
                let ret_ty = if let Some(declared) = declared_ret {
                    let expected_ret = parse_type_string(declared, &self.types)
                        .unwrap_or(Type::Error);
                    if let Err(err) = self.unify(&closure_return_type, &expected_ret, expr.span) {
                        self.errors.push(err);
                    }
                    expected_ret
                } else {
                    // A body that diverges returns nothing, so no constraint
                    // reaches the return variable: `spawn(|| { panic("boom") })`
                    // finished inference with it open, and every consumer
                    // downstream then invented a width for a value that never
                    // exists. Register `Never` as the answer of last resort — a
                    // `return` in the same body coerces later and wins.
                    if matches!(inferred_ret, Type::Never) && !body_returns_a_value(body) {
                        if let Type::Var(id) = self.ctx.apply(&closure_return_type) {
                            self.ctx.default_var_to(id, Type::Never);
                        }
                    }
                    closure_return_type
                };

                Type::Fn {
                    params: param_types,
                    ret: Box::new(ret_ty),
                }
            }

            ExprKind::Cast { expr: inner, ty } => {
                let inner_ty = self.infer_expr(inner);
                let target = parse_type_string(ty, &self.types).unwrap_or(Type::Error);

                // Validate trait satisfaction for `as any Trait` casts
                if let Type::TraitObject { ref trait_name } = target {
                    if !matches!(inner_ty, Type::Var(_) | Type::Error) {
                        if !crate::traits::implements_trait(&self.types, &inner_ty, trait_name) {
                            let ty_desc = match &inner_ty {
                                Type::Named(id) => self.types.type_name(*id),
                                other => format!("{}", other),
                            };
                            self.errors.push(TypeError::TraitNotSatisfied {
                                ty: ty_desc,
                                trait_name: trait_name.clone(),
                                context: super::TraitBoundContext::TraitObjectCast,
                                span: expr.span,
                            });
                        }
                    }
                } else if is_scalar_ty(&target) {
                    // CV1–CV4, CH5, BL3: `as` is lossless widening only. Defer the
                    // check until literal defaults resolve `inner_ty`.
                    self.pending_casts.push(PendingCast {
                        source: inner_ty,
                        target: target.clone(),
                        target_name: ty.clone(),
                        convert: None,
                        span: expr.span,
                    });
                } else if !matches!(target, Type::Error)
                    && !matches!(inner_ty, Type::Var(_) | Type::Error)
                    && inner_ty != target
                {
                    // A number or a trait object are the only two things `as`
                    // converts to. To anything else it reinterprets the bits,
                    // which is what `transmute` needs `unsafe` for.
                    //
                    // This branch used to not exist: a non-scalar target fell
                    // off the end of the `if` with no check at all and the
                    // expression was simply declared to have the target type,
                    // so `[1, 2, 3] as Vec<i64>` lowered to a stack array whose
                    // address was handed to `Vec_len` — and indexing it
                    // segfaulted from ordinary safe code (#862).
                    self.unsafe_ops.push((expr.span, super::UnsafeCategory::Transmute));
                    if !self.in_unsafe {
                        self.errors.push(TypeError::AsCastNotConvertible {
                            src_ty: inner_ty.clone(),
                            target_name: ty.clone(),
                            span: expr.span,
                        });
                    }
                }

                target
            }

            ExprKind::Convert { expr: inner, target, kind } => {
                let inner_ty = self.infer_expr(inner);
                let target_ty = parse_type_string(target, &self.types).unwrap_or(Type::Error);
                self.pending_casts.push(PendingCast {
                    source: inner_ty,
                    target: target_ty.clone(),
                    target_name: target.clone(),
                    convert: Some(*kind),
                    span: expr.span,
                });
                // CV7/CV10 yield `T?`. CV11/CV15/CV16 — and CV14 to an integer
                // target — yield `T or ConvertError`. Everything else yields a
                // bare `T`, which is the rule the spec states as "anything that
                // can fail yields a result".
                if kind.is_optional() {
                    Type::option(target_ty)
                } else if kind.yields_result(matches!(prim_of(&target_ty), Some(Prim::Int { .. }))) {
                    Type::Result {
                        ok: Box::new(target_ty),
                        err: Box::new(self.convert_error_type()),
                    }
                } else {
                    target_ty
                }
            }

            ExprKind::Loop { body, .. } => {
                // Loop-as-expression gets its type from break values — each
                // `break v` unifies into this.
                let result = self.ctx.fresh_var();
                self.loop_value_types.push(result.clone());
                for stmt in body {
                    self.check_stmt(stmt);
                }
                self.loop_value_types.pop();
                result
            }

            ExprKind::Unsafe { body } => {
                let was_unsafe = self.in_unsafe;
                self.in_unsafe = true;
                // Also #696: inferring the trailing expression twice built two
                // sets of type vars for the same call and handed back the
                // second, which nothing solved — `let v = unsafe p.as_ptr()`
                // came out "type is still open here". The shared helper walks
                // it once, same as every other block kind.
                let result = self.check_block_body(body);
                self.in_unsafe = was_unsafe;
                result
            }

            ExprKind::Comptime { body } => self.check_block_body(body),

            ExprKind::Spawn { body } => {
                // CC1: direct spawn must be lexically inside a `using Multitasking { }` block
                if self.multitasking_depth == 0 {
                    self.errors.push(TypeError::SpawnOutsideBlock { span: expr.span });
                }

                // Spawn blocks are like anonymous functions - they have their own return type
                let outer_return_type = self.current_return_type.take();
                let outer_try_blocks = std::mem::take(&mut self.try_block_errors);
                let outer_accumulate = self.accumulate_errors;
                let outer_inferred_errors = std::mem::take(&mut self.inferred_errors);
                self.accumulate_errors = false;
                let spawn_return_type = self.ctx.fresh_var();
                self.current_return_type = Some(spawn_return_type.clone());

                // Check all statements except the last (which we infer separately)
                let last_idx = body.len().saturating_sub(1);
                for (i, stmt) in body.iter().enumerate() {
                    if i < last_idx {
                        self.check_stmt(stmt);
                    }
                }

                // Infer the return type from the last statement (only process once)
                let inner_type = if let Some(last) = body.last() {
                    match &last.kind {
                        StmtKind::Expr(e) => self.infer_expr(e),
                        StmtKind::Return(_) => {
                            self.check_stmt(last);
                            Type::Never
                        }
                        _ => {
                            self.check_stmt(last);
                            Type::Unit
                        }
                    }
                } else {
                    Type::Unit
                };

                self.ctx.add_constraint(TypeConstraint::Equal(
                    spawn_return_type.clone(),
                    inner_type,
                    expr.span,
                ));

                self.current_return_type = outer_return_type;
                self.try_block_errors = outer_try_blocks;
                self.accumulate_errors = outer_accumulate;
                self.inferred_errors = outer_inferred_errors;

                Type::UnresolvedGeneric {
                    name: "ThreadHandle".to_string(),
                    args: vec![GenericArg::Type(Box::new(spawn_return_type))],
                }
            }

            ExprKind::UsingBlock { name, args, body } => {
                // Validate context name
                match name.as_str() {
                    "Multitasking" | "MultiTasking" | "multitasking"
                    | "ThreadPool" | "threadpool" => {}
                    _ => {
                        self.errors.push(TypeError::UnknownContext {
                            name: name.clone(),
                            span: expr.span,
                        });
                    }
                }
                for arg in args {
                    self.infer_expr(&arg.expr);
                }
                // CC1: track nesting depth so spawn() inside this block is allowed
                let is_multitasking = matches!(
                    name.as_str(),
                    "Multitasking" | "MultiTasking" | "multitasking"
                );
                if is_multitasking {
                    self.multitasking_depth += 1;
                }
                for stmt in body {
                    self.check_stmt(stmt);
                }
                if is_multitasking {
                    self.multitasking_depth -= 1;
                }
                // Check if the block ends with a diverging statement (return/break/continue)
                if let Some(last) = body.last() {
                    match &last.kind {
                        StmtKind::Return(_) | StmtKind::Break { .. } | StmtKind::Continue(_) => {
                            return Type::Never;
                        }
                        _ => {}
                    }
                }
                Type::Unit
            }

            ExprKind::WithAs { bindings, body } => {
                self.push_scope();
                // Guard element types by binding name, so the tail expression
                // can be checked against the right one below — a binding's
                // own name shadows anything outer, and there can be several.
                let mut guard_elem_types: std::collections::HashMap<String, Type> =
                    std::collections::HashMap::new();
                for binding in bindings {
                    let raw_ty = self.infer_expr(&binding.source);
                    // Deciding whether to unwrap needs the source's concrete type.
                    // A module-level const initialized with `Mutex.new(...)` is still
                    // an unsolved type var here (module consts are solved only at the
                    // end), so resolve pending constraints before inspecting it —
                    // otherwise the wrapper isn't recognized and `v` stays a `Mutex`.
                    self.solve_constraints();
                    let source_ty = self.ctx.apply(&raw_ty);
                    // `with box as v { ... }` binds `v` to the inner T, never the
                    // wrapper — conc.sync/MX1 for Mutex, mem.cell/CE4 for Cell.
                    // Access is held for the block's duration and dropped when
                    // `with` exits.
                    //
                    // Shared<T> is here too: bare `with shared as v` is rejected by
                    // R4 elsewhere, but if it slips through, the body should still
                    // see the payload rather than the wrapper.
                    //
                    // Pool is deliberately absent — a pool is reached by element
                    // (`with pool[h] as e`), so the source is already the payload.
                    let unwraps = |name: &str| matches!(name, "Mutex" | "Shared" | "Cell");
                    let inner_of = |args: &[GenericArg]| match args.first() {
                        Some(GenericArg::Type(inner)) => Some((**inner).clone()),
                        _ => None,
                    };
                    let elem_ty = match &source_ty {
                        Type::Generic { base, args } if !args.is_empty() => {
                            let base_name = self.types.type_name(*base);
                            unwraps(&base_name).then(|| inner_of(args)).flatten()
                        }
                        Type::UnresolvedGeneric { name, args } if !args.is_empty() => {
                            unwraps(name).then(|| inner_of(args)).flatten()
                        }
                        _ => None,
                    }
                    .unwrap_or_else(|| source_ty.clone());
                    // conc.sync/R4: bare `with shared as v` names no lock, and the
                    // two locks don't behave the same — a read binding permits
                    // other readers and never writes back, a write binding blocks
                    // them and does. Nothing enforced this: the interpreter got as
                    // far as a runtime error whose message contradicted itself
                    // ("expected Cell, Mutex, Shared … got Shared") and native
                    // compiled it and read the wrong bytes, printing 0 for a field
                    // that held 4 (#880).
                    let names_a_lock = matches!(
                        &binding.source.kind,
                        ExprKind::MethodCall { method, .. }
                            if matches!(method.as_str(), "read" | "write" | "staged")
                    );
                    // ST3a: `staged()` under `Local`. The strategy is in the
                    // type, so the compiler can decide it — and ctrl.panic/S7
                    // says a condition fixed at the declaration is a diagnostic,
                    // not a runtime message.
                    let stages = matches!(
                        &binding.source.kind,
                        ExprKind::MethodCall { method, .. } if method == "staged"
                    );
                    // `source_ty` is the type of the whole `box.staged()` call —
                    // the payload — so the strategy has to come off the receiver.
                    let staged_recv = match &binding.source.kind {
                        ExprKind::MethodCall { object, method, .. } if method == "staged" => {
                            let ty = self
                                .node_types
                                .get(&object.id)
                                .map(|t| self.resolve_named(&self.ctx.apply(t)));
                            ty.map(|t| (object.as_ref(), t))
                        }
                        _ => None,
                    };
                    if let Some((recv, recv_ty)) = staged_recv {
                        if Self::type_is_shared(&recv_ty, &self.types)
                            && self.shared_strategy_name(&recv_ty) == "Local"
                        {
                            self.errors.push(TypeError::StagedOnLocal {
                                name: Self::source_text_for(recv)
                                    .unwrap_or_else(|| "the box".to_string()),
                                span: binding.source.span,
                            });
                        }
                    }
                    // W9 (tool.warnings, W0907): two or more fields of the
                    // locked value written without staging. Checked here rather
                    // than in a syntactic pass because it must not fire under
                    // `Local` — there is nothing to tear and `staged()` is an
                    // error there (ST3a), so the suggestion would be one.
                    self.check_torn_lock_update(binding, body);

                    if !names_a_lock && Self::type_is_shared(&source_ty, &self.types) {
                        self.errors.push(TypeError::BareSharedWith {
                            name: Self::source_text_for(&binding.source)
                                .unwrap_or_else(|| "shared".to_string()),
                            binding: binding.name.clone(),
                            span: binding.source.span,
                        });
                    }
                    // Bindings are mutable, with one exception: conc.sync/R1 —
                    // a shared read lock permits concurrent readers, so its
                    // binding is read-only and never writes back. Mutation
                    // through it is rejected at the mutation site.
                    let is_read_lock = matches!(
                        &binding.source.kind,
                        ExprKind::MethodCall { object, method, .. }
                            if method == "read" && self.expr_is_shared(object)
                    );
                    guard_elem_types.insert(binding.name.clone(), elem_ty.clone());
                    if is_read_lock {
                        self.define_local_with_read(binding.name.clone(), elem_ty);
                    } else {
                        self.define_local(binding.name.clone(), elem_ty);
                    }
                }
                // A guard is access to the box's payload for this block only,
                // not a value (mem.boxes, "Why scoped access, not guards") —
                // the bare identifier can't be the block's own produced value
                // when the payload is a struct/enum. A field read or a method
                // call already produces an independent value, so only the
                // plain identifier is checked. Reads `e` without walking it,
                // so it stays outside the single walk below.
                if let Some(last) = body.last() {
                    if let StmtKind::Expr(e) = &last.kind {
                        if let ExprKind::Ident(name) = &e.kind {
                            if let Some(elem_ty) = guard_elem_types.get(name) {
                                if let Some(type_name) = self.guard_escape_type_name(elem_ty) {
                                    self.errors.push(TypeError::WithGuardEscapes {
                                        name: name.clone(),
                                        type_name,
                                        span: e.span,
                                    });
                                }
                            }
                        }
                    }
                }
                let result = self.check_block_body(body);
                self.pop_scope();
                result
            }

            ExprKind::BlockCall { body, .. } => {
                for stmt in body {
                    self.check_stmt(stmt);
                }
                Type::Unit
            }

            ExprKind::ArrayRepeat { value, count } => {
                let elem_ty = self.infer_expr(value);
                self.infer_expr(count);
                // Extract literal size when available, otherwise use 0 as placeholder
                let len = match &count.kind {
                    ExprKind::Int(n, _) => *n as usize,
                    _ => 0,
                };
                Type::Array {
                    elem: Box::new(elem_ty),
                    len,
                }
            }


            ExprKind::NullCoalesce { value, default } => {
                // ER16b: `try f() ?? v` is the composite for a flat `T? or E`.
                // Tell the `try` it's the left half, so ER47 lets it through.
                if matches!(value.kind, ExprKind::Try { .. }) {
                    self.flat_try_sites.insert(value.id);
                }
                // `m[k] ?? d` is the mistake worth its own advice, and the
                // syntax that identifies it is gone by the time the operand's
                // type settles (#645).
                if matches!(value.kind, ExprKind::Index { .. }) {
                    self.coalesce_index_operands.insert(expr.id);
                }
                let val_ty = self.infer_expr(value);
                let resolved_val = self.ctx.apply(&val_ty);

                // ER12: `?` marks absence. A result's other branch is an error,
                // and dropping it should say so.
                if let Type::Result { err, .. } = &resolved_val {
                    if **err != Type::None && !matches!(**err, Type::Var(_) | Type::Error) {
                        self.errors.push(TypeError::CoalesceOnResult {
                            found: resolved_val.clone(),
                            span: expr.span,
                        });
                        return Type::Error;
                    }
                }

                let def_ty = self.infer_expr(default);

                // ER12 from the other side: `??` supplies the value for a
                // miss, so an operand that can't miss leaves the fallback
                // dead. Reported here, while the type is already concrete,
                // rather than left to the solver — its unify against a
                // synthesized `T or _` blamed a shape the program never had.
                // Handing back the operand's own type also keeps the binding
                // typed, so the second error about the binding stops (#662).
                if !self.coalesce_operand_can_be_absent(&resolved_val) {
                    self.errors.push(TypeError::CoalesceOnNonOptional {
                        found: resolved_val.clone(),
                        from_index: self.coalesce_index_operands.contains(&expr.id),
                        value_span: value.span,
                        default_span: default.span,
                        span: expr.span,
                    });
                    return resolved_val;
                }

                // Which of ER14a's three cases applies turns on the right
                // side's shape, and a method-call return type often isn't
                // known yet. Hand the whole decision to the solver.
                let result = self.ctx.fresh_var();
                self.ctx.add_constraint(TypeConstraint::Coalesce {
                    node: expr.id,
                    value: val_ty,
                    default: def_ty,
                    result: result.clone(),
                    value_span: value.span,
                    default_span: default.span,
                    span: expr.span,
                });
                result
            }

            ExprKind::OptionalField { object, field } => {
                let inferred = self.infer_expr(object);
                let obj_ty = self.ctx.apply(&inferred);
                // ?. unwraps Option, accesses field, wraps in Option (flatten if already Option)
                let inner_ty = match obj_ty.as_option() {
                    Some(inner) => inner.clone(),
                    None => obj_ty.clone(),
                };
                let field_ty = self.ctx.fresh_var();
                self.ctx.add_constraint(TypeConstraint::HasField {
                    ty: inner_ty,
                    field: field.clone(),
                    expected: field_ty.clone(),
                    span: expr.span,
                    self_type: self.current_self_type.clone(),
                });
                // Flatten: a field that is already `T?` stays one layer deep
                // (OPT10). That can't be decided here — `field_ty` is the fresh
                // variable the `HasField` above will fill in, so asking "is it
                // an option?" now always answers no and the chain came out
                // `T??` (#938). Defer it: `OptionalChain` settles once the
                // field's type does.
                let result = self.ctx.fresh_var();
                self.ctx.add_constraint(TypeConstraint::OptionalChain {
                    field: field_ty,
                    result: result.clone(),
                    span: expr.span,
                });
                result
            }

            ExprKind::Select { arms, .. } => {
                if arms.is_empty() {
                    self.errors.push(TypeError::GenericError(
                        "select must have at least one arm".to_string(),
                        expr.span,
                    ));
                    return Type::Unit;
                }
                let mut result_ty: Option<Type> = None;
                for arm in arms {
                    // A receive arm binds its value for the body only, so each
                    // arm gets its own scope. Skipping the binding left `v` as a
                    // free variable, which spread to the whole select's type and
                    // came out as "couldn't work out the type of" the binding it
                    // fed (#620).
                    self.push_scope();
                    match &arm.kind {
                        rask_ast::expr::SelectArmKind::Recv { channel, binding } => {
                            let chan_ty = self.infer_expr(channel);
                            let elem = self
                                .channel_element_type(&chan_ty)
                                .unwrap_or_else(|| self.ctx.fresh_var());
                            self.define_local(binding.clone(), elem);
                        }
                        rask_ast::expr::SelectArmKind::Send { channel, value } => {
                            self.infer_expr(channel);
                            self.infer_expr(value);
                        }
                        rask_ast::expr::SelectArmKind::Default => {}
                    }
                    let body_ty = self.infer_expr(&arm.body);
                    self.pop_scope();
                    if let Some(ref prev) = result_ty {
                        let _ = self.unify(prev, &body_ty, arm.body.span);
                    } else {
                        result_ty = Some(body_ty);
                    }
                }
                result_ty.unwrap_or(Type::Unit)
            }

            ExprKind::Assert { condition, message } | ExprKind::Check { condition, message } => {
                let cond_ty = self.infer_expr(condition);
                self.ctx.add_constraint(TypeConstraint::Equal(
                    cond_ty,
                    Type::Bool,
                    condition.span,
                ));
                if let Some(msg) = message {
                    let msg_ty = self.infer_expr(msg);
                    self.ctx.add_constraint(TypeConstraint::Equal(
                        msg_ty,
                        Type::String,
                        msg.span,
                    ));
                }
                Type::Unit
            }
        };

        // ER16a: this is a step of a postfix chain under a `try`. If it came
        // back wrapped, this is the fallible step — the `try` attaches here and
        // the rest of the chain works on the payload. The wrappers carry no
        // methods, so at most one step in a chain can do this.
        let ty = self.unwrap_try_chain_step(expr, ty);

        self.node_types.insert(expr.id, ty.clone());
        self.note_node_origin(expr);
        ty
    }


    /// Remember where a node came from, for the open-node census. Off unless
    /// `RASK_TRACE_OPEN_NODES` is set — a span and a kind name per expression
    /// is not worth carrying otherwise.
    fn note_node_origin(&mut self, expr: &Expr) {
        if !crate::checker::resolved_types::tracing_open_nodes() {
            return;
        }
        let kind = rask_ast::expr::expr_kind_name(&expr.kind);
        self.node_origins.insert(expr.id, (expr.span, kind));
    }

    /// ER16a: mark every postfix step below `chain` as a candidate for the
    /// `try` to attach to. Receivers only — `try` does not slide into call
    /// arguments, so a fallible call in an argument list keeps its own `try`.
    fn mark_try_chain_steps(&mut self, chain: &Expr) {
        self.try_chain_steps.clear();
        self.try_chain_unwrapped = None;
        let mut node = chain;
        loop {
            let inner = match &node.kind {
                ExprKind::MethodCall { object, .. }
                | ExprKind::Field { object, .. }
                | ExprKind::Index { object, .. }
                | ExprKind::DynamicField { object, .. } => object,
                ExprKind::Call { func, .. } => func,
                _ => break,
            };
            self.try_chain_steps.insert(inner.id);
            node = inner;
        }
    }

    /// ER16a: strip the wrapper off the chain step the `try` attaches to.
    /// Only the first such step in a chain is taken.
    fn unwrap_try_chain_step(&mut self, expr: &Expr, ty: Type) -> Type {
        if self.try_chain_unwrapped.is_some() || !self.try_chain_steps.contains(&expr.id) {
            return ty;
        }
        let resolved = self.ctx.apply(&ty);
        let Type::Result { ok, err } = &resolved else { return ty };
        // A flat `T? or E` has two branches that could leave, so it needs the
        // `try … ??` composite and can't be resolved by placement alone (ER47).
        if ok.is_option() {
            return ty;
        }
        self.try_chain_unwrapped = Some((expr.id, (**err).clone()));
        (**ok).clone()
    }

    /// ER16a: record where a `try` landed and run the propagation bookkeeping
    /// for it — the same checks bare `try r` does, against the step's error.
    fn record_try_placement(&mut self, try_id: NodeId, step_id: NodeId, err: &Type, span: Span) {
        self.try_chain_placement.insert(try_id, step_id);
        if matches!(err, Type::None) {
            self.check_absence_can_leave(span);
            return;
        }
        if !self.error_can_leave(span) {
            return;
        }
        if self.accumulate_errors {
            self.inferred_errors.push(err.clone());
            return;
        }
        let Some(return_ty) = self.current_return_type.clone() else { return };
        let resolved_ret = self.ctx.apply(&return_ty);
        if let Type::Result { err: ret_err, .. } = &resolved_ret {
            let ret_err = ret_err.clone();
            self.propagate_try_error(try_id, err, &ret_err, span);
        } else if matches!(resolved_ret, Type::Var(_)) {
            // GC7: the return type is still open — `try` says it has an error
            // branch, and this is it.
            let ret_ok = self.ctx.fresh_var();
            let ret_result = Type::Result {
                ok: Box::new(ret_ok),
                err: Box::new(err.clone()),
            };
            let _ = self.unify(&resolved_ret, &ret_result, span);
        }
    }

    // ------------------------------------------------------------------------
    // Specific Type Checks
    // ------------------------------------------------------------------------

    /// What `object[index]` yields, given the container's type. `None` means the
    /// container's shape isn't known yet — the caller defers instead of guessing.
    ///
    /// Shared with the deferred `Index` constraint so both readings of an index
    /// agree; they used to be the same match written once, inline, with a fresh
    /// variable where this returns `None`.
    /// What `container[index]` yields, for the shapes it can be read off the
    /// container's type. `None` means "nothing to say" — an unresolved container,
    /// or a generic whose argument list doesn't carry the element.
    pub(super) fn index_result_type(&self, obj_ty: &Type, is_range: bool) -> Option<Type> {
        match obj_ty {
            Type::Array { elem, .. } | Type::Slice(elem) => Some(if is_range {
                Type::Slice(elem.clone())
            } else {
                *elem.clone()
            }),
            // `[]` on a string means bytes in both forms (std.strings/U1b):
            // a range slices, a scalar index reads one byte. It used to yield
            // a `char` at a *character* index, so the same bracket counted two
            // different units and `s[i]` in a loop scanned from byte zero.
            Type::String => Some(if is_range { Type::String } else { Type::U8 }),
            // Vec<T>, Pool<T>, Handle<T> → element from first type arg.
            // Map<K,V> indexed by K → value type from second arg.
            Type::Generic { args, .. } | Type::UnresolvedGeneric { args, .. } => {
                let is_map = match obj_ty {
                    Type::UnresolvedGeneric { name, .. } => name == "Map",
                    Type::Generic { base, .. } => self
                        .types
                        .get_type_id("Map")
                        .map_or(false, |id| id == *base),
                    _ => false,
                };
                let elem_arg = if is_map { args.get(1) } else { args.first() };
                match elem_arg {
                    Some(GenericArg::Type(elem)) => Some(if is_range {
                        Type::Slice(elem.clone())
                    } else {
                        *elem.clone()
                    }),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    pub(super) fn check_binary(&mut self, op: BinOp, left: &Expr, right: &Expr, span: Span) -> Type {
        let left_ty = self.infer_expr(left);
        let right_ty = self.infer_expr(right);

        self.ctx.add_constraint(TypeConstraint::Equal(
            left_ty.clone(),
            right_ty.clone(),
            span,
        ));

        match op {
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => Type::Bool,
            BinOp::And | BinOp::Or => {
                self.ctx
                    .add_constraint(TypeConstraint::Equal(Type::Bool, left_ty, span));
                Type::Bool
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => left_ty,
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::Shl | BinOp::Shr => left_ty,
        }
    }

    pub(super) fn check_call(&mut self, call_id: NodeId, func: &Expr, args: &[CallArg], span: Span) -> Type {
        if let ExprKind::Ident(name) = &func.kind {
            // OPT2/ER2: reject legacy `Some(x)`, `Ok(x)`, `Err(x)` constructors.
            // The new model auto-wraps bare values at return/assignment, and
            // error values use their own constructor (e.g., `DivError.ByZero`).
            if matches!(name.as_str(), "Some" | "Ok" | "Err") {
                self.errors.push(TypeError::LegacyWrapperConstructor {
                    name: name.clone(),
                    span,
                });
                for arg in args { self.infer_expr(&arg.expr); }
                return Type::Error;
            }

            // transmute(val) — reinterpret bits, requires unsafe
            if name == "transmute" {
                self.unsafe_ops.push((span, super::UnsafeCategory::Transmute));
                if !self.in_unsafe {
                    self.errors.push(TypeError::UnsafeRequired {
                        operation: "transmute".to_string(),
                        span,
                    });
                }
                if args.len() != 1 {
                    self.errors.push(TypeError::ArityMismatch {
                        expected: 1,
                        found: args.len(),
                        span,
                    });
                }
                for arg in args {
                    self.infer_expr(&arg.expr);
                }
                return self.ctx.fresh_var();
            }

            // mem.owned/OW3: `drop(p)` consumes one `Owned` and frees its value.
            // It takes whatever `own` produced — `Owned<T>` erases to `T` here
            // (OW5), so there is no type to check against; the arity is.
            if name == "drop" && args.len() != 1 {
                self.errors.push(TypeError::ArityMismatch {
                    expected: 1,
                    found: args.len(),
                    span,
                });
            }

            if self.is_builtin_function(name) {
                // std.fmt/D3/D4 on `print(x)` comes from the desugar pass, which
                // rewrites each argument to `x.to_string()` — the same shape
                // `{x}` becomes. Nothing to check here (#772).
                for arg in args {
                    self.infer_expr(&arg.expr);
                }
                return match name.as_str() {
                    "panic" | "todo" | "unreachable" | "skip" => Type::Never,
                    "format" => Type::String,
                    _ => Type::Unit,
                };
            }

            // Nominal type constructor: UserId(42)
            if let Some(type_id) = self.types.get_type_id(name) {
                if let Some(TypeDef::NominalAlias { underlying, .. }) = self.types.get(type_id) {
                    let underlying = underlying.clone();
                    if args.len() != 1 {
                        self.errors.push(TypeError::ArityMismatch {
                            expected: 1,
                            found: args.len(),
                            span,
                        });
                        for arg in args { self.infer_expr(&arg.expr); }
                        return Type::Error;
                    }
                    let arg_ty = self.infer_expr_expecting(&args[0].expr, &underlying);
                    self.ctx.add_constraint(TypeConstraint::Equal(
                        underlying,
                        arg_ty,
                        span,
                    ));
                    return Type::Named(type_id);
                }
            }
        }

        // Extern and unsafe function calls require unsafe context
        // Also: CC1 — spawn() must be inside a `using Multitasking { }` block
        // conc.sync/SH7 applies to any call named `spawn`, however it reached
        // scope — a builtin, or the `async.spawn` import. Judged after solving.
        if matches!(&func.kind, ExprKind::Ident(n) if n == "spawn" || n.ends_with(".spawn")) {
            self.spawn_arg_spans.extend(args.iter().map(|a| a.expr.span));
        }
        if let ExprKind::Ident(_) = &func.kind {
            if let Some(&sym_id) = self.resolved.resolutions.get(&func.id) {
                if let Some(sym) = self.resolved.symbols.get(sym_id) {
                    // CC1: spawn() outside any using Multitasking block
                    if matches!(&sym.kind, SymbolKind::BuiltinFunction { builtin }
                        if *builtin == rask_resolve::BuiltinFunctionKind::Spawn)
                    {
                        if self.multitasking_depth == 0 {
                            self.errors.push(TypeError::SpawnOutsideBlock { span });
                        }
                        // conc.sync/SH7: `Local` takes no lock, so a box using it
                        // must not reach a second task. This is the whole reason
                        // the default can be the cheap one — the unsafe direction
                        // doesn't compile.
                    }

                    let unsafe_category = match &sym.kind {
                        SymbolKind::ExternFunction { .. } => Some(super::UnsafeCategory::ExternCall),
                        SymbolKind::Function { is_unsafe: true, .. } => Some(super::UnsafeCategory::UnsafeFuncCall),
                        _ => None,
                    };
                    if let Some(category) = unsafe_category {
                        self.unsafe_ops.push((span, category));
                        if !self.in_unsafe {
                            let operation = match category {
                                super::UnsafeCategory::ExternCall => "extern function call",
                                _ => "unsafe function call",
                            };
                            self.errors.push(TypeError::UnsafeRequired {
                                operation: operation.to_string(),
                                span,
                            });
                        }
                    }
                }
            }
        }

        // CALL6: record the resolved free-function target once, keyed by the
        // call node. Downstream passes read this instead of re-deriving the
        // callee from its name.
        if let ExprKind::Ident(_) = &func.kind {
            if let Some(&sym_id) = self.resolved.resolutions.get(&func.id) {
                self.call_targets.insert(call_id, super::Callee::Free(sym_id));
            }
        }

        // Call-site annotations (mutate/own) are optional — IDE shows ghost
        // annotations but the compiler doesn't require them (spec decision).
        // Validate when present, but don't error on missing annotations.
        self.check_call_annotations(func, args, span);

        // For generic function calls, create fresh type vars for each type param
        // and build a substitution map (param name → fresh var). After getting
        // the function type, we apply this substitution so that UnresolvedNamed("T")
        // in the param/return types becomes the fresh var. Constraint solving then
        // links the fresh vars to concrete types from the call arguments.
        //
        // `make<i32>(2)` — the parser keeps the written type arguments in the
        // callee's name, and nothing had ever read them back out. Inference
        // covered for that wherever a *parameter* also mentioned the type
        // parameter, so it only showed when none did: a parameter appearing
        // solely in the return type stayed a free variable, however explicitly
        // the call had spelled it (#712).
        let written_type_args: Vec<Type> = match &func.kind {
            ExprKind::Ident(name) => Self::written_type_args(name),
            _ => Vec::new(),
        };

        let generic_subst: Option<Vec<(String, Type)>> = if let ExprKind::Ident(_) = &func.kind {
            // Resolve the callee's SymbolId, then look up its type params
            self.resolved.resolutions.get(&func.id).copied()
                .and_then(|sym_id| self.fn_type_params.get(&sym_id).cloned().map(|tp| (sym_id, tp)))
                .map(|(sym_id, type_params)| {
                    let bounds = self.fn_type_param_bounds.get(&sym_id).cloned();
                    let pairs: Vec<(String, Type)> = type_params.into_iter()
                        .enumerate()
                        .map(|(i, name)| {
                            let fresh = self.ctx.fresh_var();
                            // A written type argument pins the variable now.
                            // Inference still runs — a wrong one meets the
                            // argument types and reports an ordinary mismatch,
                            // which is the message that names both sides.
                            if let Some(written) = written_type_args.get(i) {
                                let resolved = self.resolve_named(written);
                                if !matches!(resolved, Type::Error) {
                                    let _ = self.unify(&fresh, &resolved, span);
                                }
                            }
                            // #314: obligate the type arg to satisfy its bounds.
                            if let Some(param_bounds) = bounds.as_ref().and_then(|b| b.get(&name)) {
                                self.pending_bound_checks.push((fresh.clone(), param_bounds.clone(), span));
                            }
                            (name, fresh)
                        })
                        .collect();
                    let fresh_vars: Vec<Type> = pairs.iter().map(|(_, v)| v.clone()).collect();
                    self.pending_call_type_args.push((call_id, fresh_vars));
                    pairs
                })
        } else {
            None
        };

        let func_ty = self.infer_expr(func);

        // Substitute type param names with fresh vars in the function signature
        let func_ty = if let Some(ref pairs) = generic_subst {
            let subst: std::collections::HashMap<&str, Type> = pairs.iter()
                .map(|(k, v)| (k.as_str(), v.clone()))
                .collect();
            // ER3a: the callee's `T or E` nodes are disjointness obligations on
            // this call's type args. Read them off before substituting, while
            // the params are still spelled by name.
            let callee_name = match &func.kind {
                ExprKind::Ident(name) => name.clone(),
                _ => String::from("this function"),
            };
            self.note_disjointness_obligations(&callee_name, &func_ty, &subst, span);
            Self::substitute_type_params(&func_ty, &subst)
        } else {
            func_ty
        };

        match func_ty {
            Type::Fn { ref params, ref ret } => {
                if params.is_empty() && !args.is_empty() {
                    for arg in args { self.infer_expr(&arg.expr); }
                    return *ret.clone();
                }

                if params.len() != args.len() {
                    for arg in args { self.infer_expr(&arg.expr); }
                    self.errors.push(TypeError::ArityMismatch {
                        expected: params.len(),
                        found: args.len(),
                        span,
                    });
                    return Type::Error;
                }

                // Propagate expected param types to arguments
                let ret = *ret.clone();
                for (param, arg) in params.clone().iter().zip(args.iter()) {
                    // TR5: record implicit trait coercion for MIR boxing
                    if let Type::TraitObject { ref trait_name } = param {
                        let is_explicit_cast = matches!(
                            &arg.expr.kind,
                            ExprKind::Cast { ty, .. } if ty.starts_with("any ")
                        );
                        if !is_explicit_cast {
                            let arg_ty = self.infer_expr(&arg.expr);
                            if !matches!(arg_ty, Type::TraitObject { .. } | Type::Error) {
                                self.trait_coercions.insert(
                                    arg.expr.id,
                                    trait_name.clone(),
                                );
                            }
                        }
                    }
                    let arg_ty = self.infer_expr_expecting(&arg.expr, param);
                    // OPT6: optional parameters widen bare arguments. Bind
                    // position keeps non-optional sums strict (ER11).
                    self.coerce_into(CoercionSite::Argument, arg_ty, param.clone(), span);
                }

                ret
            }
            Type::Var(_) => {
                let arg_types: Vec<_> = args.iter().map(|a| self.infer_expr(&a.expr)).collect();
                let ret = self.ctx.fresh_var();
                self.ctx.add_constraint(TypeConstraint::Equal(
                    func_ty,
                    Type::Fn {
                        params: arg_types,
                        ret: Box::new(ret.clone()),
                    },
                    span,
                ));
                ret
            }
            Type::Error => {
                for arg in args { self.infer_expr(&arg.expr); }
                Type::Error
            }
            // A type name in call position. `TaskId(1)` looks like a
            // constructor but only nominal types have one (T7) — for a struct
            // or enum it used to slip through here and blow up much later, in
            // MIR lowering, as "method `next` on receiver of unresolved type".
            Type::Named(type_id) => {
                for arg in args { self.infer_expr(&arg.expr); }
                match self.types.get(type_id) {
                    Some(TypeDef::Struct { name, fields, .. }) => {
                        let (name, fields) = (
                            name.clone(),
                            fields.iter().map(|(f, _)| f.clone()).collect(),
                        );
                        self.errors.push(TypeError::TypeCalledAsFunction {
                            name,
                            kind: "a struct".to_string(),
                            fields,
                            span,
                        });
                        Type::Error
                    }
                    Some(TypeDef::Enum { name, .. }) => {
                        let name = name.clone();
                        self.errors.push(TypeError::TypeCalledAsFunction {
                            name,
                            kind: "an enum".to_string(),
                            fields: Vec::new(),
                            span,
                        });
                        Type::Error
                    }
                    _ => self.ctx.fresh_var(),
                }
            }
            _ => {
                for arg in args { self.infer_expr(&arg.expr); }
                self.ctx.fresh_var()
            }
        }
    }

    /// The argument as the reader wrote it, for PM4's message and its fix.
    ///
    /// Only the shapes a `mutate` argument can be — a name or a field path.
    /// Anything else has no place to write the value back to, so it can't reach
    /// a `mutate` parameter in the first place.
    fn argument_text(expr: &Expr) -> Option<String> {
        match &expr.kind {
            ExprKind::Ident(name) => Some(name.clone()),
            ExprKind::Field { object, field } => {
                Some(format!("{}.{}", Self::argument_text(object)?, field))
            }
            _ => None,
        }
    }

    pub(super) fn is_builtin_function(&self, name: &str) -> bool {
        matches!(name, "println" | "print" | "panic" | "todo" | "unreachable"
            | "assert" | "debug" | "format" | "fence" | "compiler_fence"
            | "assert_eq" | "skip" | "expect_fail" | "drop")
    }


    /// Validate that call-site annotations match parameter declarations.
    /// Check that call-site `mutate`/`take` annotations match the declaration.
    ///
    /// This reads the callee's parameter *symbols*, so it does nothing for a
    /// callee that has none. Stdlib stubs used to be exactly that — the resolver
    /// registered them with an empty parameter list — which meant these rules
    /// silently did not apply to any stdlib call. Stubs carry their real
    /// parameters and modes now, so the checks apply there like anywhere else.
    ///
    /// Nothing changed for existing code: the only `public func` stub taking a
    /// mode is `drop(take ptr: Heap)`, and `drop` returns before reaching here.
    /// A stub added later with a `mutate` parameter will start enforcing at its
    /// call sites, which is the intent — noted because it turned on as a side
    /// effect of fixing the parameter list, not as a change written here.
    fn check_call_annotations(&mut self, func: &Expr, args: &[CallArg], _span: Span) {
        use rask_ast::expr::ArgMode;
        use rask_resolve::SymbolKind;

        // Get the function's symbol ID
        let sym_id = if let ExprKind::Ident(_) = &func.kind {
            self.resolved.resolutions.get(&func.id).copied()
        } else {
            None
        };

        let Some(sym_id) = sym_id else { return };
        let Some(sym) = self.resolved.symbols.get(sym_id) else { return };

        // Get parameter symbols
        let param_ids = match &sym.kind {
            SymbolKind::Function { params, .. } => params.clone(),
            _ => return,
        };

        let callee_name = match &func.kind {
            ExprKind::Ident(n) => n.clone(),
            _ => sym.name.clone(),
        };

        // Validate each argument annotation
        for (i, (arg, &param_id)) in args.iter().zip(param_ids.iter()).enumerate() {
            let Some(param_sym) = self.resolved.symbols.get(param_id) else { continue };
            let (is_take, is_mutate, is_deleting) = match &param_sym.kind {
                SymbolKind::Parameter { is_take, is_mutate, is_deleting } => {
                    (*is_take, *is_mutate, *is_deleting)
                }
                _ => continue,
            };

            let param_name = &param_sym.name;

            // Deep const: passing a const binding to a `mutate` parameter is
            // rejected. `take` (ownership transfer) is still allowed — moving
            // a value is not mutation.
            //
            // A link argument is exempt from the *`let`* half of this, and only
            // that half. `mutate n: Link<T>` writes the node the link points at;
            // the link itself is a pointer that stays exactly as it was, so
            // demanding `mut` on a `let` binding asks permission to change the one
            // thing that isn't changing — and that demand is what would drag `mut`
            // into a link's type position, which the box family doesn't do.
            //
            // A read-only *parameter* is a different question and stays rejected:
            // there the caller never granted write access, so passing it on as
            // `mutate` would launder a view into a writer in one hop. That's the
            // guarantee that makes `n: Link<T>` a usable read-only view.
            let arg_is_link = param_sym
                .ty
                .as_deref()
                .is_some_and(|t| t.starts_with("Link<"));
            if is_mutate && !is_take {
                if let ExprKind::Ident(arg_name) = &arg.expr.kind {
                    match self.lookup_binding_kind(arg_name) {
                        Some(super::BindingKind::Let) if !arg_is_link => {
                            self.errors.push(TypeError::MutateConst {
                                name: arg_name.clone(),
                                span: arg.expr.span,
                            });
                        }
                        Some(super::BindingKind::WithRead) => {
                            self.errors.push(TypeError::MutateWithBinding {
                                name: arg_name.clone(),
                                span: arg.expr.span,
                            });
                        }
                        Some(super::BindingKind::Param) => {
                            self.errors.push(TypeError::MutateReadOnlyParam {
                                name: arg_name.clone(),
                                span: arg.expr.span,
                            });
                        }
                        Some(super::BindingKind::Bound(from)) => {
                            self.errors.push(TypeError::MutateBoundName {
                                name: arg_name.clone(),
                                from,
                                span: arg.expr.span,
                            });
                        }
                        _ => {}
                    }
                }
            }

            match (&arg.mode, is_take, is_mutate) {
                // PM4: an argument going into a `mutate` parameter is written
                // `mutate arg`. The checker backstops a misread *move* — using
                // a value after it's moved is an error — but nothing backstops
                // a misread *mutation*: both readings are legal code, so the
                // one the compiler can't catch is the one that gets marked.
                //
                // `own` on a `take` argument stays optional (PM4), because a
                // wrong reading there does get caught.
                (ArgMode::Default, false, true) => {
                    let arg_text = Self::argument_text(&arg.expr)
                        .unwrap_or_else(|| param_name.clone());
                    self.errors.push(TypeError::MissingMutateMarker {
                        callee: callee_name.clone(),
                        arg: arg_text,
                        param_name: param_name.clone(),
                        span: arg.expr.span,
                    });
                }
                (ArgMode::Default, true, _) => {}
                // Correct annotations are fine
                (ArgMode::Own, true, _) => {}
                (ArgMode::Mutate, _, true) if !is_deleting => {}
                (ArgMode::Deleting, _, _) if is_deleting => {}
                // PM5: the marker follows the signature. A `deleting` parameter is
                // a `mutate` parameter that may also delete nodes the caller
                // never named, so the call site says the more specific word —
                // otherwise two different contracts print identically.
                (ArgMode::Mutate, _, _) if is_deleting => {
                    let arg_text = Self::argument_text(&arg.expr)
                        .unwrap_or_else(|| param_name.clone());
                    self.errors.push(TypeError::MissingDeletingMarker {
                        callee: callee_name.clone(),
                        arg: arg_text,
                        param_name: param_name.clone(),
                        span: arg.expr.span,
                    });
                }
                (ArgMode::Deleting, _, _) => {
                    self.errors.push(TypeError::UnexpectedAnnotation {
                        annotation: "deleting".to_string(),
                        param_name: param_name.clone(),
                        param_index: i,
                        span: arg.expr.span,
                    });
                }
                // Wrong annotation type: `mutate` where `take` expected
                (ArgMode::Mutate, true, false) => {
                    self.errors.push(TypeError::UnexpectedAnnotation {
                        annotation: "mutate".to_string(),
                        param_name: param_name.clone(),
                        param_index: i,
                        span: arg.expr.span,
                    });
                }
                // Unexpected `own` annotation on borrow param
                (ArgMode::Own, false, _) => {
                    self.errors.push(TypeError::UnexpectedAnnotation {
                        annotation: "own".to_string(),
                        param_name: param_name.clone(),
                        param_index: i,
                        span: arg.expr.span,
                    });
                }
                // Unexpected `mutate` annotation on borrow param
                (ArgMode::Mutate, _, false) => {
                    self.errors.push(TypeError::UnexpectedAnnotation {
                        annotation: "mutate".to_string(),
                        param_name: param_name.clone(),
                        param_index: i,
                        span: arg.expr.span,
                    });
                }
                // All other cases are valid
                _ => {}
            }
        }
    }

    /// True when a variable of this name is in scope and holds an ordinary
    /// value. `import os` also defines a local — a `__module_os` marker that
    /// carries field access like `os.Command` — and that one is the namespace,
    /// not a shadow of it.
    pub(super) fn local_shadows_namespace(&self, name: &str) -> bool {
        match self.lookup_local(name) {
            Some(Type::UnresolvedNamed(n)) => !n.starts_with("__module_"),
            Some(_) => true,
            None => false,
        }
    }

    pub(super) fn check_method_call(
        &mut self,
        call_id: NodeId,
        object: &Expr,
        method: &str,
        args: &[CallArg],
        type_args: Option<&[String]>,
        span: Span,
    ) -> Type {
        // AN8: a `get<A>()` that reaches here wasn't field-projected — the
        // projection is handled in `check_field_access` and never recurses into
        // the receiver. So this is a bare read: a binding, an argument, a
        // returned value. There is no annotation value to be any of those.
        if method == "get" {
            if let Some(name) = type_args.and_then(|ta| ta.first()) {
                if self.annotation_types.contains(name) {
                    self.errors.push(TypeError::BadAnnotation {
                        name: name.clone(),
                        problem: "an annotation read has to name a field".to_string(),
                        fix: format!("read one field: `.get<{}>().<field>`", name),
                        why: "there is no annotation value to hold — `get` splices the field's constant, so a read that names no field has no result [type.annotations/AN6, AN8]",
                        span,
                    });
                    return Type::Error;
                }
            }
        }

        // What the call wrote between the angle brackets. `resolve_method` binds
        // the method's own type parameters to these instead of freshening them,
        // so `s.parse<i64>()` means i64 whether or not anything downstream would
        // have pinned it (#1029).
        //
        // Recorded before any dispatch, because several paths below file their
        // constraint and return. The type-namespace one is why `Vec.new<string>()`
        // lost its `<string>` (#1084).
        if let Some(ta) = type_args {
            let written: Vec<Type> = ta
                .iter()
                .map(|name| self.resolve_type_name(name, span))
                .collect();
            self.written_method_type_args.insert(call_id, written);
        }

        // Check if this is a builtin module method call (e.g., fs.open). A local
        // of the same name wins — `let fs = Vec.new()` is an ordinary variable,
        // and routing `fs.len()` to the filesystem module reported "no method
        // `len` found for type `fs`".
        if let ExprKind::Ident(name) = &object.kind {
            if self.types.builtin_modules.is_module(name) && !self.local_shadows_namespace(name) {
                return self.check_module_method(name, method, args, type_args, span);
            }
        }

        // Primitive type namespace: char.from_u32(n). `char` is a type name here,
        // not a variable — route to the primitive's method resolver.
        if let ExprKind::Ident(name) = &object.kind {
            if name == "char" && self.lookup_local(name).is_none() {
                let arg_types: Vec<_> = args.iter().map(|a| self.infer_expr(&a.expr)).collect();
                let ret_ty = self.ctx.fresh_var();
                self.ctx.add_constraint(TypeConstraint::HasMethod {
                    ty: Type::Char,
                    method: method.to_string(),
                    args: arg_types,
                    ret: ret_ty.clone(),
                    span,
                    call_node: Some(call_id),
                });
                return ret_ty;
            }
        }

        // Type-level namespaces: Vec.new(), Map.new(), Rng.new(), Pool.new()
        // These are type names, not variables — skip ESAD borrow check and
        // emit UnresolvedNamed directly instead of calling infer_expr
        // (which would return Type::Error for unregistered type names).
        // Also handles generic forms like Vec<Route>.from().
        if let ExprKind::Ident(name) = &object.kind {
            // Extract base type name for generic types (e.g. "Vec<Route>" → "Vec")
            let spelled = name.split('<').next().unwrap_or(name);
            // IM3: a transparent alias names the same type, so a namespace call
            // through one is a call on the target. The gate below asks the stub
            // registry by spelling, and `import time.Duration as Span` puts
            // `Span` in scope under a name that isn't in there — so the branch
            // was skipped, the receiver went through `infer_expr`, and `let d =
            // Span.from_millis(1)` came back "couldn't work out the type of `d`"
            // (#923).
            let base_name = self.types.alias_target(spelled).unwrap_or(spelled);
            let name = if base_name == spelled {
                name.clone()
            } else {
                name.replacen(spelled, base_name, 1)
            };
            let name = &name;
            // A real local of the same name wins. The stub registry holds the
            // module namespaces (`fs`, `io`, `os`, `time`, `http`, …) as types,
            // so `let fs = Vec.new()` used to land here and answer "no method
            // `len` found for type `fs`".
            let shadowed = !name.contains('<') && self.local_shadows_namespace(spelled);
            if !shadowed
                && (matches!(base_name, "Vec" | "Map" | "Pool" | "Rack" | "Random" | "Thread" | "ThreadPool" | "Mutex" | "Shared" | "Channel" | "Atomic")
                    || rask_stdlib::StubRegistry::load().get_type(base_name).is_some())
            {
                let obj_ty = if name.contains('<') {
                    // Parse generic args, respecting nested angle brackets:
                    // "Shared<Map<string, bool>>" → ["Map<string, bool>"]
                    let inner = &name[base_name.len()+1..name.len()-1];
                    let generic_args = split_type_args(inner)
                        .into_iter()
                        .map(|s| GenericArg::Type(Box::new(parse_type_arg(&s))))
                        .collect();
                    Type::UnresolvedGeneric {
                        name: base_name.to_string(),
                        args: generic_args,
                    }
                } else {
                    Type::UnresolvedNamed(name.clone())
                };
                let arg_types: Vec<_> = args.iter().map(|a| self.infer_expr(&a.expr)).collect();
                let ret_ty = self.ctx.fresh_var();
                self.ctx.add_constraint(TypeConstraint::HasMethod {
                    ty: obj_ty,
                    method: method.to_string(),
                    args: arg_types,
                    ret: ret_ty.clone(),
                    span,
                    call_node: Some(call_id),
                });
                return ret_ty;
            }
        }

        // User-defined enum variant construction: LexError.UnexpectedChar(c, line)
        // The name might be shadowed in scope by a same-named variant from
        // another enum (e.g. CompileError { LexError(LexError) }). Check the
        // type table directly — it's authoritative for type names.
        if let ExprKind::Ident(name) = &object.kind {
            // Look up the type table (not scope) to avoid variant-name shadowing.
            let variant_fields = self.types.get_type_id(name).and_then(|type_id| {
                if let Some(TypeDef::Enum { variants, .. }) = self.types.get(type_id) {
                    variants.iter()
                        .find(|(v, _)| v == method)
                        .map(|(_, fields)| (type_id, fields.clone()))
                } else {
                    None
                }
            });
            if let Some((type_id, field_types)) = variant_fields {
                // A generic enum written without type arguments takes them from
                // the payload: `GrowError.Full(item)` gives each declared
                // parameter a fresh variable that the argument binds. Answering
                // bare `Named(type_id)` dropped them, so the value never matched
                // a declared `void or GrowError<Item>` — the error branch of a
                // generic error type was unwritable (#666).
                let params = self.enum_type_params(type_id);
                let (instantiated, result_ty) = if params.is_empty() {
                    (self.instantiate_type_vars(&field_types), Type::Named(type_id))
                } else {
                    let fresh: Vec<Type> = params.iter().map(|_| self.ctx.fresh_var()).collect();
                    let subst: std::collections::HashMap<&str, Type> = params
                        .iter()
                        .map(|p| p.as_str())
                        .zip(fresh.iter().cloned())
                        .collect();
                    let fields: Vec<Type> = field_types
                        .iter()
                        .map(|t| Self::substitute_type_params(t, &subst))
                        .collect();
                    let ty = Type::Generic {
                        base: type_id,
                        args: fresh
                            .into_iter()
                            .map(|t| crate::types::GenericArg::Type(Box::new(t)))
                            .collect(),
                    };
                    (fields, ty)
                };
                // C4: the slot picks the shape, and a declared payload is a
                // slot. Inferring the argument on its own first made
                // `Node.Branch([1, 2, 3])` a `[i32; 3]` against a `Vec<i32>`
                // payload and rejected it — while the same literal filled a
                // struct field, an argument or a return with no trouble (#922).
                let arg_types: Vec<Type> = args
                    .iter()
                    .enumerate()
                    .map(|(i, a)| match instantiated.get(i) {
                        Some(want) => self.infer_expr_expecting(&a.expr, want),
                        None => self.infer_expr(&a.expr),
                    })
                    .collect();
                // Declared type first: `Equal` reports its left as "expected",
                // and the payload is what the reader has to match. Reversed, the
                // fix line told them to change their expression to the type they
                // had just written.
                for (arg_ty, field_ty) in arg_types.iter().zip(instantiated.iter()) {
                    self.ctx.add_constraint(TypeConstraint::Equal(
                        field_ty.clone(),
                        arg_ty.clone(),
                        span,
                    ));
                }
                if arg_types.len() != instantiated.len() {
                    self.errors.push(TypeError::ArityMismatch {
                        expected: instantiated.len(),
                        found: arg_types.len(),
                        span,
                    });
                }
                return result_ty;
            }
        }

        // ESAD Phase 1: Push borrow for the object being called
        if let ExprKind::Ident(var_name) = &object.kind {
            let mode = self.method_borrow_mode(var_name, method);

            // Deep const: reject `mutate self` methods on const bindings and read-only params.
            // `take self` is allowed — it consumes the value, not mutates it.
            //
            // A receiver bound in the same block is often still a type variable
            // here — nothing solves between the statements of a loop body — and
            // `method_mutates_self` then falls back to "is this name `mutate
            // self` on *any* stdlib type", which says yes to `close`. That is
            // how `let h = Handle.open(i)` followed by `h.close()` — a `take
            // self` method on a user struct — was rejected inside a loop and
            // accepted outside one (#928). Defer to after solving, where the
            // receiver has a real type, exactly as field and index writes
            // already are.
            let recv_ty = self.lookup_local(var_name).unwrap_or(Type::Error);
            let recv_open = matches!(
                self.resolve_named(&self.ctx.apply(&recv_ty)),
                Type::Var(_)
            );
            if self.method_mutates_self(var_name, method) && recv_open {
                if let Some(kind) = self.lookup_binding_kind(var_name) {
                    self.pending_self_mutations.push(super::PendingSelfMutation {
                        root: var_name.clone(),
                        ty: recv_ty,
                        kind,
                        method: method.to_string(),
                        span: object.span,
                    });
                }
            } else if self.method_mutates_self(var_name, method) {
                match self.lookup_binding_kind(var_name) {
                    Some(super::BindingKind::Let) => {
                        self.errors.push(TypeError::MutateConst {
                            name: var_name.clone(),
                            span: object.span,
                        });
                    }
                    // PS2: package-level state behind a sync box is the
                    // sanctioned shape; a bare one is the data race.
                    Some(super::BindingKind::ModuleConst)
                        if !self.is_sync_box(&self.resolve_named(&self.ctx.apply(&recv_ty))) =>
                    {
                        let ty = self.render_type(&self.resolve_named(&self.ctx.apply(&recv_ty)));
                        self.errors.push(TypeError::MutatePackageState {
                            name: var_name.clone(),
                            ty,
                            span: object.span,
                        });
                    }
                    Some(super::BindingKind::WithRead) => {
                        self.errors.push(TypeError::MutateWithBinding {
                            name: var_name.clone(),
                            span: object.span,
                        });
                    }
                    Some(super::BindingKind::Param) => {
                        self.errors.push(TypeError::MutateReadOnlyParam {
                            name: var_name.clone(),
                            span: object.span,
                        });
                    }
                    Some(super::BindingKind::Bound(from)) => {
                        self.errors.push(TypeError::MutateBoundName {
                            name: var_name.clone(),
                            from,
                            span: object.span,
                        });
                    }
                    _ => {}
                }
            }

            // ESAD Phase 2: Check persistent borrow conflict for exclusive methods
            if matches!(mode, BorrowMode::Exclusive) {
                if let Some(borrow) = self.check_persistent_borrow_conflict(var_name) {
                    self.errors.push(TypeError::MutateBorrowedSource {
                        source_var: var_name.clone(),
                        view_var: borrow.view_var.clone(),
                        borrow_span: borrow.borrow_span,
                        mutate_span: object.span,
                    });
                }
            }

            self.push_borrow(var_name.clone(), mode, object.span);
        }

        let obj_ty_raw = self.infer_expr(object);
        let obj_ty = self.resolve_named(&obj_ty_raw);
        let arg_types: Vec<_> = args.iter().map(|a| self.infer_expr(&a.expr)).collect();

        // TR5 for a collection element. `Vec<any Shape>.push(Circle { … })` has
        // to box, but the parameter type here is the container's element
        // variable, so the expected type isn't known at the argument. The
        // receiver's own type argument is: if it's `any Trait`, a concrete
        // argument can only be that element. Without this, push stored a bare
        // struct pointer into a 16-byte element slot and every element read
        // back through whichever vtable was written last (#335).
        // The receiver reached through a *field* isn't resolved yet here — its
        // type arrives from a deferred constraint — so there was no element type
        // to compare against and the push went in unboxed. `h.shapes.push(Circle
        // { r: 2 })` on a `Holder { shapes: Vec<any Shape> }` wrote eight bytes
        // into a sixteen-byte slot and the first `area()` call read a vtable
        // pointer out of whatever followed: SIGSEGV natively, right on the
        // interpreter (#955). Ask again once the receiver has settled.
        if matches!(self.ctx.apply(&obj_ty), Type::Var(_)) {
            for (arg, arg_ty) in args.iter().zip(arg_types.iter()) {
                self.pending_trait_elem_coercions.push((
                    arg.expr.id,
                    Self::is_any_cast(&arg.expr),
                    obj_ty.clone(),
                    arg_ty.clone(),
                ));
            }
        }
        for (arg, arg_ty) in args.iter().zip(arg_types.iter()) {
            let applied = self.ctx.apply(arg_ty);
            for elem in Self::trait_object_type_args(&self.ctx.apply(&obj_ty)) {
                // Only an argument that satisfies the trait can be the element.
                // Without this a `Map<string, any Shape>`'s key was flagged too,
                // and codegen went looking for `string_area`.
                let Type::TraitObject { ref trait_name } = elem else { continue };
                if crate::traits::implements_trait(&self.types, &applied, trait_name) {
                    self.note_trait_coercion(&arg.expr, &elem, arg_ty);
                }
            }
        }

        // SP3: zero step on range is a compile error
        // SP1/SP2: step direction mismatch is a warning
        if method == "step" {
            let is_range = matches!(
                &self.ctx.apply(&obj_ty),
                Type::UnresolvedNamed(n) if n == "Range"
            ) || matches!(
                &self.ctx.apply(&obj_ty),
                Type::UnresolvedGeneric { name, .. } if name == "Range"
            );
            if is_range {
                if let Some(first_arg) = args.first() {
                    let is_zero = matches!(
                        &first_arg.expr.kind,
                        rask_ast::expr::ExprKind::Int(0, _)
                    );
                    if is_zero {
                        self.errors.push(TypeError::ZeroStep { span: first_arg.expr.span });
                    } else {
                        // SP1/SP2: check direction mismatch when literals are available
                        self.check_step_direction(object, first_arg);
                    }
                }
            }
        }

        // `p.cast<U>()` says what the result points at, and it used to be
        // thrown away: the result got a fresh type variable instead, so the
        // `*p` after it had no pointee width to read at and took a whole word.
        // `*p.cast<u8>()` on "abc" answered 6513249 — 0x636261, three bytes read
        // as one number — while the annotated `let q: *u8 = p.cast<u8>()` was
        // right, because the annotation supplied what the call already knew
        // (#986). Recorded before either path runs, since both need it.
        if method == "cast" {
            if let Some(first) = type_args.and_then(|a| a.first()) {
                if let Ok(target) = parse_type_string(first, &self.types) {
                    self.ptr_cast_targets.insert(span, target);
                }
            }
        }

        // Raw pointer methods — resolve directly instead of through HasMethod constraints
        let resolved_obj = self.ctx.apply(&obj_ty);
        if let Type::RawPtr(ref inner) = resolved_obj {
            if let Some(ret) = self.check_raw_ptr_method(inner, method, &arg_types, span) {
                return ret;
            }
        }

        // The receiver's type may not be known yet — `s.as_ptr()` hands back a
        // fresh var that only becomes `*u8` once the constraints are solved. If
        // this call names a pointer method, remember whether we were inside
        // `unsafe`, so the solver can hold it to the same rule the branch above
        // applies (#696).
        if rask_stdlib::ptr_methods::lookup(method).is_some() {
            self.ptr_method_sites.insert(span, self.in_unsafe);
        }

        // `{:debug}` asks nothing of the value — every type derives Debug
        // (std.fmt/G2) — while `{}` goes through Displayable. Both desugar to
        // `__fmt`, and the spec's type code is the first argument, a literal
        // the desugar pass put there. Recorded here because the resolver sees
        // argument *types*, where a constant 1 and a constant 0 look the same.
        if method == "__fmt" && args.len() == 5 {
            if let ExprKind::Int(code, _) = &args[0].expr.kind {
                if rask_ast::fmt_spec::SpecType::from_code(*code as i64)
                    == rask_ast::fmt_spec::SpecType::Debug
                {
                    self.debug_fmt_calls.insert(call_id);
                }
            }
        }

        let ret_ty = self.ctx.fresh_var();

        self.ctx.add_constraint(TypeConstraint::HasMethod {
            ty: obj_ty,
            method: method.to_string(),
            args: arg_types,
            ret: ret_ty.clone(),
            span,
            call_node: Some(call_id),
        });

        ret_ty
    }

    /// SP1/SP2: Check for step direction mismatch on range literals.
    /// Only fires when start, end, and step are all integer literals.
    fn check_step_direction(&mut self, range_expr: &rask_ast::expr::Expr, step_arg: &rask_ast::expr::CallArg) {
        use rask_ast::expr::ExprKind;

        // Extract step value from literal
        let step_val: Option<i128> = match &step_arg.expr.kind {
            ExprKind::Int(v, _) => Some(*v),
            // After desugar, `-1` becomes `(1).neg()`
            ExprKind::MethodCall { object, method: neg_method, args: neg_args, .. }
                if neg_method == "neg" && neg_args.is_empty() =>
            {
                if let ExprKind::Int(v, _) = &object.kind {
                    Some(-v)
                } else {
                    None
                }
            }
            _ => None,
        };

        let step_val = match step_val {
            Some(v) => v,
            None => return, // non-literal step, can't check at compile time
        };

        // Extract start/end from Range expression
        let (start_val, end_val, range_span) = match &range_expr.kind {
            ExprKind::Range { start, end, .. } => {
                let s = start.as_ref().and_then(|e| {
                    if let ExprKind::Int(v, _) = &e.kind { Some(*v) } else { None }
                });
                let e = end.as_ref().and_then(|e| {
                    if let ExprKind::Int(v, _) = &e.kind { Some(*v) } else { None }
                });
                (s, e, range_expr.span)
            }
            _ => return,
        };

        let (start, end) = match (start_val, end_val) {
            (Some(s), Some(e)) => (s, e),
            _ => return, // non-literal bounds, can't check
        };

        // SP1: positive step requires start < end (ascending)
        // SP2: negative step requires start > end (descending)
        let mismatch = if step_val > 0 && start >= end {
            Some(("descending", "positive"))
        } else if step_val < 0 && start <= end {
            Some(("ascending", "negative"))
        } else {
            None
        };

        if let Some((range_dir, step_dir)) = mismatch {
            self.errors.push(TypeError::StepDirectionMismatch {
                range_span,
                step_span: step_arg.expr.span,
                range_direction: range_dir.to_string(),
                step_direction: step_dir.to_string(),
            });
        }
    }

    /// Resolve methods on raw pointer types (*T).
    /// Returns Some(return_type) if the method is recognized, None otherwise.
    ///
    /// Shape and unsafe-ness both come from `ptr_methods::PTR_METHODS` so this
    /// path and the constraint solver's can't drift apart.
    fn check_raw_ptr_method(
        &mut self,
        inner: &Type,
        method: &str,
        _args: &[Type],
        span: Span,
    ) -> Option<Type> {
        let entry = rask_stdlib::ptr_methods::lookup(method)?;

        if entry.needs_unsafe {
            let category = match entry.sig {
                rask_stdlib::PtrSig::Arith => super::UnsafeCategory::PointerArithmetic,
                _ => super::UnsafeCategory::PointerMethod,
            };
            self.unsafe_ops.push((span, category));
            if !self.in_unsafe {
                self.errors.push(TypeError::UnsafeRequired {
                    operation: format!("pointer method .{}()", method),
                    span,
                });
            }
        }

        Some(self.raw_ptr_method_return(entry, inner, span))
    }

    /// The type a pointer method hands back, given the pointee.
    pub(super) fn raw_ptr_method_return(
        &mut self,
        entry: &rask_stdlib::PtrMethod,
        inner: &Type,
        span: Span,
    ) -> Type {
        use rask_stdlib::PtrSig;
        match entry.sig {
            PtrSig::Read => inner.clone(),
            PtrSig::Write => Type::Unit,
            PtrSig::Arith => Type::RawPtr(Box::new(inner.clone())),
            PtrSig::Predicate | PtrSig::PredicateInt | PtrSig::Comparison => Type::Bool,
            PtrSig::ToInt => Type::I64,
            // The written `<U>` when there is one. A bare `p.cast()` still gets
            // a variable, and the annotation on its binding settles it.
            PtrSig::Cast => match self.ptr_cast_targets.get(&span).cloned() {
                Some(target) => Type::RawPtr(Box::new(target)),
                None => Type::RawPtr(Box::new(self.ctx.fresh_var())),
            },
        }
    }

    pub(super) fn check_module_method(
        &mut self,
        module: &str,
        method: &str,
        args: &[CallArg],
        type_args: Option<&[String]>,
        span: Span,
    ) -> Type {
        let arg_types: Vec<_> = args.iter().map(|a| self.infer_expr(&a.expr)).collect();

        // A signature with nothing behind it. Methods on a receiver are caught
        // in resolve_method; module functions took a different route and got
        // no check at all, so `json.decode(body)` type-checked, compiled, and
        // segfaulted at the call (#506).
        if rask_stdlib::mir_metadata::is_unimplemented(module, method) {
            self.errors.push(TypeError::UnimplementedStdlibMethod {
                ty: module.to_string(),
                method: method.to_string(),
                span,
            });
            return Type::Error;
        }

        // Cloned rather than borrowed: recording a trait coercion below mutates
        // the checker, and the borrow would outlive the whole body.
        if let Some(sig) = self.types.builtin_modules.get_method(module, method).cloned() {
            let mut trait_params: Vec<(Expr, Type, Type)> = Vec::new();
            // Check parameter count — skip for wildcard params (_Any accepts anything)
            let has_wildcard = sig.params.iter().any(|p| {
                matches!(p, Type::UnresolvedNamed(n) if n == "_Any")
            });
            if !has_wildcard && sig.params.len() != arg_types.len() {
                self.errors.push(TypeError::ArityMismatch {
                    expected: sig.params.len(),
                    found: arg_types.len(),
                    span,
                });
                return Type::Error;
            }

            // Check parameter types (skip _Any wildcards)
            if !has_wildcard {
                for ((param_ty, arg_ty), arg) in
                    sig.params.iter().zip(arg_types.iter()).zip(args.iter())
                {
                    // TR5: a concrete value flowing into an `any Trait`
                    // parameter needs a vtable, and MIR builds it from this
                    // note. A module function's arguments were the one call
                    // position that never recorded it — `io.copy(buf, out)`
                    // passed the raw struct pointer, and the first dispatch
                    // through it jumped to address zero (#860).
                    trait_params.push((arg.expr.clone(), param_ty.clone(), arg_ty.clone()));
                    self.ctx.add_constraint(TypeConstraint::Equal(
                        param_ty.clone(),
                        arg_ty.clone(),
                        span,
                    ));
                }
            }
            for (arg_expr, param_ty, arg_ty) in trait_params {
                self.note_trait_coercion(&arg_expr, &param_ty, &arg_ty);
            }

            // If explicit type args provided (e.g., json.decode<Foo>),
            // substitute them directly instead of using unconstrained fresh vars
            let ret = sig.ret.clone();
            let bounds = sig.type_param_bounds.clone();
            if let Some(ta) = type_args {
                if ta.len() == 1 {
                    let explicit_ty = self.resolve_type_name(&ta[0], span);
                    // The stub's bound is the whole check on this path — there's
                    // no body to infer from. `json.decode<T: Decode>` with a T
                    // that isn't Decode used to type-check clean and then fail in
                    // MIR lowering on native, or blame a missing field on interp.
                    if let Some((_, bound)) = bounds.first() {
                        self.check_type_arg_bound(&explicit_ty, bound, span);
                    }
                    return self.freshen_module_return_type_with(&ret, &explicit_ty);
                }
            }

            // Replace placeholder types with fresh vars for generic module methods
            self.freshen_module_return_type(&ret)
        } else {
            self.errors.push(TypeError::NoSuchMethod {
                ty: Type::UnresolvedNamed(module.to_string()),
                method: method.to_string(),
                span,
            });
            Type::Error
        }
    }

    /// Replace internal placeholder types (_JsonDecodeResult, _Any) with fresh type vars.
    pub(super) fn freshen_module_return_type(&mut self, ty: &Type) -> Type {
        match ty {
            Type::UnresolvedNamed(n) if n.starts_with('_') => self.ctx.fresh_var(),
            Type::Result { ok, err } if **err == Type::None => {
                Type::option(self.freshen_module_return_type(ok))
            }
            Type::Result { ok, err } => Type::Result {
                ok: Box::new(self.freshen_module_return_type(ok)),
                err: Box::new(self.freshen_module_return_type(err)),
            },
            _ => ty.clone(),
        }
    }

    /// Replace internal placeholder types with an explicit type (from type args).
    fn freshen_module_return_type_with(&mut self, ty: &Type, explicit: &Type) -> Type {
        match ty {
            Type::UnresolvedNamed(n) if n.starts_with('_') => explicit.clone(),
            Type::Result { ok, err } if **err == Type::None => {
                Type::option(self.freshen_module_return_type_with(ok, explicit))
            }
            Type::Result { ok, err } => Type::Result {
                ok: Box::new(self.freshen_module_return_type_with(ok, explicit)),
                err: Box::new(self.freshen_module_return_type_with(err, explicit)),
            },
            _ => ty.clone(),
        }
    }

    /// Check a written type argument against the bound the stub declared for it.
    ///
    /// Unresolved names are skipped — a bare type parameter forwarded from an
    /// enclosing generic isn't concrete yet, and reporting it here would blame
    /// the wrong call site.
    fn check_type_arg_bound(&mut self, ty: &Type, bound: &str, span: Span) {
        let resolved = self.resolve_named(ty);
        match &resolved {
            Type::Var(_) | Type::Error => return,
            Type::UnresolvedNamed(_) => return,
            _ => {}
        }
        let trait_bound = crate::traits::TraitBound::new("_", vec![bound.to_string()]);
        if let Err(errs) = crate::traits::verify_instantiation(
            &self.types,
            &resolved,
            std::slice::from_ref(&trait_bound),
            span,
        ) {
            for e in errs {
                let (ty_name, trait_name) = super::validate::trait_error_parts(&e);
                let err = self.bound_error(&resolved, ty_name, trait_name, span);
                self.errors.push(err);
            }
        }
    }

    /// The type arguments written at a call, read back out of the callee's
    /// name.
    ///
    /// The parser folds `make<i32>` into one identifier rather than a separate
    /// node, so this is where the written arguments live. Empty for a call with
    /// none, and for a name whose `<…>` isn't a well-formed argument list.
    fn written_type_args(callee: &str) -> Vec<Type> {
        let Some(open) = callee.find('<') else { return Vec::new() };
        if !callee.ends_with('>') {
            return Vec::new();
        }
        split_type_args(&callee[open + 1..callee.len() - 1])
            .iter()
            .map(|a| parse_type_arg(a.trim()))
            .collect()
    }

    /// Resolve a type name string to a Type.
    ///
    /// Generic spellings have to be parsed, not wrapped whole: `json.decode
    /// <Vec<Point>>` handed back `UnresolvedNamed("Vec<Point>")`, and nothing
    /// finds `len` on a type whose name is that string.
    fn resolve_type_name(&self, name: &str, _span: Span) -> Type {
        let parsed = parse_type_arg(name.trim());
        match &parsed {
            Type::UnresolvedNamed(_) => self.resolve_named(&parsed),
            _ => parsed,
        }
    }

    /// Format a type with resolved names (Named(id) → "TypeName").
    pub(super) fn fmt_ty(&self, ty: &Type) -> String {
        format!("{}", self.types.resolve_type_names(ty))
    }

    /// Hand a plain `try`'s error to the enclosing function: the same type, a
    /// member of its error union (ER31), or a variant of its error enum (ER31a).
    ///
    /// When the source error is still an unresolved variable and the target is
    /// an enum that could wrap it, the decision waits for
    /// `resolve_pending_try_wraps`. Unifying now would answer the question by
    /// force: `try dto.validate()` inside a `-> Response or ApiError` handler
    /// would pin `validate`'s own error type to `ApiError` before method
    /// resolution ever ran, and the real signature then looks like the mistake.
    /// ER18: `try { … } catch e => …`. The handler covers the whole block — the
    /// first error any inner `try` raises goes to it — so the binder is whatever
    /// the block propagates, not the enclosing function's error type.
    fn check_try_block_with_handler(
        &mut self,
        block: &Expr,
        clause: &rask_ast::expr::CatchClause,
        span: Span,
    ) -> Type {
        let block_err = self.ctx.fresh_var();
        self.try_block_errors.push(block_err.clone());
        let block_ty = self.infer_expr(block);
        self.try_block_errors.pop();
        // A block with no `try` in it leaves the variable open; the enclosing
        // function's error type is the only other thing the binder could stand
        // for, and that's what this used to read in every case.
        let err_ty = match self.ctx.apply(&block_err) {
            Type::Var(_) => self
                .current_return_type
                .as_ref()
                .map(|t| self.ctx.apply(t))
                .and_then(|t| match t {
                    Type::Result { err, .. } => Some(*err),
                    _ => None,
                })
                .unwrap_or(block_err),
            settled => settled,
        };
        self.push_scope();
        if !clause.is_discard() {
            self.define_local_bound(
                clause.binder.clone(),
                err_ty,
                super::BoundFrom::Payload,
            );
        }
        let handler_ty = self.infer_expr(&clause.body);
        self.pop_scope();
        // A diverging handler produces nothing, and constraining it would drag
        // the whole expression's type down to `!`.
        if !matches!(self.ctx.apply(&handler_ty), Type::Never) {
            self.ctx.add_constraint(TypeConstraint::Equal(
                block_ty.clone(),
                handler_ty,
                span,
            ));
        }
        block_ty
    }

    pub(super) fn propagate_try_error(
        &mut self,
        node: rask_ast::NodeId,
        err: &Type,
        ret_err: &Type,
        span: Span,
    ) {
        let src = self.ctx.apply(err);
        let target = self.ctx.apply(ret_err);
        if matches!(src, Type::Var(_)) && self.wrap_candidate(&target) {
            self.pending_try_errors.push((node, src, target, span));
            return;
        }
        self.settle_try_error(node, &src, &target, span);
    }

    /// Decide one `try` site's error propagation, source type in hand.
    fn settle_try_error(&mut self, node: rask_ast::NodeId, src: &Type, target: &Type, span: Span) {
        if self.try_wrap_error(node, src, target, span) {
            return;
        }
        if self.unify(src, target, span).is_err() {
            let inner_err = self.fmt_ty(&self.ctx.apply(src));
            let outer_err = self.fmt_ty(&self.ctx.apply(target));
            self.errors.push(TypeError::TryErrorMismatch { inner_err, outer_err, span });
        }
    }

    /// Could this error type wrap something? True for an enum with at least one
    /// single-payload variant — the only shape ER31a can target.
    fn wrap_candidate(&self, ty: &Type) -> bool {
        let Type::Named(id) = ty else { return false };
        matches!(self.types.get(*id), Some(super::TypeDef::Enum { variants, .. })
            if variants.iter().any(|(_, payload)| payload.len() == 1))
    }

    /// Settle the `try` sites that were waiting on their source error type.
    /// Runs after constraint solving, so a method call's real error type is in.
    pub(super) fn resolve_pending_try_wraps(&mut self) {
        for (node, src, target, span) in std::mem::take(&mut self.pending_try_errors) {
            let src = self.ctx.apply(&src);
            let target = self.ctx.apply(&target);
            self.settle_try_error(node, &src, &target, span);
        }
    }

    /// ER31a: can this `try` hand its error to the enclosing function by wrapping
    /// it in a variant of the function's error enum? The enum analogue of ER31's
    /// subset check — `StoreError` reaches an `ApiError` return because
    /// `ApiError.Store(StoreError)` is the one variant shaped to hold it.
    ///
    /// Records the variant for both backends and returns true when it applies.
    /// Two candidate variants is a compile error, not a coin flip.
    fn try_wrap_error(
        &mut self,
        node: rask_ast::NodeId,
        src: &Type,
        target: &Type,
        span: Span,
    ) -> bool {
        let src = self.ctx.apply(src);
        let target = self.ctx.apply(target);
        let variants = self.types.error_wrap_variants(&src, &target);
        match variants.len() {
            0 => false,
            1 => {
                let enum_name = self.fmt_ty(&target);
                self.error_wraps.insert(
                    node,
                    super::ErrorWrap { enum_name, variant: variants[0].clone() },
                );
                true
            }
            _ => {
                self.errors.push(TypeError::AmbiguousErrorWrap {
                    inner_err: self.fmt_ty(&src),
                    outer_err: self.fmt_ty(&target),
                    variants,
                    span,
                });
                // Reported — don't also report a plain mismatch on the same site.
                true
            }
        }
    }

    /// The declared type of `field` on the annotation `get<A>()` names, when
    /// `object` is such a call (type.annotations/AN6). `None` for anything else.
    ///
    /// An unknown field errors here rather than falling through, so the message
    /// names the annotation instead of blaming a missing method on a value that
    /// was never going to exist.
    fn annotation_projection_type(&mut self, object: &Expr, field: &str, span: Span) -> Option<Type> {
        let ExprKind::MethodCall { method, type_args, .. } = &object.kind else { return None };
        if method != "get" {
            return None;
        }
        let name = type_args.as_ref()?.first()?;
        if !self.annotation_types.contains(name) {
            return None;
        }
        let id = match self.types.lookup(name) {
            Some(Type::Named(id)) => id,
            _ => return Some(Type::Error),
        };
        let fields = match self.types.get(id) {
            Some(TypeDef::Struct { fields, .. }) => fields.clone(),
            _ => return Some(Type::Error),
        };
        match fields.iter().find(|(n, _)| n == field) {
            Some((_, ty)) => Some(ty.clone()),
            None => {
                self.errors.push(TypeError::BadAnnotation {
                    name: name.clone(),
                    problem: format!("no field `{}` to read", field),
                    fix: format!(
                        "fields: {}",
                        fields.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ")
                    ),
                    why: "an annotation read names one of the fields the declaration lists — there is no value to look anything else up on [type.annotations/AN6]",
                    span,
                });
                Some(Type::Error)
            }
        }
    }

    /// The field name in `value.(expr)` when the source spells it out: a string
    /// literal, or a `comptime { … }` block whose value is one. Read off the
    /// syntax — this pass has no comptime evaluator, and doesn't need one for
    /// the shapes a reader actually writes.
    /// Whether `expr` is a shape whose value the compiler will know, and so
    /// can name a field in `value.(expr)`.
    ///
    /// The list is MIR's `comptime_field_name`, which is what does the rewrite:
    /// a string literal, a `comptime { … }` block, a `let` bound to either, or
    /// a `comptime for` binding's `.name`/`.serial_name`/`.type_name`. The last
    /// of those only resolves when the loop unrolls, so it's accepted here on
    /// its shape and settled in lowering.
    pub(super) fn comptime_field_name_shape(&self, expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::String(_) | ExprKind::Comptime { .. } => true,
            ExprKind::Ident(name) => self.is_comptime_string(name),
            ExprKind::Field { field, .. } => {
                matches!(field.as_str(), "name" | "serial_name" | "type_name")
            }
            _ => false,
        }
    }

    fn literal_field_name(expr: &Expr) -> Option<String> {
        match &expr.kind {
            ExprKind::String(s) => Some(s.clone()),
            ExprKind::Comptime { body } => match body.as_slice() {
                [Stmt { kind: StmtKind::Expr(e), .. }] => Self::literal_field_name(e),
                _ => None,
            },
            _ => None,
        }
    }

    pub(super) fn check_field_access(&mut self, object: &Expr, field: &str, span: Span) -> Type {
        // AN6/AN8: `item.get<A>().weight` — the one legal shape for `get`. The
        // field's type comes straight from the annotation's declaration, and
        // the receiver is deliberately not checked: reaching `check_method_call`
        // for a `get<A>()` means it wasn't projected, which AN8 rejects.
        if let Some(ty) = self.annotation_projection_type(object, field, span) {
            return ty;
        }

        // Primitive type constants: u64.MAX, i32.MIN, etc.
        if let ExprKind::Ident(name) = &object.kind {
            if let Some(ty) = Self::primitive_type_constant(name, field) {
                return ty;
            }
            // G4: @binary struct SIZE/SIZE_BITS constants
            if matches!(field, "SIZE" | "SIZE_BITS") {
                if let Some(type_id) = self.types.get_type_id(name) {
                    if self.types.is_binary_type_by_id(type_id) {
                        return Type::U64;
                    }
                }
            }
        }

        let obj_ty_raw = self.infer_expr(object);
        let obj_ty = self.resolve_named(&obj_ty_raw);

        // UN2: union field reads require unsafe (UN3: writes are safe)
        if !self.in_assign_target {
            if let Type::Named(type_id) = &obj_ty {
                if let Some(TypeDef::Union { .. }) = self.types.get(*type_id) {
                    self.unsafe_ops.push((span, super::UnsafeCategory::UnionFieldAccess));
                    if !self.in_unsafe {
                        self.errors.push(TypeError::UnsafeRequired {
                            operation: "union field access".to_string(),
                            span,
                        });
                    }
                }
            }
        }

        let field_ty = self.ctx.fresh_var();

        self.ctx.add_constraint(TypeConstraint::HasField {
            ty: obj_ty,
            field: field.to_string(),
            expected: field_ty.clone(),
            span,
            self_type: self.current_self_type.clone(),
        });

        field_ty
    }

    /// type.primitives/NT1 — `ZERO`, `ONE`, `MIN`, `MAX` on every numeric type,
    /// plus the float-only `EPSILON`/`NAN`/`INFINITY`.
    ///
    /// The constant has the type it names: `i32.MAX` is an `i32`. Handing back
    /// `UnresolvedNamed("i32")` instead meant nothing downstream could see a
    /// number there, so `i32.MAX == 2147483647` failed with "no method `eq`
    /// found for type `i32`".
    pub(super) fn primitive_type_constant(type_name: &str, field: &str) -> Option<Type> {
        let float_only = matches!(field, "EPSILON" | "NAN" | "INFINITY");
        if !float_only && !matches!(field, "MAX" | "MIN" | "ZERO" | "ONE") {
            return None;
        }
        let ty = match type_name {
            "u8" => Type::U8,
            "u16" => Type::U16,
            "u32" => Type::U32,
            "u64" => Type::U64,
            "usize" => Type::usize_ty(),
            "u128" => Type::U128,
            "i8" => Type::I8,
            "i16" => Type::I16,
            "i32" => Type::I32,
            "i64" => Type::I64,
            "isize" => Type::isize_ty(),
            "i128" => Type::I128,
            "f32" => Type::F32,
            "f64" => Type::F64,
            _ => return None,
        };
        if float_only && !matches!(ty, Type::F32 | Type::F64) {
            return None;
        }
        Some(ty)
    }

    /// A symbol's type, with its own type parameters in scope while it parses.
    ///
    /// A callee's signature is parsed here — at the *call site* — so without
    /// this the scope in effect is the caller's, and a parameter named like a
    /// real type resolves to that type: `func first<Output>(…)` was read as the
    /// stdlib's `os.Output` from every call, and the argument mismatched
    /// against a type nobody wrote (#915). Wrapping rather than pushing inline
    /// because the scope has to come back off on every path out, and there are
    /// a dozen.
    pub(super) fn get_symbol_type(&mut self, sym_id: SymbolId) -> Type {
        let callee_params: Vec<String> =
            self.fn_type_params.get(&sym_id).cloned().unwrap_or_default();
        if callee_params.is_empty() {
            return self.get_symbol_type_scoped(sym_id);
        }
        let outer = self.types.push_type_params(callee_params);
        let ty = self.get_symbol_type_scoped(sym_id);
        self.types.pop_type_params(outer);
        ty
    }

    fn get_symbol_type_scoped(&mut self, sym_id: SymbolId) -> Type {
        if let Some(ty) = self.symbol_types.get(&sym_id) {
            return ty.clone();
        }

        if let Some(sym) = self.resolved.symbols.get(sym_id) {
            match &sym.kind {
                SymbolKind::Function { ret_ty, params, .. } => {
                    let param_types: Vec<_> = params
                        .iter()
                        .filter_map(|pid| {
                            self.resolved.symbols.get(*pid).and_then(|p| {
                                p.ty.as_ref()
                                    .and_then(|t| parse_type_string(t, &self.types).ok())
                            })
                        })
                        .collect();
                    let ret = ret_ty
                        .as_ref()
                        .and_then(|t| parse_type_string(t, &self.types).ok())
                        .unwrap_or(Type::Unit);
                    return Type::Fn {
                        params: param_types,
                        ret: Box::new(ret),
                    };
                }
                SymbolKind::ExternFunction { params, ret_ty, .. } => {
                    let param_types: Vec<_> = params
                        .iter()
                        .filter_map(|p| parse_type_string(p, &self.types).ok())
                        .collect();
                    let ret = ret_ty
                        .as_ref()
                        .and_then(|t| parse_type_string(t, &self.types).ok())
                        .unwrap_or(Type::Unit);
                    return Type::Fn {
                        params: param_types,
                        ret: Box::new(ret),
                    };
                }
                SymbolKind::Variable { .. } | SymbolKind::Parameter { .. } => {
                    if let Some(ty_str) = &sym.ty {
                        if let Ok(ty) = parse_type_string(ty_str, &self.types) {
                            return ty;
                        }
                    }
                }
                SymbolKind::Struct { .. } => {
                    if let Some(type_id) = self.types.get_type_id(&sym.name) {
                        return Type::Named(type_id);
                    }
                }
                SymbolKind::Enum { .. } => {
                    if let Some(type_id) = self.types.get_type_id(&sym.name) {
                        return Type::Named(type_id);
                    }
                }
                SymbolKind::EnumVariant { enum_id } => {
                    if let Some(enum_sym) = self.resolved.symbols.get(*enum_id) {
                        let type_id = if enum_sym.span == Span::new(0, 0) {
                            match enum_sym.name.as_str() {
                                "Result" => self.types.get_result_type_id(),
                                "Option" => self.types.get_option_type_id(),
                                _ => None,
                            }
                        } else {
                            self.types.get_type_id(&enum_sym.name)
                        };

                        if let Some(id) = type_id {
                            let variant_fields = self.types.get(id).and_then(|def| {
                                if let TypeDef::Enum { variants, .. } = def {
                                    variants.iter()
                                        .find(|(n, _)| n == &sym.name)
                                        .map(|(_, fields)| fields.clone())
                                } else {
                                    None
                                }
                            });

                            if let Some(fields) = variant_fields {
                                if fields.is_empty() {
                                    return Type::Named(id);
                                } else {
                                    let (param_types, ret_type) = if Some(id) == self.types.get_result_type_id() {
                                        let t_var = self.ctx.fresh_var();
                                        let e_var = self.ctx.fresh_var();
                                        let params = match sym.name.as_str() {
                                            "Ok" => vec![t_var.clone()],
                                            "Err" => vec![e_var.clone()],
                                            _ => fields.clone(),
                                        };
                                        let ret = Type::Result {
                                            ok: Box::new(t_var),
                                            err: Box::new(e_var),
                                        };
                                        (params, ret)
                                    } else if Some(id) == self.types.get_option_type_id() {
                                        let t_var = self.ctx.fresh_var();
                                        let params = if sym.name == "Some" {
                                            vec![t_var.clone()]
                                        } else {
                                            vec![]
                                        };
                                        let ret = Type::option(t_var);
                                        (params, ret)
                                    } else {
                                        // A generic enum takes its type args from
                                        // the payload: `GrowError.Full(item)` writes
                                        // no `<…>`, so each declared parameter gets
                                        // a fresh variable the argument then binds.
                                        // Typing the result as bare `Named(id)`
                                        // dropped them, and the value never matched
                                        // the declared `void or GrowError<Item>` —
                                        // so no generic error type could be returned
                                        // at all (#666).
                                        let params = self.enum_type_params(id);
                                        if params.is_empty() {
                                            let instantiated = self.instantiate_type_vars(&fields);
                                            (instantiated, Type::Named(id))
                                        } else {
                                            let fresh: Vec<Type> =
                                                params.iter().map(|_| self.ctx.fresh_var()).collect();
                                            let subst: std::collections::HashMap<&str, Type> = params
                                                .iter()
                                                .map(|p| p.as_str())
                                                .zip(fresh.iter().cloned())
                                                .collect();
                                            let instantiated: Vec<Type> = fields
                                                .iter()
                                                .map(|f| Self::substitute_type_params(f, &subst))
                                                .collect();
                                            let ret = Type::Generic {
                                                base: id,
                                                args: fresh
                                                    .into_iter()
                                                    .map(|t| crate::types::GenericArg::Type(Box::new(t)))
                                                    .collect(),
                                            };
                                            (instantiated, ret)
                                        }
                                    };

                                    return Type::Fn {
                                        params: param_types,
                                        ret: Box::new(ret_type),
                                    };
                                }
                            } else {
                                return Type::Named(id);
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        let var = self.ctx.fresh_var();
        self.symbol_types.insert(sym_id, var.clone());
        var
    }

    /// Check that a match on an enum or `T or E` result covers all branches.
    fn check_match_exhaustiveness(&mut self, scrutinee_ty: &Type, arms: &[MatchArm], span: Span) {
        let resolved = self.ctx.apply(scrutinee_ty);

        // ER30: exhaustiveness check for `T or E` result matches.
        // Collect required coverage: ok type + all error leaf types.
        if matches!(resolved, Type::Result { .. }) {
            // Every branch the value could actually be. A flat `T? or E` has
            // three (`T`, `none`, `E`) — the layers are matched apart, not
            // collapsed (OPT30).
            let leaves = super::check_pattern::two_branch_leaves(
                &mut self.ctx,
                &self.types,
                &resolved,
            );
            let required: Vec<String> = leaves.iter().map(|t| self.fmt_ty(t)).collect();

            let mut has_wildcard = false;
            let mut covered: std::collections::HashSet<String> = std::collections::HashSet::new();
            for arm in arms {
                self.collect_result_covered(&arm.pattern, &required, &mut covered, &mut has_wildcard);
            }

            if has_wildcard {
                return;
            }

            // A generic branch named without its arguments covers it —
            // `CasFailed` for a `CasFailed<i64>` branch. Only when the base
            // name picks out one branch; two instantiations of the same type
            // would both answer to it and neither would be covered.
            let base_of = |t: &str| t.split('<').next().unwrap_or(t).to_string();
            let missing: Vec<String> = required
                .iter()
                .filter(|r| {
                    if covered.contains(*r) {
                        return false;
                    }
                    let base = base_of(r);
                    let same_base = required.iter().filter(|o| base_of(o) == base).count();
                    !(same_base == 1 && covered.contains(&base))
                })
                .cloned()
                .collect();

            if !missing.is_empty() {
                self.errors.push(TypeError::NonExhaustiveMatch { missing, span });
            }
            return;
        }

        // Only check enums for the Named case
        let type_id = match &resolved {
            Type::Named(id) => *id,
            _ => return,
        };

        let all_variants: Vec<String> = match self.types.get(type_id) {
            Some(TypeDef::Enum { variants, .. }) => {
                variants.iter().map(|(name, _)| name.clone()).collect()
            }
            _ => return,
        };

        // Collect covered variant names from patterns
        let mut has_wildcard = false;
        let mut covered = std::collections::HashSet::new();
        for arm in arms {
            self.collect_covered_variants(&arm.pattern, &mut covered, &mut has_wildcard, &all_variants);
        }

        if has_wildcard {
            return;
        }

        let missing: Vec<String> = all_variants
            .into_iter()
            .filter(|v| !covered.contains(v))
            .collect();

        if !missing.is_empty() {
            self.errors.push(TypeError::NonExhaustiveMatch {
                missing,
                span,
            });
        }
    }

    fn collect_result_covered(
        &self,
        pattern: &Pattern,
        required: &[String],
        covered: &mut std::collections::HashSet<String>,
        has_wildcard: &mut bool,
    ) {
        match pattern {
            Pattern::Wildcard => *has_wildcard = true,
            Pattern::Ident(name) => {
                // Bare ident that doesn't match a required type name → catch-all
                if required.contains(name) {
                    covered.insert(name.clone());
                } else {
                    *has_wildcard = true;
                }
            }
            Pattern::TypePat { ty_name, .. } => {
                covered.insert(ty_name.clone());
            }
            Pattern::Or(alts) => {
                for alt in alts {
                    self.collect_result_covered(alt, required, covered, has_wildcard);
                }
            }
            _ => {}
        }
    }

    fn collect_covered_variants(
        &self,
        pattern: &Pattern,
        covered: &mut std::collections::HashSet<String>,
        has_wildcard: &mut bool,
        enum_variants: &[String],
    ) {
        match pattern {
            Pattern::Wildcard => *has_wildcard = true,
            Pattern::Ident(name) => {
                // Bare identifier matching an enum variant name is a variant match,
                // not a catch-all binding
                if enum_variants.contains(name) {
                    covered.insert(name.clone());
                } else {
                    *has_wildcard = true;
                }
            }
            Pattern::Constructor { name, .. } => {
                // Qualified names like "Enum.Variant" — extract the variant part
                let variant = name.rsplit('.').next().unwrap_or(name);
                covered.insert(variant.to_string());
            }
            // Struct-style variant pattern `Enum.Variant { field, .. }` —
            // same coverage semantics as Constructor.
            Pattern::Struct { name, .. } => {
                let variant = name.rsplit('.').next().unwrap_or(name);
                covered.insert(variant.to_string());
            }
            Pattern::Or(patterns) => {
                for p in patterns {
                    self.collect_covered_variants(p, covered, has_wildcard, enum_variants);
                }
            }
            _ => {}
        }
    }

    /// Detect `opt is Some` (no bindings) in an if-condition and extract
    /// the variable name and its narrowed inner type (OPT10 type narrowing).
    /// Also handles `opt is Some` within `&&` chains.
    /// ER22: what an `else as e` binds after `if r is T as v`. The scrutinee
    /// has two branches; the pattern named one, so the else gets the other.
    /// `None` when the scrutinee isn't two-branch, or the pattern didn't name
    /// a branch of it.
    fn complement_branch(
        &mut self,
        pattern: &Pattern,
        scrutinee_ty: &Type,
    ) -> Option<Type> {
        let Pattern::TypePat { ty_name, .. } = pattern else { return None };
        let resolved = self.ctx.apply(scrutinee_ty);
        if !matches!(resolved, Type::Result { .. }) {
            return None;
        }
        let named = super::check_pattern::normalize_type(
            &super::parse_type::parse_type_string(ty_name, &self.types).ok()?,
            &self.types,
        );
        let leaves = super::check_pattern::two_branch_leaves(&mut self.ctx, &self.types, &resolved);
        let rest: Vec<Type> = leaves.into_iter().filter(|t| *t != named).collect();
        match rest.len() {
            0 => None,
            1 => Some(rest.into_iter().next().unwrap()),
            _ => Some(Type::Union(rest)),
        }
    }

    /// OPT19/ER23: the `as v` bind on a presence test — `if x? as v`. Returns
    /// the name, the type `v` gets, and (when the scrutinee can fail) the type
    /// an `else as e` would bind.
    ///
    /// This is a *binding*, not a narrow: the scrutinee keeps its own type
    /// everywhere. Without the `as`, there's nothing to introduce.
    ///
    /// Must run after the cond has been inferred, so the scrutinee's type is
    /// in `node_types`.
    pub(super) fn extract_presence_binding(
        &mut self,
        cond: &Expr,
    ) -> Option<(String, Type, Option<Type>)> {
        let ExprKind::IsPresent { expr: inner, binding } = &cond.kind else {
            return None;
        };
        let name = binding.clone()?;

        let scrutinee_ty = self.node_types.get(&inner.id).cloned()?;
        let resolved = self.ctx.apply(&scrutinee_ty);

        // An unsolved scrutinee (a `Map.get` whose V is only fixed by later
        // inserts) gets constrained to a two-branch shape now, so the payload
        // var is available. The fresh error var defers the optional-vs-result
        // question to constraint solving.
        let resolved = if let Type::Var(_) = resolved {
            let ok = self.ctx.fresh_var();
            let err = self.ctx.fresh_var();
            let target = Type::Result {
                ok: Box::new(ok),
                err: Box::new(err),
            };
            let _ = self.unify(&resolved, &target, cond.span);
            self.ctx.apply(&resolved)
        } else {
            resolved
        };

        match resolved {
            Type::Result { ok, err } if *err == Type::None => Some((name, *ok, None)),
            Type::Result { ok, err } => Some((name, *ok, Some(*err))),
            _ => None,
        }
    }

    /// ER47: bare `try` on an optional needs a return with an absent branch.
    fn check_absence_can_leave(&mut self, span: rask_ast::Span) {
        let Some(return_ty) = &self.current_return_type else {
            self.errors.push(TypeError::TryOutsideFunction { span });
            return;
        };
        let resolved = self.ctx.apply(return_ty);
        match &resolved {
            _ if resolved.is_option() => {}
            // Not pinned yet — `try` says the return has an absent branch.
            Type::Var(_) => {
                let ok = self.ctx.fresh_var();
                let _ = self.unify(&resolved, &Type::option(ok), span);
            }
            Type::Error => {}
            Type::Result { .. } => {
                self.errors.push(TypeError::TryAbsenceIntoResult {
                    return_ty: resolved.clone(),
                    span,
                });
            }
            _ => {
                self.errors.push(TypeError::TryInNonPropagatingContext {
                    return_ty: resolved.clone(),
                    span,
                });
            }
        }
    }

    /// ER47: bare `try` on a result needs a return with an error branch. False
    /// when it reported, so the caller stops rather than piling on.
    fn error_can_leave(&mut self, span: rask_ast::Span) -> bool {
        // No return type at all is a `test` (or `benchmark`) block, which has no
        // caller to propagate to: the error ends the test instead, which is what
        // the interpreter has always done and what native does since #932.
        let Some(return_ty) = &self.current_return_type else {
            return true;
        };
        let resolved = self.ctx.apply(return_ty);
        if resolved.is_option() {
            self.errors.push(TypeError::TryErrorIntoOptional {
                return_ty: resolved,
                span,
            });
            return false;
        }
        // A function that returns nothing has nowhere to send the error either,
        // and unlike a test block it isn't a place where ending the run is the
        // right answer. This used to fall through: `func helper() { try f() }`
        // type-checked, then native panicked with a message about test blocks
        // and the interpreter dropped the error on the floor and carried on.
        // The unresolved-operand path below has always reported this; the
        // concrete-Result path is where it leaked.
        if matches!(resolved, Type::Unit) {
            self.errors.push(TypeError::TryInNonPropagatingContext {
                return_ty: resolved,
                span,
            });
            return false;
        }
        true
    }

    /// The variable a place expression is rooted at — `conn.pending` → `conn`.
    fn place_root_name(place: &Expr) -> Option<String> {
        match &place.kind {
            ExprKind::Ident(n) => Some(n.clone()),
            ExprKind::Field { object, .. } | ExprKind::Index { object, .. } => {
                Self::place_root_name(object)
            }
            _ => None,
        }
    }

    /// Is this expression's inferred type `Shared<...>`? (Receiver must have
    /// been inferred already — with-binding sources are.)
    /// Is this resolved type a `Shared<T>`? The by-type twin of `expr_is_shared`,
    /// for a place that already has the type in hand.
    /// Report every task-local `Shared` a spawned closure reaches (SH7).
    ///
    /// The box is captured by naming it, so the names checked inside a `spawn`
    /// argument's span are exactly the boxes that task can touch. Matching on
    /// span containment beats re-walking the body, which would have to know
    /// every expression and statement shape to be right.
    ///
    /// Runs after constraint solving for the same reason
    /// `validate_pending_mutations` does: during the walk, the type of a
    /// `let c = Shared.new(0)` is usually still a variable.
    pub(super) fn validate_spawn_captures(&mut self) {
        if self.spawn_arg_spans.is_empty() {
            return;
        }
        let spans = std::mem::take(&mut self.spawn_arg_spans);
        let uses = std::mem::take(&mut self.local_shared_uses);
        let mut reported: std::collections::HashSet<(String, usize)> =
            std::collections::HashSet::new();
        for (name, ty, span) in uses {
            let Some(i) = spans.iter().position(|s| {
                s.file_id == span.file_id && span.start >= s.start && span.end <= s.end
            }) else {
                continue;
            };
            let resolved = self.resolve_named(&self.ctx.apply(&ty));
            if !Self::type_is_shared(&resolved, &self.types) {
                continue;
            }
            if self.shared_strategy_name(&resolved) != "Local" {
                continue;
            }
            if reported.insert((name.clone(), i)) {
                self.errors.push(TypeError::LocalSharedSent { name, span });
            }
        }
    }

    /// W9: warn when a `with` block over a sync box assigns two or more fields
    /// of the locked binding without `staged()`.
    ///
    /// Exclusive access only — a read lock can't be written through (R1), and
    /// `staged()` is already the fix. `Local` is excluded: nothing else can
    /// observe a torn update there and `staged()` is refused (ST3a), so a
    /// warning would point at a compile error.
    ///
    /// Field assignments only. A mutating method call (`q.push(a)` twice) leaves
    /// the same invariant torn, but a method body is opaque and flagging every
    /// pair of calls would drown the real signal — the spec draws that line, not
    /// this code (tool.warnings, W9 scope).
    fn check_torn_lock_update(
        &mut self,
        binding: &rask_ast::expr::WithBinding,
        body: &[rask_ast::stmt::Stmt],
    ) {
        use rask_ast::stmt::StmtKind;

        if self.allowed_warnings.iter().any(|a| a == "torn_lock_update") {
            return;
        }
        let ExprKind::MethodCall { object, method, args, .. } = &binding.source.kind else {
            return;
        };
        if !args.is_empty() || !matches!(method.as_str(), "write" | "lock") {
            return;
        }
        let Some(recv_ty) = self
            .node_types
            .get(&object.id)
            .map(|t| self.resolve_named(&self.ctx.apply(t)))
        else {
            return;
        };
        if !Self::type_is_shared(&recv_ty, &self.types)
            || self.shared_strategy_name(&recv_ty) == "Local"
        {
            return;
        }

        // Distinct fields of the binding, in the order they are first written.
        let mut written: Vec<(String, rask_ast::Span)> = Vec::new();
        for stmt in body {
            let StmtKind::Assign { target, .. } = &stmt.kind else { continue };
            let ExprKind::Field { object: base, field } = &target.kind else { continue };
            if !matches!(&base.kind, ExprKind::Ident(n) if *n == binding.name) {
                continue;
            }
            if written.iter().any(|(f, _)| f == field) {
                continue;
            }
            written.push((field.clone(), target.span));
            if written.len() == 2 {
                break;
            }
        }
        if written.len() < 2 {
            return;
        }
        self.errors.push(TypeError::TornLockUpdate {
            binding: binding.name.clone(),
            box_name: Self::source_text_for(object).unwrap_or_else(|| "the box".to_string()),
            first_field: written[0].0.clone(),
            second_field: written[1].0.clone(),
            first_span: written[0].1,
            second_span: written[1].1,
        });
    }

    /// The strategy argument's name, or `"Readers"` when there isn't one.
    ///
    /// SH3: bare `Shared<T>` is `Shared<T, Readers>` in every position — a
    /// `let`, a parameter, a field, a return type. This said `Local` and cited
    /// SH3 for it, which is the opposite of what SH3 says; `rask-mir`'s
    /// `shared_strategy` had it right ("a lock you didn't need costs time, and
    /// one you did need and skipped costs correctness", SH8). Both callers here
    /// test for `Local` specifically, so the wrong default only ever made a bare
    /// `Shared<T>` look like the one strategy it can't be.
    ///
    /// Callers guard on `type_is_shared` first; anything unreadable lands on the
    /// default, which is the safe side of both rules that read this.
    fn shared_strategy_name(&self, ty: &Type) -> String {
        let args = match ty {
            Type::UnresolvedGeneric { args, .. } | Type::Generic { args, .. } => args.as_slice(),
            _ => return "Readers".to_string(),
        };
        match args.get(1) {
            Some(GenericArg::Type(s)) => match self.resolve_named(s) {
                Type::UnresolvedNamed(n) => n,
                Type::Named(id) => self.types.type_name(id),
                _ => "Readers".to_string(),
            },
            _ => "Readers".to_string(),
        }
    }

    fn type_is_shared(ty: &Type, types: &crate::TypeTable) -> bool {
        match ty {
            Type::Generic { base, .. } => types.type_name(*base) == "Shared",
            Type::UnresolvedGeneric { name, .. } => name == "Shared",
            _ => false,
        }
    }

    /// How to spell a `with` source back to the author. A name for a plain
    /// binding, a field path for a field; `None` for anything longer, where the
    /// suggestion is better off generic than wrong.
    fn source_text_for(e: &Expr) -> Option<String> {
        match &e.kind {
            ExprKind::Ident(name) => Some(name.clone()),
            ExprKind::Field { object, field } => {
                Some(format!("{}.{}", Self::source_text_for(object)?, field))
            }
            _ => None,
        }
    }

    fn expr_is_shared(&mut self, e: &Expr) -> bool {
        let Some(t) = self.node_types.get(&e.id).cloned() else { return false };
        match self.ctx.apply(&t) {
            Type::Generic { base, .. } => self.types.type_name(base) == "Shared",
            Type::UnresolvedGeneric { name, .. } => name == "Shared",
            _ => false,
        }
    }

    /// Check every void-bodied `catch` whose scrutinee's success type was
    /// still open when it ran, now that solving and literal defaulting have
    /// had a chance to pin that type down some other way (another catch on
    /// the same value, a `try`, an annotation). If nothing ever did, void is
    /// a safe default — there was no other evidence to contradict it.
    pub(super) fn validate_pending_catch_void_checks(&mut self) {
        let pending = std::mem::take(&mut self.pending_catch_void_checks);
        for (ok_ty, span) in pending {
            match self.ctx.apply(&ok_ty) {
                Type::Var(id) => self.ctx.bind_var(id, Type::Unit),
                Type::Unit | Type::Error => {}
                other => {
                    self.errors.push(TypeError::Mismatch {
                        expected: other,
                        found: Type::Unit,
                        span,
                    });
                }
            }
        }
    }

    /// Check every integer literal against the type it ended up with. Deferred
    /// to here because at the literal itself the type is usually still a var.
    ///
    /// Without this `const b: u8 = 300` type-checked, and the backends then
    /// disagreed about what it meant — the interpreter kept 300, codegen kept
    /// the low byte.
    pub(super) fn validate_pending_int_literals(&mut self) {
        let pending = std::mem::take(&mut self.pending_int_literals);
        for (value, bit_pattern, ty, span) in pending {
            let ty = self.ctx.apply(&ty);
            let Some((min, max)) = int_range(&ty) else { continue };
            // Compare as sign plus magnitude: a `u128` above `i128::MAX` is the
            // one literal that doesn't fit an `i128`, and it arrives as a bit
            // pattern rather than a number.
            let (negative, magnitude) = if bit_pattern {
                (false, value as u128)
            } else if value < 0 {
                (true, value.unsigned_abs())
            } else {
                (false, value as u128)
            };
            let fits = if negative {
                min < 0 && magnitude <= min.unsigned_abs()
            } else {
                magnitude <= max
            };
            if fits {
                continue;
            }
            let literal = if negative {
                format!("-{}", magnitude)
            } else {
                magnitude.to_string()
            };
            self.errors.push(TypeError::IntLiteralOutOfRange {
                literal,
                ty,
                min: min.to_string(),
                max: max.to_string(),
                span,
            });
        }
    }

    /// CV1–CV10: validate deferred cast/convert sites. Source types are now
    /// concrete (literal defaults applied), so `1 as bool` sees `i32`.
    pub(super) fn validate_pending_casts(&mut self) {
        let pending = std::mem::take(&mut self.pending_casts);
        for pc in pending {
            let src = self.ctx.apply(&pc.source);
            // Skip unresolved/cascading — don't pile errors on an earlier failure.
            if matches!(src, Type::Var(_) | Type::Error) || matches!(pc.target, Type::Error) {
                continue;
            }
            match pc.convert {
                None => self.check_as_cast(&src, &pc),
                Some(kind) => self.check_convert(&src, kind, &pc),
            }
        }
    }

    /// CV1–CV4, CH5, BL3: reject any `as` cast that isn't lossless widening.
    fn check_as_cast(&mut self, src: &Type, pc: &PendingCast) {
        let (Some(s), Some(t)) = (prim_of(src), prim_of(&pc.target)) else {
            return;
        };
        if as_cast_is_lossless(src, &pc.target, s, t) {
            return;
        }
        self.errors.push(TypeError::InvalidCast {
            src_ty: src.clone(),
            dst_ty: pc.target.clone(),
            target_name: pc.target_name.clone(),
            class: classify_invalid_cast(s, t),
            span: pc.span,
        });
    }

    /// The `ConvertError` type conversions fail with (CV11). Declared in
    /// `stdlib/builtins.rk`, which is always in scope — a conversion is core
    /// language, so its error can't need an import.
    pub(super) fn convert_error_type(&self) -> Type {
        match self.types.get_type_id("ConvertError") {
            Some(id) => Type::Named(id),
            None => Type::UnresolvedNamed("ConvertError".to_string()),
        }
    }

    /// Each method is defined only where its policy means something, so
    /// anywhere else is a compile error rather than a no-op. A method that
    /// reads as if it did something always did.
    fn check_convert_method(&mut self, src: &Type, kind: ConvertKind, pc: &PendingCast) {
        let src_is_int = matches!(prim_of(src), Some(Prim::Int { .. }));
        let src_is_float = matches!(prim_of(src), Some(Prim::Float { .. }));
        let target_is_int = matches!(prim_of(&pc.target), Some(Prim::Int { .. }));
        let target_is_float = matches!(prim_of(&pc.target), Some(Prim::Float { .. }));
        if !(src_is_int || src_is_float) {
            self.errors.push(TypeError::InvalidConvert {
                message: format!(
                    "`{}` converts a number, but `{}` is not a numeric type",
                    kind.surface(), src
                ),
                span: pc.span,
            });
            return;
        }
        let message = match kind {
            // CV11: any numeric to any numeric.
            ConvertKind::To if target_is_int || target_is_float => None,
            ConvertKind::To => Some(format!(
                "`to` produces a number, but the target `{}` is not a numeric type",
                pc.target_name
            )),
            // CV14: never int→int — there is nothing to round.
            ConvertKind::Round if src_is_int && target_is_int => Some(format!(
                "`round` has nothing to round going from `{}` to `{}` — use `to`, `wrap` or `clamp`",
                src, pc.target_name
            )),
            ConvertKind::Round if target_is_int || target_is_float => None,
            ConvertKind::Round => Some(format!(
                "`round` produces a number, but the target `{}` is not a numeric type",
                pc.target_name
            )),
            // CV12/CV13: integers only. "What would a float wrap?" has no
            // answer, and `clamp` would have to pick a fraction policy silently
            // to stay total — the one thing its name can't say.
            ConvertKind::Wrap | ConvertKind::Clamp if !src_is_int => Some(format!(
                "`{}` works between integer types, but `{}` is a float — a float conversion names its fraction policy: `to`, `round`, `floor` or `ceil`",
                kind.surface(), src
            )),
            ConvertKind::Wrap | ConvertKind::Clamp if !target_is_int => Some(format!(
                "`{}` produces an integer, but the target `{}` is not an integer type",
                kind.surface(), pc.target_name
            )),
            // CV15/CV16: float source, integer target.
            ConvertKind::Floor | ConvertKind::Ceil if !src_is_float => Some(format!(
                "`{}` rounds a float to an integer, but `{}` is not a float — for integer-to-integer use `to`, `wrap` or `clamp`",
                kind.surface(), src
            )),
            ConvertKind::Floor | ConvertKind::Ceil if !target_is_int => Some(format!(
                "`{}` produces an integer, but the target `{}` is not an integer type",
                kind.surface(), pc.target_name
            )),
            _ => None,
        };
        if let Some(message) = message {
            self.errors.push(TypeError::InvalidConvert { message, span: pc.span });
        }
    }

    /// CV11–CV16: reject a conversion method applied where its policy means
    /// nothing. `CheckedOption` has no surface form to reject.
    fn check_convert(&mut self, src: &Type, kind: ConvertKind, pc: &PendingCast) {
        if matches!(kind, ConvertKind::CheckedOption) {
            return;
        }
        self.check_convert_method(src, kind, pc)
    }

    /// V8: an index or count argument accepts any integer type. Used by the
    /// `vec.get(i)` method family so it matches `vec[i]`, which has always
    /// taken any integer. Pinning these to `i64` made the most ordinary loop
    /// in the language — `for i in 0..v.len() { v.get(i) }` — a type error,
    /// because `len()` answers `usize` (V9) and `usize` → `i64` is the one
    /// int→int pair CV1a refuses.
    ///
    /// Deferred like the bracket form so an unsuffixed literal is still a
    /// literal var when it's checked, and the runtime takes the index in an
    /// `int64_t` either way.
    pub(super) fn check_integer_arg(&mut self, container: &Type, index: &Type, span: Span) {
        self.pending_index.push(PendingIndex {
            container: container.clone(),
            index: index.clone(),
            kind: PendingIndexKind::Integer,
            span,
        });
    }

    /// #310: classify a container at an index site. A range on a non-sequence
    /// is rejected immediately; every other index is deferred to
    /// `validate_pending_index` so a literal index can adapt to the key/element
    /// type after constraint solving.
    pub(super) fn check_index_types(&mut self, container: &Type, index: &Type, is_range: bool, span: Span) {
        match self.classify_index_container(container) {
            Some(IndexContainer::Sequence) => {
                // A range index is a valid slice; a scalar index must be integer.
                if !is_range {
                    self.pending_index.push(PendingIndex {
                        container: container.clone(),
                        index: index.clone(),
                        kind: PendingIndexKind::Integer,
                        span,
                    });
                }
            }
            Some(IndexContainer::Map(key)) => {
                if is_range {
                    self.errors.push(TypeError::IndexTypeMismatch {
                        container: container.clone(),
                        found: index.clone(),
                        kind: IndexErrorKind::NotSliceable,
                        span,
                    });
                } else {
                    self.pending_index.push(PendingIndex {
                        container: container.clone(),
                        index: index.clone(),
                        kind: PendingIndexKind::MapKey(key),
                        span,
                    });
                }
            }
            Some(IndexContainer::Pool(elem)) => {
                if is_range {
                    self.errors.push(TypeError::IndexTypeMismatch {
                        container: container.clone(),
                        found: index.clone(),
                        kind: IndexErrorKind::NotSliceable,
                        span,
                    });
                } else {
                    self.pending_index.push(PendingIndex {
                        container: container.clone(),
                        index: index.clone(),
                        kind: PendingIndexKind::Handle(elem),
                        span,
                    });
                }
            }
            // Unknown / unresolved container, or `Handle<T>` itself — leave it.
            None => {}
        }
    }

    /// Recognize the indexable stdlib containers. Returns `None` for anything
    /// whose index type we don't police (user generics, type vars, `Handle`).
    fn classify_index_container(&self, ty: &Type) -> Option<IndexContainer> {
        match ty {
            Type::Array { .. } | Type::Slice(_) | Type::String => Some(IndexContainer::Sequence),
            Type::Generic { args, .. } | Type::UnresolvedGeneric { args, .. } => {
                match self.generic_base_name(ty)? {
                    "Vec" => Some(IndexContainer::Sequence),
                    "Map" => match args.first() {
                        Some(GenericArg::Type(k)) => Some(IndexContainer::Map((**k).clone())),
                        _ => None,
                    },
                    "Pool" => match args.first() {
                        Some(GenericArg::Type(t)) => Some(IndexContainer::Pool((**t).clone())),
                        _ => None,
                    },
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Name of a builtin generic container, matching by TypeId (resolved) or by
    /// spelling (unresolved).
    /// The element type a `for` loop takes out of `ty`.
    ///
    /// Every container the language can walk is named here. Anything else that
    /// has *resolved to a concrete type* is a program that says `for x in 3` —
    /// leaving that alone reported it as "couldn't work out the type of x",
    /// which blames the binding for the container's problem.
    pub(super) fn container_elem_type(&self, ty: &Type) -> ContainerElem {
        let arg = |n: usize| -> Option<Type> {
            let args = match ty {
                Type::UnresolvedGeneric { args, .. } | Type::Generic { args, .. } => args,
                _ => return None,
            };
            match args.get(n) {
                Some(GenericArg::Type(t)) => Some((**t).clone()),
                _ => None,
            }
        };
        match ty {
            Type::Array { elem, .. } | Type::Slice(elem) => {
                ContainerElem::Known((**elem).clone())
            }
            // Still open, a generic parameter, or already errored — the body
            // pins these, and an error here would land on working code.
            Type::Var(_) | Type::Error | Type::Never => ContainerElem::Deferred,
            // An adapted range (`(0..5).rev()`) still resolves to a bare `Range`,
            // which carries no element type, so the body's arithmetic pins the
            // width there.
            Type::UnresolvedNamed(_) => ContainerElem::Deferred,
            Type::Generic { .. } | Type::UnresolvedGeneric { .. } => {
                // `Iterator<T>` is what every `.iter().map(…)` chain resolves
                // to, so this is the common case, not an edge one.
                if matches!(ty, Type::UnresolvedGeneric { name, .. } if name == "Iterator") {
                    return arg(0).map_or(ContainerElem::Deferred, ContainerElem::Known);
                }
                // ctrl.ranges: a range's bounds share the loop variable's type.
                // The old bare `Range` carried none, so `for i in 1..6` left `i`
                // free — and `mut v = Vec.new()` filled by `v.push(i)` then had
                // no element type either, all the way down to `v[0]` (#620).
                if matches!(ty, Type::UnresolvedGeneric { name, .. } if name == "Range") {
                    return arg(0).map_or(ContainerElem::Deferred, ContainerElem::Known);
                }
                match self.generic_base_name(ty) {
                    Some("Vec") => arg(0).map_or(ContainerElem::Deferred, ContainerElem::Known),
                    // stdlib.collections: a map iterates its (key, value) entries.
                    Some("Map") => match (arg(0), arg(1)) {
                        (Some(k), Some(v)) => ContainerElem::Known(Type::Tuple(vec![k, v])),
                        _ => ContainerElem::Deferred,
                    },
                    // mem.pools/PF1: a pool iterates its handles, not its values.
                    Some("Pool") => match arg(0) {
                        Some(elem) => ContainerElem::Known(Type::UnresolvedGeneric {
                            name: "Handle".to_string(),
                            args: vec![GenericArg::Type(Box::new(elem))],
                        }),
                        None => ContainerElem::Deferred,
                    },
                    // A store iterates its links — the same shape as a pool
                    // iterating handles, minus the redemption step.
                    Some("Rack") => match arg(0) {
                        Some(node) => ContainerElem::Known(Type::UnresolvedGeneric {
                            name: "Link".to_string(),
                            args: vec![GenericArg::Type(Box::new(node))],
                        }),
                        None => ContainerElem::Deferred,
                    },
                    // A channel end isn't a sequence. There's no cursor to
                    // advance and no end to reach — you ask it for the next
                    // value and it tells you when the channel closed. `for v in
                    // rx` type-checked on the deferred branch below and then
                    // died in lowering with a type error about the element
                    // (#1067).
                    _ if matches!(
                        self.generic_name_of(ty).as_deref(),
                        Some("Receiver") | Some("Sender") | Some("Channel")
                    ) => ContainerElem::NotIterable,
                    // A user generic may implement the iterator protocol, and
                    // its element type isn't readable from here.
                    _ => ContainerElem::Deferred,
                }
            }
            _ => ContainerElem::NotIterable,
        }
    }

    /// The head name of a generic type, whatever spelling it's in.
    /// `generic_base_name` only answers for the six container names it knows.
    fn generic_name_of(&self, ty: &Type) -> Option<String> {
        match ty {
            Type::UnresolvedGeneric { name, .. } => Some(name.clone()),
            Type::Generic { base, .. } => Some(self.types.type_name(*base)),
            _ => None,
        }
    }

    pub(super) fn generic_base_name(&self, ty: &Type) -> Option<&'static str> {
        const NAMES: [&str; 6] = ["Vec", "Map", "Pool", "Handle", "Rack", "Link"];
        match ty {
            Type::UnresolvedGeneric { name, .. } => {
                NAMES.iter().copied().find(|n| *n == name)
            }
            Type::Generic { base, .. } => NAMES.iter().copied().find(|n| {
                self.types.get_type_id(n).map_or(false, |id| id == *base)
            }),
            _ => None,
        }
    }

    /// #310: validate deferred index sites. Runs after constraint solving but
    /// before literal defaults, so an unsuffixed literal index is still a
    /// literal var — it can adapt to an integer Map key instead of forcing i32.
    /// Judge every field/index write against its root binding, now that the
    /// root's type is resolved.
    ///
    /// Writing through a reference is not mutating the binding that holds it: a
    /// `Handle<T>` write lands in pool storage (mem.context/CC1) and a `Link<T>`
    /// write lands in the node, so a read-only binding is fine for both. Any
    /// other root gets the read-only-binding error.
    ///
    /// This runs after constraint solving because the answer depends on the
    /// root's type, and during the statement walk that type is often still a
    /// variable — a link bound by `if e.target? as t` comes from a deferred
    /// `HasField`, and a handle can arrive the same way.
    /// Re-ask "does this method mutate its receiver?" now that the receiver has
    /// a type. A method whose name happens to match some stdlib type's `mutate
    /// self` method is not one — `Handle.close(take self)` is a consume, and
    /// consuming a `let` binding is fine (SYNTAX.md, mem.parameters/PM2).
    fn validate_pending_self_mutations(&mut self) {
        let pending = std::mem::take(&mut self.pending_self_mutations);
        for pm in pending {
            let ty = self.resolve_named(&self.ctx.apply(&pm.ty));
            // Still open after solving: the receiver has its own diagnostic,
            // and guessing here would stack a wronger one on top.
            if matches!(ty, Type::Var(_) | Type::Error) {
                continue;
            }
            if !self.method_mutates_self_ty(&ty, &pm.method) {
                continue;
            }
            // PS2: a module-level `const` holding a sync box is package state
            // done right — the lock is what makes the write safe — so only a
            // bare one is an error.
            if matches!(pm.kind, super::BindingKind::ModuleConst) && self.is_sync_box(&ty) {
                continue;
            }
            let name = pm.root;
            let span = pm.span;
            match pm.kind {
                super::BindingKind::Let => {
                    self.errors.push(TypeError::MutateConst { name, span })
                }
                super::BindingKind::ModuleConst => {
                    let ty = self.render_type(&ty);
                    self.errors.push(TypeError::MutatePackageState { name, ty, span })
                }
                super::BindingKind::WithRead => {
                    self.errors.push(TypeError::MutateWithBinding { name, span })
                }
                super::BindingKind::Param => {
                    self.errors.push(TypeError::MutateReadOnlyParam { name, span })
                }
                super::BindingKind::Bound(from) => {
                    self.errors.push(TypeError::MutateBoundName { name, from, span })
                }
                _ => {}
            }
        }
    }

    /// PS2's sanctioned wrappers: the ones that carry their own synchronization,
    /// so a module-level `const` holding one is package state done right rather
    /// than package state got at. `Shared` covers `Shared.mutex` too — the
    /// strategy is a type argument, not a different type (mem.boxes/BX2).
    fn is_sync_box(&self, ty: &Type) -> bool {
        let name = match ty {
            Type::Named(id) | Type::Generic { base: id, .. } => self.types.type_name(*id),
            Type::UnresolvedNamed(n) => n.clone(),
            Type::UnresolvedGeneric { name, .. } => name.clone(),
            _ => return false,
        };
        let base = name.split('<').next().unwrap_or(&name);
        base.starts_with("Atomic")
            || matches!(base, "Shared" | "Mutex" | "Channel" | "Sender" | "Receiver")
    }

    pub(super) fn validate_pending_mutations(&mut self) {
        self.validate_pending_self_mutations();
        let pending = std::mem::take(&mut self.pending_mutations);
        for pm in pending {
            if matches!(pm.kind, super::BindingKind::Mut) {
                continue;
            }
            let ty = self.resolve_named(&self.ctx.apply(&pm.ty));
            if self.handle_element_type(&ty).is_some() || self.link_node_type(&ty).is_some() {
                continue;
            }
            // Still unknown after solving — stay quiet rather than guess. An
            // unresolved root has its own diagnostic; guessing here would stack
            // a second, wronger one on top.
            if matches!(ty, Type::Var(_) | Type::Error) {
                continue;
            }
            if matches!(pm.kind, super::BindingKind::ModuleConst) && self.is_sync_box(&ty) {
                continue;
            }
            let name = pm.root;
            let span = pm.span;
            match pm.kind {
                super::BindingKind::Let => {
                    self.errors.push(TypeError::MutateConst { name, span })
                }
                super::BindingKind::ModuleConst => {
                    let ty = self.render_type(&ty);
                    self.errors.push(TypeError::MutatePackageState { name, ty, span })
                }
                super::BindingKind::WithRead => {
                    self.errors.push(TypeError::MutateWithBinding { name, span })
                }
                super::BindingKind::Param => {
                    self.errors.push(TypeError::MutateReadOnlyParam { name, span })
                }
                super::BindingKind::Bound(from) => {
                    self.errors.push(TypeError::MutateBoundName { name, from, span })
                }
                super::BindingKind::Mut => {}
            }
        }
    }

    /// mem.pools/PF5: a write through a handle whose element type is backed by a
    /// `using frozen Pool<T>` context is rejected. Deferred alongside the
    /// read-only check for the same reason — it needs the handle's element type.
    pub(super) fn validate_pending_frozen_writes(&mut self) {
        let pending = std::mem::take(&mut self.pending_frozen_writes);
        for pfw in pending {
            let ty = self.resolve_named(&self.ctx.apply(&pfw.ty));
            let Some(elem) = self.handle_element_type(&ty) else { continue };
            if self.frozen_context_elems.iter().any(|e| *e == elem) {
                self.errors.push(TypeError::FrozenContextWrite {
                    op: "write".to_string(),
                    elem: self.fmt_ty(&elem),
                    span: pfw.span,
                });
            }
        }
    }

    pub(super) fn validate_pending_index(&mut self) {
        let pending = std::mem::take(&mut self.pending_index);
        for pi in pending {
            let container = self.ctx.apply(&pi.container);
            let index = self.ctx.apply(&pi.index);
            // Don't pile errors on an already-failed index type.
            if matches!(index, Type::Error) {
                continue;
            }
            match pi.kind {
                PendingIndexKind::Integer => {
                    match self.index_integerness(&index) {
                        Integerness::Yes | Integerness::Unknown => {}
                        Integerness::No => self.errors.push(TypeError::IndexTypeMismatch {
                            container,
                            found: index,
                            kind: IndexErrorKind::ExpectedInteger,
                            span: pi.span,
                        }),
                    }
                }
                PendingIndexKind::MapKey(key) => {
                    let key = self.ctx.apply(&key);
                    if !self.index_matches_key(&index, &key) {
                        self.errors.push(TypeError::IndexTypeMismatch {
                            container,
                            found: index,
                            kind: IndexErrorKind::ExpectedKey(key),
                            span: pi.span,
                        });
                    }
                }
                PendingIndexKind::Handle(elem) => {
                    let elem = self.ctx.apply(&elem);
                    // Skip only a genuinely-unresolved index; a scalar literal
                    // var is resolved enough to know it isn't a handle.
                    if let Type::Var(id) = index {
                        if !self.ctx.is_integer_literal_var(id)
                            && !self.ctx.is_float_literal_var(id)
                        {
                            continue;
                        }
                    }
                    if !self.index_is_matching_handle(&index, &elem) {
                        let expected = Type::UnresolvedGeneric {
                            name: "Handle".to_string(),
                            args: vec![GenericArg::Type(Box::new(elem))],
                        };
                        self.errors.push(TypeError::IndexTypeMismatch {
                            container,
                            found: index,
                            kind: IndexErrorKind::ExpectedHandle(expected),
                            span: pi.span,
                        });
                    }
                }
            }
        }
    }

    /// Classify an index type as integer / not / can't-tell. A still-unresolved
    /// literal-integer var counts as integer (it defaults to i32); a plain
    /// unresolved var is unknown (don't reject).
    fn index_integerness(&self, index: &Type) -> Integerness {
        match index {
            Type::Var(id) => {
                if self.ctx.is_integer_literal_var(*id) {
                    Integerness::Yes
                } else if self.ctx.is_float_literal_var(*id) {
                    Integerness::No
                } else {
                    Integerness::Unknown
                }
            }
            _ if Self::is_integer_type(index) => Integerness::Yes,
            _ => Integerness::No,
        }
    }

    /// True if `index` can serve as a `Map<K, V>` key.
    ///
    /// The match runs both ways. A literal-integer index adapts to an integer K
    /// (bound, so codegen sees K rather than the default); and an open K takes
    /// the index's type, because `mut m = Map.new()` has no other source for it
    /// — the key comes from the first `m["a"] = 1` and nothing else says what it
    /// is (#1026). Without that, K stayed a variable and every use of `m`
    /// inherited it.
    fn index_matches_key(&mut self, index: &Type, key: &Type) -> bool {
        if let Type::Var(id) = index {
            let id = *id;
            // A literal index adapts to a compatible scalar key, binding the var
            // so the key type flows to codegen instead of defaulting.
            if self.ctx.is_integer_literal_var(id) {
                if Self::is_integer_type(key) {
                    self.ctx.bind_var(id, key.clone());
                    return true;
                }
                // Both open: make them one variable so the literal default
                // settles the key too.
                if let Type::Var(k) = key {
                    if *k != id {
                        self.ctx.bind_var(*k, index.clone());
                    }
                    return true;
                }
                return false;
            }
            if self.ctx.is_float_literal_var(id) {
                if Self::is_float_type(key) {
                    self.ctx.bind_var(id, key.clone());
                    return true;
                }
                if let Type::Var(k) = key {
                    if *k != id {
                        self.ctx.bind_var(*k, index.clone());
                    }
                    return true;
                }
                return false;
            }
            return true; // genuinely unresolved index — don't guess
        }
        if matches!(index, Type::Error) {
            return true;
        }
        if let Type::Var(k) = key {
            if !self.ctx.occurs_in(*k, index) {
                self.ctx.bind_var(*k, index.clone());
            }
            return true;
        }
        if matches!(key, Type::Error) {
            return true;
        }
        // A tuple key element by element, so the literal vars inside `m[(1, 2)]`
        // adapt to the key's widths the same way a bare `m[1]` does. Comparing
        // the two whole types instead rejected the index against its own key
        // type — the error said `cannot index Map<(i32, i32), string> with
        // (i32, i32)`, which reads like a compiler bug because it is one.
        if let (Type::Tuple(ix), Type::Tuple(ky)) = (index, key) {
            return ix.len() == ky.len()
                && ix.iter().zip(ky.iter()).all(|(i, k)| {
                    let (i, k) = (self.ctx.apply(i), self.ctx.apply(k));
                    self.index_matches_key(&i, &k)
                });
        }
        self.types.resolve_type_names(index) == self.types.resolve_type_names(key)
    }

    /// True if `index` is a `Handle<U>` whose `U` matches the pool's element
    /// type. Cross-pool handles of the same element type aren't statically
    /// distinguishable (that's the runtime pool_id check), so accept them;
    /// only a statically-wrong element type is rejected.
    fn index_is_matching_handle(&self, index: &Type, pool_elem: &Type) -> bool {
        let handle_arg = match index {
            Type::UnresolvedGeneric { name, args } if name == "Handle" => args.first(),
            Type::Generic { base, args }
                if self.types.get_type_id("Handle").map_or(false, |id| id == *base) =>
            {
                args.first()
            }
            _ => return false,
        };
        let Some(GenericArg::Type(u)) = handle_arg else {
            return true; // bare `Handle` — nothing to compare
        };
        let u = self.ctx.apply(u);
        // Unresolved on either side — don't reject.
        if matches!(u, Type::Var(_) | Type::Error) || matches!(pool_elem, Type::Var(_) | Type::Error)
        {
            return true;
        }
        self.types.resolve_type_names(&u) == self.types.resolve_type_names(pool_elem)
    }
}

/// What `container_elem_type` could work out about a `for` loop's source.
pub(super) enum ContainerElem {
    /// The element type, read off the container.
    Known(Type),
    /// A container whose element type isn't readable here but which the body
    /// legitimately pins — a bare `Range`, a type variable, a user generic that
    /// may implement the iterator protocol.
    Deferred,
    /// Resolved to something no `for` loop can walk.
    NotIterable,
}

/// Indexable container class at an index site (#310).
enum IndexContainer {
    /// Vec, array, slice, string — position-indexed by an integer.
    Sequence,
    /// `Map<K, V>` — indexed by `K` (carried).
    Map(Type),
    /// `Pool<T>` — indexed by `Handle<T>` (T carried).
    Pool(Type),
}

/// An index site validated after inference finalizes (mod.rs).
pub(super) struct PendingIndex {
    pub container: Type,
    pub index: Type,
    pub kind: PendingIndexKind,
    pub span: Span,
}

pub(super) enum PendingIndexKind {
    /// Sequence index — must be an integer type.
    Integer,
    /// Map index — must match the carried key type `K`.
    MapKey(Type),
    /// Pool index — must be `Handle<T>` for the carried element type `T`.
    Handle(Type),
}

/// Whether an index type is an integer (#310).
enum Integerness {
    Yes,
    No,
    /// Unresolved — can't tell, don't reject.
    Unknown,
}

/// A cast/convert site validated after inference finalizes (mod.rs).
pub(super) struct PendingCast {
    pub source: Type,
    pub target: Type,
    /// Original target spelling (`usize`, `i8`, …) for the suggested fix.
    pub target_name: String,
    /// `None` = `as` cast; `Some` = explicit conversion form.
    pub convert: Option<ConvertKind>,
    pub span: Span,
}

/// Primitive scalar classification for conversion rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prim {
    Int { bits: u32, signed: bool },
    Float { bits: u32 },
    Bool,
    Char,
}

fn prim_of(ty: &Type) -> Option<Prim> {
    Some(match ty {
        Type::I8 => Prim::Int { bits: 8, signed: true },
        Type::I16 => Prim::Int { bits: 16, signed: true },
        Type::I32 => Prim::Int { bits: 32, signed: true },
        Type::I64 => Prim::Int { bits: 64, signed: true },
        Type::I128 => Prim::Int { bits: 128, signed: true },
        Type::U8 => Prim::Int { bits: 8, signed: false },
        Type::U16 => Prim::Int { bits: 16, signed: false },
        Type::U32 => Prim::Int { bits: 32, signed: false },
        Type::U64 => Prim::Int { bits: 64, signed: false },
        Type::U128 => Prim::Int { bits: 128, signed: false },
        Type::F32 => Prim::Float { bits: 32 },
        Type::F64 => Prim::Float { bits: 64 },
        Type::Bool => Prim::Bool,
        Type::Char => Prim::Char,
        _ => return None,
    })
}

/// Inclusive range of an integer type. `None` for anything that isn't one —
/// a float target takes any literal, and an unresolved type has nothing to say.
/// i128/u128 are checked as i128, which can't represent all of u128; the top of
/// that range needs 128-bit literals to reach anyway.
/// The inclusive range of an integer type. The top is a `u128` because
/// `u128::MAX` is the one bound no signed type can hold.
fn int_range(ty: &Type) -> Option<(i128, u128)> {
    Some(match prim_of(ty)? {
        Prim::Int { bits: 128, signed: false } => (0, u128::MAX),
        Prim::Int { bits: 128, signed: true } => (i128::MIN, i128::MAX as u128),
        Prim::Int { bits, signed: true } => (-(1i128 << (bits - 1)), (1u128 << (bits - 1)) - 1),
        Prim::Int { bits, signed: false } => (0, (1u128 << bits) - 1),
        _ => return None,
    })
}

/// The primitive scalars `as`/conversion forms operate on.
fn is_scalar_ty(ty: &Type) -> bool {
    prim_of(ty).is_some()
}

/// CV1: is this `as` cast lossless widening?
///
/// - int→int: value-preserving widening (wider, and same-signed or unsigned source).
/// - int→float: allowed (spec's own examples use `as`; only float→int is blocked).
/// - float→float: widening only (f32→f64).
/// - char→int: lossless when the target holds a full Unicode scalar (≥32 bits) [CH4].
fn as_cast_is_lossless(src_ty: &Type, tgt_ty: &Type, s: Prim, t: Prim) -> bool {
    if src_ty == tgt_ty {
        return true;
    }
    match (s, t) {
        (Prim::Int { bits: sb, signed: ss }, Prim::Int { bits: tb, signed: ts }) => {
            tb > sb && (ss == ts || !ss)
        }
        (Prim::Int { .. }, Prim::Float { .. }) => true,
        (Prim::Float { bits: sb }, Prim::Float { bits: tb }) => tb >= sb,
        (Prim::Char, Prim::Int { bits, .. }) => bits >= 32,
        _ => false,
    }
}

fn classify_invalid_cast(s: Prim, t: Prim) -> InvalidCastClass {
    match (s, t) {
        (Prim::Bool, _) | (_, Prim::Bool) => InvalidCastClass::Bool,
        (Prim::Int { .. }, Prim::Char) => InvalidCastClass::IntToChar,
        (Prim::Float { .. }, Prim::Int { .. }) => InvalidCastClass::FloatToInt,
        (Prim::Float { .. }, Prim::Float { .. }) => InvalidCastClass::FloatNarrowing,
        (Prim::Int { signed: ss, .. }, Prim::Int { signed: ts, .. }) => {
            if ss && !ts {
                InvalidCastClass::SignReinterpret
            } else {
                InvalidCastClass::Narrowing
            }
        }
        _ => InvalidCastClass::Other,
    }
}

/// Does this closure body hand a value out through a `return`?
///
/// A body's type is `Never` both when it panics and when it ends in `return x`
/// — control doesn't fall off the end either way. Only the first means no value
/// ever comes out, and that's the one whose return type nothing constrains.
/// Nested closures don't count: their `return` returns from them.
fn body_returns_a_value(body: &Expr) -> bool {
    fn in_stmts(stmts: &[Stmt]) -> bool {
        stmts.iter().any(in_stmt)
    }
    fn in_stmt(stmt: &Stmt) -> bool {
        match &stmt.kind {
            StmtKind::Return(Some(_)) => true,
            StmtKind::Expr(e) => in_expr(e),
            StmtKind::Let { init, .. } => in_expr(init),
            StmtKind::While { body, .. }
            | StmtKind::WhileLet { body, .. }
            | StmtKind::Loop { body, .. }
            | StmtKind::For { body, .. }
            | StmtKind::Comptime(body) => in_stmts(body),
            StmtKind::Ensure { body, else_handler } => {
                in_stmts(body)
                    || else_handler.as_ref().map_or(false, |(_, h)| in_stmts(h))
            }
            _ => false,
        }
    }
    fn in_expr(expr: &Expr) -> bool {
        match &expr.kind {
            // A nested closure's `return` is its own.
            ExprKind::Closure { .. } => false,
            ExprKind::Block(body) | ExprKind::Loop { body, .. } | ExprKind::Spawn { body } => {
                in_stmts(body)
            }
            ExprKind::If { then_branch, else_branch, .. } => {
                in_expr(then_branch)
                    || else_branch.as_ref().map_or(false, |e| in_expr(e))
            }
            ExprKind::Match { arms, .. } => arms.iter().any(|a| in_expr(&a.body)),
            _ => false,
        }
    }
    in_expr(body)
}
