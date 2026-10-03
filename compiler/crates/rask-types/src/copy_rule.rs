// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Which values are copied rather than moved: one rule, read by the ownership
//! pass for moves and by the checker for a `T: Copy` bound.

use crate::{Type, TypeTable};

impl TypeTable {
    /// Compiler-native generic containers whose layout lives in the runtime
    /// rather than in a visible struct decl (an empty `struct Vec<T> { }`
    /// stub) — field-based size/Copy inference can't see them, so they're
    /// named explicitly instead.
    pub fn is_native_opaque_generic(base_name: &str) -> bool {
        matches!(base_name,
            "Vec" | "Map" | "Wide" | "Cell"
            | "Rack" | "Link"
            | "Handle" | "Sender" | "Receiver")
    }

    /// Map a generic struct/enum's own type parameter names to the concrete
    /// types plugged in at this instantiation (`Wrapping<u32>`'s `T` -> `u32`).
    /// Const-generic args have nothing to bind to a type parameter and are
    /// skipped.
    pub fn generic_field_subst(type_params: &[String], args: &[crate::GenericArg]) -> std::collections::HashMap<String, Type> {
        type_params.iter().zip(args.iter()).filter_map(|(name, arg)| match arg {
            crate::GenericArg::Type(t) => Some((name.clone(), (**t).clone())),
            crate::GenericArg::ConstUsize(_) => None,
        }).collect()
    }

    /// Replace a struct/enum field's type parameter with the concrete type
    /// from `subst`, through the compound shapes a field can have.
    pub fn substitute_generic_field(ty: &Type, subst: &std::collections::HashMap<String, Type>) -> Type {
        match ty {
            Type::UnresolvedNamed(name) => subst.get(name).cloned().unwrap_or_else(|| ty.clone()),
            Type::Array { elem, len } => Type::Array {
                elem: Box::new(Self::substitute_generic_field(elem, subst)),
                len: *len,
            },
            Type::Tuple(elems) => Type::Tuple(elems.iter().map(|e| Self::substitute_generic_field(e, subst)).collect()),
            ty if ty.is_option() => Type::option(Self::substitute_generic_field(ty.as_option().unwrap(), subst)),
            Type::Generic { base, args } => Type::Generic {
                base: *base,
                args: args.iter().map(|a| match a {
                    crate::GenericArg::Type(t) => crate::GenericArg::Type(Box::new(Self::substitute_generic_field(t, subst))),
                    other => other.clone(),
                }).collect(),
            },
            _ => ty.clone(),
        }
    }

    /// Is a value of this type copied rather than moved (mem.value/VS1)?
    pub fn is_copy(&self, ty: &Type) -> bool {
        self.is_copy_with(ty, &|_| false)
    }

    /// `is_copy`, with a say for type parameters: inside a generic body the
    /// caller knows which of its names are bounded by `Copy`.
    pub fn is_copy_with(&self, ty: &Type, param_is_copy: &dyn Fn(&str) -> bool) -> bool {
        // L1: a linear value is never Copy, whatever its size or its fields.
        // `@resource struct Conn { id: i64 }` is eight bytes of Copy field, so
        // this said Copy — and `consume_arg` skips a Copy argument, so passing a
        // connection to a `take` parameter consumed nothing and the caller was
        // then told it had leaked the value it had just handed away.
        if self.is_linear_value(ty) {
            return false;
        }
        match ty {
            // Primitives are always Copy
            Type::Unit | Type::None | Type::Bool | Type::Char => true,
            Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128 => true,
            Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128 => true,
            Type::F32 | Type::F64 => true,
            Type::Never => true,

            // String is Copy (immutable, refcounted, 16 bytes — std.strings/S1)
            Type::String => true,

            // Arrays: Copy if element is Copy and size <= 16 bytes
            Type::Array { elem, len: _ } => {
                self.is_copy_with(elem, param_is_copy) && self.value_size(ty) <= 16
            }

            // Tuples: Copy if all elements are Copy and size <= 16 bytes
            Type::Tuple(elems) => {
                elems.iter().all(|t| self.is_copy_with(t, param_is_copy)) && self.value_size(ty) <= 16
            }

            // Option (T or none): Copy if inner is Copy and size <= 16 bytes
            ty if ty.is_option() => {
                let inner = ty.as_option().unwrap();
                self.is_copy_with(inner, param_is_copy) && self.value_size(ty) <= 16
            }

            // Result: NOT Copy (usually contains error info)
            Type::Result { .. } => false,

            // Union: NOT Copy (error union types)
            Type::Union(_) => false,

            // User-defined types: need to check size and fields
            Type::Named(type_id) => {
                if let Some(def) = self.get(*type_id) {
                    match def {
                        crate::TypeDef::Struct { fields, is_unique, .. } => {
                            // U1: @unique disables implicit copy regardless of size
                            if *is_unique { return false; }
                            fields.iter().all(|(_, t)| self.is_copy_with(t, param_is_copy))
                                && self.value_size(ty) <= 16
                        }
                        crate::TypeDef::Enum { variants, .. } => {
                            variants.iter().all(|(_, data)| data.iter().all(|t| self.is_copy_with(t, param_is_copy)))
                                && self.value_size(ty) <= 16
                        }
                        // A primitive is always Copy; it never reaches here as
                        // a `Named` anyway.
                        crate::TypeDef::Primitive { .. } => true,
                        crate::TypeDef::Interface { .. } => false,
                        crate::TypeDef::Union { fields, .. } => {
                            fields.iter().all(|(_, t)| self.is_copy_with(t, param_is_copy))
                                && self.value_size(ty) <= 16
                        }
                        crate::TypeDef::NominalAlias { underlying, .. } => {
                            self.is_copy_with(underlying, param_is_copy)
                        }
                    }
                } else {
                    false
                }
            }

            // A `Link` is Copy: a machine word naming a node,
            // whose whole point is to be duplicated freely (mem.racks). For
            // `Link<T>` the rack spec says so from the other side — RK5 has
            // using one after its node is deleted reported "as a use after free
            // rather than as a move", which only reads as a rule if links copy.
            // Without it, `v.push(link)` consumed the name and a later
            // `rack.delete(link)` drew a bogus use-after-move.
            //
            // The other compiler-native generics (Vec, Map, Rack, ...) have no
            // fields visible to the type system — their layout lives in the
            // runtime, not in a struct decl — so field-based inference can't
            // see them and they stay hardcoded move-only.
            //
            // A user-defined generic struct (`struct Wrapping<T> { value: T }`)
            // *does* have real fields, so its Copy-ness depends on what T ends
            // up being at this instantiation — same rule as a non-generic
            // struct, just substituted first (W4).
            Type::Generic { base, args } => {
                let base_name = self.type_name(*base);
                if Self::is_native_opaque_generic(&base_name) {
                    base_name.as_str() == "Link"
                } else if let Some(def) = self.get(*base) {
                    match def {
                        crate::TypeDef::Struct { type_params, fields, is_unique, .. } => {
                            if *is_unique { return false; }
                            let subst = Self::generic_field_subst(type_params, args);
                            fields.iter().all(|(_, t)| self.is_copy_with(&Self::substitute_generic_field(t, &subst), param_is_copy))
                                && self.value_size(ty) <= 16
                        }
                        crate::TypeDef::Enum { type_params, variants, .. } => {
                            let subst = Self::generic_field_subst(type_params, args);
                            variants.iter().all(|(_, data)| data.iter().all(|t| self.is_copy_with(&Self::substitute_generic_field(t, &subst), param_is_copy)))
                                && self.value_size(ty) <= 16
                        }
                        // A primitive is always Copy; it never reaches here as
                        // a `Named` anyway.
                        crate::TypeDef::Primitive { .. } => true,
                        crate::TypeDef::Interface { .. } => false,
                        // Unions aren't generic (no type_params to substitute) —
                        // reaching this arm through a `Type::Generic` would mean
                        // a union name got parsed with type arguments, which
                        // shouldn't happen.
                        crate::TypeDef::Union { .. } => false,
                        crate::TypeDef::NominalAlias { underlying, .. } => {
                            self.is_copy_with(underlying, param_is_copy)
                        }
                    }
                } else {
                    false
                }
            }

            // Function types are Copy (just a pointer)
            Type::Fn { .. } => true,

            // Type variables: conservative
            Type::Var(_) => false,

            // AT6: a projection is read off a conformance during type
            // checking, so one reaching here never resolved. Conservative,
            // same as a type variable.
            Type::Assoc { .. } => false,

            // Raw pointers are always Copy (just an address)
            Type::RawPtr(_) => true,

            // SIMD vectors: NOT Copy (large, stack-allocated)
            Type::SimdVector { .. } => false,

            // Unresolved types: conservative, except `Link`,
            // which are Copy regardless of how the name was spelled — same
            // three as the resolved `Type::Generic` arm above.
            Type::UnresolvedGeneric { name, .. } => {
                name.as_str() == "Link"
            }
            // A type parameter is Copy where its bound says so (`T: Copy`);
            // the caller knows which names those are.
            Type::UnresolvedNamed(name) => param_is_copy(name),

            // Interface objects: never Copy (TR11 — owns heap data)
            Type::InterfaceObject { .. } => false,

            // Error: don't report more errors
            Type::Error => true,
        }
    }

    /// Estimate type size in bytes (simplified).
    pub fn value_size(&self, ty: &Type) -> usize {
        match ty {
            Type::Unit | Type::None => 0,
            Type::Bool | Type::I8 | Type::U8 => 1,
            Type::I16 | Type::U16 => 2,
            Type::I32 | Type::U32 | Type::F32 | Type::Char => 4,
            Type::I64 | Type::U64 | Type::F64 => 8,
            // Two words, and the only scalar that is. Falling through to the
            // 8-byte default made `struct Wide { a: i128, b: i64 }` measure 16
            // instead of 24, so it sat on the Copy threshold instead of over it
            // and two bindings aliased one value with nothing said (#936).
            Type::I128 | Type::U128 => 16,
            Type::Tuple(elems) => elems.iter().map(|t| self.value_size(t)).sum(),
            Type::Array { elem, len } => self.value_size(elem) * len,
            ty if ty.is_option() => self.value_size(ty.as_option().unwrap()) + 1, // tag byte
            Type::Named(type_id) => {
                if let Some(def) = self.get(*type_id) {
                    match def {
                        crate::TypeDef::Struct { fields, .. } => {
                            fields.iter().map(|(_, t)| self.value_size(t)).sum()
                        }
                        crate::TypeDef::Enum { variants, .. } => {
                            let max_variant = variants
                                .iter()
                                .map(|(_, data)| data.iter().map(|t| self.value_size(t)).sum::<usize>())
                                .max()
                                .unwrap_or(0);
                            max_variant + 1
                        }
                        _ => 8,
                    }
                } else {
                    8
                }
            }
            // A user-defined generic struct/enum is sized the same way as a
            // non-generic one, once its own type parameter is substituted
            // with the type argument at this instantiation (`Wrapping<u8>` is
            // one byte, not whatever the unsubstituted `T` would default to).
            // The compiler-native generics (Vec, Map, ...) declare an empty
            // field list — their real layout lives in the runtime, not in the
            // struct decl — so summing fields would say 0 instead of their
            // actual size. Keep them at the old flat 8-byte guess rather than
            // let an empty sum silently answer 0.
            Type::Generic { base, args } if !Self::is_native_opaque_generic(&self.type_name(*base)) => {
                if let Some(def) = self.get(*base) {
                    match def {
                        crate::TypeDef::Struct { type_params, fields, .. } => {
                            let subst = Self::generic_field_subst(type_params, args);
                            fields.iter().map(|(_, t)| self.value_size(&Self::substitute_generic_field(t, &subst))).sum()
                        }
                        crate::TypeDef::Enum { type_params, variants, .. } => {
                            let subst = Self::generic_field_subst(type_params, args);
                            let max_variant = variants
                                .iter()
                                .map(|(_, data)| data.iter().map(|t| self.value_size(&Self::substitute_generic_field(t, &subst))).sum::<usize>())
                                .max()
                                .unwrap_or(0);
                            max_variant + 1
                        }
                        _ => 8,
                    }
                } else {
                    8
                }
            }
            // Strings, closures and interface objects: fat pointer
            Type::String | Type::Fn { .. } | Type::InterfaceObject { .. } => 16,
            _ => 8,
        }
    }

}
