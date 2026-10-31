// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! std.reflect — compile-time type introspection (interpreter implementation).

use std::sync::{Arc, Mutex};
use indexmap::IndexMap;

use rask_ast::ty::TypeExpr;
use rask_check::reflect;

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
        self.0.struct_decls.get(name)
            .is_some_and(|d| d.attrs.iter().any(|a| a.is("resource")))
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

    fn type_table(&self) -> &rask_check::TypeTable {
        &self.0.types
    }

    fn type_params(&self, name: &str) -> Vec<String> {
        let params = self.0.struct_decls.get(name).map(|s| &s.type_params)
            .or_else(|| self.0.enums.get(name).map(|e| &e.type_params));
        params.map(|p| p.iter().map(|t| t.name.clone()).collect()).unwrap_or_default()
    }
}

impl Interpreter {
    /// `reflect.<method><T>()`, with `T` already resolved in the running body.
    ///
    /// `rest` is any further type argument (`implements<T, I>`'s `I`), resolved
    /// the same way; `args` the call's ordinary arguments.
    pub(crate) fn call_reflect_method(
        &self,
        method: &str,
        ty: &TypeExpr,
        rest: &[TypeExpr],
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        // The type-table questions are answered in rask-types, from the same
        // table native reads.
        let refused = |why: String| RuntimeError::TypeError(why);
        match method {
            "fields" => return self.reflect_fields(ty),
            "variants" => return self.reflect_variants(ty),
            "methods" => {
                let facts = reflect::methods(&self.types, ty).map_err(refused)?;
                return Ok(Value::vec(facts.into_iter().map(method_info).collect()));
            }
            "has_method" => {
                let name = string_arg(&args, method)?;
                let facts = reflect::methods(&self.types, ty).map_err(refused)?;
                return Ok(Value::Bool(facts.iter().any(|m| m.name == name)));
            }
            "has_field" => {
                let name = string_arg(&args, method)?;
                return Ok(Value::Bool(reflect::field_names(&self.types, ty).contains(&name)));
            }
            "interfaces" => {
                let names = reflect::interfaces(&self.types, ty).map_err(refused)?;
                return Ok(Value::vec(names.into_iter().map(string_value).collect()));
            }
            "implements" => {
                let Some(iface) = rest.first() else {
                    return Err(refused(
                        "reflect.implements needs the interface too: reflect.implements<T, I>()".into(),
                    ));
                };
                return reflect::implements(&self.types, ty, iface).map(Value::Bool).map_err(refused);
            }
            _ => {}
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
                "reflect.{method}<{}>() can't be answered: {why}", ty.source()
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
    ) -> Option<rask_layout::StructLayout> {
        let args = ty.args();
        if args.len() != params.len() {
            return None;
        }
        let type_args: Vec<rask_types::Type> = args.iter().map(rask_layout::field_type).collect();
        // `compute_struct_layout` wants the declaration in its `Decl` wrapper,
        // which is why the list is kept: the span decides whether the type
        // counts as stdlib, and a synthesized one would answer that wrong.
        let owner = self.type_decls.iter().find(|d| {
            matches!(&d.kind, rask_ast::decl::DeclKind::Struct(s) if s.name == decl.name)
        })?;
        Some(rask_layout::compute_struct_layout(owner, &type_args, &self.layout_cache))
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
            .enumerate()
            .map(|(index, f)| {
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
                // AN6: which declaration and field this describes, hidden
                // behind `__` — `field.has<A>()` reads the attachments there.
                fields.insert(
                    "__owner".to_string(),
                    Value::String(Arc::new(Mutex::new(decl.name.clone()))),
                );
                fields.insert("__index".to_string(), Value::int(index as i64));
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

impl Interpreter {
    /// reflect.variants<T>() → []VariantInfo, from the enum's layout — the one
    /// mono computes, with this instantiation's arguments, so the offsets are
    /// the ones native reports.
    fn reflect_variants(&self, ty: &TypeExpr) -> Result<Value, RuntimeError> {
        let not_enum = || {
            RuntimeError::TypeError(format!("reflect.variants<{}>(): not an enum type", ty.source()))
        };
        // `enums` rather than `type_decls`: it holds the stdlib's enums too,
        // `Ordering` among them.
        let base = ty.name().ok_or_else(not_enum)?;
        let decl = self.enums.get(&base).ok_or_else(not_enum)?;
        let decl = rask_ast::decl::Decl {
            id: rask_ast::NodeId(0),
            kind: rask_ast::decl::DeclKind::Enum(decl.clone()),
            span: rask_ast::Span::new(0, 0),
        };
        let type_args: Vec<rask_types::Type> = ty.args().iter().map(rask_layout::field_type).collect();
        let layout = rask_layout::compute_enum_layout(&decl, &type_args, &self.layout_cache);
        let facts = rask_mono::reflect::variant_facts(&layout, &self.types, &[]);
        Ok(Value::vec(facts.into_iter().map(variant_info).collect()))
    }
}

fn string_value(s: String) -> Value {
    Value::String(Arc::new(Mutex::new(s)))
}

fn info_struct(name: &str, fields: IndexMap<String, Value>) -> Value {
    Value::Struct(Arc::new(Mutex::new(StructData {
        name: name.to_string(),
        fields,
        resource_id: None,
    })))
}

/// The name `has_field`/`has_method` ask about.
fn string_arg(args: &[Value], method: &str) -> Result<String, RuntimeError> {
    match args.first() {
        Some(Value::String(s)) => Ok(s.lock().unwrap().clone()),
        _ => Err(RuntimeError::TypeError(format!(
            "reflect.{method}<T>(name) needs the name it's asking about"
        ))),
    }
}

fn method_info(m: reflect::MethodFact) -> Value {
    let mut f = IndexMap::new();
    f.insert("name".to_string(), string_value(m.name));
    f.insert("is_public".to_string(), Value::Bool(m.is_public));
    f.insert("param_count".to_string(), Value::int(m.param_count as i64));
    f.insert("return_type_name".to_string(), string_value(m.return_type_name));
    info_struct("MethodInfo", f)
}

fn variant_info(v: rask_mono::reflect::VariantFact) -> Value {
    let fields: Vec<Value> = v
        .fields
        .into_iter()
        .map(|p| {
            let mut f = IndexMap::new();
            f.insert("name".to_string(), string_value(p.name.clone()));
            f.insert("type_name".to_string(), string_value(p.type_name));
            f.insert("offset".to_string(), Value::int(p.offset as i64));
            f.insert("size".to_string(), Value::int(p.size as i64));
            // A payload has no visibility or wire annotations of its own.
            f.insert("is_public".to_string(), Value::Bool(true));
            f.insert("serial_name".to_string(), string_value(p.name));
            f.insert("is_skipped".to_string(), Value::Bool(false));
            f.insert("has_default".to_string(), Value::Bool(p.has_default));
            info_struct("FieldInfo", f)
        })
        .collect();
    let mut f = IndexMap::new();
    f.insert("name".to_string(), string_value(v.name));
    f.insert("has_fields".to_string(), Value::Bool(!fields.is_empty()));
    f.insert("field_count".to_string(), Value::int(fields.len() as i64));
    f.insert("fields".to_string(), Value::vec(fields));
    f.insert("serial_name".to_string(), string_value(v.serial_name));
    info_struct("VariantInfo", f)
}

/// True if the attribute `name` is present, bare or with arguments.
fn has_attr(attrs: &[rask_ast::attr::Attr], name: &str) -> bool {
    rask_ast::attr::has(attrs, name)
}

/// The key `@rename("...")` gives the field, if it has one.
fn rename_of(attrs: &[rask_ast::attr::Attr]) -> Option<String> {
    rask_ast::attr::find(attrs, "rename")?.positional()?.string()
}
