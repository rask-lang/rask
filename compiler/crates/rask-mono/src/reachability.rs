// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Monomorphization driver - reachability-driven instantiation.
//!
//! Walks the call graph from main(), instantiating generic functions on demand.
//! This is the core loop per spec rules M1-M4:
//!   M1: Walk reachable code from main()
//!   M2: Instantiate each unique (function_id, [type_args])
//!   M3: Compute layouts (done after this pass)
//!   M4: Transitive - new instantiations may discover more calls

use crate::instantiate::instantiate_function;
use crate::MonoFunction;
use rask_ast::{
    decl::{Decl, DeclKind, FnDecl},
    expr::{Expr, ExprKind},
    stmt::{Stmt, StmtKind},
};
use rask_ast::{NodeId, Span};
use rask_types::{Callee, Type, TypeId, TypedProgram};
use std::collections::{HashMap, HashSet, VecDeque};

/// Monomorphization work item
struct WorkItem {
    name: String,
    type_args: Vec<Type>,
}

/// A call whose method body can't be reached through its mangled name.
///
/// Functions are keyed by `Type_method` from here through codegen, so two types
/// with the same name produce one symbol for two bodies. The type that owns the
/// name gets it; a call on the other one has nowhere to go.
#[derive(Debug, Clone)]
pub struct AmbiguousMethod {
    pub type_name: String,
    pub method: String,
    pub span: Span,
}

/// A callee name with any written type arguments removed: `make<i32>` →
/// `make`. Names with no `<…>` come back unchanged.
///
/// Only for call position. A *type* name like `Vec<i64>` keeps its arguments —
/// they're part of which type it is.
pub(crate) fn strip_written_type_args(name: &str) -> &str {
    match name.find('<') {
        Some(i) if name.ends_with('>') => &name[..i],
        _ => name,
    }
}

/// Generate a mangled name for a generic function instantiation.
/// e.g., ("render_children", [Inline]) → "render_children$Inline"
pub fn mangle_name(base: &str, type_args: &[Type]) -> String {
    if type_args.is_empty() {
        return base.to_string();
    }
    let args_str: Vec<String> = type_args.iter().map(symbol_spelling).collect();
    format!("{}${}", base, args_str.join("_"))
}

/// One type argument as it appears inside a symbol name.
///
/// A written instantiation flattens: `Wrap<i64>` becomes `Wrap$i64`,
/// `Pair<i64, Big>` becomes `Pair$i64$Big`. A `<` in a symbol name is what
/// `strip_written_type_args` cuts off, so `get$Wrap<i64>` would come back out of
/// it as `get$Wrap` — a name for a different instantiation (#871).
///
/// `?` and ` or ` get names of their own rather than being dropped: `i64?` and
/// `i64` are different instantiations of the same method and can't share a
/// symbol (#872).
fn symbol_spelling(ty: &Type) -> String {
    let spelled = format!("{}", ty);
    if spelled.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return spelled;
    }
    spelled
        .replace(" or ", "$or$")
        .replace('?', "$opt")
        .replace(", ", "$")
        .replace('<', "$")
        .replace('>', "")
        .replace(' ', "_")
}

/// Drives monomorphization: reachability first, instantiation on demand.
pub struct Monomorphizer<'a> {
    /// Lookup table: function name → original declaration
    fn_table: HashMap<String, &'a Decl>,
    /// Methods extracted from struct/enum/impl declarations (owned).
    /// Keyed by qualified name: "Type_method".
    method_table: HashMap<String, Decl>,
    /// Reverse lookup: bare method name → list of qualified names.
    /// Used to resolve instance method calls where receiver type is unknown.
    method_by_bare_name: HashMap<String, Vec<String>>,
    /// Resolved type args per call site (from typechecker)
    call_type_args: &'a HashMap<NodeId, Vec<Type>>,
    /// Type checker output, when monomorphizing a real program. The standalone
    /// reachability tests build a Monomorphizer without it and keep the
    /// conservative bare-name behaviour.
    typed: Option<&'a TypedProgram>,
    /// Mangled symbol → every type declaring it. Two entries mean the name
    /// alone no longer identifies a body.
    symbol_owners: HashMap<String, Vec<TypeId>>,
    /// Calls that need a body the mangled name can't address (see
    /// `AmbiguousMethod`). Collected here and reported by `monomorphize`.
    pub ambiguous_methods: Vec<AmbiguousMethod>,
    /// External package module names — `pkg.func()` enqueues `func`, not `pkg_func`
    package_modules: std::collections::HashSet<String>,
    /// Already processed (name, type_args) pairs
    seen: HashMap<(String, Vec<Type>), bool>,
    /// BFS work queue
    queue: VecDeque<WorkItem>,
    /// Resulting instantiated functions
    pub results: Vec<MonoFunction>,
    /// Call expression NodeId → mangled callee name.
    /// Used by MIR lowering to rewrite calls to generic function instantiations.
    pub call_rewrites: HashMap<NodeId, String>,
    /// Node ids handed out to instantiated copies. Starts above every id the
    /// original program used, so a copy's nodes can never be mistaken for the
    /// nodes they were cloned from.
    next_instantiated_id: u32,
    /// Per-node facts carried onto the instantiated copies: the checker keys
    /// everything by node id, and a copy's nodes are new. Populated from the
    /// origin map each instantiation reports.
    pub instantiated_node_types: HashMap<NodeId, rask_types::Type>,
    /// Dispatch targets for the copies, same idea as `instantiated_node_types`.
    pub instantiated_call_targets: HashMap<NodeId, rask_types::Callee>,
    /// ER31a error wraps for the copies, same idea. The wrapping variant names a
    /// concrete enum, so it carries over unchanged.
    pub instantiated_error_wraps: HashMap<NodeId, rask_types::ErrorWrap>,
    /// ER14a: instantiated `??` nodes whose right side is still wrapped.
    pub instantiated_fallback_keeps_shape: HashSet<NodeId>,
    /// Per-call-site type arguments for the copies. A generic calling another
    /// generic (`func outer<T>(x: T) { inner(x) }`) records `[T]` at the inner
    /// call; substituting this instantiation's arguments turns that into the
    /// concrete pair that reachability needs to enqueue.
    instantiated_call_type_args: HashMap<NodeId, Vec<Type>>,
    /// Trait name → object-compatible method names (TR1–TR3).
    /// A vtable references a slot per compatible method, so boxing a value as
    /// `any Trait` makes every such method of the concrete type reachable even
    /// if it's never called explicitly.
    trait_methods: HashMap<String, Vec<String>>,
    /// Expression NodeId → trait name, for implicit TR5 coercion sites (a value
    /// passed where `any Trait` is expected, with no written-out cast).
    trait_coercions: HashMap<NodeId, String>,
    /// All declarations, for roots that don't come from a function body
    /// (module-level `const` initializers).
    decls: &'a [Decl],
    /// Qualified method name → the type parameters of the type it's declared on.
    /// `One_get` → `("One", ["A"])`. A method's own signature never mentions
    /// where `A` came from, so without this the receiver's instantiation has no
    /// name to bind to (#814).
    method_owners: HashMap<String, MethodOwner>,
}

/// The generic type a method is declared on.
#[derive(Clone)]
struct MethodOwner {
    /// Bare name — `One` out of `One<A>`.
    base: String,
    /// Type parameter names in declaration order, bounds stripped. A tuple
    /// argument contributes each of its members, so `Sequence<(K, V)>` says
    /// `["K", "V"]` — the two names a call binds — rather than one name that
    /// happens to be spelled `(K, V)`.
    params: Vec<String>,
    /// The target as written, so the receiver's type can be rebuilt by
    /// substituting into it. Reconstructing it as `base<args…>` is only right
    /// when the parameters sit directly under the angle brackets:
    /// `extend Sequence<(K, V)>` came back as `Sequence<i32>`, dropping `V`,
    /// and the receiver then had a scalar element type where a pair was due.
    template: String,
}

/// Split `One<A, B: Trait>` into `("One", ["A", "B"])`. `None` when the name
/// carries no parameters.
fn parse_owner(type_name: &str) -> Option<MethodOwner> {
    let open = type_name.find('<')?;
    let inner = type_name[open + 1..].trim_end().strip_suffix('>')?;
    let mut params: Vec<String> = Vec::new();
    for arg in split_top_level(inner, ',') {
        let bare = arg.split(':').next().unwrap_or(arg).trim();
        // A tuple argument binds each of its members, not itself.
        if let Some(members) = bare.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
            for m in split_top_level(members, ',') {
                let m = m.split(':').next().unwrap_or(m).trim();
                if !m.is_empty() {
                    params.push(m.to_string());
                }
            }
        } else if !bare.is_empty() {
            params.push(bare.to_string());
        }
    }
    if params.is_empty() {
        return None;
    }
    Some(MethodOwner {
        base: type_name[..open].trim().to_string(),
        params,
        template: type_name.trim().to_string(),
    })
}

/// Replace whole-word occurrences of a type parameter name in a type string.
///
/// Whole-word so `V` doesn't rewrite the `V` inside `Value`, and so a parameter
/// named `T` leaves `Task` alone.
fn substitute_param_name(ty: &str, param: &str, arg: &str) -> String {
    let mut out = String::with_capacity(ty.len());
    let bytes = ty.as_bytes();
    let mut i = 0usize;
    while i < ty.len() {
        let rest = &ty[i..];
        let boundary_before = i == 0 || !is_ident_byte(bytes[i - 1]);
        if boundary_before && rest.starts_with(param) {
            let after = i + param.len();
            let boundary_after = after >= ty.len() || !is_ident_byte(bytes[after]);
            if boundary_after {
                out.push_str(arg);
                i = after;
                continue;
            }
        }
        let ch = rest.chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Split on `sep` at nesting depth zero, so `Map<K, V>, T` gives two parts.
fn split_top_level(s: &str, sep: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            c if c == sep && depth == 0 => {
                parts.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

/// Methods declared directly by a type declaration or an `extend` block.
fn methods_of(decl: &Decl) -> &[FnDecl] {
    match &decl.kind {
        DeclKind::Struct(s) => &s.methods,
        DeclKind::Enum(e) => &e.methods,
        DeclKind::Impl(i) => &i.methods,
        _ => &[],
    }
}

/// Wrap a method FnDecl as a top-level Decl and register it under its
/// qualified name (Type_method). Also records the bare→qualified mapping.
fn register_method(
    type_name: &str,
    method: &FnDecl,
    parent_decl: &Decl,
    method_table: &mut HashMap<String, Decl>,
    method_by_bare_name: &mut HashMap<String, Vec<String>>,
    method_owners: &mut HashMap<String, MethodOwner>,
    owner_params: &HashMap<String, Vec<String>>,
) {
    let qualified = format!("{}_{}", type_name, method.name);
    // `extend Wrapper<T>` says its parameters in the header. `extend Wrapper`
    // doesn't, and the owning declaration is where they live — the header is
    // allowed to leave them out. Without this fallback such a method had no
    // parameters to bind, so it never got a per-receiver copy: one shared
    // `Wrapper_get` returning a word served `Wrapper<f64>` (which read the
    // double's bits as an integer) and `Wrapper<string>` (which segfaulted on a
    // 16-byte value in an 8-byte return slot) alike (#916).
    let owner = parse_owner(type_name).or_else(|| {
        let base = type_name.split('<').next().unwrap_or(type_name).trim();
        owner_params
            .get(base)
            .filter(|params| !params.is_empty())
            .map(|params| MethodOwner {
                base: base.to_string(),
                template: format!("{}<{}>", base, params.join(", ")),
                params: params.clone(),
            })
    });
    if let Some(owner) = owner {
        method_owners.insert(format!("{}_{}", owner.base, method.name), owner.clone());
        method_owners.insert(qualified.clone(), owner);
    }
    let wrapped = Decl {
        id: parent_decl.id,
        kind: DeclKind::Fn(method.clone()),
        span: parent_decl.span,
    };
    method_table.insert(qualified.clone(), wrapped.clone());
    method_by_bare_name
        .entry(method.name.clone())
        .or_default()
        .push(qualified.clone());

    // For generic types like Box<T>, also register under the stripped name (Box_new)
    // so MIR calls that strip generic params can resolve the method.
    let base = type_name.split('<').next().unwrap_or(type_name);
    if base != type_name {
        let stripped = format!("{}_{}", base, method.name);
        method_table.entry(stripped.clone()).or_insert(wrapped);
        method_by_bare_name
            .entry(method.name.clone())
            .or_default()
            .push(stripped);
    }
}

impl<'a> Monomorphizer<'a> {
    pub fn new(decls: &'a [Decl], call_type_args: &'a HashMap<NodeId, Vec<Type>>) -> Self {
        let mut fn_table = HashMap::new();
        let mut method_table = HashMap::new();
        let mut method_by_bare_name: HashMap<String, Vec<String>> = HashMap::new();
        let mut method_owners: HashMap<String, MethodOwner> = HashMap::new();

        // Type parameters per declared type, by bare name — PC1, so a struct
        // that never wrote `<T>` still reports the parameters its field types
        // imply. `register_method` falls back to this when an `extend` header
        // leaves them out.
        let mut owner_params: HashMap<String, Vec<String>> = HashMap::new();
        for decl in decls {
            let (name, params) = match &decl.kind {
                DeclKind::Struct(s) => (&s.name, rask_types::struct_type_param_names(s)),
                DeclKind::Enum(e) => (&e.name, rask_types::enum_type_param_names(e)),
                _ => continue,
            };
            if params.is_empty() {
                continue;
            }
            let bare = name.split('<').next().unwrap_or(name).trim().to_string();
            owner_params.insert(bare, params);
        }

        // Every type a method could belong to. `Type_method`-shaped free
        // functions are registered as instance methods below, and without this
        // the "type" half was whatever came before the first underscore —
        // making `print_fields` a `fields` method (see below).
        let mut declared_types: std::collections::HashSet<String> = std::collections::HashSet::new();
        for decl in decls {
            let name = match &decl.kind {
                DeclKind::Struct(s) => &s.name,
                DeclKind::Enum(e) => &e.name,
                DeclKind::Impl(i) => &i.target_ty,
                _ => continue,
            };
            declared_types.insert(name.split('<').next().unwrap_or(name).trim().to_string());
        }

        for decl in decls {
            match &decl.kind {
                DeclKind::Fn(f) => {
                    fn_table.insert(f.name.clone(), decl);
                    // Also register under base name for generic functions:
                    // parser stores "foo<T: Trait>" but call sites use "foo"
                    let base = f.name.split('<').next().unwrap_or(&f.name);
                    if base != f.name {
                        fn_table.insert(base.to_string(), decl);
                    }
                    // Free functions with Type_method naming (e.g. compiled stdlib
                    // wrappers) should also be discoverable as instance methods.
                    //
                    // Only when the half before the underscore is a type that
                    // exists. Any underscore used to do, so an ordinary function
                    // named `print_fields` was filed as a `fields` method of a
                    // type called `print`. A call to `something.fields()` whose
                    // receiver this pass couldn't pin down then swept it up along
                    // with the real ones — and handed it that call's type
                    // arguments. `reflect.fields<T>()` inside an uninstantiated
                    // generic template did exactly that, so `print_fields` got
                    // queued with a `T` that was still an open variable and MIR
                    // was asked to lower `print_fields$_` (#931).
                    if let Some(underscore_pos) = f.name.find('_') {
                        let (owner, bare_method) =
                            (&f.name[..underscore_pos], &f.name[underscore_pos + 1..]);
                        if !bare_method.is_empty() && declared_types.contains(owner) {
                            method_by_bare_name
                                .entry(bare_method.to_string())
                                .or_default()
                                .push(f.name.clone());
                        }
                    }
                }
                DeclKind::Struct(s) => {
                    for method in &s.methods {
                        register_method(
                            &s.name, method, decl,
                            &mut method_table, &mut method_by_bare_name,
                            &mut method_owners, &owner_params,
                        );
                    }
                }
                DeclKind::Enum(e) => {
                    for method in &e.methods {
                        register_method(
                            &e.name, method, decl,
                            &mut method_table, &mut method_by_bare_name,
                            &mut method_owners, &owner_params,
                        );
                    }
                }
                DeclKind::Impl(i) => {
                    for method in &i.methods {
                        register_method(
                            &i.target_ty, method, decl,
                            &mut method_table, &mut method_by_bare_name,
                            &mut method_owners, &owner_params,
                        );
                    }
                }
                _ => {}
            }
        }

        // TR1–TR3: object-compatible methods per trait — a method drops out if
        // it declares its own type params (TR3) or returns Self (TR2), matching
        // the vtable layout in codegen.
        let mut trait_methods: HashMap<String, Vec<String>> = HashMap::new();
        for decl in decls {
            if let DeclKind::Trait(t) = &decl.kind {
                let compatible = t.methods.iter()
                    .filter(|m| m.type_params.is_empty()
                        && m.ret_ty.as_deref() != Some("Self"))
                    .map(|m| m.name.clone())
                    .collect();
                trait_methods.insert(t.name.clone(), compatible);
            }
        }
        // Compiler-provided traits have no declaration to read. Without an
        // entry, boxing a value as `any Error` marked none of the concrete
        // type's `message` bodies reachable, so the vtable slot pointed at a
        // function nobody had emitted (#708). Method names only — the
        // signatures live in rask-types, and the concrete bodies are found by
        // bare name below, same as for a declared trait.
        for name in rask_types::COMPILER_PROVIDED_TRAITS {
            trait_methods
                .entry(name.to_string())
                .or_insert_with(|| rask_types::builtin_trait_method_names(name));
        }

        Self {
            fn_table,
            method_table,
            method_by_bare_name,
            call_type_args,
            typed: None,
            symbol_owners: HashMap::new(),
            ambiguous_methods: Vec::new(),
            package_modules: std::collections::HashSet::new(),
            seen: HashMap::new(),
            queue: VecDeque::new(),
            results: Vec::new(),
            call_rewrites: HashMap::new(),
            next_instantiated_id: 0,
            instantiated_node_types: HashMap::new(),
            instantiated_call_targets: HashMap::new(),
            instantiated_error_wraps: HashMap::new(),
            instantiated_fallback_keeps_shape: HashSet::new(),
            instantiated_call_type_args: HashMap::new(),
            trait_methods,
            trait_coercions: HashMap::new(),
            decls,
            method_owners,
        }
    }

    /// Build a reachability pass that resolves methods through the type checker.
    ///
    /// `new` keys methods by `Type_method`, so when a program type shadows a
    /// stdlib one — a user `struct JsonError` over stdlib's `enum JsonError` —
    /// whichever declaration comes last in the flattened decl list wins the
    /// entry, and calls on the other type reach the wrong body. The checker
    /// already bound every method to a TypeId; this rebinds the table to match.
    pub fn with_typed_program(decls: &'a [Decl], typed: &'a TypedProgram) -> Self {
        let mut mono = Self::new(decls, &typed.call_type_args);
        mono.typed = Some(typed);
        // Instantiated copies number their nodes from here up. Anything at or
        // below this is a real node of the original program, and a copy reusing
        // one would answer type and dispatch queries with that node's record.
        mono.next_instantiated_id = typed
            .node_types
            .keys()
            .chain(typed.call_targets.keys())
            .map(|n| n.0)
            .max()
            .map_or(0, |m| m + 1);
        mono.bind_methods_by_type(typed);
        mono
    }

    /// Copy the checker's per-node records onto an instantiated body.
    ///
    /// The copy's nodes are new, so nothing the checker recorded reaches them
    /// on their own ids. Each one knows which original node it came from, which
    /// is enough to bring the type and the dispatch target across.
    ///
    /// Types recorded against the *generic* body still mention its type
    /// parameters, so they're substituted on the way over: a receiver the
    /// checker typed `T` becomes the concrete type this instantiation is for.
    /// A record that still names a type parameter afterwards is dropped rather
    /// than carried — a wrong answer here is worse than no answer, because
    /// lowering's fallbacks can recognise absence but not incorrectness.
    fn carry_node_records(
        &mut self,
        origins: &HashMap<NodeId, NodeId>,
        type_args: &[Type],
        param_names: &[String],
    ) {
        let bindings: HashMap<&str, &Type> = param_names
            .iter()
            .map(|n| n.as_str())
            .zip(type_args.iter())
            .collect();
        for (&new_id, &old_id) in origins {
            if let Some(args) = self.call_type_args.get(&old_id) {
                let concrete: Vec<Type> = args
                    .iter()
                    .map(|a| Self::substitute_param(a, &bindings, type_args))
                    .collect();
                if !concrete.is_empty() {
                    self.instantiated_call_type_args.insert(new_id, concrete);
                }
            }
        }
        let Some(typed) = self.typed else { return };
        for (&new_id, &old_id) in origins {
            if let Some(ty) = typed.node_types.get(&old_id) {
                if let Some(concrete) = Self::concretize(ty, type_args, &bindings) {
                    self.instantiated_node_types.insert(new_id, concrete);
                }
            }
            if let Some(callee) = typed.call_targets.get(&old_id) {
                let carried = match callee {
                    rask_types::Callee::Free(sym) => Some(rask_types::Callee::Free(*sym)),
                    rask_types::Callee::Method { recv, method } => {
                        Self::concretize(recv, type_args, &bindings).map(|recv| {
                            rask_types::Callee::Method { recv, method: method.clone() }
                        })
                    }
                };
                if let Some(c) = carried {
                    self.instantiated_call_targets.insert(new_id, c);
                }
            }
            // ER31a: the wrapping variant names a concrete enum, so it carries
            // over as-is — no substitution to do.
            if let Some(wrap) = typed.error_wraps.get(&old_id) {
                self.instantiated_error_wraps.insert(new_id, wrap.clone());
            }
            // ER14a: whether a `??` keeps its shape is a property of the two
            // operand types, which substitution preserves.
            if typed.fallback_keeps_shape.contains(&old_id) {
                self.instantiated_fallback_keeps_shape.insert(new_id);
            }
        }
    }

    /// A type argument with this instantiation's parameters filled in. Bind by
    /// name first — `pair<A, B>` calling `level1(a)` records `[A]`, and only the
    /// name says which of the two arguments that is. The positional fallback
    /// covers a single-parameter instantiation whose name list is unavailable.
    fn substitute_param(ty: &Type, bindings: &HashMap<&str, &Type>, type_args: &[Type]) -> Type {
        if let Type::UnresolvedNamed(name) = ty {
            if let Some(bound) = bindings.get(name.as_str()) {
                return (*bound).clone();
            }
        }
        Self::concretize(ty, type_args, bindings).unwrap_or_else(|| ty.clone())
    }

    /// A recorded type with this instantiation's arguments substituted in, or
    /// `None` if it still refers to something only the generic body knows.
    ///
    /// Single-letter uppercase names are type parameters (type.gradual/PC3), so
    /// they're the marker for "this came from the generic and wasn't resolved".
    fn concretize(
        ty: &Type,
        type_args: &[Type],
        bindings: &HashMap<&str, &Type>,
    ) -> Option<Type> {
        fn is_type_param(name: &str) -> bool {
            let mut chars = name.chars();
            matches!((chars.next(), chars.next()), (Some(c), None) if c.is_ascii_uppercase())
        }
        match ty {
            // The receiver is the type parameter itself. Bind by name, which is
            // the only thing that says *which* parameter it is: this used to be
            // positional and gave up whenever a function had more than one, so
            // `describe_all<T: Describable, U: Describable>` carried no dispatch
            // target for either `first.describe()` or `second.describe()` and
            // lowering fell back to guessing (#425).
            Type::UnresolvedNamed(name) if is_type_param(name) => {
                if let Some(bound) = bindings.get(name.as_str()) {
                    return Some((*bound).clone());
                }
                // No name list available — a single argument still pins it.
                if type_args.len() == 1 { Some(type_args[0].clone()) } else { None }
            }
            // `Vec<T>` and friends: carry it only when every argument came
            // through concrete, and carry the *substituted* form. Returning the
            // original meant a copy made for `Pair<i32, string>` still recorded
            // its result as `Pair<B, A>`, so nothing could name the layout that
            // copy's caller was passing (#814).
            Type::UnresolvedGeneric { name, args } => {
                if is_type_param(name) {
                    return None;
                }
                Some(Type::UnresolvedGeneric {
                    name: name.clone(),
                    args: Self::concretize_args(args, type_args, bindings)?,
                })
            }
            Type::Generic { base, args } => Some(Type::Generic {
                base: *base,
                args: Self::concretize_args(args, type_args, bindings)?,
            }),
            Type::Tuple(elems) => Some(Type::Tuple(Self::concretize_all(
                elems, type_args, bindings,
            )?)),
            Type::Union(elems) => Some(Type::Union(Self::concretize_all(
                elems, type_args, bindings,
            )?)),
            Type::Array { elem, len } => Some(Type::Array {
                elem: Box::new(Self::concretize(elem, type_args, bindings)?),
                len: *len,
            }),
            Type::Slice(elem) => Some(Type::Slice(Box::new(Self::concretize(
                elem, type_args, bindings,
            )?))),
            Type::RawPtr(inner) => Some(Type::RawPtr(Box::new(Self::concretize(
                inner, type_args, bindings,
            )?))),
            Type::Result { ok, err } => Some(Type::Result {
                ok: Box::new(Self::concretize(ok, type_args, bindings)?),
                err: Box::new(Self::concretize(err, type_args, bindings)?),
            }),
            Type::Fn { params, ret } => Some(Type::Fn {
                params: Self::concretize_all(params, type_args, bindings)?,
                ret: Box::new(Self::concretize(ret, type_args, bindings)?),
            }),
            Type::Var(_) => None,
            other => Some(other.clone()),
        }
    }

    /// `concretize` over a list — `None` if any element still refers to a
    /// parameter this instantiation doesn't bind.
    fn concretize_all(
        tys: &[Type],
        type_args: &[Type],
        bindings: &HashMap<&str, &Type>,
    ) -> Option<Vec<Type>> {
        tys.iter()
            .map(|t| Self::concretize(t, type_args, bindings))
            .collect()
    }

    /// `concretize` over generic arguments. A const argument has no type in it,
    /// so it carries over unchanged.
    fn concretize_args(
        args: &[rask_types::GenericArg],
        type_args: &[Type],
        bindings: &HashMap<&str, &Type>,
    ) -> Option<Vec<rask_types::GenericArg>> {
        args.iter()
            .map(|arg| match arg {
                rask_types::GenericArg::Type(inner) => {
                    Self::concretize(inner, type_args, bindings)
                        .map(|t| rask_types::GenericArg::Type(Box::new(t)))
                }
                other => Some(other.clone()),
            })
            .collect()
    }

    /// Re-key `method_table` on type identity instead of declaration order.
    fn bind_methods_by_type(&mut self, typed: &TypedProgram) {
        let by_id: HashMap<NodeId, &Decl> = self.decls.iter().map(|d| (d.id, d)).collect();

        let owned: Vec<(TypeId, Vec<NodeId>)> = typed
            .types
            .types_with_methods()
            .map(|(id, decls)| (id, decls.to_vec()))
            .collect();

        for (type_id, decl_ids) in owned {
            let type_name = typed.types.type_name(type_id);
            // The type the bare name resolves to. Only that one can claim the
            // plain `Type_method` symbol; a shadowed type has no other spelling.
            let owns_name = typed.types.get_type_id(&type_name) == Some(type_id);

            for decl_id in decl_ids {
                let Some(decl) = by_id.get(&decl_id) else { continue };
                for method in methods_of(decl) {
                    let qualified = format!("{}_{}", type_name, method.name);
                    let owners = self.symbol_owners.entry(qualified.clone()).or_default();
                    if !owners.contains(&type_id) {
                        owners.push(type_id);
                    }
                    if owns_name {
                        self.method_table.insert(qualified, Decl {
                            id: decl.id,
                            kind: DeclKind::Fn(method.clone()),
                            span: decl.span,
                        });
                    }
                }
            }
        }
    }

    /// The type whose body `symbol` resolves to, when more than one declares it.
    ///
    /// Every owner of a contested symbol shares the same type name — that's what
    /// made them collide — so any of them gives the name to look up.
    fn contested_owner(&self, symbol: &str) -> Option<TypeId> {
        let typed = self.typed?;
        let owners = self.symbol_owners.get(symbol)?;
        if owners.len() < 2 {
            return None;
        }
        let name = typed.types.type_name(*owners.first()?);
        typed.types.get_type_id(&name)
    }

    /// Record implicit trait-coercion sites (TR5) from the type checker.
    pub fn set_trait_coercions(&mut self, coercions: &HashMap<NodeId, String>) {
        self.trait_coercions = coercions.clone();
    }

    /// Boxing a value as `any Trait` needs every object-compatible method of
    /// the concrete type in the vtable. The receiver type isn't resolved here,
    /// so enqueue every implementation of each compatible method name — the
    /// same conservative widening used for ordinary instance calls.
    fn mark_trait_object_methods(&mut self, trait_name: &str) {
        let base = trait_name.split('<').next().unwrap_or(trait_name);
        let Some(methods) = self.trait_methods.get(base).cloned() else { return };
        for method in methods {
            if let Some(qualified_names) = self.method_by_bare_name.get(&method).cloned() {
                for qname in qualified_names {
                    self.enqueue(qname, Vec::new());
                }
            }
        }
    }

    /// Set the external package module names for cross-package call discovery.
    pub fn set_package_modules(&mut self, modules: std::collections::HashSet<String>) {
        self.package_modules = modules;
    }

    /// Seed the work queue from module-level `const` initializers.
    ///
    /// MIR lowering injects these initializers into every function that can
    /// reference the const, so whatever they call is genuinely reachable even
    /// though no function body mentions it. Without this a const like
    /// `const config = Shared.new(Config.from_env())` leaves `Config_from_env`
    /// undeclared and codegen fails on the injected call.
    pub fn add_module_const_roots(&mut self) {
        for decl in self.decls {
            if let DeclKind::Const(c) = &decl.kind {
                let init = c.init.clone();
                self.visit_expr(&init);
            }
        }
    }

    /// An `extern "C"` function with a body is exported for something outside
    /// the program to call, so nothing in the call graph reaches it — it *is*
    /// the edge of the graph. Without this it was dropped as dead code and the
    /// symbol never made it into the object file: a C driver linking against it
    /// got "undefined reference", which is why struct.c-interop/EX1's export
    /// form had no working path through the compiler at all.
    pub fn add_exported_roots(&mut self) {
        for decl in self.decls {
            let DeclKind::Fn(f) = &decl.kind else { continue };
            if f.abi.as_deref() == Some("C") && !f.body.is_empty() && f.type_params.is_empty() {
                self.enqueue(f.name.clone(), Vec::new());
            }
        }
    }

    /// Is there a body here to make a concrete copy of? Stdlib stubs declare a
    /// signature and an empty block — the implementation is in the runtime.
    fn has_instantiable_body(&self, name: &str) -> bool {
        let decl = self
            .fn_table
            .get(name)
            .copied()
            .or_else(|| self.method_table.get(name));
        matches!(decl.map(|d| &d.kind), Some(DeclKind::Fn(f)) if !f.body.is_empty())
    }

    /// A non-generic top-level function with a body — the only thing a bare
    /// name can refer to as a value. Methods and generics are excluded: there
    /// are no type arguments at a bare-name reference to instantiate them with.
    fn is_plain_fn(&self, name: &str) -> bool {
        matches!(
            self.fn_table.get(name).map(|d| &d.kind),
            Some(DeclKind::Fn(f)) if !f.body.is_empty() && f.type_params.is_empty()
        )
    }

    /// Seed the work queue with main()
    pub fn add_entry(&mut self, name: &str) -> bool {
        if self.fn_table.contains_key(name) {
            self.enqueue(name.to_string(), Vec::new());
            self.add_entry_error_message_roots(name);
            true
        } else {
            false
        }
    }

    /// struct.targets/EX4: an error out of main is printed before the process
    /// exits 1, so `{ErrType}_message` is called from the entry's return path
    /// even when nothing in the program calls it. Nothing in the call graph
    /// says so, hence this root.
    fn add_entry_error_message_roots(&mut self, entry: &str) {
        let Some(decl) = self.fn_table.get(entry) else { return };
        let DeclKind::Fn(f) = &decl.kind else { return };
        let Some(ret_ty) = f.ret_ty.clone() else { return };
        let err_branch = match ret_ty.split_once(" or ") {
            Some((_, e)) => e.trim().to_string(),
            // The canonical form. Splitting it lives in rask_ast::type_str so
            // this and the checker's stub parser can't drift — they already had,
            // over whether `[` `]` nest (a `Vec[T, N]` lane count has a comma).
            None => match rask_ast::type_str::result_parts(ret_ty.trim()) {
                Some((_, e)) => e.trim().to_string(),
                None => return,
            },
        };
        // `A | B` — every arm can be the one that reaches main.
        for arm in err_branch.split('|') {
            let name = format!("{}_message", arm.trim());
            if self.method_table.contains_key(&name) {
                self.enqueue(name, Vec::new());
            }
        }
    }

    /// Run until fixpoint: process queue, instantiate, discover more calls
    pub fn run(&mut self) {
        while let Some(item) = self.queue.pop_front() {
            let key = (item.name.clone(), item.type_args.clone());
            if let Some(visited) = self.seen.get(&key) {
                if *visited {
                    continue;
                }
            }
            self.seen.insert(key, true);

            let original = match self.fn_table.get(&item.name) {
                Some(decl) => *decl,
                None => match self.method_table.get(&item.name) {
                    Some(decl) => decl,
                    None => continue, // External or unknown function
                },
            };

            // Instantiate: if type_args present, clone AST with substitution.
            // Otherwise use original decl directly.
            let concrete = if item.type_args.is_empty() {
                original.clone()
            } else {
                let (param_names, self_ty) =
                    self.instantiation_params(&item.name, original, &item.type_args);
                let (mut cloned, origins) =
                    crate::instantiate::instantiate_function_with_params(
                        original, &param_names, &item.type_args,
                        &mut self.next_instantiated_id,
                    );
                // The receiver's own layout. `self` is spelled `Self`, which
                // nothing substitutes, so a copy made for `One<Big>` otherwise
                // kept the shared placeholder layout for it while its caller
                // passed the 24-byte one (#814).
                if let (Some(self_ty), DeclKind::Fn(f)) = (self_ty, &mut cloned.kind) {
                    if let Some(p) = f.params.first_mut() {
                        if p.name == "self" && p.ty == "Self" {
                            p.ty = self_ty;
                        }
                    }
                }
                self.carry_node_records(&origins, &item.type_args, &param_names);
                cloned
            };

            // Walk the concrete body to discover more calls (M4: transitive)
            if let DeclKind::Fn(fn_decl) = &concrete.kind {
                for stmt in &fn_decl.body {
                    self.visit_stmt(stmt);
                }
            }

            let mangled = mangle_name(&item.name, &item.type_args);
            self.results.push(MonoFunction {
                name: mangled,
                type_args: item.type_args,
                body: concrete,
            });
        }
    }

    /// The parameter names this instantiation's arguments bind to, and the
    /// concrete spelling of `self` when the callee is a method on a generic type.
    ///
    /// Arguments arrive owner-first: `One<Big>.map<i64>()` enqueues `[Big, i64]`,
    /// which binds `A` from `extend One<A>` and then `B` from `map<B>`. The
    /// call site assembles them in that order too, so the two agree.
    fn instantiation_params(
        &self,
        name: &str,
        decl: &Decl,
        type_args: &[Type],
    ) -> (Vec<String>, Option<String>) {
        let Some(owner) = self.method_owners.get(name) else {
            return (crate::instantiate::type_param_names(decl, type_args), None);
        };
        let n = owner.params.len().min(type_args.len());
        let mut names: Vec<String> = owner.params[..n].to_vec();
        names.extend(crate::instantiate::type_param_names(decl, &type_args[n..]));
        let spelled: Vec<String> =
            type_args[..n].iter().map(|t| format!("{}", t)).collect();
        // Substitute into the target as written rather than rebuilding it from
        // the base name: `extend Sequence<(K, V)>` has two parameters under one
        // argument, and `base<args…>` flattened them into `Sequence<i32>`.
        let self_ty = (n == owner.params.len()).then(|| {
            let mut out = owner.template.clone();
            for (param, arg) in owner.params.iter().zip(spelled.iter()) {
                out = substitute_param_name(&out, param, arg);
            }
            out
        });
        (names, self_ty)
    }

    /// A method's own type arguments, minus any that name one of the owning
    /// type's parameters.
    ///
    /// `func wrapped_width(self) -> i64 where T: Sized2` inside `extend Wrap<T>`
    /// puts `T` in the *method's* parameter list — a `where` clause adds an entry
    /// for any name it doesn't already find there, and the parser can't see that
    /// the enclosing header declared it. The receiver has already bound `T`, so
    /// taking a second argument for it bound it twice, and the second one was an
    /// unsolved inference variable. The copy came out as
    /// `Wrap_wrapped_width$Wide__` with `_` substituted over `Wide`, and
    /// `self.value.width()` then had no receiver type to dispatch on (#872).
    fn own_type_args(&self, qualified: &str, args: Vec<Type>) -> Vec<Type> {
        let Some(owner) = self.method_owners.get(qualified) else { return args };
        let Some(decl) = self.method_table.get(qualified) else { return args };
        let DeclKind::Fn(f) = &decl.kind else { return args };
        if f.type_params.len() != args.len() {
            return args;
        }
        f.type_params
            .iter()
            .zip(args)
            .filter(|(tp, _)| !owner.params.contains(&tp.name))
            .map(|(_, a)| a)
            .collect()
    }

    /// The resolved type arguments at a call site. A node inside an
    /// instantiated copy has an id the checker never saw, so its arguments come
    /// from what `carry_node_records` substituted.
    fn type_args_at(&self, id: NodeId) -> Vec<Type> {
        let args = self
            .call_type_args
            .get(&id)
            .or_else(|| self.instantiated_call_type_args.get(&id))
            .cloned()
            .unwrap_or_default();
        // An argument that is itself an instantiation has to be spelled by name.
        // Left as a `Generic` it displays as `<type#84><i32>`, so
        // `unwrap_or(j, fallback)` on a `Maybe<Wrap<i64>>` mangled to a symbol
        // carrying a type id and substituted a string nothing could resolve —
        // the copy took its `Wrap<i64>` parameter as a bare pointer and its
        // match arm loaded the payload word instead of pointing at it (#871).
        let Some(typed) = self.typed else { return args };
        args.into_iter()
            .map(|arg| match &arg {
                Type::Generic { .. }
                | Type::UnresolvedGeneric { .. }
                | Type::Result { .. } => {
                    Self::nameable_type(&arg, &typed.types).unwrap_or(arg)
                }
                _ => arg,
            })
            .collect()
    }

    /// The type arguments the receiver's own type was instantiated with.
    ///
    /// A method declared in `extend One<A>` takes `A` from the extend header, not
    /// from its own signature, so `type_args_at` — which is the *method's* own
    /// arguments — is empty for it. Nothing named a copy per instantiation, so one
    /// shared `One_get` served both an 8-byte and a 24-byte `self` (#814).
    ///
    /// Empty unless every argument is settled and nameable: a type parameter still
    /// standing for itself would mangle to `One_get$A`, which is an instance of
    /// nothing. Also empty unless the count matches what the owning type declares,
    /// so a partial answer never mangles a name it can't fill.
    /// A static method — `Box.new(x)` — has no receiver to read the
    /// instantiation off. Its own call node carries it instead, when the method
    /// returns the type it's declared on, which is what a constructor does. The
    /// base check is what makes that safe: a static method returning some *other*
    /// one-parameter generic would otherwise hand its arguments to the wrong type
    /// parameter.
    fn owner_type_args_from_result(&self, id: NodeId, qualified: &str) -> Vec<Type> {
        let Some(owner) = self.method_owners.get(qualified) else { return Vec::new() };
        let Some(typed) = self.typed else { return Vec::new() };
        let ty = self
            .instantiated_node_types
            .get(&id)
            .or_else(|| typed.node_types.get(&id));
        let Some(Type::Generic { base, .. }) = ty else { return Vec::new() };
        let base_name = typed.types.type_name(*base);
        let bare = base_name.split('<').next().unwrap_or(&base_name).trim();
        if bare != owner.base {
            return Vec::new();
        }
        self.receiver_type_args(id, qualified)
    }

    fn receiver_type_args(&self, id: NodeId, qualified: &str) -> Vec<Type> {
        let Some(owner) = self.method_owners.get(qualified) else { return Vec::new() };
        let Some(typed) = self.typed else { return Vec::new() };
        let ty = self
            .instantiated_node_types
            .get(&id)
            .or_else(|| typed.node_types.get(&id));
        let Some(Type::Generic { args, .. }) = ty else { return Vec::new() };
        let mut out = Vec::with_capacity(args.len());
        for arg in args {
            let rask_types::GenericArg::Type(t) = arg else { return Vec::new() };
            match t.as_ref() {
                // Named through the table, so `mangle_name`'s Display gives a name
                // rather than `<type#N>`.
                Type::Named(tid) => {
                    let name = typed.types.type_name(*tid);
                    let bare = name.split('<').next().unwrap_or(&name).trim().to_string();
                    if bare.is_empty() {
                        return Vec::new();
                    }
                    out.push(Type::UnresolvedNamed(bare));
                }
                Type::Bool | Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128
                | Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128
                | Type::F32 | Type::F64 | Type::Char | Type::String => out.push(t.as_ref().clone()),
                // A nested instantiation — `Wrap<Wrap<i32>>`. It has to say
                // *which* inner type: `get` on `Wrap<Wrap<i32>>` hands back a
                // `Wrap<i32>` and on `Wrap<i32>` an `i32`, so one shared body
                // can't serve both. Bailing out here left both on the same body,
                // and the fallback then bound the method's parameter to the
                // *inner* argument — `Wrap<Wrap<i64>>.get()` compiled as if it
                // returned an i64 (#871).
                Type::Generic { .. }
                | Type::UnresolvedGeneric { .. }
                | Type::Result { .. } => {
                    match Self::nameable_type(t.as_ref(), &typed.types) {
                        Some(named) => out.push(named),
                        None => return Vec::new(),
                    }
                }
                _ => return Vec::new(),
            }
        }
        if out.len() != owner.params.len() {
            return Vec::new();
        }
        out
    }

    /// The same type, spelled so both a symbol name and a layout lookup can be
    /// built from it: every interned id replaced by the name it stands for.
    ///
    /// A `Type::Named` prints as `<type#7>` and a `Type::Generic` as
    /// `<type#84><i32>`, neither of which names anything downstream. `None` when
    /// some part of the type has no name at all — an inference variable, a type
    /// parameter still standing for itself.
    fn nameable_type(ty: &Type, types: &rask_types::TypeTable) -> Option<Type> {
        let bare = |n: &str| n.split('<').next().unwrap_or(n).trim().to_string();
        match ty {
            Type::Named(id) => {
                let name = bare(&types.type_name(*id));
                (!name.is_empty()).then(|| Type::UnresolvedNamed(name))
            }
            Type::UnresolvedNamed(name) => {
                let name = bare(name);
                (!name.is_empty()).then(|| Type::UnresolvedNamed(name))
            }
            Type::Bool | Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128
            | Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128
            | Type::F32 | Type::F64 | Type::Char | Type::String => Some(ty.clone()),
            Type::Generic { base, args } => {
                Self::nameable_generic(&bare(&types.type_name(*base)), args, types)
            }
            Type::UnresolvedGeneric { name, args } => {
                Self::nameable_generic(&bare(name), args, types)
            }
            // `T?` is a `Result` whose error side is `none`, so both shapes come
            // through here. A generic type instantiated with one needs its own
            // body: an optional is 16 bytes and a result 24, where the shared
            // layout's slot is 8 (#872).
            Type::Result { ok, err } => Some(Type::Result {
                ok: Box::new(Self::nameable_type(ok, types)?),
                err: Box::new(Self::nameable_type(err, types)?),
            }),
            Type::None | Type::Unit => Some(ty.clone()),
            _ => None,
        }
    }

    fn nameable_generic(
        head: &str,
        args: &[rask_types::GenericArg],
        types: &rask_types::TypeTable,
    ) -> Option<Type> {
        if head.is_empty() || args.is_empty() {
            return None;
        }
        let mut out = Vec::with_capacity(args.len());
        for arg in args {
            let rask_types::GenericArg::Type(inner) = arg else { return None };
            let named = Self::nameable_type(inner, types)?;
            out.push(rask_types::GenericArg::Type(Box::new(named)));
        }
        Some(Type::UnresolvedGeneric { name: head.to_string(), args: out })
    }

    /// The name of the type the checker gave a node, if it has one.
    fn arg_type_name(&self, id: NodeId) -> Option<String> {
        let typed = self.typed?;
        let ty = self
            .instantiated_node_types
            .get(&id)
            .or_else(|| typed.node_types.get(&id))?;
        rask_types::receiver_name(ty, &typed.types)
    }

    /// Add a (name, type_args) pair to queue if not already seen
    fn enqueue(&mut self, name: String, type_args: Vec<Type>) {
        let key = (name.clone(), type_args.clone());
        if !self.seen.contains_key(&key) {
            self.seen.insert(key, false);
            self.queue.push_back(WorkItem { name, type_args });
        }
    }

    // --- AST visitors: find calls, enqueue discovered functions ---

    fn visit_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Expr(e) => self.visit_expr(e),
            StmtKind::Mut { init, .. } | StmtKind::Let { init, .. } => {
                self.visit_expr(init);
            }
            StmtKind::Assign { target, value, .. } => {
                self.visit_expr(target);
                self.visit_expr(value);
            }
            StmtKind::Return(Some(e)) => self.visit_expr(e),
            StmtKind::Return(None) => {}
            StmtKind::While { cond, body, .. } => {
                self.visit_expr(cond);
                for s in body {
                    self.visit_stmt(s);
                }
            }
            StmtKind::For { iter, body, .. } => {
                self.visit_expr(iter);
                for s in body {
                    self.visit_stmt(s);
                }
            }
            StmtKind::Loop { body, .. } => {
                for s in body {
                    self.visit_stmt(s);
                }
            }
            StmtKind::Ensure { body, else_handler } => {
                for s in body {
                    self.visit_stmt(s);
                }
                if let Some((_param, handler)) = else_handler {
                    for s in handler {
                        self.visit_stmt(s);
                    }
                }
            }
            StmtKind::WhileLet { expr, body, .. } => {
                self.visit_expr(expr);
                for s in body {
                    self.visit_stmt(s);
                }
            }
            StmtKind::Comptime(body) => {
                for s in body {
                    self.visit_stmt(s);
                }
            }
            StmtKind::ComptimeFor { iter, body, .. } => {
                self.visit_expr(iter);
                for s in body {
                    self.visit_stmt(s);
                }
            }
            StmtKind::MutTuple { init, .. }
            | StmtKind::LetTuple { init, .. }
            | StmtKind::LetStruct { init, .. } => {
                self.visit_expr(init);
            }
            StmtKind::Break { value: Some(e), .. } => self.visit_expr(e),
            StmtKind::Break { value: None, .. } | StmtKind::Continue(_) => {}
            StmtKind::Discard { .. } => {}
        }
    }

    fn visit_expr(&mut self, expr: &Expr) {
        // TR5: implicit coercion to `any Trait` (function arg, field, element)
        // with no written-out cast — the checker flags these by NodeId.
        if let Some(trait_name) = self.trait_coercions.get(&expr.id).cloned() {
            self.mark_trait_object_methods(&trait_name);
        }

        match &expr.kind {
            ExprKind::Call { func, args } => {
                if let ExprKind::Ident(name) = &func.kind {
                    // `make<i32>(2)` — the parser folds the written type
                    // arguments into the callee's name, so the function table's
                    // bare key never matched and nothing was ever queued for
                    // the call (#712). The arguments are already in
                    // `type_args_at`, put there by the checker.
                    let name = &strip_written_type_args(name).to_string();
                    let type_args = self.type_args_at(expr.id);
                    // Record call rewrite so MIR lowering uses the mangled name.
                    // Only for functions with a body to instantiate — a stdlib
                    // stub like `spawn(f: func() -> T)` is generic in its
                    // signature but resolves to one C entry point, so mangling
                    // it produced a call to `spawn$i64` that nothing emits.
                    if !type_args.is_empty() && self.has_instantiable_body(name) {
                        let mangled = mangle_name(name, &type_args);
                        self.call_rewrites.insert(expr.id, mangled);
                    }
                    self.enqueue(name.clone(), type_args);
                }
                self.visit_expr(func);
                for arg in args {
                    self.visit_expr(&arg.expr);
                }
            }
            ExprKind::MethodCall { object, method, args, type_args: written_type_args, .. } => {
                let type_args = self.type_args_at(expr.id);

                // `json.encode(v)` where v is a JsonValue is `v.to_string()` —
                // the encoder is in stdlib/json.rk, in Rask. The name of that
                // body is decided here, by the pass that compiles it, and MIR
                // reads the rewrite. Naming it in lowering instead is how
                // `json.encode` on a JsonValue came out of codegen as
                // `Function not found: JsonValue_to_string`: nothing had
                // queued the body, because the pass that queues bodies never
                // heard the name.
                if matches!(&object.kind, ExprKind::Ident(n) if n == "json")
                    && matches!(method.as_str(), "encode" | "encode_pretty")
                    && args.len() == 1
                    && self.arg_type_name(args[0].expr.id).as_deref() == Some("JsonValue")
                {
                    let body = if method == "encode_pretty" {
                        "JsonValue_to_string_pretty".to_string()
                    } else {
                        "JsonValue_to_string".to_string()
                    };
                    self.call_rewrites.insert(expr.id, body.clone());
                    self.enqueue(body, Vec::new());
                    for arg in args {
                        self.visit_expr(&arg.expr);
                    }
                    return;
                }

                // CALL6 already picked the receiver type. Use it rather than
                // widening to every method sharing the bare name — that pulled
                // in unrelated stdlib bodies and lowered them out of context.
                // Only user-defined receivers steer reachability here. A stdlib
                // receiver is recorded too, but its body isn't a user function
                // to enqueue — those keep the name-based path below.
                let dispatched = self.typed
                    .and_then(|typed| match typed.call_targets.get(&expr.id) {
                        Some(callee @ Callee::Method { method, .. }) => callee
                            .recv_type_id()
                            .map(|id| (id, typed.types.type_name(id), method.clone())),
                        _ => None,
                    });

                if let Some((type_id, type_name, method_name)) = dispatched {
                    let mut qualified = format!("{}_{}", type_name, method_name);
                    // A `{x}` and a `{x:>10}` both need the receiver's own
                    // rendering — `to_string`, or `message` for an error type
                    // that gets Displayable from it (std.fmt/D5). Neither name
                    // is what the call says, so resolve it here and record the
                    // answer for lowering; nothing downstream can see which of
                    // the two a given type actually defines.
                    if method_name == "to_string" || method_name == "__fmt" {
                        let renderer = [
                            format!("{}_to_string", type_name),
                            format!("{}_message", type_name),
                        ]
                        .into_iter()
                        .find(|name| self.method_table.contains_key(name));
                        if let Some(name) = renderer.filter(|n| *n != qualified) {
                            qualified = name.clone();
                            self.call_rewrites.insert(expr.id, name);
                        }
                    }
                    match self.contested_owner(&qualified) {
                        Some(owner) if owner != type_id => {
                            self.ambiguous_methods.push(AmbiguousMethod {
                                type_name,
                                method: method_name,
                                span: expr.span,
                            });
                        }
                        _ => {
                            // A method on a generic type gets one body per receiver
                            // instantiation, so `One<Big>.get()` and `One<i64>.get()`
                            // don't share a `self` layout. Receiver arguments come
                            // first, then the method's own — `instantiation_params`
                            // reads the two lists back in that order (#814).
                            let recv_args = if self.has_instantiable_body(&qualified) {
                                // `Box.new("hei")` is dispatched here too — the
                                // checker records the owning type as the receiver —
                                // but the object is the *type name*, so it carries
                                // no instantiation. The call's own result type does,
                                // when the method returns the type it's declared
                                // on. Without it the constructor stayed on the
                                // shared placeholder layout while
                                // `Box<string>.get()` got a per-instantiation one,
                                // and the value one wrote the other read back at the
                                // wrong field size (#820).
                                let from_recv = self.receiver_type_args(object.id, &qualified);
                                if from_recv.is_empty() {
                                    self.owner_type_args_from_result(expr.id, &qualified)
                                } else {
                                    from_recv
                                }
                            } else {
                                Vec::new()
                            };
                            let own_args = self.own_type_args(&qualified, type_args.clone());
                            let type_args: Vec<Type> =
                                recv_args.into_iter().chain(own_args).collect();
                            // A method with type parameters gets one body per set
                            // of arguments, same as a generic function — so the
                            // call has to name the copy. Only where there's a body
                            // to instantiate: a stdlib stub like `Map<K, V>.len()`
                            // is generic in its signature and resolves to one C
                            // entry point, so mangling it produced a call to
                            // `Map_len$string_string` that nothing emits.
                            if !type_args.is_empty() && self.has_instantiable_body(&qualified) {
                                self.call_rewrites.insert(
                                    expr.id,
                                    mangle_name(&qualified, &type_args),
                                );
                            }
                            self.enqueue(qualified, type_args);
                        }
                    }
                } else {
                    // Static method call: Type.method() → enqueue "Type_method"
                    // Cross-package call: pkg.func() → enqueue "func" (the function
                    // is registered under its original name from the dependency).
                    if let ExprKind::Ident(name) = &object.kind {
                        if self.package_modules.contains(name) {
                            self.enqueue(method.clone(), type_args.clone());
                        } else {
                            self.enqueue(format!("{}_{}", name, method), type_args.clone());
                        }
                        // `json.decode<JsonValue>` lowers to a call to
                        // `json.parse` — same job, already written in Rask — so
                        // that body has to be reachable even though the source
                        // never names it.
                        if name == "json"
                            && method == "decode"
                            && written_type_args
                                .as_ref()
                                .and_then(|t| t.first())
                                .map(|t| t.trim() == "JsonValue")
                                .unwrap_or(false)
                        {
                            self.enqueue("json_parse".to_string(), Vec::new());
                        }
                    }

                    // Receiver type unknown here — enqueue every method with this
                    // bare name and let the unused ones fall out.
                    if let Some(qualified_names) = self.method_by_bare_name.get(method) {
                        for qname in qualified_names.clone() {
                            self.enqueue(qname, type_args.clone());
                        }
                    }
                }

                self.visit_expr(object);
                for arg in args {
                    self.visit_expr(&arg.expr);
                }
            }
            ExprKind::Binary { left, right, .. } => {
                self.visit_expr(left);
                self.visit_expr(right);
            }
            ExprKind::Unary { operand, .. } => {
                self.visit_expr(operand);
            }
            ExprKind::Block(stmts) => {
                for stmt in stmts {
                    self.visit_stmt(stmt);
                }
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                self.visit_expr(cond);
                self.visit_expr(then_branch);
                if let Some(else_br) = else_branch {
                    self.visit_expr(else_br);
                }
            }
            ExprKind::IfLet {
                expr,
                then_branch,
                else_branch,
                ..
            } => {
                self.visit_expr(expr);
                self.visit_expr(then_branch);
                if let Some(else_br) = else_branch {
                    self.visit_expr(else_br);
                }
            }
            ExprKind::GuardPattern {
                expr, else_branch, ..
            } => {
                self.visit_expr(expr);
                self.visit_expr(else_branch);
            }
            ExprKind::IsPattern { expr, .. } => {
                self.visit_expr(expr);
            }
            ExprKind::Match { scrutinee, arms } => {
                self.visit_expr(scrutinee);
                for arm in arms {
                    self.visit_expr(&arm.body);
                    if let Some(guard) = &arm.guard {
                        self.visit_expr(guard);
                    }
                }
            }
            ExprKind::Try { expr: e } | ExprKind::Take { place: e } => self.visit_expr(e),
            ExprKind::Catch { value, ref clause } => {
                self.visit_expr(value);
                self.visit_expr(&clause.body);
            }
            ExprKind::IsPresent { expr: e, .. } => self.visit_expr(e),
            ExprKind::Unwrap { expr: e, .. } => self.visit_expr(e),
            ExprKind::NullCoalesce { value, default } => {
                self.visit_expr(value);
                self.visit_expr(default);
            }
            ExprKind::Field { object, .. } | ExprKind::OptionalField { object, .. } => {
                self.visit_expr(object);
            }
            ExprKind::DynamicField { object, field_expr } => {
                self.visit_expr(object);
                self.visit_expr(field_expr);
            }
            ExprKind::Index { object, index } => {
                self.visit_expr(object);
                self.visit_expr(index);
            }
            ExprKind::StructLit { fields, spread, .. } => {
                for field in fields {
                    self.visit_expr(&field.value);
                }
                if let Some(s) = spread {
                    self.visit_expr(s);
                }
            }
            ExprKind::Array(elems) => {
                for elem in elems {
                    self.visit_expr(elem);
                }
            }
            ExprKind::ArrayRepeat { value, count } => {
                self.visit_expr(value);
                self.visit_expr(count);
            }
            ExprKind::Tuple(elems) => {
                for elem in elems {
                    self.visit_expr(elem);
                }
            }
            ExprKind::Range { start, end, .. } => {
                if let Some(s) = start {
                    self.visit_expr(s);
                }
                if let Some(e) = end {
                    self.visit_expr(e);
                }
            }
            ExprKind::Closure { body, .. } => {
                self.visit_expr(body);
            }
            ExprKind::Cast { expr: inner, ty } => {
                // TR5: `value as any Trait` boxes `value` — pull in the vtable's methods.
                if let Some(trait_name) = rask_ast::traits::trait_object_name(ty) {
                    self.mark_trait_object_methods(trait_name);
                }
                self.visit_expr(inner);
            }
            ExprKind::Convert { expr, .. } => self.visit_expr(expr),
            ExprKind::Spawn { body } | ExprKind::Unsafe { body } | ExprKind::Comptime { body }
            | ExprKind::Loop { body, .. } => {
                for s in body {
                    self.visit_stmt(s);
                }
            }
            ExprKind::UsingBlock { args, body, .. } => {
                for arg in args {
                    self.visit_expr(&arg.expr);
                }
                for s in body {
                    self.visit_stmt(s);
                }
            }
            ExprKind::WithAs { bindings, body } => {
                for binding in bindings {
                    self.visit_expr(&binding.source);
                }
                for s in body {
                    self.visit_stmt(s);
                }
            }
            ExprKind::BlockCall { body, .. } => {
                for s in body {
                    self.visit_stmt(s);
                }
            }
            ExprKind::Select { arms, .. } => {
                for arm in arms {
                    self.visit_expr(&arm.body);
                }
            }
            ExprKind::Assert { condition, message }
            | ExprKind::Check { condition, message } => {
                self.visit_expr(condition);
                if let Some(msg) = message {
                    self.visit_expr(msg);
                }
            }
            // Leaves - no sub-expressions
            ExprKind::Int(..)
            | ExprKind::Float(..)
            | ExprKind::String(_) | ExprKind::StringInterp(_)
            | ExprKind::Char(_)
            | ExprKind::Bool(_)
            | ExprKind::Null
            | ExprKind::None => {}

            // A bare name that resolves to a function is a reference to it —
            // `http.serve(addr, handle)` passes the handler this way.
            // Treated as a leaf, the function was never marked reachable, so
            // nothing emitted it and MIR lowering reported the name as an
            // unresolved variable.
            ExprKind::Ident(name) => {
                // Only a plain top-level function. A generic one has nothing to
                // instantiate from here — a bare name carries no type arguments
                // — and enqueuing it with none produced a call to the
                // uninstantiated `T_greet` that nothing emits.
                if self.is_plain_fn(name) {
                    self.enqueue(name.clone(), Vec::new());
                }
            }
        }
    }
}

