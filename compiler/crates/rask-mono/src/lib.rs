// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Monomorphization pass - eliminates generics by instantiating concrete copies.
//!
//! Takes type-checked AST and produces monomorphized program with:
//! - Concrete function instances for each unique (function_id, [type_args])
//! - Computed memory layouts for all structs and enums
//! - Reachability analysis starting from main()

pub mod abi;
mod instantiate;
mod layout;
mod reachability;

pub use instantiate::instantiate_function;
pub use layout::{
    compute_enum_layout, compute_struct_layout, compute_union_layout, is_stdlib_span,
    ordering_layout, type_size_align,
    EnumLayout, FieldLayout, LayoutCache, StructLayout, VariantLayout,
};
pub use reachability::{mangle_name, Monomorphizer};

use rask_ast::decl::{Decl, DeclKind};
use rask_ast::NodeId;
use rask_types::{Type, TypeBinding, TypedProgram};
use std::collections::{HashMap, HashSet, VecDeque};

/// Monomorphized program with all generics eliminated
pub struct MonoProgram {
    pub functions: Vec<MonoFunction>,
    pub struct_layouts: Vec<StructLayout>,
    pub enum_layouts: Vec<EnumLayout>,
    /// Call expression NodeId → mangled callee name for generic function calls.
    pub call_rewrites: HashMap<NodeId, String>,
    /// Types and dispatch targets for the nodes of instantiated generic bodies.
    ///
    /// Those nodes don't exist in the checker's output — they were created
    /// here — so without these, every lookup inside an instantiated body misses
    /// and lowering falls back to guessing from AST shape.
    pub instantiated_node_types: HashMap<NodeId, Type>,
    pub instantiated_call_targets: HashMap<NodeId, rask_types::Callee>,
    /// ER31a: `try` sites in instantiated bodies that wrap their error, same idea.
    pub instantiated_error_wraps: HashMap<NodeId, rask_types::ErrorWrap>,
    /// ER14a: instantiated `??` nodes whose right side is still wrapped.
    pub instantiated_fallback_keeps_shape: HashSet<NodeId>,
}

impl MonoProgram {
    /// Node types for the whole program: the checker's, plus the ones carried
    /// onto instantiated bodies.
    ///
    /// Lowering runs after monomorphization and sees both kinds of node, so it
    /// wants one map. The two sets of ids are disjoint by construction —
    /// instantiation allocates above everything the checker used.
    pub fn all_node_types(&self, typed: &TypedProgram) -> HashMap<NodeId, Type> {
        let mut merged = typed.node_types.clone();
        merged.extend(self.instantiated_node_types.iter().map(|(k, v)| (*k, v.clone())));
        merged
    }

    /// Dispatch targets for the whole program, merged the same way.
    pub fn all_call_targets(
        &self,
        typed: &TypedProgram,
    ) -> HashMap<NodeId, rask_types::Callee> {
        let mut merged = typed.call_targets.clone();
        merged.extend(self.instantiated_call_targets.iter().map(|(k, v)| (*k, v.clone())));
        merged
    }

    /// ER31a error wraps for the whole program, merged the same way.
    pub fn all_error_wraps(
        &self,
        typed: &TypedProgram,
    ) -> HashMap<NodeId, rask_types::ErrorWrap> {
        let mut merged = typed.error_wraps.clone();
        merged.extend(self.instantiated_error_wraps.iter().map(|(k, v)| (*k, v.clone())));
        merged
    }

    /// ER14a: every `??` site that keeps the optional shape, source and
    /// instantiated alike.
    pub fn all_fallback_keeps_shape(&self, typed: &TypedProgram) -> HashSet<NodeId> {
        let mut merged = typed.fallback_keeps_shape.clone();
        merged.extend(self.instantiated_fallback_keeps_shape.iter().copied());
        merged
    }
}

/// Monomorphized function instance
pub struct MonoFunction {
    pub name: String,
    /// The type parameters this copy fixes, each named by the parameter it binds.
    pub type_args: Vec<TypeBinding>,
    pub body: Decl,
}

/// Collect user-defined type names referenced in a parsed Type.
fn collect_type_deps(ty: &Type, out: &mut HashSet<String>) {
    match ty {
        Type::UnresolvedNamed(name) => {
            out.insert(name.clone());
        }
        Type::UnresolvedGeneric { name, args } => {
            out.insert(name.clone());
            for arg in args {
                if let rask_types::GenericArg::Type(inner) = arg {
                    collect_type_deps(inner, out);
                }
            }
        }
        Type::Result { ok, err } => {
            collect_type_deps(ok, out);
            collect_type_deps(err, out);
        }
        Type::Slice(inner) => collect_type_deps(inner, out),
        _ => {}
    }
}

/// Topological sort of type declarations by field dependencies (Kahn's algorithm).
/// Returns indices into `decls` for struct/enum/union declarations only,
/// ordered so that dependencies come before dependents.
fn topo_sort_type_decls(decls: &[Decl]) -> Vec<usize> {
    // Map type name → decl index for struct/enum/union declarations
    let mut name_to_idx: HashMap<String, usize> = HashMap::new();
    let mut type_indices: Vec<usize> = Vec::new();

    for (i, decl) in decls.iter().enumerate() {
        match &decl.kind {
            DeclKind::Struct(s) if s.type_params.is_empty() => {
                name_to_idx.insert(s.name.clone(), i);
                type_indices.push(i);
            }
            DeclKind::Enum(e) if e.type_params.is_empty() => {
                name_to_idx.insert(e.name.clone(), i);
                type_indices.push(i);
            }
            DeclKind::Union(u) => {
                name_to_idx.insert(u.name.clone(), i);
                type_indices.push(i);
            }
            // Nominal newtypes take part in the ordering: a struct with a field
            // typed by one needs the alias's size known first (#445).
            DeclKind::TypeAlias(a) if !a.is_transparent && a.type_params.is_empty() => {
                name_to_idx.insert(a.name.clone(), i);
                type_indices.push(i);
            }
            _ => {}
        }
    }

    // Build dependency edges: decl_idx → set of decl indices it depends on
    let mut deps: HashMap<usize, HashSet<usize>> = HashMap::new();
    let mut rdeps: HashMap<usize, Vec<usize>> = HashMap::new();

    for &idx in &type_indices {
        let mut field_deps = HashSet::new();
        let fields: Vec<&str> = match &decls[idx].kind {
            DeclKind::Struct(s) => s.fields.iter().map(|f| f.ty.as_str()).collect(),
            DeclKind::Enum(e) => e.variants.iter()
                .flat_map(|v| v.fields.iter().map(|f| f.ty.as_str()))
                .collect(),
            DeclKind::Union(u) => u.fields.iter().map(|f| f.ty.as_str()).collect(),
            DeclKind::TypeAlias(a) => vec![a.target.as_str()],
            _ => vec![],
        };

        let mut type_names = HashSet::new();
        for ty_str in fields {
            let parsed = layout::parse_field_type(ty_str);
            collect_type_deps(&parsed, &mut type_names);
        }

        for name in type_names {
            if let Some(&dep_idx) = name_to_idx.get(&name) {
                if dep_idx != idx {
                    field_deps.insert(dep_idx);
                    rdeps.entry(dep_idx).or_default().push(idx);
                }
            }
        }
        deps.insert(idx, field_deps);
    }

    // Kahn's algorithm
    let mut queue: VecDeque<usize> = VecDeque::new();
    for &idx in &type_indices {
        if deps.get(&idx).map_or(true, |d| d.is_empty()) {
            queue.push_back(idx);
        }
    }

    let mut sorted = Vec::with_capacity(type_indices.len());
    while let Some(idx) = queue.pop_front() {
        sorted.push(idx);
        if let Some(dependents) = rdeps.get(&idx) {
            for &dep in dependents {
                if let Some(dep_set) = deps.get_mut(&dep) {
                    dep_set.remove(&idx);
                    if dep_set.is_empty() {
                        queue.push_back(dep);
                    }
                }
            }
        }
    }

    // Any remaining (cycles) — append in source order
    if sorted.len() < type_indices.len() {
        let in_sorted: HashSet<usize> = sorted.iter().copied().collect();
        for &idx in &type_indices {
            if !in_sorted.contains(&idx) {
                sorted.push(idx);
            }
        }
    }

    sorted
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

fn bare_type_name(name: &str) -> String {
    name.split('<').next().unwrap_or(name).trim().to_string()
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
        Type::Named(id) => bare_type_name(type_names.get(id)?),
        // An argument substituted into an instantiated copy is named, not
        // interned — the copy's types were built by rewriting strings, not by
        // going back through the checker's table (#814).
        Type::UnresolvedNamed(name) => bare_type_name(name),
        Type::UnresolvedGeneric { name, args } => {
            let base = bare_type_name(name);
            let mut parts = Vec::with_capacity(args.len());
            for arg in args {
                let GenericArg::Type(inner) = arg else { return None };
                parts.push(type_arg_key(inner, type_names)?);
            }
            format!("{}${}", base, parts.join("$"))
        }
        Type::Generic { base, args } => {
            let base = bare_type_name(type_names.get(base)?);
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
            let base_name = bare_type_name(name);
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
            let name = bare_type_name(type_names.get(id)?);
            cache.get(&name).map(|(size, _)| *size)
        }
        // A nested instantiation is as wide as *its* layout — `One<One<Big>>` has
        // to see 24, not the 8 the shared `One` layout reports.
        Type::Generic { base, args } => {
            let base_name = bare_type_name(type_names.get(base)?);
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

/// How deeply a type argument nests other types. Used only to order layout
/// construction, so the exact numbers don't matter beyond inner < outer.
fn type_depth(ty: &Type) -> u32 {
    use rask_types::GenericArg;
    match ty {
        Type::Generic { args, .. } | Type::UnresolvedGeneric { args, .. } => {
            1 + args
                .iter()
                .filter_map(|a| match a {
                    GenericArg::Type(t) => Some(type_depth(t)),
                    _ => None,
                })
                .max()
                .unwrap_or(0)
        }
        Type::Tuple(elems) | Type::Union(elems) => {
            1 + elems.iter().map(type_depth).max().unwrap_or(0)
        }
        Type::Slice(inner) | Type::RawPtr(inner) => 1 + type_depth(inner),
        Type::Array { elem, .. } => 1 + type_depth(elem),
        Type::Result { ok, err } => 1 + type_depth(ok).max(type_depth(err)),
        _ => 0,
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
            .map(|n| Type::UnresolvedNamed(bare_type_name(n)))
            .unwrap_or_else(|| ty.clone()),
        Type::Generic { base, args } => {
            let Some(base_name) = type_names.get(base).map(|n| bare_type_name(n)) else {
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
                    out.push((bare_type_name(name), arg_tys));
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
                out.push((bare_type_name(name), arg_tys));
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
        Type::Slice(inner) | Type::RawPtr(inner) => {
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
    mono.set_trait_coercions(&program.trait_coercions);

    if !mono.add_entry("main") {
        if !entryless {
            return Err(MonomorphizeError::NoEntryPoint);
        }
        mono.add_all_plain_fn_roots();
    }
    mono.add_module_const_roots();
    mono.add_exported_roots();

    mono.run();

    if let Some(ambiguous) = mono.ambiguous_methods.first() {
        return Err(MonomorphizeError::AmbiguousMethod {
            type_name: ambiguous.type_name.clone(),
            method: ambiguous.method.clone(),
            span: ambiguous.span,
        });
    }

    // Compute layouts for concrete (non-generic) struct/enum types.
    let mut layout_cache = LayoutCache::new();
    let mut struct_layouts = Vec::new();
    let mut enum_layouts = Vec::new();

    let sorted = topo_sort_type_decls(decls);
    for idx in sorted {
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
                    &Type::UnresolvedNamed(a.target.clone()),
                    &layout_cache,
                );
                layout_cache.insert(a.name.clone(), (size, align));
            }
            _ => {}
        }
    }

    // Compute layouts for generic struct/enum types. The 8-byte-everything
    // layout model means all scalar type parameters produce the same field
    // sizes, so a single layout per generic struct suffices. Use i64 as the
    // placeholder type for each type parameter.
    for decl in decls {
        match &decl.kind {
            DeclKind::Struct(s) if !s.type_params.is_empty() => {
                let placeholder_args: Vec<Type> = s.type_params.iter()
                    .map(|_| Type::I64)
                    .collect();
                let mut layout = compute_struct_layout(decl, &placeholder_args, &layout_cache);
                // Strip type params from name so struct literals ("Box") match
                let base_name = s.name.split('<').next().unwrap_or(&s.name).to_string();
                layout.name = base_name.clone();
                layout_cache.insert(base_name, (layout.size, layout.align));
                struct_layouts.push(layout);
            }
            DeclKind::Enum(e) if !e.type_params.is_empty() => {
                let placeholder_args: Vec<Type> = e.type_params.iter()
                    .map(|_| Type::I64)
                    .collect();
                let mut layout = compute_enum_layout(decl, &placeholder_args, &layout_cache);
                let base_name = e.name.split('<').next().unwrap_or(&e.name).to_string();
                layout.name = base_name.clone();
                layout_cache.insert(base_name, (layout.size, layout.align));
                enum_layouts.push(layout);
            }
            _ => {}
        }
    }

    // One layout per *instantiation*, but only where the shared one is too small.
    // The placeholder above gives every type parameter a word, which is right for
    // a scalar and right for anything boxed (a `Vec`, a `Map`, a `Shared`) since
    // those are pointers. It is wrong for anything that *is* its bytes — a struct,
    // enum, union, tuple, array, or a `string`: `One<Big>` stored 24 bytes into an
    // 8-byte slot and segfaulted on the read back (#781).
    //
    // Emitted only when the instantiated layout is bigger than the shared one, so
    // `One<i32>` keeps using the shared layout and nothing that worked changes
    // shape.
    {
        let type_names: HashMap<rask_types::TypeId, String> = program
            .types
            .iter()
            .enumerate()
            .map(|(i, def)| {
                let name = match def {
                    rask_types::TypeDef::Struct { name, .. }
                    | rask_types::TypeDef::Enum { name, .. }
                    | rask_types::TypeDef::Trait { name, .. }
                    | rask_types::TypeDef::Union { name, .. }
                    | rask_types::TypeDef::NominalAlias { name, .. } => name.clone(),
                };
                (rask_types::TypeId(i as u32), name)
            })
            .collect();

        let generic_decls: HashMap<String, &Decl> = decls
            .iter()
            .filter_map(|d| match &d.kind {
                // PC1 counts: a single letter in a field or payload type makes
                // the type generic whether or not `<T>` was written. Gating on
                // the explicit list meant an implicit-param struct never got an
                // instance layout at all, so a `Pair<i32, string>` kept the
                // shared one — where every parameter is a single word — and its
                // 16-byte string field was written into an 8-byte slot (#913).
                DeclKind::Struct(s) if !rask_types::struct_type_param_names(s).is_empty() => {
                    Some((s.name.split('<').next().unwrap_or(&s.name).to_string(), d))
                }
                DeclKind::Enum(e) if !rask_types::enum_type_param_names(e).is_empty() => {
                    Some((e.name.split('<').next().unwrap_or(&e.name).to_string(), d))
                }
                _ => None,
            })
            .collect();

        let mut instances: Vec<(String, Vec<Type>)> = Vec::new();
        if !generic_decls.is_empty() {
            for ty in program
                .node_types
                .values()
                .chain(mono.instantiated_node_types.values())
            {
                collect_generic_instances(ty, &type_names, &mut instances);
            }
        }

        // Shallowest first. `One<One<Big>>` can only be sized once `One$Big` is in
        // the cache, and depth is enough of an order for that — a type argument is
        // always shallower than the instantiation that holds it.
        instances.sort_by_key(|(_, args)| args.iter().map(type_depth).max().unwrap_or(0));

        let mut emitted: HashSet<String> = HashSet::new();
        for (base, args) in instances {
            let Some(decl) = generic_decls.get(&base) else { continue };
            let Some(instance_name) = generic_instance_name(&base, &args, &type_names) else {
                continue;
            };
            if !emitted.insert(instance_name.clone()) {
                continue;
            }
            // Only when an argument can actually overflow the shared slot.
            let overflows = args.iter().any(|a| {
                inline_arg_size(a, &type_names, &program.types, &layout_cache)
                    .is_some_and(|size| size > 8)
            });
            if !overflows {
                continue;
            }
            // The type arguments have to be nameable to the layout code too —
            // `type_size_align` reads the cache by name, and a `Named(id)` isn't
            // one. A nested instantiation is named by its own instance layout.
            let named_args: Vec<Type> = args
                .iter()
                .map(|a| arg_as_cache_name(a, &type_names, &layout_cache))
                .collect();
            let shared = layout_cache.get(&base).map(|(size, _)| *size).unwrap_or(0);
            match &decl.kind {
                DeclKind::Struct(_) => {
                    let mut layout = compute_struct_layout(decl, &named_args, &layout_cache);
                    if layout.size <= shared {
                        continue;
                    }
                    layout.name = instance_name.clone();
                    layout_cache.insert(instance_name, (layout.size, layout.align));
                    struct_layouts.push(layout);
                }
                DeclKind::Enum(_) => {
                    let mut layout = compute_enum_layout(decl, &named_args, &layout_cache);
                    if layout.size <= shared {
                        continue;
                    }
                    layout.name = instance_name.clone();
                    layout_cache.insert(instance_name, (layout.size, layout.align));
                    enum_layouts.push(layout);
                }
                _ => {}
            }
        }
    }

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
        call_rewrites: mono.call_rewrites,
        instantiated_node_types: mono.instantiated_node_types,
        instantiated_call_targets: mono.instantiated_call_targets,
        instantiated_error_wraps: mono.instantiated_error_wraps,
        instantiated_fallback_keeps_shape: mono.instantiated_fallback_keeps_shape,
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
    /// Two types share a name, both need `Type_method`, and only one can have
    /// it. Compiled functions are keyed by that string all the way through
    /// codegen, so the call has no body to reach.
    AmbiguousMethod {
        type_name: String,
        method: String,
        span: rask_ast::Span,
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
            Self::AmbiguousMethod { type_name, method, .. } => write!(
                f,
                "two different types named `{0}` both define `{1}`, and this call needs \
                 the one that isn't in scope here. Rename one of them — the compiled \
                 program identifies the method as `{0}_{1}`, which can only mean one of \
                 the two",
                type_name, method,
            ),
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
                        ty: ty.to_string(),
                        is_take: false,
                        is_mutate: false, is_deleting: false,
                        default: None,
                    })
                    .collect(),
                ret_ty: ret_ty.map(|s| s.to_string()),
                context_clauses: vec![],
                body,
                is_pub: false,
                is_private: false,
                is_comptime: false,
                is_unsafe: false,
                abi: None,
                attrs: vec![],
                doc: None,
                span: sp(),
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
                    })
                    .collect(),
                params: params
                    .into_iter()
                    .map(|(n, ty)| Param {
                        name: n.to_string(),
                        name_span: sp(),
                        ty: ty.to_string(),
                        is_take: false,
                        is_mutate: false, is_deleting: false,
                        default: None,
                    })
                    .collect(),
                ret_ty: ret_ty.map(|s| s.to_string()),
                context_clauses: vec![],
                body,
                is_pub: false,
                is_private: false,
                is_comptime: false,
                is_unsafe: false,
                abi: None,
                attrs: vec![],
                doc: None,
                span: sp(),
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
            trait_coercions: std::collections::HashMap::new(),
            error_wraps: std::collections::HashMap::new(),
            fallback_keeps_shape: std::collections::HashSet::new(),
            try_chain_placement: std::collections::HashMap::new(),
            unsafe_ops: Vec::new(),
            span_types: std::collections::HashMap::new(),
            channel_send_sites: std::collections::HashSet::new(),
            inferred_fn_ret: std::collections::HashMap::new(),
            inferred_fn_params: std::collections::HashMap::new(),
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
                        Field { name: "x".to_string(), name_span: sp(), ty: "i32".to_string(), visibility: FieldVisibility::Package, attrs: vec![], default: None },
                        Field { name: "y".to_string(), name_span: sp(), ty: "i32".to_string(), visibility: FieldVisibility::Package, attrs: vec![], default: None },
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
                        Field { name: "kind".to_string(), name_span: sp(), ty: "Kind".to_string(), visibility: FieldVisibility::Package, attrs: vec![], default: None },
                        Field { name: "value".to_string(), name_span: sp(), ty: "i32".to_string(), visibility: FieldVisibility::Package, attrs: vec![], default: None },
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
                                Field { name: "x".to_string(), name_span: sp(), ty: "i32".to_string(), visibility: FieldVisibility::Package, attrs: vec![], default: None },
                                Field { name: "y".to_string(), name_span: sp(), ty: "i32".to_string(), visibility: FieldVisibility::Package, attrs: vec![], default: None },
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
            assert_eq!(f.params[0].ty, "i32"); // substituted
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
        let mut mono = Monomorphizer::new(&decls, &empty_type_args);
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
        let mut mono = Monomorphizer::new(&decls, &empty_type_args);
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
                    ty: ty.to_string(),
                    is_take: false,
                    is_mutate: false, is_deleting: false,
                    default: None,
                })
                .collect(),
            ret_ty: ret_ty.map(|s| s.to_string()),
            context_clauses: vec![],
            body,
            is_pub: false,
            is_private: false,
            is_comptime: false,
            is_unsafe: false,
            abi: None,
            attrs: vec![],
            doc: None,
            span: sp(),
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
                        Field { name: "x".to_string(), name_span: sp(), ty: "i32".to_string(), visibility: FieldVisibility::Package, attrs: vec![], default: None },
                        Field { name: "y".to_string(), name_span: sp(), ty: "i32".to_string(), visibility: FieldVisibility::Package, attrs: vec![], default: None },
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
        let mut mono = Monomorphizer::new(&decls, &empty_type_args);
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
                    trait_names: vec![],
                    target_ty: "Point".to_string(),
                    methods: vec![
                        make_method("distance", vec![("self", "Point")], Some("f64"), vec![return_stmt(None)]),
                    ],
                    doc: None,
                    is_unsafe: false,
                    is_scoped: false,
                    where_bounds: vec![],
                }),
                span: sp(),
            },
        ];

        let empty_type_args = std::collections::HashMap::new();
        let mut mono = Monomorphizer::new(&decls, &empty_type_args);
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
                    trait_names: vec![],
                    target_ty: "Counter".to_string(),
                    methods: vec![
                        make_method("increment", vec![("self", "Counter")], None, vec![return_stmt(None)]),
                    ],
                    doc: None,
                    is_unsafe: false,
                    is_scoped: false,
                    where_bounds: vec![],
                }),
                span: sp(),
            },
        ];

        let empty_type_args = std::collections::HashMap::new();
        let mut mono = Monomorphizer::new(&decls, &empty_type_args);
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
        let mut mono = Monomorphizer::new(&decls, &empty_type_args);
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
        let mut mono = Monomorphizer::new(&decls, &empty_type_args);
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
        let mut mono = Monomorphizer::new(&decls, &empty_type_args);
        mono.add_entry("main");
        mono.run();

        let names: Vec<&str> = mono.results.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"Color_default"), "enum method should be reachable: {:?}", names);
    }
}
