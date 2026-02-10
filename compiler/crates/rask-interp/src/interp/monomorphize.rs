// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Generic struct monomorphization.

use std::collections::HashMap;

use rask_ast::decl::{Field, StructDecl};
use rask_types::GenericArg;

use super::{Interpreter, RuntimeError};

impl Interpreter {
    pub(super) fn monomorphize_struct_from_name(&mut self, full_name: &str) -> Result<String, RuntimeError> {
        // Extract base name and args from "Buffer<i32, 256>"
        let lt_pos = full_name.find('<').ok_or_else(|| {
            RuntimeError::Generic(format!("Expected generic arguments in '{}'", full_name))
        })?;

        let base_name = &full_name[..lt_pos];
        let args_str = &full_name[lt_pos + 1..full_name.len() - 1]; // Remove < and >

        // Parse generic arguments
        let args = self.parse_generic_args(args_str)?;

        // Monomorphize
        self.monomorphize_struct(base_name, &args)
    }

    /// Parse generic arguments from a string like "i32, 256".
    fn parse_generic_args(&self, args_str: &str) -> Result<Vec<GenericArg>, RuntimeError> {
        let mut args = Vec::new();
        let parts: Vec<&str> = args_str.split(',').map(|s| s.trim()).collect();

        for part in parts {
            // Try to parse as usize (const generic)
            if let Ok(n) = part.parse::<usize>() {
                args.push(GenericArg::ConstUsize(n));
            } else {
                // It's a type - for now just store as Type with a placeholder
                // In a real implementation, we'd parse the type properly
                use rask_types::Type;
                let ty = match part {
                    "i32" => Type::I32,
                    "i64" => Type::I64,
                    "f32" => Type::F32,
                    "f64" => Type::F64,
                    "bool" => Type::Bool,
                    "string" => Type::String,
                    "usize" => Type::U64, // Map usize to u64 for now
                    _ => Type::UnresolvedNamed(part.to_string()),
                };
                args.push(GenericArg::Type(Box::new(ty)));
            }
        }

        Ok(args)
    }

    /// Monomorphize a generic struct by substituting type and const parameters.
    /// Returns the monomorphized struct name (e.g., "Buffer<i32, 256>").
    fn monomorphize_struct(&mut self, base_name: &str, args: &[GenericArg]) -> Result<String, RuntimeError> {
        // Build the full instantiated name
        let mut full_name = base_name.to_string();
        full_name.push('<');
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                full_name.push_str(", ");
            }
            match arg {
                GenericArg::Type(ty) => full_name.push_str(&ty.to_string()),
                GenericArg::ConstUsize(n) => full_name.push_str(&n.to_string()),
            }
        }
        full_name.push('>');

        // Check if already monomorphized
        if self.monomorphized_structs.contains_key(&full_name) {
            return Ok(full_name);
        }

        // Get the generic struct declaration
        // Look for a struct whose name is either exactly base_name or starts with "base_name<"
        let base_decl = self.struct_decls.get(base_name)
            .or_else(|| {
                // Search for generic version: "Pair<T, U>"
                self.struct_decls.iter()
                    .find(|(k, _)| k.starts_with(base_name) && k.contains('<'))
                    .map(|(_, v)| v)
            })
            .ok_or_else(|| RuntimeError::UndefinedVariable(base_name.to_string()))?
            .clone();

        // Verify argument count matches parameter count
        if base_decl.type_params.len() != args.len() {
            return Err(RuntimeError::Generic(format!(
                "Wrong number of generic arguments for {}: expected {}, got {}",
                base_name, base_decl.type_params.len(), args.len()
            )));
        }

        // Build substitution map: param name -> GenericArg
        let mut subst_map = HashMap::new();
        for (param, arg) in base_decl.type_params.iter().zip(args.iter()) {
            subst_map.insert(param.name.clone(), arg.clone());
        }

        // Substitute in field types
        let mut new_fields = Vec::new();
        for field in &base_decl.fields {
            let new_ty = self.substitute_type(&field.ty, &subst_map)?;
            new_fields.push(Field {
                name: field.name.clone(),
                ty: new_ty,
                is_pub: field.is_pub,
            });
        }

        // Create monomorphized struct declaration
        let mono_decl = StructDecl {
            name: full_name.clone(),
            type_params: vec![], // Monomorphized structs have no params
            fields: new_fields,
            methods: base_decl.methods.clone(),
            is_pub: base_decl.is_pub,
            attrs: base_decl.attrs.clone(),
        };

        // Cache it
        self.monomorphized_structs.insert(full_name.clone(), mono_decl.clone());
        self.struct_decls.insert(full_name.clone(), mono_decl);

        Ok(full_name)
    }

    /// Substitute type parameters in a type string.
    fn substitute_type(&self, ty: &str, subst_map: &HashMap<String, GenericArg>) -> Result<String, RuntimeError> {
        // Handle array types [T; N]
        if ty.starts_with('[') && ty.ends_with(']') {
            if let Some(semi_pos) = ty.find(';') {
                let elem_ty = ty[1..semi_pos].trim();
                let size_expr = ty[semi_pos + 1..ty.len() - 1].trim();

                let new_elem = self.substitute_type(elem_ty, subst_map)?;
                let new_size = if let Some(arg) = subst_map.get(size_expr) {
                    match arg {
                        GenericArg::ConstUsize(n) => n.to_string(),
                        GenericArg::Type(_) => {
                            return Err(RuntimeError::Generic(format!(
                                "Expected const value for array size, got type"
                            )));
                        }
                    }
                } else {
                    size_expr.to_string()
                };

                return Ok(format!("[{}; {}]", new_elem, new_size));
            }
        }

        // Simple type parameter substitution
        if let Some(arg) = subst_map.get(ty) {
            match arg {
                GenericArg::Type(t) => Ok(t.to_string()),
                GenericArg::ConstUsize(n) => Ok(n.to_string()),
            }
        } else {
            Ok(ty.to_string())
        }
    }
}

