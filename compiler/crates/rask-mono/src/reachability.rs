// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Monomorphization driver - reachability-driven instantiation.
//!
//! Walks the call graph from main(), instantiating generic functions on demand.
//! This is the core loop per spec rules M1-M4:
//!   M1: Walk reachable code from main()
//!   M2: Instantiate each unique (function_id, [type_args])
//!   M3: Compute layouts (done after this pass)
//!   M4: Transitive - new instantiations may discover more calls

use crate::MonoFunction;
use rask_ast::{
    decl::{Decl, DeclKind, FnDecl},
    expr::{Expr, ExprKind},
    stmt::{Stmt, StmtKind},
};
use rask_ast::NodeId;
use rask_ast::ty::TypeExpr;
use rask_types::{Type, TypeBinding, TypeDef, TypeId};
use rask_check::{Callee, TypedProgram};
use std::collections::{HashMap, HashSet, VecDeque};

/// Monomorphization work item
struct WorkItem {
    name: String,
    /// Every type parameter this copy fixes, named. Owner's first, then the
    /// method's own — the order the symbol name is built from.
    type_args: Vec<TypeBinding>,
    /// How many generic instantiations in a row demanded this one (CT68).
    depth: usize,
}

/// ctrl.comptime/CT68: an instantiation chain longer than this is an error.
/// Polymorphic recursion (`nest<T>` calling `nest<Vec<T>>`) makes one that
/// never ends; anything legitimate is far shorter.
pub const INSTANTIATION_LIMIT: usize = 64;

/// Generate a mangled name for a generic function instantiation.
/// e.g., ("render_children", [Inline]) → "render_children$Inline"
///
/// Bindings carry the checker's types, ids and all; this is the one place they
/// become names, because a symbol is text (#1393).
pub fn mangle_name(base: &str, type_args: &[TypeBinding], types: Option<&rask_check::TypeTable>) -> String {
    if type_args.is_empty() {
        return base.to_string();
    }
    let args_str: Vec<String> = type_args
        .iter()
        .map(|b| {
            let named = types
                .and_then(|t| Monomorphizer::nameable_type(&b.ty, t))
                .unwrap_or_else(|| b.ty.clone());
            symbol_spelling(&named)
        })
        .collect();
    format!("{}${}", base, args_str.join("_"))
}

/// One type argument as it appears inside a symbol name.
///
/// A written instantiation flattens: `Wrap<i64>` becomes `Wrap$i64`,
/// `Pair<i64, Big>` becomes `Pair$i64$Big`, so a symbol name never holds a
/// `<` (#871).
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
    /// Every function's symbol by its declaration, and what a method call
    /// runs (`crate::Symbols`).
    pub symbols: crate::Symbols,
    bound_symbols: HashMap<NodeId, String>,
    /// Resolved type args per call site (from typechecker)
    call_type_args: &'a HashMap<NodeId, Vec<TypeBinding>>,
    /// Type checker output, when monomorphizing a real program. The standalone
    /// reachability tests build a Monomorphizer without it and keep the
    /// conservative bare-name behaviour.
    typed: Option<&'a TypedProgram>,
    /// External package module names — `pkg.func()` enqueues `func`, not `pkg_func`
    package_modules: std::collections::HashSet<String>,
    /// Already processed (name, type_args) pairs
    /// Keyed by the symbol an item is emitted under. Two spellings of one type
    /// (`Version` by id and by name) are one instance.
    seen: HashMap<String, bool>,
    /// BFS work queue
    queue: VecDeque<WorkItem>,
    /// The item whose body is being walked: its symbol and chain depth, so
    /// what it enqueues knows who demanded it.
    current: Option<(String, usize)>,
    /// Which instantiation demanded each generic one, by symbol, for the
    /// chain CT68 prints.
    demanded_by: HashMap<String, String>,
    /// Each generic instance as a person reads it (`nest<Vec<i32>>`), by
    /// symbol, for the message.
    shown: HashMap<String, String>,
    /// Set when a chain passed `INSTANTIATION_LIMIT`: the generic instances
    /// in it as a person reads them, outermost first. Reachability stops
    /// there.
    pub too_deep: Option<Vec<String>>,
    /// Resulting instantiated functions
    pub results: Vec<MonoFunction>,
    /// Call expression NodeId → mangled callee name.
    /// Used by MIR lowering to rewrite calls to generic function instantiations.
    pub call_rewrites: HashMap<NodeId, String>,
    /// The key `hash`/`eq` each map-building call needs (`note_map_key_fns`).
    pub map_key_fns: HashMap<NodeId, crate::MapKeyFns>,
    /// The field defaults each decoded struct calls (`note_decode_defaults`).
    pub decode_defaults: HashMap<(String, String), String>,
    /// Type → the `generate` that builds it as a claim input (type.claims/IN2).
    pub claim_generators: HashMap<String, String>,
    /// Node ids handed out to instantiated copies. Starts above every id the
    /// original program used, so a copy's nodes can never be mistaken for the
    /// nodes they were cloned from.
    /// True while walking a `test`/`benchmark` block's synthesized body. A `try`
    /// there ends the test rather than propagating (std.testing/T20), and the
    /// message it reports needs its `message()` body queued like `r!` does.
    in_test_body: bool,
    /// The records of the generic instance being worked out: what the
    /// checker recorded for its body's nodes, with its arguments in. Moved
    /// into its `MonoFunction` when the instance is done.
    records: crate::InstanceCalls,
    /// Whether the function being walked is a generic instance, whose records
    /// are its own rather than the program's.
    in_instance: bool,
    /// The type arguments of the instance being walked; empty for a function
    /// that isn't generic.
    instance_args: Vec<TypeBinding>,
    instance_projections: HashMap<(String, String), Type>,
    /// Interface name → object-compatible method names (TR1–TR3).
    /// A vtable references a slot per compatible method, so boxing a value as
    /// `any Interface` makes every such method of the concrete type reachable even
    /// if it's never called explicitly.
    interface_methods: HashMap<String, Vec<String>>,
    /// Expression NodeId → interface name, for implicit TR5 coercion sites (a value
    /// passed where `any Interface` is expected, with no written-out cast).
    interface_coercions: HashMap<NodeId, String>,
    /// All declarations, for roots that don't come from a function body
    /// (module-level `const` initializers).
    decls: &'a [Decl],
    /// Types with a type parameter that carries an interface bound
    /// (`BufferedReader<R: Reader>`). Their methods call through that
    /// parameter, so a body only exists per instantiation — see the widening
    /// in `visit_expr`.
    bounded_generic_owners: HashSet<String>,
    /// Every vtable a box site needs (`mark_boxed`).
    pub vtables: HashMap<(Type, String), crate::VTable>,
}

use crate::symbols::MethodOwner;

/// The owner an `extend` header writes: `extend One<A, B>` gives `One` with
/// `["A", "B"]`. `None` when the header carries no arguments.
fn header_owner(target: &TypeExpr) -> Option<MethodOwner> {
    let mut params: Vec<String> = Vec::new();
    for arg in target.args() {
        // A tuple argument binds each of its members, not itself.
        match arg {
            TypeExpr::Tuple(members) => params.extend(members.iter().map(|m| m.to_string())),
            other => params.push(other.to_string()),
        }
    }
    if params.is_empty() || target.name().is_none() {
        return None;
    }
    Some(MethodOwner { params, template: target.clone() })
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

/// A method with `self`'s type written down.
///
/// `self` parses as `self: Self` and nothing substituted it, so MIR lowering
/// recovered the receiver's type by splitting the mangled function name at its
/// first underscore — `Document_delete_line` → `Document`. That is only right
/// while no type name contains an underscore. A dependency's declarations
/// carry the package they came from (`Helper_liba`), so all of them do, and
/// `Helper_liba_describe` read its receiver as a type called `Helper`: `self.n`
/// found no such field and fell back to `i64`, printing the receiver's address
/// where the field's value belongs (#1129).
///
/// Both places that file a method under `Type_method` know the type, so both
/// write it in — the bare name, generic owner included. A per-receiver copy in
/// `run` replaces it with the instantiated type; the template is laid out by
/// the shared layout. Keeping `Self` for a generic owner left the name split as
/// the only answer, and a private stdlib enum (`HandleList_async`) read as
/// `HandleList`.
fn with_self_type(method: &FnDecl, owner: &TypeExpr) -> FnDecl {
    let mut method = method.clone();
    if let Some(p) = method.params.first_mut() {
        if p.name == "self" && p.ty.as_ref().is_some_and(|t| t.is_name("Self")) {
            p.ty = Some(owner.clone());
        }
    }
    method
}

/// OR4: `Mul<f64>` and `Mul<Meters>` on one type both call their method
/// `mul`, so a conformance's operator method goes by the name with the applied
/// argument in it, the name the checker files it under.
fn operator_filed_name(decl: &Decl, method: &FnDecl, conformance_interfaces: &HashMap<NodeId, String>) -> Option<String> {
    let DeclKind::Impl(i) = &decl.kind else { return None };
    let (t, iface) = i.interface.as_ref().zip(conformance_interfaces.get(&decl.id))?;
    rask_stdlib::operators::conformance_method_name(&i.target_ty, t, iface, &method.name)
}

/// Wrap a method FnDecl as a top-level Decl and register it under its
/// qualified name (Type_method).
fn register_method(
    target: &TypeExpr,
    method: &FnDecl,
    parent_decl: &Decl,
    method_table: &mut HashMap<String, Decl>,
    method_owners: &mut HashMap<String, MethodOwner>,
    owner_params: &HashMap<String, Vec<String>>,
) {
    let Some(type_name) = target.name() else { return };
    let qualified = format!("{}_{}", type_name, method.name);
    // `extend Wrapper<T>` says its parameters in the header. `extend Wrapper`
    // doesn't, and the owning declaration is where they live — the header is
    // allowed to leave them out. Without this fallback such a method had no
    // parameters to bind, so it never got a per-receiver copy: one shared
    // `Wrapper_get` returning a word served `Wrapper<f64>` (which read the
    // double's bits as an integer) and `Wrapper<string>` (which segfaulted on a
    // 16-byte value in an 8-byte return slot) alike (#916).
    let owner = header_owner(target).or_else(|| {
        owner_params
            .get(&type_name)
            .filter(|params| !params.is_empty())
            .map(|params| MethodOwner {
                template: TypeExpr::generic(
                    type_name.clone(),
                    params.iter().map(TypeExpr::named).collect(),
                ),
                params: params.clone(),
            })
    });
    // The owner's bare name, generic or not: a per-receiver copy in `run`
    // replaces it with the instantiated type, and the template itself is laid
    // out by the shared layout that name finds.
    let method = with_self_type(method, &TypeExpr::named(type_name.as_str()));
    if let Some(owner) = owner {
        method_owners.insert(qualified.clone(), owner);
    }
    let wrapped = Decl {
        id: parent_decl.id,
        kind: DeclKind::Fn(method.clone()),
        span: parent_decl.span,
    };
    method_table.insert(qualified, wrapped);
}

impl<'a> Monomorphizer<'a> {
    /// `conformance_interfaces` is the checker's: each `implements` block's
    /// interface by its own name, which decides how an operator conformance's
    /// methods are filed (OR4).
    pub fn new(
        decls: &'a [Decl],
        call_type_args: &'a HashMap<NodeId, Vec<TypeBinding>>,
        conformance_interfaces: &HashMap<NodeId, String>,
    ) -> Self {
        let mut fn_table: HashMap<String, &Decl> = HashMap::new();
        let mut method_table = HashMap::new();
        let mut method_owners: HashMap<String, MethodOwner> = HashMap::new();

        // Type parameters per declared type, by bare name — PC1, so a struct
        // that never wrote `<T>` still reports the parameters its field types
        // imply. `register_method` falls back to this when an `extend` header
        // leaves them out.
        let mut owner_params: HashMap<String, Vec<String>> = HashMap::new();
        for decl in decls {
            let (name, params) = match &decl.kind {
                DeclKind::Struct(s) => (&s.name, rask_ast::type_params::struct_type_param_names(s)),
                DeclKind::Enum(e) => (&e.name, rask_ast::type_params::enum_type_param_names(e)),
                _ => continue,
            };
            if params.is_empty() {
                continue;
            }
            let bare = name.trim().to_string();
            owner_params.insert(bare, params);
        }

        let bounded_generic_owners: HashSet<String> = decls
            .iter()
            .filter_map(|decl| match &decl.kind {
                DeclKind::Struct(s) if s.type_params.iter().any(|p| !p.bounds.is_empty()) => {
                    Some(s.name.trim().to_string())
                }
                DeclKind::Enum(e) if e.type_params.iter().any(|p| !p.bounds.is_empty()) => {
                    Some(e.name.trim().to_string())
                }
                _ => None,
            })
            .collect();


        for decl in decls {
            match &decl.kind {
                DeclKind::Fn(f) => {
                    // One entry per declaration. A program function named like
                    // a stdlib one keeps the plain name and the stdlib's goes
                    // by a qualified symbol, so each call reaches the body the
                    // checker resolved it to. One entry per *name* kept
                    // whichever came last, and a program's own `clamp` was
                    // compiled as calls to the stdlib's (#1706).
                    let symbol = match fn_table.get(&f.name) {
                        Some(other) if other.id != decl.id => {
                            let std_symbol = rask_types::conformance_symbol(&f.name, "std");
                            if rask_stdlib::is_stdlib_span(decl.span) {
                                std_symbol
                            } else {
                                let stdlib_one = fn_table.remove(&f.name).expect("checked above");
                                fn_table.insert(std_symbol, stdlib_one);
                                f.name.clone()
                            }
                        }
                        _ => f.name.clone(),
                    };
                    fn_table.insert(symbol, decl);
                }
                DeclKind::Struct(s) => {
                    for method in &s.methods {
                        register_method(
                            &TypeExpr::named(s.name.as_str()), method, decl,
                            &mut method_table,
                            &mut method_owners, &owner_params,
                        );
                    }
                }
                DeclKind::Enum(e) => {
                    for method in &e.methods {
                        register_method(
                            &TypeExpr::named(e.name.as_str()), method, decl,
                            &mut method_table,
                            &mut method_owners, &owner_params,
                        );
                    }
                }
                DeclKind::Impl(i) => {
                    for method in &i.methods {
                        // OR4: `Mul<f64>` and `Mul<Meters>` on one type both
                        // call their method `mul`, and one `Meters_mul` symbol
                        // between them would mean the second body overwrote the
                        // first. The applied argument goes into the name, the
                        // same rule the checker files them under.
                        let filed;
                        let method = match operator_filed_name(decl, method, conformance_interfaces) {
                            Some(name) => {
                                filed = FnDecl { name, ..method.clone() };
                                &filed
                            }
                            None => method,
                        };
                        register_method(
                            &i.target_ty, method, decl,
                            &mut method_table,
                            &mut method_owners, &owner_params,
                        );
                    }
                }
                _ => {}
            }
        }

        // TR1–TR3: object-compatible methods per interface — a method drops out if
        // it declares its own type params (TR3) or returns Self (TR2), matching
        // the vtable layout in codegen.
        let mut interface_methods: HashMap<String, Vec<String>> = HashMap::new();
        for decl in decls {
            if let DeclKind::Interface(t) = &decl.kind {
                let compatible = t.methods.iter()
                    .filter(|m| m.type_params.is_empty()
                        && !m.ret_ty.as_ref().is_some_and(|t| t.is_name("Self")))
                    .map(|m| m.name.clone())
                    .collect();
                interface_methods.insert(t.name.clone(), compatible);
            }
        }
        // Compiler-provided interfaces have no declaration to read. Without an
        // entry, boxing a value as `any Error` marked none of the concrete
        // type's `message` bodies reachable, so the vtable slot pointed at a
        // function nobody had emitted (#708). Method names only — the
        // signatures live in rask-types, and the concrete bodies are found by
        // bare name below, same as for a declared interface.
        for name in rask_check::COMPILER_PROVIDED_TRAITS {
            interface_methods
                .entry(name.to_string())
                .or_insert_with(|| rask_check::builtin_interface_method_names(name));
        }

        Self {
            fn_table,
            method_table,
            call_type_args,
            typed: None,
            package_modules: std::collections::HashSet::new(),
            seen: HashMap::new(),
            queue: VecDeque::new(),
            current: None,
            demanded_by: HashMap::new(),
            shown: HashMap::new(),
            too_deep: None,
            results: Vec::new(),
            call_rewrites: HashMap::new(),
            bound_symbols: HashMap::new(),
            map_key_fns: HashMap::new(),
            decode_defaults: HashMap::new(),
            claim_generators: HashMap::new(),
            in_test_body: false,
            records: crate::InstanceCalls::default(),
            instance_args: Vec::new(),
            instance_projections: HashMap::new(),
            in_instance: false,
            interface_methods,
            interface_coercions: HashMap::new(),
            decls,
            symbols: crate::Symbols { by_decl: HashMap::new(), owners: method_owners, instantiable: HashSet::new() },
            bounded_generic_owners,
            vtables: HashMap::new(),
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
        let mut mono = Self::new(decls, &typed.call_type_args, &typed.conformance_interfaces);
        mono.typed = Some(typed);
        // One method list per interface declaration, keyed the way MIR and the
        // vtables name it. Read off the declarations by name, a program's
        // `interface Writer` and the stdlib's shared one entry (#1426).
        mono.interface_methods = rask_check::interface_vtable_methods(&typed.types);
        mono.bind_methods_by_type(typed);
        mono.symbols.by_decl = mono.symbols_by_decl();
        mono.symbols.instantiable = mono
            .method_table
            .values()
            .chain(mono.fn_table.values().copied())
            .filter_map(|d| match &d.kind {
                DeclKind::Fn(f) if !f.body.is_empty() => Some(f.id),
                _ => None,
            })
            .collect();
        mono
    }

    /// Each function and method declaration's symbol. A method of a type
    /// takes the one `bind_methods_by_type` filed it under; the rest are
    /// registered under one name each.
    fn symbols_by_decl(&self) -> HashMap<NodeId, String> {
        let mut out: HashMap<NodeId, String> = self.bound_symbols.clone();
        let mut entries: Vec<(&String, &Decl)> =
            self.method_table.iter().chain(self.fn_table.iter().map(|(k, d)| (k, *d))).collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        for (name, decl) in entries {
            let DeclKind::Fn(f) = &decl.kind else { continue };
            out.entry(f.id).or_insert_with(|| name.clone());
        }
        out
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
            let self_owner = TypeExpr::named(type_name.as_str());

            for decl_id in decl_ids {
                let Some(decl) = by_id.get(&decl_id) else { continue };
                // XC5: a block whose methods another package also declares on
                // this type gets the package in its symbol, so both bodies are
                // emitted instead of the later block's overwriting the earlier.
                // The checker decides which blocks those are and the call site
                // asks for the same name.
                let suffix = typed.conformance_disambiguation.get(&decl_id);
                for method in methods_of(decl) {
                    let name = operator_filed_name(decl, method, &typed.conformance_interfaces)
                        .unwrap_or_else(|| method.name.clone());
                    let plain = format!("{}_{}", type_name, name);
                    let qualified = match suffix {
                        Some(pkg) => rask_types::conformance_symbol(&plain, pkg),
                        None => plain,
                    };
                    let body = Decl {
                        id: decl.id,
                        kind: DeclKind::Fn(with_self_type(method, &self_owner)),
                        span: decl.span,
                    };
                    self.bound_symbols.insert(method.id, qualified.clone());
                    self.method_table.insert(qualified, body);
                }
            }
        }
    }

    /// Record implicit interface-coercion sites (TR5) from the type checker.
    pub fn set_interface_coercions(&mut self, coercions: &HashMap<NodeId, String>) {
        self.interface_coercions = coercions.clone();
    }

    /// The vtable boxing `ty` as `any interface_name` points at: the boxed
    /// type's own body for each of the interface's methods. An instance of a
    /// type that `needs_instantiation` gets the instance's body:
    /// `io.copy(reader, …)` with a `BufferedReader<Buffer>` needs
    /// `BufferedReader_read$Buffer`.
    fn mark_boxed(&mut self, ty: Type, interface_name: &str) {
        let Some(typed) = self.typed else { return };
        let key = (ty, interface_name.to_string());
        if self.vtables.contains_key(&key) {
            return;
        }
        let ty = &key.0;
        let Some(methods) = self.interface_methods.get(interface_name).cloned() else { return };
        let mut slots = Vec::with_capacity(methods.len());
        for method in methods {
            // No body is left empty, and codegen reports the slot by name.
            let Some(qualified) = typed
                .types
                .method_body(ty, &method, None)
                .and_then(|decl| self.symbols.by_decl.get(&decl).cloned())
            else {
                slots.push(crate::VTableSlot { method, symbol: String::new() });
                continue;
            };
            let bindings: Vec<TypeBinding> = match ty {
                Type::Generic { base, args }
                    if self.bounded_generic_owners.contains(&typed.types.type_name(*base)) =>
                {
                    match self.symbols.owners.get(&qualified) {
                        Some(owner) => owner
                            .params
                            .iter()
                            .zip(args.iter())
                            .filter_map(|(param, arg)| match arg {
                                rask_types::GenericArg::Type(t) => Some(TypeBinding::new(param.clone(), t.as_ref().clone())),
                                _ => None,
                            })
                            .collect(),
                        None => Vec::new(),
                    }
                }
                _ => Vec::new(),
            };
            let symbol = if bindings.is_empty() {
                qualified.clone()
            } else {
                mangle_name(&qualified, &bindings, Some(&typed.types))
            };
            slots.push(crate::VTableSlot { method, symbol });
            self.enqueue(qualified, bindings);
        }
        let name = format!(".vtable.{}.{}", self.vtables.len(), interface_name);
        self.vtables.insert(key, crate::VTable { name, slots });
    }

    /// The vtable for the value at `id`, boxed as `any interface_name`.
    fn mark_boxed_value(&mut self, id: NodeId, interface_name: &str) {
        if let Some(ty) = self.node_type(id) {
            self.mark_boxed(ty, interface_name);
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
    /// Every non-generic top-level function with a body, as a root. For a file
    /// with no `main` — a test-only file — where an analysis pass still needs
    /// layouts and call targets.
    pub fn add_all_plain_fn_roots(&mut self) {
        for decl in self.decls {
            let DeclKind::Fn(f) = &decl.kind else { continue };
            if !f.body.is_empty() && f.type_params.is_empty() {
                self.enqueue(f.name.clone(), Vec::new());
            }
        }
    }

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


    /// Every function a test or benchmark runner calls (`RUNNER_ENTRY`).
    pub fn add_runner_entries(&mut self) {
        let entries: Vec<String> = self
            .fn_table
            .iter()
            .filter(|(_, d)| matches!(&d.kind, DeclKind::Fn(f) if f.attrs.iter().any(|a| a.is(crate::RUNNER_ENTRY))))
            .map(|(symbol, _)| symbol.clone())
            .collect();
        for symbol in entries {
            self.enqueue(symbol, Vec::new());
        }
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
        let Some(TypeExpr::Result { err, .. }) = &f.ret_ty else { return };
        // `A | B` — every arm can be the one that reaches main.
        let arms = match err.as_ref() {
            TypeExpr::Union(arms) => arms.clone(),
            one => vec![one.clone()],
        };
        for arm in arms {
            // A module-qualified error, `cli.CliError`, has its methods under
            // the type's own name: the module is how the program reached it.
            let qualified = matches!(&arm, TypeExpr::Named { path, args } if path.len() > 1 && args.is_empty());
            let name = match arm.last_segment() {
                Some(last) if qualified => format!("{last}_message"),
                _ => format!("{}_message", arm),
            };
            if self.method_table.contains_key(&name) {
                self.enqueue(name, Vec::new());
            }
        }
    }


    /// Where a call's instance goes: the instance being walked has its own,
    /// since every instance shares the body's node ids.
    fn set_rewrite(&mut self, call: NodeId, symbol: String) {
        let map = if self.in_instance { &mut self.records.call_rewrites } else { &mut self.call_rewrites };
        map.insert(call, symbol);
    }

    fn map_key_fns_here(&mut self) -> &mut HashMap<NodeId, crate::MapKeyFns> {
        if self.in_instance { &mut self.records.map_key_fns } else { &mut self.map_key_fns }
    }

    /// Run until fixpoint: process queue, instantiate, discover more calls
    pub fn run(&mut self) {
        while let Some(item) = self.queue.pop_front() {
            if self.too_deep.is_some() {
                break;
            }
            let key = mangle_name(&item.name, &item.type_args, self.typed.map(|t| &t.types));
            if let Some(visited) = self.seen.get(&key) {
                if *visited {
                    continue;
                }
            }
            self.seen.insert(key.clone(), true);
            self.current = Some((key, item.depth));
            self.records = crate::InstanceCalls::default();
            self.in_instance = !item.type_args.is_empty();
            self.instance_args = item.type_args.clone();

            let original = match self.fn_table.get(&item.name) {
                Some(decl) => *decl,
                None => match self.method_table.get(&item.name) {
                    Some(decl) => decl,
                    None => continue, // External or unknown function
                },
            };

            // Instantiate: if type_args present, clone AST with substitution.
            // Otherwise use original decl directly.
            // A generic body is lowered once per instance as it is; what
            // differs is read through the instance (`Instance`).
            let concrete = original.clone();
            let projections = if item.type_args.is_empty() {
                HashMap::new()
            } else {
                self.projections_for(&concrete, &item.type_args)
            };
            self.instance_projections = projections.clone();

            // Walk the concrete body to discover more calls (M4: transitive)
            if let DeclKind::Fn(fn_decl) = &concrete.kind {
                self.in_test_body = fn_decl.attrs.iter().any(|a| a.is("test_body"));
                for stmt in &fn_decl.body {
                    self.visit_stmt(stmt);
                }
                self.in_test_body = false;
            }

            let mangled = mangle_name(&item.name, &item.type_args, self.typed.map(|t| &t.types));
            self.results.push(MonoFunction {
                name: mangled,
                type_args: item.type_args,
                projections,
                calls: std::mem::take(&mut self.records),
                body: concrete,
            });
        }
    }

    /// AT6/AT8: what each `T.Out` a generic function writes reads on this
    /// instance. The projection goes through `T`'s bound — `T: Mul<f64>` asks
    /// the argument's `Mul<f64>` conformance, not whichever `Mul` it has — and
    /// is spelled into the copy like any other type. Left alone, `H.Out` reached
    /// lowering unresolved and became a pointer, so an `f64` came back as a
    /// truncated integer (#1365).
    fn projections_for(
        &self,
        original: &Decl,
        bindings: &[TypeBinding],
    ) -> HashMap<(String, String), Type> {
        let mut out = HashMap::new();
        let (Some(typed), DeclKind::Fn(f)) = (self.typed, &original.kind) else { return out };
        let types = &typed.types;
        let spelled = |ty: &Type| Self::nameable_type(ty, types).unwrap_or_else(|| ty.clone());
        let arg_of = |name: &str| bindings.iter().find(|b| b.param == name).map(|b| spelled(&b.ty).to_type_expr());
        for tp in &f.type_params {
            let Some(binding) = bindings.iter().find(|b| b.param == tp.name) else { continue };
            let bounds = tp.bound_types();
            for bound in &bounds {
                let Some(TypeDef::Interface { assoc_types, .. }) =
                    types.interface_decl(bound).and_then(|id| types.get(id))
                else {
                    continue;
                };
                // `T: Mul<U>` asks about the conformance to `Mul<` U's argument `>`.
                let applied = bound.substitute(&arg_of);
                for a in assoc_types {
                    if types.projection_bound(&bounds, &a.name) != Some(bound) {
                        continue;
                    }
                    if let Some(ty) = types.project(&binding.ty, &applied, &a.name) {
                        out.insert((tp.name.clone(), a.name.clone()), ty);
                    }
                }
            }
        }
        out
    }

    /// The resolved type arguments at a call site, at the instance being
    /// walked.
    fn type_args_at(&self, id: NodeId) -> Vec<TypeBinding> {
        let args = match self.instance() {
            Some(inst) => inst.call_type_args(id).unwrap_or_default(),
            None => self.call_type_args.get(&id).cloned().unwrap_or_default(),
        };
        // Kept as the checker typed them, ids and all: names are spelled where a
        // symbol or a copied declaration needs text (`mangle_name`, `run`), and
        // nowhere else (#1393).
        args
    }

    /// `Symbols::receiver_bindings` for a receiver type in hand.
    fn bindings_for(&self, recv: &Type, qualified: &str) -> Vec<TypeBinding> {
        match self.typed {
            Some(typed) => self.symbols.receiver_bindings(recv, qualified, &typed.types),
            None => Vec::new(),
        }
    }

    /// The same type, spelled so both a symbol name and a layout lookup can be
    /// built from it: every interned id replaced by the name it stands for.
    ///
    /// A `Type::Named` prints as `<type#7>` and a `Type::Generic` as
    /// `<type#84><i32>`, neither of which names anything downstream. `None` when
    /// some part of the type has no name at all — an inference variable, a type
    /// parameter still standing for itself.
    pub fn nameable_type(ty: &Type, types: &rask_check::TypeTable) -> Option<Type> {
        let bare = |n: &str| n.trim().to_string();
        match ty {
            Type::Named(id) => {
                let name = bare(&types.type_name(*id));
                (!name.is_empty()).then(|| Type::UnresolvedNamed(name))
            }
            Type::UnresolvedNamed(name) => {
                let name = bare(name);
                (!name.is_empty()).then(|| Type::UnresolvedNamed(name))
            }
            Type::Bool | Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128 | Type::Isize
            | Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128 | Type::Usize
            | Type::F32 | Type::F64 | Type::Char | Type::String | Type::Bytes => Some(ty.clone()),
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
            // A tuple whose members are all nameable is nameable. It has to be:
            // `extend Sequence<(K, V)>` binds its parameters through the
            // receiver's tuple element, so bailing here left the whole header
            // unbound and `to_map` shared one unmangled body (#1046).
            Type::Tuple(elems) => {
                let named: Option<Vec<Type>> =
                    elems.iter().map(|e| Self::nameable_type(e, types)).collect();
                Some(Type::Tuple(named?))
            }
            // A closure type is nameable when its parameters and return are.
            // Bailing here left `Sequence<func() -> i64>.to_vec()` on the one
            // shared unmangled body, whose vector was built for 8-byte
            // integers and so never freed the closures it held (#1386).
            Type::Fn { params, ret } => {
                let params: Option<Vec<rask_types::FnParam>> = params
                    .iter()
                    .map(|p| {
                        Some(rask_types::FnParam { mode: p.mode, ty: Self::nameable_type(&p.ty, types)? })
                    })
                    .collect();
                Some(Type::Fn {
                    params: params?,
                    ret: Box::new(Self::nameable_type(ret, types)?),
                })
            }
            Type::None | Type::Unit => Some(ty.clone()),
            _ => None,
        }
    }

    fn nameable_generic(
        head: &str,
        args: &[rask_types::GenericArg],
        types: &rask_check::TypeTable,
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
    /// The `{ErrType}_message` bodies a `r!` on this operand could need: one
    /// for a concrete error type, one per member for a union. Empty for an
    /// optional (`err` is `none`) and whenever any candidate has no body,
    /// since a switch that can't cover every member is worse than the message
    /// it replaces.
    fn forced_error_message_fns(&self, id: NodeId) -> Vec<String> {
        let Some(typed) = self.typed else { return Vec::new() };
        let Some(ty) = self.node_type(id)
        else {
            return Vec::new();
        };
        let ty = &ty;
        let Type::Result { err, .. } = ty else { return Vec::new() };
        if **err == Type::None {
            return Vec::new();
        }
        let members: Vec<&Type> = match &**err {
            Type::Union(types) => types.iter().collect(),
            other => vec![other],
        };
        let mut names = Vec::new();
        for member in members {
            // `receiver_name` answers "Result" for a nested result and a bare
            // primitive name for a primitive; neither can carry a `message()`,
            // and E0344 means neither is a legal error type anyway. Asking the
            // method table for a body settles all of it at once — including an
            // error type whose `message()` is derived (ER6), since the derive
            // runs in desugaring and is an ordinary method by now.
            let Some(name) = rask_check::receiver_name(member, &typed.types) else {
                return Vec::new();
            };
            let mangled = format!("{}_message", name);
            if !self.has_instantiable_body(&mangled) {
                return Vec::new();
            }
            names.push(mangled);
        }
        names
    }

    /// The element `compare` a `sort()` on this receiver will be lowered to.
    ///
    /// `sort()` is `T: Comparable` (std.collections/SO3), so a Vec of
    /// aggregates is ordered by the element type's own `compare` — and MIR
    /// lowering decides that, after this pass. So the body is queued from here,
    /// the same way `r!` queues an error type's `message`.
    ///
    /// It used to arrive by accident: the bare-name widening enqueued every
    /// `compare` in the program, this one among them. Narrowing that took the
    /// accident away and `sort()` fell back to comparing raw bytes — the
    /// elements came out ordered by whichever field landed in the first eight
    /// (#1062).
    fn sort_comparator_fn(&self, recv_id: NodeId) -> Option<String> {
        let typed = self.typed?;
        let ty = self.node_type(recv_id)?;
        let ty = &ty;
        let args = match ty {
            Type::Generic { base, args } if typed.types.type_name(*base) == "Vec" => args,
            Type::UnresolvedGeneric { name, args } if name == "Vec" => args,
            _ => return None,
        };
        let elem = match args.first()? {
            rask_types::GenericArg::Type(inner) => inner,
            _ => return None,
        };
        let name = rask_check::receiver_name(elem, &typed.types)?;
        let mangled = format!("{}_compare", name);
        self.method_table.contains_key(&mangled).then_some(mangled)
    }

    /// A call that hands back a `Map` whose key compares through its own `eq`
    /// and `hash` — a struct, an enum, a `Vec` — needs both bodies, and nothing
    /// in the source calls them: the map does, from the runtime. So they're
    /// queued from here, and the names they were emitted under are recorded
    /// for lowering to take the addresses of (#1391). The same reason
    /// `sort_comparator_fn` exists.
    ///
    /// Every call is asked rather than only `Map.new`: a map comes out of
    /// `Map.from`, `with_capacity`, a generic constructor like `Set.new` whose
    /// body builds one, and whatever comes next. Lowering looks the record up
    /// where it builds a map, so a call that only passes one along costs a
    /// lookup nobody reads.
    fn note_map_key_fns(&mut self, id: NodeId) {
        let Some(typed) = self.typed else { return };
        let Some(ty) = self.node_type(id) else {
            return;
        };
        let ty = &ty;

        let key = match ty {
            Type::Generic { base, args } if typed.types.type_name(*base) == "Map" => args.first(),
            Type::UnresolvedGeneric { name, args } if name == "Map" => args.first(),
            _ => None,
        };
        let Some(rask_types::GenericArg::Type(key)) = key else { return };
        let key = key.as_ref().clone();
        let Some(fns) = self.key_fns(&key) else { return };
        self.map_key_fns_here().insert(id, fns);
    }

    /// The `hash` and `eq` a map with this key calls, queued, or `None` when
    /// the key's bytes are its identity (an integer, a string, a link: the
    /// runtime has those).
    fn key_fns(&mut self, key: &Type) -> Option<crate::MapKeyFns> {
        let typed = self.typed?;
        let name_of = |reach: &mut Self, method: &str| -> Option<String> {
            match key {
                Type::Named(id) if matches!(
                    typed.types.get(*id),
                    Some(rask_types::TypeDef::Struct { .. } | rask_types::TypeDef::Enum { .. })
                ) => {
                    let qualified = format!("{}_{method}", rask_check::receiver_name(key, &typed.types)?);
                    reach.method_table.contains_key(&qualified).then(|| {
                        reach.enqueue(qualified.clone(), Vec::new());
                        qualified
                    })
                }
                // A tuple, optional or result key: the checker wrote its pair
                // as free functions (`TypedProgram::wrapper_fns`).
                Type::Tuple(_) | Type::Result { .. } => {
                    let fns = typed.wrapper_fns.iter().find(|w| &w.ty == key)?;
                    let name = if method == "hash" { fns.hash.clone()? } else { fns.eq.clone()? };
                    reach.enqueue(name.clone(), Vec::new());
                    Some(name)
                }
                Type::Generic { base, .. } if typed.types.type_name(*base) == "Vec" => {
                    let qualified = format!("Vec_{method}");
                    let bindings = reach.bindings_for(key, &qualified);
                    if bindings.is_empty() {
                        return None;
                    }
                    let mangled = mangle_name(&qualified, &bindings, Some(&typed.types));
                    reach.enqueue(qualified, bindings);
                    Some(mangled)
                }
                _ => None,
            }
        };
        let hash = name_of(self, "hash")?;
        let eq = name_of(self, "eq")?;
        Some(crate::MapKeyFns { hash, eq })
    }

    /// Queue the field defaults a `json.decode` call may run, and record which
    /// method is which. Nothing in the source calls them: the decoder does,
    /// for a field the input leaves out (std.encoding/E20).
    fn note_decode_defaults(&mut self, id: NodeId) {
        let ty = self.node_type(id);
        let Some(Type::Result { ok, .. }) = ty else { return };
        let ok = ok.as_ref().clone();
        self.queue_decode_defaults(&ok, &mut HashSet::new());
    }

    fn queue_decode_defaults(&mut self, ty: &Type, seen: &mut HashSet<rask_types::TypeId>) {
        let Some(typed) = self.typed else { return };
        match ty {
            Type::Named(id) => {
                if !seen.insert(*id) {
                    return;
                }
                // A type that builds itself is built by its `generate`, and
                // its fields are that function's business.
                if typed.types.builds_claim_inputs(*id) {
                    let name = typed.types.type_name(*id);
                    let method = format!("{name}_generate");
                    if self.method_table.contains_key(&method) {
                        self.enqueue(method.clone(), Vec::new());
                        self.claim_generators.insert(name, method);
                    }
                    return;
                }
                let (name, fields) = match typed.types.get(*id) {
                    Some(rask_types::TypeDef::Struct { name, fields, .. }) => (name, fields),
                    // An enum's payloads are built too (claims only; a decode
                    // never reaches one).
                    Some(rask_types::TypeDef::Enum { variants, .. }) => {
                        for (_, payload) in variants.clone() {
                            for t in payload {
                                self.queue_decode_defaults(&t, seen);
                            }
                        }
                        return;
                    }
                    _ => return,
                };
                for (field, field_ty) in fields {
                    let method = format!(
                        "{name}_{}",
                        rask_ast::decl::field_attrs::decode_default_method(field)
                    );
                    if self.method_table.contains_key(&method) {
                        self.enqueue(method.clone(), Vec::new());
                        self.decode_defaults.insert((name.clone(), field.clone()), method);
                    }
                    self.queue_decode_defaults(field_ty, seen);
                }
            }
            Type::Generic { args, .. } | Type::UnresolvedGeneric { args, .. } => {
                for arg in args {
                    if let rask_types::GenericArg::Type(t) = arg {
                        self.queue_decode_defaults(t, seen);
                    }
                }
            }
            // A claim builds either side of a `T or E`.
            Type::Result { ok, err } => {
                self.queue_decode_defaults(ok, seen);
                self.queue_decode_defaults(err, seen);
            }
            Type::Tuple(elems) => {
                for e in elems {
                    self.queue_decode_defaults(e, seen);
                }
            }
            _ => {}
        }
    }

    /// Whether the `json.decode` call `id` builds a `JsonValue`: the ok side of
    /// its type, which is what lowering reads to choose `json.parse`.
    /// `json.decode<JsonValue>` lowers to a call to `json.parse` (same job,
    /// already written in Rask), so that body has to be reachable even though
    /// the source never names it. Any other `T` needs its decode defaults.
    ///
    /// Keyed on the checker's target, not on how the call was spelled: a
    /// syntactic `json.decode` test sat in one branch of the method-call walk,
    /// and a call that reached another branch, or `import json as j`, got a
    /// `json_parse` nobody compiled (#1435, #1600). Asked of the checker's type
    /// for the call too, as lowering asks it: the written argument is a
    /// spelling, and `json.JsonValue` didn't match one.
    fn note_json_decode(&mut self, id: NodeId) {
        let is_decode = matches!(
            self.call_target(id),
            Some(Callee::Method { recv: Type::UnresolvedNamed(m), method, .. })
                if m == "json" && method == "decode"
        );
        if !is_decode {
            return;
        }
        if self.decodes_json_value(id) {
            self.enqueue("json_parse".to_string(), Vec::new());
        } else {
            self.note_decode_defaults(id);
        }
    }

    /// `claims.input<T>` builds a `T` the way a decode would, so a field the
    /// wire form leaves out takes its default here too (type.claims/IN1).
    fn note_claim_input(&mut self, id: NodeId) {
        let Some(typed) = self.typed else { return };
        // `claims.input<T>`, and `Choices.make<T>()` in a generator, which
        // builds a `T` the same way.
        let is_input = match self.call_target(id) {
            Some(Callee::Method { recv, method, .. }) => matches!(
                (rask_check::receiver_name(&recv, &typed.types).as_deref(), method.as_str()),
                (Some("claims"), "input") | (Some("Choices"), "make")
            ),
            _ => false,
        };
        if !is_input {
            return;
        }
        let Some(ty) = self.node_type(id) else {
            return;
        };
        let ty = &ty;
        self.queue_decode_defaults(&ty, &mut HashSet::new());
    }

    fn decodes_json_value(&self, id: NodeId) -> bool {
        let Some(typed) = self.typed else { return false };
        let ty = self.node_type(id);
        match &ty {
            Some(rask_types::Type::Result { ok, .. }) => {
                rask_check::receiver_name(ok, &typed.types).as_deref() == Some("JsonValue")
            }
            _ => false,
        }
    }

    fn arg_type_name(&self, id: NodeId) -> Option<String> {
        let typed = self.typed?;
        let ty = self.node_type(id)?;
        let ty = &ty;
        rask_check::receiver_name(ty, &typed.types)
    }

    /// Add a (name, type_args) pair to queue if not already seen
    /// What the checker dispatched a call to. A node in an instantiated copy
    /// has its own record, with the receiver already concrete; the generic
    /// body's record still names the type parameter. Reading only the latter
    /// is what widened `self[i].hash()` inside `Vec_hash$i64` to every `hash`
    /// there is, the generic `Vec_hash` among them, which then had no `T`.
    fn call_target(&self, id: NodeId) -> Option<rask_check::Callee> {
        match self.instance() {
            Some(inst) => inst.call_target(id),
            None => self.typed?.call_targets.get(&id).cloned(),
        }
    }

    /// The type of a node in the body being walked, at the instance being
    /// walked.
    fn node_type(&self, id: NodeId) -> Option<Type> {
        match self.instance() {
            Some(inst) => inst.node_type(id),
            None => self.typed?.node_types.get(&id).cloned(),
        }
    }

    /// The instance being walked, when it is one.
    fn instance(&self) -> Option<crate::Instance<'_>> {
        let typed = self.typed?;
        (!self.instance_args.is_empty())
            .then(|| crate::Instance::new(&self.instance_args, &self.instance_projections, typed))
    }


    fn enqueue(&mut self, name: String, type_args: Vec<TypeBinding>) {
        let key = mangle_name(&name, &type_args, self.typed.map(|t| &t.types));
        if self.seen.contains_key(&key) {
            return;
        }
        // CT68: a generic instance demanded by a generic instance is one
        // step deeper in a chain; anything else starts a new one.
        if !type_args.is_empty() {
            let args: Vec<String> = type_args
                .iter()
                .map(|b| match self.typed {
                    Some(t) => t.types.resolve_type_names(&b.ty).to_string(),
                    None => b.ty.to_string(),
                })
                .collect();
            self.shown.insert(key.clone(), format!("{}<{}>", name, args.join(", ")));
        }
        let depth = match (&self.current, type_args.is_empty()) {
            (Some((parent, d)), false) => {
                self.demanded_by.insert(key.clone(), parent.clone());
                d + 1
            }
            _ => 0,
        };
        if depth > INSTANTIATION_LIMIT {
            if self.too_deep.is_none() {
                let mut chain = vec![key.clone()];
                let mut at = key.clone();
                while let Some(parent) = self.demanded_by.get(&at) {
                    chain.push(parent.clone());
                    at = parent.clone();
                }
                chain.reverse();
                // Only the generic links read as a chain; the root that
                // started it (`main`) is where it was reached from, not part
                // of it.
                let chain = chain.iter().filter_map(|k| self.shown.get(k).cloned()).collect();
                self.too_deep = Some(chain);
            }
            return;
        }
        self.seen.insert(key, false);
        self.queue.push_back(WorkItem { name, type_args, depth });
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
        // TR5: implicit coercion to `any Interface` (function arg, field, element)
        // with no written-out cast — the checker flags these by NodeId.
        if let Some(interface_name) = self.interface_coercions.get(&expr.id).cloned() {
            self.mark_boxed_value(expr.id, &interface_name);
        }
        if matches!(expr.kind, ExprKind::Call { .. } | ExprKind::MethodCall { .. }) {
            self.note_map_key_fns(expr.id);
            self.note_json_decode(expr.id);
            self.note_claim_input(expr.id);
        }

        match &expr.kind {
            ExprKind::Call { func, args } => {
                // The checker said what the call runs: a function by its
                // symbol, or for `sleep(d)` after `import time.sleep` the
                // module's function (#1359). `make<i32>(2)`'s written
                // arguments are in `type_args_at` (#712). MIR resolves the
                // same call with the same `Symbols`.
                let type_args = self.type_args_at(expr.id);
                let resolved = match (self.typed, self.call_target(expr.id)) {
                    (Some(typed), Some(callee)) => self
                        .symbols
                        .free_call(&callee, type_args.clone(), typed)
                        .or_else(|| self.symbols.method_call(&callee, type_args, &typed.types)),
                    _ => None,
                };
                if let Some(call) = resolved {
                    self.enqueue(call.qualified, call.type_args);
                }
                // A name being called isn't a function value; anything else in
                // that position (a closure, a field holding one) is walked.
                if !matches!(func.kind, ExprKind::Ident(_) | ExprKind::GenericName { .. }) {
                    self.visit_expr(func);
                }
                for arg in args {
                    self.visit_expr(&arg.expr);
                }
            }
            ExprKind::MethodCall { object, method, args, .. } => {
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
                    self.set_rewrite(expr.id, body.clone());
                    self.enqueue(body, Vec::new());
                    for arg in args {
                        self.visit_expr(&arg.expr);
                    }
                    return;
                }

                // The checker said which declaration the call runs, and an
                // instance says it for a call on its type parameter. The
                // symbol is that declaration's; no name is built here.
                // `sort()` on a Vec of aggregates needs the element type's own
                // `compare`, and nothing in the source names it — see
                // `sort_comparator_fn`. Before either branch below, because a
                // `Vec` receiver resolves through the first one and a
                // still-generic one through the second.
                if method == "sort" {
                    if let Some(cmp) = self.sort_comparator_fn(object.id) {
                        self.enqueue(cmp, Vec::new());
                    }
                }

                match self.call_target(expr.id) {
                    Some(callee @ Callee::Method { body: Some(_), .. }) => {
                        // A native has a declaration and no body to compile.
                        // MIR asks `Symbols::method_call` the same question.
                        if let Some(call) = self
                            .typed
                            .and_then(|t| self.symbols.method_call(&callee, type_args.clone(), &t.types))
                        {
                            self.enqueue(call.qualified, call.type_args);
                        }
                    }
                    // A union of error types switches on its member at run
                    // time, so each member's own body is reachable.
                    Some(Callee::Method { recv: Type::Union(members), method: m, package, body: None }) => {
                        for member in &members {
                            let body = self.typed.and_then(|t| t.types.method_body(member, &m, package.as_deref()));
                            if let Some(symbol) = body.and_then(|d| self.symbols.by_decl.get(&d).cloned()) {
                                self.enqueue(symbol, Vec::new());
                            }
                        }
                    }
                    // A primitive's operation or a builtin the compiler lowers
                    // itself: nothing to generate.
                    Some(Callee::Method { body: None, .. }) => {}
                    // Not a method the checker dispatched: `Type.method()` on a
                    // type the checker routed elsewhere, or `pkg.func()`
                    // reaching another package's function by its own name.
                    _ => {
                        if let Some(name) = object.name() {
                            if self.package_modules.contains(name) {
                                self.enqueue(method.clone(), type_args.clone());
                            } else {
                                self.enqueue(format!("{}_{}", name, method), type_args.clone());
                            }
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
            ExprKind::Try { expr: e } => {
                // T20: a `try` in a test block ends that test rather than
                // propagating, and what it reports is the error's own
                // `message()`. Same shape as `r!` above — the name is decided
                // here, where bodies are queued, and MIR reads the rewrite.
                // Only in a test body: anywhere else the error goes to the
                // caller and nothing here needs to print it.
                if self.in_test_body {
                    let names = self.forced_error_message_fns(e.id);
                    if !names.is_empty() {
                        self.set_rewrite(expr.id, names.join("|"));
                        for name in names {
                            self.enqueue(name, Vec::new());
                        }
                    }
                }
                // ER32: the error leaves boxed as the function's `any I`.
                if let Some(erased) = self.typed.and_then(|t| t.erased_try_errors.get(&expr.id)) {
                    let error = match self.instance() {
                        Some(inst) => inst.ty(&erased.error),
                        None => Some(erased.error.clone()),
                    };
                    if let Some(error) = error {
                        self.mark_boxed(error, &erased.interface);
                    }
                }
                self.visit_expr(e)
            }
            ExprKind::Take { place: e } => self.visit_expr(e),
            ExprKind::Catch { value, ref clause } => {
                self.visit_expr(value);
                self.visit_expr(&clause.body);
            }
            ExprKind::IsPresent { expr: e, .. } => self.visit_expr(e),
            ExprKind::Unwrap { expr: e, message, .. } => {
                // ER15: `r!` panics *using* the error's `message()`. Lowering
                // can't name that method itself — the same lesson as
                // `json.encode` above, which came out of codegen as "Function
                // not found: JsonValue_to_string" because nothing had queued
                // the body. So the name is decided here, where bodies are
                // queued, and MIR reads the rewrite (#1009).
                //
                // Only when no custom message was written (`r! "msg"` says
                // what to print), and only for a concrete error type: a union
                // needs a switch on the member, which is more than a rewrite
                // can carry.
                if message.is_none() {
                    let names = self.forced_error_message_fns(e.id);
                    if !names.is_empty() {
                        // A union error contributes one per member. The rewrite
                        // carries the whole set joined by `|`, which no mangled
                        // name can contain, and lowering matches each member
                        // against it — so the two never have to agree on the
                        // order members come out in.
                        self.set_rewrite(expr.id, names.join("|"));
                        for name in names {
                            self.enqueue(name, Vec::new());
                        }
                    }
                }
                self.visit_expr(e)
            }
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
            ExprKind::Closure { body, .. } | ExprKind::Spawn { body, .. } => {
                self.visit_expr(body);
            }
            ExprKind::Cast { expr: inner, ty } => {
                // TR5: `value as any Interface` boxes `value` — pull in the vtable's methods.
                if let TypeExpr::Any(interface) = ty {
                    self.mark_boxed_value(inner.id, &interface.to_string());
                }
                self.visit_expr(inner);
            }
            ExprKind::Convert { expr, .. } => self.visit_expr(expr),
            ExprKind::Unsafe { body } | ExprKind::Comptime { body }
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
            | ExprKind::String(_) | ExprKind::StringInterp(_) | ExprKind::Bytes(_)
            | ExprKind::Char(_)
            | ExprKind::Bool(_)
            | ExprKind::None => {}

            // A bare name that resolves to a function is a reference to it —
            // `http.serve(addr, handle)` passes the handler this way.
            // Treated as a leaf, the function was never marked reachable, so
            // nothing emitted it and MIR lowering reported the name as an
            // unresolved variable.
            ExprKind::Ident(_) | ExprKind::GenericName { .. } => {
                // A generic one is instantiated where it's named: the checker
                // recorded its type arguments under this node, the same as for
                // a call, and MIR reads the rewrite to point the function value
                // at that copy. Enqueuing one with no arguments produced a
                // call to the uninstantiated `T_greet` that nothing emits.
                let type_args = self.type_args_at(expr.id);
                if let Some(f) = self.typed.and_then(|t| self.symbols.function_value(expr.id, type_args, t)) {
                    self.enqueue(f.qualified, f.type_args);
                }
            }
        }
    }
}

/// Every expression and statement node of a declaration's body: what an
/// instance has records for.
pub(crate) fn body_nodes(decl: &Decl) -> Vec<NodeId> {
    struct Collect(Vec<NodeId>);
    impl rask_ast::rewrite::Rewrite for Collect {
        fn expr(&mut self, e: &mut rask_ast::expr::Expr) {
            self.0.push(e.id);
        }
        fn body(&mut self, b: &mut Vec<rask_ast::stmt::Stmt>) {
            self.0.extend(b.iter().map(|s| s.id));
        }
    }
    let mut c = Collect(Vec::new());
    rask_ast::rewrite::rewrite_decl(&mut decl.clone(), &mut c);
    c.0
}
