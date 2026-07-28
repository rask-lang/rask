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
            "size_of" | "align_of" => Ok(Value::int(0)), // Placeholder
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
                fields.insert("offset".to_string(), Value::int(0));
                fields.insert("size".to_string(), Value::int(0));
                fields.insert(
                    "is_public".to_string(),
                    Value::Bool(f.visibility.is_pub()),
                );
                // E18: @rename("...") overrides the serialized key name.
                let serial_name = rename_of(&f.attrs).unwrap_or_else(|| f.name.clone());
                fields.insert(
                    "serial_name".to_string(),
                    Value::String(Arc::new(Mutex::new(serial_name))),
                );
                // E19: @skip excludes a field from serialization.
                fields.insert(
                    "is_skipped".to_string(),
                    Value::Bool(has_attr(&f.attrs, "skip")),
                );
                // E20/FD6: a declared default (`x: T = v`) or a decode-only
                // @default(expr) makes the field optional during decode.
                fields.insert(
                    "has_default".to_string(),
                    Value::Bool(f.default.is_some() || has_attr(&f.attrs, "default")),
                );
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

/// True if an attribute with the given base name is present.
/// Matches both bare (`skip`) and call-form (`default(0)`) attributes.
fn has_attr(attrs: &[String], name: &str) -> bool {
    attrs.iter().any(|a| a == name || a.starts_with(&format!("{name}(")))
}

/// Extract the string argument of `@rename("...")`, if present.
fn rename_of(attrs: &[String]) -> Option<String> {
    let raw = attrs.iter().find(|a| a.starts_with("rename("))?;
    // Stored as `rename("user_name")` — pull out the quoted contents.
    let inner = raw.strip_prefix("rename(")?.strip_suffix(')')?;
    let inner = inner.trim();
    inner.strip_prefix('"')?.strip_suffix('"').map(|s| s.to_string())
}
