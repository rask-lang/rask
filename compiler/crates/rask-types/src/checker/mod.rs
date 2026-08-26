// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Type checker implementation.

use std::collections::HashMap;

use rask_ast::decl::Decl;
use rask_ast::NodeId;
use rask_resolve::{ResolvedProgram, SymbolId};

use crate::types::Type;

mod type_defs;
mod builtins;
mod type_table;
mod inference;
mod errors;
mod parse_type;
mod borrow;
mod declarations;
mod annotations;
mod check_pattern;
mod check_fn;
mod check_stmt;
mod check_expr;
mod unify;
mod generics;
mod resolve;
mod validate;
mod resolved_types;

pub use type_defs::{Callee, ErrorWrap, TypeDef, MethodSig, SelfParam, ParamMode, TypedProgram, receiver_name};
pub use type_table::TypeTable;
pub use inference::{TypeConstraint, InferenceContext};
pub use errors::{TypeError, MapKeyFix, InvalidCastClass, IndexErrorKind, TraitBoundContext};
pub use parse_type::parse_type_string;
pub use declarations::{signature_type_param_names, struct_type_param_names, enum_type_param_names};

use borrow::{ActiveBorrow, PersistentBorrow};

/// Binding mutability and origin — used to pick the right error message
/// when a read-only binding is mutated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingKind {
    /// `mut x` — rebindable, mutable
    Mut,
    /// `let x` — deep-immutable local binding
    Let,
    /// Default parameter (read-only; use `mutate` to allow mutation)
    Param,
    /// `with expr as v` — read-only with-binding (use `as mut v` to mutate)
    WithRead,
    /// A name a test or a pattern introduced, not a declaration: `if x? as v`
    /// (OPT19 binds a let), a match-arm payload, `catch e =>`, a `for` element
    /// in the default read-only mode (std.iteration/I1). Read-only like `let`,
    /// but there is no `let` to change — the fix depends on where it came from,
    /// which is what `Bound` carries.
    Bound(BoundFrom),
}

/// Where a `Bound` name came from, so the mutation error can name the remedy
/// that exists for that form instead of "add `mut`", which none of them accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundFrom {
    /// `if x? as v`, `r is E as e`, a match-arm payload, `catch e =>`,
    /// `while q.pop()? as item`. The name is the payload the test proved
    /// present, read out of the scrutinee — writing to it can't reach back.
    Payload,
    /// A `for` element in the default mode. `for mutate x in xs` is the form
    /// that writes back (std.iteration/I1, I4).
    Element,
}

/// A deferred read-only-binding check. See `pending_mutations`.
#[derive(Debug, Clone)]
pub struct PendingMutation {
    pub root: String,
    pub ty: crate::types::Type,
    pub kind: BindingKind,
    pub span: rask_ast::Span,
}

/// A deferred frozen-context write check. See `pending_frozen_writes`.
#[derive(Debug, Clone)]
pub struct PendingFrozenWrite {
    pub ty: crate::types::Type,
    pub span: rask_ast::Span,
}

impl BindingKind {
    pub fn is_read_only(&self) -> bool {
        matches!(
            self,
            BindingKind::Let
                | BindingKind::Param
                | BindingKind::WithRead
                | BindingKind::Bound(_)
        )
    }
}

/// Classification of unsafe operations for auditing and tooling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnsafeCategory {
    PointerDeref,
    PointerDerefWrite,
    PointerArithmetic,
    PointerMethod,
    ExternCall,
    UnsafeFuncCall,
    Transmute,
    UnionFieldAccess,
    UncheckedArith,
}

pub struct TypeChecker {
    /// Symbol table from resolution.
    pub(super) resolved: ResolvedProgram,
    /// Type registry.
    pub(super) types: TypeTable,
    /// Inference state.
    pub(super) ctx: InferenceContext,
    /// Types assigned to nodes.
    pub(super) node_types: HashMap<NodeId, Type>,
    /// Types assigned to symbols (for bindings without annotations).
    pub(super) symbol_types: HashMap<SymbolId, Type>,
    /// Collected errors.
    pub(super) errors: Vec<TypeError>,
    /// Current function's return type (for checking return statements).
    pub(super) current_return_type: Option<Type>,
    /// Result type of each enclosing loop-as-expression, innermost last. A
    /// `break v` unifies `v` with the top of this; without it the loop's type
    /// was a fresh variable nothing ever wrote to, so `let found = loop { … }`
    /// stayed open however the breaks were typed.
    pub(super) loop_value_types: Vec<Type>,
    /// Current Self type (inside extend blocks).
    pub(super) current_self_type: Option<Type>,
    /// Trait bounds on the current function's type params (name → trait names).
    /// Lets `g.greet()` resolve against `T: Greeter` for static dispatch (#314).
    pub(super) current_type_param_bounds: HashMap<String, Vec<String>>,
    /// Trait bounds from the enclosing `extend Foo<T> where T: Trait { }`
    /// block's own where-clause, distinct from a method's own bounds (those
    /// live on the method's `FnDecl` and are folded into
    /// `current_type_param_bounds` directly). A bound declared at the extend
    /// level covers every method in the block, so `check_fn` seeds
    /// `current_type_param_bounds` from this before layering the method's own
    /// bounds on top (#838).
    pub(super) current_impl_type_param_bounds: HashMap<String, Vec<String>>,
    /// Scope stack for local variable types (innermost scope last).
    /// Tuple: (type, binding kind). Const bindings and default params are read-only.
    pub(super) local_types: Vec<HashMap<String, (Type, BindingKind)>>,
    /// Active borrows for aliasing detection (ESAD Phase 1).
    pub(super) borrow_stack: Vec<ActiveBorrow>,
    /// Persistent borrows across statements within a scope (ESAD Phase 2).
    pub(super) persistent_borrows: Vec<PersistentBorrow>,
    /// Pending generic call sites: (call NodeId, fresh type vars for type params).
    /// Resolved after constraint solving to populate TypedProgram.call_type_args.
    pub(super) pending_call_type_args: Vec<(NodeId, Vec<Type>)>,
    /// CALL6: the function each call resolves to, keyed by the call's NodeId.
    /// Free calls record here immediately; method calls record when their
    /// `HasMethod` constraint resolves.
    pub(super) call_targets: HashMap<NodeId, type_defs::Callee>,
    /// SymbolId → type param names for generic functions.
    /// Keyed by SymbolId (not name) to avoid collisions between
    /// same-named functions in different scopes.
    pub(super) fn_type_params: HashMap<SymbolId, Vec<String>>,
    /// SymbolId → (type param name → trait bounds) for generic functions.
    /// Used to check bound satisfaction at call sites (#314).
    pub(super) fn_type_param_bounds: HashMap<SymbolId, HashMap<String, Vec<String>>>,
    /// Names declared as annotations (type.annotations). Registered as struct
    /// types for `has<A>()` name resolution, but comptime-only: runtime
    /// construction is rejected.
    pub(super) annotation_types: std::collections::HashSet<String>,
    /// Call-site bound obligations: (type-arg var, bound trait names, span).
    /// Verified after constraint solving resolves the var to a concrete type.
    pub(super) pending_bound_checks: Vec<(Type, Vec<String>, rask_ast::Span)>,
    /// ER3a: call-site disjointness obligations read off the callee's signature.
    /// Verified after constraint solving resolves the type-arg vars.
    pub(super) pending_disjointness: Vec<validate::DisjointObligation>,
    /// Whether we're inside an `unsafe {}` block (for validating pointer ops and extern calls).
    pub(super) in_unsafe: bool,
    /// Collected unsafe operations with their locations (for tooling/auditing).
    pub(super) unsafe_ops: Vec<(rask_ast::Span, UnsafeCategory)>,
    /// Was this call site inside `unsafe`? Recorded for calls that *name* a
    /// pointer method but whose receiver type isn't known yet. The constraint
    /// solver resolves those later, long after `in_unsafe` has been unwound, so
    /// it reads the flag from here instead of from the walk.
    pub(super) ptr_method_sites: std::collections::HashMap<rask_ast::Span, bool>,
    /// Whether we're inferring an assignment target (union field writes are safe per UN3).
    pub(super) in_assign_target: bool,
    /// Whether we're inferring an expression in statement position (value discarded).
    /// Suppresses branch-type agreement for if/else and match.
    pub(super) in_stmt_expr: bool,
    /// GC1/GC2: Pre-created type vars for functions with inferred params/return.
    /// Key is function name, value is (param_type_vars, return_type_var).
    pub(super) inferred_fn_types: HashMap<String, (Vec<(String, Type)>, Type)>,
    /// TR5: implicit trait coercion sites. NodeId of expression → trait name.
    /// MIR lowering uses this to emit TraitBox instructions at coercion sites.
    pub(super) trait_coercions: HashMap<NodeId, String>,
    /// ER31a: `try` sites where the propagated error gets wrapped in a variant
    /// of the enclosing function's error enum. NodeId of the `try` expression →
    /// the wrapping variant. Both backends read this to build the enum value.
    pub(super) error_wraps: HashMap<NodeId, type_defs::ErrorWrap>,
    /// ER31a: `try` sites whose source error type wasn't known yet and whose
    /// target error enum could wrap it. Settled after constraint solving.
    /// (`try` node, source error, target error, span).
    pub(super) pending_try_errors: Vec<(NodeId, Type, Type, rask_ast::Span)>,
    /// ER14a: `??` sites whose right side is still wrapped, so the whole
    /// expression keeps the optional shape. Both backends read this to know
    /// whether the present path yields the payload or the operand as-is.
    pub(super) fallback_keeps_shape: std::collections::HashSet<NodeId>,
    /// ER16b: `try` nodes that are the left half of a `try … ??` composite.
    /// Only there may a `try` take a flat `T? or E` operand (ER47).
    pub(super) flat_try_sites: std::collections::HashSet<NodeId>,
    /// ER16a: steps of the postfix chain currently under a `try`, excluding the
    /// outermost. The first one to come back `T or E` is where the `try`
    /// attaches, and the rest of the chain sees its payload.
    pub(super) try_chain_steps: std::collections::HashSet<NodeId>,
    /// ER16a: the step that took the `try`, with the error type it propagates.
    /// Set while a chain is being inferred, taken by the `try` node.
    pub(super) try_chain_unwrapped: Option<(NodeId, Type)>,
    /// ER16a: `try` node → the chain step it attaches to, when that isn't the
    /// operand itself. Lowering reads this to put the branch in the same place
    /// the checker did.
    pub(super) try_chain_placement: HashMap<NodeId, NodeId>,
    /// `??` nodes whose left side is an index expression. `m[k]` panics on a
    /// miss instead of yielding a `T?`, so a `??` after it is the mistake
    /// people actually make, and the fix is `.get(k)` rather than anything
    /// about the `??` itself (#645). Recorded during the walk because by the
    /// time the operand's type settles, the syntax is out of reach.
    pub(super) coalesce_index_operands: std::collections::HashSet<NodeId>,
    /// ER20: Collected error types from `try` calls in error-accumulation mode.
    pub(super) inferred_errors: Vec<Type>,
    /// ER20: Whether we're collecting errors instead of unifying them.
    pub(super) accumulate_errors: bool,
    /// Types for binding names and parameters, keyed by (span.start, span.end).
    pub(super) span_types: HashMap<(usize, usize, u16), Type>,
    /// GC9: methods whose `self` is mutable — declared `mutate self`/`take self`,
    /// or a private method the inference decided writes through self. Keyed by
    /// the method's span so lowering can look up the answer instead of working
    /// the rule out a second time.
    pub(super) mutate_self_fns: std::collections::HashSet<(usize, usize, u16)>,
    /// D1: Bindings invalidated by `discard`. Maps name → discard span.
    pub(super) discarded_bindings: HashMap<String, rask_ast::Span>,
    /// CC1: nesting depth of `using Multitasking { }` blocks in current function.
    pub(super) multitasking_depth: u32,
    /// CV1–CV10: cast/convert sites validated after literal defaults resolve
    /// their source types. Deferred so `1 as bool` sees `i32`, not a fresh var.
    pub(super) pending_casts: Vec<check_expr::PendingCast>,
    /// #310: index sites validated after literal defaults resolve their index
    /// type. Deferred so `v[0]` sees `i32`, not a fresh literal var.
    pub(super) pending_index: Vec<check_expr::PendingIndex>,
    /// Field/index writes whose root type was still a variable when the
    /// statement was walked. Judged in `validate_pending_mutations`, so a write
    /// through a `Handle`/`Link` bound by optional narrowing is recognised for
    /// what it is instead of being reported as mutating a `let`.
    pub(super) pending_mutations: Vec<PendingMutation>,
    /// Every use of a name that might turn out to be a `Shared`, with its type
    /// and span. Checked after constraint solving for SH7 — during the walk the
    /// type is usually still a variable, since `let c = Shared.new(0)` is solved
    /// later.
    pub(super) local_shared_uses: Vec<(String, Type, rask_ast::Span)>,
    /// The argument spans of every `spawn` call seen. A use inside one of these
    /// is a use in another task.
    pub(super) spawn_arg_spans: Vec<rask_ast::Span>,
    /// mem.pools/PF5 frozen-context write sites, deferred for the same reason as
    /// `pending_mutations` — the check needs the handle's element type.
    pub(super) pending_frozen_writes: Vec<PendingFrozenWrite>,
    /// Every integer literal, checked against its final type once solving is
    /// done. Deferred because the type is usually a var at the point the literal
    /// is seen. (value, whether the text was above `i64::MAX`, type, span).
    pub(super) pending_int_literals: Vec<(i128, bool, Type, rask_ast::Span)>,
    /// Method calls whose receiver was still an inference variable when solving
    /// finished — retried after literal defaults land (`retry_deferred_methods`).
    pub(super) deferred_methods: Vec<TypeConstraint>,
    /// RC1/RC3: container-typed sites (bindings, params, returns, fields, alias
    /// targets) validated after solving, so an inferred `Vec.new()` element that
    /// unifies to a resource is caught. (Span, container type).
    pub(super) pending_linear_containers: Vec<(rask_ast::Span, Type)>,
    /// T1: method-call spans that resolved to a channel `Sender.send`. The
    /// ownership checker reads these to consume the sent value (`mem.ownership/T1`),
    /// even when inference leaves the receiver as a bare type variable in
    /// `node_types` (deferred `Sender.send` resolution).
    pub(super) channel_send_sites: std::collections::HashSet<rask_ast::Span>,
    /// ER3/ER4: `T or E` sites in type declarations (struct/enum/union/alias),
    /// validated after `register_impl_methods` so an error type whose `message()`
    /// comes from an `extend` block is recognized regardless of declaration order.
    pub(super) pending_result_validations: Vec<(Type, rask_ast::Span)>,
    /// mem.pools/PF5: element types `T` of the current function's
    /// `using frozen Pool<T>` clauses. A write through a `Handle<T>`
    /// (`h.field = v`) in such a context is rejected. Structural ops
    /// (insert/remove/clear on the named binding) are caught by the effects
    /// analysis (`rask-effects`), so they aren't re-checked here.
    pub(super) frozen_context_elems: Vec<Type>,
    /// S2: view bindings (`s[i..]`, `s.trim()`) whose source type was still a
    /// variable during the walk — a field, a loop variable, an inferred local.
    /// Validated after solving, so `const q = self.url[i..]` is caught too.
    pub(super) pending_view_bindings: Vec<borrow::PendingViewBinding>,
    /// ER14a: `catch` bodies that yield nothing while the scrutinee's success
    /// type is still open. Unifying the guess eagerly let whichever such catch
    /// ran first decide the type, so a later, better-typed use of the same
    /// value failed instead of the catch that's actually wrong (#876).
    /// Checked once solving (and literal defaulting) is done, against
    /// whatever the success type actually settled to.
    pub(super) pending_catch_void_checks: Vec<(Type, rask_ast::Span)>,
}

impl TypeChecker {
    /// Record that `value` is going into a position declared as `target`, and
    /// may need `T?` / `T or E` layers added on the way in.
    ///
    /// **Every position that coerces goes through here.** The rule itself lives
    /// in one place (`resolve_coercion`) keyed by one enum (`CoercionSite`,
    /// shared with MIR lowering). Before this, each position decided for itself
    /// whether to widen or to plain-unify, so the same rule got a different
    /// answer depending on which code path reached it — `f(2)` widened a bare
    /// `2` into an `i64?` parameter and `w.m(2)` rejected it (#701).
    ///
    /// The decision is deferred: whether a value needs wrapping depends on the
    /// target's shape, and at an argument or a field the target is often still
    /// an inference variable when the position is first visited.
    pub(super) fn coerce_into(
        &mut self,
        site: rask_ast::coercion::CoercionSite,
        value: Type,
        target: Type,
        span: rask_ast::Span,
    ) {
        self.coerce_into_node(site, value, target, None, span)
    }

    /// `coerce_into`, naming the expression being coerced.
    ///
    /// Worth the extra argument only where the decision has to reach a backend:
    /// ER32's error branch erases a concrete error into `any Trait`, and MIR
    /// boxes at the value, keyed by its node.
    pub(super) fn coerce_into_node(
        &mut self,
        site: rask_ast::coercion::CoercionSite,
        value: Type,
        target: Type,
        value_node: Option<NodeId>,
        span: rask_ast::Span,
    ) {
        self.ctx.add_constraint(inference::TypeConstraint::Coerce {
            value,
            target,
            site,
            value_node,
            span,
        });
    }

    /// Create a new type checker.
    pub fn new(resolved: ResolvedProgram) -> Self {
        Self {
            resolved,
            types: TypeTable::new(),
            ctx: InferenceContext::new(),
            node_types: HashMap::new(),
            symbol_types: HashMap::new(),
            errors: Vec::new(),
            current_return_type: None,
            loop_value_types: Vec::new(),
            current_self_type: None,
            current_type_param_bounds: HashMap::new(),
            current_impl_type_param_bounds: HashMap::new(),
            local_types: Vec::new(),
            borrow_stack: Vec::new(),
            persistent_borrows: Vec::new(),
            pending_call_type_args: Vec::new(),
            call_targets: HashMap::new(),
            fn_type_params: HashMap::new(),
            fn_type_param_bounds: HashMap::new(),
            annotation_types: std::collections::HashSet::new(),
            pending_bound_checks: Vec::new(),
            pending_disjointness: Vec::new(),
            in_unsafe: false,
            unsafe_ops: Vec::new(),
            ptr_method_sites: HashMap::new(),
            inferred_fn_types: HashMap::new(),
            in_assign_target: false,
            in_stmt_expr: false,
            trait_coercions: HashMap::new(),
            error_wraps: HashMap::new(),
            pending_try_errors: Vec::new(),
            fallback_keeps_shape: std::collections::HashSet::new(),
            flat_try_sites: std::collections::HashSet::new(),
            try_chain_steps: std::collections::HashSet::new(),
            try_chain_unwrapped: None,
            try_chain_placement: HashMap::new(),
            coalesce_index_operands: std::collections::HashSet::new(),
            inferred_errors: Vec::new(),
            span_types: HashMap::new(),
            mutate_self_fns: std::collections::HashSet::new(),
            accumulate_errors: false,
            discarded_bindings: HashMap::new(),
            multitasking_depth: 0,
            pending_casts: Vec::new(),
            pending_int_literals: Vec::new(),
            deferred_methods: Vec::new(),
            pending_index: Vec::new(),
            pending_mutations: Vec::new(),
            local_shared_uses: Vec::new(),
            spawn_arg_spans: Vec::new(),
            pending_frozen_writes: Vec::new(),
            pending_linear_containers: Vec::new(),
            pending_view_bindings: Vec::new(),
            channel_send_sites: std::collections::HashSet::new(),
            pending_result_validations: Vec::new(),
            frozen_context_elems: Vec::new(),
            pending_catch_void_checks: Vec::new(),
        }
    }

    pub fn check(self, decls: &[Decl]) -> Result<TypedProgram, Vec<TypeError>> {
        let (program, errors) = self.check_lenient(decls);
        if errors.is_empty() {
            Ok(program)
        } else {
            Err(errors)
        }
    }

    /// Declarations that carry a body to check, as opposed to ones that only
    /// introduce a name or a value. Bodies are checked in a second pass so
    /// they see module-level consts already typed.
    fn is_body_decl(decl: &Decl) -> bool {
        use rask_ast::decl::DeclKind;
        matches!(
            decl.kind,
            DeclKind::Fn(_) | DeclKind::Impl(_) | DeclKind::Test(_) | DeclKind::Benchmark(_)
        )
    }

    /// Lenient variant: always returns the (partial) TypedProgram plus any errors.
    ///
    /// The TypedProgram is usable even when errors exist — node_types contains
    /// types for every expression that was successfully inferred. Callers can
    /// run ownership/effects analysis on the partial program to collect more
    /// diagnostics in a single pipeline pass.
    pub fn check_lenient(self, decls: &[Decl]) -> (TypedProgram, Vec<TypeError>) {
        self.check_lenient_with_stdlib(&[], decls)
    }

    /// Check stdlib bodies and the program together, each in its own scope.
    ///
    /// Two phases rather than one concatenated list, because a name can mean
    /// different types on either side: a program `struct Headers` shadows the
    /// stdlib's, and the stdlib's own body still has to mean its own (#515).
    /// The resolver sequences its two sets the same way.
    pub fn check_lenient_with_stdlib(
        mut self,
        stdlib_decls: &[Decl],
        decls: &[Decl],
    ) -> (TypedProgram, Vec<TypeError>) {
        // Stdlib types register first so a program declaration of the same name
        // shadows rather than overwrites.
        if !stdlib_decls.is_empty() {
            self.types.stdlib_mode = true;
            self.collect_type_declarations(stdlib_decls);
            self.types.stdlib_mode = false;
        }
        self.collect_type_declarations(decls);
        self.check_user_annotations(decls);

        // Global scope for module-level bindings (imports, etc.)
        self.push_scope();

        if !stdlib_decls.is_empty() {
            self.types.stdlib_mode = true;
            for decl in stdlib_decls {
                self.check_decl(decl);
            }
            self.types.stdlib_mode = false;
        }

        // Everything that isn't a body first — imports, module-level consts, type
        // aliases — then the bodies. A body that reads a module-level const needs
        // its type, and files arrive in whatever order the package lists them:
        // `main.rk` before `store.rk` meant a body was checked while
        // `const store = Mutex.new(Store.new())` was still an inference variable,
        // so `store.lock()` had no receiver type to dispatch on and the method's
        // result type was lost — silently, and only for consts without an
        // annotation (#566, #569). Imports keep their place ahead of the consts
        // that use them (`const started = time.Instant.now()`).
        for decl in decls {
            if !Self::is_body_decl(decl) {
                self.check_decl(decl);
            }
        }
        for decl in decls {
            if Self::is_body_decl(decl) {
                self.check_decl(decl);
            }
        }
        self.pop_scope();

        self.solve_constraints();

        // ER31a: `try` sites whose source error type only became concrete here
        // (a method call's signature, say) pick widen-or-wrap now.
        self.resolve_pending_try_wraps();
        self.solve_constraints();

        // #310: validate index expression types (integer for Vec/slice/string,
        // K for Map, Handle<T> for Pool) BEFORE literal defaults land, so a
        // literal index can adapt to an integer Map key instead of forcing i32.
        self.validate_pending_index();
        self.validate_pending_mutations();
        self.validate_spawn_captures();
        self.validate_pending_frozen_writes();

        // #314: verify generic call type args satisfy their declared bounds.
        self.validate_pending_bound_checks();

        // ER3a: verify no `T or E` in a callee's signature collapsed to `E or E`
        // once the type args are known.
        self.validate_pending_disjointness();

        // Default unresolved literal type vars (unsuffixed int → i32, float → f64)
        self.ctx.apply_literal_defaults();

        // A method call that deferred on an unsuffixed literal receiver can be
        // resolved now that the literal has a type.
        self.retry_deferred_methods();

        // An integer literal has to fit the type it landed in.
        self.validate_pending_int_literals();

        // ER14a: a void-bodied `catch` whose scrutinee's success type was still
        // open when it ran — check it against whatever that type settled to
        // (from another catch, a `try`, literal defaults, anything else),
        // instead of having decided it on the spot.
        self.validate_pending_catch_void_checks();

        // CV1–CV10: validate casts/conversions now that literal source types
        // are concrete (e.g. `1 as bool` sees `i32`).
        self.validate_pending_casts();

        // RC1/RC3: reject Vec/Map holding linear elements now that inferred
        // element types (`Vec.new()` + `push`, `collect`, generic returns) are
        // concrete.
        self.validate_pending_linear_containers();

        // S2: view bindings whose source was a field or loop variable — the type
        // is concrete now, so "is this a string slice / a growable view" has an
        // answer it didn't have during the walk.
        self.validate_pending_view_bindings();

        let node_types: HashMap<_, _> = self
            .node_types
            .iter()
            .map(|(id, ty)| (*id, self.ctx.apply(ty)))
            .collect();

        self.validate_resolved_binding_types(decls, &node_types);

        // Build reverse map TypeId → name for normalizing Named types
        let id_to_name: HashMap<crate::TypeId, String> = self.types.type_names
            .iter()
            .map(|(name, id)| (*id, name.clone()))
            .collect();

        // Resolve pending generic call type args, normalizing Named(TypeId)
        // to UnresolvedNamed(name) so monomorphizer can use consistent names.
        // A call site can be recorded more than once — method resolution runs
        // again whenever the constraint solver makes progress, and the earlier
        // attempts hold variables that never got bound. Keep the entry that
        // actually resolved; a half-resolved one mangles to `Box2_echo$_` and
        // collides with every other unresolved instantiation of the same method.
        let mut call_type_args: HashMap<NodeId, Vec<Type>> = HashMap::new();
        for (node_id, vars) in &self.pending_call_type_args {
            let resolved: Vec<Type> = vars.iter().map(|v| {
                let applied = self.ctx.apply(v);
                Self::normalize_named_types(&applied, &id_to_name)
            }).collect();
            let fully_resolved = !resolved.iter().any(Self::contains_type_var);
            match call_type_args.get(node_id) {
                Some(existing) if !fully_resolved
                    && !existing.iter().any(Self::contains_type_var) => {}
                _ => { call_type_args.insert(*node_id, resolved); }
            }
        }

        // Return types the checker inferred for functions that don't declare one.
        // Normalized like call type args so lowering sees names, not TypeIds.
        // Unresolved ones are dropped — "still a variable" is no better an answer
        // than the declaration gave.
        let inferred_fn_ret: HashMap<String, Type> = self
            .inferred_fn_types
            .iter()
            .map(|(name, (_, ret))| {
                let applied = Self::normalize_named_types(&self.ctx.apply(ret), &id_to_name);
                (name.clone(), applied)
            })
            .filter(|(_, ty)| !Self::contains_type_var(ty))
            .collect();

        let trait_coercions = self.trait_coercions.clone();
        let error_wraps = self.error_wraps.clone();
        let fallback_keeps_shape = self.fallback_keeps_shape.clone();
        let try_chain_placement = self.try_chain_placement.clone();

        let unsafe_ops = self.unsafe_ops;

        let span_types: HashMap<_, _> = self
            .span_types
            .iter()
            .map(|(key, ty)| (*key, self.ctx.apply(ty)))
            .collect();

        let errors: Vec<_> = {
            let ctx = &self.ctx;
            let types = &self.types;
            self.errors.into_iter()
                .map(|e| Self::apply_error_substitutions_with_ctx(e, ctx))
                .map(|e| types.resolve_error_types(e))
                // Filter out cascading errors where either side resolved to <error>.
                // These are always consequences of an earlier failure, not root causes.
                .filter(|e| !matches!(e,
                    TypeError::Mismatch { expected: Type::Error, .. }
                    | TypeError::Mismatch { found: Type::Error, .. }
                ))
                .collect()
        };

        let program = TypedProgram {
            symbols: self.resolved.symbols,
            resolutions: self.resolved.resolutions,
            types: self.types,
            node_types,
            call_type_args,
            call_targets: self.call_targets,
            trait_coercions,
            error_wraps,
            fallback_keeps_shape,
            try_chain_placement,
            unsafe_ops,
            span_types,
            mutate_self_fns: self.mutate_self_fns,
            channel_send_sites: self.channel_send_sites,
            inferred_fn_ret,
        };

        (program, errors)
    }

    /// Replace Named(TypeId) with UnresolvedNamed(name) so the monomorphizer
    /// sees consistent string-based type names regardless of resolution order.
    /// True when any part of the type is still an unbound inference variable.
    /// Such a type mangles to `_`, which is not a name that identifies anything.
    fn contains_type_var(ty: &Type) -> bool {
        match ty {
            Type::Var(_) => true,
            Type::RawPtr(inner) | Type::Slice(inner)
            | Type::Array { elem: inner, .. } => Self::contains_type_var(inner),
            Type::Result { ok, err } => {
                Self::contains_type_var(ok) || Self::contains_type_var(err)
            }
            Type::Tuple(items) | Type::Union(items) => items.iter().any(Self::contains_type_var),
            Type::Fn { params, ret } => {
                params.iter().any(Self::contains_type_var) || Self::contains_type_var(ret)
            }
            Type::Generic { args, .. } | Type::UnresolvedGeneric { args, .. } => {
                args.iter().any(|a| match a {
                    crate::GenericArg::Type(t) => Self::contains_type_var(t),
                    _ => false,
                })
            }
            _ => false,
        }
    }

    fn normalize_named_types(ty: &Type, id_to_name: &HashMap<crate::TypeId, String>) -> Type {
        match ty {
            Type::Named(id) => {
                if let Some(name) = id_to_name.get(id) {
                    Type::UnresolvedNamed(name.clone())
                } else {
                    ty.clone()
                }
            }
            Type::UnresolvedGeneric { name, args } => Type::UnresolvedGeneric {
                name: name.clone(),
                args: args.iter().map(|a| match a {
                    crate::GenericArg::Type(inner) => {
                        crate::GenericArg::Type(Box::new(Self::normalize_named_types(inner, id_to_name)))
                    }
                    other => other.clone(),
                }).collect(),
            },
            Type::Result { ok, err } if **err == Type::None => {
                Type::option(Self::normalize_named_types(ok, id_to_name))
            }
            Type::Result { ok, err } => Type::Result {
                ok: Box::new(Self::normalize_named_types(ok, id_to_name)),
                err: Box::new(Self::normalize_named_types(err, id_to_name)),
            },
            _ => ty.clone(),
        }
    }

    fn apply_error_substitutions_with_ctx(error: TypeError, ctx: &InferenceContext) -> TypeError {
        match error {
            TypeError::Mismatch { expected, found, span } => TypeError::Mismatch {
                expected: ctx.apply(&expected),
                found: ctx.apply(&found),
                span,
            },
            TypeError::NotCallable { ty, span } => TypeError::NotCallable {
                ty: ctx.apply(&ty),
                span,
            },
            TypeError::NoSuchField { ty, field, span } => TypeError::NoSuchField {
                ty: ctx.apply(&ty),
                field,
                span,
            },
            TypeError::NoSuchMethod { ty, method, span } => TypeError::NoSuchMethod {
                ty: ctx.apply(&ty),
                method,
                span,
            },
            TypeError::MissingReturn { function_name, expected_type, span } => TypeError::MissingReturn {
                function_name,
                expected_type: ctx.apply(&expected_type),
                span,
            },
            TypeError::TryInNonPropagatingContext { return_ty, span } => TypeError::TryInNonPropagatingContext {
                return_ty: ctx.apply(&return_ty),
                span,
            },
            TypeError::InfiniteType { var, ty, span } => TypeError::InfiniteType {
                var,
                ty: ctx.apply(&ty),
                span,
            },
            TypeError::TryOnNonResult { found, span } => TypeError::TryOnNonResult {
                found: ctx.apply(&found),
                span,
            },
            TypeError::NominalMismatch { expected, found, nominal_name, span } => TypeError::NominalMismatch {
                expected: ctx.apply(&expected),
                found: ctx.apply(&found),
                nominal_name,
                span,
            },
            TypeError::IndexTypeMismatch { container, found, kind, span } => TypeError::IndexTypeMismatch {
                container: ctx.apply(&container),
                found: ctx.apply(&found),
                kind: match kind {
                    errors::IndexErrorKind::ExpectedHandle(h) => {
                        errors::IndexErrorKind::ExpectedHandle(ctx.apply(&h))
                    }
                    errors::IndexErrorKind::ExpectedKey(k) => {
                        errors::IndexErrorKind::ExpectedKey(ctx.apply(&k))
                    }
                    other => other,
                },
                span,
            },
            other => other,
        }
    }
}

impl Default for TypeChecker {
    fn default() -> Self {
        Self::new(ResolvedProgram::default())
    }
}

// ============================================================================
// Public API
// ============================================================================

pub fn typecheck(resolved: ResolvedProgram, decls: &[Decl]) -> Result<TypedProgram, Vec<TypeError>> {
    let checker = TypeChecker::new(resolved);
    checker.check(decls)
}

/// Typecheck with stdlib type/method declarations registered but not body-checked.
pub fn typecheck_with_stdlib(
    resolved: ResolvedProgram,
    decls: &[Decl],
    stdlib_decls: &[Decl],
) -> Result<TypedProgram, Vec<TypeError>> {
    let mut checker = TypeChecker::new(resolved);
    // In stdlib scope: these registrations are what stdlib code means by a
    // name, and they must not be overwritten when the program declares its own.
    checker.types.stdlib_mode = true;
    checker.collect_type_declarations(stdlib_decls);
    checker.types.stdlib_mode = false;
    checker.check(decls)
}

/// Lenient typecheck: always returns the (partial) TypedProgram plus errors.
///
/// Enables cross-stage error accumulation — the driver can feed the partial
/// program to ownership/effects analysis even when type errors exist, so
/// users see type errors + ownership errors + effect warnings in one pass
/// instead of fixing them one category at a time.
pub fn typecheck_with_stdlib_lenient(
    resolved: ResolvedProgram,
    decls: &[Decl],
    stdlib_decls: &[Decl],
) -> (TypedProgram, Vec<TypeError>) {
    let mut checker = TypeChecker::new(resolved);
    checker.types.stdlib_mode = true;
    checker.collect_type_declarations(stdlib_decls);
    checker.types.stdlib_mode = false;
    // The stdlib's bodies are compiled into every program, so they're checked
    // with it. Without that, everything downstream sees them as untyped and
    // lowering has to guess what a call inside them refers to (#425).
    //
    // Bodies only: the types were registered from the stub set above, and
    // re-declaring them here would mint a second TypeId per name — which is
    // how `JsonValue` ended up not unifying with itself.
    let bodies: Vec<Decl> = rask_stdlib::StubRegistry::compilable_decls()
        .into_iter()
        .filter(|d| matches!(d.kind,
            rask_ast::decl::DeclKind::Fn(_) | rask_ast::decl::DeclKind::Impl(_)))
        .collect();
    checker.check_lenient_with_stdlib(&bodies, decls)
}
