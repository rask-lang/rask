// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! std.reflect — compile-time type introspection (interpreter implementation).

use std::sync::{Arc, Mutex};
use indexmap::IndexMap;

use rask_ast::ty::TypeExpr;
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

    fn member_types(&self, name: &str) -> Option<Vec<TypeExpr>> {
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
    /// `reflect.<method><T>()`, with `T` already resolved in the running body.
    pub(crate) fn call_reflect_method(
        &self,
        method: &str,
        ty: &TypeExpr,
    ) -> Result<Value, RuntimeError> {
        if method == "fields" {
            return self.reflect_fields(ty);
        }

        // The rules are in rask-types so native folds the same answers — each
        // backend deriving its own is how `is_integer<i32>()` came back `false`
        // here while native couldn't lower the call at all (#775).
        let decls = InterpDecls(self);
        match reflect::answer(method, ty, &decls) {
            reflect::ReflectAnswer::Bool(b) => Ok(Value::Bool(b)),
            reflect::ReflectAnswer::Int(n) => Ok(Value::int(n as i64)),
            reflect::ReflectAnswer::Str(s) => Ok(Value::String(Arc::new(Mutex::new(s)))),
            reflect::ReflectAnswer::Unsupported(why) => Err(RuntimeError::TypeError(format!(
                "reflect.{method}<{}>() isn't implemented on either backend — {why} (#791)", ty.source()
            ))),
            reflect::ReflectAnswer::NoSuchMethod => Err(RuntimeError::NoSuchMethod {
                ty: "reflect".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// The layout of the struct `type_name` names, with its type arguments
    /// substituted.
    ///
    /// A generic instantiation gets its own: `Pair<string>` puts 16 bytes where
    /// `Pair<i64>` puts 8, and reading the shared layout — where every
    /// parameter stands in as a word — would report the field after it at the
    /// wrong offset. Same rule native follows, for the same reason (#781, #968).
    fn struct_layout_of(
        &self,
        decl: &rask_ast::decl::StructDecl,
        ty: &TypeExpr,
        params: &[String],
    ) -> Option<rask_mono::StructLayout> {
        let args = ty.args();
        if args.len() != params.len() {
            return None;
        }
        let type_args: Vec<rask_types::Type> = args.iter().map(rask_mono::field_type).collect();
        // `compute_struct_layout` wants the declaration in its `Decl` wrapper,
        // which is why the list is kept: the span decides whether the type
        // counts as stdlib, and a synthesized one would answer that wrong.
        let owner = self.type_decls.iter().find(|d| {
            matches!(&d.kind, rask_ast::decl::DeclKind::Struct(s) if s.name == decl.name)
        })?;
        Some(rask_mono::compute_struct_layout(owner, &type_args, &self.layout_cache))
    }

    /// reflect.fields<T>() → []FieldInfo
    ///
    /// A generic instantiation is written `Ring<i64>` and declared as `Ring`, so
    /// the declaration is found under the base name and the type arguments are
    /// substituted into the field types — otherwise every generic struct
    /// answered "not a struct type" (#968).
    fn reflect_fields(&self, ty: &TypeExpr) -> Result<Value, RuntimeError> {
        let decl = ty
            .name()
            .and_then(|n| self.struct_decls.get(&n))
            .ok_or_else(|| {
                RuntimeError::TypeError(format!(
                    "reflect.fields<{}>(): not a struct type",
                    ty.source()
                ))
            })?;
        let params: Vec<String> = decl.type_params.iter().map(|p| p.name.clone()).collect();
        let args = ty.args();
        let substitute = |field_ty: &TypeExpr| {
            field_ty.substitute(&|name| {
                params.iter().position(|p| p == name).and_then(|i| args.get(i)).cloned()
            })
        };
        let layout = self.struct_layout_of(decl, ty, &params);

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
                    Value::String(Arc::new(Mutex::new(substitute(&f.ty).to_string()))),
                );
                // Both were 0 here while native reported the truth (#1104).
                // The numbers come from the layout pass mono already runs, so
                // the two backends can't drift into two answers.
                let (offset, size) = layout
                    .as_ref()
                    .and_then(|l| l.fields.iter().find(|fl| fl.name == f.name))
                    .map(|fl| (fl.offset as i64, fl.size as i64))
                    .unwrap_or((0, 0));
                fields.insert("offset".to_string(), Value::int(offset));
                fields.insert("size".to_string(), Value::int(size));
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
                // AN6: raw attachments, hidden behind `__` — what
                // `field.has<A>()` answers from. Mirrors the native
                // ReflectFieldConst.attrs.
                fields.insert(
                    "__attrs".to_string(),
                    Value::vec(
                        f.attrs
                            .iter()
                            .map(|a| Value::String(Arc::new(Mutex::new(a.clone()))))
                            .collect(),
                    ),
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
