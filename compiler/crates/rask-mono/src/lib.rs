// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Monomorphization pass - eliminates generics by instantiating concrete copies.
//!
//! Takes type-checked AST and produces monomorphized program with:
//! - Concrete function instances for each unique (function_id, [type_args])
//! - Computed memory layouts for all structs and enums
//! - Reachability analysis starting from main()

pub mod drop_names;
pub mod abi;
mod instantiate;
mod layout;
mod reachability;

pub use instantiate::instantiate_function;
pub use layout::{
    arg_owns_storage, compute_enum_layout, compute_struct_layout, compute_union_layout,
    is_stdlib_span, ordering_layout, field_type, type_size_align,
    EnumLayout, FieldLayout, LayoutCache, StructLayout, VariantLayout,
};
pub use reachability::{mangle_name, Monomorphizer};

use rask_ast::decl::{Decl, DeclKind};
use rask_ast::NodeId;
use rask_ast::ty::TypeExpr;
use rask_types::{Type, TypeBinding, TypedProgram};
use std::collections::{HashMap, HashSet, VecDeque};

/// Monomorphized program with all generics eliminated
/// The symbols a map calls to hash and compare its keys.
#[derive(Debug, Clone)]
pub struct MapKeyFns {
    pub hash: String,
    pub eq: String,
}

pub struct MonoProgram {
    pub functions: Vec<MonoFunction>,
    pub struct_layouts: Vec<StructLayout>,
    pub enum_layouts: Vec<EnumLayout>,
    /// The checker's name for each type id. A resolved type carries an id, and
    /// whatever needs to know what it is — which handle a field owns, say —
    /// reads the name here rather than off the type's rendering.
    pub type_names: HashMap<rask_types::TypeId, String>,
    /// Call expression NodeId → mangled callee name for generic function calls.
    pub call_rewrites: HashMap<NodeId, String>,
    /// Calls that build a `Map` whose key compares through its own `eq`/`hash`,
    /// and the functions that are (#1391).
    pub map_key_fns: HashMap<NodeId, MapKeyFns>,
    /// Types and dispatch targets for the nodes of instantiated generic bodies.
    ///
    /// Those nodes don't exist in the checker's output — they were created
    /// here — so without these, every lookup inside an instantiated body misses
    /// and lowering falls back to guessing from AST shape.
    pub instantiated_node_types: HashMap<NodeId, Type>,
    pub instantiated_call_targets: HashMap<NodeId, rask_types::Callee>,
    /// OR1: operator calls a conformance answered, in instantiated bodies.
    pub instantiated_operator_targets: HashMap<NodeId, rask_types::OperatorTarget>,
    /// ER31a: `try` sites in instantiated bodies that wrap their error, same idea.
    pub instantiated_error_wraps: HashMap<NodeId, rask_types::ErrorWrap>,
    /// ER14a: instantiated `??` nodes whose right side is still wrapped.
    pub instantiated_fallback_keeps_shape: HashSet<NodeId>,
    pub instantiated_escaping_closures: HashSet<NodeId>,
    pub instantiated_field_reuses: HashSet<NodeId>,
    /// Closures in instantiated bodies that may not reach another task, each
    /// decided from its copy's concrete capture types (#1356).
    pub instantiated_task_bound_closures: HashSet<NodeId>,
}

/// Everything recorded per node, for the whole program: the checker's records
/// plus the ones monomorphization carried onto instantiated bodies.
///
/// Lowering runs after monomorphization and sees both kinds of node, so it
/// wants one map of each. The two sets of ids are disjoint by construction:
/// instantiation allocates above everything the checker used. One struct so
/// no pipeline can take one merged table and forget the next; the native run
/// path used to lower with the checker's own `escaping_closures`, and so knew
/// nothing about a closure in a generic body.
pub struct NodeRecords {
    pub node_types: HashMap<NodeId, Type>,
    pub call_targets: HashMap<NodeId, rask_types::Callee>,
    /// OR1: operator calls a conformance answered.
    pub operator_targets: HashMap<NodeId, rask_types::OperatorTarget>,
    /// ER31a: `try` sites that wrap their error.
    pub error_wraps: HashMap<NodeId, rask_types::ErrorWrap>,
    /// ER14a: `??` sites that keep the optional shape.
    pub fallback_keeps_shape: HashSet<NodeId>,
    /// CM1: closure literals that outlive the frame that built them.
    pub escaping_closures: HashSet<NodeId>,
    /// Assignments whose new value takes the old one (ownership).
    pub field_reuses: HashSet<NodeId>,
    /// Closure literals that may not reach another task (#1356).
    pub task_bound_closures: HashSet<NodeId>,
}

impl NodeRecords {
    /// The checker's records alone, for a program with no instantiated bodies.
    pub fn from_typed(typed: &TypedProgram) -> Self {
        Self {
            node_types: typed.node_types.clone(),
            call_targets: typed.call_targets.clone(),
            operator_targets: typed.operator_targets.clone(),
            error_wraps: typed.error_wraps.clone(),
            fallback_keeps_shape: typed.fallback_keeps_shape.clone(),
            escaping_closures: typed.escaping_closures.clone(),
            field_reuses: typed.field_reuses.clone(),
            task_bound_closures: typed.task_bound_closures.clone(),
        }
    }
}

impl MonoProgram {
    /// The checker's records merged with the ones carried onto instantiated
    /// bodies.
    pub fn node_records(&self, typed: &TypedProgram) -> NodeRecords {
        let mut r = NodeRecords::from_typed(typed);
        r.node_types.extend(self.instantiated_node_types.iter().map(|(k, v)| (*k, v.clone())));
        r.call_targets.extend(self.instantiated_call_targets.iter().map(|(k, v)| (*k, v.clone())));
        r.operator_targets
            .extend(self.instantiated_operator_targets.iter().map(|(k, v)| (*k, v.clone())));
        r.error_wraps.extend(self.instantiated_error_wraps.iter().map(|(k, v)| (*k, v.clone())));
        r.fallback_keeps_shape.extend(self.instantiated_fallback_keeps_shape.iter().copied());
        r.escaping_closures.extend(self.instantiated_escaping_closures.iter().copied());
        r.field_reuses.extend(self.instantiated_field_reuses.iter().copied());
        r.task_bound_closures.extend(self.instantiated_task_bound_closures.iter().copied());
        r
    }
}

/// Monomorphized function instance
pub struct MonoFunction {
    pub name: String,
    /// The type parameters this copy fixes, each named by the parameter it binds.
    pub type_args: Vec<TypeBinding>,
    pub body: Decl,
}

/// The type names a field's layout waits on.
///
/// Only what the field holds by value. A builtin box or collection is a
/// pointer, so its size is known before what it points at; a program's generic
/// waits only on the arguments it holds inline (`inline_params`). Following
/// every argument made `enum Expr { Neg(Wrapped) }` and
/// `struct Wrapped { items: Vec<Expr> }` a cycle, the cycle fell back to
/// source order, and `Expr` was laid out before `Wrapped` had a size: an
/// 8-byte guess for a 32-byte payload (#1436).
///
/// A program's generic with its arguments also names its instance layout
/// (`One$Big`), which has to exist before a type holding a `One<Big>` is sized
/// against it (#1444).
fn collect_type_deps(
    ty: &Type,
    inline_params: &HashMap<String, Vec<bool>>,
    type_names: &HashMap<rask_types::TypeId, String>,
    out: &mut HashSet<String>,
) {
    let go = |t: &Type, out: &mut HashSet<String>| collect_type_deps(t, inline_params, type_names, out);
    let generic = |name: &str, args: &[rask_types::GenericArg], out: &mut HashSet<String>| {
        if layout::generic_is_one_word(name) {
            return;
        }
        out.insert(name.to_string());
        let arg_tys: Vec<Type> = args
            .iter()
            .filter_map(|a| match a {
                rask_types::GenericArg::Type(t) => Some((**t).clone()),
                _ => None,
            })
            .collect();
        if arg_tys.len() == args.len() {
            if let Some(instance) = generic_instance_name(name, &arg_tys, type_names) {
                out.insert(instance);
            }
        }
        let inline = inline_params.get(name);
        for (i, arg) in args.iter().enumerate() {
            let held = inline.map_or(true, |flags| flags.get(i).copied().unwrap_or(true));
            if let (true, rask_types::GenericArg::Type(inner)) = (held, arg) {
                go(inner, out);
            }
        }
    };
    match ty {
        Type::UnresolvedNamed(name) => {
            out.insert(name.clone());
        }
        Type::Named(id) => {
            if let Some(name) = type_names.get(id) {
                out.insert(name.clone());
            }
        }
        Type::UnresolvedGeneric { name, args } => generic(name, args, out),
        Type::Generic { base, args } => {
            if let Some(name) = type_names.get(base) {
                generic(name, args, out);
            }
        }
        Type::Result { ok, err } => {
            go(ok, out);
            go(err, out);
        }
        Type::Tuple(elems) => {
            for e in elems {
                go(e, out);
            }
        }
        Type::Array { elem, .. } => go(elem, out),
        _ => {}
    }
}

/// For each generic declaration, which of its parameters it holds by value
/// somewhere in its fields — not only behind a pointer. `List<T> { items:
/// Vec<T> }` holds none; `One<T> { v: T }` holds its one. A parameter passed
/// on to another of the program's generics counts as held, since that one
/// may hold it.
fn inline_type_params(decls: &[Decl]) -> HashMap<String, Vec<bool>> {
    let mut out = HashMap::new();
    for decl in decls {
        let (name, params, fields): (&str, Vec<String>, Vec<&TypeExpr>) = match &decl.kind {
            DeclKind::Struct(s) => (
                s.name.as_str(),
                rask_types::struct_type_param_names(s),
                s.fields.iter().map(|f| &f.ty).collect(),
            ),
            DeclKind::Enum(e) => (
                e.name.as_str(),
                rask_types::enum_type_param_names(e),
                e.variants.iter().flat_map(|v| v.fields.iter().map(|f| &f.ty)).collect(),
            ),
            _ => continue,
        };
        if params.is_empty() {
            continue;
        }
        let mut held = HashSet::new();
        for ty in fields {
            collect_type_deps(&layout::field_type(ty), &HashMap::new(), &HashMap::new(), &mut held);
        }
        out.insert(name.to_string(), params.iter().map(|p| held.contains(p)).collect());
    }
    out
}

/// Every layout a declaration list defines on its own, plus the size/align
/// cache they were computed against.
///
/// One pass in dependency order, so a type holding another sees its real size
/// rather than a guess — generic ones included: `Group<T>` holding a
/// `Tasks<T>` needs the enum's size first. A generic declaration gets one
/// layout with a word standing in for each type parameter.
///
/// Public because the interpreter needs the same answers. `reflect.fields<T>()`
/// reports each field's offset and size, and the interpreter had no layouts at
/// all — it answered 0 for both while native answered the truth (#1104).
/// Computing them a second way there is how two backends drift; this is the
/// one that already exists.
///
/// What it leaves out is the per-*instantiation* layout, which needs the
/// checker's type table to know which instantiations a program reaches.
/// `monomorphize` hands those to `compute_layouts`.
pub fn compute_declared_layouts(
    decls: &[Decl],
) -> (Vec<StructLayout>, Vec<EnumLayout>, LayoutCache) {
    compute_layouts(decls, None)
}

/// What laying out a program's generic instantiations needs from the checker.
struct Instantiations<'a> {
    /// Each instantiation the program mentions, as base name and arguments.
    reached: Vec<(String, Vec<Type>)>,
    type_names: &'a HashMap<rask_types::TypeId, String>,
    types: &'a rask_types::TypeTable,
}

/// One layout to compute.
enum LayoutStep {
    Decl(usize),
    /// An instantiation of the generic declaration `decl`, laid out as `name`.
    Instance { decl: usize, args: Vec<Type>, name: String },
}

/// The declarations' layouts and, given `inst`, one per instantiation that
/// needs its own, all in one dependency order. A declaration and an
/// instantiation can each hold the other: `Holder { o: One<Big> }` is sized
/// against `One$Big`, which is sized against `Big`. Laying out instantiations
/// in a second pass after every declaration left `Holder` with the shared
/// one-word `One` for a 24-byte field (#1444).
fn compute_layouts(
    decls: &[Decl],
    inst: Option<&Instantiations>,
) -> (Vec<StructLayout>, Vec<EnumLayout>, LayoutCache) {
    let mut layout_cache = LayoutCache::new();
    let mut struct_layouts = Vec::new();
    let mut enum_layouts = Vec::new();

    let no_names = HashMap::new();
    let type_names = inst.map_or(&no_names, |i| i.type_names);
    let instances = inst.map_or_else(Vec::new, |i| instance_steps(decls, &i.reached, type_names));
    let concrete = concrete_type_names(decls);
    let inline_params = inline_type_params(decls);
    for step in layout_order(decls, instances, &inline_params, type_names) {
        let idx = match step {
            LayoutStep::Decl(idx) => idx,
            LayoutStep::Instance { decl, args, name } => {
                let inst = inst.expect("instantiations come with their inputs");
                lay_out_instance(
                    &decls[decl],
                    &args,
                    name,
                    inst,
                    &inline_params,
                    &mut layout_cache,
                    &mut struct_layouts,
                    &mut enum_layouts,
                );
                continue;
            }
        };
        let decl = &decls[idx];
        match &decl.kind {
            DeclKind::Struct(s) if s.type_params.is_empty() => {
                let layout = compute_struct_layout(decl, &[], &layout_cache);
                layout_cache.insert(s.name.clone(), (layout.size, layout.align));
                struct_layouts.push(layout);
            }
            DeclKind::Enum(e) if e.type_params.is_empty() => {
                let layout = compute_enum_layout(decl, &[], &layout_cache);
                layout_cache.insert(e.name.clone(), (layout.size, layout.align));
                enum_layouts.push(layout);
            }
            // The 8-byte-everything model means every scalar argument gives the
            // same field sizes, so a word stands in for each type parameter.
            DeclKind::Struct(s) => {
                let mut layout = layout::compute_shared_struct_layout(decl, &layout_cache);
                // Strip type params from name so struct literals ("Box") match
                let base_name = s.name.to_string();
                layout.name = base_name.clone();
                if !concrete.contains(&base_name) {
                    layout_cache.insert(base_name, (layout.size, layout.align));
                }
                struct_layouts.push(layout);
            }
            DeclKind::Enum(e) => {
                let mut layout = layout::compute_shared_enum_layout(decl, &layout_cache);
                let base_name = e.name.to_string();
                layout.name = base_name.clone();
                if !concrete.contains(&base_name) {
                    layout_cache.insert(base_name, (layout.size, layout.align));
                }
                enum_layouts.push(layout);
            }
            DeclKind::Union(u) => {
                let layout = compute_union_layout(decl, &layout_cache);
                layout_cache.insert(u.name.clone(), (layout.size, layout.align));
                struct_layouts.push(layout);
            }
            // A nominal newtype has the same layout as what it wraps — it's
            // transparent, so it needs no layout of its own, just an entry so
            // fields typed by it get the right size. Without this a
            // `type Name = string` field was sized 8 instead of 16 and the
            // struct's later fields overlapped it (#445).
            DeclKind::TypeAlias(a) if !a.is_transparent && a.type_params.is_empty() => {
                let (size, align) = type_size_align(
                    &layout::field_type(&a.target),
                    &layout_cache,
                );
                layout_cache.insert(a.name.clone(), (size, align));
            }
            _ => {}
        }
    }

    (struct_layouts, enum_layouts, layout_cache)
}

/// One layout per *instantiation*, where the shared one has the wrong shape.
/// The placeholder gives every type parameter a word, which is right for a
/// scalar and right for anything boxed (a `Vec`, a `Map`, a `Shared`) since
/// those are pointers. It is wrong for anything that *is* its bytes — a struct,
/// enum, union, tuple, array, or a `string`: `One<Big>` stored 24 bytes into an
/// 8-byte slot and segfaulted on the read back (#781).
///
/// Only where such an argument is held by value, so `One<i32>` and
/// `Stack<Big> { items: Vec<T> }` keep the shared layout. Not "only where the
/// instance comes out bigger": `Maybe<string> { r: T? or MyErr }` is as big as
/// the shared one, because the error side is the wider, but its `T?` is a
/// `string?` inside. Reading the field through the shared layout typed the
/// string as a word and printed its bytes as a number (#1445).
fn lay_out_instance(
    decl: &Decl,
    args: &[Type],
    instance_name: String,
    inst: &Instantiations,
    inline_params: &HashMap<String, Vec<bool>>,
    layout_cache: &mut LayoutCache,
    struct_layouts: &mut Vec<StructLayout>,
    enum_layouts: &mut Vec<EnumLayout>,
) {
    let base = match &decl.kind {
        DeclKind::Struct(s) => s.name.as_str(),
        DeclKind::Enum(e) => e.name.as_str(),
        _ => return,
    };
    let held = inline_params.get(base);
    // Only when an argument the type holds by value overflows the shared slot...
    let overflows = args.iter().enumerate().any(|(i, a)| {
        held.map_or(true, |flags| flags.get(i).copied().unwrap_or(true))
            && inline_arg_size(a, inst.type_names, inst.types, layout_cache).is_some_and(|size| size > 8)
    });
    // ...or when it owns storage. A container argument fits the shared word
    // fine, and that is the problem: the shared layout says `i64`, the release
    // walk reads the layout, and `Pair<i64, Vec<i64>>`'s vector was freed by
    // nobody. The instance layout names the real type, and this one is kept
    // whatever its size.
    //
    // User declarations only. The stdlib's own generics are runtime objects
    // behind an empty struct — there is no field to describe — and giving
    // `Map<string, Vec<i32>>` an instance layout renamed the type out from
    // under method dispatch: `Map$string$Vec$i32_index`, a function nobody
    // emitted.
    let owns = !is_stdlib_span(decl.span) && args.iter().any(|a| arg_owns_heap(a, inst.type_names));
    if !overflows && !owns {
        return;
    }
    // The type arguments have to be nameable to the layout code too —
    // `type_size_align` reads the cache by name, and a `Named(id)` isn't one. A
    // nested instantiation is named by its own instance layout.
    let named_args: Vec<Type> = args
        .iter()
        .map(|a| arg_as_cache_name(a, inst.type_names, layout_cache))
        .collect();
    match &decl.kind {
        DeclKind::Struct(_) => {
            let mut layout = compute_struct_layout(decl, &named_args, layout_cache);
            layout.name = instance_name.clone();
            layout_cache.insert(instance_name, (layout.size, layout.align));
            struct_layouts.push(layout);
        }
        DeclKind::Enum(_) => {
            let mut layout = compute_enum_layout(decl, &named_args, layout_cache);
            layout.name = instance_name.clone();
            layout_cache.insert(instance_name, (layout.size, layout.align));
            enum_layouts.push(layout);
        }
        _ => {}
    }
}

/// The reached instantiations of the program's generic declarations, one per
/// layout name.
fn instance_steps(
    decls: &[Decl],
    reached: &[(String, Vec<Type>)],
    type_names: &HashMap<rask_types::TypeId, String>,
) -> Vec<LayoutStep> {
    // PC1 counts: a single letter in a field or payload type makes the type
    // generic whether or not `<T>` was written. Gating on the explicit list
    // meant an implicit-param struct never got an instance layout at all, so a
    // `Pair<i32, string>` kept the shared one — where every parameter is a
    // single word — and its 16-byte string field was written into an 8-byte
    // slot (#913).
    let generic_decls: HashMap<&str, usize> = decls
        .iter()
        .enumerate()
        .filter_map(|(i, d)| match &d.kind {
            DeclKind::Struct(s) if !rask_types::struct_type_param_names(s).is_empty() => {
                Some((s.name.as_str(), i))
            }
            DeclKind::Enum(e) if !rask_types::enum_type_param_names(e).is_empty() => {
                Some((e.name.as_str(), i))
            }
            _ => None,
        })
        .collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (base, args) in reached {
        let Some(&decl) = generic_decls.get(base.as_str()) else { continue };
        let Some(name) = generic_instance_name(base, args, type_names) else { continue };
        if seen.insert(name.clone()) {
            out.push(LayoutStep::Instance { decl, args: args.clone(), name });
        }
    }
    out
}

fn concrete_type_names(decls: &[Decl]) -> HashSet<String> {
    decls
        .iter()
        .filter_map(|d| match &d.kind {
            DeclKind::Struct(s) if s.type_params.is_empty() => Some(s.name.clone()),
            DeclKind::Enum(e) if e.type_params.is_empty() => Some(e.name.clone()),
            _ => None,
        })
        .collect()
}

/// The field types a declaration's layout is computed from.
fn layout_field_types(decl: &Decl) -> Vec<&TypeExpr> {
    match &decl.kind {
        DeclKind::Struct(s) => s.fields.iter().map(|f| &f.ty).collect(),
        DeclKind::Enum(e) => e.variants.iter().flat_map(|v| v.fields.iter().map(|f| &f.ty)).collect(),
        DeclKind::Union(u) => u.fields.iter().map(|f| &f.ty).collect(),
        DeclKind::TypeAlias(a) => vec![&a.target],
        _ => vec![],
    }
}

/// Every struct/enum/union/newtype declaration and every instantiation in
/// `instances`, ordered so each comes after what its fields hold by value
/// (Kahn's algorithm). An instantiation waits on its fields with the arguments
/// written in: `One<Big>` waits on `Big`, `Group<string>` on `Tasks$string`.
/// A cycle falls back to source order, instantiations last.
fn layout_order(
    decls: &[Decl],
    instances: Vec<LayoutStep>,
    inline_params: &HashMap<String, Vec<bool>>,
    type_names: &HashMap<rask_types::TypeId, String>,
) -> Vec<LayoutStep> {
    let mut nodes: Vec<LayoutStep> = Vec::new();
    let mut by_key: HashMap<String, usize> = HashMap::new();

    // A concrete declaration owns its name over a generic one spelled the same:
    // a program's own `struct Wide` beside the stdlib's `Wide<T>`.
    let concrete = concrete_type_names(decls);
    for (i, decl) in decls.iter().enumerate() {
        let (name, generic) = match &decl.kind {
            DeclKind::Struct(s) => (s.name.as_str(), !s.type_params.is_empty()),
            DeclKind::Enum(e) => (e.name.as_str(), !e.type_params.is_empty()),
            DeclKind::Union(u) => (u.name.as_str(), false),
            // Nominal newtypes take part in the ordering: a struct with a field
            // typed by one needs the alias's size known first (#445).
            DeclKind::TypeAlias(a) if !a.is_transparent && a.type_params.is_empty() => {
                (a.name.as_str(), false)
            }
            _ => continue,
        };
        if !generic || !concrete.contains(name) {
            by_key.insert(name.to_string(), nodes.len());
        }
        nodes.push(LayoutStep::Decl(i));
    }
    for step in instances {
        if let LayoutStep::Instance { name, .. } = &step {
            by_key.insert(name.clone(), nodes.len());
        }
        nodes.push(step);
    }

    let mut deps: Vec<HashSet<usize>> = Vec::with_capacity(nodes.len());
    let mut rdeps: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for (n, node) in nodes.iter().enumerate() {
        let mut keys = HashSet::new();
        match node {
            LayoutStep::Decl(idx) => {
                for ty in layout_field_types(&decls[*idx]) {
                    collect_type_deps(&layout::field_type(ty), inline_params, type_names, &mut keys);
                }
            }
            LayoutStep::Instance { decl, args, .. } => {
                let params = match &decls[*decl].kind {
                    DeclKind::Struct(s) => rask_types::struct_type_param_names(s),
                    DeclKind::Enum(e) => rask_types::enum_type_param_names(e),
                    _ => Vec::new(),
                };
                let subst: HashMap<&str, &Type> =
                    params.iter().map(String::as_str).zip(args.iter()).collect();
                for ty in layout_field_types(&decls[*decl]) {
                    let ty = layout::substitute_inside(&layout::field_type(ty), &subst);
                    collect_type_deps(&ty, inline_params, type_names, &mut keys);
                }
            }
        }
        let mut node_deps = HashSet::new();
        for key in keys {
            if let Some(&dep) = by_key.get(&key) {
                if dep != n && node_deps.insert(dep) {
                    rdeps[dep].push(n);
                }
            }
        }
        deps.push(node_deps);
    }

    let mut queue: VecDeque<usize> = (0..nodes.len()).filter(|&n| deps[n].is_empty()).collect();
    let mut order = Vec::with_capacity(nodes.len());
    while let Some(n) = queue.pop_front() {
        order.push(n);
        for &dependent in &rdeps[n] {
            if deps[dependent].remove(&n) && deps[dependent].is_empty() {
                queue.push_back(dependent);
            }
        }
    }
    if order.len() < nodes.len() {
        let placed: HashSet<usize> = order.iter().copied().collect();
        order.extend((0..nodes.len()).filter(|n| !placed.contains(n)));
    }

    let mut slots: Vec<Option<LayoutStep>> = nodes.into_iter().map(Some).collect();
    order.into_iter().filter_map(|n| slots[n].take()).collect()
}

/// The layout name of one instantiation of a generic type — `One$Big`,
/// `Pair$i64$Big`.
///
/// `mangle_name` does this for functions, whose type arguments arrive already
/// normalized to names. A type argument here hasn't been: it can be a
/// `Named(TypeId)`, which prints as `<type#N>`. So the id gets resolved, and an
/// argument that can't be named at all means there is no instance layout to make
/// — the caller falls back to the shared placeholder layout.
///
/// MIR asks the same question of the same function, so the two agree by
/// construction rather than by convention.
pub fn generic_instance_name(
    base: &str,
    args: &[Type],
    type_names: &HashMap<rask_types::TypeId, String>,
) -> Option<String> {
    if args.is_empty() {
        return None;
    }
    let mut parts = Vec::with_capacity(args.len());
    for arg in args {
        parts.push(type_arg_key(arg, type_names)?);
    }
    Some(format!("{}${}", base, parts.join("$")))
}

/// The layout a written type is laid out by: `Tagged<string>` is
/// `Tagged$string` when that instance was made, else the shared `Tagged`.
/// Codegen describes what a value owns by reading layouts by name — so without
/// this a generic's nodes matched no layout and a `Heap` holding one freed
/// nothing inside it.
pub fn layout_name_for(ty: &Type, exists: impl Fn(&str) -> bool) -> String {
    let written = ty.to_string();
    if exists(&written) {
        return written;
    }
    if let Type::UnresolvedGeneric { name, args } = ty {
        let tys: Vec<Type> = args
            .iter()
            .filter_map(|a| match a {
                rask_types::GenericArg::Type(t) => Some((**t).clone()),
                _ => None,
            })
            .collect();
        if let Some(instance) = generic_instance_name(name, &tys, &HashMap::new()) {
            if exists(&instance) {
                return instance;
            }
        }
        let base = name.to_string();
        if exists(&base) {
            return base;
        }
    }
    written
}

/// The head name of a type argument, in whichever spelling it arrives in.
///
/// A resolved generic carries its base as a `TypeId` and renders as
/// `<type#7><i64>`, so `Display` is no use — the name comes from the table.
fn arg_head_name(ty: &Type, type_names: &HashMap<rask_types::TypeId, String>) -> Option<String> {
    match ty {
        Type::UnresolvedNamed(name) | Type::UnresolvedGeneric { name, .. } => {
            Some(name.to_string())
        }
        Type::Named(id) | Type::Generic { base: id, .. } => {
            type_names.get(id).map(|n| n.to_string())
        }
        _ => None,
    }
}

/// Does storing this type argument in a field make the aggregate responsible
/// for heap the shared layout wouldn't know about?
///
/// A container is a handle and a closure is a pointer to its block, so both fit
/// the shared word and both are invisible in it: the shared layout says `i64`,
/// the release walk believes it, and nobody gives the storage back. The
/// instance layout names the real type, which is what `container_free_for`
/// reads to pick `rask_vec_free` or `rask_closure_free`.
fn arg_owns_heap(ty: &Type, type_names: &HashMap<rask_types::TypeId, String>) -> bool {
    match ty {
        Type::Fn { .. } => true,
        _ => arg_head_name(ty, type_names).is_some_and(|h| arg_owns_storage(&h)),
    }
}

/// One type argument, spelled so it can key a layout. `None` for anything whose
/// identity isn't settled — an inference variable, an unresolved parameter name,
/// a shape with one of those inside.
fn type_arg_key(
    ty: &Type,
    type_names: &HashMap<rask_types::TypeId, String>,
) -> Option<String> {
    use rask_types::GenericArg;
    Some(match ty {
        Type::Bool | Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128
        | Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128
        | Type::F32 | Type::F64 | Type::Char | Type::String | Type::Unit => format!("{}", ty),
        Type::Named(id) => type_names.get(id)?.to_string(),
        // An argument substituted into an instantiated copy is named, not
        // interned — the copy's types were built by rewriting strings, not by
        // going back through the checker's table (#814).
        Type::UnresolvedNamed(name) => name.to_string(),
        Type::UnresolvedGeneric { name, args } => {
            let base = name.to_string();
            let mut parts = Vec::with_capacity(args.len());
            for arg in args {
                let GenericArg::Type(inner) = arg else { return None };
                parts.push(type_arg_key(inner, type_names)?);
            }
            format!("{}${}", base, parts.join("$"))
        }
        Type::Generic { base, args } => {
            let base = type_names.get(base)?.to_string();
            let mut parts = Vec::with_capacity(args.len());
            for arg in args {
                let GenericArg::Type(inner) = arg else { return None };
                parts.push(type_arg_key(inner, type_names)?);
            }
            format!("{}${}", base, parts.join("$"))
        }
        Type::Tuple(elems) => {
            let mut parts = Vec::with_capacity(elems.len());
            for elem in elems {
                parts.push(type_arg_key(elem, type_names)?);
            }
            format!("tup{}", parts.join("$"))
        }
        // A closure argument. The arity goes in the key because the parameters
        // and the return run together otherwise, and `func(i64, i64) -> void`
        // would key the same as `func(i64) -> func(i64) -> void`.
        Type::Fn { params, ret } => {
            let mut parts = Vec::with_capacity(params.len() + 1);
            // The mode is part of the type (FT1): `func(take T)` and
            // `func(T)` are two instantiations, not one.
            for p in params {
                let key = type_arg_key(&p.ty, type_names)?;
                parts.push(match p.mode.keyword() {
                    Some(kw) => format!("{}_{}", kw, key),
                    None => key,
                });
            }
            parts.push(type_arg_key(ret, type_names)?);
            format!("fn{}${}", params.len(), parts.join("$"))
        }
        // Spelled exactly as the source writes it, because MIR reaches the same
        // layout from a type *string* — `Wrap<i64?>` there splits into the
        // argument `i64?`, and the two have to agree on the key (#872).
        Type::Result { ok, err } if **err == Type::None => {
            format!("{}?", type_arg_key(ok, type_names)?)
        }
        Type::Result { ok, err } => format!(
            "{} or {}",
            type_arg_key(ok, type_names)?,
            type_arg_key(err, type_names)?,
        ),
        _ => return None,
    })
}

/// How wide this type argument is *inline*, when it's the kind of argument that
/// lives inline at all.
///
/// The shared placeholder layout gives every type parameter one word. A scalar
/// fits. A `Vec`, `Map` or any other box is a pointer, so it fits too. What
/// doesn't is anything that *is* its bytes: a struct, enum, union, tuple, array,
/// or a `string` — a string is 16 bytes of header, and a generic slot that only
/// holds the pointer to them needs a reading convention of its own at every site
/// that touches it. One of those sites didn't have it, so a `string` payload in a
/// generic enum variant printed its address.
fn inline_arg_size(
    ty: &Type,
    type_names: &HashMap<rask_types::TypeId, String>,
    type_defs: &rask_types::TypeTable,
    cache: &LayoutCache,
) -> Option<u32> {
    match ty {
        Type::UnresolvedNamed(name) => {
            let id = type_defs.get_type_id(name)?;
            inline_arg_size(&Type::Named(id), type_names, type_defs, cache)
        }
        Type::UnresolvedGeneric { name, args } => {
            let base_name = name.to_string();
            let arg_tys: Vec<Type> = args
                .iter()
                .filter_map(|a| match a {
                    rask_types::GenericArg::Type(t) => Some((**t).clone()),
                    _ => None,
                })
                .collect();
            let instance = generic_instance_name(&base_name, &arg_tys, type_names)
                .and_then(|n| cache.get(&n).map(|(size, _)| *size));
            instance.or_else(|| cache.get(&base_name).map(|(size, _)| *size))
        }
        Type::Named(id) => {
            let def = type_defs.get(*id)?;
            if !matches!(
                def,
                rask_types::TypeDef::Struct { .. }
                    | rask_types::TypeDef::Enum { .. }
                    | rask_types::TypeDef::Union { .. }
            ) {
                return None;
            }
            let name = type_names.get(id)?.to_string();
            cache.get(&name).map(|(size, _)| *size)
        }
        // A nested instantiation is as wide as *its* layout — `One<One<Big>>` has
        // to see 24, not the 8 the shared `One` layout reports.
        Type::Generic { base, args } => {
            let base_name = type_names.get(base)?.to_string();
            let arg_tys: Vec<Type> = args
                .iter()
                .filter_map(|a| match a {
                    rask_types::GenericArg::Type(t) => Some((**t).clone()),
                    _ => None,
                })
                .collect();
            let instance = generic_instance_name(&base_name, &arg_tys, type_names)
                .and_then(|n| cache.get(&n).map(|(size, _)| *size));
            instance.or_else(|| cache.get(&base_name).map(|(size, _)| *size))
        }
        // A `T?` or a `T or E` is its bytes too — 16 for an optional scalar, 24
        // for a result — so a generic slot that only holds a word can't take
        // one. Without this `Wrap { value: opt(3) }` was refused outright:
        // nothing emitted an instance layout, so the shared 8-byte slot was all
        // there was (#872).
        Type::Tuple(_) | Type::Array { .. } | Type::String | Type::Result { .. } => {
            Some(type_size_align(ty, cache).0)
        }
        _ => None,
    }
}

/// A type argument respelled so `type_size_align` can find it: a name the layout
/// cache holds. A nested instantiation resolves to its own instance layout when
/// there is one, and to the shared layout otherwise.
fn arg_as_cache_name(
    ty: &Type,
    type_names: &HashMap<rask_types::TypeId, String>,
    cache: &LayoutCache,
) -> Type {
    match ty {
        Type::Named(id) => type_names
            .get(id)
            .map(|n| Type::UnresolvedNamed(n.to_string()))
            .unwrap_or_else(|| ty.clone()),
        Type::Generic { base, args } => {
            let Some(base_name) = type_names.get(base).map(|n| n.to_string()) else {
                return ty.clone();
            };
            let arg_tys: Vec<Type> = args
                .iter()
                .filter_map(|a| match a {
                    rask_types::GenericArg::Type(t) => Some((**t).clone()),
                    _ => None,
                })
                .collect();
            let instance = generic_instance_name(&base_name, &arg_tys, type_names)
                .filter(|n| cache.contains_key(n));
            Type::UnresolvedNamed(instance.unwrap_or(base_name))
        }
        // A shape that holds other types has to be walked, not just handed over.
        // `i64 or MyErr` sized its error side as one word, because a `Named`
        // buried inside it never reached the rename and `type_size_align` can't
        // resolve an id — so the layout came out 32 bytes where MIR wanted 40
        // (#872).
        Type::Result { ok, err } => Type::Result {
            ok: Box::new(arg_as_cache_name(ok, type_names, cache)),
            err: Box::new(arg_as_cache_name(err, type_names, cache)),
        },
        Type::Tuple(elems) => Type::Tuple(
            elems.iter().map(|e| arg_as_cache_name(e, type_names, cache)).collect(),
        ),
        Type::Array { elem, len } => Type::Array {
            elem: Box::new(arg_as_cache_name(elem, type_names, cache)),
            len: *len,
        },
        other => other.clone(),
    }
}

/// Every generic struct/enum instantiation this type mentions, at any depth.
fn collect_generic_instances(
    ty: &Type,
    type_names: &HashMap<rask_types::TypeId, String>,
    out: &mut Vec<(String, Vec<Type>)>,
) {
    use rask_types::GenericArg;
    match ty {
        Type::Generic { base, args } => {
            if let Some(name) = type_names.get(base) {
                let arg_tys: Vec<Type> = args
                    .iter()
                    .filter_map(|a| match a {
                        GenericArg::Type(t) => Some((**t).clone()),
                        _ => None,
                    })
                    .collect();
                if arg_tys.len() == args.len() {
                    out.push((name.to_string(), arg_tys));
                }
            }
            for arg in args {
                if let GenericArg::Type(inner) = arg {
                    collect_generic_instances(inner, type_names, out);
                }
            }
        }
        Type::UnresolvedGeneric { name, args } => {
            let arg_tys: Vec<Type> = args
                .iter()
                .filter_map(|a| match a {
                    GenericArg::Type(t) => Some((**t).clone()),
                    _ => None,
                })
                .collect();
            if arg_tys.len() == args.len() {
                out.push((name.to_string(), arg_tys));
            }
            for arg in args {
                if let GenericArg::Type(inner) = arg {
                    collect_generic_instances(inner, type_names, out);
                }
            }
        }
        Type::Tuple(elems) | Type::Union(elems) => {
            for elem in elems {
                collect_generic_instances(elem, type_names, out);
            }
        }
        Type::RawPtr(inner) => {
            collect_generic_instances(inner, type_names, out)
        }
        Type::Array { elem, .. } => collect_generic_instances(elem, type_names, out),
        Type::Result { ok, err } => {
            collect_generic_instances(ok, type_names, out);
            collect_generic_instances(err, type_names, out);
        }
        _ => {}
    }
}

/// Monomorphize a type-checked program.
///
/// Architecture: reachability drives instantiation (tree-shaking).
/// Only functions reachable from main() get instantiated.
///
/// 1. Build function lookup table from declarations
/// 2. BFS from main(): discover calls → instantiate on demand → walk instantiated body
/// 3. Compute layouts for all referenced structs/enums
pub fn monomorphize(
    program: &TypedProgram,
    decls: &[Decl],
) -> Result<MonoProgram, MonomorphizeError> {
    monomorphize_with_packages(program, decls, std::collections::HashSet::new())
}

/// Monomorphize a program that may have no `main` — a file of `test` blocks has
/// none, and `rask check` still has to answer whether its comptime consts fold.
/// Every non-generic top-level function is a root instead of the entry point,
/// so layouts and call targets exist for whatever the file defines.
///
/// Only for analysis. The result is not a program you can run: nothing in it
/// says which function starts.
pub fn monomorphize_for_analysis(
    program: &TypedProgram,
    decls: &[Decl],
) -> Result<MonoProgram, MonomorphizeError> {
    monomorphize_inner(program, decls, std::collections::HashSet::new(), true)
}

/// Monomorphize with cross-package module awareness.
///
/// `package_modules` contains names of imported external packages so the
/// reachability pass correctly discovers `pkg.func()` calls.
pub fn monomorphize_with_packages(
    program: &TypedProgram,
    decls: &[Decl],
    package_modules: std::collections::HashSet<String>,
) -> Result<MonoProgram, MonomorphizeError> {
    monomorphize_inner(program, decls, package_modules, false)
}

/// `entryless` seeds every plain function as a root when there's no `main`,
/// for the analysis entry point above.
fn monomorphize_inner(
    program: &TypedProgram,
    decls: &[Decl],
    package_modules: std::collections::HashSet<String>,
    entryless: bool,
) -> Result<MonoProgram, MonomorphizeError> {
    // A struct out of an `import c` header has no declaration in the source —
    // the type checker synthesizes one so the header's structs get layouts,
    // fields and codegen like any other struct (#948).
    let with_c_types: Vec<Decl>;
    let decls: &[Decl] = if program.c_type_decls.is_empty() {
        decls
    } else {
        with_c_types = decls
            .iter()
            .cloned()
            .chain(program.c_type_decls.iter().cloned())
            .collect();
        &with_c_types
    };

    let mut mono = Monomorphizer::with_typed_program(decls, program);
    mono.set_package_modules(package_modules);
    mono.set_interface_coercions(&program.interface_coercions);

    if !mono.add_entry("main") {
        if !entryless {
            return Err(MonomorphizeError::NoEntryPoint);
        }
        mono.add_all_plain_fn_roots();
    }
    mono.add_module_const_roots();
    mono.add_exported_roots();

    mono.run();

    let type_names: HashMap<rask_types::TypeId, String> = program
        .types
        .iter()
        .enumerate()
        .map(|(i, def)| {
            let name = match def {
                rask_types::TypeDef::Struct { name, .. }
                | rask_types::TypeDef::Enum { name, .. }
                | rask_types::TypeDef::Interface { name, .. }
                | rask_types::TypeDef::Union { name, .. }
                | rask_types::TypeDef::NominalAlias { name, .. }
                | rask_types::TypeDef::Primitive { name, .. } => name.clone(),
            };
            (rask_types::TypeId(i as u32), name)
        })
        .collect();

    // Every instantiation the program mentions: in an expression's type, as a
    // call's type argument, or as a field of a concrete type.
    let mut reached: Vec<(String, Vec<Type>)> = Vec::new();
    for ty in program
        .node_types
        .values()
        .chain(mono.instantiated_node_types.values())
        // A type argument is an instantiation even when nothing builds one.
        // `reflect.fields<Box2<string>>()` names the type and never constructs
        // it, so it appeared in no expression's type — and the layout that
        // would have said `value` is sixteen bytes was never emitted.
        // Reflection then read the shared layout and reported a `string` field
        // as an eight-byte `i64` (#968).
        .chain(mono.results.iter().flat_map(|f| f.type_args.iter().map(|b| &b.ty)))
    {
        collect_generic_instances(ty, &type_names, &mut reached);
    }
    for decl in decls {
        let concrete = match &decl.kind {
            DeclKind::Struct(s) => s.type_params.is_empty(),
            DeclKind::Enum(e) => e.type_params.is_empty(),
            DeclKind::Union(_) => true,
            _ => false,
        };
        if concrete {
            for ty in layout_field_types(decl) {
                collect_generic_instances(&layout::field_type(ty), &type_names, &mut reached);
            }
        }
    }

    // Declared and instance layouts in one dependency order, the declared ones
    // shared with the interpreter so both backends read one set of offsets
    // (#1104).
    let (struct_layouts, mut enum_layouts, _) = compute_layouts(
        decls,
        Some(&Instantiations { reached, type_names: &type_names, types: &program.types }),
    );

    // `Ordering` has no decl to compute a layout from — the compiler registers
    // it instead. Give it one anyway so it behaves like every other fieldless
    // enum downstream: `compare` can hand back a real Ordering value rather
    // than a bare tag, and `{}` on one reaches whatever Displayable was
    // written for it (#729).
    if !enum_layouts.iter().any(|l| l.name == "Ordering") {
        enum_layouts.push(layout::ordering_layout());
    }

    Ok(MonoProgram {
        functions: mono.results,
        struct_layouts,
        enum_layouts,
        type_names: program.types.type_name_map(),
        call_rewrites: mono.call_rewrites,
        map_key_fns: mono.map_key_fns,
        instantiated_node_types: mono.instantiated_node_types,
        instantiated_call_targets: mono.instantiated_call_targets,
        instantiated_operator_targets: mono.instantiated_operator_targets,
        instantiated_error_wraps: mono.instantiated_error_wraps,
        instantiated_fallback_keeps_shape: mono.instantiated_fallback_keeps_shape,
        instantiated_escaping_closures: mono.instantiated_escaping_closures,
        instantiated_field_reuses: mono.instantiated_field_reuses,
        instantiated_task_bound_closures: mono.instantiated_task_bound_closures,
    })
}

#[derive(Debug)]
pub enum MonomorphizeError {
    NoEntryPoint,
    UnresolvedGeneric {
        function_name: String,
        type_param: String,
    },
    LayoutError {
        type_name: String,
        reason: String,
    },
}

impl std::fmt::Display for MonomorphizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoEntryPoint => write!(f, "no `main` function to compile from"),
            Self::UnresolvedGeneric { function_name, type_param } => write!(
                f,
                "`{}` needs a concrete type for `{}`, but the call site never fixed one",
                function_name, type_param,
            ),
            Self::LayoutError { type_name, reason } => {
                write!(f, "cannot lay out `{}` in memory: {}", type_name, reason)
            }
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rask_ast::decl::{
        Decl, DeclKind, EnumDecl, Field, FieldVisibility, FnDecl, ImplDecl, Param, StructDecl, TypeParam, Variant,
    };
    use rask_ast::expr::{ArgMode, CallArg, Expr, ExprKind};
    use rask_ast::stmt::{Stmt, StmtKind};
    use rask_ast::{NodeId, Span};

    fn sp() -> Span {
        Span::new(0, 0)
    }

    fn int_expr(val: i128) -> Expr {
        Expr {
            id: NodeId(100),
            kind: ExprKind::Int(val, None),
            span: sp(),
        }
    }

    fn ident_expr(name: &str) -> Expr {
        Expr {
            id: NodeId(101),
            kind: ExprKind::Ident(name.to_string()),
            span: sp(),
        }
    }

    fn call_expr(func_name: &str, args: Vec<Expr>) -> Expr {
        Expr {
            id: NodeId(102),
            kind: ExprKind::Call {
                func: Box::new(ident_expr(func_name)),
                args: args.into_iter().map(|expr| CallArg { name: None, mode: ArgMode::Default, expr }).collect(),
            },
            span: sp(),
        }
    }

    fn return_stmt(val: Option<Expr>) -> Stmt {
        Stmt {
            id: NodeId(200),
            kind: StmtKind::Return(val),
            span: sp(),
        }
    }

    fn expr_stmt(e: Expr) -> Stmt {
        Stmt {
            id: NodeId(201),
            kind: StmtKind::Expr(e),
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
                        ty: rask_parser::parse_type(ty),
                        is_take: false,
                        is_mutate: false, is_deleting: false,
                        default: None,
                    })
                    .collect(),
                ret_ty: ret_ty.map(|s| rask_parser::parse_type(s).unwrap()),
                body,
                is_pub: false,
                is_private: false,
                is_comptime: false,
                is_unsafe: false,
                abi: None,
                attrs: vec![],
                doc: None,
                span: sp(),
                decl_start: sp().start,
            }),
            span: sp(),
        }
    }

    fn make_generic_fn(
        name: &str,
        type_params: Vec<&str>,
        params: Vec<(&str, &str)>,
        ret_ty: Option<&str>,
        body: Vec<Stmt>,
    ) -> Decl {
        Decl {
            id: NodeId(0),
            kind: DeclKind::Fn(FnDecl {
                name: name.to_string(),
                type_params: type_params
                    .into_iter()
                    .map(|tp| TypeParam {
                        name: tp.to_string(),
                        is_comptime: false,
                        comptime_type: None,
                        bounds: vec![],
                        default: None,
                    })
                    .collect(),
                params: params
                    .into_iter()
                    .map(|(n, ty)| Param {
                        name: n.to_string(),
                        name_span: sp(),
                        ty: rask_parser::parse_type(ty),
                        is_take: false,
                        is_mutate: false, is_deleting: false,
                        default: None,
                    })
                    .collect(),
                ret_ty: ret_ty.map(|s| rask_parser::parse_type(s).unwrap()),
                body,
                is_pub: false,
                is_private: false,
                is_comptime: false,
                is_unsafe: false,
                abi: None,
                attrs: vec![],
                doc: None,
                span: sp(),
                decl_start: sp().start,
            }),
            span: sp(),
        }
    }

    fn dummy_typed_program() -> TypedProgram {
        TypedProgram {
            symbols: rask_resolve::SymbolTable::new(),
            c_type_decls: Vec::new(),
            mutate_self_fns: std::collections::HashSet::new(),
            resolutions: std::collections::HashMap::new(),
            types: rask_types::TypeTable::new(),
            node_types: std::collections::HashMap::new(),
            call_type_args: std::collections::HashMap::new(),
            call_targets: std::collections::HashMap::new(),
            operator_targets: std::collections::HashMap::new(),
            interface_coercions: std::collections::HashMap::new(),
            file_packages: std::collections::HashMap::new(),
            conformance_disambiguation: std::collections::HashMap::new(),
            conformance_interfaces: std::collections::HashMap::new(),
            error_wraps: std::collections::HashMap::new(),
            fallback_keeps_shape: std::collections::HashSet::new(),
            escaping_closures: std::collections::HashSet::new(),
            field_reuses: std::collections::HashSet::new(),
            task_bound_closures: std::collections::HashSet::new(),
            generic_closure_captures: std::collections::HashMap::new(),
            try_chain_placement: std::collections::HashMap::new(),
            unsafe_ops: Vec::new(),
            span_types: std::collections::HashMap::new(),
            channel_send_sites: std::collections::HashSet::new(),
            type_test_patterns: std::collections::HashSet::new(),
            default_fills: std::collections::HashMap::new(),
            inferred_fn_ret: std::collections::HashMap::new(),
            inferred_fn_params: std::collections::HashMap::new(),
            derived_decls: Vec::new(),
            wrapper_eq_calls: std::collections::HashMap::new(),
            sequence_coercions: std::collections::HashMap::new(),
            wrapper_fns: Vec::new(),
            derived_generic_methods: std::collections::HashSet::new(),
        }
    }

    // ── Monomorphize entry point ────────────────────────────────

    #[test]
    fn no_main_returns_error() {
        let decls = vec![make_fn("helper", vec![], None, vec![return_stmt(None)])];
        let tp = dummy_typed_program();
        let result = monomorphize(&tp, &decls);
        assert!(matches!(result, Err(MonomorphizeError::NoEntryPoint)));
    }

    #[test]
    fn main_only() {
        let decls = vec![make_fn(
            "main",
            vec![],
            None,
            vec![return_stmt(None)],
        )];
        let tp = dummy_typed_program();
        let result = monomorphize(&tp, &decls).unwrap();
        assert_eq!(result.functions.len(), 1);
        assert_eq!(result.functions[0].name, "main");
    }

    #[test]
    fn main_calls_helper() {
        let decls = vec![
            make_fn(
                "main",
                vec![],
                None,
                vec![expr_stmt(call_expr("helper", vec![])), return_stmt(None)],
            ),
            make_fn("helper", vec![], None, vec![return_stmt(None)]),
        ];
        let tp = dummy_typed_program();
        let result = monomorphize(&tp, &decls).unwrap();
        assert_eq!(result.functions.len(), 2);
        let names: Vec<&str> = result.functions.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"main"));
        assert!(names.contains(&"helper"));
    }

    #[test]
    fn unreachable_function_excluded() {
        let decls = vec![
            make_fn("main", vec![], None, vec![return_stmt(None)]),
            make_fn("dead_code", vec![], None, vec![return_stmt(None)]),
        ];
        let tp = dummy_typed_program();
        let result = monomorphize(&tp, &decls).unwrap();
        assert_eq!(result.functions.len(), 1);
        assert_eq!(result.functions[0].name, "main");
    }

    #[test]
    fn transitive_calls() {
        // main → a → b → c
        let decls = vec![
            make_fn(
                "main",
                vec![],
                None,
                vec![expr_stmt(call_expr("a", vec![])), return_stmt(None)],
            ),
            make_fn(
                "a",
                vec![],
                None,
                vec![expr_stmt(call_expr("b", vec![])), return_stmt(None)],
            ),
            make_fn(
                "b",
                vec![],
                None,
                vec![expr_stmt(call_expr("c", vec![])), return_stmt(None)],
            ),
            make_fn("c", vec![], None, vec![return_stmt(None)]),
        ];
        let tp = dummy_typed_program();
        let result = monomorphize(&tp, &decls).unwrap();
        assert_eq!(result.functions.len(), 4);
    }

    #[test]
    fn recursive_function_terminates() {
        // main calls itself (cycle)
        let decls = vec![make_fn(
            "main",
            vec![],
            None,
            vec![expr_stmt(call_expr("main", vec![])), return_stmt(None)],
        )];
        let tp = dummy_typed_program();
        let result = monomorphize(&tp, &decls).unwrap();
        assert_eq!(result.functions.len(), 1);
    }

    #[test]
    fn mutual_recursion_terminates() {
        // a → b → a (cycle)
        let decls = vec![
            make_fn(
                "main",
                vec![],
                None,
                vec![expr_stmt(call_expr("a", vec![])), return_stmt(None)],
            ),
            make_fn(
                "a",
                vec![],
                None,
                vec![expr_stmt(call_expr("b", vec![])), return_stmt(None)],
            ),
            make_fn(
                "b",
                vec![],
                None,
                vec![expr_stmt(call_expr("a", vec![])), return_stmt(None)],
            ),
        ];
        let tp = dummy_typed_program();
        let result = monomorphize(&tp, &decls).unwrap();
        assert_eq!(result.functions.len(), 3);
    }

    #[test]
    fn struct_layouts_computed() {
        let decls = vec![
            make_fn("main", vec![], None, vec![return_stmt(None)]),
            Decl {
                id: NodeId(0),
                kind: DeclKind::Struct(StructDecl {
                    name: "Point".to_string(),
                    type_params: vec![],
                    fields: vec![
                        Field { name: "x".to_string(), name_span: sp(), ty: rask_parser::parse_type("i32").unwrap(), visibility: FieldVisibility::Package, attrs: vec![], default: None, doc: None },
                        Field { name: "y".to_string(), name_span: sp(), ty: rask_parser::parse_type("i32").unwrap(), visibility: FieldVisibility::Package, attrs: vec![], default: None, doc: None },
                    ],
                    methods: vec![],
                    is_pub: false,
                    attrs: vec![],
                    doc: None,
                }),
                span: sp(),
            },
        ];
        let tp = dummy_typed_program();
        let result = monomorphize(&tp, &decls).unwrap();
        assert_eq!(result.struct_layouts.len(), 1);
        assert_eq!(result.struct_layouts[0].name, "Point");
    }

    #[test]
    fn enum_layouts_computed() {
        let decls = vec![
            make_fn("main", vec![], None, vec![return_stmt(None)]),
            Decl {
                id: NodeId(0),
                kind: DeclKind::Enum(EnumDecl {
                    name: "Color".to_string(),
                    type_params: vec![],
                    variants: vec![
                        Variant { name: "Red".to_string(), name_span: rask_ast::Span::new(0, 0), fields: vec![], attrs: vec![], discriminant: None },
                        Variant { name: "Green".to_string(), name_span: rask_ast::Span::new(0, 0), fields: vec![], attrs: vec![], discriminant: None },
                    ],
                    methods: vec![],
                    is_pub: false,
                    attrs: vec![],
                    doc: None,
                    backing_type: None,
                }),
                span: sp(),
            },
        ];
        let tp = dummy_typed_program();
        let result = monomorphize(&tp, &decls).unwrap();
        // `Ordering` is synthesized alongside the declared enums, so assert on
        // the declared ones rather than the raw count.
        let declared: Vec<&str> = result.enum_layouts.iter()
            .map(|l| l.name.as_str())
            .filter(|n| *n != "Ordering")
            .collect();
        assert_eq!(declared, vec!["Color"]);
    }

    #[test]
    fn struct_forward_references_enum() {
        // Struct declared BEFORE the enum it references — topo sort
        // must process the enum first so its layout is in the cache.
        let decls = vec![
            make_fn("main", vec![], None, vec![return_stmt(None)]),
            Decl {
                id: NodeId(0),
                kind: DeclKind::Struct(StructDecl {
                    name: "Container".to_string(),
                    type_params: vec![],
                    fields: vec![
                        Field { name: "kind".to_string(), name_span: sp(), ty: rask_parser::parse_type("Kind").unwrap(), visibility: FieldVisibility::Package, attrs: vec![], default: None, doc: None },
                        Field { name: "value".to_string(), name_span: sp(), ty: rask_parser::parse_type("i32").unwrap(), visibility: FieldVisibility::Package, attrs: vec![], default: None, doc: None },
                    ],
                    methods: vec![],
                    is_pub: false,
                    attrs: vec![],
                    doc: None,
                }),
                span: sp(),
            },
            Decl {
                id: NodeId(0),
                kind: DeclKind::Enum(EnumDecl {
                    name: "Kind".to_string(),
                    type_params: vec![],
                    variants: vec![
                        Variant {
                            name: "Alpha".to_string(),
                            name_span: sp(),
                            fields: vec![
                                Field { name: "x".to_string(), name_span: sp(), ty: rask_parser::parse_type("i32").unwrap(), visibility: FieldVisibility::Package, attrs: vec![], default: None, doc: None },
                                Field { name: "y".to_string(), name_span: sp(), ty: rask_parser::parse_type("i32").unwrap(), visibility: FieldVisibility::Package, attrs: vec![], default: None, doc: None },
                            ],
                            attrs: vec![],
                            discriminant: None,
                        },
                        Variant { name: "Beta".to_string(), name_span: rask_ast::Span::new(0, 0), fields: vec![], attrs: vec![], discriminant: None },
                    ],
                    methods: vec![],
                    is_pub: false,
                    attrs: vec![],
                    doc: None,
                    backing_type: None,
                }),
                span: sp(),
            },
        ];
        let tp = dummy_typed_program();
        let result = monomorphize(&tp, &decls).unwrap();

        // `Ordering` is synthesized alongside the declared enums, so assert on
        // the declared ones rather than the raw count.
        let declared: Vec<&str> = result.enum_layouts.iter()
            .map(|l| l.name.as_str())
            .filter(|n| *n != "Ordering")
            .collect();
        assert_eq!(declared, vec!["Kind"]);

        assert_eq!(result.struct_layouts.len(), 1);
        let container = &result.struct_layouts[0];
        assert_eq!(container.name, "Container");
        // Kind enum: tag(8) + field_x(8) + field_y(8) = 24 bytes
        // Container should embed Kind at its full size, not the (8,8) default.
        let kind_field = container.fields.iter().find(|f| f.name == "kind").unwrap();
        assert_eq!(kind_field.size, 24, "Kind field should be 24 bytes (tag + 2 fields), not 8");
    }

    // ── Instantiation ───────────────────────────────────────────

    #[test]
    fn instantiate_removes_type_params() {
        let decl = make_generic_fn(
            "identity",
            vec!["T"],
            vec![("x", "T")],
            Some("T"),
            vec![return_stmt(Some(ident_expr("x")))],
        );
        let result = instantiate_function(&decl, &[Type::I32]);
        if let DeclKind::Fn(f) = &result.kind {
            assert!(f.type_params.is_empty());
            assert_eq!(f.params[0].ty.as_ref().map(|t| t.to_string()).as_deref(), Some("i32")); // substituted
        } else {
            panic!("Expected function declaration");
        }
    }

    #[test]
    fn instantiate_preserves_body() {
        let decl = make_generic_fn(
            "identity",
            vec!["T"],
            vec![("x", "T")],
            Some("T"),
            vec![return_stmt(Some(ident_expr("x")))],
        );
        let result = instantiate_function(&decl, &[Type::I64]);
        if let DeclKind::Fn(f) = &result.kind {
            assert_eq!(f.body.len(), 1);
            assert!(matches!(f.body[0].kind, StmtKind::Return(Some(_))));
        } else {
            panic!("Expected function declaration");
        }
    }

    #[test]
    fn instantiate_fresh_node_ids() {
        // Use a distinct NodeId for the original so we can verify the clone gets a different one
        let mut decl = make_generic_fn(
            "id",
            vec!["T"],
            vec![("x", "T")],
            None,
            vec![return_stmt(Some(ident_expr("x")))],
        );
        decl.id = NodeId(9999);
        let result = instantiate_function(&decl, &[Type::Bool]);
        // Substitutor generates sequential IDs starting at 0, so result.id != 9999
        assert_ne!(result.id, decl.id);
    }

    // ── Reachability walker ─────────────────────────────────────

    #[test]
    fn reachability_discovers_nested_calls() {
        // main → { let x = foo(1); bar(x) }
        let decls = vec![
            make_fn(
                "main",
                vec![],
                None,
                vec![
                    Stmt {
                        id: NodeId(10),
                        kind: StmtKind::Let {
                            name: "x".to_string(),
                            name_span: sp(),
                            ty: None,
                            init: call_expr("foo", vec![int_expr(1)]),
                        },
                        span: sp(),
                    },
                    expr_stmt(call_expr("bar", vec![ident_expr("x")])),
                    return_stmt(None),
                ],
            ),
            make_fn("foo", vec![("n", "i32")], Some("i32"), vec![return_stmt(Some(ident_expr("n")))]),
            make_fn("bar", vec![("n", "i32")], None, vec![return_stmt(None)]),
            make_fn("unused", vec![], None, vec![return_stmt(None)]),
        ];

        let empty_type_args = std::collections::HashMap::new();
        let mut mono = Monomorphizer::new(&decls, &empty_type_args, &HashMap::new());
        assert!(mono.add_entry("main"));
        mono.run();

        let names: Vec<&str> = mono.results.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"main"));
        assert!(names.contains(&"foo"));
        assert!(names.contains(&"bar"));
        assert!(!names.contains(&"unused"));
    }

    #[test]
    fn reachability_handles_conditionals() {
        // main → if true { a() } else { b() }
        let decls = vec![
            make_fn(
                "main",
                vec![],
                None,
                vec![expr_stmt(Expr {
                    id: NodeId(50),
                    kind: ExprKind::If {
                        cond: Box::new(Expr {
                            id: NodeId(51),
                            kind: ExprKind::Bool(true),
                            span: sp(),
                        }),
                        then_branch: Box::new(call_expr("a", vec![])),
                        else_branch: Some(Box::new(call_expr("b", vec![]))),
                        else_binding: None,
                    },
                    span: sp(),
                })],
            ),
            make_fn("a", vec![], None, vec![return_stmt(None)]),
            make_fn("b", vec![], None, vec![return_stmt(None)]),
        ];

        let empty_type_args = std::collections::HashMap::new();
        let mut mono = Monomorphizer::new(&decls, &empty_type_args, &HashMap::new());
        mono.add_entry("main");
        mono.run();

        // Both branches are conservatively included
        let names: Vec<&str> = mono.results.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
    }

    // ── Method reachability ─────────────────────────────────────

    fn method_call_expr(object: Expr, method: &str, args: Vec<Expr>) -> Expr {
        Expr {
            id: NodeId(300),
            kind: ExprKind::MethodCall {
                object: Box::new(object),
                method: method.to_string(),
                type_args: None,
                args: args.into_iter().map(|expr| CallArg { name: None, mode: ArgMode::Default, expr }).collect(),
            },
            span: sp(),
        }
    }

    fn make_method(name: &str, params: Vec<(&str, &str)>, ret_ty: Option<&str>, body: Vec<Stmt>) -> FnDecl {
        FnDecl {
            name: name.to_string(),
            type_params: vec![],
            params: params
                .into_iter()
                .map(|(n, ty)| Param {
                    name: n.to_string(),
                    name_span: sp(),
                    ty: rask_parser::parse_type(ty),
                    is_take: false,
                    is_mutate: false, is_deleting: false,
                    default: None,
                })
                .collect(),
            ret_ty: ret_ty.map(|s| rask_parser::parse_type(s).unwrap()),
            body,
            is_pub: false,
            is_private: false,
            is_comptime: false,
            is_unsafe: false,
            abi: None,
            attrs: vec![],
            doc: None,
            span: sp(),
            decl_start: 0,
        }
    }

    #[test]
    fn method_call_on_type_enqueues_static_method() {
        // main calls Point.new() — static method on struct
        let decls = vec![
            make_fn(
                "main",
                vec![],
                None,
                vec![
                    expr_stmt(method_call_expr(ident_expr("Point"), "new", vec![])),
                    return_stmt(None),
                ],
            ),
            Decl {
                id: NodeId(0),
                kind: DeclKind::Struct(StructDecl {
                    name: "Point".to_string(),
                    type_params: vec![],
                    fields: vec![
                        Field { name: "x".to_string(), name_span: sp(), ty: rask_parser::parse_type("i32").unwrap(), visibility: FieldVisibility::Package, attrs: vec![], default: None, doc: None },
                        Field { name: "y".to_string(), name_span: sp(), ty: rask_parser::parse_type("i32").unwrap(), visibility: FieldVisibility::Package, attrs: vec![], default: None, doc: None },
                    ],
                    methods: vec![
                        make_method("new", vec![], Some("Point"), vec![return_stmt(None)]),
                    ],
                    is_pub: false,
                    attrs: vec![],
                    doc: None,
                }),
                span: sp(),
            },
        ];

        let empty_type_args = std::collections::HashMap::new();
        let mut mono = Monomorphizer::new(&decls, &empty_type_args, &HashMap::new());
        mono.add_entry("main");
        mono.run();

        let names: Vec<&str> = mono.results.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"main"));
        assert!(names.contains(&"Point_new"), "static method should be reachable: {:?}", names);
    }

    #[test]
    fn method_call_on_value_enqueues_instance_method() {
        // main calls p.distance() — instance method via bare name
        let decls = vec![
            make_fn(
                "main",
                vec![],
                None,
                vec![
                    expr_stmt(method_call_expr(ident_expr("p"), "distance", vec![])),
                    return_stmt(None),
                ],
            ),
            Decl {
                id: NodeId(0),
                kind: DeclKind::Impl(ImplDecl {
                    interface: None,
                    target_ty: rask_parser::parse_type("Point").unwrap(),
                    methods: vec![
                        make_method("distance", vec![("self", "Point")], Some("f64"), vec![return_stmt(None)]),
                    ],
                    doc: None,
                    is_unsafe: false,
                    is_pub: false,
                    where_bounds: vec![],
                    assoc_bindings: Vec::new(),
                }),
                span: sp(),
            },
        ];

        let empty_type_args = std::collections::HashMap::new();
        let mut mono = Monomorphizer::new(&decls, &empty_type_args, &HashMap::new());
        mono.add_entry("main");
        mono.run();

        let names: Vec<&str> = mono.results.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"main"));
        // Instance call on "p" enqueues via bare name → Point_distance (from method_by_bare_name)
        assert!(names.contains(&"Point_distance"), "instance method should be reachable: {:?}", names);
    }

    #[test]
    fn method_in_impl_block_reachable() {
        // main calls Counter.increment() via extend block
        let decls = vec![
            make_fn(
                "main",
                vec![],
                None,
                vec![
                    expr_stmt(method_call_expr(ident_expr("Counter"), "increment", vec![])),
                    return_stmt(None),
                ],
            ),
            Decl {
                id: NodeId(0),
                kind: DeclKind::Impl(ImplDecl {
                    interface: None,
                    target_ty: rask_parser::parse_type("Counter").unwrap(),
                    methods: vec![
                        make_method("increment", vec![("self", "Counter")], None, vec![return_stmt(None)]),
                    ],
                    doc: None,
                    is_unsafe: false,
                    is_pub: false,
                    where_bounds: vec![],
                    assoc_bindings: Vec::new(),
                }),
                span: sp(),
            },
        ];

        let empty_type_args = std::collections::HashMap::new();
        let mut mono = Monomorphizer::new(&decls, &empty_type_args, &HashMap::new());
        mono.add_entry("main");
        mono.run();

        let names: Vec<&str> = mono.results.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"Counter_increment"), "impl method should be reachable: {:?}", names);
    }

    #[test]
    fn unreachable_method_excluded() {
        // main doesn't call any methods — dead_method should be excluded
        let decls = vec![
            make_fn("main", vec![], None, vec![return_stmt(None)]),
            Decl {
                id: NodeId(0),
                kind: DeclKind::Struct(StructDecl {
                    name: "Widget".to_string(),
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![
                        make_method("dead_method", vec![], None, vec![return_stmt(None)]),
                    ],
                    is_pub: false,
                    attrs: vec![],
                    doc: None,
                }),
                span: sp(),
            },
        ];

        let empty_type_args = std::collections::HashMap::new();
        let mut mono = Monomorphizer::new(&decls, &empty_type_args, &HashMap::new());
        mono.add_entry("main");
        mono.run();

        assert_eq!(mono.results.len(), 1);
        assert_eq!(mono.results[0].name, "main");
    }

    #[test]
    fn method_body_transitively_discovers_calls() {
        // main → Point.new() → helper() (transitive through method body)
        let decls = vec![
            make_fn(
                "main",
                vec![],
                None,
                vec![
                    expr_stmt(method_call_expr(ident_expr("Point"), "new", vec![])),
                    return_stmt(None),
                ],
            ),
            Decl {
                id: NodeId(0),
                kind: DeclKind::Struct(StructDecl {
                    name: "Point".to_string(),
                    type_params: vec![],
                    fields: vec![],
                    methods: vec![
                        make_method("new", vec![], Some("Point"), vec![
                            expr_stmt(call_expr("helper", vec![])),
                            return_stmt(None),
                        ]),
                    ],
                    is_pub: false,
                    attrs: vec![],
                    doc: None,
                }),
                span: sp(),
            },
            make_fn("helper", vec![], None, vec![return_stmt(None)]),
            make_fn("unused", vec![], None, vec![return_stmt(None)]),
        ];

        let empty_type_args = std::collections::HashMap::new();
        let mut mono = Monomorphizer::new(&decls, &empty_type_args, &HashMap::new());
        mono.add_entry("main");
        mono.run();

        let names: Vec<&str> = mono.results.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"main"));
        assert!(names.contains(&"Point_new"));
        assert!(names.contains(&"helper"), "transitive call from method body should be discovered");
        assert!(!names.contains(&"unused"));
    }

    #[test]
    fn enum_method_reachable() {
        // main calls Color.default()
        let decls = vec![
            make_fn(
                "main",
                vec![],
                None,
                vec![
                    expr_stmt(method_call_expr(ident_expr("Color"), "default", vec![])),
                    return_stmt(None),
                ],
            ),
            Decl {
                id: NodeId(0),
                kind: DeclKind::Enum(EnumDecl {
                    name: "Color".to_string(),
                    type_params: vec![],
                    variants: vec![
                        Variant { name: "Red".to_string(), name_span: rask_ast::Span::new(0, 0), fields: vec![], attrs: vec![], discriminant: None },
                        Variant { name: "Blue".to_string(), name_span: rask_ast::Span::new(0, 0), fields: vec![], attrs: vec![], discriminant: None },
                    ],
                    methods: vec![
                        make_method("default", vec![], Some("Color"), vec![return_stmt(None)]),
                    ],
                    is_pub: false,
                    attrs: vec![],
                    doc: None,
                    backing_type: None,
                }),
                span: sp(),
            },
        ];

        let empty_type_args = std::collections::HashMap::new();
        let mut mono = Monomorphizer::new(&decls, &empty_type_args, &HashMap::new());
        mono.add_entry("main");
        mono.run();

        let names: Vec<&str> = mono.results.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"Color_default"), "enum method should be reachable: {:?}", names);
    }
}
