// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Generic struct monomorphization.

use std::collections::HashMap;

use rask_ast::decl::{Field, StructDecl};
use rask_ast::ty::TypeExpr;

use super::{Interpreter, RuntimeError};

impl Interpreter {
    /// Instantiate a generic struct with written arguments. Returns the
    /// instantiated declaration's name (`Buffer<i32, 256>`).
    pub(super) fn monomorphize_struct(&mut self, base_name: &str, args: &[TypeExpr]) -> Result<String, RuntimeError> {
        let full_name = TypeExpr::generic(base_name, args.to_vec()).to_string();
        if self.monomorphized_structs.contains_key(&full_name) {
            return Ok(full_name);
        }

        let base_decl = self.struct_decls.get(base_name)
            .ok_or_else(|| RuntimeError::UndefinedVariable(base_name.to_string()))?
            .clone();

        if base_decl.type_params.len() != args.len() {
            return Err(RuntimeError::Generic(format!(
                "Wrong number of generic arguments for {}: expected {}, got {}",
                base_name, base_decl.type_params.len(), args.len()
            )));
        }

        let subst_map: HashMap<&str, &TypeExpr> = base_decl
            .type_params
            .iter()
            .map(|p| p.name.as_str())
            .zip(args.iter())
            .collect();

        let new_fields = base_decl
            .fields
            .iter()
            .map(|field| Field {
                ty: field.ty.substitute(&|name| subst_map.get(name).map(|t| (*t).clone())),
                ..field.clone()
            })
            .collect();

        let mono_decl = StructDecl {
            name: full_name.clone(),
            type_params: vec![],
            fields: new_fields,
            methods: base_decl.methods.clone(),
            is_pub: base_decl.is_pub,
            attrs: base_decl.attrs.clone(),
            doc: None,
        };

        self.monomorphized_structs.insert(full_name.clone(), mono_decl.clone());
        self.struct_decls.insert(full_name.clone(), mono_decl);

        Ok(full_name)
    }
}
