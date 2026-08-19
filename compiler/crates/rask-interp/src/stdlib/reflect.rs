// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! std.reflect — compile-time type introspection (interpreter implementation).

use std::sync::{Arc, Mutex};
use indexmap::IndexMap;

use rask_types::reflect;

use crate::interp::{Interpreter, RuntimeError};
use crate::value::{StructData, Value};

/// The interpreter's answer to "does the program declare this name" — its
/// declaration maps, which is all the shared classifier asks for.
struct InterpDecls<'a>(&'a Interpreter);

impl reflect::ReflectDecls for InterpDecls<'_> {
    fn declares_struct(&self, name: &str) -> bool {
        self.0.struct_decls.contains_key(name)
    }

    fn declares_enum(&self, name: &str) -> bool {
        self.0.enums.contains_key(name)
    }

    fn is_resource(&self, name: &str) -> bool {
        // `File` is the compiler's own resource — it has no declaration to carry
        // the annotation, and the runtime tracks it as one.
        name == "File"
            || self.0.struct_decls.get(name)
                .is_some_and(|d| d.attrs.iter().any(|a| a == "resource"))
    }

    fn member_type_names(&self, name: &str) -> Option<Vec<String>> {
        if let Some(s) = self.0.struct_decls.get(name) {
            return Some(s.fields.iter().map(|f| f.ty.clone()).collect());
        }
        if let Some(e) = self.0.enums.get(name) {
            return Some(
                e.variants.iter()
                    .flat_map(|v| v.fields.iter().map(|f| f.ty.clone()))
                    .collect(),
            );
        }
        // A nominal newtype is whatever it wraps (type.aliases/T11).
        self.0.nominal_targets.get(name).map(|t| vec![t.clone()])
    }

    fn type_params(&self, name: &str) -> Vec<String> {
        let params = self.0.struct_decls.get(name).map(|s| &s.type_params)
            .or_else(|| self.0.enums.get(name).map(|e| &e.type_params));
        params.map(|p| p.iter().map(|t| t.name.clone()).collect()).unwrap_or_default()
    }
}

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

        if method == "fields" {
            return self.reflect_fields(&type_name);
        }

        // The rules are in rask-types so native folds the same answers — each
        // backend deriving its own is how `is_integer<i32>()` came back `false`
        // here while native couldn't lower the call at all (#775).
        let decls = InterpDecls(self);
        match reflect::answer(method, &type_name, &decls) {
            reflect::ReflectAnswer::Bool(b) => Ok(Value::Bool(b)),
            reflect::ReflectAnswer::Int(n) => Ok(Value::int(n as i64)),
            reflect::ReflectAnswer::Str(s) => Ok(Value::String(Arc::new(Mutex::new(s)))),
            reflect::ReflectAnswer::Unsupported(why) => Err(RuntimeError::TypeError(format!(
                "reflect.{method}<{type_name}>() isn't implemented on either backend — {why} (#791)"
            ))),
            reflect::ReflectAnswer::NoSuchMethod => Err(RuntimeError::NoSuchMethod {
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
                // E19: @no_serialize excludes a field from serialization, in
                // both directions.
                fields.insert(
                    "is_skipped".to_string(),
                    Value::Bool(has_attr(&f.attrs, "no_serialize")),
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

        Ok(Value::vec(field_infos))
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
