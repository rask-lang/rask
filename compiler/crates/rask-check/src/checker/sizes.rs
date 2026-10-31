// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! What the checker asks of layout: sizes for the copy rule, `reflect` and
//! `transmute`, from the declarations it recorded.

use std::collections::HashMap;

use rask_layout::{
    compute_enum_layout, compute_shared_enum_layout, compute_shared_struct_layout, compute_struct_layout,
    compute_union_layout, generic_instance_name, generic_is_one_word, known_size_align, ordering_layout,
    spelled, substitute_inside, type_size_align, FieldLayout, LayoutCache, StructLayout, Unsolved,
};
use rask_types::{GenericArg, Type, TypeDef};

/// A checked type spelled by name, as the layout cache is keyed, or `None`
/// for one with no layout yet (a type parameter, an open variable).
pub fn layout_spelling(ty: &Type, types: &crate::TypeTable) -> Option<Type> {
    spelled(ty, &|id| types.type_name(id), Unsolved::Refuse)
}

/// `layout_spelling` for a question that has to be answered whatever the type
/// holds — the copy rule inside a generic body. A type parameter or an open
/// variable stands in as a word, the size the shared layout gives it.
fn spell_lenient(ty: &Type, types: &crate::TypeTable) -> Type {
    spelled(ty, &|id| types.type_name(id), Unsolved::Word).expect("a lenient spelling always answers")
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
    /// Record a struct, enum or union for laying out, under the name its
    /// layout is keyed by.
    ///
    /// By id rather than looked up by name later: a program's `interface
    /// Parser` takes the name from the stdlib's `cli.Parser` struct, and the
    /// struct still needs its layout.
    pub(crate) fn note_layout_type(&mut self, id: crate::TypeId) {
        // A stdlib generic with no fields (`Sequence<T>`) stands in for a
        // value the runtime holds: one word, whatever its declaration says.
        let stand_in = self.stdlib_mode
            && matches!(self.get(id), Some(TypeDef::Struct { fields, type_params, .. })
                if fields.is_empty() && !type_params.is_empty());
        if stand_in {
            self.layout_stand_ins.insert(self.type_name(id));
            return;
        }
        self.layout_types.insert(self.type_name(id), id);
    }

    /// The struct, enum or union laid out under `name`.
    fn layout_def(&self, name: &str) -> Option<&TypeDef> {
        self.get(*self.layout_types.get(name)?)
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
        let def = self.layout_def(name)?;
        let TypeDef::Struct { .. } = def else { return None };
        for ty in args {
            self.fill_layouts(ty, &mut cache, &mut Vec::new());
        }
        self.fill_decl_fields(def, args, &mut cache, &mut vec![name.to_string()]);
        Some(compute_struct_layout(def, args, &|id| self.type_name(id), &cache))
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
                if !self.layout_types.contains_key(name.as_str()) {
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
        let Some(def) = self.layout_def(name) else {
            // The compiler registers `Ordering` without a declaration.
            let ordering = ordering_layout();
            if name == ordering.name {
                cache.insert(key, (ordering.size, ordering.align));
                return;
            }
            // A nominal newtype is laid out as what it wraps.
            if let Some(TypeDef::NominalAlias { underlying, .. }) = self.get_type_id(name).and_then(|id| self.get(id)) {
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
        self.fill_decl_fields(def, args, cache, visiting);
        visiting.pop();
        let name_of = &|id| self.type_name(id);
        let size_align = match def {
            TypeDef::Struct { type_params, .. } if !type_params.is_empty() && args.is_empty() => {
                let l = compute_shared_struct_layout(def, name_of, cache);
                (l.size, l.align)
            }
            TypeDef::Struct { .. } => {
                let l = compute_struct_layout(def, args, name_of, cache);
                (l.size, l.align)
            }
            TypeDef::Enum { type_params, .. } if !type_params.is_empty() && args.is_empty() => {
                let l = compute_shared_enum_layout(def, name_of, cache);
                (l.size, l.align)
            }
            TypeDef::Enum { .. } => {
                let l = compute_enum_layout(def, args, name_of, cache);
                (l.size, l.align)
            }
            TypeDef::Union { .. } => {
                let l = compute_union_layout(def, name_of, cache);
                (l.size, l.align)
            }
            _ => return,
        };
        cache.insert(key, size_align);
    }

    /// The layouts a declaration's fields need, with `args` written in.
    fn fill_decl_fields(&self, def: &TypeDef, args: &[Type], cache: &mut LayoutCache, visiting: &mut Vec<String>) {
        let (params, fields): (&[String], Vec<&Type>) = match def {
            TypeDef::Struct { type_params, fields, .. } => (type_params, fields.iter().map(|(_, t)| t).collect()),
            TypeDef::Enum { type_params, variants, .. } => {
                (type_params, variants.iter().flat_map(|(_, payload)| payload).collect())
            }
            TypeDef::Union { fields, .. } => (&[], fields.iter().map(|(_, t)| t).collect()),
            _ => return,
        };
        let subst: HashMap<&str, &Type> = params.iter().map(String::as_str).zip(args.iter()).collect();
        for field in fields {
            let declared = spelled(field, &|id| self.type_name(id), Unsolved::KeepParams)
                .expect("keeping parameters always answers");
            self.fill_layouts(&substitute_inside(&declared, &subst), cache, visiting);
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────

