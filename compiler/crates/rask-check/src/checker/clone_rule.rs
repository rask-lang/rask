// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Whether `x.clone()` runs code the program wrote.
//!
//! Clone elision may replace a clone with a move only when nothing observable
//! is lost (comp.clone-elision CE3). A clone the compiler or the stdlib
//! supplies only duplicates storage; a `clone` the program wrote is a function
//! like any other, and dropping the call drops whatever it does.

use std::collections::HashSet;

use super::type_table::{TypeOwner, TypeTable};
use crate::{GenericArg, Type, TypeId};
use rask_types::{MethodOrigin, TypeDef};

impl TypeTable {
    /// Does `.clone()` on a value of this type run a `clone` the program
    /// declared, on the value itself or on anything a derived clone reaches?
    ///
    /// A type the table can't place answers yes: the safe direction, since
    /// the answer only ever stops an optimisation.
    pub fn clone_runs_program_code(&self, ty: &Type) -> bool {
        self.clone_runs_program_code_in(ty, &mut HashSet::new())
    }

    fn clone_runs_program_code_in(&self, ty: &Type, seen: &mut HashSet<TypeId>) -> bool {
        let any = |tys: &mut dyn Iterator<Item = &Type>, seen: &mut HashSet<TypeId>| {
            for t in tys {
                if self.clone_runs_program_code_in(t, seen) {
                    return true;
                }
            }
            false
        };
        match ty {
            Type::Unit | Type::None | Type::Never | Type::Bool | Type::Char | Type::String | Type::Bytes
            | Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128 | Type::Isize
            | Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128 | Type::Usize
            | Type::F32 | Type::F64
            | Type::RawPtr(_) | Type::Fn { .. } | Type::SimdVector { .. } | Type::Error => false,
            Type::Array { elem, .. } => self.clone_runs_program_code_in(elem, seen),
            Type::Tuple(elems) => any(&mut elems.iter(), seen),
            Type::Result { ok, err } => any(&mut [ok.as_ref(), err.as_ref()].into_iter(), seen),
            Type::Union(members) => any(&mut members.iter(), seen),
            Type::Named(id) => self.def_clone_runs_program_code(*id, &[], seen),
            Type::Generic { base, args } => self.def_clone_runs_program_code(*base, args, seen),
            // A stdlib container named but not entered in the table: its clone
            // is the runtime's, and what it holds decides the rest.
            Type::UnresolvedGeneric { args, .. } => {
                any(&mut args.iter().filter_map(type_arg), seen)
            }
            // A type parameter, an unsettled variable, a projection, an
            // interface object: nothing here says which `clone` runs.
            Type::UnresolvedNamed(_) | Type::Var(_) | Type::Assoc { .. }
            | Type::InterfaceObject { .. } => true,
        }
    }

    fn def_clone_runs_program_code(
        &self,
        id: TypeId,
        args: &[GenericArg],
        seen: &mut HashSet<TypeId>,
    ) -> bool {
        // A type reached again through its own fields adds nothing new.
        if !seen.insert(id) {
            return false;
        }
        let Some(def) = self.get(id) else { return true };
        let (methods, type_params, parts): (&[rask_types::MethodSig], &[String], Vec<&Type>) = match def {
            TypeDef::Struct { methods, type_params, fields, .. } => {
                (methods, type_params, fields.iter().map(|(_, t)| t).collect())
            }
            TypeDef::Enum { methods, type_params, variants, .. } => {
                (methods, type_params, variants.iter().flat_map(|(_, f)| f.iter()).collect())
            }
            TypeDef::NominalAlias { methods, type_params, underlying, .. } => {
                (methods, type_params, vec![underlying])
            }
            TypeDef::Union { fields, .. } => (&[], &[], fields.iter().map(|(_, t)| t).collect()),
            TypeDef::Primitive { .. } => return false,
            TypeDef::Interface { .. } => return true,
        };
        let declared = methods
            .iter()
            .any(|m| m.name == "clone" && m.origin == MethodOrigin::Declared);
        if declared && self.declared_by(id) != TypeOwner::Stdlib {
            return true;
        }
        // A derived or structural clone clones each part; a stdlib clone
        // copies storage and clones what the type arguments put in it.
        let subst = Self::generic_field_subst(type_params, args);
        if declared {
            return args
                .iter()
                .filter_map(type_arg)
                .any(|t| self.clone_runs_program_code_in(t, seen));
        }
        parts
            .into_iter()
            .any(|t| self.clone_runs_program_code_in(&Self::substitute_generic_field(t, &subst), seen))
    }
}

fn type_arg(arg: &GenericArg) -> Option<&Type> {
    match arg {
        GenericArg::Type(t) => Some(t),
        GenericArg::ConstUsize(_) => None,
    }
}
