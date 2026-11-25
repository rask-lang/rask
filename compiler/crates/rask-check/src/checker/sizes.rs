// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! What the checker asks of layout: sizes for the copy rule, `reflect` and
//! `transmute`, from the declarations it recorded.

use std::collections::HashMap;

use rask_layout::{
    compute_enum_layout, compute_shared_enum_layout, compute_shared_struct_layout, compute_struct_layout,
    compute_union_layout, generic_is_one_word, MAX_VALUE_SIZE, known_size_align, settle, substitute_inside, type_size_align,
    EnumLayout, FieldLayout, IsParam, LayoutCache, LayoutKey, StructLayout, Unsolved,
};
use rask_types::{GenericArg, Type, TypeDef, TypeId};

impl crate::TypeTable {
    /// Record a struct, enum or union for laying out.
    ///
    /// By id, never looked up by name later: a program's `interface Parser`
    /// takes the name from the stdlib's `cli.Parser` struct, and the struct
    /// still needs its layout.
    pub(crate) fn note_layout_type(&mut self, id: TypeId) {
        // A stdlib generic with no fields (`Sequence<T>`) stands in for a
        // value the runtime holds: one word, whatever its declaration says.
        let stand_in = self.stdlib_mode
            && matches!(self.get(id), Some(TypeDef::Struct { fields, type_params, .. })
                if fields.is_empty() && !type_params.is_empty());
        if stand_in {
            self.layout_stand_ins.insert(id);
            return;
        }
        self.layout_types.insert(id);
    }

    /// Size and alignment of `ty`, or `None` for a type with no layout yet: one
    /// naming a type parameter (`is_param` says which names are) or an
    /// inference variable nobody solved.
    pub fn size_align(&self, ty: &Type, is_param: IsParam) -> Option<(u32, u32)> {
        let ty = settle(ty, Unsolved::Refuse, is_param)?;
        let mut cache = LayoutCache::new(self.target());
        self.fill_layouts(&ty, &mut cache, &mut Vec::new());
        known_size_align(&ty, &cache)
    }

    /// VS12: whether `ty` is over the 64 KiB a value can take. `Some(Some(bytes))`
    /// for an array, whose size is exact; `Some(None)` for a struct, tuple,
    /// enum or union whose parts add up past it. A type with a part already
    /// over is left to that part's report, so two 40 KB arrays in a struct are
    /// one error, at the struct.
    ///
    /// `None` while declarations are still being collected: a field naming a
    /// type declared further down, or the type it sits in, has no layout yet.
    /// Each declared struct, enum and union is measured once collection ends.
    pub(crate) fn too_large(&self, ty: &Type) -> Option<Option<u64>> {
        if !self.declarations_settled {
            return None;
        }
        // Inside `struct Chain<T>`, `Wrapper<Chain<T>>` has no size until `T`
        // has one; read as a concrete type, `T` sent layout looking for a
        // declaration that isn't there.
        let in_scope = |n: &str| self.is_type_param_in_scope(n);
        let is_param: IsParam = &in_scope;
        if let Type::Array { elem, len } = ty {
            let size = self.rule_size(elem, Unsolved::Refuse, is_param)?;
            let bytes = u64::from(size).saturating_mul(*len as u64);
            return (bytes > u64::from(MAX_VALUE_SIZE) && size <= MAX_VALUE_SIZE).then_some(Some(bytes));
        }
        if self.rule_size(ty, Unsolved::Refuse, is_param).is_none_or(|size| size <= MAX_VALUE_SIZE) {
            return None;
        }
        let part_sizes: Vec<u32> = match ty {
            Type::Tuple(elems) => elems.iter().filter_map(|e| self.rule_size(e, Unsolved::Refuse, is_param)).collect(),
            Type::Named(_) | Type::Generic { .. } => {
                let (id, args) = match ty {
                    Type::Named(id) => (*id, Vec::new()),
                    Type::Generic { base, args } => (*base, type_args_of(args)),
                    _ => unreachable!(),
                };
                match self.get(id) {
                    Some(TypeDef::Struct { .. }) => {
                        self.struct_layout_in(id, &args, LayoutCache::for_copy_rule())?.fields.iter().map(|f| f.size).collect()
                    }
                    Some(TypeDef::Enum { .. }) => self
                        .enum_layout_in(id, &args, LayoutCache::for_copy_rule())?
                        .variants
                        .iter()
                        .flat_map(|v| v.fields.iter().map(|f| f.size))
                        .collect(),
                    Some(TypeDef::Union { fields, .. }) => fields
                        .iter()
                        .filter_map(|(_, f)| self.rule_size(f, Unsolved::Refuse, is_param))
                        .collect(),
                    _ => Vec::new(),
                }
            }
            _ => Vec::new(),
        };
        (!part_sizes.iter().any(|&s| s > MAX_VALUE_SIZE)).then_some(None)
    }

    /// Sizes the language's rules read, as opposed to the target's layout:
    /// counted with 64-bit pointers on every target, so a program compiles
    /// everywhere or nowhere. The copy rule (`mem.value/VS7a`) asks with
    /// `Unsolved::Word`, since it has to answer inside a generic body where a
    /// type parameter counts as a word. The value cap (VS12) asks with
    /// `Unsolved::Refuse`; measured with the target's pointers it let
    /// `[Vec<i64>; 8193]` through on wasm32 and stopped it on x86_64 (#2066).
    fn rule_size(&self, ty: &Type, unsolved: Unsolved, is_param: IsParam) -> Option<u32> {
        let ty = settle(ty, unsolved, is_param)?;
        let mut cache = LayoutCache::for_copy_rule();
        self.fill_layouts(&ty, &mut cache, &mut Vec::new());
        match unsolved {
            // The copy rule has to answer for every type; what layout can't
            // size counts as a word.
            Unsolved::Word => Some(type_size_align(&ty, &cache).0),
            Unsolved::Refuse | Unsolved::KeepParams => known_size_align(&ty, &cache).map(|(size, _)| size),
        }
    }

    /// The size the copy rule compares against 16 bytes.
    pub fn copy_size(&self, ty: &Type, is_param: IsParam) -> usize {
        self.rule_size(ty, Unsolved::Word, is_param).expect("a lenient settling always answers") as usize
    }

    /// Size and alignment of a type with nothing left to settle, as a laid-out
    /// field's is.
    pub fn size_align_settled(&self, ty: &Type) -> (u32, u32) {
        let mut cache = LayoutCache::new(self.target());
        self.fill_layouts(ty, &mut cache, &mut Vec::new());
        type_size_align(ty, &cache)
    }

    /// The layout of the struct `id`, with these type arguments.
    pub fn struct_layout(&self, id: TypeId, args: &[Type]) -> Option<StructLayout> {
        self.struct_layout_in(id, args, LayoutCache::new(self.target()))
    }

    /// The layout of the enum `id`, with these type arguments.
    pub fn enum_layout(&self, id: TypeId, args: &[Type]) -> Option<EnumLayout> {
        self.enum_layout_in(id, args, LayoutCache::new(self.target()))
    }

    fn enum_layout_in(&self, id: TypeId, args: &[Type], mut cache: LayoutCache) -> Option<EnumLayout> {
        if !self.layout_types.contains(&id) {
            return None;
        }
        let def = self.get(id)?;
        let TypeDef::Enum { .. } = def else { return None };
        for ty in args {
            self.fill_layouts(ty, &mut cache, &mut Vec::new());
        }
        self.fill_decl_fields(def, args, &mut cache, &mut vec![LayoutKey::instance(id, args.to_vec())]);
        Some(compute_enum_layout(def, args, &cache))
    }

    /// `struct_layout` as the copy rule measures it. What a `@small` error
    /// explains itself with, so it has to agree with `copy_size`.
    fn struct_layout_for_copy_rule(&self, id: TypeId, args: &[Type]) -> Option<StructLayout> {
        self.struct_layout_in(id, args, LayoutCache::for_copy_rule())
    }

    fn struct_layout_in(&self, id: TypeId, args: &[Type], mut cache: LayoutCache) -> Option<StructLayout> {
        if !self.layout_types.contains(&id) {
            return None;
        }
        let def = self.get(id)?;
        let TypeDef::Struct { .. } = def else { return None };
        for ty in args {
            self.fill_layouts(ty, &mut cache, &mut Vec::new());
        }
        self.fill_decl_fields(def, args, &mut cache, &mut vec![LayoutKey::instance(id, args.to_vec())]);
        Some(compute_struct_layout(def, args, &cache))
    }

    /// The field of struct `ty` that runs past byte `limit` first, in memory
    /// order: its name, its size, and its type. What a `@small` error names.
    pub fn first_field_past(&self, ty: &Type, limit: u32, is_param: IsParam) -> Option<FieldLayout> {
        let (id, args) = match settle(ty, Unsolved::Refuse, is_param)? {
            Type::Named(id) => (id, Vec::new()),
            Type::Generic { base, args } => (base, type_args_of(&args)),
            _ => return None,
        };
        let mut layout = self.struct_layout_for_copy_rule(id, &args)?;
        layout.fields.sort_by_key(|f| f.offset);
        layout.fields.into_iter().find(|f| f.offset + f.size > limit)
    }

    /// Put the layout of every declared type `ty` holds by value into `cache`,
    /// innermost first, so `type_size_align` finds each.
    fn fill_layouts(&self, ty: &Type, cache: &mut LayoutCache, visiting: &mut Vec<LayoutKey>) {
        match ty {
            Type::Named(id) => self.fill_def(*id, &[], cache, visiting),
            Type::Generic { base, args } => {
                let args = type_args_of(args);
                // A declared generic lays out what it holds itself: its fields,
                // with the arguments written in. Laying out every argument first
                // followed ones it only holds behind a pointer, so the order of
                // asking decided sizes: `Holder { kids: Stack<Expr> }` laid
                // `Expr` out while `Holder` was half-done, and `Expr`'s
                // `Boxed(Holder)` payload got a guessed word (#1436's cycle,
                // on demand).
                if !self.layout_types.contains(base) && !self.is_one_word(*base) {
                    for arg in &args {
                        self.fill_layouts(arg, cache, visiting);
                    }
                }
                self.fill_def(*base, &args, cache, visiting);
            }
            // A compiler-provided generic is a handle; its arguments live
            // behind its pointer.
            Type::UnresolvedGeneric { name, args } if !generic_is_one_word(name) => {
                for arg in type_args_of(args) {
                    self.fill_layouts(&arg, cache, visiting);
                }
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

    /// One word whatever its arguments: a runtime handle, or a stdlib generic
    /// standing in for one.
    fn is_one_word(&self, id: TypeId) -> bool {
        self.layout_stand_ins.contains(&id)
            || self.is_handle(id)
    }

    fn fill_def(&self, id: TypeId, args: &[Type], cache: &mut LayoutCache, visiting: &mut Vec<LayoutKey>) {
        let key = LayoutKey::instance(id, args.to_vec());
        if cache.contains(&key) || visiting.contains(&key) {
            return;
        }
        let Some(def) = self.get(id) else { return };
        // Before the stand-in check: `Link` is both an empty stdlib generic and
        // a niche handle, and only its declaration says the second.
        if cache.note_declared_size(id, def) {
            return;
        }
        // A recursive type was already reported; its fields never settle.
        if self.layout_stand_ins.contains(&id) || self.sizeless.contains(&id) {
            let w = cache.word();
            cache.insert(key, (w, w));
            return;
        }
        if !self.layout_types.contains(&id) {
            // A nominal newtype is laid out as what it wraps.
            if let TypeDef::NominalAlias { underlying, .. } = def {
                let params = def.type_params();
                let wrapped = settle(underlying, Unsolved::Word, &|n| params.iter().any(|p| p == n))
                    .expect("a lenient settling always answers");
                visiting.push(key.clone());
                self.fill_layouts(&wrapped, cache, visiting);
                visiting.pop();
                let size_align = type_size_align(&wrapped, cache);
                cache.insert(key, size_align);
            }
            return;
        }
        visiting.push(key.clone());
        self.fill_decl_fields(def, args, cache, visiting);
        visiting.pop();
        let size_align = match def {
            TypeDef::Struct { type_params, .. } if !type_params.is_empty() && args.is_empty() => {
                let l = compute_shared_struct_layout(def, cache);
                (l.size, l.align)
            }
            TypeDef::Struct { .. } => {
                let l = compute_struct_layout(def, args, cache);
                (l.size, l.align)
            }
            TypeDef::Enum { type_params, .. } if !type_params.is_empty() && args.is_empty() => {
                let l = compute_shared_enum_layout(def, cache);
                (l.size, l.align)
            }
            TypeDef::Enum { .. } => {
                let l = compute_enum_layout(def, args, cache);
                (l.size, l.align)
            }
            TypeDef::Union { .. } => {
                let l = compute_union_layout(def, cache);
                (l.size, l.align)
            }
            _ => return,
        };
        cache.insert(key, size_align);
    }

    /// The layouts a declaration's fields need, with `args` written in.
    fn fill_decl_fields(&self, def: &TypeDef, args: &[Type], cache: &mut LayoutCache, visiting: &mut Vec<LayoutKey>) {
        let params = def.type_params();
        let subst: HashMap<&str, &Type> = params.iter().map(String::as_str).zip(args.iter()).collect();
        for field in def.held_types() {
            self.fill_layouts(&substitute_inside(field, &subst), cache, visiting);
        }
    }
}

/// The type arguments of a generic, when each is a type.
fn type_args_of(args: &[GenericArg]) -> Vec<Type> {
    args.iter()
        .filter_map(|a| match a {
            GenericArg::Type(t) => Some((**t).clone()),
            _ => None,
        })
        .collect()
}

// ─── Tests ──────────────────────────────────────────────────────────

