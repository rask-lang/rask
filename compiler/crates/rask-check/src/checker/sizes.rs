// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! What the checker asks of layout: sizes for the copy rule, `reflect` and
//! `transmute`, from the declarations it recorded.

use std::collections::HashMap;

use rask_ast::decl::Decl;
use rask_ast::ty::TypeExpr;
use rask_layout::{
    compute_enum_layout, compute_shared_enum_layout, compute_shared_struct_layout, compute_struct_layout,
    compute_union_layout, field_type, generic_instance_name, generic_is_one_word, known_size_align,
    ordering_layout, substitute_inside, type_size_align, FieldLayout, LayoutCache, StructLayout,
};
use rask_types::{GenericArg, Type};

pub fn layout_spelling(ty: &Type, types: &crate::TypeTable) -> Option<Type> {
    spell(ty, types, false)
}

/// `layout_spelling` for a question that has to be answered whatever the type
/// holds — the copy rule inside a generic body. A type parameter or an open
/// variable stands in as a word, the size the shared layout gives it.
fn spell_lenient(ty: &Type, types: &crate::TypeTable) -> Type {
    spell(ty, types, true).expect("a lenient spelling always answers")
}

fn spell(ty: &Type, types: &crate::TypeTable, lenient: bool) -> Option<Type> {
    let each = |tys: &[Type]| tys.iter().map(|t| spell(t, types, lenient)).collect::<Option<Vec<_>>>();
    let args_of = |args: &[GenericArg]| {
        args.iter()
            .map(|a| match a {
                GenericArg::Type(t) => {
                    Some(GenericArg::Type(Box::new(spell(t, types, lenient)?)))
                }
                other => Some(other.clone()),
            })
            .collect::<Option<Vec<_>>>()
    };
    let layout_spelling = |t: &Type, types: &crate::TypeTable| spell(t, types, lenient);
    Some(match ty {
        Type::Var(_) | Type::Assoc { .. } | Type::Error if lenient => Type::I64,
        Type::UnresolvedNamed(name) if lenient && rask_ast::type_params::is_type_param_name(name) => Type::I64,
        Type::Var(_) | Type::Assoc { .. } | Type::Error => return None,
        Type::UnresolvedNamed(name) if rask_ast::type_params::is_type_param_name(name) => return None,
        Type::Named(id) => Type::UnresolvedNamed(types.type_name(*id)),
        Type::Generic { base, args } => Type::UnresolvedGeneric {
            name: types.type_name(*base),
            args: args_of(args)?,
        },
        Type::UnresolvedGeneric { name, args } => Type::UnresolvedGeneric {
            name: name.clone(),
            args: args_of(args)?,
        },
        Type::Tuple(elems) => Type::Tuple(each(elems)?),
        Type::Union(members) => Type::Union(each(members)?),
        Type::Array { elem, len } => Type::Array { elem: Box::new(layout_spelling(elem, types)?), len: *len },
        Type::Result { ok, err } => Type::Result {
            ok: Box::new(layout_spelling(ok, types)?),
            err: Box::new(layout_spelling(err, types)?),
        },
        Type::RawPtr(inner) => Type::RawPtr(Box::new(layout_spelling(inner, types)?)),
        Type::SimdVector { elem, lanes } => Type::SimdVector {
            elem: Box::new(layout_spelling(elem, types)?),
            lanes: *lanes,
        },
        other => other.clone(),
    })
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

impl crate::TypeTable {
    /// Record a struct, enum or union declaration for laying out.
    pub(crate) fn note_layout_decl(&mut self, id: crate::TypeId, kind: rask_ast::decl::DeclKind) {
        // A stdlib generic with no fields (`Sequence<T>`) stands in for a
        // value the runtime holds: one word, whatever its declaration says.
        let stand_in = self.stdlib_mode
            && matches!(&kind, rask_ast::decl::DeclKind::Struct(s)
                if s.fields.is_empty() && !rask_ast::type_params::struct_type_param_names(s).is_empty());
        if stand_in {
            self.layout_stand_ins.insert(self.type_name(id));
            return;
        }
        let decl = Decl { id: rask_ast::NodeId(0), kind, span: rask_ast::Span::new(0, 0) };
        self.layout_decls.insert(self.type_name(id), decl);
    }

    /// Size and alignment of `ty`, or `None` for a type with no layout yet: one
    /// naming a type parameter or an inference variable nobody solved.
    pub fn size_align(&self, ty: &Type) -> Option<(u32, u32)> {
        let spelled = layout_spelling(ty, self)?;
        let mut cache = LayoutCache::new();
        self.fill_layouts(&spelled, &mut cache, &mut Vec::new());
        known_size_align(&spelled, &cache)
    }

    /// `size_align` for the copy rule, which has to answer inside a generic
    /// body too: a type parameter counts as a word there.
    ///
    /// Measured with 64-bit pointers on every target (`mem.value/VS7a`).
    pub fn copy_size(&self, ty: &Type) -> usize {
        let spelled = spell_lenient(ty, self);
        let mut cache = LayoutCache::for_copy_rule();
        self.fill_layouts(&spelled, &mut cache, &mut Vec::new());
        type_size_align(&spelled, &cache).0 as usize
    }

    /// Size and alignment of a type already spelled by name, as a declaration
    /// field is.
    pub fn size_align_spelled(&self, spelled: &Type) -> (u32, u32) {
        let mut cache = LayoutCache::new();
        self.fill_layouts(spelled, &mut cache, &mut Vec::new());
        type_size_align(spelled, &cache)
    }

    /// The layout of the struct `name`, with these type arguments.
    pub fn struct_layout_named(&self, name: &str, args: &[Type]) -> Option<StructLayout> {
        self.struct_layout_in(name, args, LayoutCache::new())
    }

    /// `struct_layout_named` as the copy rule measures it. What a `@small`
    /// error explains itself with, so it has to agree with `copy_size`.
    fn struct_layout_for_copy_rule(&self, name: &str, args: &[Type]) -> Option<StructLayout> {
        self.struct_layout_in(name, args, LayoutCache::for_copy_rule())
    }

    fn struct_layout_in(&self, name: &str, args: &[Type], mut cache: LayoutCache) -> Option<StructLayout> {
        let decl = self.layout_decls.get(name)?;
        let rask_ast::decl::DeclKind::Struct(_) = &decl.kind else { return None };
        for ty in args {
            self.fill_layouts(ty, &mut cache, &mut Vec::new());
        }
        self.fill_decl_fields(decl, args, &mut cache, &mut vec![name.to_string()]);
        Some(compute_struct_layout(decl, args, &cache))
    }

    /// The field of struct `ty` that runs past byte `limit` first, in memory
    /// order: its name, its size, and its type. What a `@small` error names.
    pub fn first_field_past(&self, ty: &Type, limit: u32) -> Option<FieldLayout> {
        let (Type::UnresolvedNamed(name) | Type::UnresolvedGeneric { name, .. }) = layout_spelling(ty, self)? else {
            return None;
        };
        let args: Vec<Type> = match layout_spelling(ty, self)? {
            Type::UnresolvedGeneric { args, .. } => args
                .into_iter()
                .filter_map(|a| match a {
                    GenericArg::Type(t) => Some(*t),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        let mut layout = self.struct_layout_for_copy_rule(&name, &args)?;
        layout.fields.sort_by_key(|f| f.offset);
        layout.fields.into_iter().find(|f| f.offset + f.size > limit)
    }

    /// Put the layout of every declared type `ty` holds by value into `cache`,
    /// innermost first, so `type_size_align` finds each by name.
    fn fill_layouts(&self, ty: &Type, cache: &mut LayoutCache, visiting: &mut Vec<String>) {
        match ty {
            Type::UnresolvedNamed(name) => self.fill_named(name, &[], cache, visiting),
            Type::UnresolvedGeneric { name, args } => {
                // A handle's arguments live behind its pointer.
                if generic_is_one_word(name) || rask_stdlib::is_runtime_handle(name) {
                    return;
                }
                let args: Vec<Type> = args
                    .iter()
                    .filter_map(|a| match a {
                        GenericArg::Type(t) => Some((**t).clone()),
                        _ => None,
                    })
                    .collect();
                // A declared generic lays out what it holds itself: its fields,
                // with the arguments written in. Laying out every argument first
                // followed ones it only holds behind a pointer, so the order of
                // asking decided sizes: `Holder { kids: Stack<Expr> }` laid
                // `Expr` out while `Holder` was half-done, and `Expr`'s
                // `Boxed(Holder)` payload got a guessed word (#1436's cycle,
                // on demand).
                if !self.layout_decls.contains_key(name.as_str()) {
                    for arg in &args {
                        self.fill_layouts(arg, cache, visiting);
                    }
                }
                self.fill_named(name, &args, cache, visiting);
            }
            Type::Tuple(elems) | Type::Union(elems) => {
                for e in elems {
                    self.fill_layouts(e, cache, visiting);
                }
            }
            Type::Array { elem, .. } | Type::SimdVector { elem, .. } => self.fill_layouts(elem, cache, visiting),
            Type::Result { ok, err } => {
                self.fill_layouts(ok, cache, visiting);
                self.fill_layouts(err, cache, visiting);
            }
            _ => {}
        }
    }

    fn fill_named(&self, name: &str, args: &[Type], cache: &mut LayoutCache, visiting: &mut Vec<String>) {
        let key = generic_instance_name(name, args, &HashMap::new()).unwrap_or_else(|| name.to_string());
        if cache.contains_key(&key) || visiting.contains(&key) {
            return;
        }
        if self.layout_stand_ins.contains(name) {
            let w = cache.word();
            cache.insert(key, (w, w));
            return;
        }
        let Some(decl) = self.layout_decls.get(name) else {
            // The compiler registers `Ordering` without a declaration.
            let ordering = ordering_layout();
            if name == ordering.name {
                cache.insert(key, (ordering.size, ordering.align));
                return;
            }
            // A nominal newtype is laid out as what it wraps.
            if let Some(rask_types::TypeDef::NominalAlias { underlying, .. }) =
                self.get_type_id(name).and_then(|id| self.get(id))
            {
                let wrapped = spell_lenient(underlying, self);
                visiting.push(key.clone());
                self.fill_layouts(&wrapped, cache, visiting);
                visiting.pop();
                let size_align = type_size_align(&wrapped, cache);
                cache.insert(key, size_align);
            }
            return;
        };
        visiting.push(key.clone());
        self.fill_decl_fields(decl, args, cache, visiting);
        visiting.pop();
        use rask_ast::decl::DeclKind;
        let generic = match &decl.kind {
            DeclKind::Struct(s) => !rask_ast::type_params::struct_type_param_names(s).is_empty(),
            DeclKind::Enum(e) => !rask_ast::type_params::enum_type_param_names(e).is_empty(),
            _ => false,
        };
        let size_align = match &decl.kind {
            DeclKind::Struct(_) if generic && args.is_empty() => {
                let l = compute_shared_struct_layout(decl, cache);
                (l.size, l.align)
            }
            DeclKind::Struct(_) => {
                let l = compute_struct_layout(decl, args, cache);
                (l.size, l.align)
            }
            DeclKind::Enum(_) if generic && args.is_empty() => {
                let l = compute_shared_enum_layout(decl, cache);
                (l.size, l.align)
            }
            DeclKind::Enum(_) => {
                let l = compute_enum_layout(decl, args, cache);
                (l.size, l.align)
            }
            DeclKind::Union(_) => {
                let l = compute_union_layout(decl, cache);
                (l.size, l.align)
            }
            _ => return,
        };
        cache.insert(key, size_align);
    }

    /// The layouts a declaration's fields need, with `args` written in.
    fn fill_decl_fields(&self, decl: &Decl, args: &[Type], cache: &mut LayoutCache, visiting: &mut Vec<String>) {
        use rask_ast::decl::DeclKind;
        let (params, fields): (Vec<String>, Vec<&TypeExpr>) = match &decl.kind {
            DeclKind::Struct(s) => (rask_ast::type_params::struct_type_param_names(s), s.fields.iter().map(|f| &f.ty).collect()),
            DeclKind::Enum(e) => (
                rask_ast::type_params::enum_type_param_names(e),
                e.variants.iter().flat_map(|v| v.fields.iter().map(|f| &f.ty)).collect(),
            ),
            DeclKind::Union(u) => (Vec::new(), u.fields.iter().map(|f| &f.ty).collect()),
            _ => return,
        };
        let subst: HashMap<&str, &Type> = params.iter().map(String::as_str).zip(args.iter()).collect();
        for field in fields {
            let ty = substitute_inside(&field_type(field), &subst);
            self.fill_layouts(&ty, cache, visiting);
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────

