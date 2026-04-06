// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! std.reflect — compile-time type introspection (interpreter implementation).

use std::sync::{Arc, Mutex};
use indexmap::IndexMap;

use crate::interp::{Interpreter, RuntimeError};
use crate::value::{StructData, Value};

impl Interpreter {
    pub(crate) fn call_reflect_method(
        &self,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        // All reflect methods take a type name as first arg (injected from type_args)
        let type_name = match args.first() {
            Some(Value::String(s)) => s.lock().unwrap().clone(),
            _ => {
                return Err(RuntimeError::TypeError(
                    "reflect methods require a type argument: reflect.fields<T>()".into(),
                ));
            }
        };

        match method {
            "fields" => self.reflect_fields(&type_name),
            "name_of" => Ok(Value::String(Arc::new(Mutex::new(type_name)))),
            "is_struct" => Ok(Value::Bool(self.struct_decls.contains_key(&type_name))),
            "is_enum" => Ok(Value::Bool(self.enums.contains_key(&type_name))),
            "size_of" | "align_of" => Ok(Value::Int(0)), // Placeholder
            "is_copy" | "is_resource" | "is_flat" => Ok(Value::Bool(false)), // Placeholder
            "is_optional" | "is_vec" | "is_map" | "is_integer" | "is_float" => Ok(Value::Bool(false)),
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "reflect".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// reflect.fields<T>() → []FieldInfo
    fn reflect_fields(&self, type_name: &str) -> Result<Value, RuntimeError> {
        let decl = self.struct_decls.get(type_name).ok_or_else(|| {
            RuntimeError::TypeError(format!(
                "reflect.fields<{}>(): not a struct type",
                type_name
            ))
        })?;

        let field_infos: Vec<Value> = decl
            .fields
            .iter()
            .map(|f| {
                let mut fields = IndexMap::new();
                fields.insert(
                    "name".to_string(),
                    Value::String(Arc::new(Mutex::new(f.name.clone()))),
                );
                fields.insert(
                    "type_name".to_string(),
                    Value::String(Arc::new(Mutex::new(f.ty.clone()))),
                );
                fields.insert("offset".to_string(), Value::Int(0));
                fields.insert("size".to_string(), Value::Int(0));
                fields.insert(
                    "is_public".to_string(),
                    Value::Bool(f.visibility.is_pub()),
                );
                fields.insert(
                    "serial_name".to_string(),
                    Value::String(Arc::new(Mutex::new(f.name.clone()))),
                );
                fields.insert("is_skipped".to_string(), Value::Bool(false));
                fields.insert("has_default".to_string(), Value::Bool(false));
                Value::Struct(Arc::new(Mutex::new(StructData {
                    name: "FieldInfo".to_string(),
                    fields,
                    resource_id: None,
                })))
            })
            .collect();

        Ok(Value::Vec(Arc::new(Mutex::new(field_infos))))
    }
}
